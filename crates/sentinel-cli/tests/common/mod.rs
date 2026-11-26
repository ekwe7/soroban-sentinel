//! Shared wiremock fixtures for CLI integration tests.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use sentinel_core::adapters::derive_contract_data_entry_size_limit_key;
use serde_json::{json, Value};
use stellar_xdr::curr::{
    ConfigSettingEntry, ContractDataDurability, ContractDataEntry, ContractExecutable,
    ExtensionPoint, Hash, LedgerEntryData, Limits, ScAddress, ScBytes, ScContractInstance, ScMap,
    ScMapEntry, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// One ledger entry served by [`mount_ledger`]: `(key_xdr, entry_xdr, live_until_ledger)`.
pub type ServedEntry = (String, String, u32);

/// Base64 `LedgerEntryData` for a Stellar Asset instance with an optional storage map.
pub fn instance_entry_xdr(contract_id: [u8; 32], storage: Option<ScMap>) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::StellarAsset,
            storage,
        }),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

/// An instance storage map of `keys` entries, each holding `value_bytes` bytes.
pub fn storage_map(keys: u32, value_bytes: usize) -> ScMap {
    let entries: Vec<ScMapEntry> = (0..keys)
        .map(|i| ScMapEntry {
            key: ScVal::U32(i),
            val: ScVal::Bytes(ScBytes(vec![0x42; value_bytes].try_into().unwrap())),
        })
        .collect();
    ScMap(entries.try_into().unwrap())
}

/// The network `ContractDataEntrySizeBytes` config-setting entry with the given limit.
pub fn entry_size_limit_entry(limit: u32) -> ServedEntry {
    (
        derive_contract_data_entry_size_limit_key().unwrap(),
        LedgerEntryData::ConfigSetting(ConfigSettingEntry::ContractDataEntrySizeBytes(limit))
            .to_xdr_base64(Limits::none())
            .unwrap(),
        0,
    )
}

/// Base64 `LedgerEntryData` for a contract-data entry.
pub fn data_entry_xdr(
    contract_id: [u8; 32],
    key: ScVal,
    durability: ContractDataDurability,
) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key,
        durability,
        val: ScVal::U32(1),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

fn rpc_result(result: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "jsonrpc": "2.0", "id": 1, "result": result }))
}

/// Serves `getLatestLedger` and `getLedgerEntries`; each scan iteration `n` (counted by
/// `getLatestLedger` calls, starting at 0) is answered from `iterations[min(n, last)]`.
/// `getLedgerEntries` returns only the requested keys. Other methods get a JSON-RPC error.
pub async fn mount_ledger_sequence(server: &MockServer, iterations: Vec<(u32, Vec<ServedEntry>)>) {
    assert!(!iterations.is_empty());
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let current = calls.load(Ordering::SeqCst).saturating_sub(1);
            let (latest, entries) = &iterations[current.min(iterations.len() - 1)];
            match body["method"].as_str().unwrap_or_default() {
                "getLatestLedger" => {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    let (latest, _) = &iterations[n.min(iterations.len() - 1)];
                    rpc_result(json!({ "sequence": latest, "protocolVersion": 22 }))
                }
                "getLedgerEntries" => {
                    let requested: Vec<String> = body["params"]["keys"]
                        .as_array()
                        .map(|keys| {
                            keys.iter()
                                .filter_map(|k| k.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let served: Vec<Value> = entries
                        .iter()
                        .filter(|(key, _, _)| requested.contains(key))
                        .map(|(key, xdr, live_until)| {
                            json!({
                                "key": key,
                                "xdr": xdr,
                                "liveUntilLedgerSeq": live_until,
                                "lastModifiedLedgerSeq": latest - 1
                            })
                        })
                        .collect();
                    rpc_result(json!({ "latestLedger": latest, "entries": served }))
                }
                _ => ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": { "code": -32601, "message": "method not found" }
                })),
            }
        })
        .mount(server)
        .await;
}

/// Serves a single, unchanging ledger state (see [`mount_ledger_sequence`]).
pub async fn mount_ledger(server: &MockServer, latest_ledger: u32, entries: Vec<ServedEntry>) {
    mount_ledger_sequence(server, vec![(latest_ledger, entries)]).await;
}
