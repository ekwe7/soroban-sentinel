use std::collections::HashMap;
use std::env::VarError;

use clap::Parser;
use sentinel_cli::adapters::alert_telegram::{
    SentinelEvent, TelegramAlertDispatcher, TelegramAlertError, ENV_TELEGRAM_BOT_TOKEN,
    ENV_TELEGRAM_CHAT_ID,
};
use sentinel_cli::commands::watch::{dispatch_telegram_report, init_telegram_dispatcher};
use sentinel_cli::{Cli, Commands};
use sentinel_core::{
    Durability, EntryType, RiskLevel, StorageEntry, StorageEntryReport, TTLReport,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_TOKEN: &str = "123456789:AAH_test_bot_token_secret";
const TEST_CHAT_ID: &str = "-1001234567890";
const CONTRACT_ID: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";
const LEDGER_KEY_XDR: &str = "sample-ledger-key-xdr";

fn sample_entry(key: &str, live_until: u32) -> StorageEntry {
    StorageEntry::new(
        key,
        EntryType::PersistentData,
        Durability::Persistent,
        256,
        live_until,
        5_400_000,
    )
}

fn sample_report(network: &str, status: RiskLevel, live_until: u32) -> TTLReport {
    TTLReport {
        contract_id: CONTRACT_ID.into(),
        network: network.into(),
        latest_ledger: 5_400_000,
        ledger_velocity_seconds: 5.0,
        burn_rate: Default::default(),
        entries: vec![StorageEntryReport::new(
            "persistent:TREASURY",
            EntryType::PersistentData,
            Durability::Persistent,
            256,
            live_until,
            i64::from(live_until) - 5_400_000,
            500.0,
            0.005,
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
async fn dispatches_warn_and_critical_html_payloads_to_send_message_endpoint() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(format!("/bot{TEST_TOKEN}/sendMessage")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(3)
        .mount(&server)
        .await;

    let dispatcher = TelegramAlertDispatcher::new(TEST_TOKEN, TEST_CHAT_ID)
        .unwrap()
        .with_api_base_url(&server.uri())
        .unwrap();

    // 1. Warn on testnet
    let warn_event = SentinelEvent::risk_threshold_breached(
        CONTRACT_ID,
        "testnet",
        sample_entry("persistent:TREASURY", 5_450_000),
        50_000,
        RiskLevel::Warn,
    );
    assert!(dispatcher.handle_event(&warn_event).await.unwrap());

    // 2. Critical on mainnet
    let critical_event = SentinelEvent::risk_threshold_breached(
        CONTRACT_ID,
        "mainnet",
        sample_entry("instance", 5_401_000),
        1_000,
        RiskLevel::Critical,
    );
    assert!(dispatcher.handle_event(&critical_event).await.unwrap());

    // 3. Archived on mainnet
    let archived_event = SentinelEvent::risk_threshold_breached(
        CONTRACT_ID,
        "mainnet",
        sample_entry("persistent:EXPIRED", 5_399_000),
        -1_000,
        RiskLevel::Archived,
    );
    assert!(dispatcher.handle_event(&archived_event).await.unwrap());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);

    let warn_body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(warn_body["chat_id"], TEST_CHAT_ID);
    assert_eq!(warn_body["parse_mode"], "HTML");
    assert_eq!(warn_body["disable_web_page_preview"], true);
    let warn_text = warn_body["text"].as_str().unwrap();
    assert!(warn_text.contains("⚠️"));
    assert!(warn_text.contains("WARN"));
    assert!(warn_text.contains(CONTRACT_ID));
    assert!(warn_text.contains("5450000"));
    assert!(warn_text.contains(&format!(
        "https://stellar.expert/explorer/testnet/contract/{CONTRACT_ID}"
    )));

    let crit_body: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(crit_body["chat_id"], TEST_CHAT_ID);
    assert_eq!(crit_body["parse_mode"], "HTML");
    let crit_text = crit_body["text"].as_str().unwrap();
    assert!(crit_text.contains("🚨"));
    assert!(crit_text.contains("CRITICAL"));
    assert!(crit_text.contains("5401000"));
    assert!(crit_text.contains(&format!(
        "https://stellar.expert/explorer/public/contract/{CONTRACT_ID}"
    )));

    let archived_body: Value = serde_json::from_slice(&requests[2].body).unwrap();
    let archived_text = archived_body["text"].as_str().unwrap();
    assert!(archived_text.contains("🚨"));
    assert!(archived_text.contains("ARCHIVED"));
    assert!(archived_text.contains("5399000"));
}

#[tokio::test]
async fn escapes_html_characters_in_contract_ids_and_symbols() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(format!("/bot{TEST_TOKEN}/sendMessage")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    let dispatcher = TelegramAlertDispatcher::new(TEST_TOKEN, TEST_CHAT_ID)
        .unwrap()
        .with_api_base_url(&server.uri())
        .unwrap();

    let event = SentinelEvent::risk_threshold_breached(
        "CAAA<script>&\"alert\"",
        "testnet",
        sample_entry("persistent:<VAULT & OPS>", 5_410_000),
        10_000,
        RiskLevel::Critical,
    );

    assert!(dispatcher.handle_event(&event).await.unwrap());

    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["parse_mode"], "HTML");
    let text = body["text"].as_str().unwrap();

    assert!(text.contains("CAAA&lt;script&gt;&amp;&quot;alert&quot;"));
    assert!(text.contains("persistent:&lt;VAULT &amp; OPS&gt;"));
    assert!(!text.contains("<script>"));
    assert!(!text.contains("<VAULT"));
}

#[tokio::test]
async fn preserves_base_url_query_params_and_appends_telegram_query_params() {
    let server = MockServer::start().await;

    let event = SentinelEvent::risk_threshold_breached(
        CONTRACT_ID,
        "testnet",
        sample_entry("persistent:POOL", 5_440_000),
        40_000,
        RiskLevel::Warn,
    );

    let dispatcher = TelegramAlertDispatcher::new(TEST_TOKEN, TEST_CHAT_ID)
        .unwrap()
        .with_api_base_url(&format!("{}/proxy?env=staging", server.uri()))
        .unwrap()
        .with_disable_notification(true)
        .with_message_thread_id(77)
        .with_query_param("source", "soroban-sentinel");

    let expected_payload = dispatcher.build_payload(&event).unwrap();

    Mock::given(method("POST"))
        .and(path(format!("/proxy/bot{TEST_TOKEN}/sendMessage")))
        .and(query_param("env", "staging"))
        .and(query_param("disable_notification", "true"))
        .and(query_param("message_thread_id", "77"))
        .and(query_param("source", "soroban-sentinel"))
        .and(body_json(&expected_payload))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    assert!(dispatcher.handle_event(&event).await.unwrap());

    // Ensure bot tokens cannot inject query parameters or path segments.
    let injection_err =
        TelegramAlertDispatcher::new("123456:token?disable_notification=false", TEST_CHAT_ID)
            .unwrap_err();
    assert!(matches!(
        injection_err,
        TelegramAlertError::InvalidBotToken(_)
    ));
}

#[tokio::test]
async fn consumes_sentinel_events_from_mpsc_channel_and_ignores_non_breaches() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(format!("/bot{TEST_TOKEN}/sendMessage")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    let dispatcher = TelegramAlertDispatcher::new(TEST_TOKEN, TEST_CHAT_ID)
        .unwrap()
        .with_api_base_url(&server.uri())
        .unwrap();

    let (tx, rx) = mpsc::channel(8);

    tx.send(SentinelEvent::LedgerTicked {
        sequence: 5_400_000,
        close_time: 1_700_000_000,
    })
    .await
    .unwrap();
    tx.send(SentinelEvent::EntryEvaluated {
        contract_id: CONTRACT_ID.into(),
        network: "testnet".into(),
        entry: sample_entry("instance", 5_900_000),
        remaining_ledgers: 500_000,
        risk: RiskLevel::Ok,
    })
    .await
    .unwrap();
    tx.send(SentinelEvent::RpcPollFailed {
        error: "timeout".into(),
        consecutive_failures: 1,
    })
    .await
    .unwrap();
    tx.send(SentinelEvent::risk_threshold_breached(
        CONTRACT_ID,
        "testnet",
        sample_entry("instance", 5_900_000),
        500_000,
        RiskLevel::Ok,
    ))
    .await
    .unwrap();
    tx.send(SentinelEvent::risk_threshold_breached(
        CONTRACT_ID,
        "testnet",
        sample_entry("persistent:ADMIN", 5_400_500),
        500,
        RiskLevel::Critical,
    ))
    .await
    .unwrap();
    drop(tx);

    let errors = dispatcher.run(rx).await;
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn reads_credentials_from_env_vars_and_redacts_token_on_errors() {
    let missing_token =
        TelegramAlertDispatcher::from_env_with(|_| Err(VarError::NotPresent)).unwrap_err();
    assert_eq!(
        missing_token,
        TelegramAlertError::MissingEnvVar(ENV_TELEGRAM_BOT_TOKEN)
    );

    let missing_chat = TelegramAlertDispatcher::from_env_with(|key| match key {
        ENV_TELEGRAM_BOT_TOKEN => Ok(TEST_TOKEN.to_owned()),
        _ => Err(VarError::NotPresent),
    })
    .unwrap_err();
    assert_eq!(
        missing_chat,
        TelegramAlertError::MissingEnvVar(ENV_TELEGRAM_CHAT_ID)
    );

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TEST_TOKEN}/sendMessage")))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "ok": false,
            "error_code": 401,
            "description": format!("Unauthorized for token {TEST_TOKEN}")
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dispatcher = TelegramAlertDispatcher::from_env_with(|key| match key {
        ENV_TELEGRAM_BOT_TOKEN => Ok(TEST_TOKEN.to_owned()),
        ENV_TELEGRAM_CHAT_ID => Ok(TEST_CHAT_ID.to_owned()),
        _ => Err(VarError::NotPresent),
    })
    .unwrap()
    .with_api_base_url(&server.uri())
    .unwrap();

    let event = SentinelEvent::risk_threshold_breached(
        CONTRACT_ID,
        "testnet",
        sample_entry("instance", 5_400_100),
        100,
        RiskLevel::Critical,
    );
    let err = dispatcher.handle_event(&event).await.unwrap_err();
    let rendered = err.to_string();
    assert!(rendered.contains("401"), "{rendered}");
    assert!(
        !rendered.contains(TEST_TOKEN),
        "bot token leaked in error: {rendered}"
    );
    assert!(rendered.contains("[REDACTED]"), "{rendered}");
}

#[tokio::test]
async fn retries_transient_429_and_detects_api_rejection() {
    let server = MockServer::start().await;
    let endpoint = format!("/bot{TEST_TOKEN}/sendMessage");

    Mock::given(method("POST"))
        .and(path(&endpoint))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path(&endpoint))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    let dispatcher = TelegramAlertDispatcher::new(TEST_TOKEN, TEST_CHAT_ID)
        .unwrap()
        .with_api_base_url(&server.uri())
        .unwrap();

    let event = SentinelEvent::risk_threshold_breached(
        CONTRACT_ID,
        "testnet",
        sample_entry("persistent:TREASURY", 5_410_000),
        10_000,
        RiskLevel::Warn,
    );
    assert!(dispatcher.handle_event(&event).await.unwrap());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);

    // Verify Telegram 200 payload with {"ok": false} returns ApiRejected.
    let rejection_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(&endpoint))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": false,
            "description": "Bad Request: chat not found"
        })))
        .expect(1)
        .mount(&rejection_server)
        .await;

    let rejecting_dispatcher = TelegramAlertDispatcher::new(TEST_TOKEN, TEST_CHAT_ID)
        .unwrap()
        .with_api_base_url(&rejection_server.uri())
        .unwrap();
    let err = rejecting_dispatcher.handle_event(&event).await.unwrap_err();
    assert_eq!(
        err,
        TelegramAlertError::ApiRejected {
            detail: "Bad Request: chat not found".into()
        }
    );
}

#[tokio::test]
async fn watch_integration_deduplicates_escalates_and_resets_telegram_alerts() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(format!("/bot{TEST_TOKEN}/sendMessage")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(3)
        .mount(&server)
        .await;

    let dispatcher = TelegramAlertDispatcher::new(TEST_TOKEN, TEST_CHAT_ID)
        .unwrap()
        .with_api_base_url(&server.uri())
        .unwrap();

    let mut reported: HashMap<String, RiskLevel> = HashMap::new();
    let keys = vec![LEDGER_KEY_XDR.to_owned()];

    // Ok -> no alert
    assert_eq!(
        dispatch_telegram_report(
            &dispatcher,
            &sample_report("testnet", RiskLevel::Ok, 5_900_000),
            &keys,
            &mut reported
        )
        .await
        .unwrap(),
        0
    );
    // Warn -> alert #1
    assert_eq!(
        dispatch_telegram_report(
            &dispatcher,
            &sample_report("testnet", RiskLevel::Warn, 5_450_000),
            &keys,
            &mut reported
        )
        .await
        .unwrap(),
        1
    );
    // Repeated Warn -> deduplicated
    assert_eq!(
        dispatch_telegram_report(
            &dispatcher,
            &sample_report("testnet", RiskLevel::Warn, 5_449_000),
            &keys,
            &mut reported
        )
        .await
        .unwrap(),
        0
    );
    // Escalation to Critical -> alert #2
    assert_eq!(
        dispatch_telegram_report(
            &dispatcher,
            &sample_report("testnet", RiskLevel::Critical, 5_401_000),
            &keys,
            &mut reported
        )
        .await
        .unwrap(),
        1
    );
    // Recovery to Ok -> clears dedup state
    assert_eq!(
        dispatch_telegram_report(
            &dispatcher,
            &sample_report("testnet", RiskLevel::Ok, 5_900_000),
            &keys,
            &mut reported
        )
        .await
        .unwrap(),
        0
    );
    // Fresh regression to Warn -> alert #3
    assert_eq!(
        dispatch_telegram_report(
            &dispatcher,
            &sample_report("testnet", RiskLevel::Warn, 5_450_000),
            &keys,
            &mut reported
        )
        .await
        .unwrap(),
        1
    );

    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[test]
fn watch_validates_partial_telegram_credentials_and_hides_secrets_in_debug() {
    assert!(init_telegram_dispatcher(None, None).unwrap().is_none());
    assert!(init_telegram_dispatcher(Some(TEST_TOKEN.into()), None).is_err());
    assert!(init_telegram_dispatcher(None, Some(TEST_CHAT_ID.into())).is_err());
    assert!(
        init_telegram_dispatcher(Some(TEST_TOKEN.into()), Some(TEST_CHAT_ID.into()))
            .unwrap()
            .is_some()
    );

    let cli = Cli::try_parse_from([
        "sentinel",
        "watch",
        CONTRACT_ID,
        "--telegram-bot-token",
        TEST_TOKEN,
        "--telegram-chat-id",
        TEST_CHAT_ID,
    ])
    .unwrap();
    assert!(matches!(
        &cli.command,
        Commands::Watch(args)
            if args.telegram_bot_token.as_deref() == Some(TEST_TOKEN)
            && args.telegram_chat_id.as_deref() == Some(TEST_CHAT_ID)
    ));
    let debug = format!("{cli:?}");
    assert!(!debug.contains(TEST_TOKEN));
    assert!(!debug.contains(TEST_CHAT_ID));
}
