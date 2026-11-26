use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_cli_benchmark_rpc_help_flags_exit_0() {
    // Root --help
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains("Usage:"));

    // Root --version
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.arg("--version");
    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains("sentinel"));

    // Subcommand benchmark --help
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["benchmark", "--help"]);
    cmd.assert().code(0).stdout(predicate::str::contains(
        "Benchmark Soroban RPC endpoint performance",
    ));
}

#[tokio::test]
async fn test_cli_benchmark_rpc_missing_rpc_url_exit_10() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["benchmark"]); // Missing URL required argument
    cmd.assert().code(10).stderr(predicate::str::contains(
        "the following required arguments were not provided",
    ));
}

#[tokio::test]
async fn test_cli_benchmark_rpc_invalid_rpc_scheme_exit_11() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["benchmark", "file:///etc/passwd"]); // URL as positional argument

    cmd.assert()
        .code(11)
        .stderr(predicate::str::contains("Unsupported scheme"));
}

#[tokio::test]
async fn test_cli_benchmark_rpc_custom_concurrency_and_duration() {
    let mock_server = MockServer::start().await;

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "benchmark",
        &mock_server.uri(), // URL as positional argument
        "--concurrency",
        "5",
        "--duration-secs",
        "10",
        "--format",
        "text",
    ]);

    cmd.assert()
        .code(0)
        .stderr(predicate::str::contains("Concurrency: 5"))
        .stderr(predicate::str::contains("Duration: 10.0s"));
}

#[tokio::test]
async fn test_cli_benchmark_rpc_json_format_outputs_valid_schema() {
    let mock_server = MockServer::start().await;

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "benchmark",
        &mock_server.uri(), // URL as positional argument
        "--format",
        "json",
    ]);

    let assert = cmd.assert().code(0);
    let stdout_bytes = assert.get_output().stdout.clone();
    let parsed: serde_json::Value =
        serde_json::from_slice(&stdout_bytes).expect("Stdout must be valid JSON");

    assert!(parsed.get("rpc_url").is_some());
    assert!(parsed.get("network").is_some());
    assert!(parsed.get("duration_seconds").is_some());
    assert!(parsed.get("concurrency").is_some());
    assert!(parsed.get("method").is_some());
    assert!(parsed.get("total_requests").is_some());
    assert!(parsed.get("requests_per_second").is_some());
    assert!(parsed.get("percentiles").is_some());
    let percentiles = parsed["percentiles"]
        .as_object()
        .expect("percentiles must be object");
    assert!(percentiles.get("p50").is_some());
    assert!(percentiles.get("p95").is_some());
    assert!(percentiles.get("p99").is_some());
}

#[tokio::test]
async fn test_cli_benchmark_rpc_with_mock_rpc_server() {
    let mock_server = MockServer::start().await;

    // Mock getLatestLedger response
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "sequence": 5_400_000,
                "protocolVersion": 22
            }
        })))
        .mount(&mock_server)
        .await;

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "benchmark",
        &mock_server.uri(), // URL as positional argument
        "--concurrency",
        "3",
        "--duration-secs",
        "5",
        "--format",
        "text",
    ]);

    cmd.assert().code(0).stderr(predicate::str::contains(
        "Soroban Sentinel RPC Benchmark Report",
    ));
}

#[tokio::test]
async fn test_cli_benchmark_rpc_measures_multiple_methods() {
    let mock_server = MockServer::start().await;

    // Mock getLatestLedger
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "sequence": 5_400_000,
                        "protocolVersion": 22
                    }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": []
                    }
                })),
                other => panic!("Unexpected method: {}", other),
            }
        })
        .mount(&mock_server)
        .await;

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "benchmark",
        &mock_server.uri(), // URL as positional argument
        "--concurrency",
        "2",
        "--duration-secs",
        "10",
        "--format",
        "json",
    ]);

    let assert = cmd.assert().code(0);
    let stdout_bytes = assert.get_output().stdout.clone();
    let parsed: serde_json::Value =
        serde_json::from_slice(&stdout_bytes).expect("Stdout must be valid JSON");

    assert!(parsed.get("method").is_some());
    let method = parsed["method"].as_str().unwrap();
    assert!(method.contains("getLatestLedger") || method.contains("getLedgerEntries"));
    assert!(parsed.get("total_requests").is_some());
    assert!(parsed.get("requests_per_second").is_some());
    let percentiles = parsed["percentiles"]
        .as_object()
        .expect("percentiles must be object");
    assert!(percentiles.get("p50").is_some());
    assert!(percentiles.get("p95").is_some());
    assert!(percentiles.get("p99").is_some());
}
