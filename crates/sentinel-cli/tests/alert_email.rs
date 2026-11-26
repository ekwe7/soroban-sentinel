//! Email alert notification tests for Soroban Sentinel.

#[tokio::test]
async fn test_email_payload_creation() {
    let payload = sentinel_cli::EmailPayload {
        contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".to_string(),
        storage_keys: vec!["COUNTER".to_string(), "ADMIN".to_string()],
        remaining_ledgers: 50000,
        remaining_ttl: "~694 days".to_string(),
        risk: sentinel_cli::RiskDisplay::Warn,
        recommended_actions: vec!["Extend storage TTL".to_string()],
        detected_at: "2026-01-15T10:30:00Z".to_string(),
    };

    assert_eq!(
        payload.contract_id,
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM"
    );
    assert_eq!(payload.storage_keys.len(), 2);
    assert_eq!(payload.remaining_ledgers, 50000);
    assert_eq!(payload.risk.risk_text(), "WARN");
}
