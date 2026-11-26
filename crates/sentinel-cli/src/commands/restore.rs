//! Execution logic for the `sentinel restore` command.
//!
//! Detects archived persistent/contract-instance keys, builds a CAP-0046
//! `RestoreFootprintOp`, simulates the resource fee, and emits an unsigned
//! transaction plus ready-to-run `stellar contract restore` commands. No
//! private keys are handled or used.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use sentinel_core::adapters::{
    derive_contract_instance_key, derive_persistent_key_from_scval, derive_persistent_symbol_key,
    derive_persistent_xdr_key, parse_contract_id, parse_key_expr, validate_rpc_url,
    SorobanRpcClient, MAX_RPC_RESPONSE_BYTES,
};
use sentinel_core::domain::plan_restore_footprint;
use sentinel_core::error::SentinelError;
use sentinel_core::ports::{LedgerReader, RawLedgerEntry};
use serde::{Deserialize, Serialize};
use stellar_strkey::ed25519::PublicKey as AccountStrkey;
use stellar_xdr::curr::{
    ExtensionPoint, LedgerFootprint, LedgerKey, Limits, Memo, MuxedAccount, Operation,
    OperationBody, Preconditions, ReadXdr, ScVal, SequenceNumber, SorobanResources,
    SorobanTransactionData, Transaction, TransactionEnvelope, TransactionExt,
    TransactionV1Envelope, Uint256, VecM, WriteXdr,
};

use crate::args::RestoreArgs;
use crate::commands::scan::{resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_RPC_FAILURE};

#[derive(Debug)]
enum RestoreCommandError {
    Invalid(String),
    Rpc(String),
}

impl From<SentinelError> for RestoreCommandError {
    fn from(error: SentinelError) -> Self {
        match error {
            SentinelError::InvalidContractAddress(_)
            | SentinelError::InvalidXdrKey(_)
            | SentinelError::InvalidRpcUrl(_) => Self::Invalid(error.to_string()),
            _ => Self::Rpc(error.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
enum TargetKind {
    Instance,
    Symbol(String),
    ScVal(String),
}

#[derive(Debug, Clone)]
struct RestoreTarget {
    ledger_key_xdr: String,
    kind: TargetKind,
    archived: bool,
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
    results: Option<Vec<serde_json::Value>>,
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

#[derive(Debug, Serialize)]
struct RestoreOutput {
    contract_id: String,
    network: String,
    latest_ledger: u32,
    source_account: String,
    signed: bool,
    archived_keys: Vec<String>,
    resource_fee_stroops: u64,
    base_fee_stroops: u32,
    total_fee_stroops: u64,
    footprint_xdr_base64: String,
    transaction_xdr_base64: String,
    stellar_commands: Vec<String>,
}

/// Executes the restore command and returns the CLI exit code.
pub async fn run_restore(args: RestoreArgs) -> i32 {
    let format = args.format.clone();
    match run_restore_inner(args).await {
        Ok(output) => {
            let rendered = if format == "json" {
                match serde_json::to_string_pretty(&output) {
                    Ok(json) => json,
                    Err(error) => {
                        eprintln!("Error: failed to encode restore output: {error}");
                        return EXIT_RPC_FAILURE;
                    }
                }
            } else {
                render_text(&output)
            };
            println!("{}", rendered);
            0
        }
        Err(RestoreCommandError::Invalid(message)) => {
            eprintln!("Error: {message}");
            EXIT_INVALID_ARGS
        }
        Err(RestoreCommandError::Rpc(message)) => {
            eprintln!("Error: {message}");
            EXIT_RPC_FAILURE
        }
    }
}

async fn run_restore_inner(args: RestoreArgs) -> Result<RestoreOutput, RestoreCommandError> {
    if args.base_fee == 0 {
        return Err(RestoreCommandError::Invalid(
            "--base-fee must be greater than 0".to_string(),
        ));
    }

    let rpc_url = resolve_rpc_url(&args.network, args.rpc_url.as_deref())
        .map_err(RestoreCommandError::Invalid)?;
    validate_rpc_url(&rpc_url)?;

    let contract_id = parse_contract_id(&args.contract_id)?;
    let source_key = match args.source_account.as_deref() {
        Some(account) => AccountStrkey::from_string(account).map_err(|error| {
            RestoreCommandError::Invalid(format!("Invalid source account '{}': {}", account, error))
        })?,
        None => AccountStrkey(contract_id),
    };
    let source_account = args
        .source_account
        .clone()
        .unwrap_or_else(|| source_key.to_string());

    let mut targets = derive_targets(&args, &contract_id)?;
    let mut seen = HashSet::with_capacity(targets.len());
    targets.retain(|target| seen.insert(target.ledger_key_xdr.clone()));

    let client = SorobanRpcClient::new(&rpc_url);
    let latest = client.get_latest_ledger().await?;
    let requested_keys: Vec<String> = targets
        .iter()
        .map(|target| target.ledger_key_xdr.clone())
        .collect();
    let entries = client.get_ledger_entries(&requested_keys).await?;
    let found: HashMap<&str, &RawLedgerEntry> = entries
        .iter()
        .map(|entry| (entry.key_xdr_base64.as_str(), entry))
        .collect();

    for target in &mut targets {
        target.archived = match found.get(target.ledger_key_xdr.as_str()) {
            None => true,
            Some(entry) => entry.live_until_ledger_seq <= latest.sequence,
        };
    }

    let archived_targets: Vec<&RestoreTarget> =
        targets.iter().filter(|target| target.archived).collect();
    if archived_targets.is_empty() {
        return Err(RestoreCommandError::Invalid(
            "No archived keys found for the requested targets; nothing to restore".to_string(),
        ));
    }

    let ledger_keys = archived_targets
        .iter()
        .map(|target| decode_ledger_key(&target.ledger_key_xdr))
        .collect::<Result<Vec<_>, _>>()?;
    let plan = plan_restore_footprint(&ledger_keys)
        .map_err(|error| RestoreCommandError::Invalid(error.to_string()))?;
    let footprint_xdr = plan
        .to_xdr_base64()
        .map_err(|error| RestoreCommandError::Invalid(error.to_string()))?;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| {
            RestoreCommandError::Rpc(format!("failed to create RPC client: {error}"))
        })?;

    let sequence = i64::from(latest.sequence);
    let initial_data = soroban_data(&plan.footprint, 0);
    let initial_envelope = build_envelope(source_key.0, sequence, args.base_fee, initial_data)?;
    let simulation = simulate_transaction(&http_client, &rpc_url, &initial_envelope).await?;

    if let Some(results) = &simulation.results {
        for result in results {
            if let Some(error) = result.get("error").and_then(|value| value.as_str()) {
                return Err(RestoreCommandError::Rpc(format!(
                    "simulateTransaction reported an operation error: {error}"
                )));
            }
        }
    }

    let resource_fee = simulation
        .min_resource_fee
        .ok_or_else(|| {
            RestoreCommandError::Rpc(
                "simulateTransaction response is missing minResourceFee".to_string(),
            )
        })?
        .parse()
        .map_err(RestoreCommandError::Rpc)?;
    let total_fee = resource_fee
        .checked_add(u64::from(args.base_fee))
        .ok_or_else(|| {
            RestoreCommandError::Invalid("simulated transaction fee overflow".to_string())
        })?;
    let transaction_fee = u32::try_from(total_fee).map_err(|_| {
        RestoreCommandError::Invalid(format!(
            "simulated transaction fee {} exceeds the XDR u32 fee limit",
            total_fee
        ))
    })?;

    let transaction_data = match simulation.transaction_data {
        Some(data_xdr) => {
            let data = SorobanTransactionData::from_xdr_base64(&data_xdr, Limits::none()).map_err(
                |error| {
                    RestoreCommandError::Rpc(format!(
                        "simulateTransaction returned invalid transactionData XDR: {error}"
                    ))
                },
            )?;
            if data.resources.footprint != plan.footprint {
                return Err(RestoreCommandError::Rpc(
                    "simulateTransaction changed the requested restore footprint".to_string(),
                ));
            }
            data
        }
        None => soroban_data(&plan.footprint, 0),
    };
    let final_data = SorobanTransactionData {
        resource_fee: i64::try_from(resource_fee).map_err(|_| {
            RestoreCommandError::Invalid("resource fee exceeds the XDR i64 limit".to_string())
        })?,
        ..transaction_data
    };
    let final_envelope = build_envelope(source_key.0, sequence, transaction_fee, final_data)?;
    let transaction_xdr = final_envelope
        .to_xdr_base64(Limits::none())
        .map_err(|error| {
            RestoreCommandError::Invalid(format!("failed to encode transaction XDR: {error}"))
        })?;

    let stellar_commands = archived_targets
        .iter()
        .map(|target| {
            stellar_restore_command(
                &args.contract_id,
                target,
                &args.network,
                args.source_account.as_deref(),
            )
        })
        .collect();

    Ok(RestoreOutput {
        contract_id: args.contract_id.clone(),
        network: args.network.clone(),
        latest_ledger: latest.sequence,
        source_account,
        signed: false,
        archived_keys: archived_targets
            .iter()
            .map(|target| target.ledger_key_xdr.clone())
            .collect(),
        resource_fee_stroops: resource_fee,
        base_fee_stroops: args.base_fee,
        total_fee_stroops: total_fee,
        footprint_xdr_base64: footprint_xdr,
        transaction_xdr_base64: transaction_xdr,
        stellar_commands,
    })
}

fn derive_targets(
    args: &RestoreArgs,
    contract_id: &[u8; 32],
) -> Result<Vec<RestoreTarget>, RestoreCommandError> {
    let mut targets = Vec::new();

    for symbol in &args.key {
        targets.push(RestoreTarget {
            ledger_key_xdr: derive_persistent_symbol_key(contract_id, symbol)?,
            kind: TargetKind::Symbol(symbol.clone()),
            archived: false,
        });
    }

    for scval_xdr in &args.key_xdr {
        targets.push(RestoreTarget {
            ledger_key_xdr: derive_persistent_xdr_key(contract_id, scval_xdr)?,
            kind: TargetKind::ScVal(scval_xdr.clone()),
            archived: false,
        });
    }

    for expression in &args.key_expr {
        let scval: ScVal = parse_key_expr(expression)?;
        let scval_xdr = scval
            .to_xdr_base64(Limits::none())
            .map_err(|error| RestoreCommandError::Invalid(error.to_string()))?;
        targets.push(RestoreTarget {
            ledger_key_xdr: derive_persistent_key_from_scval(contract_id, scval)?,
            kind: TargetKind::ScVal(scval_xdr),
            archived: false,
        });
    }

    if targets.is_empty() {
        targets.push(RestoreTarget {
            ledger_key_xdr: derive_contract_instance_key(contract_id)?,
            kind: TargetKind::Instance,
            archived: false,
        });
    }

    Ok(targets)
}

fn decode_ledger_key(key_xdr: &str) -> Result<LedgerKey, RestoreCommandError> {
    LedgerKey::from_xdr_base64(key_xdr, Limits::none())
        .map_err(|error| RestoreCommandError::Invalid(format!("invalid LedgerKey XDR: {error}")))
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
    source_account: [u8; 32],
    sequence: i64,
    fee: u32,
    transaction_data: SorobanTransactionData,
) -> Result<TransactionEnvelope, RestoreCommandError> {
    let operation = Operation {
        source_account: None,
        body: OperationBody::RestoreFootprint(stellar_xdr::curr::RestoreFootprintOp {
            ext: ExtensionPoint::V0,
        }),
    };
    let operations = VecM::<Operation, 100>::try_from(vec![operation]).map_err(|error| {
        RestoreCommandError::Invalid(format!("failed to build restore operation: {error}"))
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
) -> Result<SimulationResult, RestoreCommandError> {
    let transaction_xdr = envelope
        .to_xdr_base64(Limits::none())
        .map_err(|error| RestoreCommandError::Invalid(error.to_string()))?;
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "simulateTransaction",
        "params": { "transaction": transaction_xdr }
    });
    let response = client
        .post(rpc_url)
        .json(&request)
        .send()
        .await
        .map_err(|error| {
            RestoreCommandError::Rpc(format!("simulateTransaction request failed: {error}"))
        })?;
    let status = response.status();
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        RestoreCommandError::Rpc(format!("failed to read simulation response: {error}"))
    })? {
        if body.len().saturating_add(chunk.len()) > MAX_RPC_RESPONSE_BYTES {
            return Err(RestoreCommandError::Rpc(format!(
                "simulateTransaction response exceeded the {} byte limit",
                MAX_RPC_RESPONSE_BYTES
            )));
        }
        body.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(RestoreCommandError::Rpc(format!(
            "simulateTransaction returned HTTP {}: {}",
            status,
            String::from_utf8_lossy(&body)
        )));
    }
    let response: RpcResponse<SimulationResult> =
        serde_json::from_slice(&body).map_err(|error| {
            RestoreCommandError::Rpc(format!(
                "invalid simulateTransaction JSON response: {error}"
            ))
        })?;
    if let Some(error) = response.error {
        return Err(RestoreCommandError::Rpc(format!(
            "simulateTransaction RPC error {}: {}",
            error.code, error.message
        )));
    }
    response.result.ok_or_else(|| {
        RestoreCommandError::Rpc("simulateTransaction response is missing result".to_string())
    })
}

fn stellar_restore_command(
    contract_id: &str,
    target: &RestoreTarget,
    network: &str,
    source_account: Option<&str>,
) -> String {
    let mut parts = vec![
        "stellar".to_string(),
        "contract".to_string(),
        "restore".to_string(),
        "--id".to_string(),
        contract_id.to_string(),
    ];
    match &target.kind {
        TargetKind::Instance => {}
        TargetKind::Symbol(symbol) => {
            parts.push("--key".to_string());
            parts.push(symbol.clone());
            parts.push("--durability".to_string());
            parts.push("persistent".to_string());
        }
        TargetKind::ScVal(scval_xdr) => {
            parts.push("--key-xdr".to_string());
            parts.push(scval_xdr.clone());
            parts.push("--durability".to_string());
            parts.push("persistent".to_string());
        }
    }
    parts.push("--network".to_string());
    parts.push(network.to_string());
    if let Some(source) = source_account {
        parts.push("--source-account".to_string());
        parts.push(source.to_string());
    }
    parts.push("--build-only".to_string());
    parts.join(" ")
}

fn render_text(output: &RestoreOutput) -> String {
    let mut text = String::new();
    text.push_str("State Restoration Plan\n");
    text.push_str("======================\n");
    text.push_str(&format!("Contract:        {}\n", output.contract_id));
    text.push_str(&format!("Network:         {}\n", output.network));
    text.push_str(&format!("Latest ledger:   {}\n", output.latest_ledger));
    text.push_str(&format!("Source account:  {}\n", output.source_account));
    text.push_str(&format!(
        "Archived keys:   {}\n",
        output.archived_keys.len()
    ));
    text.push_str(&format!(
        "Resource fee:    {} stroops\n",
        output.resource_fee_stroops
    ));
    text.push_str(&format!(
        "Base fee:        {} stroops\n",
        output.base_fee_stroops
    ));
    text.push_str(&format!(
        "Total fee:       {} stroops\n",
        output.total_fee_stroops
    ));
    text.push_str("Signed:          false\n\n");
    text.push_str("Archived ledger key XDRs:\n");
    for key in &output.archived_keys {
        text.push_str(&format!("  {}\n", key));
    }
    text.push_str("\nFootprint XDR (base64):\n");
    text.push_str(&format!("{}\n\n", output.footprint_xdr_base64));
    text.push_str("Unsigned transaction XDR (base64):\n");
    text.push_str(&format!("{}\n\n", output.transaction_xdr_base64));
    text.push_str("Ready-to-run Stellar CLI commands (add your own signing step):\n");
    for command in &output.stellar_commands {
        text.push_str(&format!("  {}\n", command));
    }
    text
}
