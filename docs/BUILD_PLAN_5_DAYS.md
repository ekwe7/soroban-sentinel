# Soroban Sentinel — Five-Day MVP Master Implementation Plan

**Project:** `soroban-sentinel`  
**Target Release:** v0.1.0 (MVP)  
**Architecture:** Modular Monolith (Hexagonal / Ports and Adapters)  
**Workspace:** Two Crates (`crates/sentinel-core` and `crates/sentinel-cli`)  
**Working Directory:** `/home/ekwe/soroban-sentinel/`  
**Document Status:** Master Operational Blueprint  

---

## 1. Executive Summary & Repository Baseline Assessment

### 1.1 Repository State Prior to Execution
* **Documentation & Architecture**: Complete, aligned, and verified. Includes:
  * [`docs/ARCHITECTURE_DECISION.md`](ARCHITECTURE_DECISION.md): Adopts a 2-crate Modular Monolith with Ports & Adapters; rejects microservices, external message brokers, and premature daemon crates.
  * [`docs/PRD.md`](PRD.md): Outlines problem statements, user personas, and functional requirements.
  * [`docs/TECHNICAL_DESIGN.md`](TECHNICAL_DESIGN.md): Documents XDR key derivation mechanics, JSON-RPC 2.0 chunking, and internal domain boundaries.
  * [`docs/MVP_SCOPE.md`](MVP_SCOPE.md): Strictly bounds v0.1.0 to read-only single contract scanning (`sentinel scan`), table/JSON formats, persistent key verification, and automated tests.
  * [`docs/TESTING_STRATEGY.md`](TESTING_STRATEGY.md): Testing pyramid (domain mocks, WireMock, `assert_cmd`).
  * [`docs/SECURITY.md`](SECURITY.md): Read-only guarantees and zero-signing boundaries for MVP.
  * [`docs/OPEN_SOURCE_CONTRIBUTION_PLAN.md`](OPEN_SOURCE_CONTRIBUTION_PLAN.md): Drips Wave issue backlog.
* **Application Code**: Not yet started. Git repository uninitialized; no Cargo manifests or Rust source files exist.
* **Rust Toolchain**: Verified on host: `rustc 1.98.1` and `cargo 1.98.1`.

### 1.2 Five-Day Build Scope Boundaries
```
+-----------------------------------------------------------------------------------+
|                         FIVE-DAY MVP BOUNDARY (v0.1.0)                            |
|                                                                                   |
|  [ IN-SCOPE FOR 5-DAY BUILD ]                                                     |
|  • Day 1: Git init, 2-crate workspace, error types, clap CLI, CI workflows.      |
|  • Day 2: Pure domain entities, TTL decay math, risk engine, LedgerReader port.   |
|  • Day 3: XDR key derivation (instance, code, persistent), Soroban RPC client,   |
|           batch chunking (<= 200 keys), retry/backoff middleware, WireMock tests. |
|  • Day 4: ScanContractUseCase, text table formatter, JSON formatter, exit codes, |
|           end-to-end assert_cmd integration tests.                                |
|  • Day 5: Testnet validation, edge-case coverage, security review, docs sync.     |
|                                                                                   |
|  [ EXPLICITLY DEFERRED TO POST-MVP ROADMAP (v0.2.0+) ]                            |
|  • sentinel.toml fleet file parsing & watch scheduler mode (Phase 5 of roadmap)   |
|  • Slack / Discord webhook notification dispatchers                               |
|  • Prometheus metrics exporter (:9100/metrics)                                    |
|  • Rent forecasting (sentinel forecast / simulateTransaction)                     |
|  • Autonomous keeper bumper (sentinel bump / ExtendFootprintTTLOp)                |
|  • Historical contract event scanning (getEvents)                                 |
+-----------------------------------------------------------------------------------+
```

### 1.3 Key Technical Risks & Failure Mode Mitigations
1. **Arbitrary Persistent Key Discovery Limitation**: Soroban RPC does not support wildcard queries or prefix-scans. The tool will auto-discover Contract Instance and Code WASM entries deterministically, while requiring persistent keys to be passed explicitly via `--key` or `--key-xdr`.
2. **RPC 200-Key Request Ceiling**: Soroban RPC rejects `getLedgerEntries` requests with > 200 keys. Mitigated by enforcing client-side chunking in `sentinel-core`.
3. **Public RPC Network Flakiness**: SDF public testnet can be slow or rate-limited. Mitigated by using in-process `wiremock` for all automated CI tests, reserving live Testnet calls for manual/ignored integration checks.

---

## 2. Five-Day Schedule & Master Checklist

### Master Checklist

- [ ] **DAY 1: Workspace and Foundation**
  - [ ] `D1-T01`: Initialize Git Repository and Gitignore
  - [ ] `D1-T02`: Scaffold Two-Crate Cargo Workspace
  - [ ] `D1-T03`: Implement Workspace Error Types and Core Crate Skeleton
  - [ ] `D1-T04`: Implement Basic CLI Entrypoint and Clap Interface
  - [ ] `D1-T05`: Setup GitHub Actions CI Workflow
- [ ] **DAY 2: Domain Models and TTL Risk Engine**
  - [ ] `D2-T01`: Implement Core Domain Entities and Durability Types
  - [ ] `D2-T02`: Implement Pure TTL Decay Mathematics
  - [ ] `D2-T03`: Implement Risk Classification Engine and Threshold Rules
  - [ ] `D2-T04`: Define Core Abstract Port Interfaces and In-Memory Mocks
- [ ] **DAY 3: Soroban RPC and Storage Inspection**
  - [ ] `D3-T01`: Implement Deterministic XDR Key Derivation Adapter
  - [ ] `D3-T02`: Implement Persistent Key Parser and Derivation
  - [ ] `D3-T03`: Implement Soroban JSON-RPC 2.0 Client Adapter
  - [ ] `D3-T04`: Implement RPC Key Batching and Resilience Middleware
- [ ] **DAY 4: End-to-End Scan and Reporting**
  - [ ] `D4-T01`: Implement ScanContractUseCase Orchestrator
  - [ ] `D4-T02`: Implement Terminal Table Formatter
  - [ ] `D4-T03`: Implement Machine-Readable JSON Output Formatter
  - [ ] `D4-T04`: Wire CLI Command and Process Exit Codes
  - [ ] `D4-T05`: Implement End-to-End CLI Acceptance Integration Tests
- [ ] **DAY 5: Quality, Documentation, and Release Preparation**
  - [ ] `D5-T01`: Implement Live Soroban Testnet Verification Test
  - [ ] `D5-T02`: Expand Edge-Case Coverage and Boundary Testing
  - [ ] `D5-T03`: Perform Security, Unsafe Code, and Dependency Audit
  - [ ] `D5-T04`: Synchronize README, Polish CLI Documentation, and Lint
  - [ ] `D5-T05`: Perform Clean-Environment Release Verification and Tag v0.1.0

---

## 3. Detailed Executable Task Specifications

---

### DAY 1: Workspace and Foundation

#### Task D1-T01: Initialize Git Repository and Gitignore
* **Objective**: Establish git version control and configure standard Rust ignore rules.
* **Prerequisites**: Clean working directory in `/home/ekwe/soroban-sentinel/`.
* **Files Involved**:
  * `.gitignore` (create)
* **Implementation Instructions**:
  * Run `git init` in the repository root.
  * Create `.gitignore` ignoring: `/target/`, `Cargo.lock` (note: lockfile is kept for binaries, but during workspace init ensure target directories and editor files are ignored), `*.log`, `.env`, `.DS_Store`.
* **Terminal Commands**:
  ```bash
  cd /home/ekwe/soroban-sentinel
  git init
  ```
* **Tests**: Run `git status` to verify untracked files are restricted to documentation and `.gitignore`.
* **Acceptance Criteria**: Git repository is initialized; running `git status` cleanly tracks existing docs.
* **Dependencies**: Prerequisite for all subsequent tasks.
* **Estimated Duration**: 15 minutes.

---

#### Task D1-T02: Scaffold Two-Crate Cargo Workspace
* **Objective**: Create the root `Cargo.toml` and scaffold `crates/sentinel-core` and `crates/sentinel-cli`.
* **Prerequisites**: `D1-T01`.
* **Files Involved**:
  * `Cargo.toml` (create)
  * `crates/sentinel-core/Cargo.toml` (create)
  * `crates/sentinel-cli/Cargo.toml` (create)
* **Implementation Instructions**:
  * Create root `Cargo.toml` with `resolver = "2"` and members `crates/sentinel-core` and `crates/sentinel-cli`.
  * Add shared workspace dependencies:
    * `stellar-xdr = { version = "22.0.0", default-features = false, features = ["curr", "std"] }`
    * `stellar-strkey = "0.0.10"`
    * `reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }`
    * `tokio = { version = "1.38", features = ["macros", "rt-multi-thread", "time"] }`
    * `clap = { version = "4.5", features = ["derive", "env"] }`
    * `serde = { version = "1.0", features = ["derive"] }`
    * `serde_json = "1.0"`
    * `comfy-table = "7.1"`
    * `async-trait = "0.1"`
    * `thiserror = "1.0"`
    * `wiremock = "0.6"` (dev-dependency)
    * `assert_cmd = "2.0"` (dev-dependency)
    * `predicates = "3.1"` (dev-dependency)
  * Scaffold `crates/sentinel-core/Cargo.toml` as a library crate.
  * Scaffold `crates/sentinel-cli/Cargo.toml` as a binary crate depending on `sentinel-core`.
* **Terminal Commands**:
  ```bash
  cargo check --workspace
  ```
* **Tests**: `cargo check --workspace` compiles with 0 errors.
* **Acceptance Criteria**: Both crates are registered in the workspace; `cargo check` passes.
* **Dependencies**: Relied upon by `D1-T03`.
* **Estimated Duration**: 30 minutes.

---

#### Task D1-T03: Implement Workspace Error Types and Core Crate Skeleton
* **Objective**: Establish unified domain and infrastructure error types using `thiserror`.
* **Prerequisites**: `D1-T02`.
* **Files Involved**:
  * `crates/sentinel-core/src/lib.rs` (create)
  * `crates/sentinel-core/src/error.rs` (create)
* **Implementation Instructions**:
  * In `error.rs`, define `SentinelError`:
    * `InvalidContractAddress(String)`
    * `InvalidXdrKey(String)`
    * `RpcConnectionTimeout(String)`
    * `RpcResponseError { code: i64, message: String }`
    * `ContractInstanceNotFound(String)`
    * `CodeEntryNotFound(String)`
    * `SerializationError(String)`
  * Define `pub type SentinelResult<T> = Result<T, SentinelError>;`.
  * Export error types from `lib.rs`.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-core
  ```
* **Tests**: Unit test in `error.rs` validating error message display formatting.
* **Acceptance Criteria**: `SentinelError` derives `thiserror::Error` and `Debug`; formats clean human-readable error messages.
* **Dependencies**: Relied upon by all `sentinel-core` modules.
* **Estimated Duration**: 30 minutes.

---

#### Task D1-T04: Implement Basic CLI Entrypoint and Clap Interface
* **Objective**: Create the CLI argument parser with `clap 4` supporting the `scan` command and global flags.
* **Prerequisites**: `D1-T02`, `D1-T03`.
* **Files Involved**:
  * `crates/sentinel-cli/src/main.rs` (create)
  * `crates/sentinel-cli/src/args.rs` (create)
* **Implementation Instructions**:
  * In `args.rs`, define `Cli` struct with subcommands:
    * `Scan(ScanArgs)`:
      * `contract_id`: positional `String`
      * `--rpc-url`: optional `String`
      * `--network`: optional `String` (defaults to `"testnet"`)
      * `--format`: optional `String` (defaults to `"text"`, supports `"text"`, `"json"`)
      * `--key`: optional `Vec<String>`
      * `--key-xdr`: optional `Vec<String>`
      * `--threshold`: optional `u32` (defaults to `120960`)
  * In `main.rs`, parse args using `Cli::parse()`, match on command, and print stub messages.
* **Terminal Commands**:
  ```bash
  cargo run -p sentinel-cli -- --help
  cargo run -p sentinel-cli -- scan --help
  ```
* **Tests**: Verify `cargo run -p sentinel-cli -- --help` prints command overview without panicking.
* **Acceptance Criteria**: Running `--help` and `scan --help` outputs clear, formatted CLI descriptions and options.
* **Dependencies**: Relied upon by `D4-T04`.
* **Estimated Duration**: 45 minutes.

---

#### Task D1-T05: Setup GitHub Actions CI Workflow
* **Objective**: Create automated CI quality gates for formatting, linting, and tests.
* **Prerequisites**: `D1-T02`.
* **Files Involved**:
  * `.github/workflows/ci.yml` (create)
* **Implementation Instructions**:
  * Create workflow triggered on `push` and `pull_request`.
  * Steps:
    1. Checkout repository.
    2. Setup Rust stable toolchain with `clippy` and `rustfmt`.
    3. Run `cargo fmt --all -- --check`.
    4. Run `cargo clippy --workspace --all-targets -- -D warnings`.
    5. Run `cargo test --workspace`.
* **Terminal Commands**:
  ```bash
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
* **Tests**: Run all three commands locally to confirm clean baseline.
* **Acceptance Criteria**: Local check passes with 0 warnings; CI workflow syntax is valid YAML.
* **Dependencies**: Guards all future PRs and tasks.
* **Estimated Duration**: 30 minutes.

---

### DAY 2: Domain Models and TTL Risk Engine

#### Task D2-T01: Implement Core Domain Entities and Durability Types
* **Objective**: Define pure domain representations of Soroban storage entries, durabilities, and identifiers with zero I/O dependencies.
* **Prerequisites**: `D1-T03`.
* **Files Involved**:
  * `crates/sentinel-core/src/domain/mod.rs` (create)
  * `crates/sentinel-core/src/domain/entry.rs` (create)
* **Implementation Instructions**:
  * Define `Durability` enum: `Instance`, `Persistent`, `Temporary`.
  * Define `EntryType` enum: `ContractInstance`, `ContractCode`, `PersistentData`.
  * Define `StorageEntry`:
    * `key_identifier: String`
    * `entry_type: EntryType`
    * `durability: Durability`
    * `byte_size: usize`
    * `live_until_ledger: u32`
    * `last_modified_ledger: u32`
  * Implement constructor and getters. Derive `Clone, Debug, PartialEq, Eq, Serialize, Deserialize`.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-core --lib domain::entry
  ```
* **Tests**: Unit tests in `entry.rs` verifying serialization and property invariants.
* **Acceptance Criteria**: Types compile with zero external network dependencies; derive serde traits cleanly.
* **Dependencies**: Relied upon by `D2-T02`, `D2-T03`.
* **Estimated Duration**: 45 minutes.

---

#### Task D2-T02: Implement Pure TTL Decay Mathematics
* **Objective**: Implement pure calculations for remaining ledgers and wall-clock time horizon estimation.
* **Prerequisites**: `D2-T01`.
* **Files Involved**:
  * `crates/sentinel-core/src/domain/ttl.rs` (create)
* **Implementation Instructions**:
  * Define constant `TARGET_LEDGER_CLOSE_SECONDS: f64 = 5.0;`.
  * Implement `calculate_remaining_ledgers(live_until: u32, latest_ledger: u32) -> i64`:
    * If `live_until == 0`, returns `0` (archived/non-expiring).
    * Returns `(live_until as i64) - (latest_ledger as i64)`.
  * Implement `estimate_remaining_seconds(remaining_ledgers: i64) -> f64`:
    * If `remaining_ledgers <= 0`, returns `0.0`.
    * Returns `(remaining_ledgers as f64) * TARGET_LEDGER_CLOSE_SECONDS`.
  * Implement `estimate_remaining_days(remaining_seconds: f64) -> f64`.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-core --lib domain::ttl
  ```
* **Tests**: Unit tests in `ttl.rs`:
  * Positive remaining ledgers (e.g., 5,500,000 - 5,400,000 = 100,000 ledgers -> 500,000s -> 5.78 days).
  * Expired entry (latest_ledger > live_until).
  * Boundary zero value (`live_until == 0`).
* **Acceptance Criteria**: 100% test coverage of decay math; handles negative and zero spans without underflow or panics.
* **Dependencies**: Relied upon by `D2-T03`, `D4-T01`.
* **Estimated Duration**: 45 minutes.

---

#### Task D2-T03: Implement Risk Classification Engine and Threshold Rules
* **Objective**: Evaluate storage entry lifespans against risk levels (`OK`, `WARN`, `CRITICAL`, `ARCHIVED`).
* **Prerequisites**: `D2-T02`.
* **Files Involved**:
  * `crates/sentinel-core/src/domain/risk.rs` (create)
* **Implementation Instructions**:
  * Define `RiskLevel` enum: `Ok`, `Warn`, `Critical`, `Archived` (deriving `Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord`).
  * Define `ThresholdConfig`:
    * `warning_ledgers: u32` (default: `120_960`, ~7 days)
    * `critical_ledgers: u32` (default: `34_560`, ~2 days)
  * Implement `evaluate_risk(remaining_ledgers: i64, config: &ThresholdConfig) -> RiskLevel`:
    * If `remaining_ledgers <= 0`: `RiskLevel::Archived`.
    * If `(remaining_ledgers as u32) < config.critical_ledgers`: `RiskLevel::Critical`.
    * If `(remaining_ledgers as u32) < config.warning_ledgers`: `RiskLevel::Warn`.
    * Otherwise: `RiskLevel::Ok`.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-core --lib domain::risk
  ```
* **Tests**: Unit tests in `risk.rs` testing exact boundary values (`critical - 1`, `critical`, `warning - 1`, `warning`, `0`, negative).
* **Acceptance Criteria**: Correct classification across all boundary ledger values; comprehensive test suite.
* **Dependencies**: Relied upon by `D4-T01`, `D4-T02`.
* **Estimated Duration**: 45 minutes.

---

#### Task D2-T04: Define Core Abstract Port Interfaces and In-Memory Mocks
* **Objective**: Formalize the hexagonal `LedgerReader` port and provide an in-memory mock implementation for offline use case testing.
* **Prerequisites**: `D2-T01`.
* **Files Involved**:
  * `crates/sentinel-core/src/ports/mod.rs` (create)
  * `crates/sentinel-core/src/ports/reader.rs` (create)
* **Implementation Instructions**:
  * In `reader.rs`, define DTOs:
    * `LatestLedgerInfo { sequence: u32, protocol_version: u32 }`
    * `RawLedgerEntry { key_xdr_base64: String, entry_xdr_base64: String, live_until_ledger_seq: u32, last_modified_ledger_seq: u32 }`
  * Define trait:
    ```rust
    #[async_trait::async_trait]
    pub trait LedgerReader: Send + Sync {
        async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo>;
        async fn get_ledger_entries(&self, keys_xdr_base64: &[String]) -> SentinelResult<Vec<RawLedgerEntry>>;
    }
    ```
  * In `reader.rs` (under `#[cfg(test)]` or a `mock` module), implement `MockLedgerReader` allowing predefined return values.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-core --lib ports::reader
  ```
* **Tests**: Unit test verifying `MockLedgerReader` returns stubbed `LatestLedgerInfo` and `RawLedgerEntry`.
* **Acceptance Criteria**: Trait is object-safe or async-trait compliant; mock reader enables testing use cases without network.
* **Dependencies**: Relied upon by `D3-T03`, `D4-T01`.
* **Estimated Duration**: 45 minutes.

---

### DAY 3: Soroban RPC and Storage Inspection

#### Task D3-T01: Implement Deterministic XDR Key Derivation Adapter
* **Objective**: Implement derivation of `ContractInstance` and `ContractCode` XDR ledger keys from contract IDs and WASM hashes using `stellar-xdr`.
* **Prerequisites**: `D1-T03`.
* **Files Involved**:
  * `crates/sentinel-core/src/adapters/mod.rs` (create)
  * `crates/sentinel-core/src/adapters/xdr_keys.rs` (create)
* **Implementation Instructions**:
  * Implement `parse_contract_id(strkey: &str) -> SentinelResult<[u8; 32]>` using `stellar_strkey::Contract`.
  * Implement `derive_contract_instance_key(contract_id: &[u8; 32]) -> SentinelResult<String>`:
    * Constructs `LedgerKey::ContractData` with `durability: Instance` and `key: LedgerKeyContractInstance`.
    * Serializes to base64 XDR string via `stellar_xdr::curr::LedgerKey::to_xdr_base64`.
  * Implement `derive_contract_code_key(wasm_hash: &[u8; 32]) -> SentinelResult<String>`:
    * Constructs `LedgerKey::ContractCode` and serializes to base64 XDR string.
  * Implement `extract_wasm_hash_from_instance_entry(entry_xdr_base64: &str) -> SentinelResult<Option<[u8; 32]>>`:
    * Decodes `LedgerEntryData::ContractData`, matches `val` on `ScVal::ContractInstance`, extracts `executable` hash if `ContractExecutable::Wasm`.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-core --lib adapters::xdr_keys
  ```
* **Tests**: Unit tests verifying:
  * Strkey decoding of valid `C...` address.
  * Rejection of invalid/malformed address.
  * Deterministic base64 XDR matches known test vector.
  * WASM hash extraction from sample instance entry XDR.
* **Acceptance Criteria**: Valid base64 XDR keys produced; rejects invalid inputs with typed `SentinelError`.
* **Dependencies**: Relied upon by `D3-T02`, `D4-T01`.
* **Estimated Duration**: 60 minutes.

---

#### Task D3-T02: Implement Persistent Key Parser and Derivation
* **Objective**: Parse user-supplied persistent storage keys (symbols or base64 XDR `ScVal`) into valid `LedgerKey::ContractData` XDR strings.
* **Prerequisites**: `D3-T01`.
* **Files Involved**:
  * `crates/sentinel-core/src/adapters/xdr_keys.rs` (modify)
* **Implementation Instructions**:
  * Implement `derive_persistent_symbol_key(contract_id: &[u8; 32], symbol: &str) -> SentinelResult<String>`:
    * Constructs `ScVal::Symbol(ScSymbol(symbol.try_into().map_err(...)?))`.
    * Assembles `LedgerKey::ContractData` with `durability: Persistent`.
    * Encodes to base64 XDR.
  * Implement `derive_persistent_xdr_key(contract_id: &[u8; 32], scval_base64: &str) -> SentinelResult<String>`:
    * Decodes `ScVal` from base64.
    * Assembles `LedgerKey::ContractData` with `durability: Persistent` and encodes to base64.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-core --lib adapters::xdr_keys
  ```
* **Tests**: Unit tests verifying symbol derivation (e.g., symbol "Counter") and arbitrary XDR `ScVal` parsing.
* **Acceptance Criteria**: Handles valid symbols and XDR strings cleanly; rejects invalid base64 with `SentinelError::InvalidXdrKey`.
* **Dependencies**: Relied upon by `D4-T01`.
* **Estimated Duration**: 45 minutes.

---

#### Task D3-T03: Implement Soroban JSON-RPC 2.0 Client Adapter
* **Objective**: Implement concrete `SorobanRpcClient` conforming to `LedgerReader` using `reqwest`.
* **Prerequisites**: `D2-T04`.
* **Files Involved**:
  * `crates/sentinel-core/src/adapters/soroban_rpc.rs` (create)
* **Implementation Instructions**:
  * Implement `SorobanRpcClient`:
    * Fields: `rpc_url: String`, `http_client: reqwest::Client`.
    * Implements `LedgerReader`:
      * `get_latest_ledger`: sends JSON-RPC 2.0 `{"jsonrpc": "2.0", "id": 1, "method": "getLatestLedger", "params": {}}`. Parses `sequence` and `protocolVersion`.
      * `get_ledger_entries`: sends `{"jsonrpc": "2.0", "id": 1, "method": "getLedgerEntries", "params": {"keys": [...]}}`. Parses `entries` array into `RawLedgerEntry`.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-core --lib adapters::soroban_rpc
  ```
* **Tests**: Unit tests with mock JSON deserialization validating parsing of standard Soroban RPC responses.
* **Acceptance Criteria**: Formats valid JSON-RPC 2.0 requests; parses standard SDF responses cleanly.
* **Dependencies**: Relied upon by `D3-T04`, `D4-T01`.
* **Estimated Duration**: 60 minutes.

---

#### Task D3-T04: Implement RPC Key Batching and Resilience Middleware
* **Objective**: Enforce the <= 200 key limit per request via chunking and add exponential backoff for HTTP 429/503.
* **Prerequisites**: `D3-T03`.
* **Files Involved**:
  * `crates/sentinel-core/src/adapters/soroban_rpc.rs` (modify)
  * `tests/mock_rpc_chunking.rs` (create)
  * `tests/mock_rpc_retries.rs` (create)
* **Implementation Instructions**:
  * In `get_ledger_entries`, slice `keys_xdr_base64.chunks(200)`.
  * For each chunk, issue POST request.
  * Wrap call with retry loop (max 3 attempts): on HTTP 429 or 503, sleep with exponential backoff (`base_ms * 2^attempt` with jitter).
  * Consolidate results into a single `Vec<RawLedgerEntry>`.
* **Terminal Commands**:
  ```bash
  cargo test --test mock_rpc_chunking --test mock_rpc_retries
  ```
* **Tests**:
  * WireMock test with 250 keys verifying exactly 2 consecutive POST requests (200 + 50).
  * WireMock test returning HTTP 503 twice followed by HTTP 200 verifying retry recovery.
* **Acceptance Criteria**: Transparent chunking for lists > 200 keys; resilience against transient network/rate-limit errors.
* **Dependencies**: Relied upon by `D4-T01`.
* **Estimated Duration**: 60 minutes.

---

### DAY 4: End-to-End Scan and Reporting

#### Task D4-T01: Implement ScanContractUseCase Orchestrator
* **Objective**: Orchestrate key resolution, RPC fetching, WASM resolution, and risk evaluation into a unified use case.
* **Prerequisites**: `D2-T02`, `D2-T03`, `D3-T01`, `D3-T02`, `D3-T04`.
* **Files Involved**:
  * `crates/sentinel-core/src/usecases/mod.rs` (create)
  * `crates/sentinel-core/src/usecases/scan.rs` (create)
* **Implementation Instructions**:
  * Define `ScanRequest`:
    * `contract_id_strkey: String`
    * `persistent_symbols: Vec<String>`
    * `persistent_xdr_keys: Vec<String>`
    * `threshold_config: ThresholdConfig`
  * Define `TTLReport`:
    * `contract_id: String`
    * `latest_ledger: u32`
    * `ledger_velocity_seconds: f64`
    * `entries: Vec<StorageEntryReport>`
    * `overall_status: RiskLevel`
  * Implement `ScanContractUseCase<R: LedgerReader>`:
    * Coordinates key derivation, fetches latest ledger, queries entries via `R`, resolves WASM hash, queries code entry, calculates TTLs, evaluates risk.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-core --lib usecases::scan
  ```
* **Tests**: Unit tests using `MockLedgerReader` validating full end-to-end report generation with 0 network calls.
* **Acceptance Criteria**: Produces complete `TTLReport` DTO; handles missing code or missing entries gracefully without panicking.
* **Dependencies**: Relied upon by `D4-T04`.
* **Estimated Duration**: 60 minutes.

---

#### Task D4-T02: Implement Terminal Table Formatter
* **Objective**: Render `TTLReport` into an ergonomic, colored ASCII table with summary health status.
* **Prerequisites**: `D4-T01`.
* **Files Involved**:
  * `crates/sentinel-cli/src/formatters/mod.rs` (create)
  * `crates/sentinel-cli/src/formatters/text.rs` (create)
* **Implementation Instructions**:
  * Using `comfy-table`:
    * Columns: `Entry Description`, `Durability`, `Size Bytes`, `Live Until Ledger`, `Remaining Horizon`, `Status`.
    * Apply colors: `OK` (green), `WARN` (yellow), `CRITICAL` (red), `ARCHIVED` (dark grey/red).
  * Format bottom summary line: `Summary: N entries checked. X OK, Y WARN, Z CRITICAL/ARCHIVED.`
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-cli --lib formatters::text
  ```
* **Tests**: Unit test verifying rendered table string contains headers and contract ID.
* **Acceptance Criteria**: Formats cleanly across terminal widths; visually clear and color-coded.
* **Dependencies**: Relied upon by `D4-T04`.
* **Estimated Duration**: 45 minutes.

---

#### Task D4-T03: Implement Machine-Readable JSON Output Formatter
* **Objective**: Render `TTLReport` as strict, pretty-printed JSON conforming to `docs/MVP_SCOPE.md`.
* **Prerequisites**: `D4-T01`.
* **Files Involved**:
  * `crates/sentinel-cli/src/formatters/json.rs` (create)
* **Implementation Instructions**:
  * Implement `render_json(report: &TTLReport) -> SentinelResult<String>`.
  * Ensure JSON matches schema with fields: `contract_id`, `network`, `latest_ledger`, `ledger_velocity_seconds`, `summary` (`ok`, `warn`, `critical`, `archived`), and `entries` array.
* **Terminal Commands**:
  ```bash
  cargo test -p sentinel-cli --lib formatters::json
  ```
* **Tests**: Unit test verifying JSON round-trip deserialization and field validation.
* **Acceptance Criteria**: Emits 100% valid JSON matching the published schema in `docs/MVP_SCOPE.md`.
* **Dependencies**: Relied upon by `D4-T04`.
* **Estimated Duration**: 30 minutes.

---

#### Task D4-T04: Wire CLI Command and Process Exit Codes
* **Objective**: Connect `sentinel scan` CLI input flags to `ScanContractUseCase`, format output, and set standard UNIX exit codes.
* **Prerequisites**: `D1-T04`, `D4-T01`, `D4-T02`, `D4-T03`.
* **Files Involved**:
  * `crates/sentinel-cli/src/commands/mod.rs` (create)
  * `crates/sentinel-cli/src/commands/scan.rs` (create)
  * `crates/sentinel-cli/src/main.rs` (modify)
* **Implementation Instructions**:
  * Wire network presets:
    * `"testnet"` -> `https://soroban-testnet.stellar.org`
    * `"mainnet"` -> `https://soroban-rpc.stellar.org`
    * Override with `--rpc-url` if provided.
  * Instantiate `SorobanRpcClient` and `ScanContractUseCase`.
  * Execute scan. Delegate to `render_text` or `render_json`.
  * Determine exit code:
    * `0`: All entries `OK`.
    * `1`: One or more entries `WARN`.
    * `2`: One or more entries `CRITICAL` or `ARCHIVED`.
    * `10`: Invalid CLI argument / Strkey.
    * `11`: RPC communication failure.
  * Exit with `std::process::exit(code)`.
* **Terminal Commands**:
  ```bash
  cargo run -p sentinel-cli -- scan --help
  ```
* **Tests**: Manual test checking invalid address returns exit code 10.
* **Acceptance Criteria**: Seamless execution from CLI; outputs text/JSON based on flag; returns accurate exit codes.
* **Dependencies**: Relied upon by `D4-T05`.
* **Estimated Duration**: 45 minutes.

---

#### Task D4-T05: Implement End-to-End CLI Acceptance Integration Tests
* **Objective**: Write comprehensive CLI acceptance tests spawning the compiled binary via `assert_cmd`.
* **Prerequisites**: `D4-T04`.
* **Files Involved**:
  * `tests/cli_scan.rs` (create)
* **Implementation Instructions**:
  * Using `assert_cmd` and `wiremock`:
    * Test A: Valid scan returning mock OK entries -> asserts stdout table and exit code `0`.
    * Test B: Valid scan returning mock WARN entries -> asserts exit code `1`.
    * Test C: Valid scan with `--format json` -> parses stdout as valid JSON.
    * Test D: Malformed Strkey -> asserts stderr error message and exit code `10`.
    * Test E: Unreachable RPC -> asserts exit code `11`.
* **Terminal Commands**:
  ```bash
  cargo test --test cli_scan
  ```
* **Tests**: All 5 test cases pass cleanly without hitting live networks.
* **Acceptance Criteria**: Fast, fully reproducible offline acceptance test suite covering all CLI execution paths.
* **Dependencies**: Relied upon by `D5-T02`.
* **Estimated Duration**: 60 minutes.

---

### DAY 5: Quality, Documentation, and Release Preparation

#### Task D5-T01: Implement Live Soroban Testnet Verification Test
* **Objective**: Provide an end-to-end integration test against live Soroban Testnet.
* **Prerequisites**: `D4-T05`.
* **Files Involved**:
  * `tests/testnet_validation.rs` (create)
* **Implementation Instructions**:
  * Create integration test targeting `https://soroban-testnet.stellar.org`.
  * Annotate with `#[ignore]` so offline CI does not depend on external network uptime.
  * Scan a known reference Soroban Testnet contract (e.g., example token or reference instance).
  * Assert `latest_ledger > 5_000_000` and instance entry has valid `live_until_ledger`.
* **Terminal Commands**:
  ```bash
  cargo test --test testnet_validation -- --ignored
  ```
* **Tests**: Verify command succeeds when connected to the internet.
* **Acceptance Criteria**: Live testnet contracts scan successfully and extract real instance/WASM metadata.
* **Dependencies**: Relied upon by `D5-T05`.
* **Estimated Duration**: 45 minutes.

---

#### Task D5-T02: Expand Edge-Case Coverage and Boundary Testing
* **Objective**: Thoroughly test edge cases and ensure `sentinel-core` test coverage exceeds 85%.
* **Prerequisites**: `D4-T05`.
* **Files Involved**:
  * `tests/edge_cases.rs` (create)
* **Implementation Instructions**:
  * Test: Contract instance without WASM code executable (e.g. built-in Stellar asset contract).
  * Test: Contract instance already archived (`liveUntilLedgerSeq == 0`).
  * Test: Requested persistent key missing from response (never initialized).
  * Test: Extremely large remaining ledger horizon (> 10 years).
* **Terminal Commands**:
  ```bash
  cargo test --workspace
  ```
* **Tests**: All tests pass across the workspace.
* **Acceptance Criteria**: All boundary and failure modes from `docs/TESTING_STRATEGY.md` are covered with zero panics.
* **Dependencies**: Relied upon by `D5-T04`.
* **Estimated Duration**: 45 minutes.

---

#### Task D5-T03: Perform Security, Unsafe Code, and Dependency Audit
* **Objective**: Verify read-only guarantees, absence of unsafe code, and dependency integrity.
* **Prerequisites**: `D5-T02`.
* **Files Involved**:
  * All crate files (inspection)
* **Implementation Instructions**:
  * Grep workspace for `unsafe`: assert 0 instances of `unsafe` blocks.
  * Verify binary contains zero write operations (`ExtendFootprintTTLOp` or signing logic).
  * Run `cargo audit` (if installed) or verify dependency tree with `cargo tree`.
* **Terminal Commands**:
  ```bash
  grep -rn "unsafe" crates/
  cargo tree --duplicates
  ```
* **Tests**: Assert `grep -rn "unsafe"` returns 0 results in source code.
* **Acceptance Criteria**: Zero unsafe code; zero write capabilities; clean dependency tree.
* **Dependencies**: Relied upon by `D5-T05`.
* **Estimated Duration**: 30 minutes.

---

#### Task D5-T04: Synchronize README, Polish CLI Documentation, and Lint
* **Objective**: Ensure README, CLI help strings, and documentation are 100% synchronized with actual code.
* **Prerequisites**: `D5-T03`.
* **Files Involved**:
  * `README.md` (modify)
  * `crates/sentinel-cli/src/args.rs` (modify)
* **Implementation Instructions**:
  * Update `README.md` with verified `sentinel scan` output examples and command-line usage flags.
  * Polish CLI help docstrings across `args.rs`.
  * Run strict formatting and linting gates.
* **Terminal Commands**:
  ```bash
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  ```
* **Tests**: Strict clippy check passes with 0 warnings.
* **Acceptance Criteria**: Zero warnings under `-D warnings`; README reflects real CLI flags.
* **Dependencies**: Relied upon by `D5-T05`.
* **Estimated Duration**: 45 minutes.

---

#### Task D5-T05: Perform Clean-Environment Release Verification and Tag v0.1.0
* **Objective**: Verify release compilation, execute full test suite, and tag `v0.1.0`.
* **Prerequisites**: `D5-T01`, `D5-T02`, `D5-T03`, `D5-T04`.
* **Files Involved**:
  * `docs/BUILD_PLAN_5_DAYS.md` (check off all items)
* **Implementation Instructions**:
  * Run `cargo build --release --workspace`.
  * Verify release binary size and startup speed:
    ```bash
    ./target/release/sentinel --help
    ```
  * Run full test suite:
    ```bash
    cargo test --workspace
    ```
  * Mark all master checklist items complete.
* **Terminal Commands**:
  ```bash
  cargo build --release --workspace
  ./target/release/sentinel --help
  cargo test --workspace
  ```
* **Tests**: Release binary executes in `< 20ms`; 100% tests pass.
* **Acceptance Criteria**: Functional, tested, documented MVP binary compiled and verified.
* **Dependencies**: Final completion milestone.
* **Estimated Duration**: 30 minutes.

---

## 4. Reusable Task Execution Template (For Gemini CLI)

Use this exact prompt whenever executing an individual task from this master plan:

```markdown
You are executing a single task from the Soroban Sentinel master implementation plan.

TASK TO EXECUTE: [INSERT TASK ID, e.g. D1-T01]

Instructions:
1. Open and inspect `docs/BUILD_PLAN_5_DAYS.md`. Locate the task specification for [INSERT TASK ID].
2. Inspect the current workspace and Git status before making any changes.
3. Confirm that all prerequisite tasks listed for [INSERT TASK ID] are completed. If any prerequisite is missing, stop and report it as a blocker.
4. Implement ONLY what is specified for [INSERT TASK ID]. Do NOT implement future tasks, speculative abstractions, or unrelated changes.
5. Follow the agreed architecture:
   - Modular Monolith (Ports and Adapters).
   - Domain logic in `sentinel-core` must have ZERO I/O, network, or terminal dependencies.
   - The MVP is strictly read-only. Do not add private keys or transaction signers.
6. Write or update the specific unit/integration tests specified in the task.
7. Run the task's required terminal commands and checks. Verify that:
   - `cargo fmt --all -- --check` passes.
   - `cargo clippy --workspace --all-targets -- -D warnings` passes.
   - All tests associated with the task pass.
8. When finished, provide a concise summary:
   - The exact files created or modified.
   - The actual command output from tests and clippy.
   - The manual verification command I can run to inspect your work.
9. Stop and wait for my review before proceeding to any other task.
```

---

## 5. Scope Exclusions & Deferral Rationale

To guarantee delivery of a rock-solid, production-grade MVP within the 5-day timeline, the following features are deliberately deferred to post-MVP milestones:

| Feature | Roadmap Milestone | Technical Rationale for Deferral |
| :--- | :--- | :--- |
| **Fleet Watch Scheduler (`sentinel watch`)** | Phase 5 (v0.3.0) | Requires continuous polling timers, config reload, and process signal handling. The single-scan CLI must be tested and hardened first. |
| **Slack / Discord Alert Webhooks** | Phase 5 (v0.3.0) | Relies on the watch mode event dispatcher and deduplication state machines. |
| **Prometheus Exporter (`:9100/metrics`)** | Phase 5 (v0.3.0) | Requires Axum HTTP server integration; unnecessary for one-shot CLI invocations. |
| **Rent Simulation (`sentinel forecast`)** | Phase 4 (v0.2.0) | Requires building `ExtendFootprintTTLOp` envelopes and invoking `simulateTransaction`. |
| **Autonomous Keeper (`sentinel bump`)** | Phase 6 (v0.4.0) | Requires wallet key loading, sequence management, and on-chain transaction signing. Incurring financial loss or network fees is unsafe during initial MVP development. |
| **Historical Event Scraping (`getEvents`)** | Phase 7 (v0.5.0) | Requires deep block ingestion or RPC pagination over millions of ledgers. |
