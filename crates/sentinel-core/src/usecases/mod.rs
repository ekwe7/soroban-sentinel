//! Application use cases orchestrating domain rules and port adapters.

pub mod forecast;
pub mod scan;
pub mod scenario;

pub use forecast::{ForecastExtensionUseCase, ForecastReport, ForecastRequest};
pub use scan::{
    compute_overall_status, ReportSummary, ScanContractUseCase, ScanRequest, StorageEntryReport,
    TTLReport,
};
pub use scenario::ProtocolScenarioUseCase;

#[cfg(feature = "fuzzing")]
pub use scan::fuzz_compute_byte_size;
