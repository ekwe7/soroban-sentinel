use sentinel_core::{
    AlertKind, AlertMinLevel, AlertNotifier, Durability, EntryType, RiskLevel, StorageEntryReport,
    TTLReport, WebhookAlertNotifier,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn report(status: RiskLevel) -> TTLReport {
    TTLReport {
        contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".into(),
        network: "testnet".into(),
        latest_ledger: 100,
        ledger_velocity_seconds: 5.0,
        entries: vec![StorageEntryReport::new(
            "persistent:ADMIN",
            EntryType::PersistentData,
            Durability::Persistent,
            10,
            120,
            20,
            100.0,
            0.001,
            status,
        )],
        overall_status: status,
        archival_recommendations: vec![],
        sac_asset_info: None,
    }
}

#[tokio::test]
async fn slack_payload_contains_contract_and_entry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let sent = WebhookAlertNotifier::new(server.uri())
        .unwrap()
        .notify(
            &report(RiskLevel::Warn),
            AlertKind::Slack,
            AlertMinLevel::Warn,
        )
        .await
        .unwrap();
    assert!(sent);
    let request = server.received_requests().await.unwrap().pop().unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert!(body["blocks"][1]["text"]["text"]
        .as_str()
        .unwrap()
        .contains("persistent:ADMIN"));
}

#[tokio::test]
async fn discord_payload_contains_contract_and_entry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    WebhookAlertNotifier::new(server.uri())
        .unwrap()
        .notify(
            &report(RiskLevel::Critical),
            AlertKind::Discord,
            AlertMinLevel::Critical,
        )
        .await
        .unwrap();
    let request = server.received_requests().await.unwrap().pop().unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert!(body["embeds"][0]["description"]
        .as_str()
        .unwrap()
        .contains("CAAAAAAAAA"));
}

#[tokio::test]
async fn below_minimum_level_does_not_send() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let sent = WebhookAlertNotifier::new(server.uri())
        .unwrap()
        .notify(
            &report(RiskLevel::Warn),
            AlertKind::Slack,
            AlertMinLevel::Critical,
        )
        .await
        .unwrap();
    assert!(!sent);
}
