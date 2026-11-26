use assert_cmd::Command;
use sentinel_core::derive_contract_instance_key;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn instance_xdr(contract_id: [u8; 32]) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::StellarAsset,
            storage: None,
        }),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

#[tokio::test]
async fn scan_pushgateway_format_emits_openmetrics_gauges_and_labels() {
    let server = MockServer::start().await;
    let contract_id = [9u8; 32];
    let contract = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_data = instance_xdr(contract_id);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc":"2.0", "id":1,
                    "result":{"sequence":5_400_000,"protocolVersion":22}
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc":"2.0", "id":1,
                    "result":{"latestLedger":5_400_000,"entries":[{
                        "key":instance_key,"xdr":instance_data,
                        "liveUntilLedgerSeq":5_600_000,"lastModifiedLedgerSeq":5_390_000
                    }]}
                })),
                other => panic!("unexpected RPC method {other}"),
            }
        })
        .mount(&server)
        .await;

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract,
            "--rpc-url",
            &server.uri(),
            "--format",
            "pushgateway",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    for metric in [
        "soroban_sentinel_entry_remaining_ledgers",
        "soroban_sentinel_entry_byte_size",
        "soroban_sentinel_entry_risk_level",
    ] {
        assert!(text.contains(&format!("# HELP {metric} ")));
        assert!(text.contains(&format!("# TYPE {metric} gauge")));
    }
    assert!(text.contains(&format!(
        "soroban_sentinel_entry_remaining_ledgers{{contract_id=\"{contract}\",durability=\"instance\",entry_type=\"contract_instance\",network=\"testnet\"}} 200000"
    )));
    assert!(text.contains("soroban_sentinel_entry_risk_level{"));
    assert!(text.ends_with("# EOF\n"));
    assert!(!text.contains("\n\n"));
}
