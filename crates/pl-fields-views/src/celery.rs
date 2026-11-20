//! Tasks of this crate, enqueued via `pl_core::tasks::send_task` (PostgreSQL queue,
//! hooked in by pl-server).
//!
//! upstream: src/documents/views.py@v3.4.2 (`build_share_link_bundle.apply_async(kwargs=…,
//! headers={"trigger_source": "manual"})`), src/documents/signals/handlers.py@v3.4.2
//! (`before_task_publish_handler` → PaperlessTask, handled by the queue).

use pl_core::AppState;
use pl_core::tasks::{TaskRequest, trigger_source};

use crate::shim::ApiResult;

pub const BUILD_SHARE_LINK_BUNDLE: &str = "documents.tasks.build_share_link_bundle";

/// Upstream with a model instance as argument; here with `custom_field_id` (pl-consumer).
pub const PROCESS_CF_SELECT_UPDATE: &str = "documents.signals.handlers.process_cf_select_update";

pub fn build_share_link_bundle_request(bundle_id: i64) -> TaskRequest {
    TaskRequest::new(BUILD_SHARE_LINK_BUNDLE)
        .kwarg("bundle_id", bundle_id)
        .trigger_source(trigger_source::MANUAL)
}

pub async fn enqueue_build_share_link_bundle(state: &AppState, bundle_id: i64) -> ApiResult<()> {
    pl_core::tasks::send_task(state, build_share_link_bundle_request(bundle_id)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shape() {
        let r = build_share_link_bundle_request(5);
        assert_eq!(r.name, BUILD_SHARE_LINK_BUNDLE);
        assert_eq!(r.kwargs.get("bundle_id"), Some(&serde_json::json!(5)));
        assert_eq!(r.headers.get("trigger_source"), Some(&serde_json::json!("manual")));
        assert!(r.args.is_empty());
    }
}
