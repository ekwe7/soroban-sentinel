//! WireMock coverage for the Microsoft Teams Adaptive Card dispatcher.

use sentinel_cli::adapters::alert_teams::TeamsAlertDispatcher;
use sentinel_core::{Durability, EntryType, RiskLevel, StorageEntryReport, TTLReport};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn report(status: RiskLevel) -> TTLReport {
    TTLReport {
        contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".into(),
        network: "testnet".into(),
        latest_ledger: 1_000,
        ledger_velocity_seconds: 5.0,
        burn_rate: Default::default(),
        entries: vec![StorageEntryReport::new(
            "instance",
            EntryType::ContractInstance,
            Durability::Instance,
            240,
            1_500,
            500,
            2_500.0,
            0.02,
            status,
        )],
        overall_status: status,
        archival_recommendations: Vec::new(),
        sac_asset_info: None,
        contract_code_hash: None,
        contract_bytecode_size: None,
        contract_upgrade: None,
    }
}

#[tokio::test]
async fn posts_message_with_adaptive_card_v15_and_risk_details() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/teams"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let mut dispatcher =
        TeamsAlertDispatcher::new_with_webhook_url(&format!("{}/teams", server.uri())).unwrap();
    dispatcher
        .observe(&report(RiskLevel::Critical))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["type"], "message");
    let card = &body["attachments"][0]["content"];
    assert_eq!(
        body["attachments"][0]["contentType"],
        "application/vnd.microsoft.card.adaptive"
    );
    assert_eq!(card["type"], "AdaptiveCard");
    assert_eq!(card["version"], "1.5");
    assert_eq!(card["body"][0]["style"], "Attention");
    let facts = card["body"][1]["facts"].as_array().unwrap();
    assert!(facts.iter().any(|fact| fact["title"] == "Contract ID"));
    assert!(facts
        .iter()
        .any(|fact| fact["title"] == "Live until ledger" && fact["value"] == "1500"));
    assert_eq!(card["actions"][0]["url"], "https://stellar.expert/explorer/testnet/contract/CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM");
}

#[tokio::test]
async fn reports_http_failures_without_marking_alert_delivered() {
    let server = MockServer::start().await;
    let failure = Mock::given(method("POST"))
        .and(path("/teams"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount_as_scoped(&server)
        .await;
    let mut dispatcher =
        TeamsAlertDispatcher::new_with_webhook_url(&format!("{}/teams", server.uri())).unwrap();
    assert!(dispatcher
        .observe(&report(RiskLevel::Warn))
        .await
        .unwrap_err()
        .contains("HTTP 503"));
    drop(failure);

    Mock::given(method("POST"))
        .and(path("/teams"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    dispatcher.observe(&report(RiskLevel::Warn)).await.unwrap();
}

#[tokio::test]
async fn deduplicates_successfully_delivered_risk() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/teams"))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;
    let mut dispatcher =
        TeamsAlertDispatcher::new_with_webhook_url(&format!("{}/teams", server.uri())).unwrap();
    dispatcher
        .observe(&report(RiskLevel::Archived))
        .await
        .unwrap();
    dispatcher
        .observe(&report(RiskLevel::Archived))
        .await
        .unwrap();
}
