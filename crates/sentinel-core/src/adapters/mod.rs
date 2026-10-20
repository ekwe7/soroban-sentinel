//! Driven adapters connecting sentinel-core to external technologies (XDR, RPC, Network).

pub mod xdr_keys;

pub use xdr_keys::{
    derive_contract_code_key, derive_contract_instance_key, derive_persistent_key_from_scval,
    derive_persistent_symbol_key, derive_persistent_xdr_key, extract_wasm_hash_from_instance_entry,
    parse_contract_id,
};
