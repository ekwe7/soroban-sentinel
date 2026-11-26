//! Webhook authentication tests for Soroban Sentinel.

use clap::Parser;
use sentinel_cli::alerts::{compute_hmac_sha256, format_timestamp, WebhookSigner};

#[tokio::test]
async fn test_webhook_signer_creation() {
    let signer = WebhookSigner::new(
        b"secret-key-for-hmac".to_vec(),
        Some("test-token".to_string()),
    );
    assert!(signer.is_ok());
}

#[tokio::test]
async fn test_webhook_signer_missing_secret() {
    let signer = WebhookSigner::new(vec![], None);
    assert!(signer.is_err());
}

#[tokio::test]
async fn test_compute_hmac_sha256() {
    let secret = b"secret-key-for-hmac";
    let body = r#"{"test": "payload"}"#;

    let result = compute_hmac_sha256(secret, body.as_bytes());
    assert!(result.is_ok());
    let sig = result.unwrap();
    assert_eq!(sig.len(), 32); // HMAC-SHA256 produces 32 bytes
}

#[tokio::test]
async fn test_format_timestamp() {
    let result = format_timestamp();
    assert!(result.is_ok());
    let ts = result.unwrap();
    // Should be a reasonable Unix timestamp (around 1750000000 for 2025)
    assert!(ts > 1_700_000_000);
    assert!(ts < 1_800_000_000);
}

#[tokio::test]
async fn test_sign_body_creates_headers() {
    let mut signer = WebhookSigner::new(
        b"secret-key-for-hmac".to_vec(),
        Some("test-token".to_string()),
    )
    .unwrap();
    signer.set_url("https://example.com/webhook");

    let payload = serde_json::json!({
        "contract_id": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        "risk": "WARN"
    });

    let (signature, timestamp, headers) =
        signer.sign_body(serde_json::to_string(&payload).unwrap().as_bytes());

    // Verify signature is base64-encoded and 32 bytes -> ~44 chars
    assert!(signature.len() > 20); // Base64-encoded HMAC should be at least 44 chars

    // Verify timestamp is a reasonable number
    assert!(timestamp > 1_700_000_000);

    // Verify Authorization header is present
    let has_auth = headers.iter().any(|(name, _)| name == "Authorization");
    assert!(has_auth, "Expected Authorization header");

    // Verify X-Sentinel-Signature header is present
    let has_sig = headers
        .iter()
        .any(|(name, _)| name == "X-Sentinel-Signature");
    assert!(has_sig, "Expected X-Sentinel-Signature header");

    // Verify X-Sentinel-Timestamp header is present
    let has_ts = headers
        .iter()
        .any(|(name, _)| name == "X-Sentinel-Timestamp");
    assert!(has_ts, "Expected X-Sentinel-Timestamp header");
}

#[tokio::test]
async fn test_sign_body_without_token() {
    let mut signer = WebhookSigner::new(b"secret-key-for-hmac".to_vec(), None).unwrap();
    signer.set_url("https://example.com/webhook");

    let payload = serde_json::json!({"test": "payload"});

    let (signature, timestamp, headers) =
        signer.sign_body(serde_json::to_string(&payload).unwrap().as_bytes());

    // Verify no Authorization header when no token is configured
    let has_auth = headers.iter().any(|(name, _)| name == "Authorization");
    assert!(
        !has_auth,
        "Expected no Authorization header when no token configured"
    );
}
