# Comprehensive Testing Strategy & Quality Assurance Plan

**Project:** `soroban-sentinel`  
**Architecture:** Modular Monolith (Hexagonal / Ports & Adapters)  
**Test Frameworks:** Rust standard test harness (`cargo test`), `wiremock` (Mock RPC), `assert_cmd` (CLI Testing)  
**Target Environments:** Local offline harness, Mock WireMock HTTP server, Soroban Testnet  

---

## 1. Testing Philosophy in Hexagonal Architecture

Adopting a Ports-and-Adapters (Hexagonal) architecture simplifies testing by allowing pure domain logic and application use cases to be tested completely in isolation from network protocols and terminal rendering:

```
                  / \
                 /   \       Live Testnet Integration Tests
                / E2E \      (Real RPC, Real Contracts, Live State)
               /-------\
              /         \    Adapter & CLI Process Tests
             /  Adapters \   (WireMock HTTP JSON-RPC, assert_cmd)
            /-------------\
           /               \ Domain & Use Case Unit Tests (100% Offline)
          /      Unit       \(Pure Math, Mock LedgerReader, XDR Serialization)
         /-------------------\
```

* **Domain Unit Tests (Offline / Fast)**: Execute in `< 1 second`. Tests TTL decay math, horizon estimation, risk level thresholds, and XDR key derivation with **zero network access** using in-memory mock implementations of `LedgerReader`.
* **Adapter Integration Tests (WireMock)**: Simulate exact Soroban JSON-RPC 2.0 error modes, rate limiting (HTTP 429), malformed responses, and chunking boundaries without touching live blockchain nodes.
* **CLI Acceptance Tests (`assert_cmd`)**: Spawn the compiled `sentinel` binary as a child process and assert exact stdout formatting, JSON schema compliance, and UNIX exit codes (`0`, `1`, `2`, `10`, `11`).
* **Live Testnet Tests**: Verified against live deployed contracts on Soroban Testnet (`https://soroban-testnet.stellar.org`).

---

## 2. Test Suites by Component

### 2.1 Pure Domain Unit Testing (`sentinel-core::domain`)

#### Test Suite: Decay & Risk Math (`tests/unit_domain_math.rs`)
* **Test 1.1: Remaining Ledger Calculation**:
  * Inputs: `live_until = 5,500,000`, `latest_ledger = 5,400,000`.
  * Assertion: Remaining ledgers = `100,000`.
* **Test 1.2: Expired / Archived Entry Handling**:
  * Inputs: `live_until = 5,400,000`, `latest_ledger = 5,400,100`.
  * Assertion: Remaining ledgers = `0`, status evaluates to `RiskLevel::Archived`.
* **Test 1.3: Risk Threshold Classification**:
  * Assert `remaining >= 120,960` evaluates to `RiskLevel::Ok`.
  * Assert `34,560 <= remaining < 120,960` evaluates to `RiskLevel::Warn`.
  * Assert `0 < remaining < 34,560` evaluates to `RiskLevel::Critical`.

#### Test Suite: In-Memory Use Case Testing (`tests/unit_usecase_scan.rs`)
* **Test 2.1: Mock LedgerReader Injection**:
  * Implement an in-memory `MockLedgerReader` struct satisfying `ports::LedgerReader`.
  * Configure mock to return predetermined instance and WASM code entries.
  * Execute `ScanContractUseCase::new(mock_reader).execute(contract_id)`.
  * Assert resulting `TTLReport` matches expected domain state without any HTTP calls.

---

### 2.2 Adapter Integration Testing (`wiremock`)

Mock tests run against an in-process HTTP mock server using the `wiremock` crate, ensuring deterministic validation of RPC error handling and edge cases.

#### Test Suite: Key Chunking & Batch Limits (`tests/mock_rpc_chunking.rs`)
* **Scenario**: Querying 250 storage keys in a single scan.
* **Mock Setup**: Server expects exactly two consecutive POST requests:
  * Request 1: Contains first 200 keys.
  * Request 2: Contains remaining 50 keys.
* **Assertion**: Client automatically partitions the input slice and merges responses transparently without throwing a parameter size error.

#### Test Suite: RPC Resilience & Retry Handling (`tests/mock_rpc_retries.rs`)
* **Scenario A: Transient HTTP 503 / 504**:
  * Mock Server returns HTTP 503 twice, then returns HTTP 200 with valid data on attempt 3.
  * Assertion: Client retries with exponential backoff and succeeds without failing the scan.
* **Scenario B: Rate Limiting (HTTP 429)**:
  * Mock Server returns HTTP 429 with `Retry-After: 1`.
  * Assertion: Client respects backoff, pauses execution, and completes on subsequent attempt.
* **Scenario C: Permanent Connection Failure**:
  * Mock Server closes socket immediately.
  * Assertion: Client times out cleanly after configurable timeout (e.g., 10s) and returns a typed `RpcError::ConnectionTimeout` without panicking.

---

### 2.3 CLI Integration Testing (`assert_cmd`)

#### Test Suite: Input Validation & Exit Codes (`tests/cli_scan.rs`)

| Input Command | Expected Output / Behavior | Expected Exit Code |
| :--- | :--- | :--- |
| `sentinel scan CA3D... --format text` | Formatted terminal table printed to `stdout` | `0` (if all entries OK) |
| `sentinel scan CA3D... --format json` | Output parses as valid JSON against schema | `0` (if all entries OK) |
| `sentinel scan INVALID_STRKEY` | Error message: `Invalid contract ID '...'` | `10` (Bad Input) |
| `sentinel scan CA3D... --threshold 99999999` | All entries evaluated as `WARN` | `1` (Warning State) |
| `sentinel scan <NON_EXISTENT_CONTRACT>` | Warning: `Contract instance not found` | `2` (Critical / Missing) |

---

### 2.4 Future Watch Mode: Event Handler Testing Strategy

When continuous monitoring (`sentinel watch`) is introduced in Phase 5:
* **In-Process Channel Testing**:
  * Test harnesses instantiate a `tokio::sync::broadcast` channel.
  * Emit candidate domain events: `SentinelEvent::RiskThresholdBreached`.
  * Verify that alert dispatchers receive the event and construct the correct Slack/Discord payload.
  * Verify that metric handlers receive `SentinelEvent::EntryEvaluated` and increment Prometheus gauge values.
* **No External Infrastructure Required**: All event handler tests run entirely within Tokio in-process channels with zero external message broker dependencies.

---

## 3. Boundary & Failure Mode Test Matrix

| Category | Boundary Condition | Expected System Behavior | Test Harness |
| :--- | :--- | :--- | :--- |
| **Input** | 55-character Strkey (1 char short) | Rejected immediately during clap parsing with clear error message | Unit / CLI |
| **Input** | Non-base64 string passed to `--key-xdr` | Returns typed error `InvalidXdrEncoding` | Unit / CLI |
| **State** | `liveUntilLedgerSeq == 0` | Evaluated as `RiskLevel::Archived` with 0 remaining ledgers | Unit / Mock |
| **State** | Entry expired in previous ledger | Evaluated as `RiskLevel::Archived` | Unit / Mock |
| **State** | Contract instance exists, but references non-existent WASM hash | Displays instance entry, logs warning for missing code entry | Mock RPC |
| **RPC** | Public RPC enforces 200 key limit and rejects 201 keys | Validates that client never emits requests with > 200 keys | Mock RPC |
| **Resilience**| RPC endpoint unreachable | Returns typed `RpcError::ConnectionTimeout` and Exit Code `11` | CLI / Mock |

---

## 4. Reproducible Acceptance Test Commands

Developers and reviewers can run the full test battery locally with standard Cargo commands:

```bash
# 1. Run all offline unit tests (Domain & Ports)
cargo test --workspace --lib

# 2. Run mock RPC integration tests (WireMock)
cargo test --test mock_rpc_chunking --test mock_rpc_retries

# 3. Run CLI process integration tests (assert_cmd)
cargo test --test cli_scan

# 4. Run live Testnet verification (requires internet connection)
cargo test --test testnet_validation -- --ignored

# 5. Verify zero compiler warnings and strict linting
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 6. Verify formatting
cargo fmt --all -- --check
```
