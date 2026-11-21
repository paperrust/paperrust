//! Side effects outside PostgreSQL: task publishing. (The django-treenode cache invalidation is gone
//! with the adjacency-list tags, migration `pl_objects.0001_paperrust_tag_adjacency`.)
//!
//! Tasks: `pl_core::tasks::send_task` – the task goes into `paperrust_task_queue` (the
//! `pl_core::tasks::Dispatcher` hung in by pl-server; the PgQueue creates the PaperlessTask row).
//!
//! upstream: src/documents/tasks.py@v3.4.2 (bulk_update_documents.apply_async(...))

use pl_core::tasks::{TaskRequest, trigger_source};
use pl_core::{ApiError, AppState};
use serde_json::json;

const BULK_UPDATE_DOCUMENTS: &str = "documents.tasks.bulk_update_documents";

/// `bulk_update_documents.apply_async(kwargs={"document_ids": ids}, headers={"trigger_source": "system"})`
/// via `pl_core::tasks::send_task` (PaperRust queue).
pub async fn send_bulk_update_documents(state: &AppState, doc_ids: &[i64]) -> Result<(), ApiError> {
    pl_core::tasks::send_task(state, bulk_update_request(doc_ids)).await?;
    Ok(())
}

pub fn bulk_update_request(doc_ids: &[i64]) -> TaskRequest {
    TaskRequest::new(BULK_UPDATE_DOCUMENTS).kwarg("document_ids", json!(doc_ids)).trigger_source(trigger_source::SYSTEM)
}

/// Django cache key (`default_key_func`): `"<KEY_PREFIX>:<version>:<key>"`.
pub fn cache_key(prefix: &str, key: &str) -> String {
    format!("{prefix}:1:{key}")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use pl_core::tasks::TaskRequest;
    use pl_core::tasks::{DispatchFuture, Dispatcher, TaskDispatch};
    use serde_json::json;
    use uuid::Uuid;

    #[derive(Default)]
    struct Rec(Mutex<Vec<TaskRequest>>);

    impl TaskDispatch for Rec {
        fn dispatch(&self, req: TaskRequest) -> DispatchFuture<'_> {
            self.0.lock().unwrap().push(req);
            Box::pin(async { Ok(Uuid::nil()) })
        }
    }

    fn lazy_state() -> pl_core::AppState {
        let cfg = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([(
            "PAPERLESS_SECRET_KEY",
            "pl-objects-tests-only-0123456789abcdefghijklmnopqrstuvwxyz",
        )]))
        .unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://u:p@127.0.0.1:1/x").unwrap();
        pl_core::AppState::with_pool(cfg, pool).unwrap()
    }

    /// W10/W14: with a dispatcher the task goes there (same name, kwargs, trigger).
    #[tokio::test]
    async fn bulk_update_goes_to_dispatcher() {
        let st = lazy_state();
        let rec = Arc::new(Rec::default());
        st.set_service(Dispatcher(rec.clone()));
        super::send_bulk_update_documents(&st, &[4, 2]).await.unwrap();
        let got = rec.0.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "documents.tasks.bulk_update_documents");
        assert_eq!(got[0].kwargs, json!({"document_ids": [4, 2]}).as_object().unwrap().clone());
        assert_eq!(got[0].headers["trigger_source"], json!("system"));
        assert!(got[0].args.is_empty());
    }

    #[test]
    fn django_cache_key() {
        assert_eq!(super::cache_key("", "treenode_list"), ":1:treenode_list");
        assert_eq!(super::cache_key("pl", "treenode_dict"), "pl:1:treenode_dict");
    }
}
