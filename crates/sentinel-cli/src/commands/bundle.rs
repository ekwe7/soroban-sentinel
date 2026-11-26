//! Execution logic for the `sentinel bundle` command.

use std::fs;
use std::io::Read as IoRead;
use std::time::Duration;

use sentinel_core::adapters::{
    derive_contract_instance_key, derive_persistent_symbol_key, derive_persistent_xdr_key,
    parse_contract_id, validate_rpc_url, SorobanRpcClient, MAX_RPC_RESPONSE_BYTES,
};
use sentinel_core::domain::{
    plan_bundle_footprints, BundleFootprint, MAX_BUNDLE_FOOTPRINT_ENTRIES,
};
use sentinel_core::ports::LedgerReader;
use serde::{Deserialize, Serialize};
use stellar_strkey::ed25519::PublicKey as AccountStrkey;
use stellar_strkey::Contract;
use stellar_xdr::curr::{
    AccountId, ExtensionPoint, LedgerEntry, LedgerEntryData, LedgerFootprint, LedgerKey,
    LedgerKeyAccount, Limits, Memo, MuxedAccount, Operation, OperationBody, Preconditions,
    PublicKey, ReadXdr, SequenceNumber, SorobanResources, SorobanTransactionData, Transaction,
    TransactionEnvelope, TransactionExt, TransactionV1Envelope, Uint256, VecM, WriteXdr,
};

use crate::args::BundleArgs;
use crate::commands::scan::{resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_RPC_FAILURE};

const MANIFEST_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleManifest {
    contracts: Vec<ManifestContract>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestContract {
    contract_id: String,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    key_xdr: Vec<String>,
}

#[derive(Debug, Clone)]
struct ContractInput {
    contract_id: String,
    keys: Vec<String>,
    key_xdr: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BundleOutput {
    pub(crate) network: String,
    pub(crate) source_account: String,
    pub(crate) latest_ledger: u32,
    pub(crate) extend_to_ledger: u32,
    pub(crate) max_entries_per_transaction: usize,
    pub(crate) signed: bool,
    pub(crate) total_fee_stroops: u64,
    pub(crate) transactions: Vec<BundleTransactionOutput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BundleTransactionOutput {
    pub(crate) entry_count: usize,
    pub(crate) extend_to_ledger: u32,
    pub(crate) resource_fee_stroops: u64,
    pub(crate) base_fee_stroops: u32,
    pub(crate) total_fee_stroops: u64,
    pub(crate) transaction_xdr_base64: String,
}

#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimulationResult {
    min_resource_fee: Option<FeeValue>,
    transaction_data: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FeeValue {
    String(String),
    Number(u64),
}

impl FeeValue {
    fn parse(self) -> Result<u64, String> {
        match self {
            Self::String(value) => value
                .parse::<u64>()
                .map_err(|error| format!("invalid minResourceFee '{}': {}", value, error)),
            Self::Number(value) => Ok(value),
        }
    }
}

/// Executes the bundle command and returns the CLI exit code.
pub async fn run_bundle(args: BundleArgs) -> i32 {
    match run_bundle_inner(args).await {
        Ok(output) => match serde_json::to_string_pretty(&output) {
            Ok(json) => {
                println!("{json}");
                0
            }
            Err(error) => {
                eprintln!("Error: failed to encode bundle output: {error}");
                EXIT_RPC_FAILURE
            }
        },
        Err(BundleCommandError::Invalid(message)) => {
            eprintln!("Error: {message}");
            EXIT_INVALID_ARGS
        }
        Err(BundleCommandError::Rpc(message)) => {
            eprintln!("Error: {message}");
            EXIT_RPC_FAILURE
        }
    }
}

#[derive(Debug)]
pub(crate) enum BundleCommandError {
    Invalid(String),
    Rpc(String),
}

impl From<sentinel_core::error::SentinelError> for BundleCommandError {
    fn from(error: sentinel_core::error::SentinelError) -> Self {
        match error {
            sentinel_core::error::SentinelError::InvalidContractAddress(_)
            | sentinel_core::error::SentinelError::InvalidXdrKey(_)
            | sentinel_core::error::SentinelError::InvalidRpcUrl(_) => {
                Self::Invalid(error.to_string())
            }
            _ => Self::Rpc(error.to_string()),
        }
    }
}

pub(crate) async fn run_bundle_inner(args: BundleArgs) -> Result<BundleOutput, BundleCommandError> {
    if args.extend_by == 0 {
        return Err(BundleCommandError::Invalid(
            "--extend-by must be greater than 0".to_string(),
        ));
    }
    if args.base_fee == 0 {
        return Err(BundleCommandError::Invalid(
            "--base-fee must be greater than 0".to_string(),
        ));
    }
    let rpc_url = resolve_rpc_url(&args.network, args.rpc_url.as_deref())
        .map_err(BundleCommandError::Invalid)?;
    validate_rpc_url(&rpc_url).map_err(BundleCommandError::from)?;

    let contracts = resolve_contracts(&args)?;
    let source_key = AccountStrkey::from_string(&args.source_account).map_err(|error| {
        BundleCommandError::Invalid(format!(
            "Invalid source account '{}': {}",
            args.source_account, error
        ))
    })?;

    let mut ledger_keys = Vec::new();
    for contract in &contracts {
        let contract_id = parse_contract_id(&contract.contract_id)?;
        ledger_keys.push(decode_ledger_key(&derive_contract_instance_key(
            &contract_id,
        )?)?);
        for key in &contract.keys {
            ledger_keys.push(decode_ledger_key(&derive_persistent_symbol_key(
                &contract_id,
                key,
            )?)?);
        }
        for key_xdr in &contract.key_xdr {
            ledger_keys.push(decode_ledger_key(&derive_persistent_xdr_key(
                &contract_id,
                key_xdr,
            )?)?);
        }
    }

    let bundles = plan_bundle_footprints(&ledger_keys, MAX_BUNDLE_FOOTPRINT_ENTRIES)
        .map_err(|error| BundleCommandError::Invalid(error.to_string()))?;
    let rpc_client = SorobanRpcClient::new_with_timeout(rpc_url.clone(), Duration::from_secs(10));
    let latest = rpc_client
        .get_latest_ledger()
        .await
        .map_err(BundleCommandError::from)?;
    let extend_to_ledger = latest.sequence.checked_add(args.extend_by).ok_or_else(|| {
        BundleCommandError::Invalid("--extend-by exceeds the maximum ledger sequence".to_string())
    })?;

    let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(source_key.0)));
    let account_key = LedgerKey::Account(LedgerKeyAccount {
        account_id: account_id.clone(),
    });
    let account_key_xdr = account_key
        .to_xdr_base64(Limits::none())
        .map_err(|error| BundleCommandError::Invalid(error.to_string()))?;
    let account_entries = rpc_client
        .get_ledger_entries(std::slice::from_ref(&account_key_xdr))
        .await
        .map_err(BundleCommandError::from)?;
    let account_entry_xdr = account_entries
        .iter()
        .find(|entry| entry.key_xdr_base64 == account_key_xdr)
        .map(|entry| entry.entry_xdr_base64.as_str())
        .ok_or_else(|| {
            BundleCommandError::Invalid(format!(
                "Source account '{}' was not found in ledger state",
                args.source_account
            ))
        })?;
    let ledger_entry =
        LedgerEntry::from_xdr_base64(account_entry_xdr, Limits::none()).map_err(|error| {
            BundleCommandError::Rpc(format!("invalid source account ledger XDR: {error}"))
        })?;
    let account_entry = match ledger_entry.data {
        LedgerEntryData::Account(account) if account.account_id == account_id => account,
        _ => {
            return Err(BundleCommandError::Rpc(
                "RPC returned a non-matching ledger entry for the source account".to_string(),
            ))
        }
    };

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| {
            BundleCommandError::Rpc(format!("failed to create RPC client: {error}"))
        })?;

    let mut sequence = account_entry.seq_num.0;
    let mut outputs = Vec::with_capacity(bundles.len());
    let mut total_fee_stroops = 0u64;
    for bundle in bundles {
        sequence = sequence.checked_add(1).ok_or_else(|| {
            BundleCommandError::Invalid("source account sequence overflow".to_string())
        })?;
        let base_fee_stroops = args.base_fee;
        let initial_data = soroban_data(&bundle.footprint, 0);
        let initial_envelope = build_envelope(
            &bundle,
            extend_to_ledger,
            source_key.0,
            sequence,
            base_fee_stroops,
            initial_data,
        )?;
        let simulation = simulate_transaction(&http_client, &rpc_url, &initial_envelope).await?;
        let resource_fee_stroops = simulation
            .min_resource_fee
            .ok_or_else(|| {
                BundleCommandError::Rpc(
                    "simulateTransaction response is missing minResourceFee".to_string(),
                )
            })?
            .parse()
            .map_err(BundleCommandError::Rpc)?;
        let total_transaction_fee = resource_fee_stroops
            .checked_add(base_fee_stroops as u64)
            .ok_or_else(|| BundleCommandError::Invalid("transaction fee overflow".to_string()))?;
        let transaction_fee = u32::try_from(total_transaction_fee).map_err(|_| {
            BundleCommandError::Invalid(format!(
                "simulated transaction fee {} exceeds the XDR u32 fee limit",
                total_transaction_fee
            ))
        })?;
        let transaction_data = match simulation.transaction_data {
            Some(data_xdr) => {
                let data = SorobanTransactionData::from_xdr_base64(&data_xdr, Limits::none())
                    .map_err(|error| {
                        BundleCommandError::Rpc(format!(
                            "simulateTransaction returned invalid transactionData XDR: {error}"
                        ))
                    })?;
                let expected = bundle.footprint.clone();
                if data.resources.footprint != expected {
                    return Err(BundleCommandError::Rpc(
                        "simulateTransaction changed the requested read-only footprint".to_string(),
                    ));
                }
                data
            }
            None => soroban_data(&bundle.footprint, 0),
        };
        let final_data = SorobanTransactionData {
            resource_fee: i64::try_from(resource_fee_stroops).map_err(|_| {
                BundleCommandError::Invalid("resource fee exceeds the XDR i64 limit".to_string())
            })?,
            ..transaction_data
        };
        let final_envelope = build_envelope(
            &bundle,
            extend_to_ledger,
            source_key.0,
            sequence,
            transaction_fee,
            final_data,
        )?;
        let transaction_xdr_base64 = final_envelope
            .to_xdr_base64(Limits::none())
            .map_err(|error| BundleCommandError::Invalid(error.to_string()))?;

        total_fee_stroops = total_fee_stroops
            .checked_add(total_transaction_fee)
            .ok_or_else(|| BundleCommandError::Invalid("combined fee overflow".to_string()))?;
        outputs.push(BundleTransactionOutput {
            entry_count: bundle.keys.len(),
            extend_to_ledger,
            resource_fee_stroops,
            base_fee_stroops,
            total_fee_stroops: total_transaction_fee,
            transaction_xdr_base64,
        });
    }

    Ok(BundleOutput {
        network: args.network,
        source_account: args.source_account,
        latest_ledger: latest.sequence,
        extend_to_ledger,
        max_entries_per_transaction: MAX_BUNDLE_FOOTPRINT_ENTRIES,
        signed: false,
        total_fee_stroops,
        transactions: outputs,
    })
}

fn resolve_contracts(args: &BundleArgs) -> Result<Vec<ContractInput>, BundleCommandError> {
    let mut contracts = Vec::<ContractInput>::new();
    if let Some(path) = &args.manifest {
        let file = fs::File::open(path).map_err(|error| {
            BundleCommandError::Invalid(format!(
                "failed to read manifest '{}': {error}",
                path.display()
            ))
        })?;
        let mut contents = Vec::new();
        file.take(MANIFEST_MAX_BYTES + 1)
            .read_to_end(&mut contents)
            .map_err(|error| {
                BundleCommandError::Invalid(format!(
                    "failed to read manifest '{}': {error}",
                    path.display()
                ))
            })?;
        if contents.len() as u64 > MANIFEST_MAX_BYTES {
            return Err(BundleCommandError::Invalid(format!(
                "manifest '{}' exceeds the {} byte limit",
                path.display(),
                MANIFEST_MAX_BYTES
            )));
        }
        let manifest: BundleManifest = serde_json::from_slice(&contents).map_err(|error| {
            BundleCommandError::Invalid(format!(
                "invalid JSON bundle manifest '{}': {error}",
                path.display()
            ))
        })?;
        for entry in manifest.contracts {
            merge_contract(
                &mut contracts,
                &entry.contract_id,
                &entry.keys,
                &entry.key_xdr,
            )?;
        }
    }
    for contract in &args.contracts {
        merge_contract(&mut contracts, contract, &[], &[])?;
    }
    if contracts.is_empty() {
        return Err(BundleCommandError::Invalid(
            "provide at least one --contract or a manifest with contracts".to_string(),
        ));
    }
    for contract in &mut contracts {
        for key in &args.keys {
            if !contract.keys.contains(key) {
                contract.keys.push(key.clone());
            }
        }
        for key_xdr in &args.key_xdr {
            if !contract.key_xdr.contains(key_xdr) {
                contract.key_xdr.push(key_xdr.clone());
            }
        }
    }
    Ok(contracts)
}

fn merge_contract(
    contracts: &mut Vec<ContractInput>,
    contract_id: &str,
    keys: &[String],
    key_xdr: &[String],
) -> Result<(), BundleCommandError> {
    let decoded = parse_contract_id(contract_id)?;
    let canonical_id = Contract(decoded).to_string();
    let target = match contracts
        .iter_mut()
        .find(|existing| existing.contract_id == canonical_id)
    {
        Some(target) => target,
        None => {
            contracts.push(ContractInput {
                contract_id: canonical_id,
                keys: Vec::new(),
                key_xdr: Vec::new(),
            });
            contracts.last_mut().expect("just inserted contract")
        }
    };
    for key in keys {
        if !target.keys.contains(key) {
            target.keys.push(key.clone());
        }
    }
    for key in key_xdr {
        if !target.key_xdr.contains(key) {
            target.key_xdr.push(key.clone());
        }
    }
    Ok(())
}

fn decode_ledger_key(key_xdr: &str) -> Result<LedgerKey, BundleCommandError> {
    LedgerKey::from_xdr_base64(key_xdr, Limits::none())
        .map_err(|error| BundleCommandError::Invalid(format!("invalid LedgerKey XDR: {error}")))
}

fn soroban_data(footprint: &LedgerFootprint, resource_fee: i64) -> SorobanTransactionData {
    SorobanTransactionData {
        ext: ExtensionPoint::V0,
        resources: SorobanResources {
            footprint: footprint.clone(),
            instructions: 0,
            read_bytes: 0,
            write_bytes: 0,
        },
        resource_fee,
    }
}

fn build_envelope(
    bundle: &BundleFootprint,
    extend_to_ledger: u32,
    source_account: [u8; 32],
    sequence: i64,
    fee: u32,
    transaction_data: SorobanTransactionData,
) -> Result<TransactionEnvelope, BundleCommandError> {
    let operation = Operation {
        source_account: None,
        body: OperationBody::ExtendFootprintTtl(stellar_xdr::curr::ExtendFootprintTtlOp {
            ext: bundle.extension_point(),
            extend_to: extend_to_ledger,
        }),
    };
    let operations = VecM::<Operation, 100>::try_from(vec![operation]).map_err(|error| {
        BundleCommandError::Invalid(format!("failed to build transaction operation: {error}"))
    })?;
    let transaction = Transaction {
        source_account: MuxedAccount::Ed25519(Uint256(source_account)),
        fee,
        seq_num: SequenceNumber(sequence),
        cond: Preconditions::None,
        memo: Memo::None,
        operations,
        ext: TransactionExt::V1(transaction_data),
    };
    Ok(TransactionEnvelope::Tx(TransactionV1Envelope {
        tx: transaction,
        signatures: VecM::default(),
    }))
}

async fn simulate_transaction(
    client: &reqwest::Client,
    rpc_url: &str,
    envelope: &TransactionEnvelope,
) -> Result<SimulationResult, BundleCommandError> {
    let transaction_xdr = envelope
        .to_xdr_base64(Limits::none())
        .map_err(|error| BundleCommandError::Invalid(error.to_string()))?;
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "simulateTransaction",
        "params": {
            "transaction": transaction_xdr
        }
    });
    let response = client
        .post(rpc_url)
        .json(&request)
        .send()
        .await
        .map_err(|error| {
            BundleCommandError::Rpc(format!("simulateTransaction request failed: {error}"))
        })?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RPC_RESPONSE_BYTES as u64)
    {
        return Err(BundleCommandError::Rpc(format!(
            "simulateTransaction response exceeded the {} byte limit",
            MAX_RPC_RESPONSE_BYTES
        )));
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        BundleCommandError::Rpc(format!("failed to read simulation response: {error}"))
    })? {
        if body.len().saturating_add(chunk.len()) > MAX_RPC_RESPONSE_BYTES {
            return Err(BundleCommandError::Rpc(format!(
                "simulateTransaction response exceeded the {} byte limit",
                MAX_RPC_RESPONSE_BYTES
            )));
        }
        body.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(BundleCommandError::Rpc(format!(
            "simulateTransaction returned HTTP {}: {}",
            status,
            String::from_utf8_lossy(&body)
        )));
    }
    let response: RpcResponse<SimulationResult> =
        serde_json::from_slice(&body).map_err(|error| {
            BundleCommandError::Rpc(format!(
                "invalid simulateTransaction JSON response: {error}"
            ))
        })?;
    if let Some(error) = response.error {
        return Err(BundleCommandError::Rpc(format!(
            "simulateTransaction RPC error {}: {}",
            error.code, error.message
        )));
    }
    let result = response.result.ok_or_else(|| {
        BundleCommandError::Rpc("simulateTransaction response is missing result".to_string())
    })?;
    Ok(result)
}
