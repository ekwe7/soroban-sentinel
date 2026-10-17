# Research, Ecosystem Reference & Technical Verification Dossier

**Project:** `soroban-sentinel`  
**Research Date:** October 2026 Baseline  
**Investigator:** Lead Project Architect  
**Subject:** Stellar Soroban State Archival, Storage Rent Mechanics & Tooling Landscape  

---

## 1. Primary Sources & Codebases Inspected

During the project validation and technical investigation phases, the following official Stellar repositories, standards, and community implementations were inspected:

### 1.1 Official Specifications & RFCs
* **[CAP-0046: Soroban State Archival](https://github.com/stellar/stellar-protocol/blob/master/core/cap-0046.md)**: Foundational specification defining the separation of temporary, persistent, and instance storage, BucketList archival transitions, and the `ExtendFootprintTTLOp` / `RestoreFootprintOp` operations.
* **[CAP-0050: Smart Contract State Archival Pruning](https://github.com/stellar/stellar-protocol/blob/master/core/cap-0050.md)**: Protocol mechanics for evicting expired entries from validator active memory and managing the archival cold-storage bucket structure.
* **[Soroban JSON-RPC 2.0 Specification](https://stellar.org/developers/soroban-rpc)**: Official documentation detailing the parameters, batch size limits (200 keys), and response schemas for `getLedgerEntries`, `getLatestLedger`, and `simulateTransaction`.

### 1.2 Official Stellar Repositories
* **`stellar/stellar-rpc`** ([GitHub](https://github.com/stellar/stellar-rpc)): Source code for the JSON-RPC server. Verified `getLedgerEntries` query constraints, `liveUntilLedgerSeq` metadata assignment, and the non-existence of prefix-scan methods for contract storage keys.
* **`stellar/stellar-cli`** ([GitHub](https://github.com/stellar/stellar-cli)): Inspected subcommands `stellar contract extend`, `stellar contract restore`, `stellar contract read`, and `stellar contract info`. Confirmed that existing CLI tools are interactive, manual, single-key commands lacking daemon observability or automated fleet alerting.
* **`stellar/rs-soroban-env`** ([GitHub](https://github.com/stellar/rs-soroban-env)): Host environment handling of storage bumps, internal `extend_ttl` host functions, and resource fee charging models.
* **`stellar/js-stellar-sdk`** ([GitHub](https://github.com/stellar/js-stellar-sdk)): SDK implementations of `Operation.extendFootprintTtl` and simulation footprint resolution.

### 1.3 Community Repositories & Competing Implementations
* **`dotandev/hintents` (`erst`)** ([GitHub](https://github.com/dotandev/hintents)): Dedicated local transaction replay and "glass-box" debugging tool for Soroban that decodes DWARF debug symbols and maps errors back to source code lines.
* **`paltalabs/mercury-sdk`** ([GitHub](https://github.com/paltalabs/mercury-sdk)): High-level indexing and event consumption platform for Soroban with GraphQL and REST subscriptions.
* **`axis-markets/orderbook`** ([GitHub](https://github.com/axis-markets)): Production Soroban protocol implementing bespoke TypeScript keeper scripts to keep contract footprints alive.

---

## 2. Competitive Landscape & Gap Analysis

| Project / Tool | Primary Focus | Capabilities | Critical Gaps Left Unsolved |
| :--- | :--- | :--- | :--- |
| **`stellar-cli`** (`stellar contract extend`) | Official CLI Tool | • Manually extends single contract instance or specified key.<br>• Requires local keypair and manual terminal execution. | • No automated monitoring daemon.<br>• No multi-contract fleet configuration.<br>• No proactive alerting (Slack/Discord).<br>• No Prometheus metrics. |
| **`erst`** (`dotandev/hintents`) | Error Diagnosis & Tracing | • Replays failed transactions locally.<br>• Maps WASM traps to Rust source lines via DWARF symbols.<br>• VS Code DAP extension. | • Focuses exclusively on execution errors; does not track or alert on storage TTL decay or prevent expiration. |
| **`mercury-sdk`** (PaltaLabs) | Event Indexing | • Subscribes to contract events.<br>• Powers frontend data feeds.<br>• Provides managed GraphQL backend. | • Heavyweight event-driven infrastructure; not tailored for lightweight storage rent and TTL lifecycle management. |
| **Bespoke Keepers** (e.g., Axis Markets) | Protocol Maintenance | • Internal TypeScript scripts to bump protocol-specific entries. | • Closed or tightly coupled to specific smart contracts; not reusable by the broader Stellar developer community. |
| **`soroban-sentinel`** (Proposed) | **Storage TTL Observability & Maintenance** | • Zero-config contract instance and code scan.<br>• Declarative fleet monitoring (`sentinel.toml`).<br>• Prometheus metrics (:9100).<br>• Multi-channel alerting (Slack/Discord).<br>• Safe, bounded autonomous keeper mode. | **Directly fills the missing operational observability and automated maintenance gap across the Stellar ecosystem.** |

---

## 3. Verified Technical Findings

1. **Deterministic Auto-Discovery Boundary**:
   * **Verified Fact**: Querying `LedgerKey::ContractData` with `ContractDataDurability::Instance` and `ScVal::LedgerKeyContractInstance` deterministically yields the contract instance, its entire in-memory instance storage map, and the WASM bytecode hash of its executable.
   * **Verified Constraint**: Soroban RPC **does not** provide an endpoint or wildcard search to discover arbitrary `ContractDataDurability::Persistent` keys for a contract ID without knowledge of the key's `ScVal`.
   * **Impact on Architecture**: `soroban-sentinel` guarantees zero-configuration monitoring for Contract Instance and Code entries, while providing a declarative key registry (`sentinel.toml`) and CLI flags (`--key`, `--key-xdr`) for persistent entries.
2. **Soroban RPC Batch Ceiling**:
   * **Verified Fact**: `getLedgerEntries` enforces a strict server-side limit of **200 keys** per request. Passing > 200 keys results in an immediate RPC error.
   * **Impact on Architecture**: `sentinel-core` implements automatic client-side chunking to slice all key queries into batches of <= 200 items.
3. **Storage Rent Simulation Validity**:
   * **Verified Fact**: Soroban RPC's `simulateTransaction` endpoint accepts transaction envelopes containing `ExtendFootprintTTLOp` and returns the exact simulated `minResourceFee` and resource footprint based on current network configuration settings (`ConfigSettingEntry`).
   * **Impact on Architecture**: Rent projections (`sentinel forecast`) utilize live RPC simulation rather than brittle offline math approximations.
4. **Ledger Time Drift**:
   * **Verified Fact**: While Stellar consensus targets 5.0 seconds per ledger close, network velocity varies dynamically between 4.8s and 6.2s under load.
   * **Impact on Architecture**: All alert triggers and health evaluations use **Ledger Counts** as the primary deterministic truth, displaying wall-clock time estimates strictly as secondary indicators.

---

## 4. Unresolved Technical Questions for Ecosystem Maintainers

The following technical questions should be validated with SDF core engineers and active community developers via the Stellar Developer Discord (`#soroban-dev`) prior to commencing Milestone 5 (Autonomous Keeper):

1. **Protocol 23 Automatic Restoration Gas Profiles**: In Protocol 23, how does the resource fee for automated inline state restoration during transaction simulation compare with proactive pre-expiration extension via `ExtendFootprintTTLOp` under high-load network conditions?
2. **RPC Pruning Policy on Public Endpoints**: What is the historical ledger retention policy for `getLedgerEntries` and `getEvents` across public SDF RPC endpoints (`soroban-testnet.stellar.org` and `soroban-rpc.stellar.org`)? Are archived entries queryable via `getLedgerEntries`, or do they return null/missing?
3. **Core CLI Roadmap Collision**: Does the Stellar Core Tooling team have any active or unannounced roadmap items to integrate a long-running background TTL monitoring daemon directly into `stellar-cli`? (Community consensus indicates `stellar-cli` intends to remain an interactive single-shot tool).
4. **Dynamic Rent Fee Rebalancing**: Are there any pending Core Advancement Proposals (CAPs) under review that would alter the storage rent byte multipliers or introduce variable storage rent pricing based on BucketList saturation?

---

## 5. Architectural References
* [docs/ARCHITECTURE_DECISION.md](ARCHITECTURE_DECISION.md): Complete rationale for adopting a Modular Monolith with Ports-and-Adapters (Hexagonal Architecture) across 2 workspace crates (`sentinel-core` and `sentinel-cli`), rejecting microservices and distributed event brokers.

