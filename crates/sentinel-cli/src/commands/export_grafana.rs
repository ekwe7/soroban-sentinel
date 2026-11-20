//! Export Grafana dashboard and Prometheus alert rule templates.

use std::fs;

use serde::Serialize;
use serde_json::{json, Value};

use crate::args::ExportGrafanaArgs;
use crate::commands::scan::EXIT_INVALID_ARGS;

const DASHBOARD_FILE: &str = "sentinel_dashboard.json";
const ALERTS_FILE: &str = "sentinel_alerts.yml";

#[derive(Serialize)]
struct AlertRuleFile {
    groups: Vec<AlertGroup>,
}

#[derive(Serialize)]
struct AlertGroup {
    name: &'static str,
    rules: Vec<AlertRule>,
}

#[derive(Serialize)]
struct AlertRule {
    alert: &'static str,
    expr: &'static str,
    #[serde(rename = "for")]
    duration: &'static str,
    labels: AlertLabels,
    annotations: AlertAnnotations,
}

#[derive(Serialize)]
struct AlertLabels {
    severity: &'static str,
}

#[derive(Serialize)]
struct AlertAnnotations {
    summary: &'static str,
    description: &'static str,
}

/// Writes the dashboard and alert rule templates into the requested directory.
pub fn run_export_grafana(args: ExportGrafanaArgs) -> i32 {
    if let Err(error) = fs::create_dir_all(&args.output_dir) {
        eprintln!(
            "Error: unable to create output directory '{}': {error}",
            args.output_dir.display()
        );
        return EXIT_INVALID_ARGS;
    }

    let dashboard_path = args.output_dir.join(DASHBOARD_FILE);
    let alerts_path = args.output_dir.join(ALERTS_FILE);
    let dashboard = match serde_json::to_string_pretty(&build_dashboard()) {
        Ok(dashboard) => dashboard,
        Err(error) => {
            eprintln!("Error: unable to serialize Grafana dashboard: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    let alert_rules = match serde_yaml::to_string(&build_alert_rules()) {
        Ok(alert_rules) => alert_rules,
        Err(error) => {
            eprintln!("Error: unable to serialize Prometheus alert rules: {error}");
            return EXIT_INVALID_ARGS;
        }
    };

    if let Err(error) = fs::write(&dashboard_path, dashboard) {
        eprintln!(
            "Error: unable to write dashboard '{}': {error}",
            dashboard_path.display()
        );
        return EXIT_INVALID_ARGS;
    }
    if let Err(error) = fs::write(&alerts_path, alert_rules) {
        eprintln!(
            "Error: unable to write alert rules '{}': {error}",
            alerts_path.display()
        );
        return EXIT_INVALID_ARGS;
    }

    println!(
        "Generated Grafana dashboard '{}' and Prometheus alert rules '{}'.",
        dashboard_path.display(),
        alerts_path.display()
    );
    0
}

fn build_dashboard() -> Value {
    let datasource = json!({ "type": "prometheus", "uid": "${datasource}" });
    let panels = [
        (
            1,
            "Contract TTL decay",
            "timeseries",
            "sentinel_entry_ttl_remaining_ledgers{network=\"$network\",contract_id=\"$contract_id\"}",
            "{{contract_id}} / {{entry}}",
            json!({ "unit": "short" }),
            0,
            0,
        ),
        (
            2,
            "Critical entries",
            "stat",
            "sum(sentinel_critical_entries{network=\"$network\",contract_id=\"$contract_id\"})",
            "critical entries",
            json!({ "unit": "short", "color": { "mode": "thresholds" }, "thresholds": { "mode": "absolute", "steps": [{ "color": "green" }, { "color": "red", "value": 1 }] } }),
            12,
            0,
        ),
        (
            3,
            "RPC latency (p95)",
            "timeseries",
            "histogram_quantile(0.95, sum by (le) (rate(sentinel_rpc_request_duration_seconds_bucket{network=\"$network\"}[5m])))",
            "p95",
            json!({ "unit": "s" }),
            0,
            8,
        ),
    ];
    let panels: Vec<Value> = panels
        .into_iter()
        .map(
            |(id, title, panel_type, expression, legend, defaults, x, y)| {
                let options = if panel_type == "stat" {
                    json!({
                        "colorMode": "value",
                        "graphMode": "area",
                        "justifyMode": "auto",
                        "orientation": "auto",
                        "reduceOptions": {
                            "calcs": ["lastNotNull"],
                            "fields": "",
                            "values": false
                        },
                        "textMode": "auto"
                    })
                } else {
                    json!({
                        "legend": {
                            "displayMode": "list",
                            "placement": "bottom",
                            "showLegend": true
                        },
                        "tooltip": { "mode": "single", "sort": "none" }
                    })
                };
                json!({
                    "datasource": datasource,
                    "fieldConfig": { "defaults": defaults, "overrides": [] },
                    "gridPos": { "h": 8, "w": 12, "x": x, "y": y },
                    "id": id,
                    "options": options,
                    "targets": [{
                        "datasource": datasource,
                        "editorMode": "code",
                        "expr": expression,
                        "legendFormat": legend,
                        "range": true,
                        "refId": "A"
                    }],
                    "title": title,
                    "type": panel_type
                })
            },
        )
        .collect();

    json!({
        "__inputs": [],
        "__requires": [{ "type": "grafana", "id": "grafana", "name": "Grafana", "version": "10.0.0" }],
        "annotations": {
            "list": [{
                "builtIn": 1,
                "datasource": { "type": "grafana", "uid": "-- Grafana --" },
                "enable": true,
                "hide": true,
                "iconColor": "rgba(0, 211, 255, 1)",
                "name": "Annotations & Alerts",
                "type": "dashboard"
            }]
        },
        "editable": true,
        "fiscalYearStartMonth": 0,
        "graphTooltip": 0,
        "links": [],
        "liveNow": false,
        "panels": panels,
        "refresh": "30s",
        "schemaVersion": 39,
        "tags": ["soroban", "sentinel"],
        "templating": {
            "list": [
                {
                    "current": { "selected": false, "text": "Prometheus", "value": "Prometheus" },
                    "hide": 0,
                    "includeAll": false,
                    "label": "Datasource",
                    "name": "datasource",
                    "options": [],
                    "query": "prometheus",
                    "refresh": 1,
                    "type": "datasource"
                },
                {
                    "current": { "selected": false, "text": "testnet", "value": "testnet" },
                    "hide": 0,
                    "includeAll": true,
                    "label": "Network",
                    "name": "network",
                    "query": "testnet,mainnet,futurenet,local",
                    "type": "custom"
                },
                {
                    "current": { "selected": false, "text": "All", "value": "$__all" },
                    "datasource": datasource,
                    "definition": "label_values(sentinel_entry_ttl_remaining_ledgers, contract_id)",
                    "includeAll": true,
                    "label": "Contract",
                    "name": "contract_id",
                    "query": { "query": "label_values(sentinel_entry_ttl_remaining_ledgers, contract_id)", "refId": "PrometheusVariableQueryEditor-VariableQuery" },
                    "refresh": 1,
                    "type": "query"
                }
            ]
        },
        "time": { "from": "now-6h", "to": "now" },
        "timepicker": {},
        "timezone": "",
        "title": "Soroban Sentinel",
        "uid": "soroban-sentinel",
        "version": 1
    })
}

fn build_alert_rules() -> AlertRuleFile {
    AlertRuleFile {
        groups: vec![AlertGroup {
            name: "sentinel.alerts",
            rules: vec![AlertRule {
                alert: "SentinelCriticalRisk",
                expr: "sentinel_contract_risk_level{risk_level=\"CRITICAL\"} == 1",
                duration: "1m",
                labels: AlertLabels { severity: "critical" },
                annotations: AlertAnnotations {
                    summary: "Soroban contract has critical TTL risk",
                    description: "Sentinel reports RiskLevel Critical for contract {{ $labels.contract_id }}.",
                },
            }],
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_dashboard_and_alerts_have_expected_structure() {
        let dashboard = build_dashboard();
        assert_eq!(dashboard["schemaVersion"], 39);
        assert_eq!(dashboard["panels"].as_array().unwrap().len(), 3);
        let panels = dashboard["panels"].as_array().unwrap();
        assert!(panels
            .iter()
            .any(|panel| panel["title"] == "Contract TTL decay"));
        assert!(panels
            .iter()
            .any(|panel| panel["title"] == "Critical entries"));
        assert!(panels
            .iter()
            .any(|panel| panel["title"] == "RPC latency (p95)"));
        let variables = dashboard["templating"]["list"].as_array().unwrap();
        assert!(variables.iter().any(|item| item["name"] == "network"));
        assert!(variables.iter().any(|item| item["name"] == "contract_id"));

        let json = serde_json::to_string(&dashboard).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), dashboard);

        let yaml = serde_yaml::to_string(&build_alert_rules()).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            parsed["groups"][0]["rules"][0]["alert"],
            "SentinelCriticalRisk"
        );
        assert!(parsed["groups"][0]["rules"][0]["expr"]
            .as_str()
            .unwrap()
            .contains("risk_level=\"CRITICAL\""));
    }
}
