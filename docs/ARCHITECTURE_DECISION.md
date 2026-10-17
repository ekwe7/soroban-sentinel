# Architecture Decision Record (ADR): Modular Monolith with Ports & Adapters

**Document:** `docs/ARCHITECTURE_DECISION.md`  
**Status:** Approved  
**Deciders:** Core Engineering Team & Project Maintainer  
**Date:** October 2026 Baseline  

---

## 1. Context & Problem Statement

`soroban-sentinel` is designed as an open-source reliability toolkit for Soroban smart contract storage Time-To-Live (TTL) monitoring, archival-risk detection, and automated maintenance. 

Before committing to codebase scaffolding, we must formalize:
1. **Primary Architectural Style**: Balancing operational simplicity, maintainability for an individual developer, and extensibility for community contributors.
2. **Crate Boundary Structure**: Deciding whether to partition the codebase into two crates (`sentinel-core`, `sentinel-cli`) or introduce a third independent daemon crate.
3. **Internal Component Coupling**: Ensuring business and domain rules (TTL decay calculation, ledger horizon conversion, risk assessment) remain strictly decoupled from I/O mechanisms (Soroban RPC HTTP transport, terminal rendering, alerting sinks).
4. **Communication & Concurrency Model**: Designing clean, synchronous direct-call execution for single scans while establishing a pragmatic, lightweight path for future continuous monitoring without introducing distributed systems baggage.

---

## 2. Evaluation of Architectural Options

We evaluated three architectural candidates against the project's actual operational requirements:

```
+---------------------------------------------------------------------------------------------------+
|                                 ARCHITECTURAL PATTERN EVALUATION                                  |
+---------------------+------------------------------+-----------------------+----------------------+
| Criteria            | Candidate A: Microservices   | Candidate B: Event-   | Candidate C: Modular |
|                     |                              | Driven Monolith (Bus) | Monolith (Hexagonal) |
+---------------------+------------------------------+-----------------------+----------------------+
| Deployment Units    | Multiple (Scanner, Alert Svc,| Single or Dual daemon | Single CLI binary    |
|                     | Exporter, Bumper daemon)     | with Kafka/RabbitMQ   | (all commands & watch)
| Operational Overhead| Prohibitive: Requires K8s/   | High: External broker | Minimal: Single      |
|                     | docker-compose, service mesh | process required      | static binary        |
| Maintainer Burden   | High: Distributed tracing,   | High: Broker outages, | Low: Standard Rust   |
| (Solo Dev)          | IPC schemas, multiple repos  | message serialization | workspace & tests    |
| Domain Isolation    | High (Process boundary)      | Moderate (Event msgs) | High (Rust module &  |
|                     |                              |                       | trait visibility)    |
| Performance / Memory| Poor: Multi-process RAM,     | Moderate: Network     | Optimal: Zero IPC,   |
|                     | serializations overhead      | hops for events       | direct function calls|
| Verdict             | REJECTED                     | REJECTED              | ADOPTED              |
+---------------------+------------------------------+-----------------------+----------------------+
```

### Candidate A: Microservices Architecture (REJECTED)
* **Rationale for Rejection**: Breaking this system into distributed microservices (e.g., a separate RPC polling service, a standalone webhook alert worker, and a Prometheus scraping service) introduces massive operational complexity—network failure modes, distributed serialization, process orchestration, and high memory footprints. For a CLI-first utility and lightweight daemon maintained by an individual developer, microservices represent gross over-engineering with zero architectural justification.

### Candidate B: Event-Driven Monolith with External Broker (REJECTED)
* **Rationale for Rejection**: Introducing an external message queue or streaming broker (such as Apache Kafka, RabbitMQ, or Redis Streams) creates heavy runtime dependencies. Operators who want to inspect a contract in CI or run a lightweight monitor should not be forced to deploy and maintain external message brokers.

### Candidate C: Modular Monolith with Ports and Adapters (ADOPTED)
* **Rationale for Adoption**: 
  * **Unified Deployment**: A single Rust binary provides all user commands (`sentinel scan`, `sentinel forecast`, and future `sentinel watch`).
  * **Strict Domain Isolation**: Hexagonal architecture / Ports-and-Adapters cleanly separates pure domain logic from transport protocols and formatting libraries.
  * **Developer Ergonomics**: Fast compile cycles, straightforward local test harnesses without external dependencies, and clear boundaries for Drips Wave contributors.

---

## 3. The Chosen Architecture: Modular Monolith with Ports & Adapters

```
+---------------------------------------------------------------------------------------------------+
|                                     sentinel-cli (Primary Binary)                                 |
|                                                                                                   |
|   +-------------------------------------------------------------------------------------------+   |
|   | Entry Points & Command Dispatcher: clap 4 (scan, forecast, watch)                         |   |
|   +-------------------------------------------------------------------------------------------+   |
|          |                                                                           |            |
|          | (CLI Arguments & Config)                                                  |            |
|          v                                                                           v            |
|   +-------------------------------+                           +-------------------------------+   |
|   | Driving Adapters (Input)      |                           | Driven Adapters (Output)      |   |
|   | • CLI Command Handlers        |                           | • Table Formatter (comfy)     |   |
|   | • File Config Loader (toml)   |                           | • JSON Formatter (serde)      |   |
|   | • Future Watch Scheduler      |                           | • Markdown Formatter          |   |
|   +---------------+---------------+                           +---------------+---------------+   |
|                   |                                                           ^                   |
|                   | Invokes Use Cases                                         | Formats DTOs      |
|                   v                                                           |                   |
+---------------------------------------------------------------------------------------------------+
|                   |                                                           |                   |
|                   |                CROSS-CRATE BOUNDARY                       |                   |
|                   v                                                           |                   |
+---------------------------------------------------------------------------------------------------+
|                                     sentinel-core (Library Crate)                                 |
|                                                                                                   |
|   +-------------------------------------------------------------------------------------------+   |
|   | Application Services (Use Cases)                                                          |   |
|   | • ScanContractUseCase: Coordinates key resolution, TTL fetching & risk assessment        |   |
|   | • ForecastRentUseCase: Coordinates footprint building & fee simulation                   |   |
|   +-------------------------------------------------------------------------------------------+   |
|                   |                                                           ^                   |
|                   | Manipulates Entities                                      | Implements Port   |
|                   v                                                           |                   |
|   +-------------------------------+                           +---------------+---------------+   |
|   | Domain Core (Pure Logic)      |                           | Driven Adapters (External)    |   |
|   | • StorageKey, Durability      |                           | • SorobanRpcClient (reqwest)  |   |
|   | • LiveUntilLedger, Expiration |                           |   Implements: LedgerReader    |   |
|   | • RiskLevel (Ok, Warn, Crit)  |                           | • Future In-Process Notifiers |   |
|   | • LedgerVelocity (5.0s/ledger)|                           |   Implements: AlertSink       |   |
|   +-------------------------------+                           +-------------------------------+   |
|                   ^                                                           ^                   |
|                   |                                                           |                   |
|                   +------------------ Defines Ports --------------------------+                   |
|                                       • trait LedgerReader                                        |
|                                       • trait SimulationEngine                                    |
+---------------------------------------------------------------------------------------------------+
```

---

## 4. Crate Structure Decision: Two Crates vs. Three

### Decision: Adopt a Two-Crate Workspace (`sentinel-core` and `sentinel-cli`)

We evaluated whether to maintain three crates (`sentinel-core`, `sentinel-cli`, `sentinel-daemon`) or two crates.

#### Why Two Crates is Superior:
1. **Elimination of Artificial Boundaries**: A background monitoring loop does not require a distinct crate or microservice. In Rust, long-running services and interactive CLIs share 95% of configuration, logging, and runtime logic.
2. **Single Binary Distribution**: Users install one binary (`cargo install --locked sentinel-cli` or a single Docker image). Running `sentinel scan` executes a synchronous check; running `sentinel watch` (or `sentinel daemon`) runs the continuous scheduler.
3. **Reduced Compilation & CI Overhead**: Maintaining 3 crates triples workspace boilerplate and slows down continuous integration with zero practical payoff at this project scale.
4. **Future Optionality**: If a headless daemon container ever requires distinct stripping or independent enterprise dependencies, it can be split into a third crate later without refactoring `sentinel-core`.

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
│   │       │   └── risk.rs       # RiskLevel, ThresholdEvaluator
│   │       ├── ports/            # Abstract interfaces (Traits)
│   │       │   ├── mod.rs
│   │       │   ├── reader.rs     # trait LedgerReader
│   │       │   ├── simulator.rs  # trait SimulationEngine
│   │       │   └── notifier.rs   # trait AlertNotifier (Future)
│   │       ├── adapters/         # Concrete external integrations
│   │       │   ├── mod.rs
│   │       │   ├── soroban_rpc/  # JSON-RPC 2.0 implementation of LedgerReader
│   │       │   └── xdr_keys/     # stellar-xdr key derivation & codecs
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
│           │   └── forecast.rs   # `sentinel forecast` handler
│           └── formatters/       # Output rendering adapters
│               ├── mod.rs
│               ├── text.rs       # Terminal tables (comfy-table)
│               ├── json.rs       # Structured JSON output
│               └── markdown.rs   # Markdown tables for CI/CD comments
└── tests/                        # Integration test harnesses
```

---

## 5. Module Responsibilities & Dependency Rules

### 5.1 Responsibility Matrix

| Module | Location | Primary Responsibility | Allowed Dependencies | Forbidden Dependencies |
| :--- | :--- | :--- | :--- | :--- |
| **Domain Logic** | `sentinel-core::domain` | Types (`StorageEntry`, `RiskLevel`), TTL decay math, horizon estimation. | Standard library, math utilities. | `reqwest`, `tokio`, `clap`, `comfy-table`, any I/O. |
| **Ports** | `sentinel-core::ports` | Abstract trait definitions (`LedgerReader`, `SimulationEngine`). | Domain types. | Concrete adapter crates, network clients. |
| **RPC Adapter** | `sentinel-core::adapters::soroban_rpc` | Implements `LedgerReader` via Soroban JSON-RPC 2.0 calls. | `reqwest`, `serde`, `serde_json`, `stellar-xdr`. | Terminal formatters, CLI parsing. |
| **XDR Adapter** | `sentinel-core::adapters::xdr_keys` | Translates contract IDs and symbols into XDR `LedgerKey` bytes. | `stellar-xdr`, `stellar-strkey`. | Network libraries, CLI parsing. |
| **Use Cases** | `sentinel-core::usecases` | Orchestrates fetching keys via `LedgerReader`, passing them to domain logic. | Domain types, Ports. | Concrete HTTP clients, terminal formatters. |
| **CLI & Commands** | `sentinel-cli::commands` | Parses CLI flags, builds use cases, calls formatters. | `sentinel-core`, `clap`, `tokio`. | Raw host environment mutations. |
| **Formatters** | `sentinel-cli::formatters` | Renders report DTOs into terminal text, JSON, or Markdown. | `sentinel-core` DTOs, `comfy-table`, `serde_json`. | Network calls, RPC logic. |

### 5.2 Strict Dependency Rule
> [!IMPORTANT]
> **The Inward Dependency Rule**: Inner layers (Domain and Ports) **MUST NOT** depend on outer layers (Adapters, CLI, Network, or Formatters).
> * The Domain layer knows nothing about JSON-RPC, HTTP, or terminal colors.
> * Application Use Cases depend **only** on abstract traits (`dyn LedgerReader`), allowing test harnesses to swap in mock readers instantly without network access.

---

## 6. Communication Model: Synchronous Direct Calls vs. In-Process Events

### 6.1 Synchronous Direct Calls (MVP Baseline)
For all single-execution commands (`sentinel scan`, `sentinel forecast`):
* **Execution**: Strictly synchronous request-response flow managed via `async/await` within Tokio.
* **Typing**: Functions return explicit Rust `Result<T, SentinelError>` types.
* **No Event Overhead**: Zero channels, zero queues, and zero event bus overhead.

### 6.2 In-Process Events (Design for Future Watch Mode)
When continuous fleet monitoring (`sentinel watch` / daemon mode) is introduced in Phase 5:
* **Pattern**: Lightweight **in-process pub/sub** using standard `tokio::sync::broadcast` or `tokio::sync::mpsc`.
* **No Custom Event Bus**: We explicitly reject building a bespoke, home-grown event routing framework. Standard Tokio asynchronous channels provide all necessary concurrency.
* **Candidate Domain Events**:
  ```rust
  #[derive(Clone, Debug)]
  pub enum SentinelEvent {
      LedgerTicked { sequence: u32, close_time: u64 },
      EntryEvaluated { entry: StorageEntry, remaining_ledgers: i64, risk: RiskLevel },
      RiskThresholdBreached { entry: StorageEntry, threshold: RiskLevel },
      RpcPollFailed { error_message: String, consecutive_failures: u32 },
  }
  ```
* **Testing In-Process Handlers**: Event sinks (such as webhook dispatchers and Prometheus metric collectors) are tested by feeding mock `SentinelEvent` instances directly through a Tokio mpsc channel and asserting receiver side-effects with WireMock.

---

## 7. End-to-End Scanning Workflow

```
[User invokes: sentinel scan CA3D5NX... --format text]
                        |
                        v
[sentinel-cli::commands::scan]
  1. Parses Strkey contract address & flags.
  2. Instantiates SorobanRpcClient(rpc_url) as impl LedgerReader.
  3. Instantiates ScanContractUseCase(reader).
                        |
                        v
[sentinel-core::usecases::scan::ScanContractUseCase]
  4. Calls xdr_keys::derive_contract_instance_key(contract_id).
  5. Invokes LedgerReader::get_latest_ledger().
  6. Invokes LedgerReader::get_ledger_entries(&[instance_key]).
  7. If instance has WASM executable, calls derive_contract_code_key(wasm_hash)
     and queries LedgerReader for the code entry.
  8. If persistent keys specified, derives and queries their LedgerKeys.
                        |
                        v
[sentinel-core::domain::ttl & risk]
  9. Calculates remaining ledgers: liveUntilLedgerSeq - latestLedgerSequence.
 10. Evaluates RiskLevel against threshold rules (Ok, Warn, Critical, Archived).
 11. Packages data into pure domain TTLReport DTO.
                        |
                        v
[sentinel-cli::formatters::text]
 12. Receives TTLReport and renders colorized terminal table.
 13. Exits process with status code (0 for OK, 1 for WARN, 2 for CRITICAL/ARCHIVED).
```

---

## 8. Summary of Architectural Consequences

### Positive Consequences
* **Immediate Operational Simplicity**: The entire product compiles to a single, standalone binary with zero external services or daemon dependencies.
* **Exceptional Testability**: 100% of domain logic and use cases can be unit-tested using in-memory mock implementations of `LedgerReader`.
* **High Contributor Accessibility**: Clear module boundaries allow Drips Wave contributors to implement new formatters, CLI flags, or mock tests without risking regressions in core protocol logic.

### Negative / Trade-Off Consequences
* **No Out-of-the-Box Clustering**: A single modular monolith cannot be horizontally scaled across a multi-node cluster with distributed load balancing. (However, Soroban contract monitoring requires minimal CPU/memory, making single-node execution more than adequate for hundreds of contracts).
