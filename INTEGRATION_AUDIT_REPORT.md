# Soroban Sentinel Technical Audit Report (Integration Branch)

- **Date:** 2026-11-26
- **Branch:** `integration`
- **Initial Consolidation Commit:** `7273271dc52c0e8ede19069a674695503c341750`
- **Audit Scope:** Entire repository including all 16 merged contributor pull requests.

---

## 1. Executive Summary

A comprehensive architectural and code-level technical audit was conducted across the workspace (`sentinel-core` and `sentinel-cli`) following the consolidation of 16 contributor pull requests into the `integration` branch.

The codebase exhibits strong modular separation:
- Core domain logic, adapters, and use cases reside cleanly in `crates/sentinel-core`.
- CLI commands, argument definitions, formatters, and telemetry reside in `crates/sentinel-cli`.
- Contributor features (WASM inspection, multi-channel alerts, temporary key derivation, storage governance, and formatters) are intact.

However, specific integration seams, merge anomalies, and test failures were identified across the workspace. This document details every finding, its root cause, affected PRs, risk levels, and recommended remedies.

---

## 2. Detailed Findings by Category

### Finding 1: Duplicate `mod tests` in `crates/sentinel-cli/src/formatters/tabular.rs`
- **Location:** `crates/sentinel-cli/src/formatters/tabular.rs:193` and `line 250`
- **Problem:** Two separate `#[cfg(test)] mod tests` blocks are declared in the same module. The first tests temporary symbol key XDR reconstruction; the second tests `format_epoch_seconds` and `CSV_HEADER`.
- **Why It Matters:** Rust forbids duplicate module definitions in the same scope (`error[E0428]`), preventing `cargo test -p sentinel-cli` and `cargo clippy --all-targets` from compiling.
- **Affected Contributor PRs:** PR #85 / #95 (original CSV/tabular formatter) and PR #105 (temporary symbol key formatting).
- **Risk Level:** High (blocks test build).
- **Recommended Correction:** Consolidate both test sets into a single `mod tests` block.

---

### Finding 2: Missing `WatchFormat` and Unresolved Watch Arguments in `crates/sentinel-cli/src/args.rs`
- **Location:** `crates/sentinel-cli/src/args.rs:223-260` and `lines 1272, 1281`
- **Problem:** Unit tests in `args.rs` verify `--format json|text`, `--max-iterations`, and `--max-consecutive-failures` for `sentinel watch`. However, during conflict resolution between PR #133/#213 (incident alert watcher) and PR #105 (diff-state watcher), the `WatchFormat` enum and those CLI flags were omitted from `WatchArgs`.
- **Why It Matters:** Causes `error[E0433]: cannot find type WatchFormat in this scope`, blocking test execution. Furthermore, omitting these flags disables the integration tests in `crates/sentinel-cli/tests/cli_watch.rs`.
- **Affected Contributor PRs:** PR #105 (diff watcher / iteration flags) and PR #133 / #213 (incident notification watcher).
- **Risk Level:** High (compilation error and feature divergence).
- **Recommended Correction:**
  1. Define `WatchFormat` enum (`Text`, `Json`) in `args.rs`.
  2. Add `format`, `max_iterations`, and `max_consecutive_failures` to `WatchArgs`, alongside existing notification fields.
  3. Wire these fields into `commands::watch::run_watch` to support bounded iterations and formatted diff output while preserving alert dispatching.

---

### Finding 3: Unhandled WASM Parse Failure in `ScanContractUseCase`
- **Location:** `crates/sentinel-core/src/usecases/scan.rs:832`
- **Problem:** `inspect_contract_code_entry` was invoked with `?` in `ScanContractUseCase`. If a contract code entry in ledger storage contains malformed WASM, non-standard bytecode, or dummy test payloads (such as `&[1, 2, 3]` or `&[0xCA, 0xFE, 0xBA, 0xBE]`), the entire contract scan aborts with `InvalidWasm`.
- **Why It Matters:** In production, a corrupted or unparsable code entry should not prevent the operator from reading contract instance storage or persistent TTL records. Furthermore, 6 existing unit tests in `sentinel-core` failed because mock fixtures provided dummy bytecode.
- **Affected Contributor PRs:** PR #104 (WASM bytecode inspection) and PR #105 (instance storage breakdown & tests).
- **Risk Level:** Medium-High (resilience defect and test failure).
- **Recommended Correction:** Make `wasm_inspection` resilient by capturing `inspect_contract_code_entry(...).ok()`. If inspection fails or returns invalid bytecode, log an advisory warning and proceed with TTL reporting. In test fixtures testing WASM inspection explicitly, provide valid WASM headers (`b"\0asm\x01\0\0\0"`).

---

### Finding 4: Inverted Archival Assertion in `inspection_reads_soroban_sections_and_metadata`
- **Location:** `crates/sentinel-core/src/adapters/wasm.rs:331-340`
- **Problem:** The test invokes `inspect_wasm(&wasm, 100, 120, 34_560)` where `code_remaining_ledgers = 100` and `critical_ledgers = 34_560`. By definition, `100 <= 34_560`, so `report.nearing_archival` is `true`. However, line 340 asserts `assert!(!report.nearing_archival);`.
- **Why It Matters:** The test fails unconditionally on valid logic.
- **Affected Contributor PRs:** PR #104 (WASM inspection).
- **Risk Level:** Medium (test failure).
- **Recommended Correction:** Set `code_remaining_ledgers` to a safe threshold (e.g., `50_000`), or assert `assert!(report.nearing_archival)` when testing critical TTL boundaries.

---

### Finding 5: Bytecode Size Assertion Mismatch in `test_scan_happy_path_with_wasm_and_persistent_keys`
- **Location:** `crates/sentinel-core/src/usecases/scan.rs:1129`
- **Problem:** Test fixture passes `b"\0asm\x01\0\0\0"` (8 bytes) as code bytecode, but asserts `assert_eq!(code.contract_code_size_bytes, Some(4));`.
- **Why It Matters:** Causes test assertion failure (`left: Some(8), right: Some(4)`).
- **Affected Contributor PRs:** PR #104 (updated WASM fixture to 8-byte valid header) and PR #105 (added contract code size assert expecting 4 bytes).
- **Risk Level:** Medium (test assertion failure).
- **Recommended Correction:** Update assertion to `Some(8)` to match the 8-byte WASM header.

---

### Finding 6: Unused Workspace Manifest Dependencies & Package Fields
- **Location:** `Cargo.toml:13, 14, 37, 39`
- **Problem:**
  1. `clap_complete = "4.5"` and `schemars = "0.8"` are declared under `[workspace.dependencies]`, but `crates/sentinel-cli/Cargo.toml` declares them directly rather than via `.workspace = true`.
  2. `repository` and `readme` fields in `[workspace.package]` trigger `cargo::unused_workspace_package_fields`.
  3. `hdrhistogram = "7.5"` in `crates/sentinel-cli/Cargo.toml` is unreferenced in CLI code.
- **Why It Matters:** Produces compiler and workspace warnings; clutters dependencies.
- **Affected Contributor PRs:** PR #85, #94, #196, #200.
- **Risk Level:** Low (cleanliness / warning elimination).
- **Recommended Correction:** Use `.workspace = true` in `sentinel-cli`, remove unused `hdrhistogram`, and reconcile workspace package metadata.

---

### Finding 7: Dead Code Warnings in `crates/sentinel-cli/src/commands/scan.rs`
- **Location:** `crates/sentinel-cli/src/commands/scan.rs:180-205`
- **Problem:** `exit_code_for_error`, `is_rpc_failure`, and `exit_code_for_status` are declared `pub(crate)` but never invoked internally.
- **Why It Matters:** Emits `#[warn(dead_code)]` warnings during build.
- **Affected Contributor PRs:** PR #85, #105.
- **Risk Level:** Low.
- **Recommended Correction:** Either remove or use appropriately in CLI dispatch logic.

---

### Finding 8: Clippy Lint in `crates/sentinel-core/src/adapters/wasm.rs`
- **Location:** `crates/sentinel-core/src/adapters/wasm.rs:277:15`
- **Problem:** `while out.len() % 4 != 0` triggers `clippy::manual_is_multiple_of`.
- **Why It Matters:** Fails `cargo clippy -- -D warnings`.
- **Affected Contributor PRs:** PR #104.
- **Risk Level:** Low.
- **Recommended Correction:** Replace with `!out.len().is_multiple_of(4)`.
