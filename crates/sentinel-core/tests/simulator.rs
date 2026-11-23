use sentinel_core::{
    build_extend_footprint_ttl_envelope, envelope_to_xdr_base64, RentEstimate, SimulationEngine,
    SorobanRpcClient,
};
use stellar_xdr::curr::{
    ContractDataDurability, Hash, LedgerKey, LedgerKeyContractData, ScAddress, ScVal,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn key() -> LedgerKey {
    LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash([1; 32])),
        key: ScVal::U32(1),
        durability: ContractDataDurability::Persistent,
    })
}

async fn transaction_xdr() -> String {
    envelope_to_xdr_base64(&build_extend_footprint_ttl_envelope(&[key()], 1000, [7; 32]).unwrap())
        .unwrap()
}

#[tokio::test]
async fn simulate_transaction_parses_successful_fee() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": {
                "minResourceFee": "12345", "latestLedger": 99
            }
        })))
        .mount(&server)
        .await;
    let result = SimulationEngine::simulate_transaction(
        &SorobanRpcClient::new(server.uri()),
        &transaction_xdr().await,
        99,
    )
    .await
    .unwrap();
    assert_eq!(
        result,
        RentEstimate {
            min_resource_fee_stroops: 12345,
            latest_ledger: 99,
            cpu_instructions: 0,
            memory_bytes: 0
        }
    );
}

#[tokio::test]
async fn simulate_transaction_maps_rpc_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "simulation failed"}
        })))
        .mount(&server)
        .await;
    let error = SimulationEngine::simulate_transaction(
        &SorobanRpcClient::new(server.uri()),
        &transaction_xdr().await,
        99,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        sentinel_core::SentinelError::RpcResponseError { code: -32000, .. }
    ));
}

#[tokio::test]
async fn simulate_transaction_rejects_malformed_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{malformed"))
        .mount(&server)
        .await;
    let error = SimulationEngine::simulate_transaction(
        &SorobanRpcClient::new(server.uri()),
        &transaction_xdr().await,
        99,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        sentinel_core::SentinelError::SerializationError(_)
    ));
}
