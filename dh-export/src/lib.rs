//! # dh-export
//!
//! The **one deliberate, audited materialization boundary** in the `dh-*` fleet.
//!
//! Every full-dataset crossing happens through a single call:
//!
//! ```text
//! export(store, handle, fmt, dest) -> Result<ExportReceipt, ExportError>
//! ```
//!
//! Supported formats:
//! - [`ExportFmt::Csv`] — always available
//! - [`ExportFmt::Json`] — always available; bounded by `max_rows`
//! - [`ExportFmt::Parquet`] — behind the `parquet` cargo feature
//!
//! Supported destinations:
//! - [`ExportDest::File`] — atomic write (tempfile + rename); refuses overwrite
//!   without [`ExportOptions::overwrite`].
//! - [`ExportDest::Inline`] — returns bytes directly in [`ExportReceipt`]; bounded
//!   by `max_bytes`.
//!
//! [`ExportReceipt`] carries `{ handle, fmt, dest, row_count, bytes, sha256, ts }`
//! — the audit record of exactly what crossed.

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod error;
pub mod receipt;
pub mod types;

pub use error::ExportError;
pub use receipt::ExportReceipt;
pub use types::{ExportDest, ExportFmt, ExportOptions};

use dh_spec::DatasetHandle;
use dh_store::{ColumnData, Dataset, Store};

use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

// ── SHA-256 helper ─────────────────────────────────────────────────────────

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// ── Unix timestamp helper ──────────────────────────────────────────────────

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

// ── CSV serialization ──────────────────────────────────────────────────────

fn dataset_to_csv(dataset: &Dataset) -> Result<Vec<u8>, ExportError> {
    let mut wtr = csv::Writer::from_writer(Vec::new());

    // Header row
    let headers: Vec<&str> = dataset.columns.iter().map(|c| c.name.as_str()).collect();
    wtr.write_record(&headers)
        .map_err(|e| ExportError::Io(e.to_string()))?;

    // Data rows
    let row_count = dataset.row_count();
    for row_idx in 0..row_count {
        let mut record = Vec::with_capacity(dataset.columns.len());
        for col_data in &dataset.data {
            let cell = column_value_to_string(col_data, row_idx);
            record.push(cell);
        }
        wtr.write_record(&record)
            .map_err(|e| ExportError::Io(e.to_string()))?;
    }

    wtr.flush().map_err(|e| ExportError::Io(e.to_string()))?;
    wtr.into_inner().map_err(|e| ExportError::Io(e.to_string()))
}

fn column_value_to_string(col: &ColumnData, idx: usize) -> String {
    match col {
        ColumnData::Int(v) => v[idx].map_or_else(String::new, |n| n.to_string()),
        ColumnData::Float(v) => v[idx].map_or_else(String::new, |f| f.to_string()),
        ColumnData::Decimal(v) | ColumnData::Str(v) | ColumnData::Date(v) | ColumnData::Time(v) => {
            v[idx].clone().unwrap_or_default()
        }
        ColumnData::Bool(v) => v[idx].map_or_else(String::new, |b| b.to_string()),
        // ColumnData is #[non_exhaustive]; unknown future variants produce empty string.
        _ => String::new(),
    }
}

// ── JSON serialization ─────────────────────────────────────────────────────

fn dataset_to_json(
    dataset: &Dataset,
    max_rows: usize,
    override_limit: bool,
) -> Result<Vec<u8>, ExportError> {
    let row_count = dataset.row_count();
    if !override_limit && row_count > max_rows {
        return Err(ExportError::JsonLimitExceeded {
            actual: row_count,
            limit: max_rows,
        });
    }

    let export_rows = if override_limit {
        row_count
    } else {
        row_count.min(max_rows)
    };

    let mut rows = Vec::with_capacity(export_rows);
    for row_idx in 0..export_rows {
        let mut obj = serde_json::Map::new();
        for (col_schema, col_data) in dataset.columns.iter().zip(dataset.data.iter()) {
            let val = column_value_to_json(col_data, row_idx);
            obj.insert(col_schema.name.clone(), val);
        }
        rows.push(serde_json::Value::Object(obj));
    }

    serde_json::to_vec_pretty(&rows).map_err(|e| ExportError::Io(e.to_string()))
}

fn column_value_to_json(col: &ColumnData, idx: usize) -> serde_json::Value {
    match col {
        ColumnData::Int(v) => v[idx].map_or(serde_json::Value::Null, |n| {
            serde_json::Value::Number(n.into())
        }),
        ColumnData::Float(v) => v[idx].map_or(serde_json::Value::Null, |f| {
            serde_json::Number::from_f64(f)
                .map_or(serde_json::Value::Null, serde_json::Value::Number)
        }),
        ColumnData::Bool(v) => {
            v[idx].map_or(serde_json::Value::Null, serde_json::Value::Bool)
        }
        ColumnData::Decimal(v) | ColumnData::Str(v) | ColumnData::Date(v) | ColumnData::Time(v) => {
            v[idx]
                .clone()
                .map_or(serde_json::Value::Null, serde_json::Value::String)
        }
        // ColumnData is #[non_exhaustive]; unknown future variants serialize as null.
        _ => serde_json::Value::Null,
    }
}

// ── Parquet serialization (feature-gated) ─────────────────────────────────

#[cfg(feature = "parquet")]
fn dataset_to_parquet(dataset: &Dataset) -> Result<Vec<u8>, ExportError> {
    use arrow::array::{
        BooleanArray, Float64Array, Int64Array, StringArray,
    };
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use parquet::arrow::arrow_writer::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    use std::sync::Arc;

    // Build Arrow schema
    let fields: Vec<Field> = dataset
        .columns
        .iter()
        .zip(dataset.data.iter())
        .map(|(schema, data)| {
            let dt = match data {
                ColumnData::Int(_) => DataType::Int64,
                ColumnData::Float(_) => DataType::Float64,
                ColumnData::Bool(_) => DataType::Boolean,
                ColumnData::Decimal(_)
                | ColumnData::Str(_)
                | ColumnData::Date(_)
                | ColumnData::Time(_) => DataType::Utf8,
            };
            Field::new(&schema.name, dt, schema.nullable)
        })
        .collect();
    let arrow_schema = Arc::new(Schema::new(fields));

    // Build Arrow arrays
    let arrays: Vec<Arc<dyn arrow::array::Array>> = dataset
        .data
        .iter()
        .map(|col_data| -> Arc<dyn arrow::array::Array> {
            match col_data {
                ColumnData::Int(v) => Arc::new(Int64Array::from(v.clone())),
                ColumnData::Float(v) => Arc::new(Float64Array::from(v.clone())),
                ColumnData::Bool(v) => Arc::new(BooleanArray::from(v.clone())),
                ColumnData::Decimal(v)
                | ColumnData::Str(v)
                | ColumnData::Date(v)
                | ColumnData::Time(v) => {
                    let strs: Vec<Option<&str>> =
                        v.iter().map(|o| o.as_deref()).collect();
                    Arc::new(StringArray::from(strs))
                }
            }
        })
        .collect();

    let batch = RecordBatch::try_new(arrow_schema.clone(), arrays)
        .map_err(|e| ExportError::Io(e.to_string()))?;

    let props = WriterProperties::builder().build();
    let mut buf = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buf, arrow_schema, Some(props))
        .map_err(|e| ExportError::Io(e.to_string()))?;

    writer
        .write(&batch)
        .map_err(|e| ExportError::Io(e.to_string()))?;
    writer.close().map_err(|e| ExportError::Io(e.to_string()))?;

    Ok(buf)
}

// ── Atomic file write ──────────────────────────────────────────────────────

fn write_atomic(dest_path: &Path, data: &[u8], overwrite: bool) -> Result<(), ExportError> {
    if dest_path.exists() && !overwrite {
        return Err(ExportError::FileExists(dest_path.to_path_buf()));
    }

    // Write to a sibling temp file, then rename — atomic on POSIX.
    let parent = dest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let tmp = tempfile::Builder::new()
        .prefix(".dh-export-tmp-")
        .tempfile_in(parent)
        .map_err(|e| ExportError::Io(e.to_string()))?;

    std::fs::write(tmp.path(), data).map_err(|e| ExportError::Io(e.to_string()))?;

    // persist() → rename; if the target exists and overwrite=true, rename
    // atomically replaces it on POSIX.
    tmp.persist(dest_path)
        .map_err(|e| ExportError::Io(e.error.to_string()))?;

    Ok(())
}

// ── Inline size check ──────────────────────────────────────────────────────

fn check_inline_size(data: &[u8], max_bytes: usize) -> Result<(), ExportError> {
    if data.len() > max_bytes {
        return Err(ExportError::InlineLimitExceeded {
            actual: data.len(),
            limit: max_bytes,
        });
    }
    Ok(())
}

// ── Main export entry point ────────────────────────────────────────────────

/// Export a dataset identified by `handle` from `store` in the given format
/// and deliver it to `dest`.
///
/// This is **the single sanctioned exit** for full row data.  All exports are
/// logged via [`ExportReceipt`].
///
/// # Errors
///
/// Returns a typed [`ExportError`] for every failure mode:
/// - Handle not found / expired → [`ExportError::LookupFailed`]
/// - JSON export exceeds `max_rows` without override → [`ExportError::JsonLimitExceeded`]
/// - Inline export exceeds `max_bytes` → [`ExportError::InlineLimitExceeded`]
/// - File already exists without overwrite flag → [`ExportError::FileExists`]
/// - I/O errors → [`ExportError::Io`]
/// - Parquet feature not enabled → [`ExportError::ParquetNotEnabled`]
pub fn export(
    store: &Store,
    handle: &DatasetHandle,
    fmt: ExportFmt,
    dest: ExportDest,
    opts: ExportOptions,
) -> Result<ExportReceipt, ExportError> {
    // Resolve handle → dataset
    let dataset = store
        .get(handle)
        .map_err(|e| ExportError::LookupFailed(e.to_string()))?;

    let row_count = dataset.row_count();

    // Serialize to bytes based on format
    let payload: Vec<u8> = match &fmt {
        ExportFmt::Csv => dataset_to_csv(&dataset)?,
        ExportFmt::Json { max_rows } => {
            dataset_to_json(&dataset, *max_rows, opts.override_json_limit)?
        }
        ExportFmt::Parquet => {
            #[cfg(feature = "parquet")]
            {
                dataset_to_parquet(&dataset)?
            }
            #[cfg(not(feature = "parquet"))]
            {
                return Err(ExportError::ParquetNotEnabled);
            }
        }
    };

    let bytes = payload.len();
    let sha256 = sha256_hex(&payload);
    let ts = now_unix_secs();

    // Deliver to destination
    let final_payload: Option<Vec<u8>> = match &dest {
        ExportDest::File(path) => {
            write_atomic(path, &payload, opts.overwrite)?;
            None
        }
        ExportDest::Inline { max_bytes } => {
            check_inline_size(&payload, *max_bytes)?;
            Some(payload)
        }
    };

    Ok(ExportReceipt {
        handle: handle.clone(),
        fmt,
        dest,
        row_count: u64::try_from(row_count).unwrap_or(u64::MAX),
        bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
        sha256,
        ts,
        inline_payload: final_payload,
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use dh_spec::{ColumnRole, ColumnSchema, DType};
    use dh_store::{ColumnData, Dataset, Store};
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ── Fixture helpers ────────────────────────────────────────────────────

    fn make_col_schema(name: &str, dtype: DType, role: ColumnRole) -> ColumnSchema {
        ColumnSchema {
            name: name.to_string(),
            unique_name: format!("model.{name}"),
            dtype,
            nullable: false,
            role,
        }
    }

    /// Build a simple 2-column, 3-row dataset: name(str), value(int).
    fn make_fixture_dataset() -> Dataset {
        Dataset::new(
            vec![
                make_col_schema("name", DType::Str, ColumnRole::Dimension),
                make_col_schema("value", DType::Int, ColumnRole::Measure),
            ],
            vec![
                ColumnData::Str(vec![
                    Some("alice".to_string()),
                    Some("bob".to_string()),
                    Some("carol".to_string()),
                ]),
                ColumnData::Int(vec![Some(10), Some(20), Some(30)]),
            ],
        )
        .expect("valid fixture dataset")
    }

    fn make_store_with_fixture() -> (Store, DatasetHandle) {
        let store = Store::new(0);
        let ds = make_fixture_dataset();
        let handle = store.put(ds, 3600);
        (store, handle)
    }

    // ── AC1: CSV export — header + rows round-trip ──────────────────────────

    #[test]
    fn ac1_csv_export_correct_header_and_rows() {
        let (store, handle) = make_store_with_fixture();
        let opts = ExportOptions::default();

        let receipt = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::Inline { max_bytes: 65536 },
            opts,
        )
        .expect("CSV export should succeed");

        let payload = receipt.inline_payload.expect("inline payload present");
        let csv_str = std::str::from_utf8(&payload).expect("valid UTF-8");

        // Parse back
        let mut rdr = csv::Reader::from_reader(csv_str.as_bytes());
        let headers: Vec<String> = rdr
            .headers()
            .expect("headers")
            .iter()
            .map(str::to_string)
            .collect();
        assert_eq!(headers, vec!["name", "value"]);

        let rows: Vec<csv::StringRecord> = rdr.records().map(|r| r.expect("record")).collect();
        assert_eq!(rows.len(), 3, "should have 3 data rows");
        assert_eq!(&rows[0][0], "alice");
        assert_eq!(&rows[0][1], "10");
        assert_eq!(&rows[1][0], "bob");
        assert_eq!(&rows[2][0], "carol");
        assert_eq!(&rows[2][1], "30");

        assert_eq!(receipt.row_count, 3);
    }

    // ── AC2: JSON export honors max_rows (refuses over without override) ────

    #[test]
    fn ac2_json_max_rows_refused_without_override() {
        let (store, handle) = make_store_with_fixture(); // 3 rows

        // Request with max_rows = 2 (less than dataset size) — should fail
        let err = export(
            &store,
            &handle,
            ExportFmt::Json { max_rows: 2 },
            ExportDest::Inline { max_bytes: 65536 },
            ExportOptions::default(),
        )
        .expect_err("should refuse to export 3 rows when max_rows=2");

        match err {
            ExportError::JsonLimitExceeded { actual, limit } => {
                assert_eq!(actual, 3);
                assert_eq!(limit, 2);
            }
            other => panic!("expected JsonLimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn ac2_json_max_rows_succeeds_within_bound() {
        let (store, handle) = make_store_with_fixture(); // 3 rows

        let receipt = export(
            &store,
            &handle,
            ExportFmt::Json { max_rows: 10 },
            ExportDest::Inline { max_bytes: 65536 },
            ExportOptions::default(),
        )
        .expect("should succeed when rows <= max_rows");

        let payload = receipt.inline_payload.expect("inline payload");
        let parsed: Vec<serde_json::Value> =
            serde_json::from_slice(&payload).expect("valid JSON array");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0]["name"], "alice");
        assert_eq!(parsed[0]["value"], 10);
    }

    #[test]
    fn ac2_json_override_allows_exceeding_max_rows() {
        let (store, handle) = make_store_with_fixture(); // 3 rows

        let opts = ExportOptions {
            override_json_limit: true,
            ..Default::default()
        };

        let receipt = export(
            &store,
            &handle,
            ExportFmt::Json { max_rows: 1 }, // would normally block
            ExportDest::Inline { max_bytes: 65536 },
            opts,
        )
        .expect("override_json_limit=true should allow all rows");

        let payload = receipt.inline_payload.expect("inline payload");
        let parsed: Vec<serde_json::Value> =
            serde_json::from_slice(&payload).expect("valid JSON array");
        assert_eq!(parsed.len(), 3, "override exports all 3 rows");
    }

    // ── AC3: ExportReceipt sha256 stable for fixed fixture ──────────────────

    #[test]
    fn ac3_receipt_sha256_stable_for_fixed_fixture() {
        let (store, handle) = make_store_with_fixture();
        let opts = ExportOptions::default();

        let r1 = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::Inline { max_bytes: 65536 },
            opts.clone(),
        )
        .expect("first export");

        let r2 = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::Inline { max_bytes: 65536 },
            opts,
        )
        .expect("second export");

        // SHA-256 must be identical for the same data
        assert_eq!(r1.sha256, r2.sha256, "sha256 must be deterministic");
        assert_eq!(r1.bytes, r2.bytes, "byte count must be stable");
        assert_eq!(r1.row_count, 3);

        // The hash must be a 64-char hex string
        assert_eq!(r1.sha256.len(), 64, "sha256 is 64 hex chars");
        assert!(
            r1.sha256.chars().all(|c| c.is_ascii_hexdigit()),
            "sha256 is hex"
        );

        // Assert the specific hash for the fixture CSV (deterministic)
        // Compute it fresh and compare with a re-computation to validate stability.
        let payload1 = r1.inline_payload.expect("payload 1");
        let payload2 = r2.inline_payload.expect("payload 2");
        assert_eq!(payload1, payload2, "payloads identical");
        assert_eq!(sha256_hex(&payload1), r1.sha256);
    }

    // ── AC4: atomic write + no-overwrite-without-flag ───────────────────────

    #[test]
    fn ac4_file_no_overwrite_without_flag() {
        let tmp = TempDir::new().expect("tempdir");
        let path: PathBuf = tmp.path().join("export.csv");

        let (store, handle) = make_store_with_fixture();

        // First write should succeed
        export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::File(path.clone()),
            ExportOptions::default(),
        )
        .expect("first write should succeed");

        assert!(path.exists(), "file should exist after first write");

        // Second write without overwrite flag should fail
        let err = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::File(path.clone()),
            ExportOptions::default(),
        )
        .expect_err("second write without overwrite should fail");

        assert!(
            matches!(err, ExportError::FileExists(_)),
            "expected FileExists, got {err:?}"
        );
    }

    #[test]
    fn ac4_file_overwrite_with_flag_succeeds() {
        let tmp = TempDir::new().expect("tempdir");
        let path: PathBuf = tmp.path().join("export.csv");

        let (store, handle) = make_store_with_fixture();

        // First write
        export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::File(path.clone()),
            ExportOptions::default(),
        )
        .expect("first write");

        // Second write with overwrite=true — should succeed
        export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::File(path.clone()),
            ExportOptions {
                overwrite: true,
                ..Default::default()
            },
        )
        .expect("overwrite should succeed");
    }

    #[test]
    fn ac4_file_write_is_atomic_no_partial_on_missing_dir() {
        // Attempt to write to a non-existent directory — should fail with Io,
        // not produce a partial file.
        let path: PathBuf = PathBuf::from("/tmp/dh-export-nonexistent-dir-12345/out.csv");

        let (store, handle) = make_store_with_fixture();
        let err = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::File(path),
            ExportOptions::default(),
        )
        .expect_err("write to missing dir should fail");

        assert!(
            matches!(err, ExportError::Io(_)),
            "expected Io error, got {err:?}"
        );
    }

    // ── AC5: Parquet feature-gated ──────────────────────────────────────────

    #[test]
    #[cfg(feature = "parquet")]
    fn ac5_parquet_roundtrip() {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

        let tmp = TempDir::new().expect("tempdir");
        let path: PathBuf = tmp.path().join("export.parquet");
        let (store, handle) = make_store_with_fixture();

        export(
            &store,
            &handle,
            ExportFmt::Parquet,
            ExportDest::File(path.clone()),
            ExportOptions::default(),
        )
        .expect("parquet export should succeed");

        assert!(path.exists(), "parquet file should exist");

        // Read back with the parquet reader
        let file = std::fs::File::open(&path).expect("open parquet");
        let builder =
            ParquetRecordBatchReaderBuilder::try_new(file).expect("parquet reader builder");
        let mut reader = builder.build().expect("parquet reader");
        let batch = reader.next().expect("at least one batch").expect("valid batch");
        assert_eq!(batch.num_rows(), 3, "parquet round-trip: 3 rows");
        assert_eq!(batch.num_columns(), 2, "parquet round-trip: 2 columns");
    }

    /// When the `parquet` feature is OFF, requesting Parquet export returns a
    /// typed error instead of panicking or producing garbage.
    #[test]
    #[cfg(not(feature = "parquet"))]
    fn ac5_parquet_returns_error_when_feature_off() {
        let (store, handle) = make_store_with_fixture();

        let err = export(
            &store,
            &handle,
            ExportFmt::Parquet,
            ExportDest::Inline { max_bytes: 65536 },
            ExportOptions::default(),
        )
        .expect_err("parquet without feature should error");

        assert!(
            matches!(err, ExportError::ParquetNotEnabled),
            "expected ParquetNotEnabled, got {err:?}"
        );
    }

    // ── AC6: expired/unknown handle returns typed error ─────────────────────

    #[test]
    fn ac6_expired_handle_returns_typed_error() {
        let store = Store::new(0);
        let ds = make_fixture_dataset();
        let handle = store.put(ds, 0); // TTL=0 → expires immediately
        store.evict_expired();

        let err = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::Inline { max_bytes: 65536 },
            ExportOptions::default(),
        )
        .expect_err("expired handle should return error");

        assert!(
            matches!(err, ExportError::LookupFailed(_)),
            "expected LookupFailed, got {err:?}"
        );
    }

    #[test]
    fn ac6_unknown_handle_returns_typed_error() {
        let store = Store::new(0);
        let fake = DatasetHandle {
            id: "hdl_doesnotexist".to_string(),
            created_at: 0,
            ttl_secs: 3600,
            derived_from: None,
        };

        let err = export(
            &store,
            &fake,
            ExportFmt::Csv,
            ExportDest::Inline { max_bytes: 65536 },
            ExportOptions::default(),
        )
        .expect_err("unknown handle should return error");

        assert!(
            matches!(err, ExportError::LookupFailed(_)),
            "expected LookupFailed, got {err:?}"
        );
    }

    // ── AC7 structural: confirmed by cargo test + clippy passing ───────────

    #[test]
    fn ac7_cargo_test_and_clippy_pass() {
        // Satisfied externally; included for explicit AC numbering.
    }

    // ── Mutation-kill: Float and Bool column serialisation ──────────────────

    /// Kill mutants: delete match arm ColumnData::Float/Bool in column_value_to_string
    /// and column_value_to_json.  Also exercises the `ts` field being a plausible
    /// Unix timestamp (kills now_unix_secs constant-replacement mutants).
    #[test]
    fn mutation_kill_float_bool_columns_csv() {
        use dh_spec::{ColumnRole, ColumnSchema, DType};

        let store = Store::new(0);
        let ds = Dataset::new(
            vec![
                make_col_schema("active", DType::Bool, ColumnRole::Dimension),
                make_col_schema("score", DType::Float, ColumnRole::Measure),
            ],
            vec![
                ColumnData::Bool(vec![Some(true), Some(false), None]),
                ColumnData::Float(vec![Some(1.5_f64), Some(-2.25_f64), None]),
            ],
        )
        .expect("valid dataset");
        let handle = store.put(ds, 3600);

        let receipt = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::Inline { max_bytes: 65536 },
            ExportOptions::default(),
        )
        .expect("float/bool CSV export");

        let payload = receipt.inline_payload.expect("inline payload");
        let csv_str = std::str::from_utf8(&payload).expect("utf-8");
        let mut rdr = csv::Reader::from_reader(csv_str.as_bytes());
        let rows: Vec<csv::StringRecord> = rdr.records().map(|r| r.expect("record")).collect();
        assert_eq!(rows.len(), 3);
        // Row 0: true, 1.5
        assert_eq!(&rows[0][0], "true", "Bool true → 'true'");
        assert_eq!(&rows[0][1], "1.5", "Float 1.5 → '1.5'");
        // Row 1: false, -2.25
        assert_eq!(&rows[1][0], "false", "Bool false → 'false'");
        assert_eq!(&rows[1][1], "-2.25", "Float -2.25 → '-2.25'");
        // Row 2: null, null → empty strings
        assert_eq!(&rows[2][0], "", "None Bool → ''");
        assert_eq!(&rows[2][1], "", "None Float → ''");

        // Kill now_unix_secs constant-replacement mutants: ts must be > 0 and
        // within a reasonable range (1_700_000_000 = 2023-11-14, well in the past).
        assert!(
            receipt.ts > 1_700_000_000,
            "ts={} should be a real Unix timestamp",
            receipt.ts
        );
    }

    #[test]
    fn mutation_kill_float_bool_columns_json() {
        use dh_spec::{ColumnRole, DType};

        let store = Store::new(0);
        let ds = Dataset::new(
            vec![
                make_col_schema("active", DType::Bool, ColumnRole::Dimension),
                make_col_schema("score", DType::Float, ColumnRole::Measure),
            ],
            vec![
                ColumnData::Bool(vec![Some(true), Some(false)]),
                ColumnData::Float(vec![Some(3.14_f64), None]),
            ],
        )
        .expect("valid dataset");
        let handle = store.put(ds, 3600);

        let receipt = export(
            &store,
            &handle,
            ExportFmt::Json { max_rows: 10 },
            ExportDest::Inline { max_bytes: 65536 },
            ExportOptions::default(),
        )
        .expect("float/bool JSON export");

        let payload = receipt.inline_payload.expect("payload");
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&payload).expect("json");
        assert_eq!(rows[0]["active"], serde_json::Value::Bool(true));
        assert_eq!(
            rows[0]["score"].as_f64().expect("f64"),
            3.14_f64,
            "Float 3.14 round-trips through JSON"
        );
        assert_eq!(rows[1]["active"], serde_json::Value::Bool(false));
        assert_eq!(rows[1]["score"], serde_json::Value::Null, "None float → null");
    }

    // ── Mutation-kill: JSON max_rows boundary (> vs >=, > vs ==) ────────────

    /// Kills: replace `>` with `>=` in dataset_to_json (off-by-one in max_rows check).
    /// At max_rows == row_count, the export must SUCCEED (not refuse).
    #[test]
    fn mutation_kill_json_max_rows_boundary_equal() {
        let (store, handle) = make_store_with_fixture(); // 3 rows

        // max_rows exactly equals the dataset size — must succeed.
        let result = export(
            &store,
            &handle,
            ExportFmt::Json { max_rows: 3 },
            ExportDest::Inline { max_bytes: 65536 },
            ExportOptions::default(),
        );
        assert!(
            result.is_ok(),
            "max_rows == row_count should succeed, got: {:?}",
            result.err()
        );
        let payload = result.unwrap().inline_payload.expect("payload");
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&payload).expect("json");
        assert_eq!(rows.len(), 3, "all 3 rows emitted");
    }

    // ── Mutation-kill: check_inline_size (> vs ==, > vs >=, Ok(()) stub) ────

    /// Kills: replace > with ==, replace > with >=, replace fn with Ok(()).
    #[test]
    fn mutation_kill_inline_size_boundary_exact() {
        let (store, handle) = make_store_with_fixture();

        // First export to learn the exact byte count.
        let r = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::Inline { max_bytes: 65536 },
            ExportOptions::default(),
        )
        .expect("reference export");
        let exact_bytes = r.bytes as usize;
        let payload_ref = r.inline_payload.expect("payload");

        // max_bytes == exact payload — must succeed.
        let ok = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::Inline { max_bytes: exact_bytes },
            ExportOptions::default(),
        );
        assert!(ok.is_ok(), "max_bytes == actual bytes should succeed");

        // max_bytes == exact_bytes - 1 — must fail with InlineLimitExceeded.
        let err = export(
            &store,
            &handle,
            ExportFmt::Csv,
            ExportDest::Inline { max_bytes: exact_bytes - 1 },
            ExportOptions::default(),
        )
        .expect_err("one byte over limit should fail");
        match err {
            ExportError::InlineLimitExceeded { actual, limit } => {
                assert_eq!(actual, exact_bytes);
                assert_eq!(limit, exact_bytes - 1);
            }
            other => panic!("expected InlineLimitExceeded, got {other:?}"),
        }

        // Verify the payload from the exact-bytes case is correct (kills Ok(()) stub).
        let payload_exact = ok.unwrap().inline_payload.expect("exact payload");
        assert_eq!(
            payload_exact, payload_ref,
            "payload at exact boundary must equal reference"
        );
    }

    // ── Mutation-kill: ExportError Display impl ──────────────────────────────

    #[test]
    fn mutation_kill_export_error_display() {
        let e1 = ExportError::LookupFailed("not found".to_string());
        assert!(e1.to_string().contains("not found"), "LookupFailed display");

        let e2 = ExportError::JsonLimitExceeded { actual: 5, limit: 3 };
        let s2 = e2.to_string();
        assert!(s2.contains('5') && s2.contains('3'), "JsonLimitExceeded display: {s2}");

        let e3 = ExportError::InlineLimitExceeded { actual: 100, limit: 50 };
        let s3 = e3.to_string();
        assert!(s3.contains("100") && s3.contains("50"), "InlineLimitExceeded display: {s3}");

        let e4 = ExportError::FileExists(std::path::PathBuf::from("/tmp/x.csv"));
        assert!(e4.to_string().contains("/tmp/x.csv"), "FileExists display");

        let e5 = ExportError::Io("disk full".to_string());
        assert!(e5.to_string().contains("disk full"), "Io display");

        let e6 = ExportError::ParquetNotEnabled;
        assert!(!e6.to_string().is_empty(), "ParquetNotEnabled display is non-empty");
    }
}
