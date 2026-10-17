# Technical Design Document — Soroban Sentinel

**Project:** `soroban-sentinel`  
**Architecture Style:** Modular Monolith (Ports & Adapters / Hexagonal)  
**Language:** Rust (Edition 2021)  
**Primary Standards:** CAP-0046 (State Archival), Soroban JSON-RPC 2.0 Specification  
**Architecture Record:** [docs/ARCHITECTURE_DECISION.md](ARCHITECTURE_DECISION.md)  

---

## 1. High-Level Architecture & Workspace Layout

`soroban-sentinel` is organized as a **two-crate modular monolith workspace**:

```
soroban-sentinel/
├── Cargo.toml                    # Root workspace manifest
├── crates/
│   ├── sentinel-core/            # Pure domain models, ports, and RPC adapters
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            # Public exports
│   │       ├── domain/           # Core entities & pure business logic (Zero I/O)
│   │       │   ├── mod.rs
│   │       │   ├── entry.rs      # StorageEntry, Durability, EntryType
│   │       │   ├── ttl.rs        # Remaining ledgers, decay velocity & math
│   │       │   └── risk.rs       # RiskLevel (Ok, Warn, Crit, Archived), ThresholdEvaluator
│   │       ├── ports/            # Abstract interfaces (Traits)
│   │       │   ├── mod.rs
│   │       │   ├── reader.rs     # trait LedgerReader
│   │       │   ├── simulator.rs  # trait SimulationEngine (Post-MVP)
│   │       │   └── notifier.rs   # trait AlertNotifier (Post-MVP)
│   │       ├── adapters/         # Concrete external integrations
│   │       │   ├── mod.rs
│   │       │   ├── soroban_rpc/  # JSON-RPC 2.0 implementation of LedgerReader
│   │       │   └── xdr_keys/     # stellar-xdr deterministic key derivation & codecs
│   │       └── usecases/         # Application orchestration workflows
│   │           ├── mod.rs
│   │           └── scan.rs       # ScanContractUseCase implementation
│   └── sentinel-cli/             # Command-line interface & application runner
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs           # CLI entry point
│           ├── args.rs           # clap 4 argument definitions
│           ├── config.rs         # File-based configuration loader (toml)
│           ├── commands/         # Subcommand handlers
│           │   ├── mod.rs
│           │   ├── scan.rs       # `sentinel scan` handler
│           │   ├── forecast.rs   # `sentinel forecast` handler (Post-MVP)
│           │   └── watch.rs      # `sentinel watch` scheduler handler (Post-MVP)
│           └── formatters/       # Output rendering adapters
│               ├── mod.rs
│               ├── text.rs       # Terminal tables (comfy-table)
│               ├── json.rs       # Structured JSON output
│               └── markdown.rs   # Markdown tables for CI/CD comments (Post-MVP)
└── tests/                        # Integration test harnesses
```

---

## 2. Ports and Adapters (Hexagonal) Specification

### 2.1 The Core Domain (`sentinel-core::domain`)
Contains pure business logic, invariants, and calculations with **zero external I/O or network dependencies**.

* **`StorageEntry`**: Represents an on-chain ledger item with byte size, durability, and sequence metadata.
* **`RiskLevel`**:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
  pub enum RiskLevel {
      Ok,
      Warn,
      Critical,
      Archived,
  }
  ```
* **`TtlCalculator`**: Pure mathematical functions computing:
  $$\text{Remaining Ledgers} = \text{liveUntilLedgerSeq} - \text{latestLedgerSequence}$$
  $$\text{Estimated Seconds} = \text{Remaining Ledgers} \times 5.0\text{ seconds}$$

### 2.2 Ports (`sentinel-core::ports`)
Defines the abstract contracts required by application use cases:

```rust
use async_trait::async_trait;
use crate::domain::entry::StorageEntry;
use crate::SentinelResult;

#[async_trait]
pub trait LedgerReader: Send + Sync {
    /// Fetch the current network ledger height and consensus metadata
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo>;

    /// Fetch ledger entries by their raw XDR keys (auto-chunked internally)
    async fn get_ledger_entries(&self, keys: &[stellar_xdr::curr::LedgerKey]) -> SentinelResult<Vec<RawLedgerEntry>>;
}
```

### 2.3 Driven Adapters (`sentinel-core::adapters`)
* **`SorobanRpcClient`**:
  * Implements `LedgerReader` using `reqwest` to post JSON-RPC 2.0 requests.
  * Slices key queries into chunks of maximum **200 keys** to adhere to Soroban RPC limits.
  * Implements exponential backoff with jitter on HTTP 429 and 503 responses.
* **`XdrCodec`**:
  * Implements deterministic key derivation using `stellar-xdr` and `stellar-strkey`.

### 2.4 Driving Adapters (`sentinel-cli`)
* **`sentinel scan` Command**: Parses user arguments, constructs the `ScanContractUseCase` injecting the `SorobanRpcClient`, and forwards the resulting `TTLReport` DTO to the chosen formatter.
* **Formatters**: Convert pure domain DTOs into colored terminal tables (`comfy-table`) or structured JSON (`serde_json`).

---

## 3. Storage Key Derivation & Protocol Invariants

### 3.1 Deterministic Instance & Code Discovery
Soroban RPC does not support wildcard queries or prefix-scans for arbitrary persistent keys. However, the Contract Instance and Code WASM entries are **deterministically discoverable** from the contract address alone:

1. **Contract Instance Key (`ContractDataDurability::Instance`)**:
   ```rust
   pub fn derive_contract_instance_key(contract_id: &[u8; 32]) -> LedgerKey {
       LedgerKey::ContractData(LedgerKeyContractData {
           contract: ScAddress::Contract(Hash(*contract_id)),
           key: ScVal::LedgerKeyContractInstance,
           durability: ContractDataDurability::Instance,
       })
   }
   ```
2. **Contract Code Key (`LedgerKey::ContractCode`)**:
   When the instance entry is fetched, `ContractDataEntry.val` contains `ScVal::ContractInstance(instance)`. If `instance.executable` is `ContractExecutable::Wasm(hash)`, the code key is:
   ```rust
   pub fn derive_contract_code_key(wasm_hash: &[u8; 32]) -> LedgerKey {
       LedgerKey::ContractCode(LedgerKeyContractCode {
           hash: Hash(*wasm_hash),
       })
   }
   ```
3. **Instance Storage Map**:
   All key-values stored inside `instance.storage` reside inside the instance entry itself, sharing the identical TTL of the contract instance.

### 3.2 Persistent Key Derivation (`ContractDataDurability::Persistent`)
For persistent entries stored outside the instance, keys must be explicitly provided (via `--key <SYMBOL>` or `--key-xdr <BASE64>`):
```rust
pub fn derive_persistent_key(contract_id: &[u8; 32], key_scval: ScVal) -> LedgerKey {
    LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash(*contract_id)),
        key: key_scval,
        durability: ContractDataDurability::Persistent,
    })
}
```

---

## 4. End-to-End Scanning Flow

```
User (CLI)             sentinel-cli              sentinel-core               Soroban RPC Node
    |                       |                          |                            |
    |-- scan <CONTRACT_ID>->|                          |                            |
    |                       |-- ScanContractUseCase::new(rpc_adapter)               |
    |                       |-- execute() ------------>|                            |
    |                       |                          |-- get_latest_ledger() ---->|
    |                       |                          |<-- { sequence: 5400000 } --|
    |                       |                          |                            |
    |                       |                          |-- derive_instance_key()    |
    |                       |                          |-- get_ledger_entries([inst])
    |                       |                          |<-- { liveUntil: 5520960 } -|
    |                       |                          |                            |
    |                       |                          |-- extract wasm_hash        |
    |                       |                          |-- derive_code_key()        |
    |                       |                          |-- get_ledger_entries([code])
    |                       |                          |<-- { liveUntil: 5520960 } -|
    |                       |                          |                            |
    |                       |                          |-- TtlCalculator::evaluate()|
    |                       |<- return TTLReport DTO --|                            |
    |                       |                          |                            |
    |                       |-- TextFormatter::render()                             |
    |<- print table/json ---|                                                       |
```

---

## 5. Soroban JSON-RPC 2.0 API Interaction & Batching

* **Method 1: `getLatestLedger`**:
  * Returns `{ id, protocolVersion, sequence }`.
  * Used to determine current ledger height for TTL subtraction.
* **Method 2: `getLedgerEntries`**:
  * Accepts an array of base64 XDR ledger keys.
  * **Strict Limit**: Maximum **200 keys per request**.
  * **Batching Implementation**:
    ```rust
    for chunk in keys.chunks(200) {
        let chunk_response = self.raw_rpc_call("getLedgerEntries", json!({ "keys": chunk })).await?;
        results.extend(chunk_response.entries);
    }
    ```
* **Method 3: `simulateTransaction` (Post-MVP)**:
  * Used by `sentinel forecast` to project exact stroop and XLM fees for `ExtendFootprintTTLOp`.

---

## 6. Future Watch Mode: In-Process Event Architecture

When continuous background monitoring (`sentinel watch`) is introduced in Phase 4:
* **Concurrency Model**: Lightweight **in-process pub/sub** using standard `tokio::sync::broadcast` channels.
* **No External Broker**: Zero Kafka, zero RabbitMQ, zero Redis. Single-process execution.
* **Domain Events**:
  ```rust
  #[derive(Clone, Debug)]
  pub enum SentinelEvent {
      LedgerTicked { sequence: u32, close_time: u64 },
      EntryEvaluated { entry: StorageEntry, remaining_ledgers: i64, risk: RiskLevel },
      RiskThresholdBreached { entry: StorageEntry, risk: RiskLevel },
      RpcPollFailed { error: String, consecutive_failures: u32 },
  }
  ```
* **Event Handlers**:
  * `AlertDispatcher`: Subscribes to `RiskThresholdBreached`, formats Slack/Discord payloads, dispatches HTTP POST.
  * `MetricsSink`: Subscribes to `EntryEvaluated`, updates Prometheus gauge metrics on `:9100/metrics`.

---

## 7. Security Architecture & Signing Boundaries

* **Read-Only by Default (MVP)**:
  * In the MVP, `soroban-sentinel` is **100% read-only**.
  * No private keys, seed phrases, or signatures are accepted or processed.
  * Queries public ledger state using unauthenticated JSON-RPC endpoints.
* **Post-MVP Keeper Boundary (Phase 5)**:
  * When optional autonomous bumping is enabled (`sentinel bump` / `--auto-bump`):
    * Signing key loaded exclusively from `SENTINEL_KEEPER_SECRET` (scrubbed via `secrecy` crate).
    * Transactions restricted strictly to `ExtendFootprintTTLOp` with an empty `readWrite` footprint.
    * Enforced daily maximum spend ceilings.
