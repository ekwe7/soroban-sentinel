use clap::Parser;
use sentinel_cli::{alerts::matrix::MatrixAlertDispatcher, Cli, Commands};
use sentinel_core::{Durability, EntryType, RiskLevel, StorageEntryReport, TTLReport};
use wiremock::{
    matchers::{header, method},
    Mock, MockServer, ResponseTemplate,
};

const CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

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

#[test]
fn watch_accepts_matrix_configuration_flags() {
    let cli = Cli::try_parse_from([
        "sentinel",
        "watch",
        CONTRACT,
        "--alert-matrix-homeserver",
        "https://matrix.example.org",
        "--alert-matrix-room",
        "!ops:example.org",
        "--alert-matrix-token",
        "fixture-token",
    ])
    .unwrap();

    assert!(matches!(&cli.command, Commands::Watch(args)
        if args.alert_matrix_homeserver.as_deref() == Some("https://matrix.example.org")
            && args.alert_matrix_room.as_deref() == Some("!ops:example.org")
            && args.alert_matrix_token.as_deref() == Some("fixture-token")));
}

#[tokio::test]
async fn sends_formatted_notice_and_unique_transaction_ids_on_transitions() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(header("authorization", "Bearer fixture-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "event_id": "$event"
        })))
        .expect(2)
        .mount(&server)
        .await;

    let mut dispatcher =
        MatrixAlertDispatcher::new(&server.uri(), "!ops:example.org", "fixture-token").unwrap();
    dispatcher.observe(&report(RiskLevel::Warn)).await.unwrap();
    dispatcher.observe(&report(RiskLevel::Warn)).await.unwrap();
    dispatcher
        .observe(&report(RiskLevel::Critical))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let first_transaction = requests[0].url.as_str().rsplit('/').next().unwrap();
    let second_transaction = requests[1].url.as_str().rsplit('/').next().unwrap();
    assert_ne!(first_transaction, second_transaction);

    let payload: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(payload["msgtype"], "m.notice");
    assert_eq!(payload["format"], "org.matrix.custom.html");
    assert!(payload["formatted_body"]
        .as_str()
        .unwrap()
        .contains("stellar.expert/explorer/testnet/contract"));
    assert!(payload["body"]
        .as_str()
        .unwrap()
        .contains("live until ledger 10000"));
}

#[tokio::test]
async fn returns_error_for_unsuccessful_homeserver_response() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;

    let mut dispatcher =
        MatrixAlertDispatcher::new(&server.uri(), "!ops:example.org", "fixture-token").unwrap();
    let error = dispatcher
        .observe(&report(RiskLevel::Critical))
        .await
        .unwrap_err();
    assert!(error.contains("HTTP 502"));
}
