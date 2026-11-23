//! Parquet export for scan reports.
//!
//! Uses the same nine columns, in the same order, as the CSV formatter. The
//! rows come from [`build_tabular_rows`], so both formats always carry the
//! same values. This module is only compiled with the `parquet` Cargo feature.

use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use sentinel_core::{SentinelError, SentinelResult, TTLReport};

use super::tabular::{build_tabular_rows, TabularRow, CSV_HEADER};

fn parquet_error(context: &str, err: impl std::fmt::Display) -> SentinelError {
    SentinelError::SerializationError(format!("Failed to {}: {}", context, err))
}

/// Serializes a scan report as Parquet bytes (one row per storage entry).
pub fn render_parquet(report: &TTLReport) -> SentinelResult<Vec<u8>> {
    let rows = build_tabular_rows(report)?;
    let (schema, batch) = build_batch(&rows)?;

    // No compression codecs are enabled in this feature, so keep the file
    // uncompressed instead of relying on an optional codec being present.
    let properties = WriterProperties::builder()
        .set_compression(Compression::UNCOMPRESSED)
        .build();

    let mut writer = ArrowWriter::try_new(Vec::new(), schema, Some(properties))
        .map_err(|e| parquet_error("initialize Parquet writer", e))?;
    writer
        .write(&batch)
        .map_err(|e| parquet_error("write Parquet rows", e))?;
    writer
        .finish()
        .map_err(|e| parquet_error("finish Parquet output", e))?;

    // `into_inner` flushes again and errors once the writer is finished; the
    // finalized bytes are already available through the underlying buffer.
    Ok(writer.inner_mut().clone())
}

/// Builds the Arrow batch with the exact tabular schema and column order.
fn build_batch(rows: &[TabularRow]) -> SentinelResult<(SchemaRef, RecordBatch)> {
    let columns: Vec<&str> = CSV_HEADER.split(',').collect();
    debug_assert_eq!(columns.len(), 9);

    let schema = Arc::new(Schema::new(vec![
        Field::new(
            columns[0],
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        Field::new(columns[1], DataType::Utf8, true),
        Field::new(columns[2], DataType::Utf8, true),
        Field::new(columns[3], DataType::Utf8, true),
        Field::new(columns[4], DataType::Utf8, true),
        Field::new(columns[5], DataType::Int64, true),
        Field::new(columns[6], DataType::Int64, true),
        Field::new(columns[7], DataType::Int64, true),
        Field::new(columns[8], DataType::Utf8, true),
    ]));

    let timestamps: Vec<Option<i64>> = rows
        .iter()
        .map(|row| row.timestamp_seconds.checked_mul(1_000_000))
        .collect();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(TimestampMicrosecondArray::from(timestamps).with_timezone("UTC")),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|row| row.network.as_str())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|row| row.contract_id.as_str())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|row| row.key_xdr.as_deref())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter().map(|row| row.durability).collect::<Vec<_>>(),
        )),
        Arc::new(Int64Array::from(
            rows.iter()
                .map(|row| Some(row.byte_size))
                .collect::<Vec<_>>(),
        )),
        Arc::new(Int64Array::from(
            rows.iter()
                .map(|row| Some(row.live_until_ledger))
                .collect::<Vec<_>>(),
        )),
        Arc::new(Int64Array::from(
            rows.iter()
                .map(|row| Some(row.remaining_ledgers))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter().map(|row| row.status).collect::<Vec<_>>(),
        )),
    ];

    let batch = RecordBatch::try_new(schema.clone(), arrays)
        .map_err(|e| parquet_error("build Parquet batch", e))?;
    Ok((schema, batch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formatters::render_csv;
    use arrow_array::Array;
    use sentinel_core::domain::{Durability, EntryType, RiskLevel};
    use sentinel_core::usecases::StorageEntryReport;

    fn sample_report(entry_type: EntryType) -> TTLReport {
        TTLReport {
            contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            ledger_velocity_seconds: 5.0,
            entries: vec![StorageEntryReport::new(
                "code_wasm",
                entry_type,
                Durability::Code,
                128,
                5_600_000,
                200_000,
                1_000_000.0,
                10.0,
                RiskLevel::Ok,
            )],
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
        }
    }

    #[test]
    fn test_parquet_schema_columns_and_types() {
        let rows = build_tabular_rows(&sample_report(EntryType::PersistentData)).unwrap();
        let (schema, batch) = build_batch(&rows).unwrap();

        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, CSV_HEADER.split(',').collect::<Vec<_>>());
        assert_eq!(
            names,
            vec![
                "timestamp",
                "network",
                "contract_id",
                "key_xdr",
                "durability",
                "byte_size",
                "live_until_ledger",
                "remaining_ledgers",
                "status",
            ]
        );
        assert_eq!(
            schema.field(0).data_type(),
            &DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
        );
        for index in [5, 6, 7] {
            assert_eq!(schema.field(index).data_type(), &DataType::Int64);
            assert!(schema.field(index).is_nullable());
        }
        assert_eq!(batch.num_columns(), 9);
        assert_eq!(batch.num_rows(), 1);
    }

    #[test]
    fn test_missing_key_xdr_is_null_in_parquet_and_blank_in_csv() {
        let report = sample_report(EntryType::ContractCode);
        let rows = build_tabular_rows(&report).unwrap();
        assert!(rows[0].key_xdr.is_none());

        let (_, batch) = build_batch(&rows).unwrap();
        let key_xdr = batch
            .column(3)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert!(key_xdr.is_null(0));

        let csv = render_csv(&report).unwrap();
        assert!(csv
            .lines()
            .nth(1)
            .unwrap()
            .split(',')
            .nth(3)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_render_parquet_produces_readable_bytes() {
        let bytes = render_parquet(&sample_report(EntryType::PersistentData)).unwrap();
        assert!(bytes.starts_with(b"PAR1"));
        assert!(bytes.ends_with(b"PAR1"));
    }
}
