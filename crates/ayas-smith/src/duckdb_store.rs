use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::SmithError;
use crate::query::SmithQuery;
use crate::store::SmithStore;
use crate::types::{
    Dataset, Example, Feedback, FeedbackFilter, LatencyStats, Project, Run, RunFilter, RunPatch,
    TokenUsageSummary,
};
use crate::writer;

/// DuckDB + Parquet backed implementation of [`SmithStore`].
pub struct DuckDbStore {
    base_dir: PathBuf,
}

impl DuckDbStore {
    /// Create a new DuckDB-backed store.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Get the base directory.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

fn feedback_file(base_dir: &Path) -> PathBuf {
    base_dir.join("_feedback").join("feedback.json")
}

fn load_feedback_sync(base_dir: &Path) -> Result<Vec<Feedback>, SmithError> {
    let path = feedback_file(base_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&path)?;
    let items: Vec<Feedback> = serde_json::from_str(&data)?;
    Ok(items)
}

fn save_feedback_sync(base_dir: &Path, items: &[Feedback]) -> Result<(), SmithError> {
    let path = feedback_file(base_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(items)?;
    std::fs::write(path, data)?;
    Ok(())
}

// --- Project helpers ---

fn projects_file(base_dir: &Path) -> PathBuf {
    base_dir.join("_meta").join("projects.json")
}

fn load_projects_sync(base_dir: &Path) -> Result<Vec<Project>, SmithError> {
    let path = projects_file(base_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&path)?;
    let items: Vec<Project> = serde_json::from_str(&data)?;
    Ok(items)
}

fn save_projects_sync(base_dir: &Path, items: &[Project]) -> Result<(), SmithError> {
    let path = projects_file(base_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(items)?;
    std::fs::write(path, data)?;
    Ok(())
}

// --- Dataset helpers ---

fn datasets_file(base_dir: &Path) -> PathBuf {
    base_dir.join("_meta").join("datasets.json")
}

fn load_datasets_sync(base_dir: &Path) -> Result<Vec<Dataset>, SmithError> {
    let path = datasets_file(base_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&path)?;
    let items: Vec<Dataset> = serde_json::from_str(&data)?;
    Ok(items)
}

fn save_datasets_sync(base_dir: &Path, items: &[Dataset]) -> Result<(), SmithError> {
    let path = datasets_file(base_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(items)?;
    std::fs::write(path, data)?;
    Ok(())
}

// --- Example helpers ---

fn examples_file(base_dir: &Path, dataset_id: Uuid) -> PathBuf {
    base_dir
        .join("_meta")
        .join(format!("examples_{dataset_id}.json"))
}

fn load_examples_sync(base_dir: &Path, dataset_id: Uuid) -> Result<Vec<Example>, SmithError> {
    let path = examples_file(base_dir, dataset_id);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&path)?;
    let items: Vec<Example> = serde_json::from_str(&data)?;
    Ok(items)
}

fn save_examples_sync(
    base_dir: &Path,
    dataset_id: Uuid,
    items: &[Example],
) -> Result<(), SmithError> {
    let path = examples_file(base_dir, dataset_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(items)?;
    std::fs::write(path, data)?;
    Ok(())
}

#[async_trait]
impl SmithStore for DuckDbStore {
    async fn init(&self) -> Result<(), SmithError> {
        // DuckDbStore uses file-based JSON storage for metadata.
        // Ensure directories exist.
        let base = self.base_dir.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(base.join("_meta"))?;
            std::fs::create_dir_all(base.join("_feedback"))?;
            Ok(())
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn put_runs(&self, runs: &[Run]) -> Result<(), SmithError> {
        if runs.is_empty() {
            return Ok(());
        }

        let base_dir = self.base_dir.clone();
        let runs = runs.to_vec();

        tokio::task::spawn_blocking(move || {
            let mut by_project: HashMap<String, Vec<Run>> = HashMap::new();
            for run in runs {
                by_project.entry(run.project.clone()).or_default().push(run);
            }

            for (project, project_runs) in by_project {
                let batch_id = Uuid::new_v4().to_string()[..8].to_string();
                let path = writer::parquet_path(&base_dir, &project, &batch_id);
                writer::write_runs(&project_runs, &path)?;
            }

            Ok(())
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn patch_run(
        &self,
        run_id: Uuid,
        project: &str,
        patch: &RunPatch,
    ) -> Result<(), SmithError> {
        let base_dir = self.base_dir.clone();
        let project = project.to_string();
        let patch = patch.clone();

        tokio::task::spawn_blocking(move || {
            let query = SmithQuery::new(&base_dir)?;
            let run = query.get_run(run_id, &project)?;
            match run {
                Some(mut run) => {
                    run.apply_patch(&patch);
                    let batch_id = format!("patch_{}", &Uuid::new_v4().to_string()[..8]);
                    let path = writer::parquet_path(&base_dir, &project, &batch_id);
                    writer::write_runs(&[run], &path)?;
                    Ok(())
                }
                None => Err(SmithError::Query(format!(
                    "Run {run_id} not found in project {project}"
                ))),
            }
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn list_runs(&self, filter: &RunFilter) -> Result<Vec<Run>, SmithError> {
        let base_dir = self.base_dir.clone();
        let filter = filter.clone();
        tokio::task::spawn_blocking(move || {
            let query = SmithQuery::new(base_dir)?;
            query.list_runs(&filter)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn get_run(&self, run_id: Uuid, project: &str) -> Result<Option<Run>, SmithError> {
        let base_dir = self.base_dir.clone();
        let project = project.to_string();
        tokio::task::spawn_blocking(move || {
            let query = SmithQuery::new(base_dir)?;
            query.get_run(run_id, &project)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn get_trace(&self, trace_id: Uuid, project: &str) -> Result<Vec<Run>, SmithError> {
        let base_dir = self.base_dir.clone();
        let project = project.to_string();
        tokio::task::spawn_blocking(move || {
            let query = SmithQuery::new(base_dir)?;
            query.get_trace(trace_id, &project)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn get_children(
        &self,
        parent_run_id: Uuid,
        project: &str,
    ) -> Result<Vec<Run>, SmithError> {
        let base_dir = self.base_dir.clone();
        let project = project.to_string();
        tokio::task::spawn_blocking(move || {
            let query = SmithQuery::new(base_dir)?;
            query.get_children(parent_run_id, &project)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn token_usage_summary(
        &self,
        filter: &RunFilter,
    ) -> Result<TokenUsageSummary, SmithError> {
        let base_dir = self.base_dir.clone();
        let filter = filter.clone();
        tokio::task::spawn_blocking(move || {
            let query = SmithQuery::new(base_dir)?;
            query.token_usage_summary(&filter)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn latency_percentiles(&self, filter: &RunFilter) -> Result<LatencyStats, SmithError> {
        let base_dir = self.base_dir.clone();
        let filter = filter.clone();
        tokio::task::spawn_blocking(move || {
            let query = SmithQuery::new(base_dir)?;
            query.latency_percentiles(&filter)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn put_feedback(&self, feedback: &Feedback) -> Result<(), SmithError> {
        let base_dir = self.base_dir.clone();
        let feedback = feedback.clone();
        tokio::task::spawn_blocking(move || {
            let mut items = load_feedback_sync(&base_dir)?;
            items.push(feedback);
            save_feedback_sync(&base_dir, &items)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn list_feedback(&self, filter: &FeedbackFilter) -> Result<Vec<Feedback>, SmithError> {
        let base_dir = self.base_dir.clone();
        let filter = filter.clone();
        tokio::task::spawn_blocking(move || {
            let items = load_feedback_sync(&base_dir)?;
            let filtered = items
                .into_iter()
                .filter(|f| {
                    if let Some(ref run_id) = filter.run_id {
                        if f.run_id != *run_id {
                            return false;
                        }
                    }
                    if let Some(ref key) = filter.key {
                        if f.key != *key {
                            return false;
                        }
                    }
                    true
                })
                .collect();
            Ok(filtered)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    // --- Project management ---

    async fn create_project(&self, project: &Project) -> Result<(), SmithError> {
        let base_dir = self.base_dir.clone();
        let project = project.clone();
        tokio::task::spawn_blocking(move || {
            let mut items = load_projects_sync(&base_dir)?;
            items.push(project);
            save_projects_sync(&base_dir, &items)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn list_projects(&self) -> Result<Vec<Project>, SmithError> {
        let base_dir = self.base_dir.clone();
        tokio::task::spawn_blocking(move || load_projects_sync(&base_dir))
            .await
            .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn get_project(&self, id: Uuid) -> Result<Option<Project>, SmithError> {
        let base_dir = self.base_dir.clone();
        tokio::task::spawn_blocking(move || {
            let items = load_projects_sync(&base_dir)?;
            Ok(items.into_iter().find(|p| p.id == id))
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn delete_project(&self, id: Uuid) -> Result<(), SmithError> {
        let base_dir = self.base_dir.clone();
        tokio::task::spawn_blocking(move || {
            let items = load_projects_sync(&base_dir)?;
            let filtered: Vec<Project> = items.into_iter().filter(|p| p.id != id).collect();
            save_projects_sync(&base_dir, &filtered)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    // --- Dataset management ---

    async fn create_dataset(&self, dataset: &Dataset) -> Result<(), SmithError> {
        let base_dir = self.base_dir.clone();
        let dataset = dataset.clone();
        tokio::task::spawn_blocking(move || {
            let mut items = load_datasets_sync(&base_dir)?;
            items.push(dataset);
            save_datasets_sync(&base_dir, &items)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn list_datasets(&self, project_id: Option<Uuid>) -> Result<Vec<Dataset>, SmithError> {
        let base_dir = self.base_dir.clone();
        tokio::task::spawn_blocking(move || {
            let items = load_datasets_sync(&base_dir)?;
            let filtered = if let Some(pid) = project_id {
                items
                    .into_iter()
                    .filter(|d| d.project_id == Some(pid))
                    .collect()
            } else {
                items
            };
            Ok(filtered)
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn add_examples(&self, examples: &[Example]) -> Result<(), SmithError> {
        if examples.is_empty() {
            return Ok(());
        }
        let base_dir = self.base_dir.clone();
        let examples = examples.to_vec();
        tokio::task::spawn_blocking(move || {
            // Group by dataset_id
            let mut by_dataset: HashMap<Uuid, Vec<Example>> = HashMap::new();
            for ex in examples {
                by_dataset.entry(ex.dataset_id).or_default().push(ex);
            }
            for (dataset_id, new_examples) in by_dataset {
                let mut existing = load_examples_sync(&base_dir, dataset_id)?;
                existing.extend(new_examples);
                save_examples_sync(&base_dir, dataset_id, &existing)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }

    async fn list_examples(&self, dataset_id: Uuid) -> Result<Vec<Example>, SmithError> {
        let base_dir = self.base_dir.clone();
        tokio::task::spawn_blocking(move || load_examples_sync(&base_dir, dataset_id))
            .await
            .map_err(|e| SmithError::Query(format!("spawn_blocking failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::flush_runs;
    use crate::types::RunType;

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

        let runs = vec![root, child_llm];
        flush_runs(&runs, dir, "test-proj").unwrap();
        runs
    }

    #[tokio::test]
    async fn put_and_list_runs() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());

        let runs: Vec<Run> = (0..3)
            .map(|i| {
                Run::builder(format!("run-{i}"), RunType::Chain)
                    .project("test-proj")
                    .finish_ok("done")
            })
            .collect();

        store.put_runs(&runs).await.unwrap();

        let filter = RunFilter {
            project: Some("test-proj".into()),
            ..Default::default()
        };
        let result = store.list_runs(&filter).await.unwrap();
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn get_run_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let written = create_test_runs(dir.path());
        let target_id = written[0].run_id;

        let store = DuckDbStore::new(dir.path());
        let run = store.get_run(target_id, "test-proj").await.unwrap();
        assert!(run.is_some());
        assert_eq!(run.unwrap().run_id, target_id);
    }

    #[tokio::test]
    async fn get_trace_runs() {
        let dir = tempfile::tempdir().unwrap();
        let written = create_test_runs(dir.path());
        let trace_id = written[0].trace_id;

        let store = DuckDbStore::new(dir.path());
        let runs = store.get_trace(trace_id, "test-proj").await.unwrap();
        assert_eq!(runs.len(), 2);
    }

    #[tokio::test]
    async fn get_children_runs() {
        let dir = tempfile::tempdir().unwrap();
        let written = create_test_runs(dir.path());
        let parent_id = written[0].run_id;

        let store = DuckDbStore::new(dir.path());
        let children = store.get_children(parent_id, "test-proj").await.unwrap();
        assert_eq!(children.len(), 1);
    }

    #[tokio::test]
    async fn token_usage_summary_via_store() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());

        let store = DuckDbStore::new(dir.path());
        let filter = RunFilter {
            project: Some("test-proj".into()),
            ..Default::default()
        };
        let summary = store.token_usage_summary(&filter).await.unwrap();
        assert_eq!(summary.total_input_tokens, 50);
        assert_eq!(summary.total_output_tokens, 10);
        assert_eq!(summary.total_tokens, 60);
    }

    #[tokio::test]
    async fn latency_percentiles_via_store() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());

        let store = DuckDbStore::new(dir.path());
        let filter = RunFilter {
            project: Some("test-proj".into()),
            ..Default::default()
        };
        let stats = store.latency_percentiles(&filter).await.unwrap();
        assert!(stats.p50 >= 0.0);
    }

    #[tokio::test]
    async fn feedback_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());

        let run_id = Uuid::new_v4();
        let feedback = Feedback {
            id: Uuid::new_v4(),
            run_id,
            key: "correctness".into(),
            score: 0.9,
            comment: Some("Good".into()),
            created_at: chrono::Utc::now(),
        };

        store.put_feedback(&feedback).await.unwrap();

        let filter = FeedbackFilter {
            run_id: Some(run_id),
            ..Default::default()
        };
        let result = store.list_feedback(&filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].key, "correctness");
        assert!((result[0].score - 0.9).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn list_feedback_by_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());

        let run_id = Uuid::new_v4();
        for key in &["correctness", "helpfulness", "correctness"] {
            let fb = Feedback {
                id: Uuid::new_v4(),
                run_id,
                key: key.to_string(),
                score: 1.0,
                comment: None,
                created_at: chrono::Utc::now(),
            };
            store.put_feedback(&fb).await.unwrap();
        }

        let filter = FeedbackFilter {
            key: Some("correctness".into()),
            ..Default::default()
        };
        let result = store.list_feedback(&filter).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn list_feedback_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());

        let filter = FeedbackFilter::default();
        let result = store.list_feedback(&filter).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn put_runs_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());
        store.put_runs(&[]).await.unwrap();
    }

    #[tokio::test]
    async fn get_run_not_found() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());

        let store = DuckDbStore::new(dir.path());
        let run = store.get_run(Uuid::new_v4(), "test-proj").await.unwrap();
        assert!(run.is_none());
    }

    #[test]
    fn base_dir_accessor() {
        let store = DuckDbStore::new("/tmp/test");
        assert_eq!(store.base_dir(), Path::new("/tmp/test"));
    }

    #[tokio::test]
    async fn patch_run_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());

        // Create a Running run
        let run = Run::builder("my-chain", RunType::Chain)
            .project("test-proj")
            .start();
        let run_id = run.run_id;

        store.put_runs(&[run]).await.unwrap();

        // Verify it's Running
        let found = store.get_run(run_id, "test-proj").await.unwrap().unwrap();
        assert_eq!(found.status, crate::types::RunStatus::Running);
        assert!(found.end_time.is_none());

        // Patch it to Success
        let patch = crate::types::RunPatch {
            end_time: Some(chrono::Utc::now()),
            output: Some("result".into()),
            status: Some(crate::types::RunStatus::Success),
            ..Default::default()
        };
        store.patch_run(run_id, "test-proj", &patch).await.unwrap();

        // Verify the patch was applied (dedup picks the completed version)
        let patched = store.get_run(run_id, "test-proj").await.unwrap().unwrap();
        assert_eq!(patched.status, crate::types::RunStatus::Success);
        assert_eq!(patched.output.as_deref(), Some("result"));
        assert!(patched.end_time.is_some());
    }

    #[tokio::test]
    async fn patch_run_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());

        // Create some runs so the parquet directory exists
        let run = Run::builder("dummy", RunType::Chain)
            .project("test-proj")
            .finish_ok("ok");
        store.put_runs(&[run]).await.unwrap();

        let patch = crate::types::RunPatch {
            status: Some(crate::types::RunStatus::Success),
            ..Default::default()
        };
        let result = store
            .patch_run(Uuid::new_v4(), "test-proj", &patch)
            .await;
        assert!(result.is_err());
    }

    // --- Project tests ---

    #[tokio::test]
    async fn project_crud() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());

        // Empty initially
        let projects = store.list_projects().await.unwrap();
        assert!(projects.is_empty());

        // Create
        let project = crate::types::Project {
            id: Uuid::new_v4(),
            name: "my-project".into(),
            description: Some("A test project".into()),
            created_at: chrono::Utc::now(),
        };
        store.create_project(&project).await.unwrap();

        // List
        let projects = store.list_projects().await.unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "my-project");

        // Get
        let fetched = store.get_project(project.id).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().name, "my-project");

        // Get not found
        let fetched = store.get_project(Uuid::new_v4()).await.unwrap();
        assert!(fetched.is_none());

        // Delete
        store.delete_project(project.id).await.unwrap();
        let projects = store.list_projects().await.unwrap();
        assert!(projects.is_empty());
    }

    // --- Dataset tests ---

    #[tokio::test]
    async fn dataset_crud() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());
        let project_id = Uuid::new_v4();

        // Create datasets
        let ds1 = crate::types::Dataset {
            id: Uuid::new_v4(),
            name: "ds-1".into(),
            description: None,
            project_id: Some(project_id),
            created_at: chrono::Utc::now(),
        };
        let ds2 = crate::types::Dataset {
            id: Uuid::new_v4(),
            name: "ds-2".into(),
            description: None,
            project_id: None,
            created_at: chrono::Utc::now(),
        };
        store.create_dataset(&ds1).await.unwrap();
        store.create_dataset(&ds2).await.unwrap();

        // List all
        let all = store.list_datasets(None).await.unwrap();
        assert_eq!(all.len(), 2);

        // List by project
        let filtered = store.list_datasets(Some(project_id)).await.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "ds-1");
    }

    // --- Example tests ---

    #[tokio::test]
    async fn example_crud() {
        let dir = tempfile::tempdir().unwrap();
        let store = DuckDbStore::new(dir.path());
        let dataset_id = Uuid::new_v4();

        // Empty initially
        let examples = store.list_examples(dataset_id).await.unwrap();
        assert!(examples.is_empty());

        // Add examples
        let examples_to_add = vec![
            crate::types::Example {
                id: Uuid::new_v4(),
                dataset_id,
                input: r#"{"q": "2+2"}"#.into(),
                output: Some("4".into()),
                metadata: None,
                created_at: chrono::Utc::now(),
            },
            crate::types::Example {
                id: Uuid::new_v4(),
                dataset_id,
                input: r#"{"q": "3+3"}"#.into(),
                output: Some("6".into()),
                metadata: None,
                created_at: chrono::Utc::now(),
            },
        ];
        store.add_examples(&examples_to_add).await.unwrap();

        // List
        let fetched = store.list_examples(dataset_id).await.unwrap();
        assert_eq!(fetched.len(), 2);

        // Different dataset_id returns empty
        let other = store.list_examples(Uuid::new_v4()).await.unwrap();
        assert!(other.is_empty());
    }
}
