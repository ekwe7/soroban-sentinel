# Minimum Viable Product (MVP) Scope Specification

**Project:** `soroban-sentinel`  
**Target Release:** v0.1.0  
**Architecture:** Modular Monolith (2 Crates: `sentinel-core` and `sentinel-cli`)  
**Document Status:** Approved Scope Baseline  

---

## 1. Executive Summary & Design Principle

The core objective of the **Soroban Sentinel MVP (v0.1.0)** is to deliver an immediately useful, rock-solid, production-grade CLI tool that eliminates contract expiration surprises for Soroban developers with minimal setup overhead.

### The Guiding Principle: "Do One Thing Flawlessly First"
Rather than introducing background schedulers, webhooks, Prometheus exporters, or keeper transaction signers on day one, the MVP focuses strictly on:
1. **Deterministic TTL Inspection**: Instant zero-config inspection of any deployed contract's instance and code TTL.
2. **Persistent Key Verification**: Auditing specific persistent keys via `--key` or `--key-xdr`.
3. **Decay & Risk Assessment**: Accurate remaining ledger calculation and risk level assignment (`OK`, `WARN`, `CRITICAL`, `ARCHIVED`).
4. **Ergonomic Outputs**: Human-readable colorized terminal tables (`--format text`) and machine-readable JSON (`--format json`).
5. **Quality Gates**: Comprehensive automated unit, mock RPC, and CLI integration tests.

---

## 2. Explicit Inclusion List (In-Scope for v0.1.0)

The MVP release is strictly bounded to the following deliverables:

### Core Capability 1: Deterministic Contract Scan (`sentinel scan <CONTRACT_ID>`)
* Automatic deterministic derivation and querying of:
  * **Contract Instance Key** (`durability: Instance`, `key: LedgerKeyContractInstance`).
  * **Contract Code WASM Key** (`LedgerKey::ContractCode`) linked in the instance executable.
* Extraction and display of variables inside the instance storage map.
* Calculation of remaining ledgers:
  $$\text{Remaining Ledgers} = \text{liveUntilLedgerSeq} - \text{latestLedgerSequence}$$
* Estimation of remaining time horizon based on standard $5.0\text{s/ledger}$ velocity.
* Risk Level assignment:
  * `OK`: $\ge 120,960\text{ ledgers}$ (~7 days).
  * `WARN`: $< 120,960\text{ ledgers}$ and $\ge 34,560\text{ ledgers}$ (~2 days).
  * `CRITICAL`: $< 34,560\text{ ledgers}$ and $> 0\text{ ledgers}$.
  * `ARCHIVED`: $\le 0\text{ ledgers}$ or entry omitted from live state.

### Core Capability 2: Persistent Key Verification (`--key` / `--key-xdr`)
* Ability to specify explicit persistent storage keys alongside the contract address.
* Conversion of simple `Symbol` strings or arbitrary base64 XDR keys into `LedgerKey::ContractData` with `durability: Persistent`.
* Status verification: Active (with remaining ledgers), Expired/Archived, or Missing.

### Core Capability 3: Ergonomic Output Formats
* **`--format text` (Default)**: Colorized ANSI terminal table via `comfy-table` with summary status.
* **`--format json`**: Strict, typed JSON schema for integration into automated CI/CD pipelines and scripts.

### Core Capability 4: Network Configuration & Resilience
* Network presets via `--network` (`testnet`, `mainnet`, `futurenet`, `local`) or custom `--rpc-url`.
* Automatic request chunking enforcing the <= 200 key limit per `getLedgerEntries` call.
* Exponential backoff and retry on transient HTTP 429 and 503 errors.

### Core Capability 5: Automated Test Harnesses
* Unit tests for pure domain logic and XDR derivation.
* Mock RPC integration tests using `wiremock`.
* CLI process integration tests using `assert_cmd`.

---

## 3. Explicit Exclusion List (Post-MVP Roadmap)

The following capabilities are explicitly excluded from the v0.1.0 MVP release:

* ❌ **Fleet Configuration File (`sentinel.toml`)**: Multi-contract file manifests deferred to Phase 4.
* ❌ **Continuous Watch Scheduler (`sentinel watch`)**: Background monitoring loop deferred to Phase 4.
* ❌ **Markdown Output Formatter (`--format markdown`)**: Deferred to Phase 3.
* ❌ **Webhook Alert Dispatchers (Slack, Discord)**: Deferred to Phase 4.
* ❌ **Prometheus Telemetry Exporter (`:9100/metrics`)**: Deferred to Phase 4.
* ❌ **Rent Cost Forecasting (`sentinel forecast`)**: Deferred to Phase 3.
* ❌ **Autonomous Keeper Transaction Signer (`sentinel bump`)**: Deferred to Phase 5.
* ❌ **Historical Event Key Discovery (`getEvents`)**: Deferred to Phase 6.

---

## 4. CLI Command Specification, Inputs, Outputs, and Exit Codes

### Command: `sentinel scan`

#### Usage
```bash
sentinel scan <CONTRACT_ID> [OPTIONS]
```

#### Inputs & Options
* `<CONTRACT_ID>` *(Required positional)*: Strkey-encoded contract address (`C...` 56-character string).
* `--rpc-url <URL>`: Soroban JSON-RPC 2.0 endpoint (default: `https://soroban-testnet.stellar.org`).
* `--network <NETWORK>` / `-n`: Network preset (`testnet` [default], `mainnet`, `futurenet`, `local`).
* `--key <KEY>`: Optional symbol string of a persistent storage key to query alongside the instance.
* `--key-xdr <BASE64_XDR>`: Optional base64-encoded XDR `ScVal` key.
* `--format <FORMAT>`: Output format (`text` [default] or `json`).
* `--threshold <LEDGERS>`: Custom warning threshold in ledgers (default: `120960`).

#### Expected Success Output (`--format text`)
```
+-------------------------------------------------------------------------------------------------------+
| Contract: CA3D5NXQ6DNG... | Network: testnet | Latest Ledger: 5,420,100 (Velocity: 5.0s/ledger)       |
+----------------------+------------+------------+--------------------+-------------------+-------------+
| Entry Description    | Durability | Size Bytes | Live Until Ledger  | Remaining Horizon | Status      |
+----------------------+------------+------------+--------------------+-------------------+-------------+
| Contract Instance    | Instance   | 342 B      | 5,530,100          | 110,000 (~6.3 d)  | WARN        |
| Contract Code (WASM) | Code       | 18,450 B   | 5,530,100          | 110,000 (~6.3 d)  | WARN        |
| Persistent Key: Admin| Persistent | 64 B       | 5,800,000          | 379,900 (~21.9 d) | OK          |
+----------------------+------------+------------+--------------------+-------------------+-------------+
Summary: 3 entries checked. 2 WARN, 1 OK, 0 CRITICAL/ARCHIVED.
```

#### Expected Success Output (`--format json`)
```json
{
  "contract_id": "CA3D5NXQ6DNG...",
  "network": "testnet",
  "latest_ledger": 5420100,
  "ledger_velocity_seconds": 5.0,
  "summary": {
    "total_entries": 3,
    "ok": 1,
    "warn": 2,
    "critical": 0,
    "archived": 0
  },
  "entries": [
    {
      "identifier": "instance",
      "entry_type": "contract_instance",
      "durability": "instance",
      "size_bytes": 342,
      "live_until_ledger": 5530100,
      "remaining_ledgers": 110000,
      "estimated_remaining_seconds": 550000,
      "estimated_remaining_days": 6.36,
      "status": "warn"
    },
    {
      "identifier": "code_wasm",
      "entry_type": "contract_code",
      "durability": "code",
      "size_bytes": 18450,
      "live_until_ledger": 5530100,
      "remaining_ledgers": 110000,
      "estimated_remaining_seconds": 550000,
      "estimated_remaining_days": 6.36,
      "status": "warn"
    },
    {
      "identifier": "persistent:Admin",
      "entry_type": "contract_data",
      "durability": "persistent",
      "size_bytes": 64,
      "live_until_ledger": 5800000,
      "remaining_ledgers": 379900,
      "estimated_remaining_seconds": 1899500,
      "estimated_remaining_days": 21.98,
      "status": "ok"
    }
  ]
}
```

#### UNIX Exit Code Behavior
* `0`: All evaluated entries have `status == OK`.
* `1`: One or more evaluated entries have `status == WARN`.
* `2`: One or more evaluated entries have `status == CRITICAL` or `status == ARCHIVED`.
* `10`: Invalid CLI arguments or malformed Strkey address.
* `11`: RPC communication failure or timeout.

---

## 5. Concrete Acceptance Criteria for MVP Release

| Criterion ID | Requirement | Verification Method |
| :--- | :--- | :--- |
| **AC-1** | `sentinel scan <VALID_ID> --network testnet` queries active Testnet contract and correctly reports instance and code TTL | Automated testnet integration test |
| **AC-2** | Passing `--format json` outputs valid JSON adhering to published schema | JSON schema validation test |
| **AC-3** | Derivation of `ContractCode` matches the WASM hash embedded in the instance executable | Unit test asserting XDR byte equality |
| **AC-4** | Supplying `--key <SYMBOL>` derives correct `LedgerKey::ContractData` with persistent durability | Unit test verifying XDR serialization |
| **AC-5** | Querying > 200 keys splits requests into chunks of <= 200 items | Mock RPC test with 250 mock keys |
| **AC-6** | Malformed contract IDs are rejected with clear error message and Exit Code `10` | CLI integration test via `assert_cmd` |
| **AC-7** | Transient HTTP 429/503 errors trigger exponential backoff and retry | Mock RPC integration test via `wiremock` |
| **AC-8** | 100% passing tests and zero warnings under `cargo clippy --workspace --all-targets -- -D warnings` | GitHub Actions CI workflow |
