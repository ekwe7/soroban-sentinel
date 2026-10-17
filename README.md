# Soroban Sentinel (`soroban-sentinel`)

> **Autonomous storage TTL monitoring, expiry-risk detection, and automated maintenance toolkit for Soroban smart contracts on the Stellar network.**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Status: Planning & Specification](https://img.shields.io/badge/Status-Specification%20Phase-orange.svg)](#project-status)
[![Architecture: Modular Monolith](https://img.shields.io/badge/Architecture-Modular%20Monolith%20(Hexagonal)-teal.svg)](docs/ARCHITECTURE_DECISION.md)
[![Stellar Protocol](https://img.shields.io/badge/Stellar%20Protocol-20%20%7C%2021%20%7C%2022%20%7C%2023-purple.svg)](https://stellar.org)
[![Ecosystem: Stellar Drips Wave](https://img.shields.io/badge/Ecosystem-Drips%20Wave%20Stellar-blueviolet.svg)](https://drips.network)

---

> [!IMPORTANT]
> **Specification Notice**: `soroban-sentinel` is currently in the **Project Validation and Specification Phase**. This repository contains complete architectural specifications, product requirements, and contribution plans. Core implementation is scheduled according to the [Implementation Roadmap](docs/IMPLEMENTATION_ROADMAP.md). **No application binary or crate is yet published.**

---

## 1. Problem Statement

On Soroban (Stellar's smart contract platform), ledger state is **ephemeral by default** ([CAP-0046: State Archival](https://github.com/stellar/stellar-protocol/blob/master/core/cap-0046.md)). Unlike the Ethereum Virtual Machine (EVM) where storage persists permanently without ongoing maintenance, Soroban ledger entries—including **Contract Code (WASM)**, **Contract Instance Storage**, and **Persistent Storage Entries**—carry a Time-To-Live (TTL) counter tied directly to Stellar ledger sequences.

### The Operational Challenge
1. **Catastrophic Contract Freezing**: If a contract instance or contract code entry reaches its TTL expiration without being extended, it transitions into an archived state. Subsequent transaction invocations against that contract fail with host execution errors.
2. **Permanent Data Loss for Temporary Storage**: Entries stored with `Temporary` durability cannot be restored once expired; they are permanently purged from ledger state.
3. **Restoration Friction and Fee Penalties**: While Protocol 23 introduces automatic state restoration mechanisms during simulation, restoring archived state from cold storage carries heavy write and disk I/O fees, expands transaction footprint sizes, and risks exceeding network resource limits.
4. **Flawed Defensive In-Contract Bumping**: Existing dApps often attempt to defend against expiration by hardcoding `env.storage().instance().extend_ttl(...)` calls inside their core transactional methods. This imposes unpredictable gas overhead on everyday users and completely fails for contracts experiencing periods of low volume or dormancy (e.g., automated market maker pools during quiet markets, governance vaults, or escrow contracts).
5. **Lack of Dedicated Observability**: Existing developer tooling (`stellar-cli`) provides manual, single-key execution commands (`stellar contract extend`, `stellar contract restore`), but provides **no automated monitoring, no proactive alerting (Slack/Discord/webhooks), and no Prometheus metrics integration**.

---

## 2. What Soroban Sentinel Does

`soroban-sentinel` bridges this operational gap by providing an open-source, operator-grade observability and maintenance suite designed specifically for Soroban storage economics.

It is structured as a **modular monolith** adhering to **hexagonal architecture (ports and adapters)**, distributed initially as a single, lightweight CLI binary.

```
+-----------------------------------------------------------------------------------+
|                            TARGET STELLAR NETWORK                                 |
|                                                                                   |
|     +-------------------+                         +-------------------------+     |
|     | Soroban RPC Node  |                         |  Stellar Ledger State   |     |
|     | (JSON-RPC 2.0)    |<----------------------->|  (BucketList & TTLs)    |     |
|     +-------------------+                         +-------------------------+     |
+----------------\------------------------------------------------------------------+
                  \                                             
                   \ (getLedgerEntries, getLatestLedger)       
                    v                                         
+-----------------------------------------------------------------------------------+
|                        SOROBAN SENTINEL CLI BINARY                                |
|                                                                                   |
|  +-----------------------------------------------------------------------------+  |
|  | sentinel-cli (Driving Adapters & CLI Entry Point)                           |  |
|  |                                                                             |  |
|  |  • Command Parser: clap 4 (sentinel scan, forecast, watch)                  |  |
|  |  • Formatter Adapters: Terminal Table (comfy-table), JSON (serde), Markdown |  |
|  |  • Configuration Adapter: sentinel.toml loader (Future Watch Mode)         |  |
|  +-------------------------------------+---------------------------------------+  |
|                                        |                                          |
|                                        | Invokes Use Cases via Traits             |
|                                        v                                          |
|  +-----------------------------------------------------------------------------+  |
|  | sentinel-core (Pure Domain, Ports & Driven Adapters)                        |  |
|  |                                                                             |  |
|  |  [ Application Use Cases ]                                                  |  |
|  |    • ScanContractUseCase, ForecastRentUseCase                               |  |
|  |                                                                             |  |
|  |  [ Domain Core - Pure Logic, Zero I/O ]                                     |  |
|  |    • StorageEntry, TTL Decay Math, RiskLevel (Ok, Warn, Crit, Archived)     |  |
|  |                                                                             |  |
|  |  [ Ports - Abstract Traits ]                                                |  |
|  |    • trait LedgerReader, trait SimulationEngine                             |  |
|  |                                                                             |  |
|  |  [ Driven Adapters ]                                                        |  |
|  |    • SorobanRpcClient: JSON-RPC 2.0 client (implements LedgerReader)        |  |
|  |    • XdrCodec: stellar-xdr deterministic key derivation                     |  |
|  +-----------------------------------------------------------------------------+  |
+-----------------------------------------------------------------------------------+
```

---

## 3. Project Status & Feature Matrix

The following table distinguishes between MVP release features and post-MVP roadmap milestones:

| Component | Feature | Protocol Level | Implementation Status | Tracking Reference |
| :--- | :--- | :--- | :--- | :--- |
| **CLI (MVP)** | Single Contract Scan (`sentinel scan <ID>`) | Protocol 20+ | **Planned (MVP - Sprint 2)** | [MVP Scope](docs/MVP_SCOPE.md#1-mvp-feature-set-v010) |
| **CLI (MVP)** | Terminal Table Output (`--format text`) | N/A | **Planned (MVP - Sprint 2)** | [MVP Scope](docs/MVP_SCOPE.md#1-mvp-feature-set-v010) |
| **CLI (MVP)** | Machine-Readable JSON Output (`--format json`) | N/A | **Planned (MVP - Sprint 2)** | [MVP Scope](docs/MVP_SCOPE.md#1-mvp-feature-set-v010) |
| **CLI (MVP)** | Explicit Persistent Key Verification (`--key`) | Protocol 20+ | **Planned (MVP - Sprint 2)** | [MVP Scope](docs/MVP_SCOPE.md#1-mvp-feature-set-v010) |
| **CLI** | Markdown Output (`--format markdown`) | N/A | **Planned (Sprint 3)** | [Roadmap](docs/IMPLEMENTATION_ROADMAP.md#phase-3) |
| **CLI** | Rent Simulation (`sentinel forecast <ID>`) | Protocol 20+ | **Planned (Sprint 3)** | [Roadmap](docs/IMPLEMENTATION_ROADMAP.md#phase-3) |
| **CLI** | Fleet Watch Mode (`sentinel watch --config`) | Protocol 20+ | **Planned (Sprint 4)** | [Roadmap](docs/IMPLEMENTATION_ROADMAP.md#phase-4) |
| **CLI** | In-Process Webhook Alerts (Slack/Discord) | N/A | **Planned (Sprint 4)** | [Roadmap](docs/IMPLEMENTATION_ROADMAP.md#phase-4) |
| **CLI** | Prometheus Metrics Exporter (`:9100`) | N/A | **Planned (Sprint 4)** | [Roadmap](docs/IMPLEMENTATION_ROADMAP.md#phase-4) |
| **Core** | Autonomous Keeper Bumper (`sentinel bump`) | Protocol 20+ | **Planned (Sprint 5)** | [Roadmap](docs/IMPLEMENTATION_ROADMAP.md#phase-5) |
| **Core** | Event-Based Key Discovery (`getEvents`) | Protocol 20+ | **Future Scope (v0.3.0)** | [PRD](docs/PRD.md#out-of-scope-for-mvp) |

---

## 4. Key Protocol Constraints & Architectural Realities

### The Arbitrary Key Enumeration Constraint
A critical technical constraint in Soroban is that **Soroban RPC (`getLedgerEntries`) does not support wildcard queries, prefix scans, or dumping all storage entries for an arbitrary contract ID**. Storage keys are hashed and indexed individually within the network's BucketList.

To operate soundly without making unsupported protocol assumptions:
1. **Zero-Configuration Scope (MVP)**: Given only a Contract ID (`C...`), `soroban-sentinel` deterministically inspects:
   - The **Contract Instance Ledger Entry** (`SCVal::LedgerKeyContractInstance`).
   - All key-values stored inside the **Contract Instance Storage Map** (`ContractDataEntry.val.instance.storage`).
   - The **Contract Code WASM Entry** referenced by the contract instance executable hash.
2. **Explicit Persistent Keys**: Because persistent entries (`ContractDataDurability::Persistent`) live as isolated ledger keys, developers query them via `--key <SYMBOL>` or `--key-xdr <BASE64>`, or declare them in `sentinel.toml` during watch mode.

---

## 5. Planned Usage Examples (Design Preview)

### Scanning a Contract's Instance & Code TTL (MVP)
```bash
# Inspect the Contract Instance, internal instance storage, and WASM binary
sentinel scan CA3D5NX... --network testnet
```

*Example Planned Terminal Output:*
```
+----------------------------------------------------------------------------------------------------+
| Target: CA3D5NX... | Network: testnet | Current Ledger: 5,420,100                                  |
+----------------------+------------+------------------+-------------------+-------------------------+
| Entry Type           | Durability | Byte Size        | Live Until Ledger | Remaining Horizon       |
+----------------------+------------+------------------+-------------------+-------------------------+
| Contract Instance    | Instance   | 342 B            | 5,530,100         | 110,000 ledgers (~6.3 d)|
| Contract Code (WASM) | Code       | 18,450 B         | 5,530,100         | 110,000 ledgers (~6.3 d)|
| Instance Storage [0] | Instance   | (in instance)    | 5,530,100         | 110,000 ledgers (~6.3 d)|
+----------------------+------------+------------------+-------------------+-------------------------+
Status: WARN (TTL drops below 7-day safety threshold in ~6.3 days)
```

### Emitting Machine-Readable JSON for CI/CD (MVP)
```bash
sentinel scan CA3D5NX... --format json --network testnet
```

### Simulating TTL Extension Rent Cost (Post-MVP)
```bash
sentinel forecast CA3D5NX... --extend-by 200000 --network testnet
```

---

## 6. Repository Documentation Suite

For complete architectural specifications, product requirements, and contribution plans:

* [Architecture Decision Record (ADR)](docs/ARCHITECTURE_DECISION.md) — Modular monolith evaluation, Ports-and-Adapters structure, and 2-crate workspace layout.
* [Product Requirements Document (PRD)](docs/PRD.md) — Problem statement, user personas, functional/non-functional requirements, and success metrics.
* [Technical Design Document](docs/TECHNICAL_DESIGN.md) — Hexagonal architecture, domain ports, XDR serialization, and rate-limit handling.
* [MVP Scope Specification](docs/MVP_SCOPE.md) — Strictly bounded first release, CLI input/output specifications, and acceptance criteria.
* [Implementation Roadmap](docs/IMPLEMENTATION_ROADMAP.md) — Ordered sprint phases, milestone deliverables, and effort estimates.
* [Testing Strategy](docs/TESTING_STRATEGY.md) — Isolated domain unit tests, mock RPC tests, and CLI acceptance harnesses.
* [Security & Operational Safety](docs/SECURITY.md) — Threat model, read-only guarantees, secret management, and keeper transaction limits.
* [Open Source Contribution Plan](docs/OPEN_SOURCE_CONTRIBUTION_PLAN.md) — Drips Wave backlog, complexity point breakdown, and contribution workflow.
* [Research & References](docs/RESEARCH_AND_REFERENCES.md) — Verifiable findings, official Stellar source code references, and competitive analysis.

---

## 7. Contributing to Soroban Sentinel

`soroban-sentinel` is designed to be a premier collaborative project within the **Stellar Drips Wave Program**. We maintain a transparent backlog of well-scoped issues categorized by difficulty:
* **Trivial (100 Points)**: JSON/Markdown output formatters, CLI parameter validation, documentation improvements.
* **Medium (150 Points)**: Storage key parsers, webhook payload builders, Prometheus metric collectors.
* **High (200 Points)**: RPC rate-limiting middleware, transaction simulation fee estimator, autonomous keeper safety bounds.

Please read our [Contribution Plan](docs/OPEN_SOURCE_CONTRIBUTION_PLAN.md) and review the [Security Policy](docs/SECURITY.md) before submitting code.

---

## 8. License

`soroban-sentinel` is licensed under the [MIT License](LICENSE).
