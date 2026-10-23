//! Driven adapters connecting sentinel-core to external technologies (XDR, RPC, Network).

pub mod soroban_rpc;
pub mod xdr_keys;

pub use soroban_rpc::{
    validate_rpc_url, RetryPolicy, SorobanRpcClient, DEFAULT_BASE_RETRY_DELAY_MS,
    DEFAULT_MAX_RETRIES, MAX_KEYS_PER_REQUEST, MAX_RPC_RESPONSE_BYTES,
};
pub use xdr_keys::{
    derive_contract_code_key, derive_contract_instance_key, derive_persistent_key_from_scval,
    derive_persistent_symbol_key, derive_persistent_xdr_key, extract_wasm_hash_from_instance_entry,
    parse_contract_id, safe_xdr_limits, XDR_MAX_BYTE_LENGTH, XDR_MAX_RECURSION_DEPTH,
};
