use async_trait::async_trait;
use uuid::Uuid;

use crate::error::SmithError;
use crate::types::{
    Dataset, Example, Feedback, FeedbackFilter, LatencyStats, Project, Run, RunFilter, RunPatch,
    TokenUsageSummary,
};

/// Storage abstraction for Smith tracing data.
///
/// Implementations handle persistence of traced runs and feedback.
#[async_trait]
pub trait SmithStore: Send + Sync {
    /// Initialize storage (create tables, directories, etc). Called once at startup.
    async fn init(&self) -> Result<(), SmithError> {
        Ok(())
    }

    /// Persist a batch of runs.
    async fn put_runs(&self, runs: &[Run]) -> Result<(), SmithError>;

    /// Patch an existing run (2-phase lifecycle).
    async fn patch_run(
        &self,
        run_id: Uuid,
        project: &str,
        patch: &RunPatch,
    ) -> Result<(), SmithError>;

    /// List runs matching the given filter.
    async fn list_runs(&self, filter: &RunFilter) -> Result<Vec<Run>, SmithError>;

    /// Get a single run by ID and project.
    async fn get_run(&self, run_id: Uuid, project: &str) -> Result<Option<Run>, SmithError>;

    /// Get all runs belonging to a trace.
    async fn get_trace(&self, trace_id: Uuid, project: &str) -> Result<Vec<Run>, SmithError>;

    /// Get child runs of a given parent run.
    async fn get_children(
        &self,
        parent_run_id: Uuid,
        project: &str,
    ) -> Result<Vec<Run>, SmithError>;

    /// Get token usage summary for runs matching the filter.
    async fn token_usage_summary(
        &self,
        filter: &RunFilter,
    ) -> Result<TokenUsageSummary, SmithError>;

    /// Get latency percentiles for runs matching the filter.
    async fn latency_percentiles(&self, filter: &RunFilter) -> Result<LatencyStats, SmithError>;

    /// Persist a feedback entry.
    async fn put_feedback(&self, feedback: &Feedback) -> Result<(), SmithError>;

    /// List feedback matching the given filter.
    async fn list_feedback(&self, filter: &FeedbackFilter) -> Result<Vec<Feedback>, SmithError>;

    // --- Project management ---

    /// Create a new project.
    async fn create_project(&self, project: &Project) -> Result<(), SmithError>;

    /// List all projects.
    async fn list_projects(&self) -> Result<Vec<Project>, SmithError>;

    /// Get a single project by ID.
    async fn get_project(&self, id: Uuid) -> Result<Option<Project>, SmithError>;

    /// Delete a project by ID.
    async fn delete_project(&self, id: Uuid) -> Result<(), SmithError>;

    // --- Dataset management ---

    /// Create a new dataset.
    async fn create_dataset(&self, dataset: &Dataset) -> Result<(), SmithError>;

    /// List datasets, optionally filtered by project_id.
    async fn list_datasets(&self, project_id: Option<Uuid>) -> Result<Vec<Dataset>, SmithError>;

    /// Add examples to a dataset.
    async fn add_examples(&self, examples: &[Example]) -> Result<(), SmithError>;

    /// List examples in a dataset.
    async fn list_examples(&self, dataset_id: Uuid) -> Result<Vec<Example>, SmithError>;
}
