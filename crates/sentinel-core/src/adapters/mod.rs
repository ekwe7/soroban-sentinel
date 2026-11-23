//! Driven adapters connecting sentinel-core to external technologies (XDR, RPC, Network).

pub mod auth;
pub mod circuit_breaker;
pub mod failover_rpc;
pub mod fee_bump_envelope;
pub mod key_expr;
pub mod simulation;
pub mod snapshot;
pub mod soroban_rpc;
pub mod wasm_sections;
pub mod webhook;
pub mod xdr_keys;

pub use auth::{
    footprint_to_xdr_base64, parse_ledger_footprint, read_only_footprint, split_footprint_keys,
};
pub use circuit_breaker::{CircuitBreakerConfig, CircuitBreakerReader, CircuitState};
pub use failover_rpc::{EndpointHealthSnapshot, FailoverConfig, MultiRpcFailoverReader};
pub use fee_bump_envelope::{build_fee_bump_envelope, fee_bump_envelope_to_xdr_base64};
pub use key_expr::{parse_key_expr, MAX_KEY_EXPR_DEPTH, MAX_KEY_EXPR_LEN};
pub use simulation::{build_extend_footprint_ttl_envelope, envelope_to_xdr_base64};
pub use snapshot::{
    ReplayReader, SentinelSnapshot, SnapshotData, SnapshotHeader, SnapshotLedgerReader,
    SNAPSHOT_FORMAT_VERSION,
};
pub use soroban_rpc::{
    validate_rpc_url, RetryPolicy, SorobanRpcClient, DEFAULT_BASE_RETRY_DELAY_MS,
    DEFAULT_MAX_RETRIES, MAX_BATCH_REQUESTS, MAX_KEYS_PER_REQUEST, MAX_RPC_RESPONSE_BYTES,
};
pub use wasm_sections::{analyze_wasm, MAX_WASM_ANALYSIS_BYTES};
pub use webhook::WebhookAlertNotifier;
pub use xdr_keys::{
    derive_contract_code_key, derive_contract_data_entry_size_config_key,
    derive_contract_instance_key, derive_persistent_key_from_scval, derive_persistent_symbol_key,
    derive_persistent_xdr_key, derive_sac_allowance_key, derive_sac_balance_key,
    derive_state_archival_config_key, derive_temporary_key_from_scval, derive_temporary_symbol_key,
    extract_instance_storage_breakdown, extract_sac_asset_info,
    extract_wasm_hash_from_instance_entry, parse_contract_id, parse_sc_address, safe_xdr_limits,
    summarize_scval, KeyPatternMatcher, XDR_MAX_BYTE_LENGTH, XDR_MAX_RECURSION_DEPTH,
};

#[cfg(feature = "fuzzing")]
pub use soroban_rpc::{
    fuzz_parse_get_latest_ledger_response, fuzz_parse_get_ledger_entries_response,
};
mod submitter;
pub mod wasm_spec;
