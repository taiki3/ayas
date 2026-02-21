//! Integration tests for ClickHouseStore.
//!
//! Run with: `cargo test -p ayas-smith --features clickhouse --test integration_clickhouse -- --ignored`
//!
//! Requires CLICKHOUSE_URL env var to be set.

#![cfg(feature = "clickhouse")]

use chrono::Utc;
use uuid::Uuid;

use ayas_smith::clickhouse_store::ClickHouseStore;
use ayas_smith::store::SmithStore;
use ayas_smith::types::*;

fn make_store() -> ClickHouseStore {
    ClickHouseStore::new()
}

async fn setup() -> ClickHouseStore {
    let store = make_store();
    store
        .init()
        .await
        .expect("Failed to initialize ClickHouse tables — is CLICKHOUSE_URL set?");
    store
}

fn make_run(name: &str, project: &str, run_type: RunType) -> Run {
    Run::builder(name, run_type)
        .project(project)
        .input(r#"{"q":"hello"}"#)
        .finish_ok(r#"{"a":"world"}"#)
}

#[tokio::test]
#[ignore]
async fn put_and_get_run() {
    let store = setup().await;
    let project = format!("ch-get-{}", Uuid::new_v4().as_simple());
    let run = make_run("ch-test-1", &project, RunType::Chain);
    let run_id = run.run_id;

    store.put_runs(&[run]).await.unwrap();

    // ClickHouse MergeTree needs a moment to settle
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let fetched = store.get_run(run_id, &project).await.unwrap();
    assert!(fetched.is_some(), "Run not found after insert");
    let fetched = fetched.unwrap();
    assert_eq!(fetched.run_id, run_id);
    assert_eq!(fetched.name, "ch-test-1");
    assert_eq!(fetched.status, RunStatus::Success);
}

#[tokio::test]
#[ignore]
async fn list_runs_by_project() {
    let store = setup().await;
    let project = format!("ch-list-{}", Uuid::new_v4().as_simple());

    let r1 = make_run("chain-1", &project, RunType::Chain);
    let r2 = make_run("llm-1", &project, RunType::Llm);

    store.put_runs(&[r1, r2]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let all = store
        .list_runs(&RunFilter {
            project: Some(project.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
#[ignore]
async fn trace_hierarchy() {
    let store = setup().await;
    let project = format!("ch-trace-{}", Uuid::new_v4().as_simple());
    let trace_id = Uuid::new_v4();

    let parent = Run::builder("parent", RunType::Chain)
        .trace_id(trace_id)
        .project(&project)
        .input("{}")
        .finish_ok("done");
    let parent_id = parent.run_id;

    let child = Run::builder("child", RunType::Llm)
        .trace_id(trace_id)
        .parent_run_id(parent_id)
        .project(&project)
        .input("{}")
        .finish_llm("resp", 5, 10, 15);

    store.put_runs(&[parent, child]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let trace = store.get_trace(trace_id, &project).await.unwrap();
    assert_eq!(trace.len(), 2);

    let children = store.get_children(parent_id, &project).await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name, "child");
}

#[tokio::test]
#[ignore]
async fn token_usage_summary() {
    let store = setup().await;
    let project = format!("ch-stats-{}", Uuid::new_v4().as_simple());

    let r1 = {
        let mut r = Run::builder("llm-a", RunType::Llm)
            .project(&project)
            .input("{}")
            .finish_llm("a", 100, 50, 150);
        r.latency_ms = Some(200);
        r
    };
    let r2 = {
        let mut r = Run::builder("llm-b", RunType::Llm)
            .project(&project)
            .input("{}")
            .finish_llm("b", 200, 100, 300);
        r.latency_ms = Some(400);
        r
    };

    store.put_runs(&[r1, r2]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let filter = RunFilter {
        project: Some(project.clone()),
        ..Default::default()
    };

    let usage = store.token_usage_summary(&filter).await.unwrap();
    assert_eq!(usage.total_input_tokens, 300);
    assert_eq!(usage.total_output_tokens, 150);
    assert_eq!(usage.total_tokens, 450);
    assert_eq!(usage.run_count, 2);
}

#[tokio::test]
#[ignore]
async fn feedback_crud() {
    let store = setup().await;
    let run_id = Uuid::new_v4();
    let project = format!("ch-fb-{}", Uuid::new_v4().as_simple());

    let run = Run::builder("fb-test", RunType::Chain)
        .run_id(run_id)
        .project(&project)
        .input("{}")
        .finish_ok("ok");
    store.put_runs(&[run]).await.unwrap();

    let fb = Feedback {
        id: Uuid::new_v4(),
        run_id,
        key: "accuracy".into(),
        score: 0.85,
        comment: Some("Decent".into()),
        created_at: Utc::now(),
    };
    store.put_feedback(&fb).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let results = store
        .list_feedback(&FeedbackFilter {
            run_id: Some(run_id),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "accuracy");
}

#[tokio::test]
#[ignore]
async fn patch_run_replacing_merge_tree() {
    let store = setup().await;
    let project = format!("ch-patch-{}", Uuid::new_v4().as_simple());

    let run = Run::builder("patch-target", RunType::Chain)
        .project(&project)
        .input("{}")
        .start();
    let run_id = run.run_id;

    store.put_runs(&[run]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let patch = RunPatch {
        status: Some(RunStatus::Success),
        output: Some("patched".into()),
        latency_ms: Some(99),
        ..Default::default()
    };
    store.patch_run(run_id, &project, &patch).await.unwrap();

    // FINAL deduplicates immediately for reads
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let fetched = store.get_run(run_id, &project).await.unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.status, RunStatus::Success);
    assert_eq!(fetched.output.as_deref(), Some("patched"));
}

// --- Project management ---

#[tokio::test]
#[ignore]
async fn project_crud() {
    let store = setup().await;

    let project = Project {
        id: Uuid::new_v4(),
        name: format!("ch-proj-{}", Uuid::new_v4().as_simple()),
        description: Some("A ClickHouse test project".into()),
        created_at: Utc::now(),
    };
    store.create_project(&project).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let fetched = store.get_project(project.id).await.unwrap();
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().name, project.name);

    let all = store.list_projects().await.unwrap();
    assert!(all.iter().any(|p| p.id == project.id));

    store.delete_project(project.id).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let deleted = store.get_project(project.id).await.unwrap();
    assert!(deleted.is_none());
}

// --- Dataset management ---

#[tokio::test]
#[ignore]
async fn dataset_and_examples_crud() {
    let store = setup().await;
    let project_id = Uuid::new_v4();

    let dataset = Dataset {
        id: Uuid::new_v4(),
        name: format!("ch-ds-{}", Uuid::new_v4().as_simple()),
        description: Some("Test dataset".into()),
        project_id: Some(project_id),
        created_at: Utc::now(),
    };
    store.create_dataset(&dataset).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let all = store.list_datasets(None).await.unwrap();
    assert!(all.iter().any(|d| d.id == dataset.id));

    let filtered = store.list_datasets(Some(project_id)).await.unwrap();
    assert!(filtered.iter().any(|d| d.id == dataset.id));

    // Add examples
    let examples = vec![
        Example {
            id: Uuid::new_v4(),
            dataset_id: dataset.id,
            input: r#"{"q":"2+2"}"#.into(),
            output: Some("4".into()),
            metadata: None,
            created_at: Utc::now(),
        },
        Example {
            id: Uuid::new_v4(),
            dataset_id: dataset.id,
            input: r#"{"q":"3+3"}"#.into(),
            output: Some("6".into()),
            metadata: None,
            created_at: Utc::now(),
        },
    ];
    store.add_examples(&examples).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let fetched = store.list_examples(dataset.id).await.unwrap();
    assert_eq!(fetched.len(), 2);
}
