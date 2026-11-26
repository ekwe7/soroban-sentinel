use std::{collections::HashMap, time::Duration};

use sentinel_core::{RiskLevel, StorageEntry, TTLReport};

use crate::{
    adapters::{
        alert_teams::TeamsAlertDispatcher,
        alert_telegram::{
            SentinelEvent, TelegramAlertDispatcher, ENV_TELEGRAM_BOT_TOKEN, ENV_TELEGRAM_CHAT_ID,
        },
    },
    alerts::{
        datadog::{DatadogAdapter, DatadogSite},
        incident::{dedup_key, IncidentDispatcher, Provider},
    },
    args::WatchArgs,
    commands::{scan::prepare_scan, EXIT_INVALID_ARGS},
    formatters::{render_json, render_text_with_options},
};

/// Initializes an optional [`TelegramAlertDispatcher`] from CLI/environment credentials.
pub fn init_telegram_dispatcher(
    bot_token: Option<String>,
    chat_id: Option<String>,
) -> Result<Option<TelegramAlertDispatcher>, String> {
    match (bot_token, chat_id) {
        (Some(token), Some(chat_id)) => TelegramAlertDispatcher::new(token, chat_id)
            .map(Some)
            .map_err(|error| error.to_string()),
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(format!(
            "both {ENV_TELEGRAM_BOT_TOKEN} and {ENV_TELEGRAM_CHAT_ID} must be set to enable Telegram alerts"
        )),
    }
}

/// Emits and dispatches [`SentinelEvent::RiskThresholdBreached`] alerts to Telegram for
/// every entry that has entered or changed to a non-`Ok` risk level since the previous scan.
///
/// Entries that return to `RiskLevel::Ok` are cleared from `reported` so future
/// regressions trigger a fresh alert. Failed deliveries remain eligible on the next scan.
pub async fn dispatch_telegram_report(
    dispatcher: &TelegramAlertDispatcher,
    report: &TTLReport,
    keys: &[String],
    reported: &mut HashMap<String, RiskLevel>,
) -> Result<usize, String> {
    if report.entries.len() != keys.len() {
        return Err("Report entries and ledger keys differ in count".into());
    }

    let mut dispatched = 0usize;
    let mut errors = Vec::new();

    for (entry, key) in report.entries.iter().zip(keys) {
        let dedup = dedup_key(&report.network, &report.contract_id, key);
        if entry.status == RiskLevel::Ok {
            reported.remove(&dedup);
            continue;
        }
        if reported.get(&dedup).copied() == Some(entry.status) {
            continue;
        }

        let storage_entry = StorageEntry::new(
            &entry.key_identifier,
            entry.entry_type,
            entry.durability,
            entry.byte_size,
            entry.live_until_ledger,
            report.latest_ledger,
        );
        let event = SentinelEvent::risk_threshold_breached(
            &report.contract_id,
            &report.network,
            storage_entry,
            entry.remaining_ledgers,
            entry.status,
        );

        match dispatcher.handle_event(&event).await {
            Ok(true) => {
                reported.insert(dedup, entry.status);
                dispatched += 1;
            }
            Ok(false) => {
                reported.remove(&dedup);
            }
            Err(error) => {
                errors.push(error.to_string());
            }
        }
    }

    if errors.is_empty() {
        Ok(dispatched)
    } else {
        Err(errors.join(", "))
    }
}

pub async fn run_watch(args: WatchArgs) -> i32 {
    if args.scan.baseline.is_some() || args.scan.fail_on_regression {
        eprintln!("Error: baseline comparison is available with sentinel scan");
        return EXIT_INVALID_ARGS;
    }
    let format = args.scan.format.to_ascii_lowercase();
    if !matches!(format.as_str(), "json" | "text") {
        eprintln!("Error: --format must be text or json");
        return EXIT_INVALID_ARGS;
    }
    if let Err(error) = sentinel_core::parse_contract_id(&args.scan.contract_id) {
        eprintln!("Error: {error}");
        return EXIT_INVALID_ARGS;
    }
    let (usecase, request) = match prepare_scan(&args.scan) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };

    let mut teams = if let Some(webhook_url) = args.alert_teams.as_deref() {
        match TeamsAlertDispatcher::new(webhook_url) {
            Ok(dispatcher) => Some(dispatcher),
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        }
    } else {
        None
    };

    let providers = [
        args.pagerduty_routing_key
            .map(|routing_key| Provider::PagerDuty {
                endpoint: "https://events.pagerduty.com/v2/enqueue"
                    .parse()
                    .expect("valid PagerDuty URL"),
                routing_key,
            }),
        args.opsgenie_api_key.map(|api_key| Provider::Opsgenie {
            endpoint: "https://api.opsgenie.com/v2/alerts"
                .parse()
                .expect("valid Opsgenie URL"),
            api_key,
        }),
    ];
    let mut dispatchers: Vec<_> = match providers
        .into_iter()
        .flatten()
        .map(IncidentDispatcher::new)
        .collect()
    {
        Ok(value) => value,
        Err(_) => {
            eprintln!("Error: could not initialize incident HTTP client");
            return EXIT_INVALID_ARGS;
        }
    };

    // Build the optional Datadog adapter when credentials are provided.
    let mut datadog: Option<DatadogAdapter> = if let Some(api_key) = args.datadog_api_key {
        let site = match DatadogSite::parse(&args.datadog_site) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: {e}");
                return EXIT_INVALID_ARGS;
            }
        };
        match DatadogAdapter::new(api_key, &site) {
            Ok(adapter) => Some(adapter),
            Err(_) => {
                eprintln!("Error: could not initialize Datadog HTTP client");
                return EXIT_INVALID_ARGS;
            }
        }
    } else {
        None
    };

    let telegram_dispatcher =
        match init_telegram_dispatcher(args.telegram_bot_token, args.telegram_chat_id) {
            Ok(dispatcher) => dispatcher,
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        };
    let mut telegram_reported: HashMap<String, RiskLevel> = HashMap::new();

    loop {
        match usecase.execute_with_keys(request.clone()).await {
            Ok((report, keys)) => {
                let output = if format == "json" {
                    render_json(&report)
                } else {
                    Ok(render_text_with_options(&report, args.scan.no_color))
                };
                match output {
                    Ok(output) => println!("{output}"),
                    Err(error) => eprintln!("Watch report failed: {error}"),
                }
                for dispatcher in &mut dispatchers {
                    if let Err(error) = dispatcher.observe(&report, &keys).await {
                        eprintln!("Watch notification failed: {error}");
                    }
                }
                if let Some(dd) = datadog.as_mut() {
                    if let Err(error) = dd.observe(&report).await {
                        eprintln!("Datadog notification failed: {error}");
                    }
                }
                if let Some(dispatcher) = &telegram_dispatcher {
                    if let Err(error) =
                        dispatch_telegram_report(dispatcher, &report, &keys, &mut telegram_reported)
                            .await
                    {
                        eprintln!("Watch Telegram notification failed: {error}");
                    }
                }
                if let Some(teams) = teams.as_mut() {
                    if let Err(error) = teams.observe(&report).await {
                        eprintln!("Teams notification failed: {error}");
                    }
                }
            }
            Err(
                error @ (sentinel_core::SentinelError::InvalidXdrKey(_)
                | sentinel_core::SentinelError::InvalidContractAddress(_)),
            ) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
            Err(error) => eprintln!("Watch scan failed: {error}"),
        }
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }
}
