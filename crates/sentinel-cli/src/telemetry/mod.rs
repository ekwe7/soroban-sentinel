//! OpenTelemetry OTLP tracing and metrics wiring for the CLI.
//!
//! Telemetry is strictly optional. When no OTLP endpoint is configured the
//! guard is inert and all instrumentation falls through to `tracing`'s no-op
//! dispatcher. When an endpoint is configured but the collector is unavailable,
//! initialization and export failures are reported on stderr and the command
//! continues with normal output and exit codes.

use std::env;
use std::time::Duration;

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::TracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use sentinel_core::usecases::TTLReport;

/// Environment variable used when `--otlp-endpoint` is not supplied.
pub const OTLP_ENDPOINT_ENV: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";

/// OpenTelemetry service name reported to the collector.
pub const SERVICE_NAME: &str = "soroban-sentinel";

/// Export timeout applied to both traces and metrics.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// Metrics export interval for short-lived CLI runs.
const METRICS_EXPORT_INTERVAL: Duration = Duration::from_secs(2);

/// Resolved telemetry configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetryConfig {
    endpoint: Option<String>,
}

impl TelemetryConfig {
    /// Creates a disabled telemetry configuration.
    pub fn disabled() -> Self {
        Self { endpoint: None }
    }

    /// Resolves the endpoint from command-line input, then the standard
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` environment variable.
    pub fn resolve(cli_endpoint: Option<&str>) -> Self {
        Self::resolve_with_env(cli_endpoint, env::var(OTLP_ENDPOINT_ENV).ok().as_deref())
    }

    /// Resolves the endpoint from explicit sources. Used directly by tests to
    /// avoid mutating process environment state.
    pub fn resolve_with_env(cli_endpoint: Option<&str>, env_endpoint: Option<&str>) -> Self {
        let endpoint =
            normalize_endpoint(cli_endpoint).or_else(|| normalize_endpoint(env_endpoint));
        Self { endpoint }
    }

    /// Returns the normalized OTLP endpoint, if telemetry is enabled.
    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }

    /// Returns true when telemetry export is configured.
    pub fn is_enabled(&self) -> bool {
        self.endpoint.is_some()
    }
}

/// Normalizes an OTLP endpoint.
///
/// Empty and whitespace-only values disable telemetry. Endpoints without a URI
/// scheme are treated as plain gRPC HTTP endpoints, so `localhost:4317`
/// becomes `http://localhost:4317`.
pub fn normalize_endpoint(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") {
        Some(trimmed.to_string())
    } else {
        Some(format!("http://{trimmed}"))
    }
}

/// Holds initialized telemetry providers so they can be flushed before exit.
#[derive(Debug, Default)]
pub struct TelemetryGuard {
    tracer_provider: Option<TracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl TelemetryGuard {
    /// Flushes buffered spans and metrics, swallowing collector failures so the
    /// process can still exit with the command's real status code.
    pub fn shutdown(self) {
        if let Some(provider) = &self.tracer_provider {
            for result in provider.force_flush() {
                if let Err(error) = result {
                    eprintln!("Warning: failed to flush OTLP traces: {error}");
                }
            }
        }

        if let Some(provider) = &self.meter_provider {
            if let Err(error) = provider.force_flush() {
                eprintln!("Warning: failed to flush OTLP metrics: {error}");
            }
            if let Err(error) = provider.shutdown() {
                eprintln!("Warning: failed to shut down OTLP metrics: {error}");
            }
        }
    }
}

/// Initializes OTLP tracing and metrics, returning an inert guard when
/// telemetry is disabled or initialization fails.
pub fn init(config: &TelemetryConfig) -> TelemetryGuard {
    let Some(endpoint) = config.endpoint() else {
        return TelemetryGuard::default();
    };

    let resource = Resource::new(vec![
        KeyValue::new("service.name", SERVICE_NAME),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ]);

    let tracer_provider = match init_tracer(endpoint, resource.clone()) {
        Ok(provider) => Some(provider),
        Err(error) => {
            eprintln!("Warning: OpenTelemetry trace export disabled: {error}");
            None
        }
    };

    let meter_provider = match init_meter(endpoint, resource) {
        Ok(provider) => Some(provider),
        Err(error) => {
            eprintln!("Warning: OpenTelemetry metric export disabled: {error}");
            None
        }
    };

    TelemetryGuard {
        tracer_provider,
        meter_provider,
    }
}

fn init_tracer(endpoint: &str, resource: Resource) -> Result<TracerProvider, String> {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(endpoint.to_string())
        .with_timeout(EXPORT_TIMEOUT)
        .build_span_exporter()
        .map_err(|error| error.to_string())?;

    let provider = TracerProvider::builder()
        .with_config(opentelemetry_sdk::trace::Config::default().with_resource(resource))
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .build();
    let tracer = provider.tracer(SERVICE_NAME);
    let _ = global::set_tracer_provider(provider.clone());

    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .try_init()
        .map_err(|error| error.to_string())?;

    Ok(provider)
}

fn init_meter(endpoint: &str, resource: Resource) -> Result<SdkMeterProvider, String> {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(endpoint.to_string())
        .with_timeout(EXPORT_TIMEOUT);

    opentelemetry_otlp::new_pipeline()
        .metrics(opentelemetry_sdk::runtime::Tokio)
        .with_exporter(exporter)
        .with_resource(resource)
        .with_period(METRICS_EXPORT_INTERVAL)
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())
}

/// Records metrics for a completed contract scan report.
pub fn record_scan_metrics(report: &TTLReport) {
    let summary = report.summary();
    record_scan_counts(
        &report.network,
        1,
        summary.critical as u64,
        summary.archived as u64,
    );
}

/// Records contract and entry counters using the global meter provider.
///
/// The global provider is a no-op when telemetry is disabled, so this is safe
/// to call unconditionally.
pub fn record_scan_counts(network: &str, scanned: u64, critical: u64, expired: u64) {
    let meter = global::meter(SERVICE_NAME);
    let attributes = [KeyValue::new("network", network.to_string())];

    if scanned > 0 {
        meter
            .u64_counter("sentinel.contracts.scanned")
            .init()
            .add(scanned, &attributes);
    }
    if expired > 0 {
        meter
            .u64_counter("sentinel.entries.expired")
            .init()
            .add(expired, &attributes);
    }
    if critical > 0 {
        meter
            .u64_counter("sentinel.findings.critical")
            .init()
            .add(critical, &attributes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_endpoint_takes_precedence_over_environment() {
        let config = TelemetryConfig::resolve_with_env(
            Some("http://cli-collector:4317"),
            Some("http://env-collector:4317"),
        );
        assert_eq!(config.endpoint(), Some("http://cli-collector:4317"));
        assert!(config.is_enabled());
    }

    #[test]
    fn environment_endpoint_is_used_as_fallback() {
        let config = TelemetryConfig::resolve_with_env(None, Some("http://env-collector:4317"));
        assert_eq!(config.endpoint(), Some("http://env-collector:4317"));
    }

    #[test]
    fn blank_endpoints_disable_telemetry() {
        assert_eq!(
            TelemetryConfig::resolve_with_env(None, None).endpoint(),
            None
        );
        assert_eq!(
            TelemetryConfig::resolve_with_env(Some("   "), Some("\t")).endpoint(),
            None
        );
        assert!(!TelemetryConfig::disabled().is_enabled());
    }

    #[test]
    fn endpoint_without_scheme_defaults_to_http() {
        assert_eq!(
            normalize_endpoint(Some("localhost:4317")).as_deref(),
            Some("http://localhost:4317")
        );
        assert_eq!(
            normalize_endpoint(Some(" https://collector.example:4318 ")).as_deref(),
            Some("https://collector.example:4318")
        );
    }

    #[test]
    fn disabled_guard_shutdown_is_a_no_op() {
        let guard = init(&TelemetryConfig::disabled());
        guard.shutdown();
    }

    #[test]
    fn recording_metrics_without_a_provider_is_safe() {
        record_scan_counts("testnet", 1, 2, 3);
    }

    #[test]
    fn malformed_endpoint_disables_export_without_panicking() {
        let config = TelemetryConfig::resolve_with_env(Some("http://["), None);
        assert!(config.is_enabled());

        // Invalid URI syntax must degrade to a warning plus an inert guard
        // instead of aborting the CLI command.
        let guard = init(&config);
        guard.shutdown();
    }
}
