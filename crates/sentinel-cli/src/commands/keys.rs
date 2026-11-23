use sentinel_core::adapters::{
    parse_contract_id, parse_key_expr, safe_xdr_limits, summarize_scval,
};
use sentinel_core::error::SentinelError;
use serde::Serialize;
use stellar_xdr::curr::{
    ContractDataDurability, Hash, LedgerKey, LedgerKeyContractData, Limits, ReadXdr, ScAddress,
    ScVal, WriteXdr,
};

use crate::args::{KeyDecodeArgs, KeyEncodeArgs};
use crate::commands::{EXIT_INVALID_ARGS, EXIT_OK};

#[derive(Serialize)]
struct KeyOutput {
    kind: &'static str,
    xdr: String,
    representation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    contract: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    durability: Option<String>,
}

pub fn run_key_encode(args: KeyEncodeArgs) -> i32 {
    let expression = if args.expression.contains(':') || args.expression.starts_with("vec[") {
        args.expression.clone()
    } else {
        format!("sym:{}", args.expression)
    };
    let scval = match parse_key_expr(&expression) {
        Ok(value) => value,
        Err(error) => return invalid(error),
    };
    let representation = summarize_scval(&scval);
    let (kind, xdr, contract, durability) = if let Some(contract) = args.contract {
        let id = match parse_contract_id(&contract) {
            Ok(id) => id,
            Err(error) => return invalid(error),
        };
        let durability_value = match args.durability.as_str() {
            "persistent" => ContractDataDurability::Persistent,
            "temporary" => ContractDataDurability::Temporary,
            _ => {
                return invalid(SentinelError::InvalidXdrKey(
                    "durability must be persistent or temporary".into(),
                ))
            }
        };
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(Hash(id)),
            key: scval,
            durability: durability_value,
        });
        let xdr = match key.to_xdr_base64(Limits::none()) {
            Ok(xdr) => xdr,
            Err(error) => return invalid(error),
        };
        ("LedgerKey", xdr, Some(contract), Some(args.durability))
    } else {
        let xdr = match scval.to_xdr_base64(Limits::none()) {
            Ok(xdr) => xdr,
            Err(error) => return invalid(error),
        };
        ("ScVal", xdr, None, None)
    };
    emit(
        KeyOutput {
            kind,
            xdr,
            representation,
            contract,
            durability,
        },
        &args.format,
    )
}

pub fn run_key_decode(args: KeyDecodeArgs) -> i32 {
    if let Ok(key) = LedgerKey::from_xdr_base64(&args.xdr, safe_xdr_limits()) {
        let representation = match key {
            LedgerKey::ContractData(data) => summarize_scval(&data.key),
            other => format!("{other:?}"),
        };
        return emit(
            KeyOutput {
                kind: "LedgerKey",
                xdr: args.xdr,
                representation,
                contract: None,
                durability: None,
            },
            &args.format,
        );
    }
    match ScVal::from_xdr_base64(&args.xdr, safe_xdr_limits()) {
        Ok(scval) => emit(
            KeyOutput {
                kind: "ScVal",
                xdr: args.xdr,
                representation: summarize_scval(&scval),
                contract: None,
                durability: None,
            },
            &args.format,
        ),
        Err(error) => invalid(format!("invalid LedgerKey or ScVal XDR: {error}")),
    }
}

fn emit(output: KeyOutput, format: &str) -> i32 {
    if format == "json" {
        match serde_json::to_string_pretty(&output) {
            Ok(value) => println!("{value}"),
            Err(error) => return invalid(error),
        }
    } else {
        println!("{}", output.xdr);
        println!("{}: {}", output.kind, output.representation);
    }
    EXIT_OK
}

fn invalid(error: impl std::fmt::Display) -> i32 {
    eprintln!("Error: {error}");
    EXIT_INVALID_ARGS
}
