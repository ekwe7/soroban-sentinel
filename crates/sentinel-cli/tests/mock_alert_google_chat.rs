use sentinel_cli::alerts::google_chat::GoogleChatAlertDispatcher;
use sentinel_core::{Durability, EntryType, RiskLevel, StorageEntryReport, TTLReport};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

const CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

#[test]
fn watch_accepts_google_chat_webhook_flag() {
    use clap::Parser;
    use sentinel_cli::{Cli, Commands};

    let cli = Cli::try_parse_from([
        "sentinel",
        "watch",
        CONTRACT,
        "--alert-google-chat",
        "https://chat.googleapis.com/v1/spaces/fixture/messages?key=fixture&token=fixture",
    ])
    .unwrap();

    assert!(matches!(&cli.command, Commands::Watch(args)
        if args.alert_google_chat.as_deref()
            == Some("https://chat.googleapis.com/v1/spaces/fixture/messages?key=fixture&token=fixture")));
}

fn report(status: RiskLevel) -> TTLReport {
    TTLReport {
        contract_id: CONTRACT.into(),
        network: "testnet".into(),
        latest_ledger: 1_000,
        ledger_velocity_seconds: 5.0,
        burn_rate: Default::default(),
        entries: vec![StorageEntryReport::new(
            "instance",
            EntryType::ContractInstance,
            Durability::Instance,
            240,
            10_000,
            1_000,
            5_000.0,
            0.06,
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
async fn sends_card_v2_payload_and_deduplicates_unchanged_status() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let mut dispatcher = GoogleChatAlertDispatcher::new(&server.uri()).unwrap();
    let warning = report(RiskLevel::Warn);
    dispatcher.observe(&warning).await.unwrap();
    dispatcher.observe(&warning).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let payload: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let card = &payload["cardsV2"][0];
    assert_eq!(card["cardId"], "soroban-sentinel-alert");
    assert_eq!(card["card"]["header"]["title"], "Soroban Sentinel: WARN");
    let widgets = card["card"]["sections"][0]["widgets"].as_array().unwrap();
    assert!(widgets
        .iter()
        .any(|widget| widget["decoratedText"]["text"] == CONTRACT));
    assert!(widgets
        .iter()
        .any(|widget| widget["decoratedText"]["text"] == "🟠 WARN"));
    assert!(widgets
        .iter()
        .any(|widget| widget["decoratedText"]["text"] == "0.1"));
    assert!(widgets.iter().any(|widget| {
        widget["buttonList"]["buttons"][0]["onClick"]["openLink"]["url"]
            == format!("https://stellar.expert/explorer/testnet/contract/{CONTRACT}")
    }));
}

#[tokio::test]
async fn returns_error_for_unsuccessful_webhook_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let mut dispatcher = GoogleChatAlertDispatcher::new(&server.uri()).unwrap();
    let error = dispatcher
        .observe(&report(RiskLevel::Critical))
        .await
        .unwrap_err();
    assert!(error.contains("HTTP 503"));
}
