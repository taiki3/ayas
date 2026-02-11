use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use duckdb::{params, Connection};
use uuid::Uuid;

use crate::error::{Result, SmithError};
use crate::types::{LatencyStats, Run, RunFilter, RunStatus, RunType, TokenUsageSummary};

/// Explicit column list with timestamp casts for reliable reading from DuckDB.
/// DuckDB's Rust bindings may not auto-cast Timestamp columns to String,
/// so we CAST timestamp columns to VARCHAR at the SQL level.
const SELECT_COLUMNS: &str = "\
    run_id, parent_run_id, trace_id, name, run_type, project, \
    CAST(start_time AS VARCHAR) AS start_time, \
    CAST(end_time AS VARCHAR) AS end_time, \
    status, input, output, error, tags, metadata, \
    input_tokens, output_tokens, total_tokens, latency_ms, dotted_order";

/// DuckDB-based query client for reading traced runs from Parquet files.
pub struct SmithQuery {
    conn: Connection,
    base_dir: PathBuf,
}

impl SmithQuery {
    /// Create a new query client.
    pub fn new(base_dir: impl Into<PathBuf>) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        // Disable auto-install to avoid network requests in proxy environments
        conn.execute_batch(
            "SET autoinstall_known_extensions=false; SET autoload_known_extensions=true;"
        )?;
        Ok(Self {
            conn,
            base_dir: base_dir.into(),
        })
    }

    /// Get the glob pattern for Parquet files in a project directory.
    fn parquet_glob(&self, project: &str) -> String {
        let dir = self.base_dir.join(project);
        format!("{}/*.parquet", dir.display())
    }

    /// Check whether any parquet files exist for the given project.
    /// Returns false when the directory doesn't exist or contains no `.parquet` files.
    fn has_parquet_files(&self, project: &str) -> bool {
        let dir = self.base_dir.join(project);
        match std::fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .any(|e| e.path().extension().is_some_and(|ext| ext == "parquet")),
            Err(_) => false,
        }
    }

    /// List runs matching the given filter.
    pub fn list_runs(&self, filter: &RunFilter) -> Result<Vec<Run>> {
        let project = filter.project.as_deref().unwrap_or("default");
        if !self.has_parquet_files(project) {
            return Ok(Vec::new());
        }
        let glob = self.parquet_glob(project);

        let mut conditions = Vec::new();
        let mut param_values: Vec<Box<dyn duckdb::ToSql>> = Vec::new();

        if let Some(ref rt) = filter.run_type {
            conditions.push("run_type = ?".to_string());
            param_values.push(Box::new(rt.as_str().to_string()));
        }
        if let Some(ref status) = filter.status {
            conditions.push("status = ?".to_string());
            param_values.push(Box::new(status.as_str().to_string()));
        }
        if let Some(ref name) = filter.name {
            conditions.push("name = ?".to_string());
            param_values.push(Box::new(name.clone()));
        }
        if let Some(ref trace_id) = filter.trace_id {
            conditions.push("trace_id = ?".to_string());
            param_values.push(Box::new(trace_id.to_string()));
        }
        if let Some(ref parent_id) = filter.parent_run_id {
            conditions.push("parent_run_id = ?".to_string());
            param_values.push(Box::new(parent_id.to_string()));
        }
        if let Some(ref after) = filter.start_after {
            conditions.push("start_time > ?".to_string());
            param_values.push(Box::new(after.to_rfc3339()));
        }
        if let Some(ref before) = filter.start_before {
            conditions.push("start_time < ?".to_string());
            param_values.push(Box::new(before.to_rfc3339()));
        }

        let limit_clause = filter
            .limit
            .map(|l| format!(" LIMIT {l}"))
            .unwrap_or_default();
        let offset_clause = filter
            .offset
            .map(|o| format!(" OFFSET {o}"))
            .unwrap_or_default();

        let sql = format!(
            "WITH deduped AS (\
                SELECT *, ROW_NUMBER() OVER (\
                    PARTITION BY run_id \
                    ORDER BY CASE WHEN status != 'running' THEN 1 ELSE 0 END DESC, \
                             CASE WHEN end_time IS NOT NULL THEN 1 ELSE 0 END DESC\
                ) AS _rn \
                FROM read_parquet('{glob}')\
            ) \
            SELECT {SELECT_COLUMNS} FROM deduped WHERE _rn = 1{and_where_clause} \
            ORDER BY start_time DESC{limit_clause}{offset_clause}",
            and_where_clause = if conditions.is_empty() {
                String::new()
            } else {
                format!(" AND {}", conditions.join(" AND "))
            },
        );

        let params_refs: Vec<&dyn duckdb::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(row_to_run(row))
        })?;

        let mut runs = Vec::new();
        for row in rows {
            match row {
                Ok(run) => runs.push(run),
                Err(e) => return Err(SmithError::DuckDb(e)),
            }
        }
        Ok(runs)
    }

    /// Get a single run by its ID (deduplicated: prefers completed over running).
    pub fn get_run(&self, run_id: Uuid, project: &str) -> Result<Option<Run>> {
        if !self.has_parquet_files(project) {
            return Ok(None);
        }
        let glob = self.parquet_glob(project);
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM read_parquet('{glob}') \
             WHERE run_id = ? \
             ORDER BY CASE WHEN status != 'running' THEN 1 ELSE 0 END DESC, \
                      CASE WHEN end_time IS NOT NULL THEN 1 ELSE 0 END DESC \
             LIMIT 1"
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![run_id.to_string()], |row| {
            Ok(row_to_run(row))
        })?;

        match rows.next() {
            Some(Ok(run)) => Ok(Some(run)),
            Some(Err(e)) => Err(SmithError::DuckDb(e)),
            None => Ok(None),
        }
    }

    /// Get all runs belonging to a trace.
    pub fn get_trace(&self, trace_id: Uuid, project: &str) -> Result<Vec<Run>> {
        let filter = RunFilter {
            project: Some(project.into()),
            trace_id: Some(trace_id),
            ..Default::default()
        };
        self.list_runs(&filter)
    }

    /// Get child runs of a given parent run.
    pub fn get_children(&self, parent_run_id: Uuid, project: &str) -> Result<Vec<Run>> {
        let filter = RunFilter {
            project: Some(project.into()),
            parent_run_id: Some(parent_run_id),
            ..Default::default()
        };
        self.list_runs(&filter)
    }

    /// Get token usage summary for runs matching the filter.
    pub fn token_usage_summary(&self, filter: &RunFilter) -> Result<TokenUsageSummary> {
        let project = filter.project.as_deref().unwrap_or("default");
        if !self.has_parquet_files(project) {
            return Ok(TokenUsageSummary::default());
        }
        let glob = self.parquet_glob(project);

        let mut conditions = vec!["run_type = 'llm'".to_string()];
        if let Some(ref name) = filter.name {
            conditions.push(format!("name = '{name}'"));
        }
        if let Some(ref trace_id) = filter.trace_id {
            conditions.push(format!("trace_id = '{trace_id}'"));
        }

        let where_clause = format!(" WHERE {}", conditions.join(" AND "));

        let sql = format!(
            "SELECT COALESCE(SUM(input_tokens), 0) as total_input, \
             COALESCE(SUM(output_tokens), 0) as total_output, \
             COALESCE(SUM(total_tokens), 0) as total, \
             COUNT(*) as cnt \
             FROM read_parquet('{glob}'){where_clause}"
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map([], |row| {
            Ok(TokenUsageSummary {
                total_input_tokens: row.get::<_, i64>(0)?,
                total_output_tokens: row.get::<_, i64>(1)?,
                total_tokens: row.get::<_, i64>(2)?,
                run_count: row.get::<_, i64>(3)?,
            })
        })?;

        match rows.next() {
            Some(Ok(summary)) => Ok(summary),
            Some(Err(e)) => Err(SmithError::DuckDb(e)),
            None => Ok(TokenUsageSummary::default()),
        }
    }

    /// Get latency percentiles for runs matching the filter.
    pub fn latency_percentiles(&self, filter: &RunFilter) -> Result<LatencyStats> {
        let project = filter.project.as_deref().unwrap_or("default");
        if !self.has_parquet_files(project) {
            return Ok(LatencyStats::default());
        }
        let glob = self.parquet_glob(project);

        let mut conditions: Vec<String> = vec!["latency_ms IS NOT NULL".into()];
        if let Some(ref rt) = filter.run_type {
            conditions.push(format!("run_type = '{}'", rt.as_str()));
        }
        if let Some(ref name) = filter.name {
            conditions.push(format!("name = '{name}'"));
        }

        let where_clause = format!(" WHERE {}", conditions.join(" AND "));

        let sql = format!(
            "SELECT \
             COALESCE(percentile_cont(0.5) WITHIN GROUP (ORDER BY latency_ms), 0) as p50, \
             COALESCE(percentile_cont(0.9) WITHIN GROUP (ORDER BY latency_ms), 0) as p90, \
             COALESCE(percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms), 0) as p95, \
             COALESCE(percentile_cont(0.99) WITHIN GROUP (ORDER BY latency_ms), 0) as p99 \
             FROM read_parquet('{glob}'){where_clause}"
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map([], |row| {
            Ok(LatencyStats {
                p50: row.get::<_, f64>(0)?,
                p90: row.get::<_, f64>(1)?,
                p95: row.get::<_, f64>(2)?,
                p99: row.get::<_, f64>(3)?,
            })
        })?;

        match rows.next() {
            Some(Ok(stats)) => Ok(stats),
            Some(Err(e)) => Err(SmithError::DuckDb(e)),
            None => Ok(LatencyStats::default()),
        }
    }

    /// Execute a raw SQL query and return results as JSON strings.
    pub fn raw_query(&self, sql: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(sql)?;

        // Execute first to make column metadata available
        let mut rows = stmt.query([])?;
        let column_count = rows.as_ref().unwrap().column_count();
        let column_names: Vec<String> = (0..column_count)
            .map(|i| {
                rows.as_ref()
                    .unwrap()
                    .column_name(i)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| format!("col{i}"))
            })
            .collect();

        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            let mut map = serde_json::Map::new();
            for (i, name) in column_names.iter().enumerate() {
                let val: duckdb::Result<String> = row.get(i);
                match val {
                    Ok(s) => {
                        map.insert(name.clone(), serde_json::Value::String(s));
                    }
                    Err(_) => {
                        map.insert(name.clone(), serde_json::Value::Null);
                    }
                }
            }
            results.push(
                serde_json::to_string(&serde_json::Value::Object(map)).unwrap_or_default(),
            );
        }
        Ok(results)
    }
}

fn row_to_run(row: &duckdb::Row<'_>) -> Run {
    let run_id_str: String = row.get(0).unwrap_or_default();
    let parent_run_id_str: Option<String> = row.get(1).ok();
    let trace_id_str: String = row.get(2).unwrap_or_default();
    let name: String = row.get(3).unwrap_or_default();
    let run_type_str: String = row.get(4).unwrap_or_default();
    let project: String = row.get(5).unwrap_or_default();

    // DuckDB returns timestamps as strings when read from parquet
    let start_time_str: String = row.get::<_, String>(6).unwrap_or_default();
    let end_time_str: Option<String> = row.get(7).ok();

    let status_str: String = row.get(8).unwrap_or_default();
    let input: String = row.get(9).unwrap_or_default();
    let output: Option<String> = row.get(10).ok();
    let error: Option<String> = row.get(11).ok();
    let tags_str: String = row.get(12).unwrap_or_default();
    let metadata: String = row.get(13).unwrap_or_default();
    let input_tokens: Option<i64> = row.get(14).ok();
    let output_tokens: Option<i64> = row.get(15).ok();
    let total_tokens: Option<i64> = row.get(16).ok();
    let latency_ms: Option<i64> = row.get(17).ok();
    let dotted_order: Option<String> = row.get(18).ok();

    let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();

    Run {
        run_id: run_id_str.parse().unwrap_or_default(),
        parent_run_id: parent_run_id_str
            .as_deref()
            .and_then(|s| s.parse().ok()),
        trace_id: trace_id_str.parse().unwrap_or_default(),
        name,
        run_type: run_type_str.parse().unwrap_or(RunType::Chain),
        project,
        start_time: parse_timestamp(&start_time_str),
        end_time: end_time_str.as_deref().map(parse_timestamp),
        status: status_str.parse().unwrap_or(RunStatus::Success),
        input,
        output,
        error,
        tags,
        metadata,
        input_tokens,
        output_tokens,
        total_tokens,
        latency_ms,
        dotted_order,
    }
}

fn parse_timestamp(s: &str) -> DateTime<Utc> {
    // Try parsing as RFC3339 first, then fall back to common DuckDB timestamp formats
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt.with_timezone(&Utc);
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
        return Utc.from_utc_datetime(&dt);
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Utc.from_utc_datetime(&dt);
    }
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::client::flush_runs;
    use crate::types::{Run, RunType};

    fn create_test_runs(dir: &Path) -> Vec<Run> {
        let trace_id = Uuid::new_v4();
        let root_id = Uuid::new_v4();

        let root = {
            let mut run = Run::builder("my-chain", RunType::Chain)
                .run_id(root_id)
                .trace_id(trace_id)
                .project("test-proj")
                .input(r#"{"query": "hello"}"#)
                .finish_ok(r#"{"answer": "world"}"#);
            run.trace_id = trace_id;
            run.run_id = root_id;
            run
        };

        let child_llm = {
            let mut run = Run::builder("gpt-4o", RunType::Llm)
                .parent_run_id(root_id)
                .trace_id(trace_id)
                .project("test-proj")
                .finish_llm(r#""Hello!""#, 50, 10, 60);
            run.trace_id = trace_id;
            run
        };

        let child_tool = {
            let mut run = Run::builder("calculator", RunType::Tool)
                .parent_run_id(root_id)
                .trace_id(trace_id)
                .project("test-proj")
                .input(r#"{"expression": "2+3"}"#)
                .finish_ok("5");
            run.trace_id = trace_id;
            run
        };

        let runs = vec![root, child_llm, child_tool];
        flush_runs(&runs, dir, "test-proj").unwrap();
        runs
    }

    #[test]
    fn query_client_creation() {
        let dir = tempfile::tempdir().unwrap();
        let client = SmithQuery::new(dir.path()).unwrap();
        assert_eq!(client.base_dir, dir.path());
    }

    #[test]
    fn list_runs_all() {
        let dir = tempfile::tempdir().unwrap();
        let written = create_test_runs(dir.path());

        let client = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("test-proj".into()),
            ..Default::default()
        };
        let runs = client.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), written.len());
    }

    #[test]
    fn list_runs_by_type() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());

        let client = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("test-proj".into()),
            run_type: Some(RunType::Llm),
            ..Default::default()
        };
        let runs = client.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].name, "gpt-4o");
    }

    #[test]
    fn list_runs_by_name() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());

        let client = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("test-proj".into()),
            name: Some("calculator".into()),
            ..Default::default()
        };
        let runs = client.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_type, RunType::Tool);
    }

    #[test]
    fn list_runs_with_limit() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());

        let client = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("test-proj".into()),
            limit: Some(1),
            ..Default::default()
        };
        let runs = client.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), 1);
    }

    #[test]
    fn get_run_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let written = create_test_runs(dir.path());
        let target_id = written[0].run_id;

        let client = SmithQuery::new(dir.path()).unwrap();
        let run = client.get_run(target_id, "test-proj").unwrap();
        assert!(run.is_some());
        assert_eq!(run.unwrap().run_id, target_id);
    }

    #[test]
    fn get_run_not_found() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());

        let client = SmithQuery::new(dir.path()).unwrap();
        let run = client.get_run(Uuid::new_v4(), "test-proj").unwrap();
        assert!(run.is_none());
    }

    #[test]
    fn get_trace() {
        let dir = tempfile::tempdir().unwrap();
        let written = create_test_runs(dir.path());
        let trace_id = written[0].trace_id;

        let client = SmithQuery::new(dir.path()).unwrap();
        let runs = client.get_trace(trace_id, "test-proj").unwrap();
        assert_eq!(runs.len(), 3);
        for run in &runs {
            assert_eq!(run.trace_id, trace_id);
        }
    }

    #[test]
    fn get_children() {
        let dir = tempfile::tempdir().unwrap();
        let written = create_test_runs(dir.path());
        let parent_id = written[0].run_id;

        let client = SmithQuery::new(dir.path()).unwrap();
        let children = client.get_children(parent_id, "test-proj").unwrap();
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn token_usage_summary_query() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());

        let client = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("test-proj".into()),
            ..Default::default()
        };
        let summary = client.token_usage_summary(&filter).unwrap();
        assert_eq!(summary.total_input_tokens, 50);
        assert_eq!(summary.total_output_tokens, 10);
        assert_eq!(summary.total_tokens, 60);
        assert_eq!(summary.run_count, 1);
    }

    #[test]
    fn latency_percentiles_query() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());

        let client = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("test-proj".into()),
            ..Default::default()
        };
        let stats = client.latency_percentiles(&filter).unwrap();
        // All runs have latency >= 0
        assert!(stats.p50 >= 0.0);
        assert!(stats.p90 >= stats.p50);
    }

    #[test]
    fn raw_query_basic() {
        let client = SmithQuery::new("/tmp/nonexistent").unwrap();
        let results = client.raw_query("SELECT 1 as num, 'hello' as msg").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("hello"));
    }
}
