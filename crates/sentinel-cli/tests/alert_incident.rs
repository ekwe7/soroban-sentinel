use sentinel_cli::alerts::incident::{dedup_key, IncidentDispatcher, Provider};
use sentinel_core::{Durability, EntryType, RiskLevel, StorageEntryReport, TTLReport};
use serde_json::json;
use wiremock::{
    matchers::{body_json, header, method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

const CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";
const KEY: &str = "sample-ledger-key-xdr";
const SUMMARY: &str =
    "Soroban TTL risk on CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM (testnet)";

fn report(status: RiskLevel) -> TTLReport {
    TTLReport {
        contract_id: CONTRACT.into(),
        network: "testnet".into(),
        latest_ledger: 10,
        ledger_velocity_seconds: 5.0,
        entries: vec![StorageEntryReport::new(
            "instance",
            EntryType::ContractInstance,
            Durability::Instance,
            0,
            100,
            90,
            450.0,
            0.0,
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
async fn pagerduty_triggers_escalates_deduplicates_and_resolves() {
    let server = MockServer::start().await;
    let key = dedup_key("testnet", CONTRACT, KEY);
    assert_eq!(key, dedup_key("TESTNET", CONTRACT, KEY));
    assert_ne!(key, dedup_key("mainnet", CONTRACT, KEY));
    for severity in ["warning", "critical"] {
        Mock::given(method("POST"))
            .and(path("/v2/enqueue"))
            .and(body_json(
                json!({"routing_key":"test-routing","event_action":"trigger","dedup_key":key,
                "payload":{"summary":SUMMARY,"source":"soroban-sentinel","severity":severity}}),
            ))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/v2/enqueue"))
        .and(body_json(
            json!({"routing_key":"test-routing","event_action":"resolve","dedup_key":key}),
        ))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;

    let mut dispatcher = IncidentDispatcher::new(Provider::PagerDuty {
        endpoint: format!("{}/v2/enqueue", server.uri()).parse().unwrap(),
        routing_key: "test-routing".into(),
    })
    .unwrap();
    for status in [
        RiskLevel::Ok,
        RiskLevel::Warn,
        RiskLevel::Warn,
        RiskLevel::Critical,
        RiskLevel::Archived,
        RiskLevel::Ok,
        RiskLevel::Ok,
    ] {
        dispatcher
            .observe(&report(status), &[KEY.into()])
            .await
            .unwrap();
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn opsgenie_retries_creates_and_closes_by_alias() {
    let server = MockServer::start().await;
    let key = dedup_key("testnet", CONTRACT, KEY);
    let trigger = || {
        Mock::given(method("POST"))
            .and(path("/v2/alerts"))
            .and(header("authorization", "GenieKey test-api-key"))
            .and(body_json(
                json!({"alias":key,"message":SUMMARY,"source":"soroban-sentinel","priority":"P2"}),
            ))
    };
    trigger()
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    trigger()
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/v2/alerts/{key}/close")))
        .and(query_param("identifierType", "alias"))
        .and(header("authorization", "GenieKey test-api-key"))
        .and(body_json(json!({"source":"soroban-sentinel"})))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;

    let mut dispatcher = IncidentDispatcher::new(Provider::Opsgenie {
        endpoint: format!("{}/v2/alerts", server.uri()).parse().unwrap(),
        api_key: "test-api-key".into(),
    })
    .unwrap();
    dispatcher
        .observe(&report(RiskLevel::Archived), &[KEY.into()])
        .await
        .unwrap();
    dispatcher
        .observe(&report(RiskLevel::Ok), &[KEY.into()])
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[test]
fn watch_exposes_both_provider_flags_without_debugging_credentials() {
    use clap::Parser;
    use sentinel_cli::{Cli, Commands};
    let cli = Cli::try_parse_from([
        "sentinel",
        "watch",
        CONTRACT,
        "--pagerduty-routing-key",
        "fixture-routing",
        "--opsgenie-api-key",
        "fixture-api",
    ])
    .unwrap();
    assert!(matches!(&cli.command, Commands::Watch(args)
        if args.pagerduty_routing_key.as_deref() == Some("fixture-routing")
        && args.opsgenie_api_key.as_deref() == Some("fixture-api")));
    let debug = format!("{cli:?}");
    assert!(!debug.contains("fixture-routing") && !debug.contains("fixture-api"));
}
