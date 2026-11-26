//! Stellar account adapter: fetches signer configuration from Soroban RPC.
//!
//! Account entries are stored in the Stellar ledger as ordinary `LedgerEntry`
//! values and are accessible through the standard `getLedgerEntries` RPC call.
//! This adapter derives the `LedgerKey::Account` for a given Strkey address,
//! retrieves the entry, and parses it into the domain type
//! [`AccountSignerConfig`].

use stellar_strkey::ed25519::PublicKey as Ed25519PublicKey;
use stellar_xdr::curr::{
    AccountId, LedgerEntry, LedgerEntryData, LedgerKey, LedgerKeyAccount, PublicKey, ReadXdr,
    Uint256, WriteXdr,
};

use crate::adapters::safe_xdr_limits;
use crate::domain::multisig::{AccountSignerConfig, AccountThresholds, MultisigError, SignerEntry};
use crate::ports::reader::LedgerReader;

/// Derives the base64-encoded `LedgerKey::Account` XDR for a Stellar account
/// identified by its `G...` Strkey address.
fn account_ledger_key(account_strkey: &str) -> Result<String, MultisigError> {
    let pk = Ed25519PublicKey::from_string(account_strkey).map_err(|e| {
        MultisigError::XdrDecodeError {
            account: account_strkey.to_owned(),
            reason: format!("Invalid account strkey: {e}"),
        }
    })?;

    let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));
    let key = LedgerKey::Account(LedgerKeyAccount { account_id });

    key.to_xdr_base64(safe_xdr_limits())
        .map_err(|e| MultisigError::XdrDecodeError {
            account: account_strkey.to_owned(),
            reason: format!("Failed to encode account key XDR: {e}"),
        })
}

/// Converts a raw `SignerKey` to its `G...` Strkey representation.
///
/// Only Ed25519 keys are representable as `G...` addresses; other key types
/// (pre-auth, hash-x, signed-payload) are encoded as hex strings prefixed
/// with their discriminant so they remain identifiable but do not parse as
/// account Strkeys.
fn signer_key_to_strkey(key: &stellar_xdr::curr::SignerKey) -> String {
    use stellar_xdr::curr::SignerKey;
    match key {
        SignerKey::Ed25519(Uint256(bytes)) => Ed25519PublicKey(*bytes).to_string(),
        SignerKey::PreAuthTx(Uint256(bytes)) => {
            format!("pre-auth:{}", bytes_to_hex(bytes))
        }
        SignerKey::HashX(Uint256(bytes)) => {
            format!("hash-x:{}", bytes_to_hex(bytes))
        }
        SignerKey::Ed25519SignedPayload(payload) => {
            format!(
                "signed-payload:{}:{}",
                bytes_to_hex(&payload.ed25519.0),
                bytes_to_hex(&payload.payload)
            )
        }
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Fetch the full signer configuration for a Stellar account from the network.
///
/// # Parameters
///
/// * `reader`          – Any [`LedgerReader`] implementation (live RPC client
///   or mock).
/// * `account_strkey`  – The `G...` Strkey public key of the account.
///
/// # Errors
///
/// Returns [`MultisigError::AccountNotFound`] when the account does not exist
/// on the network, [`MultisigError::XdrDecodeError`] on parsing failures, and
/// propagates any RPC communication error wrapped in
/// [`MultisigError::XdrDecodeError`].
pub async fn fetch_account_signer_config(
    reader: &dyn LedgerReader,
    account_strkey: &str,
) -> Result<AccountSignerConfig, MultisigError> {
    let key_xdr = account_ledger_key(account_strkey)?;

    let entries =
        reader
            .get_ledger_entries(&[key_xdr])
            .await
            .map_err(|e| MultisigError::XdrDecodeError {
                account: account_strkey.to_owned(),
                reason: format!("RPC error: {e}"),
            })?;

    let raw = entries
        .into_iter()
        .next()
        .ok_or_else(|| MultisigError::AccountNotFound(account_strkey.to_owned()))?;

    let ledger_entry = LedgerEntry::from_xdr_base64(&raw.entry_xdr_base64, safe_xdr_limits())
        .map_err(|e| MultisigError::XdrDecodeError {
            account: account_strkey.to_owned(),
            reason: format!("Failed to decode LedgerEntry XDR: {e}"),
        })?;

    let account_entry = match ledger_entry.data {
        LedgerEntryData::Account(ae) => ae,
        other => {
            return Err(MultisigError::XdrDecodeError {
                account: account_strkey.to_owned(),
                reason: format!("Expected Account entry, got {:?}", other.name()),
            });
        }
    };

    let thresholds = AccountThresholds::from_bytes(account_entry.thresholds.0);

    // The XDR `signers` list contains all signers *except* the master key.
    let signers: Vec<SignerEntry> = account_entry
        .signers
        .iter()
        .map(|s| SignerEntry::new(signer_key_to_strkey(&s.key), s.weight))
        .collect();

    Ok(AccountSignerConfig::new(
        account_strkey,
        thresholds,
        signers,
    ))
}

// ── Helpers for constructing test XDR fixtures ────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::multisig::MultisigError;
    use crate::ports::mock::MockLedgerReader;
    use crate::ports::reader::RawLedgerEntry;

    fn bumper_strkey() -> String {
        // Ed25519PublicKey([0x00; 32]) = GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF
        Ed25519PublicKey([0x00; 32]).to_string()
    }

    fn cosigner_strkey() -> String {
        Ed25519PublicKey([0x11; 32]).to_string()
    }

    fn make_key_xdr(strkey: &str) -> String {
        account_ledger_key(strkey).expect("valid strkey")
    }

    fn build_account_entry_xdr(
        account_strkey: &str,
        thresholds: [u8; 4],
        signers: &[(&str, u32)],
    ) -> String {
        use stellar_xdr::curr::{
            AccountEntry, AccountEntryExt, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limits,
            SequenceNumber, Signer, SignerKey, String32, Thresholds, VecM,
        };
        let pk = Ed25519PublicKey::from_string(account_strkey).expect("valid account strkey");
        let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));
        let xdr_signers: Vec<Signer> = signers
            .iter()
            .map(|(s, w)| {
                let spk = Ed25519PublicKey::from_string(s).expect("valid signer strkey");
                Signer {
                    key: SignerKey::Ed25519(Uint256(spk.0)),
                    weight: *w,
                }
            })
            .collect();
        let entry = LedgerEntry {
            last_modified_ledger_seq: 0,
            data: LedgerEntryData::Account(AccountEntry {
                account_id,
                balance: 100_000_000,
                seq_num: SequenceNumber(1),
                num_sub_entries: 0,
                inflation_dest: None,
                flags: 0,
                home_domain: String32::default(),
                thresholds: Thresholds(thresholds),
                signers: VecM::try_from(xdr_signers).expect("signers fit"),
                ext: AccountEntryExt::V0,
            }),
            ext: LedgerEntryExt::V0,
        };
        entry.to_xdr_base64(Limits::none()).expect("encode entry")
    }

    fn make_raw_entry(
        account_strkey: &str,
        thresholds: [u8; 4],
        signers: &[(&str, u32)],
    ) -> RawLedgerEntry {
        let key_xdr = make_key_xdr(account_strkey);
        let entry_xdr = build_account_entry_xdr(account_strkey, thresholds, signers);
        RawLedgerEntry::new(key_xdr, entry_xdr, 0, 0)
    }

    #[tokio::test]
    async fn fetches_single_signer_account() {
        let bumper = bumper_strkey();
        let entry = make_raw_entry(&bumper, [1, 0, 1, 0], &[]);
        let reader = MockLedgerReader::new().with_entry(entry);
        let config = fetch_account_signer_config(&reader, &bumper).await.unwrap();
        assert_eq!(config.account_strkey, bumper);
        assert_eq!(config.thresholds.master_weight, 1);
        assert_eq!(config.thresholds.medium, 1);
        assert!(config.signers.is_empty());
    }

    #[tokio::test]
    async fn fetches_multisig_account() {
        let bumper = bumper_strkey();
        let cosigner = cosigner_strkey();
        let entry = make_raw_entry(&bumper, [1, 1, 2, 3], &[(&cosigner, 2)]);
        let reader = MockLedgerReader::new().with_entry(entry);
        let config = fetch_account_signer_config(&reader, &bumper).await.unwrap();
        assert_eq!(config.thresholds.medium, 2);
        assert_eq!(config.signers.len(), 1);
        assert_eq!(config.signers[0].public_key_strkey, cosigner);
        assert_eq!(config.signers[0].weight, 2);
    }

    #[tokio::test]
    async fn returns_account_not_found_when_no_entry() {
        let bumper = bumper_strkey();
        let reader = MockLedgerReader::new();
        let err = fetch_account_signer_config(&reader, &bumper)
            .await
            .unwrap_err();
        assert!(matches!(err, MultisigError::AccountNotFound(_)));
    }

    #[tokio::test]
    async fn returns_xdr_error_on_bad_entry_data() {
        let bumper = bumper_strkey();
        let key_xdr = make_key_xdr(&bumper);
        let entry = RawLedgerEntry::new(key_xdr, "not-valid-base64".to_string(), 0, 0);
        let reader = MockLedgerReader::new().with_entry(entry);
        let err = fetch_account_signer_config(&reader, &bumper)
            .await
            .unwrap_err();
        assert!(matches!(err, MultisigError::XdrDecodeError { .. }));
    }
}
