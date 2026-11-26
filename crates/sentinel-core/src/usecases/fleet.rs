//! Use case orchestrator for scanning many contracts concurrently with a bounded fan-out.

use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::domain::RiskLevel;
use crate::error::SentinelResult;
use crate::ports::reader::LedgerReader;
use crate::usecases::scan::{ScanContractUseCase, ScanRequest, TTLReport};

/// Result of scanning one contract as part of a fleet.
#[derive(Debug)]
pub struct FleetScanOutcome {
    /// Contract address strkey copied from the originating [`ScanRequest`].
    pub contract_id: String,
    /// The contract's report, or the error that stopped its scan.
    pub result: SentinelResult<TTLReport>,
}

/// Per-contract outcomes for a fleet scan plus the fleet-level risk.
#[derive(Debug)]
pub struct FleetScanReport {
    /// One outcome per request, in the same order as the input requests.
    pub outcomes: Vec<FleetScanOutcome>,
    /// Highest `overall_status` across successful scans (`Ok` when none succeeded).
    /// Failed scans are not folded into this value; check [`FleetScanReport::failed`].
    pub overall_status: RiskLevel,
}

impl FleetScanReport {
    fn new(outcomes: Vec<FleetScanOutcome>) -> Self {
        let overall_status = outcomes
            .iter()
            .filter_map(|outcome| outcome.result.as_ref().ok())
            .map(|report| report.overall_status)
            .max()
            .unwrap_or(RiskLevel::Ok);
        Self {
            outcomes,
            overall_status,
        }
    }

    /// Number of contracts whose scan returned a report.
    pub fn succeeded(&self) -> usize {
        self.outcomes.iter().filter(|o| o.result.is_ok()).count()
    }

    /// Number of contracts whose scan returned an error.
    pub fn failed(&self) -> usize {
        self.outcomes.len() - self.succeeded()
    }
}

/// Scans multiple contracts concurrently while keeping at most `max_concurrency` scans in flight.
///
/// A failing contract is recorded in its own outcome and never aborts the rest of the fleet.
pub struct FleetScanUseCase<R: LedgerReader> {
    scanner: Arc<ScanContractUseCase<R>>,
    max_concurrency: usize,
}

impl<R: LedgerReader + 'static> FleetScanUseCase<R> {
    /// Create a fleet scanner that shares `scanner` across concurrent tasks.
    ///
    /// A `max_concurrency` of `0` is treated as `1` so the fleet still makes progress.
    pub fn new(scanner: Arc<ScanContractUseCase<R>>, max_concurrency: usize) -> Self {
        Self {
            scanner,
            max_concurrency: max_concurrency.max(1),
        }
    }

    /// Maximum number of contract scans that may run at the same time.
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// Scan every request and return their outcomes in input order.
    pub async fn execute(&self, requests: Vec<ScanRequest>) -> FleetScanReport {
        let total = requests.len();
        let semaphore = Arc::new(Semaphore::new(self.max_concurrency));
        let mut tasks = JoinSet::new();

        for (index, request) in requests.into_iter().enumerate() {
            // Waiting for a permit before spawning bounds both in-flight scans
            // and the number of live tasks.
            let permit = Arc::clone(&semaphore)
                .acquire_owned()
                .await
                .expect("fleet semaphore is never closed");
            let scanner = Arc::clone(&self.scanner);
            tasks.spawn(async move {
                let _permit = permit;
                let contract_id = request.contract_id_strkey.clone();
                let result = scanner.execute(request).await;
                (
                    index,
                    FleetScanOutcome {
                        contract_id,
                        result,
                    },
                )
            });
        }

        let mut slots: Vec<Option<FleetScanOutcome>> = (0..total).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, outcome)) => slots[index] = Some(outcome),
                // Tasks are never aborted, so a join error is a panic inside a
                // scan; surface it instead of hiding a bug as a scan failure.
                Err(error) => std::panic::resume_unwind(error.into_panic()),
            }
        }

        FleetScanReport::new(
            slots
                .into_iter()
                .map(|slot| slot.expect("every spawned scan reports exactly once"))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use stellar_xdr::curr::{
        ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
        LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
    };

    use crate::adapters::derive_contract_instance_key;
    use crate::error::SentinelError;
    use crate::ports::mock::MockLedgerReader;
    use crate::ports::reader::{LatestLedgerInfo, RawLedgerEntry};

    /// Reader that tracks how many scans overlap by holding each latest-ledger call briefly.
    #[derive(Clone, Default)]
    struct ConcurrencyProbe {
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LedgerReader for ConcurrencyProbe {
        async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(LatestLedgerInfo::new(5_400_000, 22))
        }

        async fn get_ledger_entries(
            &self,
            _keys_xdr_base64: &[String],
        ) -> SentinelResult<Vec<RawLedgerEntry>> {
            Ok(Vec::new())
        }
    }

    fn contract_strkey(seed: u8) -> String {
        stellar_strkey::Contract([seed; 32]).to_string()
    }

    /// Persistent instance entry for a Stellar Asset Contract (no separate code entry).
    fn asset_instance_xdr(contract: [u8; 32]) -> String {
        LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::StellarAsset,
                storage: None,
            }),
        })
        .to_xdr_base64(Limits::none())
        .unwrap()
    }

    fn requests(count: u8) -> Vec<ScanRequest> {
        (1..=count)
            .map(|seed| ScanRequest::new(contract_strkey(seed)))
            .collect()
    }

    #[tokio::test]
    async fn test_fleet_scan_empty_request_list() {
        let fleet = FleetScanUseCase::new(
            Arc::new(ScanContractUseCase::new(MockLedgerReader::new())),
            4,
        );

        let report = fleet.execute(Vec::new()).await;

        assert!(report.outcomes.is_empty());
        assert_eq!(report.overall_status, RiskLevel::Ok);
        assert_eq!((report.succeeded(), report.failed()), (0, 0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_fleet_scan_bounds_concurrency_and_preserves_order() {
        let probe = ConcurrencyProbe::default();
        let peak = Arc::clone(&probe.peak);
        let fleet = FleetScanUseCase::new(Arc::new(ScanContractUseCase::new(probe)), 3);

        let report = fleet.execute(requests(10)).await;

        let observed_peak = peak.load(Ordering::SeqCst);
        assert!(observed_peak <= 3, "peak concurrency {observed_peak} > 3");
        assert!(observed_peak > 1, "scans never overlapped");
        let ids: Vec<String> = report
            .outcomes
            .iter()
            .map(|o| o.contract_id.clone())
            .collect();
        assert_eq!(ids, (1..=10).map(contract_strkey).collect::<Vec<_>>());
        assert_eq!(report.succeeded(), 10);
    }

    #[tokio::test]
    async fn test_fleet_scan_zero_concurrency_runs_sequentially() {
        let probe = ConcurrencyProbe::default();
        let peak = Arc::clone(&probe.peak);
        let fleet = FleetScanUseCase::new(Arc::new(ScanContractUseCase::new(probe)), 0);

        let report = fleet.execute(requests(3)).await;

        assert_eq!(fleet.max_concurrency(), 1);
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!(report.succeeded(), 3);
    }

    #[tokio::test]
    async fn test_fleet_scan_partial_failure_does_not_abort_fleet() {
        // Contract 1 has a live instance; contract 2 has none (archived);
        // the middle request has a malformed address and must fail alone.
        let live = [1u8; 32];
        let reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_entry(RawLedgerEntry::new(
                derive_contract_instance_key(&live).unwrap(),
                asset_instance_xdr(live),
                5_800_000,
                5_390_000,
            ));
        let fleet = FleetScanUseCase::new(Arc::new(ScanContractUseCase::new(reader)), 2);

        let report = fleet
            .execute(vec![
                ScanRequest::new(contract_strkey(1)),
                ScanRequest::new("not-a-contract"),
                ScanRequest::new(contract_strkey(2)),
            ])
            .await;

        assert_eq!(report.outcomes.len(), 3);
        assert_eq!(report.outcomes[0].contract_id, contract_strkey(1));
        let live_report = report.outcomes[0].result.as_ref().unwrap();
        assert_eq!(live_report.overall_status, RiskLevel::Ok);
        assert_eq!(report.outcomes[1].contract_id, "not-a-contract");
        assert!(matches!(
            report.outcomes[1].result,
            Err(SentinelError::InvalidContractAddress(_))
        ));
        let archived = report.outcomes[2].result.as_ref().unwrap();
        assert_eq!(archived.overall_status, RiskLevel::Archived);
        assert_eq!((report.succeeded(), report.failed()), (2, 1));
        assert_eq!(report.overall_status, RiskLevel::Archived);
    }

    #[tokio::test]
    async fn test_fleet_scan_all_failures_reports_errors_with_ok_aggregate() {
        let reader = MockLedgerReader::new().with_error(SentinelError::RpcConnectionTimeout(
            "https://rpc.example".into(),
        ));
        let fleet = FleetScanUseCase::new(Arc::new(ScanContractUseCase::new(reader)), 2);

        let report = fleet.execute(requests(2)).await;

        assert_eq!(report.failed(), 2);
        assert!(report
            .outcomes
            .iter()
            .all(|o| matches!(o.result, Err(SentinelError::RpcConnectionTimeout(_)))));
        assert_eq!(report.overall_status, RiskLevel::Ok);
    }
}
