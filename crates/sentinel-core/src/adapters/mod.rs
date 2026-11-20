//! Driven adapters connecting sentinel-core to external technologies (XDR, RPC, Network).

pub mod auth;
pub mod failover_rpc;
pub mod key_expr;
pub mod soroban_rpc;
pub mod xdr_keys;

pub use auth::{
    footprint_to_xdr_base64, parse_ledger_footprint, read_only_footprint, split_footprint_keys,
};
pub use failover_rpc::{EndpointHealthSnapshot, FailoverConfig, MultiRpcFailoverReader};
pub use key_expr::{parse_key_expr, MAX_KEY_EXPR_DEPTH, MAX_KEY_EXPR_LEN};
pub use soroban_rpc::{
    validate_rpc_url, RetryPolicy, SorobanRpcClient, DEFAULT_BASE_RETRY_DELAY_MS,
    DEFAULT_MAX_RETRIES, MAX_EVENTS_PER_PAGE, MAX_EVENT_PAGES, MAX_KEYS_PER_REQUEST,
    MAX_RPC_RESPONSE_BYTES,
};
pub use xdr_keys::{
    derive_contract_code_key, derive_contract_instance_key, derive_persistent_key_from_scval,
    derive_persistent_symbol_key, derive_persistent_xdr_key, derive_sac_allowance_key,
    derive_sac_balance_key, extract_instance_storage_breakdown, extract_sac_asset_info,
    extract_wasm_hash_from_instance_entry, parse_contract_id, parse_sc_address, safe_xdr_limits,
    summarize_scval, XDR_MAX_BYTE_LENGTH, XDR_MAX_RECURSION_DEPTH,
};

#[cfg(feature = "fuzzing")]
pub use soroban_rpc::{
    fuzz_parse_get_latest_ledger_response, fuzz_parse_get_ledger_entries_response,
};
