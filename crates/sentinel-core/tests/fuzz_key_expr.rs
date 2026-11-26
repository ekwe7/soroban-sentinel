use proptest::prelude::*;
use stellar_xdr::curr::{Limits, ReadXdr, ScVal, WriteXdr};

use sentinel_core::adapters::{parse_key_expr, MAX_KEY_EXPR_DEPTH, MAX_KEY_EXPR_LEN};

#[derive(Debug, Clone)]
enum ValidExpr {
    Symbol(String),
    String(String),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    U128(u128),
    Bool(bool),
    Bytes(Vec<u8>),
    Vec(Vec<ValidExpr>),
}

impl ValidExpr {
    fn render(&self) -> String {
        match self {
            Self::Symbol(value) => format!("sym:{value}"),
            Self::String(value) => format!("str:{value}"),
            Self::U32(value) => format!("u32:{value}"),
            Self::I32(value) => format!("i32:{value}"),
            Self::U64(value) => format!("u64:{value}"),
            Self::I64(value) => format!("i64:{value}"),
            Self::U128(value) => format!("u128:{value}"),
            Self::Bool(value) => format!("bool:{value}"),
            Self::Bytes(value) => {
                let hex = value
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                format!("bytes:{hex}")
            }
            Self::Vec(values) => format!(
                "vec[{}]",
                values
                    .iter()
                    .map(Self::render)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

fn safe_symbol() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_]{1,32}".prop_map(|value| value)
}

fn valid_expr_strategy() -> impl Strategy<Value = ValidExpr> {
    let leaf = prop_oneof![
        safe_symbol().prop_map(ValidExpr::Symbol),
        "[a-zA-Z0-9_]{1,24}".prop_map(ValidExpr::String),
        any::<u32>().prop_map(ValidExpr::U32),
        any::<i32>().prop_map(ValidExpr::I32),
        any::<u64>().prop_map(ValidExpr::U64),
        any::<i64>().prop_map(ValidExpr::I64),
        any::<u128>().prop_map(ValidExpr::U128),
        any::<bool>().prop_map(ValidExpr::Bool),
        prop::collection::vec(any::<u8>(), 1..=16).prop_map(ValidExpr::Bytes),
    ];
    leaf.prop_recursive(3, 80, 3, |inner| {
        prop::collection::vec(inner, 0..=2).prop_map(ValidExpr::Vec)
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        .. ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_strings_never_panic(input in prop::collection::vec(any::<char>(), 0..=2048).prop_map(|chars| chars.into_iter().collect::<String>())) {
        let _result = parse_key_expr(&input);
    }

    #[test]
    fn generated_malformed_expressions_return_errors(
        value in "[a-zA-Z0-9_]{0,40}",
        kind in 0u8..5,
    ) {
        let input = match kind {
            0 => format!("vec[u32:{value}"),
            1 => format!("sym:{value}-"),
            2 => format!("bytes:{value}g"),
            3 => format!("u32:{value}x"),
            _ => format!("unknown:{value}"),
        };
        prop_assert!(parse_key_expr(&input).is_err());
    }

    #[test]
    fn generated_valid_expressions_round_trip_deterministically(expression in valid_expr_strategy()) {
        let source = expression.render();
        prop_assert!(source.len() <= MAX_KEY_EXPR_LEN);

        let parsed = parse_key_expr(&source)
            .map_err(|error| TestCaseError::fail(format!("generated valid expression failed: {error}; input={source}")))?;
        let xdr_once = parsed.to_xdr_base64(Limits::none())
            .map_err(|error| TestCaseError::fail(format!("XDR encoding failed: {error}")))?;
        let decoded = ScVal::from_xdr_base64(&xdr_once, Limits::none())
            .map_err(|error| TestCaseError::fail(format!("XDR decoding failed: {error}")))?;
        let xdr_twice = decoded.to_xdr_base64(Limits::none())
            .map_err(|error| TestCaseError::fail(format!("second XDR encoding failed: {error}")))?;

        prop_assert_eq!(&decoded, &parsed);
        prop_assert_eq!(xdr_once, xdr_twice);
        prop_assert_eq!(parse_key_expr(&source).unwrap(), parsed);
    }

    #[test]
    fn deeply_nested_vectors_return_error_without_exceeding_parser_limits(extra_depth in 1usize..=64) {
        let depth = MAX_KEY_EXPR_DEPTH.saturating_add(extra_depth);
        let expression = format!("{}u32:1{}", "vec[".repeat(depth), "]".repeat(depth));
        prop_assert!(expression.len() <= MAX_KEY_EXPR_LEN);
        let result = parse_key_expr(&expression);
        prop_assert!(result.is_err());
    }
}
