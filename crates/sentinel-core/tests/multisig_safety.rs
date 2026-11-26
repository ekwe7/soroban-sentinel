//! Integration and unit tests for multisig threshold safety (Issue #58).
//!
//! Coverage:
//! 1. Single-signer account where the bumper meets the threshold.
//! 2. Multisig account where the bumper's weight satisfies the medium threshold.
//! 3. Multisig account where the bumper's weight is insufficient.
//! 4. Correct identification of additional required co-signers.
//! 5. Descriptive safety error when the threshold is not satisfied.
//! 6. Adapter: fetch_account_signer_config parses a valid XDR account entry.
//! 7. Adapter: returns AccountNotFound for a missing account.
//! 8. Adapter: wiremock end-to-end via SorobanRpcClient.

use sentinel_core::adapters::stellar_account::fetch_account_signer_config;
use sentinel_core::domain::{
    required_cosigners, verify_bumper_authority, AccountSignerConfig, AccountThresholds,
    MultisigError, MultisigVerificationResult, SignerEntry,
};
use sentinel_core::ports::mock::MockLedgerReader;
use sentinel_core::ports::reader::RawLedgerEntry;
use sentinel_core::{RetryPolicy, SorobanRpcClient};

use stellar_strkey::ed25519::PublicKey as Ed25519PublicKey;
use stellar_xdr::curr::{
    AccountEntry, AccountEntryExt, AccountId, LedgerEntry, LedgerEntryData, LedgerEntryExt,
    LedgerKey, LedgerKeyAccount, Limits, PublicKey, SequenceNumber, Signer, SignerKey, String32,
    Thresholds, Uint256, VecM, WriteXdr,
};

use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Test account Strkey generation ────────────────────────────────────────────
//
// All keys are derived from `Ed25519PublicKey([BYTE; 32]).to_string()` to
// guarantee they pass strkey decoding.  The library encodes with the correct
// version byte and checksum.

fn master_strkey() -> String {
    Ed25519PublicKey([0x00; 32]).to_string() // GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF
}

fn cosigner_a_strkey() -> String {
    Ed25519PublicKey([0x11; 32]).to_string()
}

fn cosigner_b_strkey() -> String {
    Ed25519PublicKey([0x22; 32]).to_string()
}

fn cosigner_c_strkey() -> String {
    Ed25519PublicKey([0x33; 32]).to_string()
}

// ── Test fixture builders ─────────────────────────────────────────────────────

/// Base64-encoded `LedgerKey::Account` XDR for a given account Strkey.
fn account_key_xdr_base64(strkey: &str) -> String {
    let pk = Ed25519PublicKey::from_string(strkey).expect("valid strkey");
    let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));
    let key = LedgerKey::Account(LedgerKeyAccount { account_id });
    key.to_xdr_base64(Limits::none()).expect("encode key")
}

/// Base64-encoded `LedgerEntry` XDR wrapping an `AccountEntry`.
fn build_account_entry_xdr(
    account_strkey: &str,
    thresholds: [u8; 4],
    signers: &[(&str, u32)],
) -> String {
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

fn raw_entry(account_strkey: &str, thresholds: [u8; 4], signers: &[(&str, u32)]) -> RawLedgerEntry {
    let key_xdr = account_key_xdr_base64(account_strkey);
    let entry_xdr = build_account_entry_xdr(account_strkey, thresholds, signers);
    RawLedgerEntry::new(key_xdr, entry_xdr, 0, 0)
}

/// Full RPC `getLedgerEntries` JSON response for an account entry.
fn ledger_entries_response(
    account_strkey: &str,
    thresholds: [u8; 4],
    signers: &[(&str, u32)],
) -> serde_json::Value {
    let key_xdr = account_key_xdr_base64(account_strkey);
    let entry_xdr = build_account_entry_xdr(account_strkey, thresholds, signers);
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "entries": [
                {
                    "key": key_xdr,
                    "xdr": entry_xdr,
                    "liveUntilLedgerSeq": 999_999,
                    "lastModifiedLedgerSeq": 1_000
                }
            ],
            "ledger": 5_000_000
        }
    })
}

fn wiremock_client(server: &MockServer) -> SorobanRpcClient {
    SorobanRpcClient::new(server.uri()).with_retry_policy(RetryPolicy {
        base_delay: Duration::from_millis(1),
        ..RetryPolicy::default()
    })
}

// ── 1. Single-signer account: bumper is the master key and meets the threshold ──

#[test]
fn single_signer_bumper_is_master_and_meets_threshold() {
    let master = master_strkey();
    let account = AccountSignerConfig::new(
        &master,
        AccountThresholds {
            master_weight: 1,
            low: 0,
            medium: 1,
            high: 0,
        },
        vec![],
    );
    let result = verify_bumper_authority(&account, &master).unwrap();
    assert!(result.is_sufficient());
    assert_eq!(
        result,
        MultisigVerificationResult {
            bumper_weight: 1,
            medium_threshold: 1
        }
    );
}

// ── 2. Multisig account: bumper is an extra signer whose weight satisfies medium ──

#[test]
fn multisig_bumper_extra_signer_meets_threshold() {
    let master = master_strkey();
    let bumper = cosigner_a_strkey();
    let account = AccountSignerConfig::new(
        &master,
        AccountThresholds {
            master_weight: 1,
            low: 1,
            medium: 2,
            high: 3,
        },
        vec![SignerEntry::new(&bumper, 2)],
    );
    let result = verify_bumper_authority(&account, &bumper).unwrap();
    assert!(result.is_sufficient());
    assert_eq!(
        result,
        MultisigVerificationResult {
            bumper_weight: 2,
            medium_threshold: 2
        }
    );
}

// ── 3. Multisig account: bumper's weight is insufficient ──────────────────────

#[test]
fn multisig_bumper_insufficient_weight_returns_error() {
    let master = master_strkey();
    let bumper = cosigner_a_strkey();
    let co_b = cosigner_b_strkey();
    let account = AccountSignerConfig::new(
        &master,
        AccountThresholds {
            master_weight: 2,
            low: 1,
            medium: 3,
            high: 5,
        },
        vec![
            SignerEntry::new(&bumper, 1), // the bumper – weight below medium
            SignerEntry::new(&co_b, 2),
        ],
    );
    let err = verify_bumper_authority(&account, &bumper).unwrap_err();
    assert!(
        matches!(
            err,
            MultisigError::InsufficientWeight {
                bumper_weight: 1,
                medium_threshold: 3,
                ..
            }
        ),
        "unexpected error: {err:?}"
    );
}

// ── 4. Correct identification of additional required co-signers ───────────────

#[test]
fn identifies_additional_cosigners_greedy_minimum() {
    // medium = 5, bumper weight = 1 → gap = 4.
    // Available: weight-3 (A), weight-2 (B), weight-1 (C).
    // Greedy pick: 3 + 2 = 5 ≥ 4 → 2 co-signers.
    let master = master_strkey();
    let co_a = cosigner_a_strkey();
    let co_b = cosigner_b_strkey();
    let co_c = cosigner_c_strkey();
    let account = AccountSignerConfig::new(
        &master,
        AccountThresholds {
            master_weight: 1,
            low: 0,
            medium: 5,
            high: 0,
        },
        vec![
            SignerEntry::new(&co_a, 3),
            SignerEntry::new(&co_b, 2),
            SignerEntry::new(&co_c, 1),
        ],
    );
    let cosigners = required_cosigners(&account, &master, 1);
    assert_eq!(
        cosigners.len(),
        2,
        "expected 2 co-signers, got {:?}",
        cosigners
    );
    // Highest weight first.
    assert_eq!(cosigners[0].public_key_strkey, co_a);
    assert_eq!(cosigners[1].public_key_strkey, co_b);
}

#[test]
fn identifies_additional_cosigners_from_error_variant() {
    let master = master_strkey();
    let bumper = cosigner_a_strkey();
    let co_b = cosigner_b_strkey();
    let account = AccountSignerConfig::new(
        &master,
        AccountThresholds {
            master_weight: 2,
            low: 1,
            medium: 3,
            high: 5,
        },
        vec![
            SignerEntry::new(&bumper, 1), // bumper
            SignerEntry::new(&co_b, 2),
        ],
    );
    let err = verify_bumper_authority(&account, &bumper).unwrap_err();
    match err {
        MultisigError::InsufficientWeight {
            additional_signers, ..
        } => {
            assert!(
                additional_signers
                    .iter()
                    .any(|k| k == &co_b || k == &master),
                "expected co_b or master in additional signers, got {additional_signers:?}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

// ── 5. Descriptive safety error ───────────────────────────────────────────────

#[test]
fn error_message_mentions_account_bumper_key_and_weights() {
    let master = master_strkey();
    let bumper = cosigner_a_strkey();
    let account = AccountSignerConfig::new(
        &master,
        AccountThresholds {
            master_weight: 1,
            low: 0,
            medium: 3,
            high: 0,
        },
        vec![SignerEntry::new(&bumper, 1)],
    );
    let err = verify_bumper_authority(&account, &bumper).unwrap_err();
    let msg = err.to_string();
    // The error must mention the account, the threshold, and the bumper weight.
    assert!(msg.contains(&master), "message missing account: {msg}");
    assert!(msg.contains('3'), "message missing threshold value: {msg}");
    assert!(msg.contains('1'), "message missing bumper weight: {msg}");
}

#[test]
fn error_message_for_unknown_key_mentions_key_and_account() {
    let master = master_strkey();
    let unknown = cosigner_a_strkey();
    let account = AccountSignerConfig::new(
        &master,
        AccountThresholds {
            master_weight: 1,
            low: 0,
            medium: 1,
            high: 0,
        },
        vec![],
    );
    let err = verify_bumper_authority(&account, &unknown).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains(&unknown), "message missing unknown key: {msg}");
    assert!(msg.contains(&master), "message missing account: {msg}");
}

// ── 6. Zero medium threshold: any signer is sufficient ───────────────────────

#[test]
fn zero_medium_threshold_always_sufficient_for_authorised_signer() {
    let master = master_strkey();
    let bumper = cosigner_a_strkey();
    let account = AccountSignerConfig::new(
        &master,
        AccountThresholds {
            master_weight: 1,
            low: 0,
            medium: 0,
            high: 0,
        },
        vec![SignerEntry::new(&bumper, 1)],
    );
    assert!(verify_bumper_authority(&account, &bumper)
        .unwrap()
        .is_sufficient());
    assert!(required_cosigners(&account, &bumper, 1).is_empty());
}

// ── 7. Adapter: fetch_account_signer_config with MockLedgerReader ─────────────

#[tokio::test]
async fn adapter_fetches_single_signer_account() {
    let master = master_strkey();
    let entry = raw_entry(&master, [1, 0, 1, 0], &[]);
    let reader = MockLedgerReader::new().with_entry(entry);
    let config = fetch_account_signer_config(&reader, &master).await.unwrap();
    assert_eq!(config.account_strkey, master);
    assert_eq!(config.thresholds.master_weight, 1);
    assert_eq!(config.thresholds.medium, 1);
    assert!(config.signers.is_empty());
}

#[tokio::test]
async fn adapter_fetches_multisig_account() {
    let master = master_strkey();
    let co_a = cosigner_a_strkey();
    let entry = raw_entry(&master, [1, 1, 2, 3], &[(&co_a, 2)]);
    let reader = MockLedgerReader::new().with_entry(entry);
    let config = fetch_account_signer_config(&reader, &master).await.unwrap();
    assert_eq!(config.thresholds.medium, 2);
    assert_eq!(config.signers.len(), 1);
    assert_eq!(config.signers[0].public_key_strkey, co_a);
    assert_eq!(config.signers[0].weight, 2);
}

#[tokio::test]
async fn adapter_returns_account_not_found_for_missing_entry() {
    let master = master_strkey();
    let reader = MockLedgerReader::new();
    let err = fetch_account_signer_config(&reader, &master)
        .await
        .unwrap_err();
    assert!(
        matches!(err, MultisigError::AccountNotFound(_)),
        "expected AccountNotFound, got {err:?}"
    );
}

// ── 8. End-to-end: adapter + domain via wiremock ──────────────────────────────

#[tokio::test]
async fn wiremock_single_signer_bumper_satisfies_medium_threshold() {
    let master = master_strkey();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ledger_entries_response(
                &master,
                [1, 0, 1, 0],
                &[],
            )),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = fetch_account_signer_config(&wiremock_client(&server), &master)
        .await
        .unwrap();
    let result = verify_bumper_authority(&config, &master).unwrap();
    assert!(result.is_sufficient());
    server.verify().await;
}

#[tokio::test]
async fn wiremock_multisig_bumper_weight_satisfies_medium_threshold() {
    let master = master_strkey();
    let co_a = cosigner_a_strkey();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ledger_entries_response(
                &master,
                [1, 1, 2, 3],
                &[(&co_a, 2)],
            )),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = fetch_account_signer_config(&wiremock_client(&server), &master)
        .await
        .unwrap();
    let result = verify_bumper_authority(&config, &co_a).unwrap();
    assert!(result.is_sufficient());
    server.verify().await;
}

#[tokio::test]
async fn wiremock_multisig_bumper_weight_insufficient_reports_cosigners() {
    let master = master_strkey();
    let co_a = cosigner_a_strkey();
    let co_b = cosigner_b_strkey();
    let server = MockServer::start().await;
    // master_weight=2, medium=3; co_a has weight=1 (insufficient alone).
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ledger_entries_response(
                &master,
                [2, 1, 3, 5],
                &[(&co_a, 1), (&co_b, 2)],
            )),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = fetch_account_signer_config(&wiremock_client(&server), &master)
        .await
        .unwrap();
    let err = verify_bumper_authority(&config, &co_a).unwrap_err();
    match err {
        MultisigError::InsufficientWeight {
            bumper_weight,
            medium_threshold,
            ref additional_signers,
            ..
        } => {
            assert_eq!(bumper_weight, 1);
            assert_eq!(medium_threshold, 3);
            assert!(
                !additional_signers.is_empty(),
                "expected at least one additional signer"
            );
            // co_b (weight=2) or master (weight=2) must be suggested.
            assert!(
                additional_signers
                    .iter()
                    .any(|k| k == &co_b || k == &master),
                "expected co_b or master in additional signers, got {additional_signers:?}"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
    server.verify().await;
}
