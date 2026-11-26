# Soroban Sentinel Integration Test Baseline Report

- **Date:** 2026-11-26
- **Target Branch:** `integration`
- **Initial Baseline Commit:** `7273271dc52c0e8ede19069a674695503c341750`

---

## 1. Summary of Baseline Execution

| Command | Status | Notes |
|---|---|---|
| `cargo fmt --all -- --check` | **PASS** (Exit 0) | Clean formatting across all crates |
| `cargo check --workspace` | **PASS** (Exit 0) | Compiles cleanly with non-fatal warnings |
| `cargo test -p sentinel-core` | **FAIL** (Exit 101) | 232 passed, 8 failed |
| `cargo test -p sentinel-cli` | **FAIL** (Exit 101) | Compilation errors in test targets (duplicate `mod tests`, missing `WatchFormat`) |
| `cargo test --workspace` | **FAIL** (Exit 101) | Blocked by `sentinel-cli` compile errors & `sentinel-core` test failures |
| `cargo clippy --workspace --all-targets --all-features` | **FAIL** (Exit 101) | Blocked by `sentinel-cli` test compilation & clippy lint in `wasm.rs` |

---

## 2. Command Details & Failure Analysis

### 2.1 `cargo fmt --all -- --check`
- **Output:** Clean exit code 0. No unformatted code detected.

### 2.2 `cargo check --workspace`
- **Output:** Exit code 0.
- **Warnings emitted:**
  1. `warning: unused workspace dependency clap_complete`
  2. `warning: unused workspace dependency schemars`
  3. `warning: unused field repository in workspace.package`
  4. `warning: unused field readme in workspace.package`
  5. `warning: unused dependency hdrhistogram` in `sentinel-cli`
  6. `warning: function exit_code_for_error is never used` in `sentinel-cli/src/commands/scan.rs:180`
  7. `warning: function is_rpc_failure is never used` in `sentinel-cli/src/commands/scan.rs:189`
  8. `warning: function exit_code_for_status is never used` in `sentinel-cli/src/commands/scan.rs:199`

### 2.3 `cargo test -p sentinel-core`
- **Result:** 232 passed, 8 failed, 0 ignored.
- **Failures:**
  1. `adapters::wasm::tests::inspection_reads_soroban_sections_and_metadata`:
     - *Cause:* `assert!(!report.nearing_archival)` failed because `code_remaining_ledgers` (100) was less than `critical_ledgers` (34,560).
  2. `usecases::scan::tests::test_scan_applies_per_durability_threshold_overrides`:
     - *Cause:* Mock bytecode was `&[0xCA, 0xFE, 0xBA, 0xBE]`, which panics when `inspect_contract_code_entry` unwrapped invalid WASM.
  3. `usecases::scan::tests::test_scan_deduplicates_repeated_symbol_keys`:
     - *Cause:* Mock bytecode was `&[1, 2, 3]`, failing with `InvalidWasm("unexpected end-of-file")`.
  4. `usecases::scan::tests::test_scan_happy_path_with_wasm_and_persistent_keys`:
     - *Cause:* `assert_eq!(code.contract_code_size_bytes, Some(4))` failed because the fixture code was 8 bytes (`b"\0asm\x01\0\0\0"`), so `contract_code_size_bytes` is `Some(8)`.
  5. `usecases::scan::tests::test_scan_instance_storage_breakdown_surface`:
     - *Cause:* Mock bytecode was `vec![0u8; 64]`, failing with `InvalidWasm("magic header not detected")`.
  6. `usecases::scan::tests::test_scan_prefers_symbol_identifier_over_equivalent_xdr_key`:
     - *Cause:* Mock bytecode from `dedup_fixture` was `&[1, 2, 3]`, failing with `InvalidWasm`.
  7. `usecases::scan::tests::test_scan_readable_identifier_replaces_earlier_xdr_in_first_position`:
     - *Cause:* Mock bytecode from `dedup_fixture` was `&[1, 2, 3]`, failing with `InvalidWasm`.
  8. `usecases::scan::tests::test_scan_summary_counts_each_deduplicated_key_once`:
     - *Cause:* Mock bytecode from `dedup_fixture` was `&[1, 2, 3]`, failing with `InvalidWasm`.

### 2.4 `cargo test -p sentinel-cli`
- **Result:** Failed during compilation of lib tests:
  1. `error[E0428]: the name tests is defined multiple times` in `crates/sentinel-cli/src/formatters/tabular.rs:250:1`.
  2. `error[E0433]: cannot find type WatchFormat in this scope` in `crates/sentinel-cli/src/args.rs:1272:34` and `1281:34`.

### 2.5 `cargo clippy --workspace --all-targets --all-features`
- **Result:** Blocked by `sentinel-cli` test compilation, plus:
  1. `warning: manual implementation of .is_multiple_of()` in `crates/sentinel-core/src/adapters/wasm.rs:277:15`.

---

## 3. Stabilization Objective

Every recorded failure represents an integration seam between contributor PRs merged in Waves 1–5. In Phase 4, each defect will be systematically addressed, validated, and recorded in small, self-contained commits without discarding contributor functionality.
