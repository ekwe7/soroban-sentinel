//! Webhook authentication and HMAC-SHA256 signature verification for alert payloads.
//! Signs outgoing webhook notifications with HMAC-SHA256 signatures and attaches
//! a Unix timestamp header. Supports bearer token authorization.
//! Zeroizes sensitive secrets on drop.

use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroize;

// ============================================================
// Webhook authentication configuration
// ============================================================

/// Configuration for webhook authentication.
#[derive(Debug, Clone)]
pub struct WebhookAuthConfig {
    /// HMAC-SHA256 secret key for signing requests.
    secret: Vec<u8>,
    /// Bearer token for authorization header.
    token: Option<String>,
}

/// Zeroized version of the secret, dropped on drop.
#[derive(Debug)]
struct ZeroizedSecret(Vec<u8>);

impl Zeroize for ZeroizedSecret {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl WebhookAuthConfig {
    /// Creates a new `WebhookAuthConfig` from the configured secret and token.
    ///
    /// The secret is zeroized on drop for security.
    pub fn new(secret: Vec<u8>, token: Option<String>) -> Self {
        Self { secret, token }
    }

    /// Returns the HMAC-SHA256 secret key.
    ///
    /// # Security
    ///
    /// The caller must not expose this secret in logs or error messages.
    pub fn secret(&self) -> &[u8] {
        &self.secret
    }

    /// Returns the bearer token if configured.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

/// Drops the sensitive secret when the config is dropped.
impl Drop for WebhookAuthConfig {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

/// Authenticated webhook config that wraps the auth config with zeroized secrets.
#[derive(Debug)]
pub struct AuthenticatedWebhookConfig {
    /// The webhook URL to post alerts to.
    url: String,
    /// Underlying authentication configuration (secrets zeroized on drop).
    auth: ZeroizedSecret,
    /// Bearer token for authorization.
    token: Option<String>,
}

impl AuthenticatedWebhookConfig {
    /// Creates a new authenticated webhook config.
    ///
    /// The secret is zeroized on drop.
    pub fn new(
        url: String,
        secret: Vec<u8>,
        token: Option<String>,
    ) -> Result<Self, WebhookAuthError> {
        if url.is_empty() {
            return Err(WebhookAuthError::MissingUrl);
        }
        Ok(Self {
            url,
            auth: ZeroizedSecret(secret),
            token,
        })
    }

    /// Returns the webhook URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the bearer token if configured.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

/// Drops the sensitive secret when the config is dropped.
impl Drop for AuthenticatedWebhookConfig {
    fn drop(&mut self) {
        // Zeroize the underlying secret
        self.auth.zeroize();
    }
}

/// Errors that can occur during webhook authentication.
#[derive(Debug, thiserror::Error)]
pub enum WebhookAuthError {
    #[error("Webhook URL is required")]
    MissingUrl,
    #[error("Webhook secret is required")]
    MissingSecret,
    #[error("Failed to compute HMAC-SHA256 signature: {0}")]
    HmacError(String),
    #[error("Failed to format timestamp")]
    TimestampError(String),
}

/// Timestamp in Unix epoch seconds.
type Timestamp = u64;

/// Base64 encoding table for HTTP header transmission.
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Compute the HMAC-SHA256 signature for the given body bytes.
///
/// The signature is calculated over the exact serialized JSON request body
/// that will be transmitted.
pub fn compute_hmac_sha256(secret: &[u8], body: &[u8]) -> Result<Vec<u8>, WebhookAuthError> {
    if secret.is_empty() {
        return Err(WebhookAuthError::MissingSecret);
    }
    const BLOCK_SIZE: usize = 64;
    let mut key = [0u8; BLOCK_SIZE];
    if secret.len() > BLOCK_SIZE {
        let hash = Sha256::digest(secret);
        key[..32].copy_from_slice(&hash);
    } else {
        key[..secret.len()].copy_from_slice(secret);
    }
    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key[i];
        opad[i] ^= key[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(body);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    Ok(outer.finalize().to_vec())
}

/// Format the current time as a Unix epoch timestamp.
pub fn format_timestamp() -> Result<Timestamp, WebhookAuthError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| WebhookAuthError::TimestampError(format!("{e}")))?;
    Ok(now.as_secs())
}

/// Sign a webhook request body with HMAC-SHA256 and attach headers.
///
/// # Signing process:
/// 1. Calculate HMAC-SHA256 signature over the exact JSON body
/// 2. Attach signature via `X-Sentinel-Signature` header
/// 3. Attach Unix timestamp via `X-Sentinel-Timestamp` header
/// 4. Attach bearer token via `Authorization: Bearer <TOKEN>` if configured
///
/// # Payload integrity
///
/// The signature is calculated over the exact bytes being transmitted,
/// ensuring the receiver can verify the request authenticity.
pub struct WebhookSigner {
    /// The HMAC-SHA256 secret (raw bytes for key creation).
    secret: Vec<u8>,
    /// The webhook URL.
    url: String,
    /// Bearer token for authorization.
    token: Option<String>,
}

impl WebhookSigner {
    /// Create a new `WebhookSigner` with the given secret and optional token.
    ///
    /// # Security
    ///
    /// The secret is zeroized when the signer is dropped.
    pub fn new(secret: Vec<u8>, token: Option<String>) -> Result<Self, WebhookAuthError> {
        if secret.is_empty() {
            return Err(WebhookAuthError::MissingSecret);
        }

        Ok(Self {
            secret,
            url: String::new(), // Will be set per request
            token,
        })
    }

    /// Set the webhook URL for this signer.
    pub fn set_url(&mut self, url: &str) {
        self.url = url.to_string();
    }

    /// Sign the given body bytes and return the headers map.
    ///
    /// The signature is calculated over the exact body bytes.
    /// Returns (signature_b64, timestamp, headers).
    pub fn sign_body(&self, body: &[u8]) -> (String, Timestamp, Vec<(String, String)>) {
        // Compute HMAC-SHA256 signature
        let signature_bytes = match compute_hmac_sha256(&self.secret, body) {
            Ok(s) => s,
            Err(_) => {
                // Return empty signatures on error
                return (String::new(), 0, Vec::new());
            }
        };

        // Base64-encode the signature for HTTP header transmission
        let signature_b64 = base64_encode(&signature_bytes);

        // Format Unix timestamp
        let timestamp = match format_timestamp() {
            Ok(t) => t,
            Err(_) => {
                return (signature_b64, 0, Vec::new());
            }
        };

        // Build headers
        let signature_b64_clone = signature_b64.clone();
        let mut headers: Vec<(String, String)> = vec![
            ("X-Sentinel-Signature".to_string(), signature_b64_clone),
            ("X-Sentinel-Timestamp".to_string(), timestamp.to_string()),
        ];

        // Add bearer token if configured
        if let Some(token) = &self.token {
            headers.push(("Authorization".to_string(), format!("Bearer {}", token)));
        }

        (signature_b64, timestamp, headers)
    }
}

/// Drops the sensitive secret when the signer is dropped.
impl Drop for WebhookSigner {
    fn drop(&mut self) {
        // Zeroize the secret
        self.secret.zeroize();
    }
}

/// Sends an authenticated webhook notification.
///
/// # Workflow
/// 1. Serialize the alert payload to JSON
/// 2. Sign the JSON body with HMAC-SHA256
/// 3. Attach timestamp and bearer token headers
/// 4. POST to the webhook URL
///
/// # Errors
///
/// Returns an error if the webhook request fails.
pub async fn send_webhook_alert(
    _config: &AuthenticatedWebhookConfig,
    payload: &serde_json::Value,
    signer: &mut WebhookSigner,
) -> Result<(), reqwest::Error> {
    // Serialize the payload to JSON (the exact body that will be transmitted)
    let body = payload.to_string();

    // Sign the body
    let (_signature, timestamp, headers) = signer.sign_body(body.as_bytes());

    // Build the HTTP request
    let mut request = reqwest::Client::new()
        .post(signer.url.clone())
        .json(&payload);

    // Add headers
    for (name, value) in &headers {
        request = request.header(name, value);
    }

    // Send the request
    let response = request.send().await;

    match response {
        Ok(_) => {
            eprintln!(
                "Sent webhook alert to {} with signature (timestamp: {})",
                signer.url, timestamp
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("Failed to send webhook alert to {}: {e:?}", signer.url);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_auth_config_creation() {
        let config = WebhookAuthConfig::new(vec![1, 2, 3, 4], Some("token123".to_string()));
        assert_eq!(config.token(), Some("token123"));
    }

    #[test]
    fn test_webhook_auth_config_no_token() {
        let config = WebhookAuthConfig::new(vec![1, 2, 3, 4], None);
        assert_eq!(config.token(), None);
    }

    #[test]
    fn test_webhook_auth_error_missing_url() {
        let result = AuthenticatedWebhookConfig::new(String::new(), vec![1, 2, 3, 4], None);
        assert!(matches!(result, Err(WebhookAuthError::MissingUrl)));
    }

    #[test]
    fn test_webhook_signer_creation() {
        let signer = WebhookSigner::new(vec![0; 32], Some("test-token".to_string()));
        assert!(signer.is_ok());
    }

    #[test]
    fn test_webhook_signer_missing_secret() {
        let signer = WebhookSigner::new(vec![], None);
        assert!(signer.is_err());
    }

    #[test]
    fn test_compute_hmac_sha256() {
        let secret = b"secret-key-for-hmac";
        let body = b"{\"test\": \"payload\"}";

        let result = compute_hmac_sha256(secret, body);
        assert!(result.is_ok());
        let sig = result.unwrap();
        assert_eq!(sig.len(), 32); // HMAC-SHA256 produces 32 bytes
    }

    #[test]
    fn test_format_timestamp() {
        let result = format_timestamp();
        assert!(result.is_ok());
        let ts = result.unwrap();
        // Should be a reasonable Unix timestamp (around 1750000000 for 2025)
        assert!(ts > 1_700_000_000);
        assert!(ts < 1_800_000_000);
    }

    #[test]
    fn test_sign_body_creates_headers() {
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

        let (_signature, timestamp, headers) =
            signer.sign_body(serde_json::to_string(&payload).unwrap().as_bytes());

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

    #[test]
    fn test_sign_body_without_token() {
        let mut signer = WebhookSigner::new(b"secret-key-for-hmac".to_vec(), None).unwrap();
        signer.set_url("https://example.com/webhook");

        let payload = serde_json::json!({"test": "payload"});

        let (_signature, _timestamp, headers) =
            signer.sign_body(serde_json::to_string(&payload).unwrap().as_bytes());

        // Verify no Authorization header when no token is configured
        let has_auth = headers.iter().any(|(name, _)| name == "Authorization");
        assert!(
            !has_auth,
            "Expected no Authorization header when no token configured"
        );
    }
}
