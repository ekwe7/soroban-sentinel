//! `sentinel explain`: decode a base64 `LedgerEntry`, `LedgerEntryData`, or `LedgerKey`
//! XDR value and describe it for operators.

use std::io::Read;

use sentinel_core::adapters::{safe_xdr_limits, summarize_scval};
use serde::Serialize;
use stellar_xdr::curr::{
    ContractDataDurability, ContractExecutable, LedgerEntry, LedgerEntryData, LedgerKey, Limits,
    ReadXdr, ScAddress, ScVal, WriteXdr,
};

use crate::args::ExplainArgs;
use crate::commands::{EXIT_INVALID_ARGS, EXIT_OK};

/// Upper bound on piped input, well above the size of any Soroban ledger entry.
const MAX_STDIN_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Default, Serialize)]
struct Explanation {
    /// `LedgerEntry`, `LedgerEntryData`, or `LedgerKey`.
    xdr_type: &'static str,
    /// Ledger entry kind, e.g. `ContractData`, `ContractCode`, `Account`.
    entry_kind: String,
    /// Size of the decoded XDR in bytes.
    xdr_byte_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_modified_ledger: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contract_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    durability: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wasm_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_storage_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_size: Option<usize>,
}

pub fn run_explain(args: ExplainArgs) -> i32 {
    let input = if args.stdin {
        let mut buffer = String::new();
        if let Err(error) = std::io::stdin()
            .take(MAX_STDIN_BYTES + 1)
            .read_to_string(&mut buffer)
        {
            return invalid(format!("failed to read stdin: {error}"));
        }
        if buffer.len() as u64 > MAX_STDIN_BYTES {
            return invalid(format!("stdin input exceeds {MAX_STDIN_BYTES} bytes"));
        }
        buffer
    } else {
        match args.xdr {
            Some(xdr) => xdr,
            None => return invalid("provide a base64 XDR argument or --stdin"),
        }
    };
    // Base64 never contains whitespace, so strip line wrapping from piped input.
    let xdr: String = input.split_whitespace().collect();
    if xdr.is_empty() {
        return invalid("no XDR input provided");
    }

    let explanation = match explain(&xdr) {
        Some(explanation) => explanation,
        None => {
            return invalid(
                "input is not a valid base64 LedgerEntry, LedgerEntryData, or LedgerKey XDR",
            )
        }
    };
    if args.format == "json" {
        match serde_json::to_string_pretty(&explanation) {
            Ok(json) => println!("{json}"),
            Err(error) => return invalid(error),
        }
    } else {
        print_text(&explanation);
    }
    EXIT_OK
}

/// Try each supported XDR type in turn; decoding must consume the whole input.
fn explain(xdr: &str) -> Option<Explanation> {
    let limits = safe_xdr_limits();
    if let Ok(entry) = LedgerEntry::from_xdr_base64(xdr, limits.clone()) {
        let mut explanation = describe_data(&entry.data);
        explanation.xdr_type = "LedgerEntry";
        explanation.last_modified_ledger = Some(entry.last_modified_ledger_seq);
        explanation.xdr_byte_size = xdr_size(&entry)?;
        return Some(explanation);
    }
    if let Ok(data) = LedgerEntryData::from_xdr_base64(xdr, limits.clone()) {
        let mut explanation = describe_data(&data);
        explanation.xdr_type = "LedgerEntryData";
        explanation.xdr_byte_size = xdr_size(&data)?;
        return Some(explanation);
    }
    let key = LedgerKey::from_xdr_base64(xdr, limits).ok()?;
    let mut explanation = describe_key(&key);
    explanation.xdr_byte_size = xdr_size(&key)?;
    Some(explanation)
}

/// Serialized size of a value that has just been decoded from XDR.
fn xdr_size(value: &impl WriteXdr) -> Option<usize> {
    value.to_xdr(Limits::none()).ok().map(|bytes| bytes.len())
}

fn describe_data(data: &LedgerEntryData) -> Explanation {
    let mut explanation = Explanation {
        entry_kind: data.name().to_owned(),
        ..Explanation::default()
    };
    match data {
        LedgerEntryData::ContractData(entry) => {
            explanation.contract_id = Some(address(&entry.contract));
            explanation.durability = Some(durability(entry.durability));
            explanation.key = Some(summarize_scval(&entry.key));
            if let ScVal::ContractInstance(instance) = &entry.val {
                match &instance.executable {
                    ContractExecutable::Wasm(hash) => {
                        explanation.executable = Some("wasm".into());
                        explanation.wasm_hash = Some(hex(&hash.0));
                    }
                    ContractExecutable::StellarAsset => {
                        explanation.executable = Some("stellar_asset".into());
                    }
                }
                explanation.instance_storage_count =
                    Some(instance.storage.as_ref().map_or(0, |map| map.0.len()));
            }
        }
        LedgerEntryData::ContractCode(entry) => {
            explanation.wasm_hash = Some(hex(&entry.hash.0));
            explanation.code_size = Some(entry.code.len());
        }
        _ => {}
    }
    explanation
}

fn describe_key(key: &LedgerKey) -> Explanation {
    let mut explanation = Explanation {
        xdr_type: "LedgerKey",
        entry_kind: key.name().to_owned(),
        ..Explanation::default()
    };
    match key {
        LedgerKey::ContractData(data) => {
            explanation.contract_id = Some(address(&data.contract));
            explanation.durability = Some(durability(data.durability));
            explanation.key = Some(summarize_scval(&data.key));
        }
        LedgerKey::ContractCode(code) => explanation.wasm_hash = Some(hex(&code.hash.0)),
        _ => {}
    }
    explanation
}

fn address(address: &ScAddress) -> String {
    summarize_scval(&ScVal::Address(address.clone()))
}

fn durability(durability: ContractDataDurability) -> &'static str {
    match durability {
        ContractDataDurability::Persistent => "persistent",
        ContractDataDurability::Temporary => "temporary",
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn print_text(explanation: &Explanation) {
    println!("XDR type:        {}", explanation.xdr_type);
    println!("Entry kind:      {}", explanation.entry_kind);
    let optional = [
        ("Contract ID", explanation.contract_id.clone()),
        ("Durability", explanation.durability.map(str::to_owned)),
        ("Key", explanation.key.clone()),
        ("Executable", explanation.executable.clone()),
        ("WASM hash", explanation.wasm_hash.clone()),
        (
            "Instance keys",
            explanation.instance_storage_count.map(|n| n.to_string()),
        ),
        (
            "Code size",
            explanation.code_size.map(|n| format!("{n} bytes")),
        ),
        (
            "Last modified",
            explanation.last_modified_ledger.map(|n| n.to_string()),
        ),
    ];
    for (label, value) in optional {
        if let Some(value) = value {
            println!("{:<17}{value}", format!("{label}:"));
        }
    }
    println!("XDR size:        {} bytes", explanation.xdr_byte_size);
}

fn invalid(error: impl std::fmt::Display) -> i32 {
    eprintln!("Error: {error}");
    EXIT_INVALID_ARGS
}
