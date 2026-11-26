use assert_cmd::Command;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn export_grafana_writes_dashboard_and_alert_rules() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output_dir = std::env::temp_dir().join(format!("sentinel-grafana-export-{unique}"));

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "export-grafana",
            "--output-dir",
            output_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    let dashboard_path = output_dir.join("sentinel_dashboard.json");
    let alerts_path = output_dir.join("sentinel_alerts.yml");
    let dashboard: Value =
        serde_json::from_slice(&std::fs::read(&dashboard_path).unwrap()).unwrap();
    assert_eq!(dashboard["schemaVersion"], 39);
    assert_eq!(dashboard["panels"].as_array().unwrap().len(), 3);

    let alerts: serde_yaml::Value =
        serde_yaml::from_slice(&std::fs::read(&alerts_path).unwrap()).unwrap();
    assert_eq!(
        alerts["groups"][0]["rules"][0]["alert"],
        "SentinelCriticalRisk"
    );
    assert!(alerts["groups"][0]["rules"][0]["expr"]
        .as_str()
        .unwrap()
        .contains("risk_level=\"CRITICAL\""));

    std::fs::remove_file(dashboard_path).unwrap();
    std::fs::remove_file(alerts_path).unwrap();
    std::fs::remove_dir(output_dir).unwrap();
}
