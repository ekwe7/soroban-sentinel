use stellar_strkey::ed25519::PublicKey;
use stellar_xdr::curr::LedgerKey;

use sentinel_core::adapters::{summarize_scval, KeyPatternMatcher};
use sentinel_core::domain::{KeyPatternCandidateInput, MAX_KEY_PATTERN_CANDIDATES};

fn address(index: u16) -> String {
    let mut bytes = [0u8; 32];
    bytes[0] = index as u8;
    bytes[1] = (index >> 8) as u8;
    PublicKey(bytes).to_string()
}

fn decode_summaries(keys: &[LedgerKey]) -> Vec<String> {
    keys.iter()
        .map(|key| match key {
            LedgerKey::ContractData(data) => summarize_scval(&data.key),
            other => panic!("unexpected ledger key: {other:?}"),
        })
        .collect()
}

#[test]
fn symbol_prefix_candidates_are_deterministic_and_bounded_by_known_suffixes() {
    let contract = [1u8; 32];
    let known = KeyPatternCandidateInput {
        addresses: Vec::new(),
        symbol_suffixes: vec!["_2".to_string(), "_1".to_string()],
    };

    let first = KeyPatternMatcher::expand(&contract, "sym:Balance*", &known).expect("first");
    let second = KeyPatternMatcher::expand(&contract, "sym:Balance*", &known).expect("second");
    assert_eq!(first, second);
    assert_eq!(first.len(), 2);

    let summaries = decode_summaries(&first);
    assert!(summaries.contains(&"Balance_1".to_string()));
    assert!(summaries.contains(&"Balance_2".to_string()));
}

#[test]
fn composite_pattern_expands_addresses_and_suffixes_cross_product() {
    let known = KeyPatternCandidateInput {
        addresses: vec![address(1), address(2)],
        symbol_suffixes: vec!["_1".to_string(), "_2".to_string()],
    };
    let keys = KeyPatternMatcher::expand(&[2u8; 32], "vec[sym:Balance*, addr:*]", &known)
        .expect("composite expansion");
    assert_eq!(keys.len(), 4);

    let summaries = decode_summaries(&keys);
    for suffix in ["_1", "_2"] {
        for index in [1u16, 2] {
            let expected = format!("[Balance{suffix}, {}]", address(index));
            assert!(summaries.contains(&expected), "missing {expected}");
        }
    }
}

#[test]
fn nested_composite_patterns_expand_recursively() {
    let known = KeyPatternCandidateInput {
        addresses: vec![address(3), address(4)],
        symbol_suffixes: Vec::new(),
    };
    let keys = KeyPatternMatcher::expand(&[5u8; 32], "vec[sym:Allow, vec[u32:1, addr:*]]", &known)
        .expect("nested expansion");
    assert_eq!(keys.len(), 2);

    let summaries = decode_summaries(&keys);
    assert!(summaries
        .iter()
        .any(|summary| summary == &format!("[Allow, [1, {}]]", address(3))));
    assert!(summaries
        .iter()
        .any(|summary| summary == &format!("[Allow, [1, {}]]", address(4))));
}

#[test]
fn wildcard_syntax_errors_include_byte_position() {
    let input = KeyPatternCandidateInput::default();
    let err = KeyPatternMatcher::expand(&[0u8; 32], "sym:Bal*ance", &input).unwrap_err();
    assert!(err.to_string().contains("position 7"), "{err}");

    let err = KeyPatternMatcher::expand(&[0u8; 32], "addr:G*", &input).unwrap_err();
    assert!(err.to_string().contains("position 6"), "{err}");

    let err = KeyPatternMatcher::expand(&[0u8; 32], "vec[u32:*]", &input).unwrap_err();
    assert!(err.to_string().contains("position 8"), "{err}");
}

#[test]
fn overlength_symbols_and_excessive_nesting_are_rejected() {
    let input = KeyPatternCandidateInput::default();
    let too_long = format!("sym:{}*", "a".repeat(33));
    assert!(KeyPatternMatcher::expand(&[0u8; 32], &too_long, &input).is_err());

    let mut deep = "u32:1".to_string();
    for _ in 0..18 {
        deep = format!("vec[{deep}]");
    }
    let err = KeyPatternMatcher::expand(&[0u8; 32], &deep, &input).unwrap_err();
    assert!(err.to_string().contains("maximum nesting depth"), "{err}");
}

#[test]
fn candidate_count_is_capped_deterministically() {
    let known = KeyPatternCandidateInput {
        addresses: (0..300).map(address).collect(),
        symbol_suffixes: Vec::new(),
    };
    let keys = KeyPatternMatcher::expand(&[6u8; 32], "addr:*", &known).expect("cap");
    assert_eq!(keys.len(), MAX_KEY_PATTERN_CANDIDATES);

    let reversed = KeyPatternCandidateInput {
        addresses: known.addresses.iter().rev().cloned().collect(),
        symbol_suffixes: Vec::new(),
    };
    let reversed_keys =
        KeyPatternMatcher::expand(&[6u8; 32], "addr:*", &reversed).expect("reversed cap");
    assert_eq!(keys, reversed_keys);
}
