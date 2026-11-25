use sentinel_core::adapters::{
    derive_sac_allowance_key, derive_sac_balance_key, parse_key_expr, parse_sc_address,
};
use sentinel_core::SentinelError;
use stellar_strkey::{
    ed25519::{PrivateKey, PublicKey as Ed25519PublicKey},
    Contract, HashX, PreAuthTx,
};
use stellar_xdr::curr::{
    AccountId, ContractDataDurability, Hash, LedgerKey, Limits, PublicKey, ReadXdr, ScAddress,
    ScVal, Uint256,
};

const ZERO_ACCOUNT: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const ZERO_CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";

fn account_address(bytes: [u8; 32]) -> ScAddress {
    ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(bytes))))
}

fn assert_invalid(input: &str) -> SentinelError {
    let error = parse_sc_address(input).expect_err("address should be rejected");
    assert!(
        matches!(error, SentinelError::InvalidContractAddress(_)),
        "unexpected error for '{}': {}",
        input,
        error
    );
    error
}

#[test]
fn parses_g_prefixed_account_strkey() {
    let expected = account_address([0x11; 32]);
    let strkey = Ed25519PublicKey([0x11; 32]).to_string();
    assert!(strkey.starts_with('G'));
    assert_eq!(parse_sc_address(&strkey).unwrap(), expected);

    assert_eq!(
        parse_sc_address(ZERO_ACCOUNT).unwrap(),
        account_address([0; 32])
    );
    assert_eq!(Ed25519PublicKey([0; 32]).to_string(), ZERO_ACCOUNT);
}

#[test]
fn parses_c_prefixed_contract_strkey() {
    let expected = ScAddress::Contract(Hash([0xAB; 32]));
    let strkey = Contract([0xAB; 32]).to_string();
    assert!(strkey.starts_with('C'));
    assert_eq!(parse_sc_address(&strkey).unwrap(), expected);

    assert_eq!(
        parse_sc_address(ZERO_CONTRACT).unwrap(),
        ScAddress::Contract(Hash([0; 32]))
    );
    assert_eq!(Contract([0; 32]).to_string(), ZERO_CONTRACT);
}

#[test]
fn parses_known_network_contract_strkey() {
    let strkey = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
    let parsed = Contract::from_string(strkey).expect("known contract strkey should decode");
    assert_eq!(
        parse_sc_address(strkey).unwrap(),
        ScAddress::Contract(Hash(parsed.0))
    );
}

#[test]
fn trims_surrounding_whitespace() {
    let account = Ed25519PublicKey([1; 32]).to_string();
    let contract = Contract([2; 32]).to_string();

    assert_eq!(
        parse_sc_address(&format!("  {}\n", account)).unwrap(),
        account_address([1; 32])
    );
    assert_eq!(
        parse_sc_address(&format!("\t{} ", contract)).unwrap(),
        ScAddress::Contract(Hash([2; 32]))
    );
}

#[test]
fn rejects_empty_and_whitespace_only_inputs() {
    for input in ["", " ", "   ", "\t\n "] {
        let error = assert_invalid(input);
        assert!(error.to_string().contains("cannot be empty"));
    }
}

#[test]
fn rejects_invalid_checksums() {
    let account = Ed25519PublicKey([3; 32]).to_string();
    let contract = Contract([4; 32]).to_string();

    let corrupted_account = corrupt_last_char(&account);
    let error = assert_invalid(&corrupted_account);
    assert!(error.to_string().contains("Invalid account strkey"));

    let corrupted_contract = corrupt_last_char(&contract);
    let error = assert_invalid(&corrupted_contract);
    assert!(error.to_string().contains("Invalid contract strkey"));
}

#[test]
fn rejects_malformed_lengths() {
    let account = Ed25519PublicKey([5; 32]).to_string();

    for input in [
        "G",
        "C",
        "G1234",
        &account[..account.len() - 1],
        &format!("{}A", account),
    ] {
        assert_invalid(input);
    }
}

#[test]
fn rejects_embedded_whitespace() {
    let account = Ed25519PublicKey([6; 32]).to_string();
    let mut malformed = account.clone();
    malformed.insert(10, ' ');
    assert_invalid(&malformed);
}

#[test]
fn rejects_unknown_or_unsupported_prefixes() {
    let candidates = [
        PrivateKey([7; 32]).to_string(),
        HashX([8; 32]).to_string(),
        PreAuthTx([9; 32]).to_string(),
        format!("Z{}", "A".repeat(55)),
        "not-a-strkey".to_string(),
        format!("é{}", "A".repeat(55)),
    ];

    for candidate in candidates {
        let error = assert_invalid(&candidate);
        assert!(
            error.to_string().contains("Unsupported strkey prefix"),
            "unexpected diagnostic for '{}': {}",
            candidate,
            error
        );
    }
}

#[test]
fn rejects_account_payload_with_contract_prefix() {
    let account = Ed25519PublicKey([10; 32]).to_string();
    let mut swapped = account;
    swapped.replace_range(0..1, "C");

    let error = assert_invalid(&swapped);
    assert!(error.to_string().contains("Invalid contract strkey"));
}

#[test]
fn round_trips_parsed_addresses_to_strkeys() {
    let account_bytes = [11; 32];
    let account_strkey = Ed25519PublicKey(account_bytes).to_string();
    match parse_sc_address(&account_strkey).unwrap() {
        ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(raw)))) => {
            assert_eq!(Ed25519PublicKey(raw).to_string(), account_strkey);
        }
        other => panic!("expected account address, got {:?}", other),
    }

    let contract_bytes = [12; 32];
    let contract_strkey = Contract(contract_bytes).to_string();
    match parse_sc_address(&contract_strkey).unwrap() {
        ScAddress::Contract(Hash(raw)) => {
            assert_eq!(Contract(raw).to_string(), contract_strkey);
        }
        other => panic!("expected contract address, got {:?}", other),
    }
}

#[test]
fn derives_account_keyed_sac_balance_ledger_key() {
    let contract_id = [13; 32];
    let holder_bytes = [14; 32];
    let holder = Ed25519PublicKey(holder_bytes).to_string();

    let key_base64 = derive_sac_balance_key(&contract_id, &holder).unwrap();
    let ledger_key = LedgerKey::from_xdr_base64(&key_base64, Limits::none()).unwrap();

    match ledger_key {
        LedgerKey::ContractData(data) => {
            assert_eq!(data.durability, ContractDataDurability::Persistent);
            assert_eq!(data.contract, ScAddress::Contract(Hash(contract_id)));
            match data.key {
                ScVal::Vec(Some(vec)) => {
                    assert_eq!(vec.0.len(), 2);
                    assert_eq!(vec.0[0], ScVal::Symbol("Balance".try_into().unwrap()));
                    assert_eq!(vec.0[1], ScVal::Address(account_address(holder_bytes)));
                }
                other => panic!("expected balance key vector, got {:?}", other),
            }
        }
        other => panic!("expected contract data ledger key, got {:?}", other),
    }
}

#[test]
fn derives_mixed_variant_sac_allowance_ledger_key() {
    let contract_id = [15; 32];
    let from_bytes = [16; 32];
    let spender_bytes = [17; 32];
    let from = Ed25519PublicKey(from_bytes).to_string();
    let spender = Contract(spender_bytes).to_string();

    let key_base64 = derive_sac_allowance_key(&contract_id, &from, &spender).unwrap();
    let ledger_key = LedgerKey::from_xdr_base64(&key_base64, Limits::none()).unwrap();

    match ledger_key {
        LedgerKey::ContractData(data) => {
            assert_eq!(data.durability, ContractDataDurability::Persistent);
            match data.key {
                ScVal::Vec(Some(vec)) => match &vec.0[1] {
                    ScVal::Map(map) => {
                        let entries = map.as_ref().expect("allowance map should exist");
                        assert_eq!(entries.len(), 2);
                        assert_eq!(entries[0].val, ScVal::Address(account_address(from_bytes)));
                        assert_eq!(
                            entries[1].val,
                            ScVal::Address(ScAddress::Contract(Hash(spender_bytes)))
                        );
                    }
                    other => panic!("expected allowance map, got {:?}", other),
                },
                other => panic!("expected allowance key vector, got {:?}", other),
            }
        }
        other => panic!("expected contract data ledger key, got {:?}", other),
    }
}

#[test]
fn key_expression_address_accepts_account_strkey() {
    let account_bytes = [18; 32];
    let strkey = Ed25519PublicKey(account_bytes).to_string();
    assert_eq!(
        parse_key_expr(&format!("addr:{}", strkey)).unwrap(),
        ScVal::Address(account_address(account_bytes))
    );
}

fn corrupt_last_char(strkey: &str) -> String {
    let mut chars: Vec<char> = strkey.chars().collect();
    let last = chars.last_mut().expect("strkey should not be empty");
    *last = if *last == 'A' { 'B' } else { 'A' };
    chars.into_iter().collect()
}
