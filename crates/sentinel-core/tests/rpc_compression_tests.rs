use sentinel_core::{LedgerReader, SorobanRpcClient};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const RESPONSE: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"sequence":123,"protocolVersion":22}}"#;

async fn assert_latest_ledger_response(body: Vec<u8>, encoding: Option<&str>) {
    let server = MockServer::start().await;
    let mut response = ResponseTemplate::new(200).set_body_bytes(body);
    if let Some(encoding) = encoding {
        response = response.insert_header("content-encoding", encoding);
    }
    Mock::given(method("POST"))
        .respond_with(response)
        .mount(&server)
        .await;

    let client = SorobanRpcClient::new(server.uri());
    let latest = client
        .get_latest_ledger()
        .await
        .expect("valid RPC response");
    assert_eq!(latest.sequence, 123);
    assert_eq!(latest.protocol_version, 22);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["accept-encoding"], "gzip, zstd");
}

#[tokio::test]
async fn accepts_uncompressed_rpc_responses_with_compression_advertised() {
    assert_latest_ledger_response(RESPONSE.as_bytes().to_vec(), None).await;
}

#[tokio::test]
async fn transparently_decodes_gzip_rpc_responses() {
    assert_latest_ledger_response(
        include_bytes!("fixtures/rpc-response.json.gz").to_vec(),
        Some("gzip"),
    )
    .await;
}

#[tokio::test]
async fn transparently_decodes_zstd_rpc_responses() {
    assert_latest_ledger_response(
        include_bytes!("fixtures/rpc-response.json.zst").to_vec(),
        Some("zstd"),
    )
    .await;
}
