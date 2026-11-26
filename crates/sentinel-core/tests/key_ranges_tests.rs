use sentinel_core::{derive_key_prefix_range, parse_contract_id};
use stellar_xdr::curr::{
    ContractDataDurability, Hash, LedgerKey, LedgerKeyContractData, Limits, ScAddress, ScSymbol,
    ScVal, WriteXdr,
};

const CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

fn symbol_key(contract: &str, symbol: &str) -> Vec<u8> {
    let contract_id = parse_contract_id(contract).unwrap();
    LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::Symbol(ScSymbol(symbol.try_into().unwrap())),
        durability: ContractDataDurability::Persistent,
    })
    .to_xdr(Limits::none())
    .unwrap()
}

#[test]
fn canonical_xdr_symbol_key_is_inside_its_range() {
    let prefix = "user_1";
    let (lower, upper) = derive_key_prefix_range(CONTRACT, prefix);
    let generated_key = symbol_key(CONTRACT, prefix);

    assert_eq!(generated_key, lower);
    assert!(lower < upper);
}

#[test]
fn range_excludes_a_different_symbol_and_contract() {
    let (lower, upper) = derive_key_prefix_range(CONTRACT, "user_1");
    let different_symbol = symbol_key(CONTRACT, "user_2");
    let longer_symbol = symbol_key(CONTRACT, "user_10");

    assert!(
        !(lower.as_slice() <= different_symbol.as_slice()
            && different_symbol.as_slice() < upper.as_slice())
    );
    assert!(
        !(lower.as_slice() <= longer_symbol.as_slice()
            && longer_symbol.as_slice() < upper.as_slice())
    );

    let other_contract = stellar_strkey::Contract([1; 32]).to_string();
    let different_contract = symbol_key(&other_contract, "user_1");
    let (other_lower, _) = derive_key_prefix_range(&other_contract, "user_1");
    assert_ne!(lower, other_lower);
    assert!(
        !(lower.as_slice() <= different_contract.as_slice()
            && different_contract.as_slice() < upper.as_slice())
    );
}
