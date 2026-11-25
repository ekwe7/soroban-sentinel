//! Pure domain logic for multisig threshold safety verification.
//!
//! Before the automated bumper executes an `ExtendFootprintTTLOp`, it must
//! confirm that the configured signing key carries enough weight to satisfy
//! the account's **medium threshold** – the threshold required for all
//! non-high-security operations, including `ExtendFootprintTtl`.
//!
//! This module is intentionally free of I/O. It operates on
//! [`AccountSignerConfig`] values produced by the adapter layer.

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Error type ───────────────────────────────────────────────────────────────

/// Errors produced during multisig threshold verification.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum MultisigError {
    /// The bumper's signing key was not found among the account's signers and
    /// is also not the account master key.
    #[error("Signing key '{bumper_key}' is not an authorised signer on account '{account}'.")]
    SignerNotFound { bumper_key: String, account: String },

    /// The bumper's total weight is below the medium threshold required for
    /// `ExtendFootprintTTLOp`.
    #[error(
        "Insufficient signing weight for '{account}': bumper weight {bumper_weight} \
         is below the medium threshold {medium_threshold}. \
         Additional co-signers needed: {additional_signers:?}"
    )]
    InsufficientWeight {
        account: String,
        bumper_weight: u32,
        medium_threshold: u32,
        /// Public key strkeys of additional signers that could collectively
        /// close the weight gap.
        additional_signers: Vec<String>,
    },

    /// The account was not found on the network.
    #[error("Source account '{0}' was not found on the network.")]
    AccountNotFound(String),

    /// An XDR parsing error occurred while decoding the account entry.
    #[error("Failed to decode account entry XDR for '{account}': {reason}")]
    XdrDecodeError { account: String, reason: String },
}

// ── Domain types ─────────────────────────────────────────────────────────────

/// A single authorised signer on a Stellar account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignerEntry {
    /// Stellar Strkey (`G...`) of the signer public key.
    pub public_key_strkey: String,
    /// Signing weight assigned to this key.
    pub weight: u32,
}

impl SignerEntry {
    /// Create a new `SignerEntry`.
    pub fn new(public_key_strkey: impl Into<String>, weight: u32) -> Self {
        Self {
            public_key_strkey: public_key_strkey.into(),
            weight,
        }
    }
}

/// The thresholds for a Stellar account expressed as ledger weights.
///
/// Per the Stellar protocol:
/// - `master_weight` – weight of the account master key (index 0 in `Thresholds`).
/// - `low`           – required for low-privilege operations.
/// - `medium`        – required for most operations, including `ExtendFootprintTTLOp`.
/// - `high`          – required for critical account operations (e.g. `SetOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountThresholds {
    pub master_weight: u8,
    pub low: u8,
    pub medium: u8,
    pub high: u8,
}

impl AccountThresholds {
    /// Construct thresholds from the raw 4-byte `Thresholds` array.
    ///
    /// The Stellar protocol encodes thresholds as `[master_weight, low, medium, high]`.
    pub fn from_bytes(bytes: [u8; 4]) -> Self {
        Self {
            master_weight: bytes[0],
            low: bytes[1],
            medium: bytes[2],
            high: bytes[3],
        }
    }
}

/// Complete signer configuration for a Stellar account, as resolved from the
/// ledger.  This is the input consumed by [`verify_bumper_authority`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSignerConfig {
    /// Stellar Strkey (`G...`) of the account itself (master key).
    pub account_strkey: String,
    /// Weight thresholds for the account.
    pub thresholds: AccountThresholds,
    /// All additional signers registered on the account (excludes master key).
    /// The master key is implicitly included at `thresholds.master_weight`.
    pub signers: Vec<SignerEntry>,
}

impl AccountSignerConfig {
    /// Construct a new `AccountSignerConfig`.
    pub fn new(
        account_strkey: impl Into<String>,
        thresholds: AccountThresholds,
        signers: Vec<SignerEntry>,
    ) -> Self {
        Self {
            account_strkey: account_strkey.into(),
            thresholds,
            signers,
        }
    }

    /// Returns the weight of a given key on this account.
    ///
    /// Checks the master key first, then the additional signers list.
    /// Returns `None` if the key is not an authorised signer.
    pub fn weight_of(&self, public_key_strkey: &str) -> Option<u32> {
        if public_key_strkey == self.account_strkey {
            return Some(u32::from(self.thresholds.master_weight));
        }
        self.signers
            .iter()
            .find(|s| s.public_key_strkey == public_key_strkey)
            .map(|s| s.weight)
    }
}

// ── Verification result ───────────────────────────────────────────────────────

/// Outcome of a successful multisig threshold safety check.
///
/// A value of this type is only returned when the bumper key is an authorised
/// signer on the account.  Insufficient weight is surfaced as
/// [`MultisigError::InsufficientWeight`] rather than as a separate variant
/// here, keeping the success path and the error path orthogonal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultisigVerificationResult {
    /// Effective weight of the bumper key on the account.
    pub bumper_weight: u32,
    /// Medium threshold the bumper weight was compared against.
    pub medium_threshold: u32,
}

impl MultisigVerificationResult {
    /// Returns `true` if the bumper can sign alone (weight ≥ medium threshold,
    /// or medium threshold is 0).
    pub fn is_sufficient(&self) -> bool {
        self.medium_threshold == 0 || self.bumper_weight >= self.medium_threshold
    }
}

// ── Core verification logic ────────────────────────────────────────────────────

/// Verify that `bumper_key` has sufficient authority to sign an
/// `ExtendFootprintTTLOp` for `account`.
///
/// The function checks whether the bumper's weight on the account meets or
/// exceeds the **medium threshold**.  If the threshold is zero the function
/// treats any positive weight as sufficient (a threshold of zero disables the
/// check on the Stellar network).
///
/// # Errors
///
/// Returns [`MultisigError::SignerNotFound`] when `bumper_key` is not an
/// authorised signer at all, or [`MultisigError::InsufficientWeight`] when
/// the key exists but its weight is below the medium threshold.
pub fn verify_bumper_authority(
    account: &AccountSignerConfig,
    bumper_key: &str,
) -> Result<MultisigVerificationResult, MultisigError> {
    let bumper_weight =
        account
            .weight_of(bumper_key)
            .ok_or_else(|| MultisigError::SignerNotFound {
                bumper_key: bumper_key.to_owned(),
                account: account.account_strkey.clone(),
            })?;

    let medium_threshold = u32::from(account.thresholds.medium);

    // A medium threshold of 0 means no threshold enforced; treat as satisfied.
    if medium_threshold == 0 || bumper_weight >= medium_threshold {
        return Ok(MultisigVerificationResult {
            bumper_weight,
            medium_threshold,
        });
    }

    // Identify candidates that could close the weight gap, sorted by weight
    // descending so the caller can greedily suggest the minimum set.
    let _weight_deficit = medium_threshold - bumper_weight;
    let mut candidates: Vec<SignerEntry> = account
        .signers
        .iter()
        .filter(|s| s.public_key_strkey != bumper_key && s.weight > 0)
        .cloned()
        .collect();
    candidates.sort_by_key(|a| std::cmp::Reverse(a.weight));

    // Also consider the master key if it isn't the bumper.
    let master_weight = u32::from(account.thresholds.master_weight);
    if account.account_strkey != bumper_key && master_weight > 0 {
        // Insert master key at the right position by weight.
        let master_entry = SignerEntry::new(account.account_strkey.clone(), master_weight);
        let pos = candidates
            .binary_search_by(|s| s.weight.cmp(&master_weight).reverse())
            .unwrap_or_else(|i| i);
        candidates.insert(pos, master_entry);
    }

    let additional_signers_strkeys: Vec<String> = candidates
        .iter()
        .map(|s| s.public_key_strkey.clone())
        .collect();

    Err(MultisigError::InsufficientWeight {
        account: account.account_strkey.clone(),
        bumper_weight,
        medium_threshold,
        additional_signers: additional_signers_strkeys,
    })
}

/// Identify which signers from `available_cosigners` are required to bring the
/// total signing weight above `medium_threshold` when combined with
/// `bumper_weight`.
///
/// Selects the minimum greedy subset (sorted by weight descending) that
/// satisfies the gap.  Returns an empty vec when the bumper already satisfies
/// the threshold alone.
pub fn required_cosigners(
    account: &AccountSignerConfig,
    bumper_key: &str,
    bumper_weight: u32,
) -> Vec<SignerEntry> {
    let medium_threshold = u32::from(account.thresholds.medium);
    if medium_threshold == 0 || bumper_weight >= medium_threshold {
        return Vec::new();
    }

    let mut gap = medium_threshold.saturating_sub(bumper_weight);
    let mut candidates: Vec<SignerEntry> = account
        .signers
        .iter()
        .filter(|s| s.public_key_strkey != bumper_key && s.weight > 0)
        .cloned()
        .collect();

    // Include master key when it is not the bumper.
    if account.account_strkey != bumper_key {
        let master_weight = u32::from(account.thresholds.master_weight);
        if master_weight > 0 {
            candidates.push(SignerEntry::new(
                account.account_strkey.clone(),
                master_weight,
            ));
        }
    }

    candidates.sort_by_key(|a| std::cmp::Reverse(a.weight));

    let mut result = Vec::new();
    for signer in candidates {
        if gap == 0 {
            break;
        }
        gap = gap.saturating_sub(signer.weight);
        result.push(signer);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_signer_account(master_weight: u8, medium: u8) -> AccountSignerConfig {
        AccountSignerConfig::new(
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            AccountThresholds {
                master_weight,
                low: 0,
                medium,
                high: 0,
            },
            vec![],
        )
    }

    #[test]
    fn master_key_meets_threshold() {
        let account = single_signer_account(1, 1);
        let result = verify_bumper_authority(
            &account,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .unwrap();
        assert!(result.is_sufficient());
        assert_eq!(
            result,
            MultisigVerificationResult {
                bumper_weight: 1,
                medium_threshold: 1
            }
        );
    }

    #[test]
    fn zero_medium_threshold_always_sufficient() {
        let account = single_signer_account(1, 0);
        let result = verify_bumper_authority(
            &account,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .unwrap();
        assert!(result.is_sufficient());
    }

    #[test]
    fn unregistered_key_returns_signer_not_found() {
        let account = single_signer_account(1, 1);
        let err = verify_bumper_authority(
            &account,
            "GBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        )
        .unwrap_err();
        assert!(matches!(err, MultisigError::SignerNotFound { .. }));
    }

    #[test]
    fn additional_signer_meets_threshold() {
        let account = AccountSignerConfig::new(
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            AccountThresholds {
                master_weight: 1,
                low: 1,
                medium: 2,
                high: 3,
            },
            vec![SignerEntry::new(
                "GBAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
                2,
            )],
        );
        let result = verify_bumper_authority(
            &account,
            "GBAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .unwrap();
        assert!(result.is_sufficient());
    }

    #[test]
    fn insufficient_weight_returns_error_with_cosigners() {
        let cosigner = "GCAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let account = AccountSignerConfig::new(
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            AccountThresholds {
                master_weight: 2,
                low: 1,
                medium: 3,
                high: 5,
            },
            vec![
                SignerEntry::new(
                    "GBAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
                    1,
                ),
                SignerEntry::new(cosigner, 2),
            ],
        );
        let err = verify_bumper_authority(
            &account,
            "GBAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
        .unwrap_err();

        match err {
            MultisigError::InsufficientWeight {
                bumper_weight,
                medium_threshold,
                ref additional_signers,
                ..
            } => {
                assert_eq!(bumper_weight, 1);
                assert_eq!(medium_threshold, 3);
                // Should list signers that can close the gap, master key
                // first because it has weight 2.
                assert!(additional_signers.contains(&cosigner.to_owned()));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn required_cosigners_empty_when_sufficient() {
        let account = single_signer_account(2, 2);
        let cosigners = required_cosigners(
            &account,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            2,
        );
        assert!(cosigners.is_empty());
    }

    #[test]
    fn required_cosigners_greedy_minimum_set() {
        let account = AccountSignerConfig::new(
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            AccountThresholds {
                master_weight: 1,
                low: 0,
                medium: 5,
                high: 0,
            },
            vec![
                SignerEntry::new(
                    "GBAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
                    3,
                ),
                SignerEntry::new(
                    "GCAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
                    2,
                ),
                SignerEntry::new(
                    "GDAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
                    1,
                ),
            ],
        );
        // Bumper has weight 1, medium = 5, needs 4 more.
        let cosigners = required_cosigners(
            &account,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            1,
        );
        // Greedy: take weight-3 signer first, then weight-2 → total gap closed.
        assert_eq!(cosigners.len(), 2);
        assert_eq!(
            cosigners[0].public_key_strkey,
            "GBAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
        );
        assert_eq!(
            cosigners[1].public_key_strkey,
            "GCAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
        );
    }

    #[test]
    fn account_thresholds_from_bytes() {
        let t = AccountThresholds::from_bytes([2, 1, 3, 5]);
        assert_eq!(t.master_weight, 2);
        assert_eq!(t.low, 1);
        assert_eq!(t.medium, 3);
        assert_eq!(t.high, 5);
    }

    #[test]
    fn weight_of_returns_none_for_unknown_key() {
        let account = single_signer_account(1, 1);
        assert!(account.weight_of("GUNKNOWNKEYSTRKEY").is_none());
    }

    #[test]
    fn weight_of_returns_master_weight() {
        let account = single_signer_account(3, 2);
        assert_eq!(
            account.weight_of("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"),
            Some(3)
        );
    }
}
