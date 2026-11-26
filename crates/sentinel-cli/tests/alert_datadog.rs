//! Integration tests for the Datadog metrics and events adapter.
//!
//! All requests are intercepted by a local WireMock server – no real Datadog
//! traffic is generated.

use sentinel_cli::alerts::datadog::{DatadogAdapter, DatadogSite};
use sentinel_core::{Durability, EntryType, RiskLevel, StorageEntryReport, TTLReport};
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

// ── helpers ───────────────────────────────────────────────────────────────────

const CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";
const NETWORK: &str = "testnet";
const API_KEY: &str = "test-dd-api-key";

fn entry(
    id: &str,
    durability: Durability,
    remaining: i64,
    status: RiskLevel,
) -> StorageEntryReport {
    StorageEntryReport::new(
        id,
        EntryType::ContractInstance,
        durability,
        240,
        10_000,
        remaining,
        remaining as f64 * 5.0,
        remaining as f64 * 5.0 / 86_400.0,
        status,
    )
}

fn report_with_entries(entries: Vec<StorageEntryReport>) -> TTLReport {
    let overall = entries
        .iter()
        .map(|e| e.status)
        .max()
        .unwrap_or(RiskLevel::Ok);
    TTLReport {
        contract_id: CONTRACT.into(),
        network: NETWORK.into(),
        latest_ledger: 1_000,
        ledger_velocity_seconds: 5.0,
        burn_rate: Default::default(),
        entries,
        overall_status: overall,
        archival_recommendations: Vec::new(),
        sac_asset_info: None,
        contract_code_hash: None,
        contract_bytecode_size: None,
        contract_upgrade: None,
    }
}

fn ok_report() -> TTLReport {
    report_with_entries(vec![entry(
        "instance",
        Durability::Instance,
        500_000,
        RiskLevel::Ok,
    )])
}

fn warn_report() -> TTLReport {
    report_with_entries(vec![entry(
        "instance",
        Durability::Instance,
        100_000,
        RiskLevel::Warn,
    )])
}

fn critical_report() -> TTLReport {
    report_with_entries(vec![entry(
        "instance",
        Durability::Instance,
        10_000,
        RiskLevel::Critical,
    )])
}

fn archived_report() -> TTLReport {
    report_with_entries(vec![entry(
        "instance",
        Durability::Instance,
        0,
        RiskLevel::Archived,
    )])
}

// ── 1. Correct gauge metric JSON schema ──────────────────────────────────────

#[tokio::test]
async fn metrics_payload_matches_datadog_series_schema() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})))
        .expect(1)
        .mount(&server)
        .await;

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    adapter.observe(&ok_report()).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);

    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("body is JSON");
    let series = body["series"].as_array().expect("series is array");
    assert_eq!(series.len(), 1);

    let point = &series[0];
    assert!(point["metric"].is_string());
    assert!(point["type"].is_string());
    assert!(point["points"].is_array());
    assert!(point["tags"].is_array());
}

// ── 2. Correct metric name and values ────────────────────────────────────────

#[tokio::test]
async fn metrics_name_and_remaining_ledgers_value() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})))
        .expect(1)
        .mount(&server)
        .await;

    let report = report_with_entries(vec![entry(
        "instance",
        Durability::Instance,
        123_456,
        RiskLevel::Ok,
    )]);

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    adapter.observe(&report).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let series = &body["series"][0];

    assert_eq!(
        series["metric"].as_str().unwrap(),
        "stellar.contract.ttl_remaining"
    );
    assert_eq!(series["type"].as_str().unwrap(), "gauge");

    // points is [[timestamp, value]]
    let point_arr = series["points"].as_array().unwrap();
    assert_eq!(point_arr.len(), 1);
    let point = point_arr[0].as_array().unwrap();
    assert_eq!(point.len(), 2);
    // Timestamp (index 0) is a UNIX epoch integer.
    assert!(point[0].as_u64().unwrap() > 0);
    // Value is the remaining_ledgers.
    assert_eq!(point[1].as_i64().unwrap(), 123_456);
}

// ── 3. Correct contract_id, network and durability tags ──────────────────────

#[tokio::test]
async fn metrics_tags_contain_contract_network_durability() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})))
        .expect(1)
        .mount(&server)
        .await;

    let report = report_with_entries(vec![entry(
        "persistent:ADMIN",
        Durability::Persistent,
        200_000,
        RiskLevel::Ok,
    )]);

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    adapter.observe(&report).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let tags: Vec<String> = serde_json::from_value(body["series"][0]["tags"].clone()).unwrap();

    assert!(tags.contains(&format!("contract_id:{CONTRACT}")));
    assert!(tags.contains(&format!("network:{NETWORK}")));
    assert!(tags.contains(&"durability:persistent".to_string()));
}

// ── 4. Multiple metric points submitted in one request ───────────────────────

#[tokio::test]
async fn multiple_entries_batched_into_single_metrics_request() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})))
        .expect(1)
        .mount(&server)
        .await;

    let report = report_with_entries(vec![
        entry("instance", Durability::Instance, 500_000, RiskLevel::Ok),
        entry("code_wasm", Durability::Code, 400_000, RiskLevel::Ok),
        entry(
            "persistent:COUNTER",
            Durability::Persistent,
            300_000,
            RiskLevel::Ok,
        ),
    ]);

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    adapter.observe(&report).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    // Exactly one HTTP request despite three entries.
    assert_eq!(requests.len(), 1);

    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let series = body["series"].as_array().unwrap();
    assert_eq!(series.len(), 3, "all three entries in one series payload");
}

// ── 5. Correct event payload for Warn / Critical / Archived transitions ──────

#[tokio::test]
async fn event_submitted_on_warn_transition_with_correct_payload() {
    let server = MockServer::start().await;

    // Metrics endpoint for the scan
    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})))
        .mount(&server)
        .await;

    // Events endpoint – exactly one event expected
    Mock::given(method("POST"))
        .and(path("/api/v1/events"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(
            ResponseTemplate::new(202)
                .set_body_json(serde_json::json!({"status":"ok","event":{"id":1}})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    adapter.observe(&warn_report()).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let event_req = requests
        .iter()
        .find(|r| r.url.path() == "/api/v1/events")
        .expect("events request sent");

    let body: serde_json::Value = serde_json::from_slice(&event_req.body).unwrap();
    assert_eq!(body["alert_type"].as_str().unwrap(), "warning");
    assert!(body["title"].as_str().unwrap().contains("WARN"));
    assert!(body["text"].as_str().unwrap().contains(CONTRACT));
    assert!(body["text"].as_str().unwrap().contains(NETWORK));
    assert_eq!(
        body["source_type_name"].as_str().unwrap(),
        "soroban-sentinel"
    );
    let tags: Vec<String> = serde_json::from_value(body["tags"].clone()).unwrap();
    assert!(tags.contains(&format!("contract_id:{CONTRACT}")));
}

#[tokio::test]
async fn event_alert_type_is_error_for_critical_and_archived() {
    for (report, label) in [
        (critical_report(), "critical"),
        (archived_report(), "archived"),
    ] {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/v1/series"))
            .and(header("DD-API-KEY", API_KEY))
            .respond_with(
                ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})),
            )
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/events"))
            .and(header("DD-API-KEY", API_KEY))
            .respond_with(
                ResponseTemplate::new(202)
                    .set_body_json(serde_json::json!({"status":"ok","event":{"id":1}})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
        adapter.observe(&report).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let event_req = requests
            .iter()
            .find(|r| r.url.path() == "/api/v1/events")
            .unwrap_or_else(|| panic!("no event sent for {label}"));

        let body: serde_json::Value = serde_json::from_slice(&event_req.body).unwrap();
        assert_eq!(
            body["alert_type"].as_str().unwrap(),
            "error",
            "{label} should produce alert_type=error"
        );
    }
}

// ── 6. No duplicate events when state is unchanged ───────────────────────────

#[tokio::test]
async fn no_duplicate_events_on_repeated_same_state() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})))
        .mount(&server)
        .await;

    // Events endpoint: should receive exactly one event across three identical scans.
    Mock::given(method("POST"))
        .and(path("/api/v1/events"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(
            ResponseTemplate::new(202)
                .set_body_json(serde_json::json!({"status":"ok","event":{"id":1}})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    // Three consecutive scans all reporting Warn – only the first should fire an event.
    adapter.observe(&warn_report()).await.unwrap();
    adapter.observe(&warn_report()).await.unwrap();
    adapter.observe(&warn_report()).await.unwrap();

    let event_requests: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/api/v1/events")
        .collect();

    assert_eq!(
        event_requests.len(),
        1,
        "only one event per state transition"
    );
}

#[tokio::test]
async fn event_fires_again_after_ok_resolves_state() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})))
        .mount(&server)
        .await;

    // Two events expected: initial Warn and the second Warn after OK clears state.
    Mock::given(method("POST"))
        .and(path("/api/v1/events"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(
            ResponseTemplate::new(202)
                .set_body_json(serde_json::json!({"status":"ok","event":{"id":1}})),
        )
        .expect(2)
        .mount(&server)
        .await;

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    adapter.observe(&warn_report()).await.unwrap(); // event 1
    adapter.observe(&ok_report()).await.unwrap(); // resolves, no event
    adapter.observe(&warn_report()).await.unwrap(); // event 2

    let event_count = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/api/v1/events")
        .count();
    assert_eq!(event_count, 2);
}

// ── 7. DD-API-KEY header present on all outbound requests ────────────────────

#[tokio::test]
async fn all_requests_carry_dd_api_key_header() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/events"))
        .and(header("DD-API-KEY", API_KEY))
        .respond_with(
            ResponseTemplate::new(202)
                .set_body_json(serde_json::json!({"status":"ok","event":{"id":1}})),
        )
        .mount(&server)
        .await;

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    adapter.observe(&warn_report()).await.unwrap();

    for request in server.received_requests().await.unwrap() {
        let key_header = request
            .headers
            .get("DD-API-KEY")
            .expect("DD-API-KEY header missing");
        assert_eq!(key_header.as_bytes(), API_KEY.as_bytes());
    }
}

// ── 8. Correct endpoint selection for US and EU sites ────────────────────────

#[test]
fn us_site_uses_datadoghq_com_base_url() {
    assert_eq!(DatadogSite::Us.base_url(), "https://api.datadoghq.com");
}

#[test]
fn eu_site_uses_datadoghq_eu_base_url() {
    assert_eq!(DatadogSite::Eu.base_url(), "https://api.datadoghq.eu");
}

#[test]
fn site_parse_roundtrip() {
    assert_eq!(
        DatadogSite::parse("datadoghq.com").unwrap(),
        DatadogSite::Us
    );
    assert_eq!(DatadogSite::parse("datadoghq.eu").unwrap(), DatadogSite::Eu);
}

#[test]
fn watch_args_expose_datadog_flags() {
    use clap::Parser;
    use sentinel_cli::{Cli, Commands};

    let cli = Cli::try_parse_from([
        "sentinel",
        "watch",
        CONTRACT,
        "--datadog-api-key",
        "fixture-key",
        "--datadog-site",
        "datadoghq.eu",
    ])
    .unwrap();

    assert!(matches!(&cli.command, Commands::Watch(args)
        if args.datadog_api_key.as_deref() == Some("fixture-key")
        && args.datadog_site == "datadoghq.eu"));

    // Credentials must not appear in the Debug output.
    let debug = format!("{cli:?}");
    assert!(
        !debug.contains("fixture-key"),
        "API key must not appear in debug output"
    );
}

// ── 9. Authentication failures and unsuccessful responses ────────────────────

#[tokio::test]
async fn authentication_failure_returns_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    let result = adapter.observe(&ok_report()).await;

    assert!(result.is_err(), "403 response should propagate as error");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("403") || msg.contains("authentication"),
        "error message should reference auth failure: {msg}"
    );
    // The raw API key must not appear in the error message.
    assert!(
        !msg.contains(API_KEY),
        "API key must not appear in error: {msg}"
    );
}

#[tokio::test]
async fn non_success_http_status_returns_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    // 500 triggers retries; after three attempts it should fail.
    let result = adapter.observe(&ok_report()).await;

    assert!(
        result.is_err(),
        "500 response should produce an error after retries"
    );
    let msg = result.unwrap_err();
    assert!(!msg.contains(API_KEY), "API key must not appear in error");
}

#[tokio::test]
async fn event_endpoint_non_success_returns_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/series"))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({"status":"ok"})))
        .mount(&server)
        .await;

    // Events endpoint returns 400
    Mock::given(method("POST"))
        .and(path("/api/v1/events"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;

    let mut adapter = DatadogAdapter::new_with_base_url(API_KEY, &server.uri()).unwrap();
    let result = adapter.observe(&warn_report()).await;

    assert!(result.is_err(), "400 on events should propagate as error");
    let msg = result.unwrap_err();
    assert!(!msg.contains(API_KEY), "API key must not appear in error");
}
