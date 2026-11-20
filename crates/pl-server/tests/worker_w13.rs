//! E2E (W13/W15): `sanity_check`, `check_scheduled_workflows` and `empty_trash` run in the
//! embedded worker (PgQueue) against the W13 copy `pl_w13_ref` (never against `paperless`).
//!
//! ```sh
//! source tests/parity/pl-documents-write/w13/w13env.sh
//! cargo test -p pl-server -j 1 --test worker_w13 -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::Duration;

use pl_queue::{EnqueueOptions, TaskStatus};
use serde_json::{Map, Value, json};

fn reset() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/pl-documents-write/w13/reset.sh");
    let st = std::process::Command::new("bash").arg(script).stdout(std::process::Stdio::null()).status().expect("reset.sh");
    assert!(st.success(), "reset failed");
}

#[tokio::test]
#[ignore = "braucht die W13-Kopie (source tests/parity/pl-documents-write/w13/w13env.sh)"]
async fn worker_runs_sanity_check_and_scheduled_workflows() {
    assert_eq!(std::env::var("PAPERLESS_DBNAME").as_deref(), Ok("pl_w13_ref"), "source w13env.sh first");
    reset();
    let state = pl_core::AppState::from_env().await.expect("state");
    let cfg = pl_server::worker::queue_config(&state.config().env).expect("queue config");
    let setup = pl_server::worker::setup_queue(&state, cfg.clone()).await.expect("setup queue");
    let ctx = pl_server::worker::WorkerContext {
        state: state.clone(),
        queue: setup.queue.clone(),
        events: setup.events.clone(),
        cfg: cfg.clone(),
    };
    let registry = pl_server::worker::build_registry(&ctx).await.expect("registry");
    for name in [
        "documents.tasks.sanity_check",
        "documents.tasks.check_scheduled_workflows",
        "documents.tasks.empty_trash",
        "documents.tasks.update_document_content_maybe_archive_file",
        "documents.bulk_edit.delete",
        "documents.bulk_edit.restore_archive_serial_numbers_task",
    ] {
        assert!(registry.is_ported(name), "{name} registered");
    }
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let worker = pl_queue::Worker::new(setup.pg.clone(), registry.into_handlers(), cfg, setup.events.clone());
    let handle = tokio::spawn(async move { worker.run(stop_rx).await });

    let mut kw = Map::new();
    kw.insert("raise_on_error".into(), Value::Bool(false));
    let sanity = setup.queue.enqueue("documents.tasks.sanity_check", kw, EnqueueOptions::trigger("manual")).await.expect("enqueue");
    let sched = setup
        .queue
        .enqueue("documents.tasks.check_scheduled_workflows", Map::new(), EnqueueOptions::trigger("scheduled"))
        .await
        .expect("enqueue");
    let trash = setup
        .queue
        .enqueue("documents.tasks.empty_trash", json!({"doc_ids": [196]}).as_object().cloned().unwrap_or_default(), EnqueueOptions::default())
        .await
        .expect("enqueue");

    let mut done = Vec::new();
    for id in [sanity, sched, trash] {
        let mut info = None;
        for _ in 0..600 {
            if let Some(i) = setup.queue.status(id).await.expect("status")
                && i.status.is_ready()
            {
                info = Some(i);
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let info = info.expect("task finished in time");
        println!("{id}: {:?} result={:?} error={:?}", info.status, info.result, info.error);
        done.push(info);
    }
    let _ = stop_tx.send(true);
    let _ = handle.await;

    assert_eq!(done[0].status, TaskStatus::Success, "sanity_check");
    let text = done[0].result.as_ref().and_then(Value::as_str).unwrap_or_default().to_string();
    assert!(text == "No issues detected." || text.ends_with("found.") || text.ends_with("Check logs for details."), "{text}");
    assert_eq!(done[1].status, TaskStatus::Success, "check_scheduled_workflows");
    assert_eq!(done[2].status, TaskStatus::Success, "empty_trash");
    let gone: Option<i32> = sqlx::query_scalar("SELECT id FROM documents_document WHERE id = 196").fetch_optional(state.db()).await.unwrap();
    assert_eq!(gone, None);
    // PaperlessTask rows of the tracked tasks
    let types: Vec<(String, String)> = sqlx::query_as(
        "SELECT task_type, status FROM documents_paperlesstask WHERE task_id = ANY($1) ORDER BY task_type",
    )
    .bind(vec![sanity.to_string(), sched.to_string(), trash.to_string()])
    .fetch_all(state.db())
    .await
    .unwrap();
    assert_eq!(types.len(), 3, "{types:?}");
    assert!(types.iter().all(|(_, s)| s == "success"), "{types:?}");
    reset();
}
