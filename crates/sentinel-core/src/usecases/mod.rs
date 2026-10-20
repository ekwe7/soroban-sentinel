//! Application use cases orchestrating domain rules and port adapters.

pub mod scan;

pub use scan::{
    compute_overall_status, ReportSummary, ScanContractUseCase, ScanRequest, StorageEntryReport,
    TTLReport,
};
