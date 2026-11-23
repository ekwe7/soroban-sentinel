//! Execution logic for the `sentinel audit-keys` CLI command.

use std::time::Duration;

use sentinel_core::adapters::{parse_contract_id, parse_key_expr, SorobanRpcClient};
use sentinel_core::domain::{
    analyze_storage_entry, AuditEntry, StorageAuditReport, MAX_SOROBAN_ENTRY_BYTES,
};
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};
use sentinel_core::{
    derive_persistent_symbol_key, derive_persistent_xdr_key, derive_temporary_key_from_scval,
    derive_temporary_symbol_key, LedgerReader, SentinelError,
};

use crate::args::AuditKeysArgs;

pub const EXIT_OK: i32 = 0;
pub const EXIT_INVALID_ARGS: i32 = 10;
pub const EXIT_RPC_FAILURE: i32 = 11;

fn resolve_rpc_url(network: &str, override_url: Option<&str>) -> Result<String, String> {
    if let Some(url) = override_url {
        return Ok(url.to_string());
    }
    match network.to_lowercase().as_str() {
        "testnet" => Ok("https://soroban-testnet.stellar.org".to_string()),
        "mainnet" => Ok("https://soroban-rpc.stellar.org".to_string()),
        "futurenet" => Ok("https://rpc-futurenet.stellar.org".to_string()),
        "local" => Ok("http://localhost:8000/soroban/rpc".to_string()),
        _ => Err(format!(
            "Unknown network '{}'. Use --rpc-url to specify a custom endpoint.",
            network
        )),
    }
}

fn render_text(report: &StorageAuditReport) -> String {
    let mut out = format!(
        "Storage Audit: {} | Network: {} | Latest Ledger: {}\n",
        report.contract_id, report.network, report.latest_ledger
    );
    out.push_str("+----------------------+-------------+------------+------------------+----------------------------------------------------------+\n");
    out.push_str("| Identifier           | Durability  | Size Bytes | Remaining Ledgers| Findings                                                 |\n");
    out.push_str("+----------------------+-------------+------------+------------------+----------------------------------------------------------+\n");
    for entry in &report.entries {
        let finding = if entry.findings.is_empty() {
            "OK".to_string()
        } else {
            entry
                .findings
                .iter()
                .map(|f| f.recommendation.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        out.push_str(&format!(
            "| {:<20} | {:<11} | {:>10} | {:>16} | {:<56}|\n",
            entry.identifier, entry.durability, entry.size_bytes, entry.remaining_ledgers, finding
        ));
    }
    out.push_str("+----------------------+-------------+------------+------------------+----------------------------------------------------------+\n");
    for recommendation in &report.recommendations {
        out.push_str(&format!("Recommendation: {}\n", recommendation));
    }
    out.push_str(&format!(
        "Summary: {} entries; {} persistent, {} temporary, {} instance, {} near 64 KiB limit.\n",
        report.total_entries,
        report.persistent_entries,
        report.temporary_entries,
        report.instance_entries,
        report.near_limit_entries
    ));
    out
}

pub async fn run_audit_keys(args: AuditKeysArgs) -> i32 {
    if let Some(timeout) = args.rpc_timeout {
        if !(1..=120).contains(&timeout) {
            eprintln!("Error: --rpc-timeout must be between 1 and 120 seconds");
            return EXIT_INVALID_ARGS;
        }
    }
    if let Some(retries) = args.rpc_max_retries {
        if retries > 10 {
            eprintln!("Error: --rpc-max-retries must be between 0 and 10");
            return EXIT_INVALID_ARGS;
        }
    }

    let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return EXIT_INVALID_ARGS;
        }
    };
    if let Err(e) = sentinel_core::adapters::validate_rpc_url(&rpc_url) {
        eprintln!("Error: {}", e);
        return EXIT_INVALID_ARGS;
    }

    let mut rpc = SorobanRpcClient::new_with_timeout(
        rpc_url,
        Duration::from_secs(args.rpc_timeout.unwrap_or(10)),
    );
    if let Some(retries) = args.rpc_max_retries {
        let mut policy = rpc.retry_policy().clone();
        policy.max_retries = retries;
        rpc = rpc.with_retry_policy(policy);
    }

    let contract_id = match parse_contract_id(&args.contract_id) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return EXIT_INVALID_ARGS;
        }
    };

    let mut persistent_descriptors: Vec<(String, String)> = Vec::new();
    for sym in &args.key {
        if let Ok(key) = derive_persistent_symbol_key(&contract_id, sym) {
            persistent_descriptors.push((format!("persistent:{}", sym), key));
        }
    }
    for xdr in &args.key_xdr {
        match derive_persistent_xdr_key(&contract_id, xdr) {
            Ok(key) => persistent_descriptors.push((format!("persistent_xdr:{}", xdr), key)),
            Err(e) => {
                eprintln!("Error: {}", e);
                return EXIT_INVALID_ARGS;
            }
        }
    }
    for expr in &args.key_expr {
        match parse_key_expr(expr)
            .and_then(|v| sentinel_core::derive_persistent_key_from_scval(&contract_id, v))
        {
            Ok(key) => persistent_descriptors.push((format!("persistent:{}", expr), key)),
            Err(e) => {
                eprintln!("Error: {}", e);
                return EXIT_INVALID_ARGS;
            }
        }
    }

    let request = ScanRequest::new(&args.contract_id)
        .with_network(&args.network)
        .with_persistent_symbols(args.key.clone())
        .with_persistent_xdr_keys(args.key_xdr.clone())
        .with_persistent_key_exprs(args.key_expr.clone());
    let usecase = ScanContractUseCase::new(rpc);
    let scan = match usecase.execute(request).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return match e {
                SentinelError::InvalidContractAddress(_)
                | SentinelError::InvalidXdrKey(_)
                | SentinelError::InvalidRpcUrl(_) => EXIT_INVALID_ARGS,
                _ => EXIT_RPC_FAILURE,
            };
        }
    };

    let mut entries: Vec<AuditEntry> = scan
        .entries
        .iter()
        .map(|e| {
            analyze_storage_entry(
                e.key_identifier.clone(),
                &e.durability.to_string(),
                e.byte_size,
                e.live_until_ledger,
                e.remaining_ledgers,
            )
        })
        .collect();

    // Temporary keys are explicit because arbitrary temporary key enumeration is not supported by Soroban RPC.
    for sym in &args.temp_key {
        if let Ok(key) = derive_temporary_symbol_key(&contract_id, sym) {
            let found = usecase
                .reader()
                .get_ledger_entries(std::slice::from_ref(&key))
                .await;
            if let Ok(raw) = found {
                if let Some(raw_entry) = raw.first() {
                    let remaining =
                        raw_entry.live_until_ledger_seq as i64 - scan.latest_ledger as i64;
                    entries.push(analyze_storage_entry(
                        format!("temporary:{}", sym),
                        "temporary",
                        raw_entry.entry_xdr_base64.len(),
                        raw_entry.live_until_ledger_seq,
                        remaining,
                    ));
                }
            }
        }
    }
    for expr in &args.temp_key_expr {
        if let Ok(scval) = parse_key_expr(expr) {
            if let Ok(key) = derive_temporary_key_from_scval(&contract_id, scval) {
                if let Ok(raw) = usecase.reader().get_ledger_entries(&[key]).await {
                    if let Some(raw_entry) = raw.first() {
                        let remaining =
                            raw_entry.live_until_ledger_seq as i64 - scan.latest_ledger as i64;
                        entries.push(analyze_storage_entry(
                            format!("temporary:{}", expr),
                            "temporary",
                            raw_entry.entry_xdr_base64.len(),
                            raw_entry.live_until_ledger_seq,
                            remaining,
                        ));
                    }
                }
            } else {
                eprintln!("Error: invalid temporary key expression '{}'", expr);
                return EXIT_INVALID_ARGS;
            }
        }
    }

    let persistent_entries = entries
        .iter()
        .filter(|e| e.durability.eq_ignore_ascii_case("persistent"))
        .count();
    let temporary_entries = entries
        .iter()
        .filter(|e| e.durability.eq_ignore_ascii_case("temporary"))
        .count();
    let instance_entries = entries
        .iter()
        .filter(|e| e.durability.eq_ignore_ascii_case("instance"))
        .count();
    let code_entries = entries
        .iter()
        .filter(|e| e.durability.eq_ignore_ascii_case("code"))
        .count();
    let near_limit_entries = entries
        .iter()
        .filter(|e| e.size_bytes >= (MAX_SOROBAN_ENTRY_BYTES * 80) / 100)
        .count();
    let persistent_bytes: usize = entries
        .iter()
        .filter(|e| e.durability.eq_ignore_ascii_case("persistent"))
        .map(|e| e.size_bytes)
        .sum();

    let mut recommendations = Vec::new();
    if persistent_entries > 0 {
        recommendations.push(format!("Review {} persistent entry/entries; move recreateable or ephemeral values to Temporary storage.", persistent_entries));
    }
    if near_limit_entries > 0 {
        recommendations.push(format!(
            "{} entry/entries are near or above the 64 KiB limit; split collection data.",
            near_limit_entries
        ));
    }

    let report = StorageAuditReport {
        contract_id: args.contract_id,
        network: args.network,
        latest_ledger: scan.latest_ledger,
        total_entries: entries.len(),
        persistent_entries,
        temporary_entries,
        instance_entries,
        code_entries,
        near_limit_entries,
        estimated_annual_rent_savings_stroops: sentinel_core::estimate_annual_rent_savings(
            persistent_bytes,
        ),
        entries,
        recommendations,
    };

    match args.format.to_lowercase().as_str() {
        "text" => {
            println!("{}", render_text(&report));
            EXIT_OK
        }
        "json" => match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                println!("{}", json);
                EXIT_OK
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                EXIT_INVALID_ARGS
            }
        },
        _ => {
            eprintln!("Error: Unsupported format. Supported formats: text, json");
            EXIT_INVALID_ARGS
        }
    }
}
