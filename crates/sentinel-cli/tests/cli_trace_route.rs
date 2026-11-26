use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn reports_structured_connection_timings_and_http_version() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .and(query_param("source", "trace"))
        .and(header("content-type", "application/json"))
        .and(body_json(
            json!({"jsonrpc":"2.0","id":1,"method":"getHealth"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(40)))
        .expect(1)
        .mount(&server)
        .await;

    let url = format!("{}/rpc?source=trace", server.uri());
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_sentinel"))
            .args(["trace-route", "--rpc-url", &url, "--json"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let timings: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(timings["dns_lookup_ms"].as_f64().unwrap() >= 0.0);
    assert!(timings["tcp_connect_ms"].as_f64().unwrap() >= 0.0);
    assert_eq!(timings.get("tls_handshake_ms"), Some(&Value::Null));
    assert!(timings["http_ttfb_ms"].as_f64().unwrap() >= 35.0);
    assert_eq!(timings["http_version"], "HTTP/1.1");
    assert_eq!(timings["http_status"], 200);
}
