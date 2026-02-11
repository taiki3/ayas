//! Integration tests for PostgresSmithStore.
//!
//! Requires: `docker compose -f docker-compose.test.yml up -d postgres`
//! Run with: `cargo test -p ayas-smith --features postgres --test integration_postgres`

#![cfg(feature = "postgres")]

use chrono::Utc;
use uuid::Uuid;

use ayas_smith::postgres_store::PostgresSmithStore;
use ayas_smith::store::SmithStore;
use ayas_smith::types::*;

const TEST_URL: &str = "host=localhost port=15432 user=ayas password=ayas dbname=ayas_test";

async fn setup() -> PostgresSmithStore {
    PostgresSmithStore::connect(TEST_URL)
        .await
        .expect("Failed to connect to PostgreSQL — is docker-compose.test.yml running?")
}

fn make_run(name: &str, project: &str, run_type: RunType) -> Run {
    Run::builder(name, run_type)
        .project(project)
        .input(r#"{"q":"hello"}"#)
        .finish_ok(r#"{"a":"world"}"#)
}

#[tokio::test]
async fn put_and_get_run() {
    let store = setup().await;
    let run = make_run("pg-test-1", "integ-test", RunType::Chain);
    let run_id = run.run_id;

    store.put_runs(&[run]).await.unwrap();

    let fetched = store.get_run(run_id, "integ-test").await.unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.run_id, run_id);
    assert_eq!(fetched.name, "pg-test-1");
    assert_eq!(fetched.status, RunStatus::Success);
    assert_eq!(fetched.project, "integ-test");
}

#[tokio::test]
async fn list_runs_with_filter() {
    let store = setup().await;
    let project = format!("list-test-{}", Uuid::new_v4().as_simple());

    let r1 = Run::builder("chain-1", RunType::Chain)
        .project(&project)
        .input("{}")
        .finish_ok("ok");
    let r2 = Run::builder("llm-1", RunType::Llm)
        .project(&project)
        .input("{}")
        .finish_llm("resp", 10, 20, 30);
    let r3 = Run::builder("tool-1", RunType::Tool)
        .project(&project)
        .input("{}")
        .finish_err("timeout");

    store.put_runs(&[r1, r2, r3]).await.unwrap();

    // Filter by project
    let all = store
        .list_runs(&RunFilter {
            project: Some(project.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(all.len(), 3);

    // Filter by run_type
    let llms = store
        .list_runs(&RunFilter {
            project: Some(project.clone()),
            run_type: Some(RunType::Llm),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(llms.len(), 1);
    assert_eq!(llms[0].name, "llm-1");

    // Filter by status
    let errors = store
        .list_runs(&RunFilter {
            project: Some(project.clone()),
            status: Some(RunStatus::Error),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].name, "tool-1");
}

#[tokio::test]
async fn patch_run_lifecycle() {
    let store = setup().await;
    let project = format!("patch-test-{}", Uuid::new_v4().as_simple());

    // Phase 1: start run
    let run = Run::builder("lifecycle", RunType::Chain)
        .project(&project)
        .input(r#"{"step":1}"#)
        .start();
    let run_id = run.run_id;

    store.put_runs(&[run]).await.unwrap();

    let fetched = store.get_run(run_id, &project).await.unwrap().unwrap();
    assert_eq!(fetched.status, RunStatus::Running);
    assert!(fetched.end_time.is_none());

    // Phase 2: patch with completion
    let patch = RunPatch {
        end_time: Some(Utc::now()),
        output: Some(r#"{"result":"done"}"#.into()),
        status: Some(RunStatus::Success),
        latency_ms: Some(42),
        ..Default::default()
    };
    store.patch_run(run_id, &project, &patch).await.unwrap();

    let fetched = store.get_run(run_id, &project).await.unwrap().unwrap();
    assert_eq!(fetched.status, RunStatus::Success);
    assert!(fetched.end_time.is_some());
    assert_eq!(fetched.output.as_deref(), Some(r#"{"result":"done"}"#));
    assert_eq!(fetched.latency_ms, Some(42));
}

#[tokio::test]
async fn trace_hierarchy() {
    let store = setup().await;
    let project = format!("trace-test-{}", Uuid::new_v4().as_simple());
    let trace_id = Uuid::new_v4();

    let parent = Run::builder("parent-chain", RunType::Chain)
        .run_id(Uuid::new_v4())
        .trace_id(trace_id)
        .project(&project)
        .input("{}")
        .finish_ok("done");
    let parent_id = parent.run_id;

    let child = Run::builder("child-llm", RunType::Llm)
        .run_id(Uuid::new_v4())
        .trace_id(trace_id)
        .parent_run_id(parent_id)
        .project(&project)
        .input("{}")
        .finish_llm("resp", 5, 10, 15);

    store.put_runs(&[parent, child]).await.unwrap();

    // get_trace
    let trace = store.get_trace(trace_id, &project).await.unwrap();
    assert_eq!(trace.len(), 2);

    // get_children
    let children = store.get_children(parent_id, &project).await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name, "child-llm");
}

#[tokio::test]
async fn token_usage_and_latency() {
    let store = setup().await;
    let project = format!("stats-test-{}", Uuid::new_v4().as_simple());

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

    let filter = RunFilter {
        project: Some(project.clone()),
        ..Default::default()
    };

    let usage = store.token_usage_summary(&filter).await.unwrap();
    assert_eq!(usage.total_input_tokens, 300);
    assert_eq!(usage.total_output_tokens, 150);
    assert_eq!(usage.total_tokens, 450);
    assert_eq!(usage.run_count, 2);

    let latency = store.latency_percentiles(&filter).await.unwrap();
    assert!(latency.p50 > 0.0);
}

#[tokio::test]
async fn feedback_crud() {
    let store = setup().await;
    let run_id = Uuid::new_v4();

    // Put a run first (feedback references run_id)
    let run = Run::builder("fb-test", RunType::Chain)
        .run_id(run_id)
        .project("feedback-proj")
        .input("{}")
        .finish_ok("ok");
    store.put_runs(&[run]).await.unwrap();

    let fb = Feedback {
        id: Uuid::new_v4(),
        run_id,
        key: "accuracy".into(),
        score: 0.9,
        comment: Some("Good".into()),
        created_at: Utc::now(),
    };
    store.put_feedback(&fb).await.unwrap();

    let results = store
        .list_feedback(&FeedbackFilter {
            run_id: Some(run_id),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "accuracy");
    assert!((results[0].score - 0.9).abs() < f64::EPSILON);
}
