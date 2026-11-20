//! Application use case for comparing baseline and hypothetical protocol rules.

use crate::domain::scenario::{
    evaluate_protocol_scenario, NetworkRentParameters, ProtocolScenarioReport, ScenarioEntry,
    ScenarioError,
};
use crate::usecases::scan::TTLReport;

/// Runs a protocol rent and TTL threshold scenario against an existing scan.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProtocolScenarioUseCase;

impl ProtocolScenarioUseCase {
    /// Evaluates a scan report using baseline and simulated rent parameters.
    pub fn execute(
        &self,
        report: &TTLReport,
        baseline: &NetworkRentParameters,
        simulated: &NetworkRentParameters,
    ) -> Result<ProtocolScenarioReport, ScenarioError> {
        let entries: Vec<_> = report
            .entries
            .iter()
            .map(|entry| {
                ScenarioEntry::new(
                    entry.key_identifier.clone(),
                    entry.byte_size,
                    entry.remaining_ledgers,
                )
            })
            .collect();

        evaluate_protocol_scenario(&entries, baseline, simulated)
    }
}
