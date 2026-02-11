use async_trait::async_trait;
use tokio_postgres::{Client, NoTls};
use uuid::Uuid;

use crate::error::SmithError;
use crate::store::SmithStore;
use crate::types::{
    Dataset, Example, Feedback, FeedbackFilter, LatencyStats, Project, Run, RunFilter, RunPatch,
    RunStatus, RunType, TokenUsageSummary,
};

/// PostgreSQL-backed SmithStore.
///
/// Feature-gated behind `postgres` feature flag.
pub struct PostgresSmithStore {
    client: Client,
    _handle: tokio::task::JoinHandle<()>,
}

impl PostgresSmithStore {
    /// Connect to PostgreSQL using the `SMITH_DATABASE_URL` environment variable.
    pub async fn from_env() -> Result<Self, SmithError> {
        let url = std::env::var("SMITH_DATABASE_URL").map_err(|_| {
            SmithError::Query("SMITH_DATABASE_URL environment variable not set".into())
        })?;
        Self::connect(&url).await
    }

    pub async fn connect(url: &str) -> Result<Self, SmithError> {
        let (client, connection) = tokio_postgres::connect(url, NoTls)
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL connection error: {e}")))?;

        let handle = tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!("PostgreSQL connection error: {e}");
            }
        });

        let store = Self {
            client,
            _handle: handle,
        };
        store.create_tables().await?;
        Ok(store)
    }

    async fn create_tables(&self) -> Result<(), SmithError> {
        self.client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS runs (
                    run_id UUID NOT NULL PRIMARY KEY,
                    parent_run_id UUID,
                    trace_id UUID NOT NULL,
                    name TEXT NOT NULL,
                    run_type TEXT NOT NULL,
                    project TEXT NOT NULL,
                    start_time TIMESTAMPTZ NOT NULL,
                    end_time TIMESTAMPTZ,
                    status TEXT NOT NULL,
                    input TEXT NOT NULL,
                    output TEXT,
                    error TEXT,
                    tags TEXT[] NOT NULL DEFAULT '{}',
                    metadata TEXT NOT NULL DEFAULT '{}',
                    input_tokens BIGINT,
                    output_tokens BIGINT,
                    total_tokens BIGINT,
                    latency_ms BIGINT,
                    dotted_order TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_runs_project ON runs (project);
                CREATE INDEX IF NOT EXISTS idx_runs_trace_id ON runs (trace_id);
                CREATE INDEX IF NOT EXISTS idx_runs_parent ON runs (parent_run_id);
                CREATE INDEX IF NOT EXISTS idx_runs_start_time ON runs (start_time);

                CREATE TABLE IF NOT EXISTS feedback (
                    id UUID NOT NULL PRIMARY KEY,
                    run_id UUID NOT NULL,
                    key TEXT NOT NULL,
                    score DOUBLE PRECISION NOT NULL,
                    comment TEXT,
                    created_at TIMESTAMPTZ NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_feedback_run_id ON feedback (run_id);
                CREATE INDEX IF NOT EXISTS idx_feedback_key ON feedback (key);",
            )
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL create tables error: {e}")))?;

        Ok(())
    }
}

#[async_trait]
impl SmithStore for PostgresSmithStore {
    async fn put_runs(&self, runs: &[Run]) -> Result<(), SmithError> {
        for run in runs {
            let tags: Vec<&str> = run.tags.iter().map(|s| s.as_str()).collect();
            self.client
                .execute(
                    "INSERT INTO runs (run_id, parent_run_id, trace_id, name, run_type, project,
                        start_time, end_time, status, input, output, error, tags, metadata,
                        input_tokens, output_tokens, total_tokens, latency_ms, dotted_order)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                             $15, $16, $17, $18, $19)
                     ON CONFLICT (run_id) DO UPDATE SET
                        end_time = EXCLUDED.end_time,
                        status = EXCLUDED.status,
                        output = EXCLUDED.output,
                        error = EXCLUDED.error,
                        input_tokens = EXCLUDED.input_tokens,
                        output_tokens = EXCLUDED.output_tokens,
                        total_tokens = EXCLUDED.total_tokens,
                        latency_ms = EXCLUDED.latency_ms",
                    &[
                        &run.run_id,
                        &run.parent_run_id,
                        &run.trace_id,
                        &run.name,
                        &run.run_type.as_str(),
                        &run.project,
                        &run.start_time,
                        &run.end_time,
                        &run.status.as_str(),
                        &run.input,
                        &run.output,
                        &run.error,
                        &tags,
                        &run.metadata,
                        &run.input_tokens,
                        &run.output_tokens,
                        &run.total_tokens,
                        &run.latency_ms,
                        &run.dotted_order,
                    ],
                )
                .await
                .map_err(|e| SmithError::Query(format!("PostgreSQL put_runs error: {e}")))?;
        }
        Ok(())
    }

    async fn patch_run(
        &self,
        run_id: Uuid,
        _project: &str,
        patch: &RunPatch,
    ) -> Result<(), SmithError> {
        let mut sets = Vec::new();
        let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        let mut idx = 1;

        if let Some(end_time) = patch.end_time {
            sets.push(format!("end_time = ${idx}"));
            params.push(Box::new(end_time));
            idx += 1;
        }
        if let Some(ref output) = patch.output {
            sets.push(format!("output = ${idx}"));
            params.push(Box::new(output.clone()));
            idx += 1;
        }
        if let Some(ref error) = patch.error {
            sets.push(format!("error = ${idx}"));
            params.push(Box::new(error.clone()));
            idx += 1;
        }
        if let Some(status) = patch.status {
            sets.push(format!("status = ${idx}"));
            params.push(Box::new(status.as_str().to_string()));
            idx += 1;
        }
        if let Some(tokens) = patch.input_tokens {
            sets.push(format!("input_tokens = ${idx}"));
            params.push(Box::new(tokens));
            idx += 1;
        }
        if let Some(tokens) = patch.output_tokens {
            sets.push(format!("output_tokens = ${idx}"));
            params.push(Box::new(tokens));
            idx += 1;
        }
        if let Some(tokens) = patch.total_tokens {
            sets.push(format!("total_tokens = ${idx}"));
            params.push(Box::new(tokens));
            idx += 1;
        }
        if let Some(ms) = patch.latency_ms {
            sets.push(format!("latency_ms = ${idx}"));
            params.push(Box::new(ms));
            idx += 1;
        }

        if sets.is_empty() {
            return Ok(());
        }

        let sql = format!(
            "UPDATE runs SET {} WHERE run_id = ${idx}",
            sets.join(", ")
        );
        params.push(Box::new(run_id));

        let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
            params.iter().map(|p| p.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();

        self.client
            .execute(&sql, &refs)
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL patch_run error: {e}")))?;

        Ok(())
    }

    async fn list_runs(&self, filter: &RunFilter) -> Result<Vec<Run>, SmithError> {
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        let mut idx = 1;

        if let Some(ref project) = filter.project {
            conditions.push(format!("project = ${idx}"));
            params.push(Box::new(project.clone()));
            idx += 1;
        }
        if let Some(run_type) = filter.run_type {
            conditions.push(format!("run_type = ${idx}"));
            params.push(Box::new(run_type.as_str().to_string()));
            idx += 1;
        }
        if let Some(status) = filter.status {
            conditions.push(format!("status = ${idx}"));
            params.push(Box::new(status.as_str().to_string()));
            idx += 1;
        }
        if let Some(ref start_after) = filter.start_after {
            conditions.push(format!("start_time > ${idx}"));
            params.push(Box::new(*start_after));
            idx += 1;
        }
        if let Some(trace_id) = filter.trace_id {
            conditions.push(format!("trace_id = ${idx}"));
            params.push(Box::new(trace_id));
            idx += 1;
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let limit = filter.limit.unwrap_or(100);
        let offset = filter.offset.unwrap_or(0);

        let sql = format!(
            "SELECT * FROM runs {where_clause} ORDER BY start_time DESC LIMIT {limit} OFFSET {offset}"
        );

        let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
            params.iter().map(|p| p.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();

        let rows = self
            .client
            .query(&sql, &refs)
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL list_runs error: {e}")))?;

        rows.iter().map(row_to_run).collect()
    }

    async fn get_run(&self, run_id: Uuid, _project: &str) -> Result<Option<Run>, SmithError> {
        let row = self
            .client
            .query_opt("SELECT * FROM runs WHERE run_id = $1", &[&run_id])
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL get_run error: {e}")))?;

        match row {
            Some(r) => Ok(Some(row_to_run(&r)?)),
            None => Ok(None),
        }
    }

    async fn get_trace(&self, trace_id: Uuid, _project: &str) -> Result<Vec<Run>, SmithError> {
        let rows = self
            .client
            .query(
                "SELECT * FROM runs WHERE trace_id = $1 ORDER BY start_time ASC",
                &[&trace_id],
            )
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL get_trace error: {e}")))?;

        rows.iter().map(row_to_run).collect()
    }

    async fn get_children(
        &self,
        parent_run_id: Uuid,
        _project: &str,
    ) -> Result<Vec<Run>, SmithError> {
        let rows = self
            .client
            .query(
                "SELECT * FROM runs WHERE parent_run_id = $1 ORDER BY start_time ASC",
                &[&parent_run_id],
            )
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL get_children error: {e}")))?;

        rows.iter().map(row_to_run).collect()
    }

    async fn token_usage_summary(
        &self,
        filter: &RunFilter,
    ) -> Result<TokenUsageSummary, SmithError> {
        let project = filter.project.as_deref().unwrap_or("default");
        let row = self
            .client
            .query_one(
                "SELECT
                    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) as total_input,
                    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) as total_output,
                    CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) as total,
                    COUNT(*) as run_count
                 FROM runs WHERE project = $1",
                &[&project],
            )
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL token_usage error: {e}")))?;

        Ok(TokenUsageSummary {
            total_input_tokens: row.get::<_, i64>("total_input"),
            total_output_tokens: row.get::<_, i64>("total_output"),
            total_tokens: row.get::<_, i64>("total"),
            run_count: row.get::<_, i64>("run_count"),
        })
    }

    async fn latency_percentiles(&self, filter: &RunFilter) -> Result<LatencyStats, SmithError> {
        let project = filter.project.as_deref().unwrap_or("default");
        let row = self
            .client
            .query_one(
                "SELECT
                    COALESCE(PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY latency_ms), 0) as p50,
                    COALESCE(PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY latency_ms), 0) as p90,
                    COALESCE(PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY latency_ms), 0) as p95,
                    COALESCE(PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY latency_ms), 0) as p99
                 FROM runs WHERE project = $1 AND latency_ms IS NOT NULL",
                &[&project],
            )
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL latency error: {e}")))?;

        Ok(LatencyStats {
            p50: row.get::<_, f64>("p50"),
            p90: row.get::<_, f64>("p90"),
            p95: row.get::<_, f64>("p95"),
            p99: row.get::<_, f64>("p99"),
        })
    }

    async fn put_feedback(&self, feedback: &Feedback) -> Result<(), SmithError> {
        self.client
            .execute(
                "INSERT INTO feedback (id, run_id, key, score, comment, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (id) DO UPDATE SET
                    score = EXCLUDED.score,
                    comment = EXCLUDED.comment",
                &[
                    &feedback.id,
                    &feedback.run_id,
                    &feedback.key,
                    &feedback.score,
                    &feedback.comment,
                    &feedback.created_at,
                ],
            )
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL put_feedback error: {e}")))?;

        Ok(())
    }

    async fn list_feedback(&self, filter: &FeedbackFilter) -> Result<Vec<Feedback>, SmithError> {
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        let mut idx = 1;

        if let Some(run_id) = filter.run_id {
            conditions.push(format!("run_id = ${idx}"));
            params.push(Box::new(run_id));
            idx += 1;
        }
        if let Some(ref key) = filter.key {
            conditions.push(format!("key = ${idx}"));
            params.push(Box::new(key.clone()));
            let _ = idx;
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT * FROM feedback {where_clause} ORDER BY created_at DESC"
        );

        let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
            params.iter().map(|p| p.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();

        let rows = self
            .client
            .query(&sql, &refs)
            .await
            .map_err(|e| SmithError::Query(format!("PostgreSQL list_feedback error: {e}")))?;

        Ok(rows
            .iter()
            .map(|row| Feedback {
                id: row.get("id"),
                run_id: row.get("run_id"),
                key: row.get("key"),
                score: row.get("score"),
                comment: row.get("comment"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    async fn create_project(&self, _project: &Project) -> Result<(), SmithError> {
        Err(SmithError::Query("PostgreSQL project management not yet implemented".into()))
    }

    async fn list_projects(&self) -> Result<Vec<Project>, SmithError> {
        Err(SmithError::Query("PostgreSQL project management not yet implemented".into()))
    }

    async fn get_project(&self, _id: Uuid) -> Result<Option<Project>, SmithError> {
        Err(SmithError::Query("PostgreSQL project management not yet implemented".into()))
    }

    async fn delete_project(&self, _id: Uuid) -> Result<(), SmithError> {
        Err(SmithError::Query("PostgreSQL project management not yet implemented".into()))
    }

    async fn create_dataset(&self, _dataset: &Dataset) -> Result<(), SmithError> {
        Err(SmithError::Query("PostgreSQL dataset management not yet implemented".into()))
    }

    async fn list_datasets(&self, _project_id: Option<Uuid>) -> Result<Vec<Dataset>, SmithError> {
        Err(SmithError::Query("PostgreSQL dataset management not yet implemented".into()))
    }

    async fn add_examples(&self, _examples: &[Example]) -> Result<(), SmithError> {
        Err(SmithError::Query("PostgreSQL example management not yet implemented".into()))
    }

    async fn list_examples(&self, _dataset_id: Uuid) -> Result<Vec<Example>, SmithError> {
        Err(SmithError::Query("PostgreSQL example management not yet implemented".into()))
    }
}

fn row_to_run(row: &tokio_postgres::Row) -> Result<Run, SmithError> {
    let run_type_str: String = row.get("run_type");
    let run_type: RunType = run_type_str
        .parse()
        .map_err(|e: String| SmithError::Query(e))?;

    let status_str: String = row.get("status");
    let status: RunStatus = status_str
        .parse()
        .map_err(|e: String| SmithError::Query(e))?;

    let tags: Vec<String> = row.get("tags");

    Ok(Run {
        run_id: row.get("run_id"),
        parent_run_id: row.get("parent_run_id"),
        trace_id: row.get("trace_id"),
        name: row.get("name"),
        run_type,
        project: row.get("project"),
        start_time: row.get("start_time"),
        end_time: row.get("end_time"),
        status,
        input: row.get("input"),
        output: row.get("output"),
        error: row.get("error"),
        tags,
        metadata: row.get("metadata"),
        input_tokens: row.get("input_tokens"),
        output_tokens: row.get("output_tokens"),
        total_tokens: row.get("total_tokens"),
        latency_ms: row.get("latency_ms"),
        dotted_order: row.get("dotted_order"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_type_roundtrip() {
        for rt in [
            RunType::Chain,
            RunType::Llm,
            RunType::Tool,
            RunType::Retriever,
            RunType::Graph,
        ] {
            let s = rt.as_str();
            let parsed: RunType = s.parse().unwrap();
            assert_eq!(rt, parsed);
        }
    }

    #[test]
    fn run_status_roundtrip() {
        for rs in [RunStatus::Running, RunStatus::Success, RunStatus::Error] {
            let s = rs.as_str();
            let parsed: RunStatus = s.parse().unwrap();
            assert_eq!(rs, parsed);
        }
    }

    #[test]
    fn missing_env_errors() {
        let original = std::env::var("SMITH_DATABASE_URL").ok();
        unsafe { std::env::remove_var("SMITH_DATABASE_URL") };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(PostgresSmithStore::from_env());
        assert!(result.is_err());

        if let Some(url) = original {
            unsafe { std::env::set_var("SMITH_DATABASE_URL", url) };
        }
    }
}
