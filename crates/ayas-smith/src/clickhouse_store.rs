use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use uuid::Uuid;

use crate::error::SmithError;
use crate::store::SmithStore;
use crate::types::{
    Dataset, Example, Feedback, FeedbackFilter, LatencyStats, Project, Run, RunFilter, RunPatch,
    RunStatus, RunType, TokenUsageSummary,
};

/// ClickHouse-backed SmithStore using the HTTP API.
///
/// Feature-gated behind `clickhouse` feature flag.
pub struct ClickHouseStore {
    client: Client,
    base_url: String,
    database: String,
    user: String,
    password: String,
}

impl Default for ClickHouseStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ClickHouseStore {
    /// Create a new ClickHouse store.
    /// URL defaults to `CLICKHOUSE_URL` env var, falling back to `http://localhost:8123`.
    pub fn new() -> Self {
        let base_url =
            std::env::var("CLICKHOUSE_URL").unwrap_or_else(|_| "http://localhost:8123".into());
        let database =
            std::env::var("CLICKHOUSE_DATABASE").unwrap_or_else(|_| "default".into());
        let user =
            std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".into());
        let password =
            std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default();
        Self {
            client: Client::new(),
            base_url,
            database,
            user,
            password,
        }
    }

    pub fn with_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    pub fn with_database(mut self, db: String) -> Self {
        self.database = db;
        self
    }

    pub fn with_user(mut self, user: String) -> Self {
        self.user = user;
        self
    }

    pub fn with_password(mut self, password: String) -> Self {
        self.password = password;
        self
    }

    /// Execute a query and return the response body.
    async fn query(&self, sql: &str) -> Result<String, SmithError> {
        let resp = self
            .client
            .post(&self.base_url)
            .basic_auth(&self.user, Some(&self.password))
            .query(&[("database", &self.database)])
            .body(sql.to_string())
            .send()
            .await
            .map_err(|e| SmithError::Query(format!("ClickHouse request error: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SmithError::Query(format!(
                "ClickHouse query error: {body}"
            )));
        }

        resp.text()
            .await
            .map_err(|e| SmithError::Query(format!("ClickHouse response error: {e}")))
    }

    /// Execute an INSERT with body data.
    async fn insert(&self, query_param: &str, body: String) -> Result<(), SmithError> {
        let resp = self
            .client
            .post(&self.base_url)
            .basic_auth(&self.user, Some(&self.password))
            .query(&[
                ("database", self.database.as_str()),
                ("query", query_param),
            ])
            .body(body)
            .send()
            .await
            .map_err(|e| SmithError::Query(format!("ClickHouse insert error: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SmithError::Query(format!(
                "ClickHouse insert error: {body}"
            )));
        }

        Ok(())
    }

    /// Ensure tables exist.
    pub async fn create_tables(&self) -> Result<(), SmithError> {
        self.query(
            "CREATE TABLE IF NOT EXISTS runs (
                run_id UUID,
                parent_run_id Nullable(UUID),
                trace_id UUID,
                name String,
                run_type String,
                project String,
                start_time DateTime64(3),
                end_time Nullable(DateTime64(3)),
                status String,
                input String,
                output Nullable(String),
                error Nullable(String),
                tags Array(String),
                metadata String,
                input_tokens Nullable(Int64),
                output_tokens Nullable(Int64),
                total_tokens Nullable(Int64),
                latency_ms Nullable(Int64),
                dotted_order Nullable(String),
                version UInt32 DEFAULT 1
            ) ENGINE = ReplacingMergeTree(version)
            ORDER BY (project, start_time, run_id)",
        )
        .await?;

        self.query(
            "CREATE TABLE IF NOT EXISTS feedback (
                id UUID,
                run_id UUID,
                key String,
                score Float64,
                comment Nullable(String),
                created_at DateTime64(3)
            ) ENGINE = MergeTree()
            ORDER BY (run_id, created_at, id)",
        )
        .await?;

        self.query(
            "CREATE TABLE IF NOT EXISTS projects (
                id UUID,
                name String,
                description Nullable(String),
                created_at DateTime64(3)
            ) ENGINE = MergeTree()
            ORDER BY (created_at, id)",
        )
        .await?;

        self.query(
            "CREATE TABLE IF NOT EXISTS datasets (
                id UUID,
                name String,
                description Nullable(String),
                project_id Nullable(UUID),
                created_at DateTime64(3)
            ) ENGINE = MergeTree()
            ORDER BY (created_at, id)",
        )
        .await?;

        self.query(
            "CREATE TABLE IF NOT EXISTS examples (
                id UUID,
                dataset_id UUID,
                input String,
                output Nullable(String),
                metadata Nullable(String),
                created_at DateTime64(3)
            ) ENGINE = MergeTree()
            ORDER BY (dataset_id, created_at, id)",
        )
        .await?;

        Ok(())
    }

    fn escape_string(s: &str) -> String {
        s.replace('\\', "\\\\").replace('\'', "\\'")
    }
}

#[async_trait]
impl SmithStore for ClickHouseStore {
    async fn init(&self) -> Result<(), SmithError> {
        self.create_tables().await
    }

    async fn put_runs(&self, runs: &[Run]) -> Result<(), SmithError> {
        if runs.is_empty() {
            return Ok(());
        }

        let mut rows = Vec::new();
        for run in runs {
            let row = serde_json::json!({
                "run_id": run.run_id.to_string(),
                "parent_run_id": run.parent_run_id.map(|id| id.to_string()),
                "trace_id": run.trace_id.to_string(),
                "name": run.name,
                "run_type": run.run_type.as_str(),
                "project": run.project,
                "start_time": run.start_time.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
                "end_time": run.end_time.map(|t| t.format("%Y-%m-%d %H:%M:%S%.3f").to_string()),
                "status": run.status.as_str(),
                "input": run.input,
                "output": run.output,
                "error": run.error,
                "tags": run.tags,
                "metadata": run.metadata,
                "input_tokens": run.input_tokens,
                "output_tokens": run.output_tokens,
                "total_tokens": run.total_tokens,
                "latency_ms": run.latency_ms,
                "dotted_order": run.dotted_order,
                "version": 1u32,
            });
            rows.push(row.to_string());
        }

        let body = rows.join("\n");
        self.insert("INSERT INTO runs FORMAT JSONEachRow", body).await
    }

    async fn patch_run(
        &self,
        run_id: Uuid,
        _project: &str,
        patch: &RunPatch,
    ) -> Result<(), SmithError> {
        // ReplacingMergeTree: read existing row, apply patch, insert new row with version+1
        let sql = format!(
            "SELECT *, version FROM runs FINAL WHERE run_id = '{}' FORMAT JSONEachRow",
            run_id
        );
        let body = self.query(&sql).await?;
        let line = body.trim();
        if line.is_empty() {
            return Err(SmithError::Query(format!("Run {run_id} not found")));
        }

        let parsed: serde_json::Value = serde_json::from_str(line)
            .map_err(SmithError::Serialization)?;

        let old_version: u32 = parsed["version"]
            .as_u64()
            .or_else(|| parsed["version"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(1) as u32;

        // Build patched row
        let end_time = if let Some(t) = patch.end_time {
            Some(t.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
        } else {
            parsed["end_time"].as_str().map(String::from)
        };

        let output = patch
            .output
            .as_deref()
            .or(parsed["output"].as_str())
            .map(String::from);

        let error = patch
            .error
            .as_deref()
            .or(parsed["error"].as_str())
            .map(String::from);

        let status = patch
            .status
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| {
                parsed["status"]
                    .as_str()
                    .unwrap_or("success")
                    .to_string()
            });

        let input_tokens = patch.input_tokens.or_else(|| ch_opt_i64(&parsed["input_tokens"]));
        let output_tokens = patch.output_tokens.or_else(|| ch_opt_i64(&parsed["output_tokens"]));
        let total_tokens = patch.total_tokens.or_else(|| ch_opt_i64(&parsed["total_tokens"]));
        let latency_ms = patch.latency_ms.or_else(|| ch_opt_i64(&parsed["latency_ms"]));

        let new_row = serde_json::json!({
            "run_id": parsed["run_id"],
            "parent_run_id": parsed["parent_run_id"],
            "trace_id": parsed["trace_id"],
            "name": parsed["name"],
            "run_type": parsed["run_type"],
            "project": parsed["project"],
            "start_time": parsed["start_time"],
            "end_time": end_time,
            "status": status,
            "input": parsed["input"],
            "output": output,
            "error": error,
            "tags": parsed["tags"],
            "metadata": parsed["metadata"],
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": total_tokens,
            "latency_ms": latency_ms,
            "dotted_order": parsed["dotted_order"],
            "version": old_version + 1,
        });

        self.insert("INSERT INTO runs FORMAT JSONEachRow", new_row.to_string())
            .await
    }

    async fn list_runs(&self, filter: &RunFilter) -> Result<Vec<Run>, SmithError> {
        let mut conditions = Vec::new();

        if let Some(ref project) = filter.project {
            conditions.push(format!("project = '{}'", Self::escape_string(project)));
        }
        if let Some(run_type) = filter.run_type {
            conditions.push(format!("run_type = '{}'", run_type.as_str()));
        }
        if let Some(status) = filter.status {
            conditions.push(format!("status = '{}'", status.as_str()));
        }
        if let Some(ref start_after) = filter.start_after {
            conditions.push(format!(
                "start_time > '{}'",
                start_after.format("%Y-%m-%d %H:%M:%S%.3f")
            ));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let limit = filter.limit.unwrap_or(100);
        let offset = filter.offset.unwrap_or(0);

        let sql = format!(
            "SELECT * FROM runs FINAL {where_clause}
             ORDER BY start_time DESC
             LIMIT {limit} OFFSET {offset}
             FORMAT JSONEachRow"
        );

        let body = self.query(&sql).await?;
        parse_json_each_row_runs(&body)
    }

    async fn get_run(&self, run_id: Uuid, _project: &str) -> Result<Option<Run>, SmithError> {
        let sql = format!(
            "SELECT * FROM runs FINAL WHERE run_id = '{}' FORMAT JSONEachRow",
            run_id
        );
        let body = self.query(&sql).await?;
        let runs = parse_json_each_row_runs(&body)?;
        Ok(runs.into_iter().next())
    }

    async fn get_trace(&self, trace_id: Uuid, _project: &str) -> Result<Vec<Run>, SmithError> {
        let sql = format!(
            "SELECT * FROM runs FINAL WHERE trace_id = '{}' ORDER BY start_time ASC FORMAT JSONEachRow",
            trace_id
        );
        let body = self.query(&sql).await?;
        parse_json_each_row_runs(&body)
    }

    async fn get_children(
        &self,
        parent_run_id: Uuid,
        _project: &str,
    ) -> Result<Vec<Run>, SmithError> {
        let sql = format!(
            "SELECT * FROM runs FINAL WHERE parent_run_id = '{}' ORDER BY start_time ASC FORMAT JSONEachRow",
            parent_run_id
        );
        let body = self.query(&sql).await?;
        parse_json_each_row_runs(&body)
    }

    async fn token_usage_summary(
        &self,
        filter: &RunFilter,
    ) -> Result<TokenUsageSummary, SmithError> {
        let project = filter.project.as_deref().unwrap_or("default");
        let sql = format!(
            "SELECT
                ifNull(sum(input_tokens), 0) as total_input,
                ifNull(sum(output_tokens), 0) as total_output,
                ifNull(sum(total_tokens), 0) as total,
                count() as run_count
             FROM runs FINAL
             WHERE project = '{}'
             FORMAT JSONEachRow",
            Self::escape_string(project)
        );

        let body = self.query(&sql).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(body.trim()).unwrap_or(serde_json::json!({}));

        Ok(TokenUsageSummary {
            total_input_tokens: ch_i64(&parsed["total_input"]),
            total_output_tokens: ch_i64(&parsed["total_output"]),
            total_tokens: ch_i64(&parsed["total"]),
            run_count: ch_i64(&parsed["run_count"]),
        })
    }

    async fn latency_percentiles(&self, filter: &RunFilter) -> Result<LatencyStats, SmithError> {
        let project = filter.project.as_deref().unwrap_or("default");
        let sql = format!(
            "SELECT
                quantile(0.5)(latency_ms) as p50,
                quantile(0.9)(latency_ms) as p90,
                quantile(0.95)(latency_ms) as p95,
                quantile(0.99)(latency_ms) as p99
             FROM runs FINAL
             WHERE project = '{}' AND latency_ms IS NOT NULL
             FORMAT JSONEachRow",
            Self::escape_string(project)
        );

        let body = self.query(&sql).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(body.trim()).unwrap_or(serde_json::json!({}));

        Ok(LatencyStats {
            p50: ch_f64(&parsed["p50"]),
            p90: ch_f64(&parsed["p90"]),
            p95: ch_f64(&parsed["p95"]),
            p99: ch_f64(&parsed["p99"]),
        })
    }

    async fn put_feedback(&self, feedback: &Feedback) -> Result<(), SmithError> {
        let row = serde_json::json!({
            "id": feedback.id.to_string(),
            "run_id": feedback.run_id.to_string(),
            "key": feedback.key,
            "score": feedback.score,
            "comment": feedback.comment,
            "created_at": feedback.created_at.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        });

        self.insert("INSERT INTO feedback FORMAT JSONEachRow", row.to_string())
            .await
    }

    async fn list_feedback(&self, filter: &FeedbackFilter) -> Result<Vec<Feedback>, SmithError> {
        let mut conditions = Vec::new();

        if let Some(run_id) = filter.run_id {
            conditions.push(format!("run_id = '{run_id}'"));
        }
        if let Some(ref key) = filter.key {
            conditions.push(format!("key = '{}'", Self::escape_string(key)));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT * FROM feedback {where_clause} ORDER BY created_at DESC FORMAT JSONEachRow"
        );

        let body = self.query(&sql).await?;
        let mut feedbacks = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parsed: serde_json::Value =
                serde_json::from_str(line).map_err(SmithError::Serialization)?;

            feedbacks.push(Feedback {
                id: parse_uuid(&parsed["id"])?,
                run_id: parse_uuid(&parsed["run_id"])?,
                key: parsed["key"].as_str().unwrap_or("").to_string(),
                score: parsed["score"].as_f64().unwrap_or(0.0),
                comment: parsed["comment"].as_str().map(String::from),
                created_at: ch_datetime(&parsed["created_at"]).unwrap_or_else(Utc::now),
            });
        }
        Ok(feedbacks)
    }

    // --- Project management ---

    async fn create_project(&self, project: &Project) -> Result<(), SmithError> {
        let row = serde_json::json!({
            "id": project.id.to_string(),
            "name": project.name,
            "description": project.description,
            "created_at": project.created_at.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        });
        self.insert("INSERT INTO projects FORMAT JSONEachRow", row.to_string())
            .await
    }

    async fn list_projects(&self) -> Result<Vec<Project>, SmithError> {
        let sql = "SELECT * FROM projects ORDER BY created_at DESC FORMAT JSONEachRow";
        let body = self.query(sql).await?;
        parse_json_each_row_projects(&body)
    }

    async fn get_project(&self, id: Uuid) -> Result<Option<Project>, SmithError> {
        let sql = format!(
            "SELECT * FROM projects WHERE id = '{}' FORMAT JSONEachRow",
            id
        );
        let body = self.query(&sql).await?;
        let projects = parse_json_each_row_projects(&body)?;
        Ok(projects.into_iter().next())
    }

    async fn delete_project(&self, id: Uuid) -> Result<(), SmithError> {
        let sql = format!("ALTER TABLE projects DELETE WHERE id = '{}'", id);
        self.query(&sql).await?;
        Ok(())
    }

    // --- Dataset management ---

    async fn create_dataset(&self, dataset: &Dataset) -> Result<(), SmithError> {
        let row = serde_json::json!({
            "id": dataset.id.to_string(),
            "name": dataset.name,
            "description": dataset.description,
            "project_id": dataset.project_id.map(|id| id.to_string()),
            "created_at": dataset.created_at.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        });
        self.insert("INSERT INTO datasets FORMAT JSONEachRow", row.to_string())
            .await
    }

    async fn list_datasets(&self, project_id: Option<Uuid>) -> Result<Vec<Dataset>, SmithError> {
        let where_clause = if let Some(pid) = project_id {
            format!("WHERE project_id = '{pid}'")
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT * FROM datasets {where_clause} ORDER BY created_at DESC FORMAT JSONEachRow"
        );
        let body = self.query(&sql).await?;
        parse_json_each_row_datasets(&body)
    }

    async fn add_examples(&self, examples: &[Example]) -> Result<(), SmithError> {
        if examples.is_empty() {
            return Ok(());
        }

        let mut rows = Vec::new();
        for ex in examples {
            let row = serde_json::json!({
                "id": ex.id.to_string(),
                "dataset_id": ex.dataset_id.to_string(),
                "input": ex.input,
                "output": ex.output,
                "metadata": ex.metadata,
                "created_at": ex.created_at.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            });
            rows.push(row.to_string());
        }

        let body = rows.join("\n");
        self.insert("INSERT INTO examples FORMAT JSONEachRow", body)
            .await
    }

    async fn list_examples(&self, dataset_id: Uuid) -> Result<Vec<Example>, SmithError> {
        let sql = format!(
            "SELECT * FROM examples WHERE dataset_id = '{}' ORDER BY created_at ASC FORMAT JSONEachRow",
            dataset_id
        );
        let body = self.query(&sql).await?;
        parse_json_each_row_examples(&body)
    }
}

fn parse_json_each_row_runs(body: &str) -> Result<Vec<Run>, SmithError> {
    let mut runs = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: serde_json::Value =
            serde_json::from_str(line).map_err(SmithError::Serialization)?;

        let run_type: RunType = parsed["run_type"]
            .as_str()
            .unwrap_or("chain")
            .parse()
            .map_err(|e: String| SmithError::Query(e))?;

        let status: RunStatus = parsed["status"]
            .as_str()
            .unwrap_or("success")
            .parse()
            .map_err(|e: String| SmithError::Query(e))?;

        runs.push(Run {
            run_id: parse_uuid(&parsed["run_id"])?,
            parent_run_id: parsed["parent_run_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .and_then(|s| Uuid::parse_str(s).ok()),
            trace_id: parse_uuid(&parsed["trace_id"])?,
            name: parsed["name"].as_str().unwrap_or("").to_string(),
            run_type,
            project: parsed["project"]
                .as_str()
                .unwrap_or("default")
                .to_string(),
            start_time: ch_datetime(&parsed["start_time"]).unwrap_or_else(Utc::now),
            end_time: ch_datetime(&parsed["end_time"]),
            status,
            input: parsed["input"].as_str().unwrap_or("{}").to_string(),
            output: parsed["output"].as_str().map(String::from),
            error: parsed["error"].as_str().map(String::from),
            tags: parsed["tags"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            metadata: parsed["metadata"].as_str().unwrap_or("{}").to_string(),
            input_tokens: ch_opt_i64(&parsed["input_tokens"]),
            output_tokens: ch_opt_i64(&parsed["output_tokens"]),
            total_tokens: ch_opt_i64(&parsed["total_tokens"]),
            latency_ms: ch_opt_i64(&parsed["latency_ms"]),
            dotted_order: parsed["dotted_order"].as_str().map(String::from),
        });
    }
    Ok(runs)
}

fn parse_json_each_row_projects(body: &str) -> Result<Vec<Project>, SmithError> {
    let mut projects = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: serde_json::Value =
            serde_json::from_str(line).map_err(SmithError::Serialization)?;

        projects.push(Project {
            id: parse_uuid(&parsed["id"])?,
            name: parsed["name"].as_str().unwrap_or("").to_string(),
            description: parsed["description"].as_str().map(String::from),
            created_at: ch_datetime(&parsed["created_at"]).unwrap_or_else(Utc::now),
        });
    }
    Ok(projects)
}

fn parse_json_each_row_datasets(body: &str) -> Result<Vec<Dataset>, SmithError> {
    let mut datasets = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: serde_json::Value =
            serde_json::from_str(line).map_err(SmithError::Serialization)?;

        datasets.push(Dataset {
            id: parse_uuid(&parsed["id"])?,
            name: parsed["name"].as_str().unwrap_or("").to_string(),
            description: parsed["description"].as_str().map(String::from),
            project_id: parsed["project_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .and_then(|s| Uuid::parse_str(s).ok()),
            created_at: ch_datetime(&parsed["created_at"]).unwrap_or_else(Utc::now),
        });
    }
    Ok(datasets)
}

fn parse_json_each_row_examples(body: &str) -> Result<Vec<Example>, SmithError> {
    let mut examples = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: serde_json::Value =
            serde_json::from_str(line).map_err(SmithError::Serialization)?;

        examples.push(Example {
            id: parse_uuid(&parsed["id"])?,
            dataset_id: parse_uuid(&parsed["dataset_id"])?,
            input: parsed["input"].as_str().unwrap_or("{}").to_string(),
            output: parsed["output"].as_str().map(String::from),
            metadata: parsed["metadata"].as_str().map(String::from),
            created_at: ch_datetime(&parsed["created_at"]).unwrap_or_else(Utc::now),
        });
    }
    Ok(examples)
}

fn parse_uuid(value: &serde_json::Value) -> Result<Uuid, SmithError> {
    let s = value.as_str().unwrap_or("");
    Uuid::parse_str(s).map_err(|e| SmithError::Query(format!("Invalid UUID '{s}': {e}")))
}

/// Parse a ClickHouse JSON value as i64.
/// ClickHouse JSONEachRow returns numbers as strings (e.g., `"300"` not `300`).
fn ch_i64(v: &serde_json::Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

/// Parse an optional i64 from ClickHouse JSON (may be string, number, or null).
fn ch_opt_i64(v: &serde_json::Value) -> Option<i64> {
    if v.is_null() {
        return None;
    }
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Parse a ClickHouse JSON value as f64.
fn ch_f64(v: &serde_json::Value) -> f64 {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0)
}

/// Parse a ClickHouse DateTime64 string into chrono DateTime.
fn ch_datetime(v: &serde_json::Value) -> Option<chrono::DateTime<Utc>> {
    let s = v.as_str()?;
    if s.is_empty() || s == "1970-01-01 00:00:00.000" {
        return None;
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .ok()
        .map(|dt| dt.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_string_basic() {
        assert_eq!(ClickHouseStore::escape_string("hello"), "hello");
        assert_eq!(
            ClickHouseStore::escape_string("it's a test"),
            "it\\'s a test"
        );
        assert_eq!(
            ClickHouseStore::escape_string("path\\to\\file"),
            "path\\\\to\\\\file"
        );
    }

    #[test]
    fn parse_empty_json_each_row() {
        let result = parse_json_each_row_runs("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn default_store_creation() {
        let store = ClickHouseStore::new()
            .with_url("http://localhost:8123".into())
            .with_database("test_db".into());
        assert_eq!(store.base_url, "http://localhost:8123");
        assert_eq!(store.database, "test_db");
    }

    #[test]
    fn store_with_auth() {
        let store = ClickHouseStore::new()
            .with_url("http://localhost:8123".into())
            .with_user("admin".into())
            .with_password("secret".into());
        assert_eq!(store.user, "admin");
        assert_eq!(store.password, "secret");
    }

    #[test]
    fn parse_uuid_valid() {
        let id = Uuid::new_v4();
        let value = serde_json::json!(id.to_string());
        let parsed = parse_uuid(&value).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn parse_uuid_invalid() {
        let value = serde_json::json!("not-a-uuid");
        assert!(parse_uuid(&value).is_err());
    }

    #[test]
    fn parse_projects_roundtrip() {
        let body = r#"{"id":"550e8400-e29b-41d4-a716-446655440000","name":"test","description":"A project","created_at":"2024-01-01 00:00:00.000"}"#;
        let projects = parse_json_each_row_projects(body).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "test");
        assert_eq!(projects[0].description.as_deref(), Some("A project"));
    }

    #[test]
    fn parse_datasets_roundtrip() {
        let body = r#"{"id":"550e8400-e29b-41d4-a716-446655440000","name":"ds-1","description":null,"project_id":"660e8400-e29b-41d4-a716-446655440000","created_at":"2024-01-01 00:00:00.000"}"#;
        let datasets = parse_json_each_row_datasets(body).unwrap();
        assert_eq!(datasets.len(), 1);
        assert_eq!(datasets[0].name, "ds-1");
        assert!(datasets[0].project_id.is_some());
    }

    #[test]
    fn parse_examples_roundtrip() {
        let body = r#"{"id":"550e8400-e29b-41d4-a716-446655440000","dataset_id":"660e8400-e29b-41d4-a716-446655440000","input":"{\"q\":\"2+2\"}","output":"4","metadata":null,"created_at":"2024-01-01 00:00:00.000"}"#;
        let examples = parse_json_each_row_examples(body).unwrap();
        assert_eq!(examples.len(), 1);
        assert_eq!(examples[0].output.as_deref(), Some("4"));
    }
}
