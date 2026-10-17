# Product Requirements Document (PRD) — Soroban Sentinel

**Project Name:** `soroban-sentinel`  
**Current Phase:** Specification & Validation Phase  
**Architecture:** Modular Monolith (Ports & Adapters)  
**Target Release:** v0.1.0 (MVP)  
**Target Ecosystem:** Stellar Network / Soroban Smart Contracts  
**Community Program Alignment:** Drips Wave Stellar Program  

---

## 1. Problem Statement & Verifiable Evidence

### 1.1 The Protocol Reality
In the Stellar network's smart contract environment (Soroban), state is not permanent. Introduced in [CAP-0046 (State Archival)](https://github.com/stellar/stellar-protocol/blob/master/core/cap-0046.md) and implemented across Protocols 20, 21, 22, and 23:
* Every ledger entry (`ContractData`, `ContractCode`) is assigned a Time-To-Live (TTL) tracked via an on-chain `TTLEntry` containing `liveUntilLedgerSeq`.
* Ledgers close on the Stellar consensus grid roughly every 5.0 seconds (~17,280 ledgers per day, ~120,960 ledgers per week).
* When the network's `latestLedgerSequence` exceeds an entry's `liveUntilLedgerSeq`:
  * **Temporary Entries** (`ContractDataDurability::Temporary`) are **permanently and irreversibly deleted**.
  * **Persistent Entries** (`ContractDataDurability::Persistent`) and **Instance Entries** (`ContractDataDurability::Instance`) are **archived** (evicted from the live BucketList into cold storage).

### 1.2 The Operational Pain Points
1. **Contract Inaccessibility**: When a contract's instance or code entry expires, users and integrated dApps attempting to call its functions encounter execution failures (`HostError: Error(Storage, MissingValue)`).
2. **Restoration Complexity & Gas Penalties**: Restoring archived persistent state requires a `RestoreFootprintOp` transaction. While Protocol 23 introduces automated restoration during transaction simulation, auto-restoration incurs substantial resource surcharges (read/write fees and rent restoration) and increases transaction footprint byte size, risking execution rejection if the transaction exceeds block resource limits.
3. **Flawed Defensive Bumping**: Many Soroban smart contracts implement defensive TTL extension logic within internal contract methods (`env.storage().instance().extend_ttl(...)`). This creates three distinct failure modes:
   * **Inactivity Risk**: If a contract experiences a period of low transaction volume (e.g., market dormancy or niche governance), no user calls the contract, and entries expire silently.
   * **Unfair Fee Shifting**: Random end-users are forced to pay the rent extension costs of the protocol through inflated transaction resource fees.
   * **Footprint Bloat**: Adding TTL extension operations into ordinary business logic transactions expands the footprint and CPU compute budget required for routine calls.
4. **Lack of Dedicated Observability**:
   * Official CLI (`stellar-cli`): Offers `stellar contract extend` and `stellar contract restore`. These are single-shot, interactive CLI tools that require manual operator execution. They provide no automated continuous monitoring, no multi-contract fleet management, no metric export, and no alerting mechanisms.
   * Ecosystem Explorers: Web-based block explorers show point-in-time entry details for specific keys if queried, but offer no proactive alert triggers, no webhook dispatch, and no machine-readable CLI interfaces for pipelines.

---

## 2. Target Users & Personas

### Persona A: The Smart Contract Protocol Operator
* **Profile**: Core engineer or DevOps lead at a DeFi protocol (DEX, lending market, yield aggregator, or liquid staking token) deployed on Stellar Mainnet.
* **Pain Point**: Operates 5–20 interconnected contracts (core pool, router, token wrappers, factory, oracle adapter). Constantly worries that a quiet market will cause secondary pool entries or oracle instances to archive without warning.
* **Need**: A reliable CLI tool to audit deployed contracts locally and in CI/CD, with an upgrade path toward continuous monitoring and alerting.

### Persona B: The Independent dApp Builder & Hackathon Alum
* **Profile**: Solo developer or small team with a live product on Soroban Testnet or Mainnet.
* **Pain Point**: Cannot afford a 24/7 dedicated DevOps infrastructure. Needs a quick, zero-friction CLI command to audit the health of deployed contracts before each release.
* **Need**: Running `sentinel scan <CONTRACT_ID> --network testnet` in terminal or GitHub Actions to verify TTL health and emit formatted summaries or JSON.

### Persona C: The DAO / Treasury Custodian
* **Profile**: Administrator of a multisig or DAO governance vault holding institutional funds or escrow agreements.
* **Pain Point**: Contracts have low transactional frequency by design (e.g., voting happens quarterly). Manual checks are error-prone.
* **Need**: Transparent cost projections to understand rent liability and plan operational budgets.

---

## 3. User Stories & Use Cases

### User Story 1: Fast CLI Contract Inspection (MVP Baseline)
* **As a** smart contract developer,  
* **I want to** pass a contract ID to a CLI command without providing private keys,  
* **So that I can** immediately see the expiration ledger, remaining time horizon, and WASM code status in my terminal.

### User Story 2: CI/CD Pipeline TTL Health Check (MVP Baseline)
* **As a** protocol engineer,  
* **I want to** run a health check command with `--format json` inside GitHub Actions,  
* **So that I can** fail PR builds or post automated bot comments if deployed contracts have less than 100,000 ledgers of TTL remaining.

### User Story 3: Explicit Key Health Check (MVP Baseline)
* **As a** protocol developer,  
* **I want to** pass explicit persistent storage keys via `--key` or `--key-xdr`,  
* **So that I can** verify that specific protocol variables (e.g., Admin, Vault balance) are live and check their remaining lifespan.

### User Story 4: Cost Projection & Budgeting (Post-MVP)
* **As a** DAO treasurer,  
* **I want to** forecast the XLM and stroop cost required to extend a contract's TTL for 6 months (approx. 3,153,600 ledgers),  
* **So that I can** allocate appropriate operational reserves for contract upkeep.

### User Story 5: Proactive Continuous Alerting (Post-MVP)
* **As a** protocol DevOps engineer,  
* **I want to** run `sentinel watch` with a `sentinel.toml` file,  
* **So that I am** notified days or weeks in advance via Slack or Discord before any monitored entry archives.

---

## 4. Functional Requirements

### 4.1 Phase 1: Core MVP Scope (v0.1.0)

* **FR-1.1: Single Contract Deterministic Scan (`sentinel scan`)**:
  * Input: Contract address (`C...`), RPC URL (or `--network testnet|mainnet|local`), optional custom threshold.
  * Deterministic Discovery: Automatically queries and resolves:
    1. Contract Instance Entry (`LedgerKey::ContractData` with `durability: Instance` and `key: LedgerKeyContractInstance`).
    2. Embedded Instance Storage Map (`ContractDataEntry.val.ContractInstance.storage`).
    3. Contract WASM Code Entry (`LedgerKey::ContractCode` matching instance executable hash).
  * Output: Displays entry byte sizes, `liveUntilLedgerSeq`, current network ledger, remaining ledgers, estimated days to expiry, and computed risk level.

* **FR-1.2: Persistent Key Verification (`--key` / `--key-xdr`)**:
  * Input: Contract address, one or more specific persistent key symbols (`--key <SYMBOL>`) or base64 XDR representations (`--key-xdr <BASE64>`).
  * Resolution: Derives `LedgerKey::ContractData` for each key and queries the Soroban RPC.
  * Output: Status of each specified key (Live with remaining ledgers, Archived, or Missing).

* **FR-1.3: Risk Level Evaluation**:
  * Evaluates each entry against standard thresholds:
    * `OK`: `remaining_ledgers >= warning_threshold` (default: 120,960 ledgers / ~7 days).
    * `WARN`: `critical_threshold <= remaining_ledgers < warning_threshold` (default: 34,560 ledgers / ~2 days).
    * `CRITICAL`: `0 < remaining_ledgers < critical_threshold`.
    * `ARCHIVED`: `remaining_ledgers == 0` or entry expired on-chain.

* **FR-1.4: Output Formatting & UNIX Exit Codes**:
  * Supports `--format text` (colorized terminal ASCII table) and `--format json` (strict machine-readable JSON schema).
  * UNIX Exit Codes:
    * `0`: All inspected entries are `OK`.
    * `1`: One or more entries are in `WARN` state.
    * `2`: One or more entries are `CRITICAL` or `ARCHIVED`.
    * `10`: Invalid user input (e.g., malformed contract Strkey).
    * `11`: RPC communication failure or timeout.

### 4.2 Phase 2: Post-MVP Roadmap Features (v0.2.0+)

* **FR-2.1: Rent Cost Simulation (`sentinel forecast`)**:
  * Constructs an `ExtendFootprintTTLOp` transaction envelope with the target entries in the `readOnly` footprint.
  * Submits to `simulateTransaction` on Soroban RPC to return exact `minResourceFee` and calculated XLM costs.
* **FR-2.2: Markdown Output Formatter (`--format markdown`)**:
  * Generates clean GitHub-flavored Markdown tables for CI/CD PR comments.
* **FR-2.3: Fleet Watch Mode (`sentinel watch`)**:
  * Continuous background monitoring loop driven by declarative `sentinel.toml`.
  * Emits in-process domain events via Tokio channels.
* **FR-2.4: In-Process Webhook Alerts**:
  * Subscribes to threshold breach events and dispatches HTTP POST payloads to Slack or Discord.
* **FR-2.5: Prometheus Metrics Exporter**:
  * Exposes HTTP `/metrics` on port 9100.
* **FR-2.6: Autonomous Keeper Mode (`sentinel bump`)**:
  * Broadcasts signed `ExtendFootprintTTLOp` transactions with daily spend limits.

---

## 5. Non-Functional Requirements (NFRs)

* **NFR-1: Reliability & Fault Tolerance**:
  * The tool must handle temporary RPC timeouts, network drops, and HTTP 429/503 errors gracefully using exponential backoff with jitter.
* **NFR-2: Performance & RPC Efficiency**:
  * Key batching: Slices key queries into chunks of maximum 200 keys per RPC request (satisfying the Soroban RPC batch limit).
  * Execution Speed: Single contract scan executes in `< 1.5 seconds` under standard broadband network latency.
* **NFR-3: Security & Non-Custodial Design**:
  * **100% Read-Only in MVP**: No private keys, seed phrases, or signatures are required. Operates against public unauthenticated JSON-RPC endpoints.
* **NFR-4: Architectural Integrity & Maintainability**:
  * Modular Monolith adhering to Ports-and-Adapters. Domain logic has zero dependencies on I/O, RPC clients, or CLI formatters.
  * 100% passing automated tests, `cargo fmt`, and strict linting (`cargo clippy -- -D warnings`).

---

## 6. MVP Acceptance Criteria (Definition of Done for v0.1.0)

To achieve MVP completion and readiness for initial Drips Wave distribution:

1. **CLI Scan Execution**:
   * Running `sentinel scan <CONTRACT_ID> --network testnet` against any active Soroban Testnet contract succeeds.
   * Accurately extracts the Contract Instance and the Contract Code WASM hash.
   * Correctly calculates remaining ledgers and estimated days to expiry.
2. **Persistent Key Verification**:
   * Running `sentinel scan <CONTRACT_ID> --key <SYMBOL>` properly queries and reports the persistent key status.
3. **Deterministic Output & Schema**:
   * Running `sentinel scan <CONTRACT_ID> --format json` produces valid JSON adhering to the published schema.
4. **Resilience & Batching**:
   * Querying > 200 keys splits requests into chunks of <= 200 items without triggering RPC batch limit errors.
5. **Quality Gates**:
   * 100% passing unit tests (offline domain logic) and mock RPC tests (`wiremock`).
   * Zero clippy warnings across the workspace.

---

## 7. Out-of-Scope for MVP (Explicit Exclusions)

The following capabilities are explicitly deferred to post-MVP releases:
* ❌ Continuous background scheduling / daemon mode (`sentinel watch`).
* ❌ Webhook alerts (Slack, Discord) and alert deduplication state machines.
* ❌ Prometheus metrics exporter (`:9100/metrics`).
* ❌ Rent simulation (`sentinel forecast`).
* ❌ Autonomous keeper transaction signing (`sentinel bump`).
* ❌ Historical contract event parsing (`getEvents`).
* ❌ State restoration transactions (`RestoreFootprintOp`).

---

## 8. Success Metrics & Validation Plan

| Metric | Target Goal (v0.1.0 MVP) | Measurement Method |
| :--- | :--- | :--- |
| **Inspection Accuracy** | 100% match with Soroban RPC `liveUntilLedgerSeq` | Automated integration test against live Testnet contracts |
| **CLI Execution Speed** | `< 1.5 seconds` for single contract scan | Benchmark on standard broadband connection |
| **Test Coverage** | `> 85%` line coverage on `sentinel-core` | `cargo-tarpaulin` or `llvm-cov` |
| **Community Backlog** | 6–8 approved issues claimed on Drips Wave | Drips Wave repository dashboard |
