use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use serde_json::{json, Value};
use wiremock::{
    matchers::{body_json, method},
    Mock, MockServer, ResponseTemplate,
};

async fn respond(server: &MockServer, name: &str, result: Value) {
    Mock::given(method("POST"))
        .and(body_json(
            json!({"jsonrpc":"2.0","id":1,"method":name,"params":{}}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result})),
        )
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_reports_healthy_endpoint_and_rejects_wrong_network_or_protocol() {
    for (network, passphrase, protocol, success) in [
        ("testnet", "Test SDF Network ; September 2015", 22, true),
        (
            "mainnet",
            "Public Global Stellar Network ; September 2015",
            22,
            true,
        ),
        (
            "futurenet",
            "Test SDF Future Network ; October 2022",
            22,
            true,
        ),
        ("mainnet", "Test SDF Network ; September 2015", 22, false),
        ("testnet", "Test SDF Network ; September 2015", 19, false),
    ] {
        let server = MockServer::start().await;
        respond(
            &server,
            "getHealth",
            json!({"status":"healthy","latestLedger":42}),
        )
        .await;
        respond(
            &server,
            "getLatestLedger",
            json!({"sequence":42,"protocolVersion":protocol}),
        )
        .await;
        respond(
            &server,
            "getNetwork",
            json!({"passphrase":passphrase,"protocolVersion":protocol}),
        )
        .await;
        cargo_bin_cmd!("sentinel")
            .args(["doctor", "--rpc-url", &server.uri(), "--network", network])
            .assert()
            .code(i32::from(!success))
            .stdout(
                predicate::str::contains("42")
                    .and(predicate::str::contains("ms round trip"))
                    .and(predicate::str::contains(if success {
                        "PASS"
                    } else {
                        "FAIL"
                    })),
            );
    }
}

#[test]
fn doctor_bad_rpc_url_exits_one() {
    cargo_bin_cmd!("sentinel")
        .args(["doctor", "--rpc-url", "file:///tmp/example"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("RPC configuration"));
}
