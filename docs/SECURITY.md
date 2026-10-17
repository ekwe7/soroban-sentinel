# Security Architecture, Threat Model & Safe Operation Policy

**Project:** `soroban-sentinel`  
**Architecture:** Modular Monolith (Single Binary CLI)  
**Security Posture:** Read-Only by Default / Strict Defense-in-Depth  
**Last Updated:** Baseline Specification  

---

## 1. Executive Security Philosophy: "Read-Only by Default"

The cornerstone of `soroban-sentinel`'s security architecture is **absolute privilege minimization**:

> [!IMPORTANT]
> **Read-Only Guarantee (MVP)**: In the MVP release (v0.1.0), `soroban-sentinel` requires **no private keys, no seed phrases, no signing authority, and no write permissions to any blockchain or database**. It queries public ledger state using unauthenticated JSON-RPC 2.0 endpoints. Even in the event of total host compromise running `sentinel scan`, an attacker gains zero access to funds or contract authority.

---

## 2. Threat Modeling & Attack Vectors

| Threat Vector | Severity | Vulnerability Description | Mitigation Strategy |
| :--- | :--- | :--- | :--- |
| **Malicious RPC Tampering** | Medium | A malicious or compromised RPC endpoint returns spoofed `liveUntilLedgerSeq` values to trigger false alarms or hide expiration. | • Support for user-specified RPC URLs (`--rpc-url`).<br>• TLS certificate verification enforced by default via `rustls`.<br>• Clear terminal disclaimers that output reflects the queried endpoint's view of ledger state. |
| **Denial of Service via Heavy Key Lists** | Low | An adversary supplies an excessively long list of `--key` parameters to exhaust memory. | • Enforced chunking (200 keys per RPC batch).<br>• Maximum parameter limits enforced during clap CLI parsing. |
| **Input Injection / Malformed Strkeys** | Low | An adversary passes crafted strings to exploit buffer overflows or formatting parsers. | • Strict Strkey validation via `stellar-strkey` crate (`C...` 56-character string).<br>• Memory-safe Rust implementation with zero unsafe blocks. |
| **Keeper Wallet Fund Drainage (Post-MVP)** | High | When autonomous keeper mode is enabled in future releases, a bug or loop repeatedly calls `ExtendFootprintTTLOp`, draining operator XLM. | • Hard daily XLM spend limit enforced in software.<br>• Minimum balance safety floor (aborts if keeper account `< 5 XLM`).<br>• Rate limiting bump operations per contract. |
| **State Tampering via Keeper Key (Post-MVP)** | Low / None | An attacker compromises the keeper key and attempts to modify contract business logic. | • The keeper account has zero contract administrative privileges.<br>• Keeper transactions assemble ONLY `ExtendFootprintTTLOp` with an empty `readWrite` footprint. Stellar protocol cryptographically prevents state mutations via this operation. |

---

## 3. Secret Management & Signing Boundaries (Post-MVP Keeper)

For the future autonomous keeper feature (`sentinel bump` / `--auto-bump` in Phase 6):

```
+-----------------------------------------------------------------------------------+
|                            KEEPER SIGNING ISOLATION                               |
|                                                                                   |
|  1. Key Loading:                                                                  |
|     • Read exclusively from environment variable SENTINEL_KEEPER_SECRET           |
|     • Stored in memory using zeroize / secrecy crate (scrubbed on drop)           |
|     • Never logged to stdout, stderr, or logfiles                                 |
|                                                                                   |
|  2. Account Separation:                                                           |
|     • Must be a dedicated "Keeper Account" created specifically for fee bumping   |
|     • NEVER reuse contract admin, multisig signers, or treasury keys              |
|     • Fund with minimal working capital (e.g., 20–50 XLM)                         |
|                                                                                   |
|  3. Operation Restriction:                                                        |
|     • The envelope builder enforces that the transaction contains exactly ONE     |
|       operation: ExtendFootprintTTLOp                                             |
|     • readWrite footprint array is strictly asserted to be EMPTY                  |
|     • Rejects any attempt to append InvokeHostFunctionOp or payment operations    |
+-----------------------------------------------------------------------------------+
```

---

## 4. Risks Associated with Autonomous TTL Bumping (Post-MVP)

While automated keepers provide unattended reliability in future milestones, operators must understand the protocol-level failure modes:

1. **Non-Refundable Storage Rent Fees**: When `ExtendFootprintTTLOp` executes, the storage rent fee is burned/consumed by the network. If an operator accidentally configures a bump horizon of 10,000,000 ledgers (~1.5 years) for a large WASM code entry, significant XLM will be spent permanently.
   * *Mitigation*: Sentinel enforces a strict maximum extension ceiling (default: max 500,000 ledgers per single transaction).
2. **Transaction Sequence Number Desynchronization**: If the keeper account is used concurrently by another script or wallet, sequence numbers will conflict (`txBAD_SEQ`).
   * *Mitigation*: Dedicated account recommendation; the bumper fetches fresh account sequence directly before transaction submission with internal mutex locking.
3. **RPC Transaction Simulation Divergence**: Network parameters could shift between simulation and ledger inclusion.
   * *Mitigation*: Sentinel adds a 15% safety buffer to the simulated resource fee to ensure inclusion during transient fee spikes.

---

## 5. Security Checklist for Developers & Operators

- [ ] Use official, trusted Soroban RPC endpoints (e.g., SDF public testnet/mainnet or dedicated QuickNode/NowNodes providers).
- [ ] In automated CI/CD pipelines, run `sentinel scan` with unprivileged service tokens.
- [ ] Never pass private keys or secret seeds to `sentinel scan` (it does not accept them).
- [ ] If evaluating future keeper features in staging:
  - [ ] Use a brand new Stellar keypair with zero prior history.
  - [ ] Fund the keeper account with less than 20 XLM.
  - [ ] Set strict daily spend limits.

---

## 6. Vulnerability Disclosure & Bug Bounty

We take the security of `soroban-sentinel` and the Stellar ecosystem seriously.

If you discover a security vulnerability or critical bug:
1. **DO NOT** file a public GitHub issue.
2. Send a detailed report to the security maintainers via private security advisory on GitHub.
3. Include:
   * Description of the vulnerability.
   * Steps to reproduce or proof-of-concept code.
   * Assessment of potential impact.
4. Maintainers will acknowledge receipt within 48 hours and coordinate a coordinated disclosure schedule.
