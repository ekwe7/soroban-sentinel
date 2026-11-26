use assert_cmd::Command;
use sentinel_cli::commands::export_openapi::specification;
use serde_json::{json, Value};

#[test]
fn json_and_yaml_are_equivalent_and_validate_against_official_openapi_schema() {
    let schema: Value =
        serde_json::from_str(include_str!("fixtures/openapi-3.1-schema.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for format in ["json", "yaml"] {
        let output = Command::cargo_bin("sentinel")
            .unwrap()
            .args(["export-openapi", "--format", format])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let document: Value = if format == "json" {
            serde_json::from_slice(&output).unwrap()
        } else {
            serde_yaml::from_slice(&output).unwrap()
        };
        if let Some(error) = validator.iter_errors(&document).next() {
            panic!("{error}");
        }
        assert_eq!(document, specification());
        assert_eq!(document["openapi"], "3.1.0");
        for name in [
            "ContractId",
            "LedgerSequence",
            "Durability",
            "RiskLevel",
            "StorageEntry",
        ] {
            assert!(document["components"]["schemas"].get(name).is_some());
        }
    }
}

#[test]
fn enum_and_payload_schemas_validate_real_domain_serialization() {
    use sentinel_core::domain::{Durability, EntryType, RiskLevel, StorageEntry};
    let spec = specification();
    for (name, instance) in [
        ("RiskLevel", serde_json::to_value(RiskLevel::Ok).unwrap()),
        ("RiskLevel", serde_json::to_value(RiskLevel::Warn).unwrap()),
        (
            "RiskLevel",
            serde_json::to_value(RiskLevel::Critical).unwrap(),
        ),
        (
            "RiskLevel",
            serde_json::to_value(RiskLevel::Archived).unwrap(),
        ),
        (
            "StorageEntry",
            serde_json::to_value(StorageEntry::new(
                "key",
                EntryType::PersistentData,
                Durability::Persistent,
                100,
                2000,
                1,
            ))
            .unwrap(),
        ),
    ] {
        let root = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":format!("#/components/schemas/{name}"),"components":spec["components"]});
        let validator = jsonschema::validator_for(&root).unwrap();
        assert!(validator.is_valid(&instance));
    }
    let root = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"#/components/schemas/RiskLevel","components":spec["components"]});
    assert!(!jsonschema::validator_for(&root)
        .unwrap()
        .is_valid(&json!("BROKEN")));
}

#[tokio::test]
async fn schema_accepts_payloads_emitted_by_the_actual_webhook_adapter() {
    use sentinel_core::domain::{Durability, EntryType, RiskLevel};
    use sentinel_core::usecases::{StorageEntryReport, TTLReport};
    use sentinel_core::{AlertKind, AlertMinLevel, AlertNotifier, WebhookAlertNotifier};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let report = TTLReport {
        contract_id: stellar_strkey::Contract([1; 32]).to_string(),
        network: "testnet".into(),
        latest_ledger: 100,
        ledger_velocity_seconds: 5.0,
        entries: vec![StorageEntryReport::new(
            "key",
            EntryType::PersistentData,
            Durability::Persistent,
            100,
            200,
            100,
            500.0,
            500.0 / 86400.0,
            RiskLevel::Warn,
        )],
        overall_status: RiskLevel::Warn,
        archival_recommendations: vec![],
        sac_asset_info: None,
        burn_rate: Default::default(),
        contract_code_hash: None,
        contract_bytecode_size: None,
        contract_upgrade: None,
    };
    let adapter = WebhookAlertNotifier::new(server.uri()).unwrap();
    for kind in [AlertKind::Slack, AlertKind::Discord] {
        adapter
            .notify(&report, kind, AlertMinLevel::Warn)
            .await
            .unwrap();
    }
    let spec = specification();
    let outer = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"#/components/schemas/SentinelEvent","components":spec["components"]});
    let inner = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"#/components/schemas/RiskThresholdBreached","components":spec["components"]});
    for request in server.received_requests().await.unwrap() {
        let value: Value = serde_json::from_slice(&request.body).unwrap();
        assert!(jsonschema::validator_for(&outer).unwrap().is_valid(&value));
        let summary = value
            .pointer("/blocks/1/text/text")
            .or_else(|| value.pointer("/embeds/0/description"))
            .unwrap()
            .as_str()
            .unwrap();
        assert!(jsonschema::validator_for(&inner)
            .unwrap()
            .is_valid(&serde_json::from_str::<Value>(summary).unwrap()));
    }
}
