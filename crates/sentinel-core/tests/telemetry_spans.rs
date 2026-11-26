use std::sync::{Arc, Mutex};

use sentinel_core::adapters::{derive_contract_instance_key, SorobanRpcClient};
use sentinel_core::ports::mock::MockLedgerReader;
use sentinel_core::ports::reader::{LedgerReader, RawLedgerEntry};
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};
use serde_json::json;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use tracing::span::Attributes;
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone, Default)]
struct SpanCollector {
    names: Arc<Mutex<Vec<String>>>,
}

impl SpanCollector {
    fn names(&self) -> Vec<String> {
        self.names.lock().unwrap().clone()
    }
}

impl<S: Subscriber> Layer<S> for SpanCollector {
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &tracing::span::Id, _ctx: Context<'_, S>) {
        self.names
            .lock()
            .unwrap()
            .push(attrs.metadata().name().to_string());
    }
}

fn instance_xdr(contract_id: [u8; 32]) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::Wasm(Hash([61u8; 32])),
            storage: None,
        }),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn scan_emits_execution_and_xdr_parsing_spans() {
    let contract_bytes = [60u8; 32];
    let contract_id = stellar_strkey::Contract(contract_bytes).to_string();
    let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
    let entry = RawLedgerEntry::new(
        &instance_key,
        instance_xdr(contract_bytes),
        5_500_000,
        5_400_000,
    );
    let reader = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_entry(entry);

    let collector = SpanCollector::default();
    let subscriber = registry().with(collector.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let report = ScanContractUseCase::new(reader)
        .execute(ScanRequest::new(&contract_id))
        .await
        .unwrap();
    assert!(!report.entries.is_empty());

    let names = collector.names();
    for expected in [
        "execute",
        "compute_byte_size",
        "extract_wasm_hash_from_instance_entry",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "missing span {expected} in {names:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rpc_dispatcher_emits_request_spans() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": 22,
                "sequence": 5_400_000
            }
        })))
        .mount(&server)
        .await;

    let collector = SpanCollector::default();
    let subscriber = registry().with(collector.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let client = SorobanRpcClient::new(server.uri());
    let latest = client.get_latest_ledger().await.unwrap();
    assert_eq!(latest.sequence, 5_400_000);

    assert!(
        collector
            .names()
            .iter()
            .any(|name| name == "send_rpc_request"),
        "missing send_rpc_request span in {:?}",
        collector.names()
    );
}
