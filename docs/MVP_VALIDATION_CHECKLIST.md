# Soroban Sentinel — MVP CLI Validation Checklist

**Target Release:** v0.1.0 (MVP)  
**Branch:** `release/mvp-contract-ttl-scanner`  
**Commit:** `67e5db9` (`fix(cli): standardize scan error exit codes`)  
**Scope:** Operator verification of the compiled CLI binary, argument parsing, security protections, formatting, and exit codes prior to pushing or publishing.  
**Validation Completed:** 2026-10-23  

---

## Safety & Non-Destructive Guarantee

All commands in this checklist are strictly **read-only**:
- **Zero On-Chain Write Operations**: Does not build or broadcast `ExtendFootprintTTLOp` or transactions.
- **Zero Signing**: Does not handle or require Stellar private keys or secret seeds.
- **Zero Cost**: Incurs zero network transaction fees.

---

## Verification Legend

- `[x] PASS (Automated)`: Verified via hermetic in-process unit/integration tests (`cargo test`).
- `[x] PASS (Live Execution)`: Verified via live CLI binary execution (`cargo run`).
- `[x] PASS (Code Inspection)`: Verified via manual code and AST inspection where fixtures are unneeded or unsafe.
- `[ ] NOT RUN`: Intentionally deferred or omitted per safety/environmental constraints.

---

## 1. CLI Startup, Version, and Help Output

### Test 1.1: Root CLI Help Display
* **Purpose**: Verify that the top-level binary executes, renders help text cleanly, and displays description and available subcommands.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- --help
  ```
* **Observed Result**: Exits with code `0`. Outputs binary description, `scan` and `help` commands, and `-h` / `-V` flags.
* **Test Coverage**: `test_cli_help_and_version_flags_return_exit_code_0` in `tests/cli_scan.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe (local CLI invocation).

### Test 1.2: Root CLI Version Flag
* **Purpose**: Verify that the version flag accurately emits the workspace version without errors.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- --version
  ```
* **Observed Result**: Exits with code `0`. Prints `sentinel 0.1.0`.
* **Test Coverage**: `test_cli_help_and_version_flags_return_exit_code_0` in `tests/cli_scan.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 1.3: Subcommand Help Display
* **Purpose**: Verify that `sentinel scan --help` documents all positional and optional arguments.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan --help
  ```
* **Observed Result**: Exits with code `0`. Displays `<CONTRACT_ID>`, `--rpc-url`, `--network`, `--format`, `--key`, `--key-xdr`, `--threshold`, and `--critical-threshold`.
* **Test Coverage**: `test_cli_help_and_version_flags_return_exit_code_0` in `tests/cli_scan.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

---

## 2. Required and Optional Arguments for `sentinel scan`

### Test 2.1: Missing Required Contract Address
* **Purpose**: Verify that invoking `sentinel scan` without the required `<CONTRACT_ID>` argument fails fast with exit code `10` (`EXIT_INVALID_ARGS`).
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan
  ```
* **Observed Result**: Exits with code `10` (`EXIT_INVALID_ARGS`). Emits Clap usage error to stderr (`error: the following required arguments were not provided: <CONTRACT_ID>`). Does not attempt network request. Does not collide with exit code `2`.
* **Test Coverage**: `test_cli_scan_missing_contract_id_returns_exit_code_10` in `tests/cli_scan.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 2.2: Malformed Contract Strkey Validation
* **Purpose**: Verify that invalid or non-contract Strkeys (e.g. invalid length, bad checksum, or account `G...` keys) are rejected at the argument boundary.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan INVALID_STRKEY_NOT_CONTRACT
  ```
* **Observed Result**: Exits with code `10` (`EXIT_INVALID_ARGS`). Emits `Error: Invalid contract address: INVALID_STRKEY_NOT_CONTRACT: the strkey is invalid`.
* **Test Coverage**: `test_cli_scan_malformed_strkey_returns_exit_code_10` in `tests/cli_scan.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 2.3: Multiple Persistent Key Flags
* **Purpose**: Verify that multiple `--key <SYMBOL>` and `--key-xdr <BASE64_XDR>` flags are parsed simultaneously.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --key ADMIN \
    --key COUNTER \
    --key-xdr AAAAFA== \
    --rpc-url http://127.0.0.1:1
  ```
* **Observed Result**: CLI parses all keys into the query batch. Because endpoint is unreachable, exits with code `11` (`EXIT_RPC_FAILURE`), proving arguments were accepted.
* **Test Coverage**: `test_parse_scan_custom_flags` in `crates/sentinel-cli/src/args.rs`.
* **Network Access**: No (fails locally on mock port).
* **On-Chain Safety**: 100% safe.

### Test 2.4: Invalid Key Symbol and Malformed XDR Key Arguments
* **Purpose**: Verify that symbols exceeding 32 characters or malformed Base64/XDR strings return exit code `10`.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Commands**:
  ```bash
  # Symbol > 32 characters
  cargo run -q --bin sentinel -- scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC --key aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  # Malformed base64
  cargo run -q --bin sentinel -- scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC --key-xdr "NOT_VALID_XDR"
  ```
* **Observed Result**: Both commands return exit code `10` (`EXIT_INVALID_ARGS`) with clear error messages.
* **Test Coverage**: `test_derive_persistent_symbol_key_too_long` and `test_derive_persistent_xdr_key_invalid` in `crates/sentinel-core/src/adapters/xdr_keys.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

---

## 3. Validation of Malformed RPC URLs and Unsupported URL Schemes

### Test 3.1: Rejection of Unsupported URL Schemes (`file://`, `ftp://`, `ws://`)
* **Purpose**: Verify that `validate_rpc_url()` blocks arbitrary URI schemes from being accessed.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --rpc-url file:///etc/passwd
  ```
* **Observed Result**: Exits with code `10` (`EXIT_INVALID_ARGS`). Emits `Error: Invalid RPC URL: Unsupported scheme 'file'. Only 'http' and 'https' are supported.`
* **Test Coverage**: `test_cli_scan_invalid_rpc_scheme_returns_exit_code_10` in `tests/cli_scan.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 3.2: Rejection of Malformed URL Syntax
* **Purpose**: Verify that non-URL strings or invalid port numbers passed to `--rpc-url` fail validation immediately.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --rpc-url not_a_valid_url
  ```
* **Observed Result**: Exits with code `10` (`EXIT_INVALID_ARGS`). Emits `Error: Invalid RPC URL: 'not_a_valid_url': relative URL without a base`.
* **Test Coverage**: `test_validate_rpc_url_schemes` in `crates/sentinel-core/src/adapters/soroban_rpc.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 3.3: Rejection of Unknown Network Presets
* **Purpose**: Verify that unrecognized `--network` names prompt an error directing the operator to use `--rpc-url`.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --network invalidnet
  ```
* **Observed Result**: Exits with code `10` (`EXIT_INVALID_ARGS`). Emits `Error: Unknown network 'invalidnet'. Use --rpc-url to specify a custom endpoint.`
* **Test Coverage**: `test_resolve_rpc_url_unknown_network` in `crates/sentinel-cli/src/commands/scan.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

---

## 4. Validation of Contradictory and Small Thresholds

### Test 4.1: Rejection of Contradictory Thresholds (`critical >= warning`)
* **Purpose**: Verify that setting critical threshold greater than or equal to warning threshold is rejected.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --threshold 10000 \
    --critical-threshold 20000 \
    --rpc-url http://127.0.0.1:1
  ```
* **Observed Result**: Exits with code `10` (`EXIT_INVALID_ARGS`). Emits `Error: Invalid threshold configuration: Critical threshold (20000) must be strictly less than warning threshold (10000)`.
* **Test Coverage**: `test_cli_scan_contradictory_thresholds_returns_exit_code_10` in `tests/cli_scan.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 4.2: Rejection of Zero Thresholds
* **Purpose**: Verify that threshold values of 0 are rejected.
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --threshold 0 \
    --rpc-url http://127.0.0.1:1
  ```
* **Observed Result**: Exits with code `10` (`EXIT_INVALID_ARGS`). Emits `Error: Warning threshold must be greater than 0`.
* **Test Coverage**: `test_threshold_validation_valid_and_invalid` in `crates/sentinel-core/src/domain/risk.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 4.3: Safe Dynamic Scaling for Low Warning Thresholds
* **Purpose**: Verify that custom low warning thresholds ($\le 34,560$) automatically scale critical threshold to $\max(1, \text{threshold} / 3)$ so warning state is not bypassed.
* **Verification Method**: `[x] PASS (Automated)`
* **Command**:
  ```bash
  cargo test -p sentinel-cli --lib commands::scan::tests::test_resolve_threshold_config_behavior
  ```
* **Observed Result**: Exits with code `0`. Confirms `threshold = 10000` scales critical to `3333`, preserving the `Warn` classification band.
* **Test Coverage**: `test_edge_case_scaled_low_threshold_preserves_warn_state` in `crates/sentinel-core/tests/edge_cases.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 4.4: Exact Risk Classification Boundaries
* **Purpose**: Verify strict inequality threshold boundaries ($\le 0 \implies \text{Archived}$, $<\text{crit} \implies \text{Critical}$, $<\text{warn} \implies \text{Warn}$, $\ge\text{warn} \implies \text{Ok}$).
* **Verification Method**: `[x] PASS (Automated)`
* **Command**:
  ```bash
  cargo test -p sentinel-core --test edge_cases test_edge_case_exact_threshold_boundaries
  ```
* **Observed Result**: Exits with code `0`. Confirms boundary values:
  - $\le 0 \implies \text{Archived}$ (code 2)
  - $1 \dots \text{crit}-1 \implies \text{Critical}$ (code 2)
  - $\text{crit} \dots \text{warn}-1 \implies \text{Warn}$ (code 1)
  - $\text{warn} \dots \infty \implies \text{Ok}$ (code 0)
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

---

## 5. Successful Scanning Against Soroban RPC Endpoint

### Test 5.1: Live Soroban Testnet Contract Scan (Table Format)
* **Purpose**: Verify end-to-end execution against live Soroban Testnet, extracting instance metadata and calculating TTL horizon.
* **Verification Method**: `[x] PASS (Live Execution)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --network testnet \
    --format text
  ```
* **Observed Result**: Exits with code `0` (`EXIT_OK`). Renders an ASCII table showing columns `Entry Description`, `Durability`, `Size Bytes`, `Live Until Ledger`, `Remaining Horizon`, and `Status`, with summary line.
* **Network Access**: **Yes** (Public Soroban Testnet: `https://soroban-testnet.stellar.org`).
* **On-Chain Safety**: 100% safe (strictly read-only `getLedgerEntries` query).

### Test 5.2: Live Soroban Testnet Contract Scan (JSON Format)
* **Purpose**: Verify machine-readable JSON emission matching the published MVP schema.
* **Verification Method**: `[x] PASS (Live Execution)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --network testnet \
    --format json
  ```
* **Observed Result**: Exits with code `0`. Emits strict valid JSON containing `contract_id`, `network`, `latest_ledger`, `ledger_velocity_seconds`, `summary` (`ok`, `warn`, `critical`, `archived`), and `entries` array. Verified via Python `json.load()` assertion script.
* **Network Access**: **Yes**.
* **On-Chain Safety**: 100% safe.

---

## 6. Behavior When RPC Endpoint Is Unreachable or Returns Malformed Data

### Test 6.1: Unreachable RPC Endpoint
* **Purpose**: Verify that connection refused or unreachable RPC endpoints exit with code `11` (`EXIT_RPC_FAILURE`).
* **Verification Method**: `[x] PASS (Live Execution & Automated)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --rpc-url http://127.0.0.1:1
  ```
* **Observed Result**: Exits with code `11` (`EXIT_RPC_FAILURE`). Emits error details to stderr without panicking. Stdout is 0 bytes.
* **Test Coverage**: `test_cli_scan_unreachable_rpc_returns_exit_code_11` in `tests/cli_scan.rs`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 6.2: Malformed JSON Response from RPC Server
* **Purpose**: Verify that non-JSON responses from an RPC endpoint return exit code `11` (`EXIT_RPC_FAILURE`).
* **Verification Method**: `[x] PASS (Automated)`
* **Command**:
  ```bash
  cargo test -p sentinel-cli --test cli_scan test_cli_scan_malformed_rpc_json_returns_exit_code_11
  ```
* **Observed Result**: Exits with code `11` (`EXIT_RPC_FAILURE`). Emits `Error: Serialization error: Failed to parse JSON-RPC response...` to stderr. Stdout is empty.
* **Test Coverage**: `test_cli_scan_malformed_rpc_json_returns_exit_code_11` in `tests/cli_scan.rs` and `test_wiremock_malformed_json_response` in `crates/sentinel-core/src/adapters/soroban_rpc.rs`.
* **Network Access**: No (uses local in-process `wiremock`).
* **On-Chain Safety**: 100% safe.

### Test 6.3: HTTP 500 / 503 RPC Failure
* **Purpose**: Verify that server-side HTTP errors return controlled `RpcResponseError` and exit code `11`.
* **Verification Method**: `[x] PASS (Automated)`
* **Command**:
  ```bash
  cargo test -p sentinel-core --lib adapters::soroban_rpc::tests::test_wiremock_http_error_status
  ```
* **Observed Result**: Exits with code `0`. Confirms HTTP 500 is mapped to `RpcResponseError { code: 500 }`, which maps to exit code `11`.
* **Test Coverage**: `test_wiremock_http_error_status` and `test_retry_recovery_after_two_http_503_failures`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 6.4: HTTP 3-Hop Redirect Policy & 10 MB Response Cap
* **Purpose**: Verify client limits for redirects and maximum payload sizes.
* **Verification Method**: `[x] PASS (Code Inspection)` / `[ ] NOT RUN (Live Payload Stress)`
* **Details**:
  - `SorobanRpcClient::new()` statically configures `reqwest::redirect::Policy::limited(3)`.
  - Streaming chunk consumer enforces `MAX_RPC_RESPONSE_BYTES = 10 * 1024 * 1024` (10 MB).
  - Live multi-hop redirect and 10 MB streaming payload stress tests were marked **NOT RUN** to avoid external network dependencies and memory exhaustion testing.

---

## 7. Output Format Verification

### Test 7.1: Rejection of Unsupported Output Formats
* **Purpose**: Verify that requesting an unsupported format (e.g. `--format xml` or `--format yaml`) fails cleanly with exit code `10`.
* **Verification Method**: `[x] PASS (Live Execution)`
* **Command**:
  ```bash
  cargo run -q --bin sentinel -- scan \
    CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
    --format yaml \
    --rpc-url http://127.0.0.1:1
  ```
* **Observed Result**: Exits with code `10` (`EXIT_INVALID_ARGS`). Emits `Error: Unsupported format 'yaml'. Supported formats: text, json`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

### Test 7.2: JSON Roundtrip & Schema Completeness
* **Purpose**: Verify that JSON output conforms to `JsonTTLReport` DTO structure.
* **Verification Method**: `[x] PASS (Automated)`
* **Command**:
  ```bash
  cargo test -p sentinel-cli --lib formatters::json::tests::test_render_json_matches_mvp_schema
  ```
* **Observed Result**: Exits with code `0`. Confirms all fields match `JsonTTLReport`, `JsonSummary`, and `JsonEntryReport`.
* **Network Access**: No.
* **On-Chain Safety**: 100% safe.

---

## 8. Process Exit Code Matrix

The CLI sets standard UNIX exit codes suitable for CI/CD automation:

| Exit Code | Constant | Condition | Verification Test |
| :---: | :--- | :--- | :--- |
| `0` | `EXIT_OK` | All entries have status `OK`; or `--help` / `--version` requested | `test_cli_scan_ok_entries_returns_exit_code_0`, `test_cli_help_and_version_flags_return_exit_code_0` |
| `1` | `EXIT_WARN` | One or more entries have status `WARN` | `test_cli_scan_warn_entries_returns_exit_code_1` |
| `2` | `EXIT_CRITICAL_OR_ARCHIVED` | One or more entries have status `CRITICAL` or `ARCHIVED` | `test_cli_scan_critical_entries_returns_exit_code_2` |
| `10` | `EXIT_INVALID_ARGS` | Missing `<CONTRACT_ID>`, malformed Strkey, invalid URL scheme, contradictory thresholds, or unsupported format | `test_cli_scan_missing_contract_id_returns_exit_code_10`, `test_cli_scan_malformed_strkey_returns_exit_code_10` |
| `11` | `EXIT_RPC_FAILURE` | Connection timeout, unreachable endpoint, HTTP 4xx/5xx, JSON-RPC error, or malformed RPC JSON | `test_cli_scan_unreachable_rpc_returns_exit_code_11`, `test_cli_scan_malformed_rpc_json_returns_exit_code_11` |

---

## 9. Regression Tests for Security Remediations & Exit Codes

Run the complete regression suite verifying fixes for SEC-01 through SEC-06 and the exit-code fixes:

```bash
# Verify bounded XDR limits (SEC-02)
cargo test -p sentinel-core --lib adapters::xdr_keys::tests::test_safe_xdr_limits_configuration
cargo test -p sentinel-core --lib adapters::xdr_keys::tests::test_extract_wasm_hash_malformed_rejected

# Verify URL scheme validation & streaming response cap (SEC-05)
cargo test -p sentinel-core --lib adapters::soroban_rpc::tests::test_validate_rpc_url_schemes
cargo test -p sentinel-cli --test cli_scan test_cli_scan_invalid_rpc_scheme_returns_exit_code_10

# Verify threshold validation & contradictory rejection (SEC-01)
cargo test -p sentinel-core --test edge_cases test_edge_case_contradictory_threshold_rejected_in_usecase
cargo test -p sentinel-cli --test cli_scan test_cli_scan_contradictory_thresholds_returns_exit_code_10

# Verify 64-bit integer horizon arithmetic without truncation (SEC-06)
cargo test -p sentinel-core --lib domain::risk::tests::test_risk_evaluation_64bit_no_truncation

# Verify exit code standardization fixes (Commit 67e5db9)
cargo test -p sentinel-cli --test cli_scan test_cli_scan_malformed_rpc_json_returns_exit_code_11
cargo test -p sentinel-cli --test cli_scan test_cli_scan_missing_contract_id_returns_exit_code_10
cargo test -p sentinel-cli --test cli_scan test_cli_scan_critical_entries_returns_exit_code_2

# Verify full workspace test suite (all 87 offline tests)
cargo test --workspace --all-features
```

* **Observed Result**: 100% of offline tests pass cleanly (`87 passed; 0 failed; 3 ignored`).

---

## 10. Documented Limitations for Public Release (v0.1.0)

Before publishing v0.1.0, the following protocol, operational, and testing limitations must be documented in release notes:

1. **Explicit Persistent Key Requirement**:
   - Soroban JSON-RPC 2.0 does not support wildcard prefix-scanning of contract storage keys.
   - Sentinel deterministically inspects `ContractInstance` and `ContractCode` WASM entries.
   - Any auxiliary persistent entries must be passed explicitly via `--key <SYMBOL>` or `--key-xdr <BASE64_XDR>`.
2. **Dual Query Pattern & Ledger Skew Window**:
   - `ScanContractUseCase` queries `getLatestLedger` and `getLedgerEntries` sequentially to maintain `LedgerReader` port decoupling.
   - There is a minor potential 1-ledger skew if a block closes between the two calls.
3. **Local Network / SSRF Trust Model**:
   - Sentinel permits connecting to `localhost` and private network IPs (`--network local`) for local testing.
   - If Sentinel is embedded as a backend worker in a multi-tenant service, network egress filtering must be enforced at the container/proxy layer.
4. **Temporary Durability Scope**:
   - MVP v0.1.0 focuses on `Persistent` and `Instance` storage tiers. Temporary storage TTL monitoring is planned for a future release.
5. **Live Testnet Test Isolation**:
   - 3 live network integration tests in `crates/sentinel-cli/tests/testnet_validation.rs` are annotated with `#[ignore]` by default to keep offline builds hermetic.
6. **Local Security Advisory Scanning**:
   - `cargo audit` has not been run locally because the tool is not installed on the build environment; CI workflows in GitHub Actions must execute `cargo-audit`.
7. **Unperformed Live Stress and Redirect Tests**:
   - Live multi-hop HTTP redirects and synthetic 10 MB streaming payload stress tests were marked `NOT RUN` and verified via static code inspection.
