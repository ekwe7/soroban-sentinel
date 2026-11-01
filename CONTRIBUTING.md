# Contributing to Soroban Sentinel

Thank you for your interest in contributing to **Soroban Sentinel**!

Soroban Sentinel is an autonomous storage Time-To-Live (TTL) monitoring, risk detection, and maintenance toolkit for Soroban smart contracts on the Stellar network. It is structured as a modular Rust workspace consisting of two primary crates:
* `crates/sentinel-core`: Domain models, risk evaluation engine, and Soroban RPC adapters.
* `crates/sentinel-cli`: CLI binary (`sentinel`) and output formatting adapters.

---

## 1. Getting Started

### Prerequisites
* **Rust**: Stable toolchain (Edition 2021). You can install Rust via [rustup.rs](https://rustup.rs/).
* **Git**: To manage branches and pull requests.

### Local Installation & Build
```bash
# Clone the repository
git clone https://github.com/ekwe7/soroban-sentinel.git
cd soroban-sentinel

# Build all workspace crates
cargo build --workspace

# Run the CLI binary help command
cargo run -p sentinel-cli -- scan --help
```

---

## 2. Selecting and Claiming Tasks

1. **Browse Issues**: Check the open issues on GitHub or inspect the curated backlog in [docs/OPEN_SOURCE_CONTRIBUTION_PLAN.md](docs/OPEN_SOURCE_CONTRIBUTION_PLAN.md).
2. **Task Levels**:
   * **Trivial**: CLI flags, formatting adapters, pure domain math unit tests. Ideal for first-time contributors.
   * **Medium**: XDR parsing, RPC chunking/retry adapters, webhook dispatchers.
   * **High**: Transaction simulation, cryptographic envelope assembly, keeper safety guards.
3. **Claiming an Issue**: To avoid duplicate work, comment on the target GitHub issue (or apply through the active community program portal, such as Drips Wave) and wait for assignment before submitting significant work.

---

## 3. Branching & Development Workflow

1. Ensure your local `main` branch is up to date:
   ```bash
   git checkout main
   git pull origin main
   ```
2. Create a dedicated feature or bugfix branch off `main`:
   ```bash
   # Documented convention: feature/issue-<number>-<short-description>
   git checkout -b feature/issue-2-json-output
   ```
3. Keep changes focused. Avoid unrelated refactorings or style adjustments outside the scope of the assigned task.

---

## 4. Quality Standards & Verification Gates

Before submitting a Pull Request, all changes must pass the workspace verification checks. Run these commands locally from the repository root:

1. **Formatting Check**:
   ```bash
   cargo fmt --all -- --check
   ```
   *(To auto-format code, run `cargo fmt --all`)*

2. **Linter & Warning Checks** (zero warnings allowed):
   ```bash
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   ```

3. **Workspace Test Suite**:
   ```bash
   cargo test --workspace --all-features
   ```

4. **Dependency & Lockfile Integrity**:
   ```bash
   cargo check --workspace --all-features --locked
   ```

---

## 5. Pull Request Expectations

* **Target Branch**: Submit PRs against the `main` branch.
* **Link Issue**: Reference the corresponding issue in your PR title or description (e.g., `Closes #2` or `Fixes #14`).
* **Description & Verification**:
  * Clearly describe what changed and why.
  * List the tests added or modified.
  * Note how the change was verified (e.g., test output, CLI command run).
* **Test Coverage**:
  * New features and bugfixes must include corresponding unit or integration tests.
  * Unit tests must remain offline and deterministic without requiring live internet access or external RPC infrastructure.

---

## 6. Security & Safe Coding Practices

* **Read-Only by Default**: The inspection CLI does not require, accept, or process private keys or secret seeds. Never add unnecessary secret-handling logic to inspection workflows.
* **No Secrets**: Never commit private keys, API tokens, or credentials to Git.
* **Memory Safety**: `soroban-sentinel` relies strictly on safe Rust. Do not introduce `unsafe` code blocks without prior architectural review.
* **Input Validation**: All user-supplied contract addresses and storage keys must be strictly validated using `stellar-strkey` and `stellar-xdr`.
* **Security Disclosures**: If you discover a security vulnerability, please refer to [docs/SECURITY.md](docs/SECURITY.md) for reporting guidelines.

---

## 7. Additional References

* [docs/OPEN_SOURCE_CONTRIBUTION_PLAN.md](docs/OPEN_SOURCE_CONTRIBUTION_PLAN.md) — Detailed backlog and task complexity breakdown.
* [docs/ARCHITECTURE_DECISION.md](docs/ARCHITECTURE_DECISION.md) — Architectural pattern (Hexagonal / Ports & Adapters).
* [docs/SECURITY.md](docs/SECURITY.md) — Security policy and threat model.
