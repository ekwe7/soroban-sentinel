# Implementation Roadmap & Milestone Schedule

**Project:** `soroban-sentinel`  
**Architecture:** Modular Monolith (2 Crates: `sentinel-core` and `sentinel-cli`)  
**Target Delivery Window:** Sprints 1–5 (Aligned with Stellar Drips Wave Cycles)  
**Maintainer Commitment:** Solo Maintainer + Wave Open Source Contributors  

---

## 1. Overview & Phased Strategy

The implementation roadmap is partitioned into structured phases. The **MVP (v0.1.0)** is reached at the end of Phase 3, delivering a complete, reliable, tested CLI tool before introducing post-MVP extensions (rent forecasting, watch mode, alerting, metrics, and keeper operations).

```
[Phase 0: Workspace & CI Scaffolding (2 Crates)]
                     |
                     v
[Phase 1: Pure Domain Core & Ports (sentinel-core)]
                     |
                     v
[Phase 2: Soroban RPC & XDR Adapters (sentinel-core)]
                     |
                     v
[Phase 3: CLI Scan, Formatters & Test Harness (sentinel-cli)] <--- MILESTONE 1: MVP RELEASE (v0.1.0)
                     |
                     +---------------------------------------+
                     | Post-MVP Roadmap Extensions           |
                     v                                       |
[Phase 4: Rent Forecaster & Markdown (sentinel forecast)]   <--- MILESTONE 2: v0.2.0
                     |
                     v
[Phase 5: Watch Mode, In-Process Events, Alerts & Metrics]   <--- MILESTONE 3: v0.3.0
                     |
                     v
[Phase 6: Safe Autonomous Bumper Keeper (sentinel bump)]     <--- MILESTONE 4: v0.4.0
```

---

## 2. Detailed Implementation Phases

### Phase 0: Workspace Scaffolding & Automated Quality Gates
* **Objective**: Establish the 2-crate Rust workspace, shared dependencies, and continuous integration workflows.
* **Tasks**:
  * Initialize root `Cargo.toml` with workspace members: `crates/sentinel-core`, `crates/sentinel-cli`.
  * Configure workspace dependencies: `stellar-xdr = "22.0.0"`, `stellar-strkey = "0.0.10"`, `reqwest = "0.12"`, `tokio = "1.38"`, `clap = "4.5"`, `serde = "1.0"`, `comfy-table = "7.1"`.
  * Establish GitHub Actions workflows:
    * `ci.yml`: Runs `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.
    * `audit.yml`: Runs `cargo audit` and `cargo deny` for dependency safety.
* **Dependencies**: None.
* **Effort Estimate**: 1 day (8 hours).
* **Definition of Done**: Clean `cargo check` and 100% green CI pipeline on initial commit.

---

### Phase 1: Pure Domain Core & Ports (`sentinel-core`)
* **Objective**: Implement core domain models, mathematical formulas, and abstract port interfaces with zero external I/O.
* **Tasks**:
  * **Task 1.1**: Implement `domain::entry`:
    * Structs `StorageEntry`, `EntryType`, `Durability`.
  * **Task 1.2**: Implement `domain::ttl`:
    * Pure calculation of remaining ledgers and wall-clock horizon estimation ($5.0\text{s/ledger}$).
  * **Task 1.3**: Implement `domain::risk`:
    * `RiskLevel` enum (`Ok`, `Warn`, `Critical`, `Archived`) and `ThresholdEvaluator`.
  * **Task 1.4**: Implement `ports::reader`:
    * Define abstract `trait LedgerReader` with `get_latest_ledger` and `get_ledger_entries`.
* **Dependencies**: Phase 0.
* **Effort Estimate**: 2 days (16 hours).
* **Definition of Done**: 100% unit test coverage on all domain math and threshold evaluation using in-memory mock structs.

---

### Phase 2: Soroban RPC & XDR Adapters (`sentinel-core`)
* **Objective**: Implement the concrete external drivers connecting the domain to Stellar network protocols.
* **Tasks**:
  * **Task 2.1**: Implement `adapters::xdr_keys`:
    * Deterministic derivation of `ContractInstance` key, `ContractCode` key, and `Persistent` storage keys via `stellar-xdr`.
  * **Task 2.2**: Implement `adapters::soroban_rpc`:
    * Concrete `SorobanRpcClient` implementing `LedgerReader`.
    * Slices key queries into batches of <= 200 keys to satisfy RPC batch limits.
    * Implements exponential backoff with jitter on HTTP 429 and 503 errors.
  * **Task 2.3**: Implement `usecases::scan::ScanContractUseCase`:
    * Coordinates key derivation, RPC calls via `LedgerReader`, code WASM resolution, and risk evaluation.
* **Dependencies**: Phase 1.
* **Effort Estimate**: 3 days (24 hours).
* **Definition of Done**: Passing integration tests with `wiremock` verifying request chunking, deserialization, and retry backoff.

---

### Phase 3: CLI Scan, Formatters & MVP Release (`sentinel-cli` — v0.1.0)
* **Objective**: Deliver the complete, production-grade CLI command `sentinel scan` and publish the MVP.
* **Tasks**:
  * **Task 3.1**: Implement CLI argument parsing in `args.rs` with `clap 4`:
    * Positional contract ID, `--network`, `--rpc-url`, `--key`, `--key-xdr`, `--format` (`text`, `json`), `--threshold`.
  * **Task 3.2**: Implement `commands::scan`:
    * Wires `ScanContractUseCase` with `SorobanRpcClient`, executes scan, and delegates to formatters.
  * **Task 3.3**: Implement `formatters::text`:
    * Colorized terminal ASCII table using `comfy-table`.
  * **Task 3.4**: Implement `formatters::json`:
    * Structured, schema-validated JSON serialization via `serde_json`.
  * **Task 3.5**: Complete end-to-end CLI integration tests (`tests/cli_scan.rs`) using `assert_cmd`.
* **Dependencies**: Phase 2.
* **Effort Estimate**: 3 days (24 hours).
* **Definition of Done**: `sentinel scan` compiles to a single binary, passes all integration tests against live Soroban Testnet, and is tagged as release **v0.1.0**.

---

### Phase 4: Rent Simulation & Markdown Formatter (Post-MVP — v0.2.0)
* **Objective**: Implement fee forecasting via Soroban RPC simulation and Markdown table formatting.
* **Tasks**:
  * **Task 4.1**: Implement `ports::simulator::SimulationEngine` and concrete `simulateTransaction` RPC adapter.
  * **Task 4.2**: Implement `sentinel forecast` command reporting itemized stroop and XLM rent costs.
  * **Task 4.3**: Implement `formatters::markdown` for GitHub Actions PR comments.
* **Dependencies**: Phase 3.
* **Effort Estimate**: 2.5 days (20 hours).
* **Definition of Done**: `sentinel forecast` returns simulated resource fees matching testnet expectations.

---

### Phase 5: Watch Mode, In-Process Events & Telemetry (Post-MVP — v0.3.0)
* **Objective**: Deliver continuous background fleet monitoring, in-process event pub/sub, alerting, and metrics.
* **Tasks**:
  * **Task 5.1**: Implement `config.rs` parsing declarative `sentinel.toml` fleet manifests.
  * **Task 5.2**: Implement `commands::watch` with Tokio async timer and in-process `tokio::sync::broadcast` channels.
  * **Task 5.3**: Implement `AlertNotifier` adapter dispatching HTTP POST webhooks to Slack and Discord with 24h deduplication cooldown.
  * **Task 5.4**: Implement Prometheus exporter serving `/metrics` on port 9100.
* **Dependencies**: Phase 4.
* **Effort Estimate**: 4 days (32 hours).
* **Definition of Done**: Daemon watch mode continuously monitors configured contracts, scrapes Prometheus metrics, and dispatches mock webhooks.

---

### Phase 6: Safe Autonomous Keeper Bumper (Post-MVP — v0.4.0)
* **Objective**: Implement optional, bounded autonomous transaction submission for unattended contract upkeep.
* **Tasks**:
  * **Task 6.1**: Implement `ExtendFootprintTTLOp` transaction envelope builder and signer in `sentinel-core`.
  * **Task 6.2**: Add `--auto-bump` flag to `sentinel watch` with daily spend caps and wallet balance safety floors.
* **Dependencies**: Phase 5.
* **Effort Estimate**: 3.5 days (28 hours).
* **Definition of Done**: Automated integration test on Testnet bumps a test contract's TTL and asserts confirmed on-chain ledger increase.

---

## 3. Milestone Summary & Deliverables

| Milestone | Target Version | Focus | Verifiable Output |
| :--- | :--- | :--- | :--- |
| **M0** | v0.0.1 | Workspace & CI Scaffolding | 2 crates, green GitHub Actions CI |
| **M1 (MVP)** | **v0.1.0** | **Deterministic CLI Scan (`sentinel scan`)** | **CLI binary, text & JSON formats, Testnet verified** |
| **M2** | v0.2.0 | Rent Simulation (`sentinel forecast`) | Simulated stroop/XLM projection + Markdown format |
| **M3** | v0.3.0 | Fleet Watch, Alerts & Prometheus | `sentinel watch`, Slack/Discord hooks, `:9100/metrics` |
| **M4** | v0.4.0 | Safe Autonomous Keeper Bumper | Automated Testnet bump with spend caps |
