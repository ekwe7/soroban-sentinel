//! Storage-key changes between two saved scan reports.

use std::collections::{BTreeMap, HashMap};
use std::fs;

use sentinel_core::{
    derive_contract_code_key, derive_contract_instance_key, derive_persistent_key_from_scval,
    parse_contract_id, parse_key_expr,
};
use serde::Serialize;
use stellar_xdr::curr::{ReadXdr, ScVal};

use crate::args::KeyDiffArgs;
use crate::commands::{EXIT_INVALID_ARGS, EXIT_OK};
use crate::formatters::{JsonEntryReport, JsonTTLReport};

#[derive(Debug, Serialize)]
struct KeyChange {
    /// Canonical XDR identity. Absent only for explicitly unresolved entries.
    canonical_key_xdr: Option<String>,
    identifier: String,
    change: &'static str,
    before: Option<KeyState>,
    after: Option<KeyState>,
}

#[derive(Debug, Clone, Serialize)]
struct KeyState {
    identifier: String,
    durability: String,
    size_bytes: usize,
}

#[derive(Debug, Serialize)]
struct UnresolvedKey {
    side: &'static str,
    identifier: String,
    reason: String,
    state: KeyState,
}

#[derive(Debug, Serialize)]
struct KeyDiff {
    changes: Vec<KeyChange>,
    unresolved: Vec<UnresolvedKey>,
    summary: BTreeMap<&'static str, usize>,
}

type KeyIndex = HashMap<String, KeyState>;

pub fn run_key_diff(args: KeyDiffArgs) -> i32 {
    let before = match load(&args.before) {
        Ok(value) => value,
        Err(error) => return invalid(error),
    };
    let after = match load(&args.after) {
        Ok(value) => value,
        Err(error) => return invalid(error),
    };
    if before.contract_id != after.contract_id {
        return invalid("scan reports must have the same contract ID");
    }

    let old = index(&before, "before");
    let new = index(&after, "after");
    let (old, mut unresolved) = match old {
        Ok(value) => value,
        Err(error) => return invalid(format!("before report: {error}")),
    };
    let (new, unresolved_after) = match new {
        Ok(value) => value,
        Err(error) => return invalid(format!("after report: {error}")),
    };
    unresolved.extend(unresolved_after);

    let mut changes = Vec::new();
    let mut summary = BTreeMap::from([
        ("Added", 0),
        ("Removed", 0),
        ("ModifiedDurability", 0),
        ("ModifiedSize", 0),
        ("Unchanged", 0),
        ("Unresolved", unresolved.len()),
    ]);
    for (key, current) in &new {
        let previous = old.get(key);
        let kind = match previous {
            None => "Added",
            Some(previous) if previous.durability != current.durability => "ModifiedDurability",
            Some(previous) if previous.size_bytes != current.size_bytes => "ModifiedSize",
            Some(_) => "Unchanged",
        };
        *summary.get_mut(kind).unwrap() += 1;
        changes.push(KeyChange {
            canonical_key_xdr: Some(key.clone()),
            identifier: current.identifier.clone(),
            change: kind,
            before: previous.cloned(),
            after: Some(current.clone()),
        });
    }
    for (key, previous) in &old {
        if !new.contains_key(key) {
            *summary.get_mut("Removed").unwrap() += 1;
            changes.push(KeyChange {
                canonical_key_xdr: Some(key.clone()),
                identifier: previous.identifier.clone(),
                change: "Removed",
                before: Some(previous.clone()),
                after: None,
            });
        }
    }
    for item in &unresolved {
        changes.push(KeyChange {
            canonical_key_xdr: None,
            identifier: item.identifier.clone(),
            change: "Unresolved",
            before: (item.side == "before").then(|| item.state.clone()),
            after: (item.side == "after").then(|| item.state.clone()),
        });
    }

    let diff = KeyDiff {
        changes,
        unresolved,
        summary,
    };
    if args.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(&diff).expect("key diff serializes")
        );
    } else {
        let color = !args.no_color && std::env::var_os("NO_COLOR").is_none();
        println!(
            "{:<20} | {:<48} | {:<12} | {:<12}",
            "CHANGE", "KEY (XDR)", "DURABILITY", "SIZE"
        );
        println!("{}", "-".repeat(101));
        for item in &diff.changes {
            let state = item.after.as_ref().or(item.before.as_ref());
            let key = item.canonical_key_xdr.as_deref().unwrap_or("unresolved");
            let row = format!(
                "{:<20} | {:<48} | {:<12} | {:<12}",
                item.change,
                key,
                state.map(|s| s.durability.as_str()).unwrap_or(""),
                state.map(|s| s.size_bytes.to_string()).unwrap_or_default()
            );
            match (color, item.change) {
                (true, "Added") => println!("\x1b[32m{row}\x1b[0m"),
                (true, "Removed") => println!("\x1b[31m{row}\x1b[0m"),
                _ => println!("{row}"),
            }
        }
        println!("Unresolved entries: {}", diff.unresolved.len());
    }
    EXIT_OK
}

fn index(
    report: &JsonTTLReport,
    side: &'static str,
) -> Result<(KeyIndex, Vec<UnresolvedKey>), String> {
    let contract = parse_contract_id(&report.contract_id)
        .map_err(|error| format!("invalid contract ID: {error}"))?;
    let mut entries = HashMap::with_capacity(report.entries.len());
    let mut unresolved = Vec::new();
    for entry in &report.entries {
        match canonical_key(&contract, report, entry) {
            Ok(key) => {
                let state = KeyState {
                    identifier: entry.identifier.clone(),
                    durability: entry.durability.clone(),
                    size_bytes: entry.size_bytes,
                };
                if entries.insert(key.clone(), state).is_some() {
                    return Err(format!(
                        "multiple entries resolve to the same canonical key XDR ({key})"
                    ));
                }
            }
            Err(reason) => unresolved.push(UnresolvedKey {
                side,
                identifier: entry.identifier.clone(),
                reason,
                state: KeyState {
                    identifier: entry.identifier.clone(),
                    durability: entry.durability.clone(),
                    size_bytes: entry.size_bytes,
                },
            }),
        }
    }
    Ok((entries, unresolved))
}

fn canonical_key(
    contract: &[u8; 32],
    report: &JsonTTLReport,
    entry: &JsonEntryReport,
) -> Result<String, String> {
    if entry.entry_type == "contract_instance" || entry.identifier == "instance" {
        return derive_contract_instance_key(contract).map_err(|error| error.to_string());
    }
    if entry.entry_type == "contract_code" || entry.identifier == "code_wasm" {
        let hash = report
            .contract_code_hash
            .as_deref()
            .ok_or_else(|| "contract code hash is missing from this scan report".to_string())?;
        let bytes = decode_hash(hash)?;
        return derive_contract_code_key(&bytes).map_err(|error| error.to_string());
    }
    if let Some(raw) = entry.identifier.strip_prefix("persistent_xdr:") {
        // The report stores ScVal XDR. Rebuild a LedgerKey with a fixed
        // durability so the identity remains stable if durability changes.
        let key = ScVal::from_xdr_base64(raw, sentinel_core::safe_xdr_limits())
            .map_err(|error| format!("invalid key XDR: {error}"))?;
        return derive_persistent_key_from_scval(contract, key)
            .map_err(|error| format!("could not serialize canonical key XDR: {error}"));
    }
    if let Some(expression) = entry.identifier.strip_prefix("persistent:") {
        let key = parse_key_expr(expression)
            .or_else(|_| parse_key_expr(&format!("sym:{expression}")))
            .map_err(|error| format!("cannot reconstruct key XDR: {error}"))?;
        return derive_persistent_key_from_scval(contract, key)
            .map_err(|error| format!("could not serialize canonical key XDR: {error}"));
    }
    Err("entry identifier does not contain reconstructable key data".to_string())
}

fn decode_hash(hash: &str) -> Result<[u8; 32], String> {
    if hash.len() != 64 {
        return Err("contract code hash must contain 64 hexadecimal characters".into());
    }
    let mut bytes = [0u8; 32];
    for (index, chunk) in hash.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let pair = std::str::from_utf8(chunk).map_err(|error| error.to_string())?;
        bytes[index] = u8::from_str_radix(pair, 16)
            .map_err(|_| "contract code hash contains invalid hexadecimal".to_string())?;
    }
    Ok(bytes)
}

fn load(path: &std::path::Path) -> Result<JsonTTLReport, String> {
    let data = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str::<JsonTTLReport>(&data)
        .map_err(|error| format!("invalid scan report '{}': {error}", path.display()))
}

fn invalid(error: impl std::fmt::Display) -> i32 {
    eprintln!("Error: {error}");
    EXIT_INVALID_ARGS
}
