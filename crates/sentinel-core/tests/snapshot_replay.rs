use sentinel_core::adapters::{
    derive_contract_instance_key, SentinelSnapshot, SnapshotData, SnapshotHeader,
    SnapshotLedgerReader, SNAPSHOT_FORMAT_VERSION,
};
use sentinel_core::ports::mock::MockLedgerReader;
use sentinel_core::ports::reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry};
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use tempfile::tempdir;

fn instance_xdr(contract_id: [u8; 32]) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::StellarAsset,
            storage: None,
        }),
    })
    .to_xdr_base64(Limits::none())
    .expect("valid instance XDR")
}

#[tokio::test]
async fn live_snapshot_and_replay_match() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("state.sentinel-snap");
    let passphrase = "Test SDF Network ; September 2015";
    let entry = RawLedgerEntry::new("key-1", "value-1", 6_000_000, 5_000_000);
    let live = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_velocity(5.5)
        .with_entry(entry.clone());
    let recording = SnapshotLedgerReader::recording(live, &path, passphrase);

    let live_latest = recording.get_latest_ledger().await.expect("latest");
    let live_entries = recording
        .get_ledger_entries(&["key-1".to_string()])
        .await
        .expect("entries");
    let live_velocity = recording
        .get_ledger_close_velocity(live_latest.sequence)
        .await
        .expect("velocity");

    let replay = SnapshotLedgerReader::replay(&path, passphrase).expect("replay");
    assert_eq!(
        live_latest,
        replay.get_latest_ledger().await.expect("replayed latest")
    );
    assert_eq!(
        live_entries,
        replay
            .get_ledger_entries(&["key-1".to_string()])
            .await
            .expect("replayed entries")
    );
    assert_eq!(
        live_velocity,
        replay
            .get_ledger_close_velocity(live_latest.sequence)
            .await
            .expect("replayed velocity")
    );
}

#[tokio::test]
async fn live_and_replay_scans_produce_identical_reports() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("scan.sentinel-snap");
    let passphrase = "Test SDF Network ; September 2015";
    let contract_id = [7u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).expect("instance key");
    let entry = RawLedgerEntry::new(
        instance_key,
        instance_xdr(contract_id),
        6_000_000,
        5_390_000,
    );
    let live = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_velocity(4.7)
        .with_entry(entry);
    let request = ScanRequest::new(&contract_strkey)
        .with_network("testnet")
        .with_ledger_seconds(None);

    let live_report =
        ScanContractUseCase::new(SnapshotLedgerReader::recording(live, &path, passphrase))
            .execute(request.clone())
            .await
            .expect("live scan");

    let replay_report = ScanContractUseCase::new(
        SnapshotLedgerReader::replay(&path, passphrase).expect("replay reader"),
    )
    .execute(request)
    .await
    .expect("replayed scan");

    assert_eq!(live_report, replay_report);
    assert_eq!(replay_report.ledger_velocity_seconds, 4.7);
}

#[test]
fn snapshot_bytes_are_deterministic_regardless_of_entry_order() {
    let dir = tempdir().expect("tempdir");
    let first = dir.path().join("first.sentinel-snap");
    let second = dir.path().join("second.sentinel-snap");
    let passphrase = "Test SDF Network ; September 2015";
    let entry_a = RawLedgerEntry::new("a", "a-value", 100, 90);
    let entry_b = RawLedgerEntry::new("b", "b-value", 200, 190);

    let snapshot = SentinelSnapshot {
        header: SnapshotHeader {
            format_version: SNAPSHOT_FORMAT_VERSION,
            network_passphrase: passphrase.to_string(),
            protocol_version: 22,
        },
        data: SnapshotData {
            latest_ledger: LatestLedgerInfo::new(5_400_000, 22),
            ledger_close_velocity_seconds: Some(5.5),
            entries: vec![entry_b, entry_a],
        },
    };
    let mut reversed = snapshot.clone();
    reversed.data.entries.reverse();

    snapshot
        .write_deterministic(&first)
        .expect("write first snapshot");
    reversed
        .write_deterministic(&second)
        .expect("write second snapshot");
    assert_eq!(
        std::fs::read(&first).expect("read first"),
        std::fs::read(&second).expect("read second")
    );
}

#[test]
fn snapshot_rejects_corrupt_truncated_wrong_passphrase_version_and_empty_entries() {
    let dir = tempdir().expect("tempdir");
    let passphrase = "Test SDF Network ; September 2015";

    let corrupt = dir.path().join("corrupt.sentinel-snap");
    std::fs::write(&corrupt, b"{").expect("write corrupt");
    assert!(SnapshotLedgerReader::replay(&corrupt, passphrase).is_err());

    let truncated = dir.path().join("truncated.sentinel-snap");
    std::fs::write(&truncated, b"{}").expect("write truncated");
    assert!(SnapshotLedgerReader::replay(&truncated, passphrase).is_err());

    let missing = dir.path().join("missing.sentinel-snap");
    assert!(SnapshotLedgerReader::replay(&missing, passphrase).is_err());

    let snapshot = SentinelSnapshot {
        header: SnapshotHeader {
            format_version: SNAPSHOT_FORMAT_VERSION,
            network_passphrase: "wrong".to_string(),
            protocol_version: 22,
        },
        data: SnapshotData {
            latest_ledger: LatestLedgerInfo::new(5_400_000, 22),
            ledger_close_velocity_seconds: Some(5.5),
            entries: vec![RawLedgerEntry::new("key", "entry", 6_000_000, 5_000_000)],
        },
    };
    let wrong = dir.path().join("wrong.sentinel-snap");
    snapshot
        .write_deterministic(&wrong)
        .expect("write wrong passphrase snapshot");
    assert!(SnapshotLedgerReader::replay(&wrong, passphrase).is_err());

    let mut unsupported = snapshot.clone();
    unsupported.header.network_passphrase = passphrase.to_string();
    unsupported.header.format_version = SNAPSHOT_FORMAT_VERSION + 1;
    let version = dir.path().join("version.sentinel-snap");
    unsupported
        .write_deterministic(&version)
        .expect("write unsupported snapshot");
    assert!(SnapshotLedgerReader::replay(&version, passphrase).is_err());

    let mut empty = unsupported;
    empty.header.format_version = SNAPSHOT_FORMAT_VERSION;
    empty.data.entries.clear();
    let empty_path = dir.path().join("empty.sentinel-snap");
    empty
        .write_deterministic(&empty_path)
        .expect("write empty snapshot");
    assert!(SnapshotLedgerReader::replay(&empty_path, passphrase).is_err());
}
