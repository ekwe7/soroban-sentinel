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
|  |  • Formatter Adapters: Terminal Table (comfy-table), JSON, CSV, Markdown |  |
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
| **CLI** | Flat CSV Export (`--format csv`, `--output`) | N/A | **Available** | Issue #55 |
| **CLI** | Flat Parquet Export (`--format parquet`, `--output`) | N/A | **Available (optional `parquet` feature)** | Issue #55 |
| **CLI (MVP)** | Explicit Persistent Key Verification (`--key`, `--key-xdr`) | Protocol 20+ | **Completed (v0.1.0)** | [MVP Scope](docs/MVP_SCOPE.md#1-mvp-feature-set-v010) |
| **CLI** | Interactive TUI Dashboard (`sentinel scan --tui`) | N/A | **Available (optional `tui` feature)** | Issue #42 |
| **CLI** | Offline Snapshot Replay (`sentinel scan --snapshot <PATH>`) | N/A | **Available** | Issue #70 |
| **CLI** | Markdown Output (`--format markdown`) | N/A | **Planned (Sprint 3)** | [Roadmap](docs/IMPLEMENTATION_ROADMAP.md#phase-3) |
| **CLI** | Rent Simulation (`sentinel forecast <ID>`) | Protocol 20+ | **Planned (Sprint 3)** | [Roadmap](docs/IMPLEMENTATION_ROADMAP.md#phase-3) |
| **CLI** | Archived State Restoration (`sentinel restore <ID>`) | CAP-0046 | **Available** | Issue #40 |
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

### Check a fleet from a manifest

Copy `sentinel.example.toml` to `sentinel.toml` and replace the sample contract IDs
with your own. Run a single check in manifest order.

```bash
sentinel check --config sentinel.toml
sentinel check --config sentinel.toml --format json
```

The optional `[defaults]` table sets the network, public RPC URL, and warning
and critical thresholds in ledgers. Each `[[contracts]]` table requires an
`id` and can specify a `name`, `keys`, `key_xdr`, and either threshold.
`SOROBAN_RPC_URL` overrides the manifest endpoint. Set credentials in the
environment. The command validates the entire manifest before querying the
network and accepts up to 100 contracts in a file of up to 256 KiB.

Text shows each contract and an overall summary. JSON contains an ordered
`contracts` array and a `summary` object with counts by outcome. Every contract
result contains `id`, `name`, `status`, `report`, and `error` fields. Unavailable
values are null.

The command continues after an individual RPC failure. Any RPC failure takes
precedence and exits 11. Otherwise the worst contract result determines the
exit code (0 for OK, 1 for warning, 2 for critical or archived). Invalid input
exits 10 before any RPC requests.

### Quickstart: Building from Source
```bash
# Clone and build release binary
git clone https://github.com/ekwe7/soroban-sentinel.git
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

### Composite Storage Key Expressions (`--key-expr`)
Soroban contracts commonly key map storage with composite enum types like `DataKey::Balance(Address)`. These encode as nested vectors (`ScVal::Vec([Symbol("Balance"), Address(..)])`). Instead of hand-crafting base64 XDR for `--key-xdr`, you can query them using `--key-expr`:

```bash
# Single symbol or address key expression
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --key-expr "sym:Balance"

# Composite tuple / map key with nested vector and contract address
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --key-expr "vec[sym:Balance, addr:CA3D5NXQ6DNG5BB25US67P3J6R74V4A7N5E55LFEV74J5R4R37E2L6G5]"
```

#### Supported Key Expression Grammar:
| Prefix | Description | Example |
| :--- | :--- | :--- |
| `sym:` | Symbol (up to 32 alphanumeric/underscore characters) | `sym:Balance` |
| `str:` | UTF-8 String | `str:AdminKey` |
| `u32:`, `i32:` | 32-bit unsigned and signed integers | `u32:42`, `i32:-10` |
| `u64:`, `i64:` | 64-bit unsigned and signed integers | `u64:1000000`, `i64:-500` |
| `u128:` | 128-bit unsigned integer | `u128:340282366920938463463374607431768211455` |
| `bool:` | Boolean value (`true` or `false`) | `bool:true` |
| `addr:` | Account (`G...`) or Contract (`C...`) Strkey address | `addr:GABC...`, `addr:CDLZ...` |
| `bytes:` | Hex-encoded raw byte array | `bytes:deadbeef`, `bytes:0x1234` |
| `vec[...]` | Nested composite vector (up to 16 nesting levels) | `vec[sym:Allowance, addr:G..., addr:C...]` |

### Offline Snapshot Replay (`--snapshot`)
Use a deterministic `.sentinel-snap` archive to debug TTL calculations, regression
diffing, and risk evaluation without RPC access:

```bash
# Replay a previously recorded snapshot
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --network testnet \
  --snapshot state.sentinel-snap \
  --format json
```

Snapshots store the network passphrase, protocol version, latest ledger,
ledger-close velocity, and every queried ledger entry. Replay validates the
passphrase and format version before use, returns entries in deterministic order,
and produces the same `TTLReport` as the live scan that recorded it. Use the same
`--network` that produced the snapshot; `--snapshot` cannot be combined with
`--rpc-url`, `--rpc-timeout`, `--rpc-max-retries`, `--baseline`, `--format csv`,
or `--format parquet`. Unset `SOROBAN_RPC_URL` when replaying offline because an
RPC URL from the environment is treated as a conflicting flag. Invalid or
corrupt snapshots exit with code 10.

### Baseline Scan Comparison & Regression Gating (`--baseline`)
Track storage decay and detect changes between inspection intervals by comparing a scan against a previous JSON report:

```bash
# Save a baseline scan
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC --format json > baseline.json

# Later: compare current state against baseline
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --baseline baseline.json

# CI Regression Gate: fail with exit code 2 if any entry regressed (status worsened or entry removed)
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --baseline baseline.json \
  --fail-on-regression
```

### Tabular CSV Export (`--format csv`)
For analytics in DuckDB, ClickHouse, Pandas, or spreadsheets, export one flat
row per storage entry:

```bash
# Stream CSV to stdout
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --format csv

# Write directly to a file (existing files are overwritten/truncated)
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --format csv --output fleet-ttl.csv
```

The header is exactly:
`timestamp,network,contract_id,key_xdr,durability,byte_size,live_until_ledger,remaining_ledgers,status`.

Timestamps are UTC ISO 8601 (`YYYY-MM-DDTHH:MM:SSZ`), numeric fields are plain
integers, and output contains no terminal escape codes. `--output` writes the
same bytes that would go to stdout, leaving stdout empty. An unwritable output
path exits 10. CSV export is incompatible with `--baseline` because baseline
diffs are object-shaped and do not fit the fixed flat schema; combining them
exits 10. Contract code entries leave `key_xdr` blank because the WASM hash is
not retained in the scan report.

### Tabular Parquet Export (`--format parquet`)

The same nine columns and values are available as a binary Parquet file for
columnar analytics. Parquet support is an optional Cargo feature so the default
binary stays light:

```bash
cargo build -p sentinel-cli --features parquet

# Write a Parquet file (binary output requires a file or a redirect)
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --format parquet --output fleet-ttl.parquet

# Piped/redirected stdout is allowed
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --format parquet > fleet-ttl.parquet
```

The `timestamp` column is a UTC `TIMESTAMP_MICROS`, `byte_size`,
`live_until_ledger`, and `remaining_ledgers` are nullable 64-bit integers, and
all other columns are strings. Rows with no ledger key XDR (contract code
entries) store a null `key_xdr`. Writing binary data to an interactive terminal
is refused with exit code 10; running without the `parquet` feature also exits
10 with a rebuild hint. `--baseline` is rejected for this flat schema just like
CSV. Scan exit codes (0, 1, 2, 10, 11) are unchanged.

### Advanced RPC & Velocity Options
```bash
# Configure RPC timeout (1–120s) and retry attempts (0–10) with exponential backoff & jitter
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --rpc-timeout 30 \
  --rpc-max-retries 5

# Override measured ledger close velocity (1.0–60.0s)
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --ledger-seconds 5.8

# Measure real burn velocity from the last 250 ledger close timestamps
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --sample-window 250
```

### OpenTelemetry OTLP Export (`--otlp-endpoint`)
Send distributed traces and metrics for scan execution, Soroban RPC calls, and
XDR parsing to any OTLP-compatible collector:

```bash
# Configure per invocation
sentinel --otlp-endpoint http://localhost:4317 scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC

# Or use the standard OpenTelemetry environment variable
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC
```

The CLI exports `sentinel.contracts.scanned`, `sentinel.entries.expired`, and
`sentinel.findings.critical` counters, flushes buffered telemetry before exit,
and continues normally with a warning if the collector is unreachable.

### Bundling TTL Extensions
Create unsigned `ExtendFootprintTTL` transaction envelopes for multiple contracts.
Each transaction contains a deduplicated read-only footprint capped at 40 entries,
is simulated against Soroban RPC, and reports its fee plus the combined fee:

```bash
sentinel bundle \
  --contract CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --contract <ANOTHER_CONTRACT_ID> \
  --key ADMIN \
  --key-xdr <BASE64_SCVAL_XDR> \
  --source-account <SOURCE_ACCOUNT_G...> \
  --extend-by 100000 \
  --network testnet
```

The source account must exist on the selected network. `--extend-by` is required;
`--base-fee` defaults to 100 stroops per transaction and can be overridden.
Repeated `--key` and `--key-xdr` values are applied to each command-line contract.
Alternatively, `--manifest contracts.json` accepts JSON in this form:

```json
{
  "contracts": [
    {
      "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
      "keys": ["ADMIN"],
      "key_xdr": ["<BASE64_SCVAL_XDR>"]
    }
  ]
}
```

Bundling does not sign or submit transactions. Review and sign the returned XDR
with a separate trusted wallet before submission.

### Restoring Archived State
When persistent keys (or a contract instance) have expired, TTL extension no
longer works. The `restore` command detects archived keys, builds a
`RestoreFootprintOp`, simulates the resource fee, and emits an unsigned
transaction envelope plus the equivalent `stellar contract restore` commands:

```bash
sentinel restore CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC \
  --key ADMIN \
  --key-expr 'sym:Balance' \
  --source-account <SOURCE_ACCOUNT_G...> \
  --network testnet \
  --format json
```

With no `--key`/`--key-expr`/`--key-xdr` flags, the contract instance is
checked. Keys that are still live are ignored; if none are archived the command
exits 10 without simulating. `--source-account` is optional; when omitted a
placeholder account is used only for simulation and the generated CLI commands
omit `--source-account` so you can supply your own identity. The output uses
`--build-only`, so the commands print a transaction for review rather than
submitting it. No signing keys are handled or required.

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

### Interactive TUI Dashboard
For contracts with many storage keys, build with the optional `tui` feature and
launch the dashboard:

```bash
cargo build --release -p sentinel-cli --features tui

sentinel scan CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC --tui
```

The dashboard shows a scrollable table with entry key, durability, byte size,
remaining ledgers, and health status, plus a color-coded TTL gauge for the
selected key. Use `Up`/`Down` (or `PgUp`/`PgDn`) to navigate rows, `Left`/`Right`
to change sort column, `s` to reverse the sort direction, `Tab` to switch between
the table and detail panels, and `q` or `Esc` to exit. Window resize events are
handled by reflowing the layout.

The `tui` feature is disabled by default so the standard binary stays
lightweight. Running `--tui` without the feature exits 10 with a rebuild hint;
with the feature enabled it also exits 10 if stdout is not a TTY. Mouse input
and web hosting are not supported.

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
`sentinel scan` uses the following exit codes for automation:

| Exit Code | Meaning | Condition |
| :---: | :--- | :--- |
| `0` | **Success / OK** | All evaluated storage entries have `status == OK` |
| `1` | **Warning** | One or more evaluated storage entries have `status == WARN` |
| `2` | **Critical / Archived / Regression** | One or more evaluated entries have `status == CRITICAL` or `ARCHIVED`, or `--fail-on-regression` triggered by status worsening / entry removal |
| `10` | **Invalid Arguments** | Malformed contract Strkey, unrecognized network, or invalid parameters |
| `11` | **RPC Failure** | Network timeout, connection refused, or HTTP 5xx/4xx RPC communication failure |

### Diagnose an RPC endpoint

```sh
sentinel doctor --network testnet
sentinel doctor --network mainnet --rpc-url https://your-rpc.example
```

The diagnostic table shows round-trip latency, node health, ledger height,
network identity, and Soroban protocol compatibility. Critical probe failures
exit with status 1.

### Watch a contract and send incident alerts

```sh
sentinel watch "$CONTRACT_ID" --network testnet --key Counter --interval 60
```

Set `PAGERDUTY_ROUTING_KEY` or `OPSGENIE_API_KEY` in the process environment to
enable incident delivery. The corresponding flags are `--pagerduty-routing-key`
and `--opsgenie-api-key`. Alert identifiers derive from the network, contract
address, and canonical ledger key XDR. Warning risk produces a warning alert,
while critical or archived risk produces a critical alert. A watched entry
returning to OK resolves its alert. The watcher tracks successfully delivered
alerts during its current process lifetime.

### Core submission and key discovery

`TransactionSubmitter::submit` takes signed envelope XDR in base64, the network
passphrase, and a submission policy. It computes the hash locally and polls for
confirmation. A timeout leaves the outcome unknown and returns the hash for
follow-up.

`discover_storage_keys` takes a contract address and decoded `ContractCodeEntry`.
It returns persistent and temporary candidates inferred from storage-named
types in the WASM spec. A caller can encode the returned `LedgerKey` values for
`getLedgerEntries` to check existence. Private or dynamic keys may be absent
from the spec and still require explicit input.

## 6. Repository Documentation Suite

For complete architectural specifications, product requirements, and contribution plans:

* [Architecture Decision Record (ADR)](docs/ARCHITECTURE_DECISION.md) — Modular monolith evaluation, Ports-and-Adapters structure, and 2-crate workspace layout.
* [Product Requirements Document (PRD)](docs/PRD.md) — Problem statement, user personas, functional/non-functional requirements, and success metrics.
* [Technical Design Document](docs/TECHNICAL_DESIGN.md) — Hexagonal architecture, domain ports, XDR serialization, and rate-limit handling.
* [MVP Scope Specification](docs/MVP_SCOPE.md) — Strictly bounded first release, CLI input/output specifications, and acceptance criteria.
* [Implementation Roadmap](docs/IMPLEMENTATION_ROADMAP.md) — Ordered sprint phases, milestone deliverables, and effort estimates.
* [Testing Strategy](docs/TESTING_STRATEGY.md) — Isolated domain unit tests, mock RPC tests, and CLI acceptance harnesses.
* [Security & Operational Safety](docs/SECURITY.md) — Threat model, read-only guarantees, secret management, and keeper transaction limits.
* [Open Source Contribution Plan](docs/OPEN_SOURCE_CONTRIBUTION_PLAN.md) — Drips Wave workflow, complexity levels, and how to find open issues.
* [Research & References](docs/RESEARCH_AND_REFERENCES.md) — Verifiable findings, official Stellar source code references, and competitive analysis.

---

## 7. Contributing to Soroban Sentinel

`soroban-sentinel` is designed to be a premier collaborative project within the **Stellar Drips Wave Program**. Well-scoped work is tracked in [GitHub Issues](https://github.com/ekwe7/soroban-sentinel/issues) and categorized by difficulty:
* **Trivial (100 Points)**: JSON/Markdown output formatters, CLI parameter validation, documentation improvements.
* **Medium (150 Points)**: Storage key parsers, webhook payload builders, Prometheus metric collectors.
* **High (200 Points)**: RPC rate-limiting middleware, transaction simulation fee estimator, autonomous keeper safety bounds.

Please read our [Contribution Plan](docs/OPEN_SOURCE_CONTRIBUTION_PLAN.md) and review the [Security Policy](docs/SECURITY.md) before submitting code.

---

## 8. License

`soroban-sentinel` is licensed under the [MIT License](LICENSE).
