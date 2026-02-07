use std::path::Path;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, Int64Array, LargeStringArray, StringArray, TimestampMicrosecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use crate::error::Result;
use crate::types::Run;

/// Build the Arrow schema for the runs table (18 columns).
pub fn runs_schema() -> Schema {
    Schema::new(vec![
        Field::new("run_id", DataType::Utf8, false),
        Field::new("parent_run_id", DataType::Utf8, true),
        Field::new("trace_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("run_type", DataType::Utf8, false),
        Field::new("project", DataType::Utf8, false),
        Field::new(
            "start_time",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        Field::new(
            "end_time",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
        Field::new("status", DataType::Utf8, false),
        Field::new("input", DataType::LargeUtf8, false),
        Field::new("output", DataType::LargeUtf8, true),
        Field::new("error", DataType::LargeUtf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new("metadata", DataType::LargeUtf8, false),
        Field::new("input_tokens", DataType::Int64, true),
        Field::new("output_tokens", DataType::Int64, true),
        Field::new("total_tokens", DataType::Int64, true),
        Field::new("latency_ms", DataType::Int64, true),
    ])
}

/// Convert a slice of Runs into an Arrow RecordBatch.
pub fn runs_to_record_batch(runs: &[Run]) -> Result<RecordBatch> {
    let schema = Arc::new(runs_schema());

    let run_ids: ArrayRef = Arc::new(StringArray::from(
        runs.iter().map(|r| r.run_id.to_string()).collect::<Vec<_>>(),
    ));
    let parent_run_ids: ArrayRef = Arc::new(StringArray::from(
        runs.iter()
            .map(|r| r.parent_run_id.map(|id| id.to_string()))
            .collect::<Vec<_>>(),
    ));
    let trace_ids: ArrayRef = Arc::new(StringArray::from(
        runs.iter().map(|r| r.trace_id.to_string()).collect::<Vec<_>>(),
    ));
    let names: ArrayRef = Arc::new(StringArray::from(
        runs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
    ));
    let run_types: ArrayRef = Arc::new(StringArray::from(
        runs.iter().map(|r| r.run_type.as_str()).collect::<Vec<_>>(),
    ));
    let projects: ArrayRef = Arc::new(StringArray::from(
        runs.iter().map(|r| r.project.as_str()).collect::<Vec<_>>(),
    ));
    let start_times: ArrayRef = Arc::new(
        TimestampMicrosecondArray::from(
            runs.iter()
                .map(|r| r.start_time.timestamp_micros())
                .collect::<Vec<_>>(),
        )
        .with_timezone("UTC"),
    );
    let end_times: ArrayRef = Arc::new(
        TimestampMicrosecondArray::from(
            runs.iter()
                .map(|r| r.end_time.map(|t| t.timestamp_micros()))
                .collect::<Vec<_>>(),
        )
        .with_timezone("UTC"),
    );
    let statuses: ArrayRef = Arc::new(StringArray::from(
        runs.iter().map(|r| r.status.as_str()).collect::<Vec<_>>(),
    ));
    let inputs: ArrayRef = Arc::new(LargeStringArray::from(
        runs.iter().map(|r| r.input.as_str()).collect::<Vec<_>>(),
    ));
    let outputs: ArrayRef = Arc::new(LargeStringArray::from(
        runs.iter()
            .map(|r| r.output.as_deref())
            .collect::<Vec<_>>(),
    ));
    let errors: ArrayRef = Arc::new(LargeStringArray::from(
        runs.iter()
            .map(|r| r.error.as_deref())
            .collect::<Vec<_>>(),
    ));
    let tags: ArrayRef = Arc::new(StringArray::from(
        runs.iter()
            .map(|r| serde_json::to_string(&r.tags).unwrap_or_default())
            .collect::<Vec<_>>(),
    ));
    let metadata: ArrayRef = Arc::new(LargeStringArray::from(
        runs.iter().map(|r| r.metadata.as_str()).collect::<Vec<_>>(),
    ));
    let input_tokens: ArrayRef = Arc::new(Int64Array::from(
        runs.iter().map(|r| r.input_tokens).collect::<Vec<_>>(),
    ));
    let output_tokens: ArrayRef = Arc::new(Int64Array::from(
        runs.iter().map(|r| r.output_tokens).collect::<Vec<_>>(),
    ));
    let total_tokens: ArrayRef = Arc::new(Int64Array::from(
        runs.iter().map(|r| r.total_tokens).collect::<Vec<_>>(),
    ));
    let latency_ms: ArrayRef = Arc::new(Int64Array::from(
        runs.iter().map(|r| r.latency_ms).collect::<Vec<_>>(),
    ));

    let batch = RecordBatch::try_new(
        schema,
        vec![
            run_ids,
            parent_run_ids,
            trace_ids,
            names,
            run_types,
            projects,
            start_times,
            end_times,
            statuses,
            inputs,
            outputs,
            errors,
            tags,
            metadata,
            input_tokens,
            output_tokens,
            total_tokens,
            latency_ms,
        ],
    )?;

    Ok(batch)
}

/// Write runs to a Parquet file with ZSTD compression.
pub fn write_runs(runs: &[Run], path: &Path) -> Result<()> {
    if runs.is_empty() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let batch = runs_to_record_batch(runs)?;
    let schema = batch.schema();

    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();

    let file = std::fs::File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;

    Ok(())
}

/// Generate the parquet file path for a batch of runs.
pub fn parquet_path(base_dir: &Path, project: &str, batch_id: &str) -> std::path::PathBuf {
    let date = chrono::Utc::now().format("%Y%m%d");
    base_dir
        .join(project)
        .join(format!("runs_{date}_{batch_id}.parquet"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Run, RunType};

    fn sample_runs(n: usize) -> Vec<Run> {
        (0..n)
            .map(|i| {
                Run::builder(format!("run-{i}"), RunType::Chain)
                    .project("test-project")
                    .input(format!(r#"{{"index": {i}}}"#))
                    .finish_ok(format!(r#"{{"result": {i}}}"#))
            })
            .collect()
    }

    #[test]
    fn schema_has_18_columns() {
        let schema = runs_schema();
        assert_eq!(schema.fields().len(), 18);
    }

    #[test]
    fn schema_column_names() {
        let schema = runs_schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"run_id"));
        assert!(names.contains(&"start_time"));
        assert!(names.contains(&"input_tokens"));
        assert!(names.contains(&"latency_ms"));
    }

    #[test]
    fn runs_to_record_batch_basic() {
        let runs = sample_runs(3);
        let batch = runs_to_record_batch(&runs).unwrap();
        assert_eq!(batch.num_rows(), 3);
        assert_eq!(batch.num_columns(), 18);
    }

    #[test]
    fn runs_to_record_batch_empty() {
        let batch = runs_to_record_batch(&[]).unwrap();
        assert_eq!(batch.num_rows(), 0);
    }

    #[test]
    fn runs_to_record_batch_with_parent() {
        let parent_id = uuid::Uuid::new_v4();
        let trace_id = uuid::Uuid::new_v4();
        let run = Run::builder("child", RunType::Llm)
            .parent_run_id(parent_id)
            .trace_id(trace_id)
            .finish_llm("response", 10, 5, 15);

        let batch = runs_to_record_batch(&[run]).unwrap();
        assert_eq!(batch.num_rows(), 1);

        let parent_col = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(parent_col.value(0), parent_id.to_string());
    }

    #[test]
    fn runs_to_record_batch_null_parent() {
        use arrow::array::Array;

        let run = Run::builder("root", RunType::Chain).finish_ok("done");

        let batch = runs_to_record_batch(&[run]).unwrap();
        let parent_col = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert!(parent_col.is_null(0));
    }

    #[test]
    fn runs_to_record_batch_token_columns() {
        let run = Run::builder("llm", RunType::Llm)
            .finish_llm("output", 100, 50, 150);

        let batch = runs_to_record_batch(&[run]).unwrap();
        let input_tokens = batch
            .column(14)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(input_tokens.value(0), 100);
    }

    #[test]
    fn write_runs_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.parquet");
        let runs = sample_runs(5);
        write_runs(&runs, &path).unwrap();
        assert!(path.exists());
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }

    #[test]
    fn write_runs_empty_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.parquet");
        write_runs(&[], &path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn write_runs_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deep").join("test.parquet");
        let runs = sample_runs(1);
        write_runs(&runs, &path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn parquet_path_format() {
        let base = Path::new("/tmp/smith");
        let path = parquet_path(base, "my-project", "abc123");
        let path_str = path.to_string_lossy();
        assert!(path_str.starts_with("/tmp/smith/my-project/runs_"));
        assert!(path_str.ends_with("_abc123.parquet"));
    }
}
