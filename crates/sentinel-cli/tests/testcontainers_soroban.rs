use std::time::Duration;
use tokio::time::Instant;

use assert_cmd::Command;
use serde_json::{json, Value};
use testcontainers::core::WaitFor;
use testcontainers::{clients, GenericImage};

const CONTRACT_ID_BYTES: [u8; 32] = [11; 32];

async fn wait_for_rpc(rpc_url: &str) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("RPC readiness client should build");
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        let health = client
            .post(rpc_url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"getHealth","params":{}}))
            .send()
            .await;
        if let Ok(response) = health {
            if response.status().is_success() {
                let latest = client
                    .post(rpc_url)
                    .json(&json!({"jsonrpc":"2.0","id":2,"method":"getLatestLedger","params":{}}))
                    .send()
                    .await;
                if let Ok(response) = latest {
                    if let Ok(body) = response.json::<Value>().await {
                        if body["result"]["sequence"].as_u64().unwrap_or_default() > 2 {
                            return;
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("Soroban RPC did not become healthy with ledger height greater than 2");
}

#[tokio::test]
#[ignore = "requires Docker and stellar/quickstart:soroban-dev"]
async fn local_soroban_rpc_supports_end_to_end_sentinel_scan() {
    let docker = clients::Cli::default();
    let image = GenericImage::new("stellar/quickstart", "soroban-dev")
        .with_exposed_port(8000)
        .with_wait_for(WaitFor::Nothing);
    let container = docker.run((
        image,
        vec![
            "--local".to_string(),
            "--enable".to_string(),
            "rpc".to_string(),
        ],
    ));
    let rpc_url = format!(
        "http://127.0.0.1:{}/rpc",
        container.get_host_port_ipv4(8000)
    );
    wait_for_rpc(&rpc_url).await;

    let contract_id = stellar_strkey::Contract(CONTRACT_ID_BYTES).to_string();
    let json_output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_id,
            "--rpc-url",
            &rpc_url,
            "--network",
            "local",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(json_output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(report["contract_id"], contract_id);
    assert!(report["entries"].is_array());
    assert!(report["summary"]["archived"].as_u64().unwrap_or_default() > 0);

    let text_output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_id,
            "--rpc-url",
            &rpc_url,
            "--network",
            "local",
            "--format",
            "text",
        ])
        .output()
        .unwrap();
    assert_eq!(text_output.status.code(), Some(2));
    let text = String::from_utf8(text_output.stdout).unwrap();
    assert!(text.contains("Entry Description"));
    assert!(text.contains("Summary:"));
}
