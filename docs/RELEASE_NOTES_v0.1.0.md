# Soroban Sentinel — Release Notes (v0.1.0 MVP)

**Release Version:** `v0.1.0`  
**Release Date:** 2026-10-23  
**Target Architecture:** Modular Monolith (Hexagonal Ports & Adapters)  
**License:** MIT  
**Release Commit:** `67e5db9` (`fix(cli): standardize scan error exit codes`)  
**Status:** MVP (Minimum Viable Product)  

---

## 1. Overview & MVP Scope

Soroban Sentinel is an open-source, operator-grade CLI tool designed to inspect and monitor Time-To-Live (TTL) storage horizons for smart contracts running on the Stellar Soroban network ([CAP-0046: State Archival](https://github.com/stellar/stellar-protocol/blob/master/core/cap-0046.md)).

On Soroban, smart contract state is ephemeral by default. If a contract instance or contract code entry reaches its TTL expiration without being bumped, it enters an archived state, causing subsequent invocations to fail. Soroban Sentinel allows developers, protocol operators, and CI/CD pipelines to audit contract storage health proactively without gas expenditure or custodial risk.

### Included in MVP (v0.1.0)
* **Single Contract Inspection (`sentinel scan <CONTRACT_ID>`)**: Evaluates `ContractInstance` and associated `ContractCode` (WASM bytecode) storage entries.
* **Explicit Persistent Storage Key Inspection**: Inspects user-specified persistent storage entries via repeated `--key <SYMBOL>` and `--key-xdr <BASE64_XDR>` options.
* **Deterministic Risk Classification**: Classifies storage entries into 4 discrete risk tiers: `OK`, `WARN`, `CRITICAL`, and `ARCHIVED`.
* **Dual Output Formats**: Human-readable colorized ASCII table (`--format text`) and schema-conforming machine-readable JSON (`--format json`).
* **Standardized CI/CD Process Exit Codes**: Dedicated exit codes for OK, warning, critical/archived findings, invalid operator arguments, and RPC failures.
* **Strict Read-Only Enforcement**: Operates purely via unauthenticated, read-only Soroban JSON-RPC 2.0 endpoints.

### Deferred to Post-MVP (v0.2.0+)
The following features are deliberately excluded from this initial MVP release:
* Automated Rent Simulation (`sentinel forecast <CONTRACT_ID>`)
* Fleet Daemon / Continuous Watch Mode (`sentinel watch --config`)
* Real-time Webhook Alerts (Slack, Discord, PagerDuty)
* Prometheus Metrics Exporter (`:9100/metrics`)
* Autonomous Keeper Bumper (`sentinel bump`)
* Event-based storage key discovery via `getEvents`

---

## 2. CLI Usage & Output Formats

### Command Syntax
```bash
sentinel scan [OPTIONS] <CONTRACT_ID>
```

### Options & Arguments
* `<CONTRACT_ID>` *(Required)*: Target Soroban smart contract address (C... Strkey format, 56 characters).
* `--network <NETWORK>`: Target Stellar network preset: `"testnet"` (default), `"mainnet"`, `"futurenet"`, or `"local"`.
* `--rpc-url <URL>`: Custom Soroban RPC endpoint URL (overrides network preset; can also be provided via `SOROBAN_RPC_URL` environment variable).
* `--format <FORMAT>`: Output format: `"text"` (colorized ASCII table, default) or `"json"` (machine-readable structured output).
* `--key <SYMBOL>`: Persistent storage key symbol to inspect (can be specified multiple times).
* `--key-xdr <BASE64_XDR>`: Persistent storage key encoded as raw base64 `ScVal` XDR (can be specified multiple times).
* `--threshold <LEDGERS>`: Warning threshold in ledgers below which entries trigger `WARN` status (default: `120,960` ledgers ≈ 7 days).
* `--critical-threshold <LEDGERS>`: Critical threshold in ledgers below which entries trigger `CRITICAL` status (default: `34,560` ledgers ≈ 2 days, or `threshold / 3` if `threshold <= 34,560`).
* `-h, --help`: Prints help text and usage.
* `-V, --version`: Prints version information.

### Text Output Example (`--format text`)
```text
Contract: CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC | Network: testnet | Latest Ledger: 5,337,036 (Velocity: 5.0s/ledger)
+-------------------+------------+------------+-------------------+----------------------+--------+
| Entry Description | Durability | Size Bytes | Live Until Ledger | Remaining Horizon    | Status |
+=================================================================================================+
| Contract Instance | Instance   | 240 B      | 8,305,943         | 2,968,907 (~171.8 d) | OK     |
+-------------------+------------+------------+-------------------+----------------------+--------+
Summary: 1 entries checked. 1 OK, 0 WARN, 0 CRITICAL/ARCHIVED.
```

### JSON Output Example (`--format json`)
Structured JSON is written directly to standard output (`stdout`), while diagnostic errors and logs are isolated strictly to standard error (`stderr`):
```json
{
  "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
  "network": "testnet",
  "latest_ledger": 5337036,
  "ledger_velocity_seconds": 5.0,
  "summary": {
    "total_entries": 1,
    "ok": 1,
    "warn": 0,
    "critical": 0,
    "archived": 0
  },
  "entries": [
    {
      "identifier": "instance",
      "entry_type": "contract_instance",
      "durability": "instance",
      "size_bytes": 240,
      "live_until_ledger": 8305943,
      "remaining_ledgers": 2968907,
      "estimated_remaining_seconds": 14844535.0,
      "estimated_remaining_days": 171.81,
      "status": "ok"
    }
  ]
}
```

---

## 3. Standardized Process Exit Codes

Soroban Sentinel emits unambiguous process exit codes suitable for automated scripting and CI/CD quality gates:

| Exit Code | Identifier | Meaning & Trigger Condition |
| :---: | :--- | :--- |
| **`0`** | `EXIT_OK` | All inspected storage entries have adequate remaining TTL (`OK`); also returned for informational `--help` and `--version` invocations. |
| **`1`** | `EXIT_WARN` | One or more storage entries have remaining ledgers below the warning threshold (`WARN`). |
| **`2`** | `EXIT_CRITICAL_OR_ARCHIVED` | One or more storage entries are in imminent danger of expiration (`CRITICAL`) or have already expired (`ARCHIVED`). Reserved strictly for contract risk findings. |
| **`10`** | `EXIT_INVALID_ARGS` | Operator input, syntax, or configuration error. Triggered by: missing `<CONTRACT_ID>`, malformed Strkey, invalid key symbol (> 32 chars), invalid base64 XDR, contradictory thresholds (`critical >= warning`), zero warning threshold, unsupported URL scheme (e.g. `file://`), unknown network preset, or unsupported output format. |
| **`11`** | `EXIT_RPC_FAILURE` | Remote RPC communication or transport failure. Triggered by: network connection timeout, unreachable RPC endpoint, HTTP 4xx/5xx status response, JSON-RPC error payload, or malformed/unparseable RPC JSON response body. |

---

## 4. Storage Inspection Scope & Mechanics

1. **Contract Instance**: The CLI computes the deterministic `LedgerKey::ContractData` for the contract's instance and queries it via `getLedgerEntries`.
2. **Contract Code (WASM Bytecode)**: If the instance references an installed WASM executable, Sentinel extracts the 32-byte bytecode hash and automatically retrieves the associated `LedgerKey::ContractCode` entry.
3. **Explicit Persistent Storage Keys**: Due to protocol constraints, Soroban JSON-RPC does not permit wildcard prefix scanning of contract storage buckets. Operators must supply any specific persistent storage entries using:
   - `--key <SYMBOL>`: Soroban symbol keys (up to 32 ASCII characters).
   - `--key-xdr <BASE64_XDR>`: Raw base64-encoded `ScVal` XDR keys for complex key types (e.g., addresses, vectors, tuples).
4. **Durability Tiers**: Only `Instance` and `Persistent` storage entries are evaluated. `Temporary` storage entries are not monitored in this release.

---

## 5. Security Hardening Summary

The v0.1.0 release includes the following verified security hardening measures:

* **Strict Read-Only Architecture (SEC-03, SEC-04)**: The codebase contains zero transaction signing primitives, zero private key or seed handling modules, and zero on-chain transaction submission logic.
* **Bounded XDR Decoding (SEC-02)**: All XDR deserialization operations use `safe_xdr_limits()` configured with a maximum recursion depth of 100 and a 10 MB payload cap to prevent stack overflow and unbounded memory consumption.
* **RPC URL Scheme Whitelisting (SEC-05)**: The RPC client strictly accepts only `http` and `https` schemes. Local file access (`file:///etc/passwd`), `ftp://`, and arbitrary protocols are rejected before network requests are dispatched.
* **Streaming Response Body Size Limit (SEC-05)**: Enforces `MAX_RPC_RESPONSE_BYTES = 10 * 1024 * 1024` (10 MB). Streaming response bodies exceeding this limit are aborted immediately to prevent heap exhaustion.
* **HTTP Client Protections (SEC-05)**: Enforces a fixed 10-second request timeout, automatic exponential backoff retries on HTTP 429/503 errors, and caps HTTP redirect chains at a maximum of 3 hops.
* **Threshold Validation & Dynamic Auto-Scaling (SEC-01)**: Enforces `warning > 0`, `critical > 0`, and `critical < warning`. When custom warning thresholds are set low ($\le 34,560$ ledgers), the critical threshold dynamically scales to $\max(1, \text{warning} / 3)$ to prevent bypassing the warning state.
* **64-bit Integer Math (SEC-06)**: Uses 64-bit signed integers (`i64`) for ledger arithmetic and expiration horizon calculations, eliminating lossy truncation or integer overflow across extreme horizons.

---

## 6. Known Limitations & Operational Considerations

Operators deploying Soroban Sentinel v0.1.0 must take note of the following documented technical limitations:

1. **No Wildcard Key Discovery**:
   - The Stellar Soroban JSON-RPC 2.0 specification does not support wildcard prefix-scanning of contract storage keys. Sentinel cannot discover auxiliary persistent storage keys automatically without operator-provided `--key` or `--key-xdr` flags.
2. **Sequential Query Pattern & Potential Ledger Skew Window**:
   - Sentinel queries `getLatestLedger` followed by `getLedgerEntries` sequentially to maintain architectural port decoupling. If a new ledger closes between these two network requests, a minor 1-ledger skew may exist between the reported latest ledger and entry live-until sequences.
3. **Local & Private Network Access Model**:
   - The CLI permits connections to `localhost` and private RFC-1918 IP addresses (e.g. `--network local` or custom `--rpc-url`) to facilitate local sandbox development. Operators embedding Sentinel in multi-tenant environments must implement egress filtering at the container or network perimeter.
4. **Storage Durability Scope**:
   - Only `Instance` and `Persistent` storage tiers are monitored. `Temporary` storage entries are outside the scope of MVP v0.1.0.
5. **Live Testnet Test Isolation**:
   - Three live network integration tests in `crates/sentinel-cli/tests/testnet_validation.rs` connect to the public SDF Testnet and are annotated with `#[ignore]` by default so offline builds remain hermetic.
6. **Live Stress and Redirect Tests Not Executed**:
   - Synthetic 10 MB streaming payload stress tests and multi-hop HTTP redirect traversal tests were verified through static code inspection rather than live network simulations to protect host resources and avoid contacting unvetted third-party services.
7. **Local Dependency Advisory Scanning (`cargo audit`)**:
   - `cargo audit` has not been executed locally because the `cargo-audit` subcommand is not installed on the build environment. Furthermore, `.github/workflows/ci.yml` currently runs code formatting, clippy linter, and build/test jobs, but does not execute `cargo audit`. Operators should not assume automated dependency vulnerability scanning is active in CI until an audit step is added.

---

## 7. Verification Results & Test Coverage

Validation was performed against commit `67e5db9` using offline test suites and live Testnet verification:

* **Full Offline Workspace Test Suite**:
  - `sentinel_cli` unit tests: **12 passed**
  - `cli_scan` integration tests: **11 passed** (including binary exit-code assertions)
  - `sentinel_core` unit tests: **48 passed**
  - `edge_cases` integration tests: **12 passed**
  - `mock_rpc_chunking` integration test: **1 passed**
  - `mock_rpc_retries` integration test: **3 passed**
  - **Total Offline Tests: 87 passed; 0 failed; 3 ignored (live Testnet)**
* **Code Formatting**: Verified via `cargo fmt --all -- --check` (0 diffs).
* **Linter Quality Gate**: Verified via `cargo clippy --workspace --all-targets --all-features -- -D warnings` (0 warnings, 0 errors).
* **Lockfile Integrity**: Verified via `cargo check --workspace --all-features --locked` (deterministic dependency resolution).
* **Live Testnet Scan Verification**: Verified end-to-end against the XLM Native Stellar Asset Contract on public Soroban Testnet (`CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`), confirming ledger extraction, table formatting, and strict JSON parsing.

*Verification Boundaries*: This verification does not constitute a formal mathematical verification or third-party cryptographic audit.

---

## 8. MVP Disclaimer

Soroban Sentinel v0.1.0 is an **initial Minimum Viable Product (MVP)**. While engineered with strict architectural standards, defensive limits, and zero unsafe code (`#![forbid(unsafe_code)]`), it should be deployed with standard operational monitoring. Operators should explicitly specify critical persistent keys and verify threshold settings appropriate for their contracts' specific operational lifecycles.
