//! Application use cases orchestrating domain rules and port adapters.

pub mod fleet;
pub mod forecast;
pub mod network;
pub mod scan;
pub(crate) mod scenario;

pub use fleet::{FleetScanOutcome, FleetScanReport, FleetScanUseCase};
pub use forecast::{ForecastExtensionUseCase, ForecastReport, ForecastRequest};
pub use network::fetch_network_limits;
pub use scan::{
    compute_overall_status, InstanceStorageReport, ReportSummary, ScanContractUseCase, ScanRequest,
    StorageEntryReport, TTLReport,
};
pub use scenario::ProtocolScenarioUseCase;

#[cfg(feature = "fuzzing")]
pub use scan::fuzz_compute_byte_size;
pub mod discover;
pub use discover::discover_storage_keys;
