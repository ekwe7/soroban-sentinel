//! `sentinel benchmark-rpc` CLI command for measuring Soroban RPC endpoint performance.

use std::time::{Duration, Instant};

use serde::Serialize;

use sentinel_core::adapters::{
    derive_contract_instance_key, validate_rpc_url, MAX_KEYS_PER_REQUEST,
};
use sentinel_core::LedgerReader;

use crate::args::BenchmarkArgs;

/// Exit code: Benchmark completed successfully.
pub const EXIT_BENCHMARK_OK: i32 = 0;
/// Exit code: Invalid arguments or RPC failure.
pub const EXIT_BENCHMARK_ERROR: i32 = 11;

/// Default number of concurrent RPC requests.
pub const DEFAULT_CONCURRENCY: usize = 1;
/// Default benchmark duration in seconds.
pub const DEFAULT_DURATION_SECS: u64 = 30;

// === Structs ===

/// JSON-serializable benchmark results.
#[derive(Debug, Serialize)]
struct BenchmarkResults {
    /// RPC endpoint URL tested.
    rpc_url: String,
    /// Target network (derived from URL or "unknown").
    network: String,
    /// Duration of benchmark in seconds.
    duration_seconds: f64,
    /// Number of concurrent requests.
    concurrency: usize,
    /// RPC method benchmarked.
    method: String,
    /// Total number of requests made.
    total_requests: usize,
    /// Requests per second.
    requests_per_second: f64,
    /// Latency percentile results.
    percentiles: Percentiles,
}

/// Latency percentile results.
#[derive(Debug, Serialize)]
struct Percentiles {
    /// 50th percentile (median) in ms.
    p50: f64,
    /// 95th percentile in ms.
    p95: f64,
    /// 99th percentile in ms.
    p99: f64,
}

// === Public API ===

/// Executes the benchmark-rpc command with the given arguments.
/// Returns the UNIX exit code suitable for CI/CD gates and automation.
pub async fn run_benchmark(args: BenchmarkArgs) -> i32 {
    // Validate RPC URL
    if let Err(e) = validate_rpc_url(&args.rpc_url) {
        eprintln!("Error: Invalid RPC URL: {}", e);
        return EXIT_BENCHMARK_ERROR;
    }

    let rpc_url = &args.rpc_url;
    let concurrency = args.concurrency.max(1);
    let duration_secs = args.duration_secs.max(1);

    // Determine network from URL if possible, otherwise "unknown"
    let network = if args.rpc_url.contains("testnet") {
        "testnet".to_string()
    } else if args.rpc_url.contains("mainnet") {
        "mainnet".to_string()
    } else if args.rpc_url.contains("futurenet") {
        "futurenet".to_string()
    } else {
        "unknown".to_string()
    };

    // Create RPC client
    let rpc_timeout = Duration::from_secs(10);
    let mut rpc_client =
        sentinel_core::adapters::SorobanRpcClient::new_with_timeout(rpc_url, rpc_timeout);

    // === Benchmark 1: getLatestLedger (RPC baseline) ===
    eprintln!("\n=== Benchmarking getLatestLedger (RPC baseline) ===");

    let baseline_result = benchmark_rpc_requests(&mut rpc_client, concurrency, duration_secs).await;

    // === Benchmark 2: getLedgerEntries single-key (1 key per request) ===
    eprintln!("\n=== Benchmarking getLedgerEntries single-key (1 key) ===");

    // Derive a deterministic contract instance key for single-key benchmark
    let contract_bytes = [0xCDu8; 32];
    let instance_key_xdr =
        derive_contract_instance_key(&contract_bytes).unwrap_or_else(|_| "ABC".to_string());

    let single_key_result = benchmark_ledger_entries_requests(
        &mut rpc_client,
        concurrency,
        duration_secs,
        std::slice::from_ref(&instance_key_xdr),
    )
    .await;

    // === Benchmark 3: getLedgerEntries chunked batch (200 keys per request) ===
    eprintln!("\n=== Benchmarking getLedgerEntries chunked batch (200 keys) ===");

    // Create 200 deterministic keys for batch benchmark
    let mut batch_keys = Vec::with_capacity(MAX_KEYS_PER_REQUEST);
    for i in 0..MAX_KEYS_PER_REQUEST {
        let key = format!("BASE64_KEY_{}", i);
        batch_keys.push(key);
    }

    let batch_result =
        benchmark_ledger_entries_requests(&mut rpc_client, concurrency, duration_secs, &batch_keys)
            .await;

    // === Combine results and output ===
    // Collect all latencies from all three methods
    let all_latencies: Vec<f64> = baseline_result
        .latencies
        .into_iter()
        .chain(single_key_result.latencies)
        .chain(batch_result.latencies)
        .collect();

    let all_percentiles = calculate_percentiles(&all_latencies);

    let total_requests = baseline_result.total + single_key_result.total + batch_result.total;
    let rps = if duration_secs > 0 {
        total_requests as f64 / duration_secs as f64
    } else {
        0.0
    };

    let combined_results = BenchmarkResults {
        rpc_url: rpc_url.clone(),
        network,
        duration_seconds: duration_secs as f64,
        concurrency,
        method: "getLatestLedger + getLedgerEntries (single + batch)".to_string(),
        total_requests,
        requests_per_second: rps,
        percentiles: all_percentiles,
    };

    // Print human-readable report to stderr (for both text and JSON CLI output)
    print_benchmark_report(&combined_results);
    eprintln!();

    // Print machine-readable JSON to stdout when requested
    if args.format == "json" {
        let json_str = serde_json::to_string_pretty(&combined_results)
            .unwrap_or_else(|_| r#"{"error":"Failed to serialize benchmark results"}"#.to_string());
        println!("{}", json_str);
    } else {
        // Text format: empty line on stdout (for CLI compatibility with previous behavior)
        println!();
    }

    EXIT_BENCHMARK_OK
}

// === Internal Functions ===

/// Spawns `concurrency` concurrent tasks that each issue RPC requests for the given `duration_secs`,
/// collecting latency measurements per request.
async fn benchmark_rpc_requests(
    rpc_client: &mut sentinel_core::adapters::SorobanRpcClient,
    concurrency: usize,
    duration_secs: u64,
) -> BenchmarkSubResult {
    let mut handles = Vec::new();
    let start = Instant::now();

    // Launch concurrent benchmark tasks by cloning the client for each task
    for _ in 0..concurrency {
        let client = rpc_client.clone();
        let handle = tokio::spawn(async move {
            let mut local_latencies = Vec::new();
            let end_time = start + Duration::from_secs(duration_secs);

            while Instant::now() < end_time {
                let instant = Instant::now();
                let _result = client.get_latest_ledger().await;
                let latency = instant.elapsed().as_millis() as f64;
                local_latencies.push(latency);
            }
            local_latencies
        });
        handles.push(handle);
    }

    // Await all tasks and collect latencies
    let mut all_latencies = Vec::new();
    for handle in handles {
        if let Ok(local_latencies) = handle.await {
            all_latencies.extend(local_latencies);
        }
    }

    let total = all_latencies.len();
    let elapsed = start.elapsed().as_secs_f64();
    let rps = if elapsed > 0.0 {
        total as f64 / elapsed
    } else {
        0.0
    };

    BenchmarkSubResult {
        total,
        latencies: all_latencies,
        rps,
    }
}

/// Benchmarks `getLedgerEntries` requests with a given set of keys.
/// Spawns `concurrency` concurrent tasks that each issue batch RPC requests for the given `duration_secs`.
async fn benchmark_ledger_entries_requests(
    rpc_client: &mut sentinel_core::adapters::SorobanRpcClient,
    concurrency: usize,
    duration_secs: u64,
    keys: &[String],
) -> BenchmarkSubResult {
    let chunk_size = MAX_KEYS_PER_REQUEST.min(keys.len());
    let chunked_keys: Vec<Vec<String>> = if keys.is_empty() {
        vec![Vec::new()]
    } else {
        // Chunk keys into batches of MAX_KEYS_PER_REQUEST
        let mut chunks = Vec::new();
        let mut i = 0;
        while i < keys.len() {
            let end = (i + chunk_size).min(keys.len());
            chunks.push(keys[i..end].to_vec());
            i = end;
        }
        chunks
    };

    let mut handles = Vec::new();
    let mut all_latencies = Vec::new();
    let start = Instant::now();

    // Launch concurrent benchmark tasks by cloning the client and key chunks for each task
    for _ in 0..concurrency {
        let client = rpc_client.clone();
        let chunk_keys = chunked_keys.clone();
        let handle = tokio::spawn(async move {
            let mut local_latencies = Vec::new();
            let end_time = start + Duration::from_secs(duration_secs);

            // Use cloned key chunks
            let key_chunks = chunk_keys.clone();

            while Instant::now() < end_time {
                // Rotate through chunks to avoid always sending the same keys
                let keys_to_send = if key_chunks.len() > 1 {
                    let idx = (local_latencies.len() / key_chunks.len()) % key_chunks.len();
                    key_chunks[idx].clone()
                } else {
                    key_chunks[0].clone()
                };
                let instant = Instant::now();
                let _result = client.get_ledger_entries(&keys_to_send).await;
                let latency = instant.elapsed().as_millis() as f64;
                local_latencies.push(latency);
            }
            local_latencies
        });
        handles.push(handle);
    }

    for handle in handles {
        if let Ok(local_latencies) = handle.await {
            all_latencies.extend(local_latencies);
        }
    }

    let total = all_latencies.len();
    let elapsed = start.elapsed().as_secs_f64();
    let rps = if elapsed > 0.0 {
        total as f64 / elapsed
    } else {
        0.0
    };

    BenchmarkSubResult {
        total,
        latencies: all_latencies,
        rps,
    }
}

/// Calculates percentile metrics (p50, p95, p99) from a list of latency measurements in milliseconds.
fn calculate_percentiles(latencies: &[f64]) -> Percentiles {
    let n = latencies.len();
    if n == 0 {
        return Percentiles {
            p50: 0.0,
            p95: 0.0,
            p99: 0.0,
        };
    }

    let mut sorted = latencies.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Use closure instead of nested fn to capture sorted from the outer scope
    let percentile = |n: usize, p: f64| -> f64 {
        // Linear interpolation method
        let k = (p / 100.0) * (n as f64 - 1.0);
        let lower = k.floor() as usize;
        let upper = k.ceil() as usize;
        if lower == upper {
            sorted[lower]
        } else {
            let d = k - lower as f64;
            sorted[lower] + d * (sorted[upper] - sorted[lower])
        }
    };

    Percentiles {
        p50: percentile(n, 50.0),
        p95: percentile(n, 95.0),
        p99: percentile(n, 99.0),
    }
}

/// Internal struct for benchmark sub-result data.
struct BenchmarkSubResult {
    /// Total number of requests completed.
    total: usize,
    /// Collected latency measurements in milliseconds.
    latencies: Vec<f64>,
    /// Calculated requests per second.
    #[allow(dead_code)]
    rps: f64,
}

// === Output Formatting ===

/// Formats and prints a human-readable benchmark performance report to stdout.
fn print_benchmark_report(results: &BenchmarkResults) {
    let percentiles = &results.percentiles;
    let rps = results.requests_per_second;

    eprintln!("Soroban Sentinel RPC Benchmark Report",);
    eprintln!("=======================================");
    eprintln!("RPC URL: {}", results.rpc_url);
    eprintln!("Network: {}", results.network);
    eprintln!("Duration: {:.1}s", results.duration_seconds);
    eprintln!("Concurrency: {}", results.concurrency);
    eprintln!("Method: {}", results.method);
    eprintln!("Total requests: {}", results.total_requests);
    eprintln!("RPS: {:.2}", rps);
    eprintln!();
    eprintln!("Latency percentiles (ms):");
    eprintln!("  p50: {:.2}", percentiles.p50);
    eprintln!("  p95: {:.2}", percentiles.p95);
    eprintln!("  p99: {:.2}", percentiles.p99);
    eprintln!();
    eprintln!("Performance summary:");
    if rps > 0.0 {
        eprintln!("  Throughput: {:.2} requests/second", rps);
    }
    if percentiles.p50 > 0.0 {
        eprintln!("  Median latency: {:.2}ms", percentiles.p50);
    }
    eprintln!();
}

// === CLI Argument Parsing (for standalone/test use) ===

#[cfg(test)]
impl BenchmarkArgs {
    /// Parses CLI arguments from the program's argv (skipping the program name).
    fn parse_from(args: &[String]) -> Self {
        let mut rpc_url = String::new();
        let mut concurrency = DEFAULT_CONCURRENCY;
        let mut duration_secs = DEFAULT_DURATION_SECS;
        let mut format = String::from("text");

        let mut i = 1; // Skip program name
        if args.get(i).map(String::as_str) == Some("benchmark-rpc") {
            i += 1;
        }
        while i < args.len() {
            let arg = args[i].as_str();

            match arg {
                "--rpc-url" => {
                    i += 1;
                    if i < args.len() {
                        rpc_url = args[i].clone();
                    } else {
                        eprintln!("Error: --rpc-url requires a value");
                        std::process::exit(EXIT_BENCHMARK_ERROR);
                    }
                }
                "--concurrency" => {
                    i += 1;
                    if i < args.len() {
                        concurrency = args[i].parse::<usize>().unwrap_or_else(|_| {
                            eprintln!("Error: --concurrency must be a valid positive integer");
                            std::process::exit(EXIT_BENCHMARK_ERROR);
                        });
                    } else {
                        eprintln!("Error: --concurrency requires a value");
                        std::process::exit(EXIT_BENCHMARK_ERROR);
                    }
                }
                "--duration-secs" => {
                    i += 1;
                    if i < args.len() {
                        duration_secs = args[i].parse::<u64>().unwrap_or_else(|_| {
                            eprintln!("Error: --duration-secs must be a valid positive integer");
                            std::process::exit(EXIT_BENCHMARK_ERROR);
                        });
                    } else {
                        eprintln!("Error: --duration-secs requires a value");
                        std::process::exit(EXIT_BENCHMARK_ERROR);
                    }
                }
                "--format" => {
                    i += 1;
                    if i < args.len() {
                        format = args[i].clone();
                        if format != "text" && format != "json" {
                            eprintln!("Error: --format must be 'text' or 'json'");
                            std::process::exit(EXIT_BENCHMARK_ERROR);
                        }
                    } else {
                        eprintln!("Error: --format requires a value ('text' or 'json')");
                        std::process::exit(EXIT_BENCHMARK_ERROR);
                    }
                }
                _ => {
                    eprintln!("Error: unrecognized argument: {}", arg);
                    std::process::exit(EXIT_BENCHMARK_ERROR);
                }
            }
            i += 1;
        }

        Self {
            rpc_url,
            concurrency,
            duration_secs,
            format,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benchmark_args_defaults() {
        let args = BenchmarkArgs::parse_from(
            &[
                "sentinel",
                "benchmark-rpc",
                "--rpc-url",
                "https://example.com",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>(),
        );
        assert_eq!(args.rpc_url, "https://example.com");
        assert_eq!(args.concurrency, DEFAULT_CONCURRENCY);
        assert_eq!(args.duration_secs, DEFAULT_DURATION_SECS);
        assert_eq!(args.format, "text");
    }

    #[test]
    fn test_benchmark_args_custom_flags() {
        let args = BenchmarkArgs::parse_from(
            &[
                "sentinel",
                "benchmark-rpc",
                "--rpc-url",
                "https://example.com",
                "--concurrency",
                "10",
                "--duration-secs",
                "60",
                "--format",
                "json",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>(),
        );
        assert_eq!(args.rpc_url, "https://example.com");
        assert_eq!(args.concurrency, 10);
        assert_eq!(args.duration_secs, 60);
        assert_eq!(args.format, "json");
    }

    #[test]
    fn test_benchmark_args_missing_rpc_url() {
        let args = BenchmarkArgs::parse_from(
            &["sentinel", "benchmark-rpc"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<String>>(),
        );
        assert!(args.rpc_url.is_empty());
    }

    #[test]
    fn test_calculate_percentiles_empty() {
        let result = calculate_percentiles(&[]);
        assert_eq!(result.p50, 0.0);
        assert_eq!(result.p95, 0.0);
        assert_eq!(result.p99, 0.0);
    }

    #[test]
    fn test_calculate_percentiles_single() {
        let result = calculate_percentiles(&[100.0, 200.0, 300.0, 400.0, 500.0]);
        // p50 = 300.0, p95 = 480.0, p99 = 498.0 (approx)
        assert!((result.p50 - 300.0).abs() < 1.0);
        assert!((result.p95 - 480.0).abs() < 10.0);
        assert!((result.p99 - 498.0).abs() < 10.0);
    }

    #[test]
    fn test_calculate_percentiles_two_elements() {
        let result = calculate_percentiles(&[10.0, 20.0]);
        // p50 = 15.0, p95 = 19.5, p99 = 19.9
        assert!((result.p50 - 15.0).abs() < 0.1);
        assert!((result.p95 - 19.5).abs() < 0.1);
        assert!((result.p99 - 19.9).abs() < 0.1);
    }
}
