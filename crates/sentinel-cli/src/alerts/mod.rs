//! Webhook authentication and HMAC-SHA256 signature verification for alert payloads.
//!
//! Signs outgoing webhook notifications with HMAC-SHA256 signatures and attaches
//! a Unix timestamp header. Supports bearer token authorization.

mod email;
mod webhook_auth;

pub use email::*;
pub use webhook_auth::*;