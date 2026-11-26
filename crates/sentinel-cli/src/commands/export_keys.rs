//! `sentinel export-keys`: turn the monitored keys in a saved JSON TTL report into a
//! reusable `sentinel.toml` snippet or a JSON key manifest.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use sentinel_core::adapters::{parse_key_expr, safe_xdr_limits};
use serde::Serialize;
use stellar_xdr::curr::{Limits, ReadXdr, ScVal, WriteXdr};

use crate::args::ExportKeysArgs;
use crate::commands::{EXIT_INVALID_ARGS, EXIT_OK};
use crate::formatters::JsonTTLReport;

/// Durability tiers that hold user-monitored keys; `instance` and `code` are built in.
const PERSISTENT: &str = "persistent";
const TEMPORARY: &str = "temporary";

/// One normalized key, identified by its canonical `ScVal` XDR.
#[derive(Debug, Serialize)]
struct ExportedKey {
    durability: &'static str,
    /// Set when the key is a plain symbol, so it can be written as `keys = [...]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
    key_xdr: String,
}

#[derive(Debug, Serialize)]
struct KeyManifest {
    contract_id: String,
    network: String,
    keys: Vec<ExportedKey>,
}

pub fn run_export_keys(args: ExportKeysArgs) -> i32 {
    let report = match load(&args.input) {
        Ok(report) => report,
        Err(error) => return invalid(error),
    };
    let keys = match normalize_keys(&report) {
        Ok(keys) => keys,
        Err(error) => return invalid(format!("{}: {error}", args.input.display())),
    };
    let manifest = KeyManifest {
        contract_id: report.contract_id,
        network: report.network,
        keys,
    };
    let rendered = if args.format == "json" {
        serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())
    } else {
        render_toml(&manifest)
    };
    match rendered {
        Ok(output) => {
            println!("{}", output.trim_end());
            EXIT_OK
        }
        Err(error) => invalid(error),
    }
}

fn load(path: &Path) -> Result<JsonTTLReport, String> {
    let contents =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("invalid TTL report '{}': {error}", path.display()))
}

/// Classify each report entry, drop built-in entries, and deduplicate by
/// durability and canonical key XDR. Output is sorted: persistent before
/// temporary, then symbols (by name) before other keys (by XDR).
fn normalize_keys(report: &JsonTTLReport) -> Result<Vec<ExportedKey>, String> {
    // Symbols map one-to-one onto their XDR, so (tier, is_raw_xdr, name-or-xdr)
    // is both a dedup key and the deterministic output order.
    let mut unique: BTreeMap<(u8, bool, String), ExportedKey> = BTreeMap::new();
    for entry in &report.entries {
        let Some((durability, scval)) = classify(&entry.identifier)? else {
            continue;
        };
        let key_xdr = scval
            .to_xdr_base64(Limits::none())
            .map_err(|error| format!("entry '{}': {error}", entry.identifier))?;
        let symbol = match &scval {
            ScVal::Symbol(symbol) => Some(symbol.0.to_string()),
            _ => None,
        };
        let tier = u8::from(durability == TEMPORARY);
        let sort_key = symbol.clone().unwrap_or_else(|| key_xdr.clone());
        unique
            .entry((tier, symbol.is_none(), sort_key))
            .or_insert(ExportedKey {
                durability,
                symbol,
                key_xdr,
            });
    }
    Ok(unique.into_values().collect())
}

/// Map a report identifier to its durability and key `ScVal`, or `None` for built-in entries.
fn classify(identifier: &str) -> Result<Option<(&'static str, ScVal)>, String> {
    if identifier == "instance" || identifier == "code_wasm" {
        return Ok(None);
    }
    let parsed = if let Some(xdr) = identifier.strip_prefix("persistent_xdr:") {
        ScVal::from_xdr_base64(xdr, safe_xdr_limits())
            .map(|scval| (PERSISTENT, scval))
            .map_err(|error| error.to_string())
    } else if let Some(key) = identifier.strip_prefix("persistent:") {
        parse_readable_key(key).map(|scval| (PERSISTENT, scval))
    } else if let Some(key) = identifier.strip_prefix("temporary:") {
        parse_readable_key(key).map(|scval| (TEMPORARY, scval))
    } else {
        return Err(format!("unrecognized entry identifier '{identifier}'"));
    };
    parsed
        .map(Some)
        .map_err(|error| format!("entry '{identifier}': {error}"))
}

/// Readable identifiers hold either a bare symbol (`--key`) or a typed expression (`--key-expr`).
fn parse_readable_key(key: &str) -> Result<ScVal, String> {
    let expression = if key.contains(':') || key.starts_with("vec[") {
        key.to_owned()
    } else {
        format!("sym:{key}")
    };
    parse_key_expr(&expression).map_err(|error| error.to_string())
}

/// Render a `[[contracts]]` block accepted by the `sentinel.toml` manifest loader.
fn render_toml(manifest: &KeyManifest) -> Result<String, String> {
    #[derive(Serialize)]
    struct Contract<'a> {
        id: &'a str,
        keys: Vec<&'a str>,
        key_xdr: Vec<&'a str>,
    }
    #[derive(Serialize)]
    struct Snippet<'a> {
        contracts: [Contract<'a>; 1],
    }

    let persistent = manifest.keys.iter().filter(|k| k.durability == PERSISTENT);
    let contract = Contract {
        id: &manifest.contract_id,
        keys: persistent
            .clone()
            .filter_map(|k| k.symbol.as_deref())
            .collect(),
        key_xdr: persistent
            .filter(|k| k.symbol.is_none())
            .map(|k| k.key_xdr.as_str())
            .collect(),
    };
    let temporary = manifest
        .keys
        .iter()
        .filter(|k| k.durability == TEMPORARY)
        .count();
    if temporary > 0 {
        eprintln!(
            "Note: {temporary} temporary key(s) omitted; sentinel.toml only lists persistent keys (use --format json to export them)"
        );
    }
    toml::to_string(&Snippet {
        contracts: [contract],
    })
    .map_err(|error| error.to_string())
}

fn invalid(error: impl std::fmt::Display) -> i32 {
    eprintln!("Error: {error}");
    EXIT_INVALID_ARGS
}
