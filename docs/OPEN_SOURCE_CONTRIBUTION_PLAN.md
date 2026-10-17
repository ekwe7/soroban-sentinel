# Open Source Contribution Plan & Drips Wave Backlog

**Project:** `soroban-sentinel`  
**Architecture:** Modular Monolith (2 Crates: `sentinel-core` and `sentinel-cli`)  
**Program:** Stellar Wave on Drips Network  
**Maintainer:** Lead Project Architect  
**Repository Policy:** Public, MIT Licensed, Continuous Integration Gated  

---

## 1. Drips Wave Program Alignment & Backlog Overview

The **Stellar Wave Program on Drips** brings external developers into high-impact Stellar repositories during week-long contribution sprints. To ensure high-quality contributions, prevent maintainer bottlenecks, and maximize reward fairness, issues are curated and weighted according to standard Wave complexity points:

* **Trivial (100 Points)**: Well-defined tasks, CLI formatting flags, domain math unit tests, parameter validation. Ideal for first-time contributors.
* **Medium (150 Points)**: XDR key derivation, RPC chunking and retry middleware, in-process alert handlers, Prometheus metric collectors.
* **High (200 Points)**: Complex cryptographic transaction assembly, rent simulation calculators, keeper safety bounds. Requires deep protocol familiarity.

---

## 2. Contributor Workflow & Standards

### Step-by-Step Workflow
1. **Find an Issue**: Browse open issues tagged with `Stellar Wave`. Review the description, acceptance criteria, and complexity points.
2. **Apply & Assign**: Apply for the task via the Drips Wave portal (or comment on the GitHub issue). The maintainer will review your profile and assign you to the issue.
3. **Fork & Branch**: Create a feature branch off `main`:
   ```bash
   git checkout -b feature/issue-2-json-output
   ```
4. **Develop & Verify**: Ensure all changes conform to repository quality standards:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
5. **Submit PR**: Open a Pull Request referencing the issue (e.g., `Closes #2`). Include a brief description of the implementation and test verification.
6. **Review & Merge**: Maintainers review PRs within 24–48 hours. Once merged, maintainers resolve the issue in Drips so rewards are distributed automatically.

---

## 3. Curated Contributor Backlog (Issues #1 through #8)

```
┌─────────────────────────────────┬─────────────────────────────────┬─────────────────────────────────┐
│     Sprint 1: MVP Core & Formats│     Sprint 2: Forecasting & CI  │     Sprint 3: Watch & Telemetry │
├─────────────────────────────────┼─────────────────────────────────┼─────────────────────────────────┤
│ Issue #1: Domain TTL Decay Math │ Issue #5: Rent Simulation Math  │ Issue #7: In-Process Webhooks   │
│ (100 pts - Trivial)             │ (200 pts - High)                │ (150 pts - Medium)              │
│                                 │                                 │                                 │
│ Issue #2: JSON Output Formatter │ Issue #6: Markdown Table Format │ Issue #8: Prometheus Exporter   │
│ (100 pts - Trivial)             │ (100 pts - Trivial)             │ (150 pts - Medium)              │
│                                 │                                 │                                 │
│ Issue #3: Persistent Key Parser │                                 │ Issue #9: Safe Keeper Bumper    │
│ (150 pts - Medium)              │                                 │ (200 pts - High)                │
│                                 │                                 │                                 │
│ Issue #4: RPC Chunking & Retry  │                                 │                                 │
│ (150 pts - Medium)              │                                 │                                 │
└─────────────────────────────────┴─────────────────────────────────┴─────────────────────────────────┘
```

---

### Issue #1: Implement Pure Domain TTL Decay Math & Risk Evaluation
* **Complexity Level**: Trivial (100 Points)
* **Suitability**: **First-Time Contributor Friendly**
* **Crate**: `crates/sentinel-core` (`domain::ttl`, `domain::risk`)
* **Description**:
  Implement the pure mathematical functions calculating remaining ledgers, wall-clock time horizon (based on $5.0\text{s/ledger}$), and `RiskLevel` evaluation (`Ok`, `Warn`, `Critical`, `Archived`).
* **Technical Guidance**:
  * Pure functions with zero I/O dependencies.
  * Implement `ThresholdEvaluator` struct with configurable warning and critical ledger thresholds.
* **Acceptance Criteria**:
  - [ ] 100% unit test coverage for negative remaining ledgers, zero ledgers, and exact threshold boundaries.
  - [ ] Zero clippy warnings.

---

### Issue #2: Implement JSON Output Formatter in `sentinel-cli`
* **Complexity Level**: Trivial (100 Points)
* **Suitability**: **First-Time Contributor Friendly**
* **Crate**: `crates/sentinel-cli` (`formatters::json`)
* **Description**:
  Developers running `sentinel scan` in automated CI/CD pipelines need machine-readable JSON output. Implement `--format json` emitting the schema defined in `docs/MVP_SCOPE.md`.
* **Technical Guidance**:
  * Use `serde_json::to_string_pretty`.
  * Ensure exit codes match status (`0` for OK, `1` for WARN, `2` for CRITICAL/ARCHIVED).
* **Acceptance Criteria**:
  - [ ] Running `sentinel scan <ID> --format json` produces valid JSON matching schema.
  - [ ] CLI integration test in `tests/cli_scan.rs` verifies stdout.

---

### Issue #3: Implement Persistent Storage Key XDR Parser
* **Complexity Level**: Medium (150 Points)
* **Suitability**: Intermediate Rust Contributor
* **Crate**: `crates/sentinel-core` (`adapters::xdr_keys`)
* **Description**:
  Extend `adapters::xdr_keys` to support querying arbitrary persistent storage keys alongside the contract instance. Implement parsers for simple symbol strings (`--key SymbolName`) and base64-encoded XDR `ScVal` strings (`--key-xdr AAAA...`).
* **Technical Guidance**:
  * Utilize `stellar_xdr::curr::ScVal::from_xdr_base64`.
  * Convert valid `ScVal` keys into `LedgerKey::ContractData` with `ContractDataDurability::Persistent`.
* **Acceptance Criteria**:
  - [ ] Unit tests verify correct `LedgerKey` generation for string symbols and base64 XDR keys.
  - [ ] Error tests confirm graceful rejection of malformed base64 strings with clear error messages.

---

### Issue #4: Implement Soroban RPC Key Chunking & Retry Logic
* **Complexity Level**: Medium (150 Points)
* **Suitability**: Intermediate Rust Contributor
* **Crate**: `crates/sentinel-core` (`adapters::soroban_rpc`)
* **Description**:
  Implement the `LedgerReader` trait for `SorobanRpcClient`. Slices key queries into chunks of maximum 200 items (enforcing Soroban RPC batch limits) and retries on HTTP 429/503 using exponential backoff with jitter.
* **Technical Guidance**:
  * Use `reqwest` async client.
  * Test with `wiremock` simulating batch chunks and 429 rate limits.
* **Acceptance Criteria**:
  - [ ] Passing 250 keys splits into 2 sequential RPC calls of 200 and 50 keys.
  - [ ] Transient HTTP 503 errors trigger backoff and recover cleanly.

---

### Issue #5: Implement Rent Simulation via `simulateTransaction`
* **Complexity Level**: High (200 Points)
* **Suitability**: Experienced Stellar / Rust Developer
* **Crate**: `crates/sentinel-core` & `crates/sentinel-cli` (`commands::forecast`)
* **Description**:
  Implement the `sentinel forecast` command. Assemble an `ExtendFootprintTTLOp` transaction envelope with the target contract entries in the `readOnly` footprint and query Soroban RPC's `simulateTransaction` endpoint to calculate exact stroop costs.
* **Technical Guidance**:
  * Populate `readOnly` footprint, assert empty `readWrite` footprint.
  * Parse `SimulateTransactionResponse` extracting `minResourceFee`.
* **Acceptance Criteria**:
  - [ ] Running `sentinel forecast <ID> --extend-by 100000` returns the simulated resource fee.
  - [ ] Itemized table output displays stroop and converted XLM costs.

---

### Issue #6: Add GitHub-Flavored Markdown Output Formatter
* **Complexity Level**: Trivial (100 Points)
* **Suitability**: **First-Time Contributor Friendly**
* **Crate**: `crates/sentinel-cli` (`formatters::markdown`)
* **Description**:
  Implement `--format markdown` to format scan outputs into clean Markdown tables suitable for GitHub Actions PR bot comments.
* **Acceptance Criteria**:
  - [ ] Formats headers: `| Entry | Durability | Size | Live Until | Remaining | Status |`.
  - [ ] CLI test asserts valid Markdown table syntax.

---

### Issue #7: In-Process Webhook Alert Dispatcher for Slack & Discord
* **Complexity Level**: Medium (150 Points)
* **Suitability**: Intermediate Rust Contributor
* **Crate**: `crates/sentinel-cli` (`commands::watch`)
* **Description**:
  Implement alert dispatchers subscribing to in-process `SentinelEvent::RiskThresholdBreached` events and posting formatted embeds to configured Slack or Discord webhooks.
* **Acceptance Criteria**:
  - [ ] WireMock tests verify Slack Block Kit and Discord embed JSON payloads.
  - [ ] 24-hour alert deduplication cooldown per entry.

---

### Issue #8: Prometheus Telemetry Exporter in `sentinel watch`
* **Complexity Level**: Medium (150 Points)
* **Suitability**: Intermediate Rust Contributor
* **Crate**: `crates/sentinel-cli` (`commands::watch`)
* **Description**:
  Expose standard Prometheus metrics using `axum` and the `prometheus` crate on port `9100/metrics` during watch mode.
* **Acceptance Criteria**:
  - [ ] `GET /metrics` returns valid OpenMetrics formatted text.
  - [ ] Integration test scrapes `/metrics` and validates gauge values.

---

### Issue #9: Autonomous Safe Keeper Envelope Signer
* **Complexity Level**: High (200 Points)
* **Suitability**: Experienced Stellar / Rust Developer
* **Crate**: `crates/sentinel-core` & `crates/sentinel-cli`
* **Description**:
  Implement transaction assembly, signing, and safety guards for the `--auto-bump` keeper feature.
* **Acceptance Criteria**:
  - [ ] Integration test on Testnet bumps a test contract's TTL.
  - [ ] Refuses execution if daily spend cap is reached or if wallet balance drops below safety floor.
