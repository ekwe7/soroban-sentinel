use assert_cmd::Command;
use predicates::prelude::*;

const CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

#[tokio::test]
async fn dry_run_validates_contract_and_key_expression_before_rpc() {
    let server = wiremock::MockServer::start().await;
    let mock = wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"getHealth","params":{}})))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"status":"healthy","latestLedger":123}})));
    mock.mount(&server).await;
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command.args([
        "dry-run",
        CONTRACT,
        "--key-expr",
        "vec[sym:Balance, u32:1]",
        "--rpc-url",
        &server.uri(),
    ]);
    command
        .assert()
        .success()
        .stdout(predicate::str::contains("rpc_health").and(predicate::str::contains("passed")));
}

#[test]
fn invalid_local_arguments_return_10_without_rpc() {
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command.args([
        "dry-run",
        "not-a-contract",
        "--key-expr",
        "bad",
        "--rpc-url",
        "http://127.0.0.1:1",
    ]);
    command
        .assert()
        .code(10)
        .stdout(predicate::str::contains("contract_id").and(predicate::str::contains("key_expr")));
}

#[test]
fn unreachable_health_endpoint_returns_20() {
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command.args(["dry-run", CONTRACT, "--rpc-url", "http://127.0.0.1:1"]);
    command
        .assert()
        .code(20)
        .stdout(predicate::str::contains("rpc_health"));
}
