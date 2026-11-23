//! Typed key expression parser for composite Soroban storage keys.
//!
//! Converts human-readable key expressions (e.g. `sym:Balance`, `vec[sym:Balance, addr:G...]`)
//! into deterministic Soroban `ScVal` types for storage querying.

use stellar_strkey::Strkey;
use stellar_xdr::curr::{
    AccountId, Hash, PublicKey, ScAddress, ScBytes, ScString, ScSymbol, ScVal, ScVec, UInt128Parts,
    Uint256,
};

use crate::error::{SentinelError, SentinelResult};

/// Maximum allowed nesting depth for composite vectors to prevent stack exhaustion.
pub const MAX_KEY_EXPR_DEPTH: usize = 16;

/// Maximum allowed character length of a key expression string to prevent unbounded parsing.
pub const MAX_KEY_EXPR_LEN: usize = 1024;

/// Parses a typed key expression into a Soroban [`ScVal`].
///
/// # Supported Grammar:
/// - `sym:<name>`: Symbol (up to 32 ASCII alphanumeric/underscore characters)
/// - `str:<text>`: String
/// - `u32:<num>`: 32-bit unsigned integer
/// - `i32:<num>`: 32-bit signed integer
/// - `u64:<num>`: 64-bit unsigned integer
/// - `i64:<num>`: 64-bit signed integer
/// - `u128:<num>`: 128-bit unsigned integer
/// - `bool:<true|false>`: Boolean value
/// - `addr:<strkey>`: Account (`G...`) or Contract (`C...`) Strkey address
/// - `bytes:<hex>`: Hex-encoded raw byte array
/// - `vec[item1, item2, ...]`: Nested composite vector of expressions
#[tracing::instrument(skip(input), fields(expr.len = input.len()))]
pub fn parse_key_expr(input: &str) -> SentinelResult<ScVal> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(SentinelError::InvalidXdrKey(
            "Empty key expression".to_string(),
        ));
    }
    if trimmed.len() > MAX_KEY_EXPR_LEN {
        return Err(SentinelError::InvalidXdrKey(format!(
            "Key expression length ({} chars) exceeds maximum allowed length of {} chars",
            trimmed.len(),
            MAX_KEY_EXPR_LEN
        )));
    }

    let mut parser = KeyExprParser::new(trimmed);
    let scval = parser.parse_expression(0)?;
    parser.skip_whitespace();
    if !parser.is_eof() {
        let remainder = parser.remaining();
        return Err(SentinelError::InvalidXdrKey(format!(
            "Unexpected trailing input '{}' in key expression '{}'",
            remainder, input
        )));
    }
    Ok(scval)
}

struct KeyExprParser {
    chars: Vec<char>,
    pos: usize,
}

impl KeyExprParser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        if self.pos < self.chars.len() {
            let ch = self.chars[self.pos];
            self.pos += 1;
            Some(ch)
        } else {
            None
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn remaining(&self) -> String {
        self.chars[self.pos..].iter().collect()
    }

    fn parse_expression(&mut self, depth: usize) -> SentinelResult<ScVal> {
        if depth > MAX_KEY_EXPR_DEPTH {
            return Err(SentinelError::InvalidXdrKey(format!(
                "Key expression exceeds maximum nesting depth of {}",
                MAX_KEY_EXPR_DEPTH
            )));
        }

        self.skip_whitespace();
        if self.is_eof() {
            return Err(SentinelError::InvalidXdrKey(
                "Unexpected end of input in key expression".to_string(),
            ));
        }

        // Check for vec[...]
        if self.starts_with_ignore_case("vec[") {
            self.pos += 4; // consume "vec["
            return self.parse_vec(depth);
        }

        // Find prefix up to ':'
        let start_pos = self.pos;
        let mut found_colon = false;
        while let Some(ch) = self.peek() {
            if ch == ':' {
                found_colon = true;
                break;
            }
            if ch == ',' || ch == ']' || ch.is_whitespace() {
                break;
            }
            self.advance();
        }

        if !found_colon {
            let token: String = self.chars[start_pos..self.pos].iter().collect();
            return Err(SentinelError::InvalidXdrKey(format!(
                "Missing ':' delimiter after type prefix in token '{}'",
                token
            )));
        }

        let prefix: String = self.chars[start_pos..self.pos].iter().collect();
        self.advance(); // consume ':'

        // Read literal value until next delimiter (',', ']', whitespace, or EOF)
        let val_start = self.pos;
        while let Some(ch) = self.peek() {
            if ch == ',' || ch == ']' || ch.is_whitespace() {
                break;
            }
            self.advance();
        }

        let literal: String = self.chars[val_start..self.pos].iter().collect();
        let trimmed_literal = literal.trim();

        if trimmed_literal.is_empty() {
            return Err(SentinelError::InvalidXdrKey(format!(
                "Empty value for prefix '{}:'",
                prefix
            )));
        }

        self.build_literal_scval(&prefix, trimmed_literal)
    }

    fn starts_with_ignore_case(&self, prefix: &str) -> bool {
        let prefix_chars: Vec<char> = prefix.chars().collect();
        if self.pos + prefix_chars.len() > self.chars.len() {
            return false;
        }
        for (i, &expected) in prefix_chars.iter().enumerate() {
            if !self.chars[self.pos + i].eq_ignore_ascii_case(&expected) {
                return false;
            }
        }
        true
    }

    fn parse_vec(&mut self, depth: usize) -> SentinelResult<ScVal> {
        let mut elements = Vec::new();

        loop {
            self.skip_whitespace();
            if self.is_eof() {
                return Err(SentinelError::InvalidXdrKey(
                    "Unbalanced brackets in vector expression: missing ']'".to_string(),
                ));
            }

            if self.peek() == Some(']') {
                self.advance(); // consume ']'
                break;
            }

            let element = self.parse_expression(depth + 1)?;
            elements.push(element);

            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.advance(); // consume ','
                                    // Allow trailing comma before ']'
                    self.skip_whitespace();
                    if self.peek() == Some(']') {
                        self.advance();
                        break;
                    }
                }
                Some(']') => {
                    self.advance(); // consume ']'
                    break;
                }
                Some(other) => {
                    return Err(SentinelError::InvalidXdrKey(format!(
                        "Expected ',' or ']' in vector expression, found '{}'",
                        other
                    )));
                }
                None => {
                    return Err(SentinelError::InvalidXdrKey(
                        "Unbalanced brackets in vector expression: missing ']'".to_string(),
                    ));
                }
            }
        }

        let sc_vec: ScVec = elements.try_into().map_err(|e| {
            SentinelError::InvalidXdrKey(format!("Failed to create ScVec from elements: {}", e))
        })?;

        Ok(ScVal::Vec(Some(sc_vec)))
    }

    fn build_literal_scval(&self, prefix: &str, literal: &str) -> SentinelResult<ScVal> {
        match prefix {
            "sym" => {
                if literal.len() > 32 {
                    return Err(SentinelError::InvalidXdrKey(format!(
                        "Symbol '{}' exceeds maximum length of 32 characters (length: {})",
                        literal,
                        literal.len()
                    )));
                }
                for c in literal.chars() {
                    if !c.is_ascii_alphanumeric() && c != '_' {
                        return Err(SentinelError::InvalidXdrKey(format!(
                            "Invalid character '{}' in symbol '{}'. Symbols may only contain ASCII alphanumeric characters and underscores",
                            c, literal
                        )));
                    }
                }
                let sym = ScSymbol(literal.try_into().map_err(|e| {
                    SentinelError::InvalidXdrKey(format!("Invalid symbol '{}': {}", literal, e))
                })?);
                Ok(ScVal::Symbol(sym))
            }
            "str" => {
                let s = ScString(literal.try_into().map_err(|e| {
                    SentinelError::InvalidXdrKey(format!("Invalid string '{}': {}", literal, e))
                })?);
                Ok(ScVal::String(s))
            }
            "u32" => {
                let n = literal.parse::<u32>().map_err(|e| {
                    SentinelError::InvalidXdrKey(format!(
                        "Failed to parse u32 literal '{}': {}",
                        literal, e
                    ))
                })?;
                Ok(ScVal::U32(n))
            }
            "i32" => {
                let n = literal.parse::<i32>().map_err(|e| {
                    SentinelError::InvalidXdrKey(format!(
                        "Failed to parse i32 literal '{}': {}",
                        literal, e
                    ))
                })?;
                Ok(ScVal::I32(n))
            }
            "u64" => {
                let n = literal.parse::<u64>().map_err(|e| {
                    SentinelError::InvalidXdrKey(format!(
                        "Failed to parse u64 literal '{}': {}",
                        literal, e
                    ))
                })?;
                Ok(ScVal::U64(n))
            }
            "i64" => {
                let n = literal.parse::<i64>().map_err(|e| {
                    SentinelError::InvalidXdrKey(format!(
                        "Failed to parse i64 literal '{}': {}",
                        literal, e
                    ))
                })?;
                Ok(ScVal::I64(n))
            }
            "u128" => {
                let n = literal.parse::<u128>().map_err(|e| {
                    SentinelError::InvalidXdrKey(format!(
                        "Failed to parse u128 literal '{}': {}",
                        literal, e
                    ))
                })?;
                let hi = (n >> 64) as u64;
                let lo = n as u64;
                Ok(ScVal::U128(UInt128Parts { hi, lo }))
            }
            "bool" => {
                let b = match literal.to_ascii_lowercase().as_str() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(SentinelError::InvalidXdrKey(format!(
                            "Invalid boolean literal '{}', expected 'true' or 'false'",
                            literal
                        )));
                    }
                };
                Ok(ScVal::Bool(b))
            }
            "addr" => {
                let strkey = Strkey::from_string(literal).map_err(|e| {
                    SentinelError::InvalidXdrKey(format!(
                        "Invalid strkey address '{}': {}",
                        literal, e
                    ))
                })?;
                match strkey {
                    Strkey::PublicKeyEd25519(pk) => {
                        let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));
                        Ok(ScVal::Address(ScAddress::Account(account_id)))
                    }
                    Strkey::Contract(c) => Ok(ScVal::Address(ScAddress::Contract(Hash(c.0)))),
                    other => Err(SentinelError::InvalidXdrKey(format!(
                        "Unsupported strkey type for address '{}': {:?}",
                        literal, other
                    ))),
                }
            }
            "bytes" => {
                let raw_bytes = decode_hex(literal).map_err(|e| {
                    SentinelError::InvalidXdrKey(format!(
                        "Invalid hex bytes literal '{}': {}",
                        literal, e
                    ))
                })?;
                let sc_bytes = ScBytes(raw_bytes.try_into().map_err(|e| {
                    SentinelError::InvalidXdrKey(format!(
                        "Invalid ScBytes from hex '{}': {}",
                        literal, e
                    ))
                })?);
                Ok(ScVal::Bytes(sc_bytes))
            }
            unknown => Err(SentinelError::InvalidXdrKey(format!(
                "Unknown key expression prefix '{}' for literal '{}'",
                unknown, literal
            ))),
        }
    }
}

/// Simple hex string decoder without third-party dependencies.
fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    let clean = hex.strip_prefix("0x").unwrap_or(hex).trim();
    if !clean.len().is_multiple_of(2) {
        return Err("Hex string must have an even number of characters".to_string());
    }
    let mut bytes = Vec::with_capacity(clean.len() / 2);
    let chars: Vec<char> = clean.chars().collect();
    for i in (0..chars.len()).step_by(2) {
        let hi = chars[i]
            .to_digit(16)
            .ok_or_else(|| format!("Invalid hex digit '{}'", chars[i]))?;
        let lo = chars[i + 1]
            .to_digit(16)
            .ok_or_else(|| format!("Invalid hex digit '{}'", chars[i + 1]))?;
        bytes.push(((hi << 4) | lo) as u8);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sym_literal() {
        let scval = parse_key_expr("sym:Balance").expect("should parse symbol");
        let expected = ScVal::Symbol(ScSymbol("Balance".try_into().unwrap()));
        assert_eq!(scval, expected);
    }

    #[test]
    fn test_parse_sym_max_length_and_overflow() {
        let exact_32 = "a".repeat(32);
        let scval =
            parse_key_expr(&format!("sym:{}", exact_32)).expect("32 char symbol should pass");
        assert_eq!(
            scval,
            ScVal::Symbol(ScSymbol(exact_32.as_str().try_into().unwrap()))
        );

        let over_32 = "a".repeat(33);
        let err = parse_key_expr(&format!("sym:{}", over_32)).unwrap_err();
        assert!(matches!(err, SentinelError::InvalidXdrKey(_)));
        assert!(err.to_string().contains("exceeds maximum length of 32"));
    }

    #[test]
    fn test_parse_str_literal() {
        let scval = parse_key_expr("str:Hello_World").expect("should parse string");
        let expected = ScVal::String(ScString("Hello_World".try_into().unwrap()));
        assert_eq!(scval, expected);
    }

    #[test]
    fn test_parse_numeric_literals() {
        // u32
        assert_eq!(parse_key_expr("u32:42").unwrap(), ScVal::U32(42));
        // i32
        assert_eq!(parse_key_expr("i32:-100").unwrap(), ScVal::I32(-100));
        // u64
        assert_eq!(
            parse_key_expr("u64:9999999999").unwrap(),
            ScVal::U64(9999999999)
        );
        // i64
        assert_eq!(
            parse_key_expr("i64:-9999999999").unwrap(),
            ScVal::I64(-9999999999)
        );
        // u128
        let val_128 = 340282366920938463463374607431768211455u128; // u128::MAX
        assert_eq!(
            parse_key_expr(&format!("u128:{}", val_128)).unwrap(),
            ScVal::U128(UInt128Parts {
                hi: u64::MAX,
                lo: u64::MAX
            })
        );
        // overflow rejected
        let err = parse_key_expr("u32:4294967296").unwrap_err();
        assert!(matches!(err, SentinelError::InvalidXdrKey(_)));
        assert!(err.to_string().contains("4294967296"));
    }

    #[test]
    fn test_parse_bool_literal() {
        assert_eq!(parse_key_expr("bool:true").unwrap(), ScVal::Bool(true));
        assert_eq!(parse_key_expr("bool:false").unwrap(), ScVal::Bool(false));
        assert!(parse_key_expr("bool:not_a_bool").is_err());
    }

    #[test]
    fn test_parse_addr_account_and_contract() {
        let account_bytes = [1u8; 32];
        let account_strkey = stellar_strkey::ed25519::PublicKey(account_bytes).to_string();
        let scval_account = parse_key_expr(&format!("addr:{}", account_strkey)).unwrap();
        let expected_account = ScVal::Address(ScAddress::Account(AccountId(
            PublicKey::PublicKeyTypeEd25519(Uint256(account_bytes)),
        )));
        assert_eq!(scval_account, expected_account);

        let contract_bytes = [2u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();
        let scval_contract = parse_key_expr(&format!("addr:{}", contract_strkey)).unwrap();
        let expected_contract = ScVal::Address(ScAddress::Contract(Hash(contract_bytes)));
        assert_eq!(scval_contract, expected_contract);

        let err = parse_key_expr("addr:G_INVALID_ACCOUNT").unwrap_err();
        assert!(matches!(err, SentinelError::InvalidXdrKey(_)));
        assert!(err.to_string().contains("G_INVALID_ACCOUNT"));
    }

    #[test]
    fn test_parse_bytes_hex() {
        let scval = parse_key_expr("bytes:deadbeef").unwrap();
        let expected = ScVal::Bytes(ScBytes(vec![0xde, 0xad, 0xbe, 0xef].try_into().unwrap()));
        assert_eq!(scval, expected);

        // 0x prefix supported
        let scval_0x = parse_key_expr("bytes:0xDEADBEEF").unwrap();
        assert_eq!(scval_0x, expected);

        // Odd length rejected
        assert!(parse_key_expr("bytes:abc").is_err());
        // Non-hex rejected
        assert!(parse_key_expr("bytes:zz").is_err());
    }

    #[test]
    fn test_parse_vec_with_nesting() {
        let contract_bytes = [3u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let expr = format!("vec[sym:Balance, addr:{}]", contract_strkey);
        let scval = parse_key_expr(&expr).expect("should parse composite vector");

        let expected_elements: Vec<ScVal> = vec![
            ScVal::Symbol(ScSymbol("Balance".try_into().unwrap())),
            ScVal::Address(ScAddress::Contract(Hash(contract_bytes))),
        ];
        let expected = ScVal::Vec(Some(ScVec(expected_elements.try_into().unwrap())));
        assert_eq!(scval, expected);

        // Nested vector: vec[sym:Data, vec[u32:1, u32:2]]
        let nested_expr = "vec[sym:Data, vec[u32:1, u32:2]]";
        let nested_scval = parse_key_expr(nested_expr).expect("nested vector should parse");
        let inner_vec: ScVec = vec![ScVal::U32(1), ScVal::U32(2)].try_into().unwrap();
        let expected_nested: ScVec = vec![
            ScVal::Symbol(ScSymbol("Data".try_into().unwrap())),
            ScVal::Vec(Some(inner_vec)),
        ]
        .try_into()
        .unwrap();
        assert_eq!(nested_scval, ScVal::Vec(Some(expected_nested)));
    }

    #[test]
    fn test_malformed_key_expr_errors() {
        // Unknown prefix
        let err = parse_key_expr("unknown:123").unwrap_err();
        assert!(err
            .to_string()
            .contains("Unknown key expression prefix 'unknown'"));

        // Missing colon
        let err = parse_key_expr("symBalance").unwrap_err();
        assert!(err.to_string().contains("Missing ':' delimiter"));

        // Unbalanced brackets
        let err = parse_key_expr("vec[sym:Balance").unwrap_err();
        assert!(err.to_string().contains("Unbalanced brackets"));

        // Trailing input
        let err = parse_key_expr("sym:Balance trailing_junk").unwrap_err();
        assert!(err.to_string().contains("Unexpected trailing input"));
    }

    #[test]
    fn test_nesting_depth_limit() {
        let mut deep_expr = "u32:1".to_string();
        for _ in 0..20 {
            deep_expr = format!("vec[{}]", deep_expr);
        }
        let err = parse_key_expr(&deep_expr).unwrap_err();
        assert!(err.to_string().contains("maximum nesting depth"));
    }

    #[test]
    fn test_input_length_limit() {
        let huge_expr = format!("str:{}", "x".repeat(MAX_KEY_EXPR_LEN + 1));
        let err = parse_key_expr(&huge_expr).unwrap_err();
        assert!(err.to_string().contains("exceeds maximum allowed length"));
    }
}
