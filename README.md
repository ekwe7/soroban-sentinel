# Soroban Sentinel (`soroban-sentinel`)

> **Autonomous storage TTL monitoring, expiry-risk detection, and automated maintenance toolkit for Soroban smart contracts on the Stellar network.**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Status: MVP Ready](https://img.shields.io/badge/Status-v0.1.0%20(MVP%20Ready)-brightgreen.svg)](#project-status)
[![Architecture: Modular Monolith](https://img.shields.io/badge/Architecture-Modular%20Monolith%20(Hexagonal)-teal.svg)](docs/ARCHITECTURE_DECISION.md)
[![Stellar Protocol](https://img.shields.io/badge/Stellar%20Protocol-20%20%7C%2021%20%7C%2022%20%7C%2023-purple.svg)](https://stellar.org)
[![Ecosystem: Stellar Drips Wave](https://img.shields.io/badge/Ecosystem-Drips%20Wave%20Stellar-blueviolet.svg)](https://drips.network)

---

> [!NOTE]
> **MVP v0.1.0 Complete**: `soroban-sentinel` is implemented and verified across 71 offline unit/integration tests and live Soroban Testnet validation. The CLI binary `sentinel` supports deterministic contract scanning, persistent storage key verification, colorized ASCII reporting, and machine-readable JSON output.

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
| **CLI (MVP)** | Single Contract Scan (`sentinel scan <ID>`) | Protocol 20+ | **Completed (v0.1.0)** | [MVP Scope](docs/MVP_SCOPE.md#1-mvp-feature-set-v010) |
| **CLI (MVP)** | Terminal Table Output (`--format text`) | N/A | **Completed (v0.1.0)** | [MVP Scope](docs/MVP_SCOPE.md#1-mvp-feature-set-v010) |
| **CLI (MVP)** | Machine-Readable JSON Output (`--format json`) | N/A | **Completed (v0.1.0)** | [MVP Scope](docs/MVP_SCOPE.md#1-mvp-feature-set-v010) |
| **CLI (MVP)** | Explicit Persistent Key Verification (`--key`, `--key-xdr`) | Protocol 20+ | **Completed (v0.1.0)** | [MVP Scope](docs/MVP_SCOPE.md#1-mvp-feature-set-v010) |
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
   - The **Contract Code WASM Entry** referenced by the contract instance executable hash (or detects built-in Stellar asset contracts).
2. **Explicit Persistent Keys**: Because persistent entries (`ContractDataDurability::Persistent`) live as isolated ledger keys, operators query them via `--key <SYMBOL>` or `--key-xdr <BASE64>`.

---

## 5. Usage Guide & Output Examples

### Quickstart: Building from Source
```bash
# Clone and build release binary
git clone https://github.com/soroban-sentinel/soroban-sentinel.git
cd soroban-sentinel
cargo build --release -p sentinel-cli

# Run scan using compiled binary
./target/release/sentinel scan --help
```

### Scanning a Contract (Default Terminal Table)
```bash
# Inspect contract instance, WASM bytecode, and persistent storage keys
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --network testnet \
  --key ADMIN \
  --key COUNTER
```

*Actual Terminal Output:*
```text
Contract: CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC | Network: testnet | Latest Ledger: 5,303,220 (Velocity: 5.0s/ledger)
+-----------------------+------------+------------+-------------------+----------------------+--------+
| Entry Description     | Durability | Size Bytes | Live Until Ledger | Remaining Horizon    | Status |
+=====================================================================================================+
| Contract Instance     | Instance   | 240 B      | 8,305,943         | 3,002,723 (~173.8 d) | OK     |
| Persistent Key: ADMIN | Persistent | 64 B       | 8,305,943         | 3,002,723 (~173.8 d) | OK     |
| Persistent Key: COUNTER| Persistent | 48 B       | 8,305,943         | 3,002,723 (~173.8 d) | OK     |
+-----------------------+------------+------------+-------------------+----------------------+--------+
Summary: 3 entries checked. 3 OK, 0 WARN, 0 CRITICAL/ARCHIVED.
```

### Emitting Machine-Readable JSON (CI/CD Pipelines)
```bash
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --network testnet \
  --format json
```

*Output:*
```json
{
  "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
  "network": "testnet",
  "latest_ledger": 5303220,
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
      "remaining_ledgers": 3002723,
      "estimated_remaining_seconds": 15013615.0,
      "estimated_remaining_days": 173.77,
      "status": "ok"
    }
  ]
}
```

### Standard Process Exit Codes
`sentinel` sets standard UNIX exit codes suitable for CI/CD gates and automation:

| Exit Code | Meaning | Condition |
| :---: | :--- | :--- |
| `0` | **Success / OK** | All evaluated storage entries have `status == OK` |
| `1` | **Warning** | One or more evaluated storage entries have `status == WARN` |
| `2` | **Critical / Archived** | One or more evaluated entries have `status == CRITICAL` or `ARCHIVED` |
| `10` | **Invalid Arguments** | Malformed contract Strkey, unrecognized network, or invalid parameters |
| `11` | **RPC Failure** | Network timeout, connection refused, or HTTP 5xx/4xx RPC communication failure |

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
