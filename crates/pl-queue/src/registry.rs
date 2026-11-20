//! Task definitions like upstream (`@shared_task(...)` options) and retry rules.
//!
//! upstream: src/documents/tasks.py, src/documents/bulk_edit.py, src/documents/signals/handlers.py,
//! src/documents/workflows/webhooks.py, src/paperless_mail/tasks.py, src/paperless_mail/mail.py
//! (`@shared_task` decorators), celery/app/task.py@5.6.3 (`Task.max_retries = 3`,
//! `Task.retry`), celery/app/autoretry.py (`add_autoretry_behaviour`),
//! celery/utils/time.py (`get_exponential_backoff_interval`).

use rand::Rng;

/// Automatic retry (`autoretry_for` + `retry_backoff`/`retry_jitter`/`max_retries`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// `max_retries` (None = unlimited).
    pub max_retries: Option<u32>,
    /// `retry_backoff`: 0 = off (then `default_retry_delay` = 180 s), otherwise factor in seconds
    /// (`True` → 1).
    pub backoff_factor: u64,
    /// `retry_backoff_max` (Standard 600).
    pub backoff_max: u64,
    /// `retry_jitter` (Standard True).
    pub jitter: bool,
}

impl RetryPolicy {
    /// Celery default for `self.retry()` without `autoretry_for`: `max_retries=3`,
    /// `default_retry_delay=180`.
    pub const DEFAULT: RetryPolicy = RetryPolicy { max_retries: Some(3), backoff_factor: 0, backoff_max: 600, jitter: true };

    /// `get_exponential_backoff_interval(factor, retries, maximum, full_jitter)`, or without
    /// backoff `default_retry_delay` (180 s).
    pub fn countdown(&self, retries: u32, rng: &mut impl Rng) -> u64 {
        if self.backoff_factor == 0 {
            return 180;
        }
        let pow = 1u64.checked_shl(retries).unwrap_or(u64::MAX);
        let mut countdown = self.backoff_max.min(self.backoff_factor.saturating_mul(pow));
        if self.jitter {
            // random.randrange(countdown + 1)
            countdown = rng.gen_range(0..=countdown);
        }
        countdown
    }

    /// `retries > max_retries` → no further attempt (`retries` = retries so far).
    pub fn exhausted(&self, retries_so_far: u32) -> bool {
        self.max_retries.is_some_and(|m| retries_so_far + 1 > m)
    }
}

/// An upstream task.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TaskDef {
    /// Full Celery name.
    pub name: &'static str,
    /// `TRACKED_TASKS[name]` → `PaperlessTask.task_type`, otherwise None (no PaperlessTask).
    pub task_type: Option<&'static str>,
    /// `autoretry_for` rule (None = no automatic retry).
    pub autoretry: Option<RetryPolicy>,
    /// `ignore_result=True`.
    pub ignore_result: bool,
}

const fn def(name: &'static str, task_type: Option<&'static str>) -> TaskDef {
    TaskDef { name, task_type, autoretry: None, ignore_result: false }
}

const INDEX_RETRY: RetryPolicy = RetryPolicy { max_retries: Some(5), backoff_factor: 60, backoff_max: 600, jitter: true };

/// All `@shared_task` in upstream v3.4.2 (order as in the contract).
pub const TASKS: &[TaskDef] = &[
    def("documents.tasks.index_optimize", None),
    TaskDef { autoretry: Some(INDEX_RETRY), ignore_result: true, ..def("documents.tasks.index_document", None) },
    TaskDef { autoretry: Some(INDEX_RETRY), ignore_result: true, ..def("documents.tasks.remove_document_from_index", None) },
    def("documents.tasks.train_classifier", Some("train_classifier")),
    def("documents.tasks.consume_file", Some("consume_file")),
    def("documents.tasks.sanity_check", Some("sanity_check")),
    def("documents.tasks.bulk_update_documents", Some("bulk_update")),
    def("documents.tasks.update_document_content_maybe_archive_file", Some("reprocess_document")),
    def("documents.tasks.empty_trash", Some("empty_trash")),
    def("documents.tasks.check_scheduled_workflows", Some("check_workflows")),
    // PaperRust (not in v3.4.2): manual workflow run, name like paperless-ngx dev #14704
    def("documents.tasks.run_workflows_for_documents", None),
    def("documents.tasks.llmindex_index", Some("llm_index")),
    TaskDef {
        autoretry: Some(RetryPolicy { max_retries: Some(3), backoff_factor: 60, backoff_max: 600, jitter: true }),
        ..def("documents.tasks.apply_ai_suggestions", Some("apply_ai_suggestions"))
    },
    def("documents.tasks.update_document_in_llm_index", None),
    def("documents.tasks.remove_document_from_llm_index", None),
    def("documents.tasks.build_share_link_bundle", Some("build_share_link")),
    def("documents.tasks.cleanup_expired_share_link_bundles", None),
    // upstream fix a139bdff4 (#14722), after v3.4.2
    def("documents.tasks.clear_expired_sessions", None),
    def("documents.bulk_edit.delete", Some("bulk_delete")),
    // upstream dev 60a35edee (#14711, after v3.4.2): large bulk metadata changes in the
    // background (PaperRust: pl-documents-write::bulk_task, only with the PaperRust queue)
    def("documents.bulk_operations.execute_bulk_operation", Some("bulk_update")),
    def("documents.bulk_edit.restore_archive_serial_numbers_task", None),
    def("documents.signals.handlers.process_cf_select_update", None),
    TaskDef {
        // retry_backoff=True → Faktor 1; retry_jitter Standard True; max 600
        autoretry: Some(RetryPolicy { max_retries: Some(3), backoff_factor: 1, backoff_max: 600, jitter: true }),
        ..def("documents.workflows.webhooks.send_webhook", None)
    },
    def("paperless_mail.tasks.process_mail_accounts", Some("mail_fetch")),
    // message_date is a datetime (kombu JSON can encode it, the pl-core producer cannot)
    def("paperless_mail.mail.apply_mail_action", None),
    def("paperless_mail.mail.error_callback", None),
];

/// Definition for a name.
pub fn task_def(name: &str) -> Option<&'static TaskDef> {
    TASKS.iter().find(|t| t.name == name)
}

/// `TRACKED_TASKS.get(name)`.
pub fn tracked_task_type(name: &str) -> Option<&'static str> {
    task_def(name).and_then(|t| t.task_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn tracked_matches_pl_core() {
        for (name, ty) in pl_core::tasks::TRACKED_TASKS {
            assert_eq!(tracked_task_type(name), Some(*ty), "{name}");
        }
        let n = TASKS.iter().filter(|t| t.task_type.is_some()).count();
        assert_eq!(n, pl_core::tasks::TRACKED_TASKS.len());
    }

    #[test]
    fn backoff_like_celery() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let p = RetryPolicy { jitter: false, ..INDEX_RETRY };
        assert_eq!([0, 1, 2, 3, 4].map(|r| p.countdown(r, &mut rng)), [60, 120, 240, 480, 600]);
        for r in 0..10 {
            assert!(INDEX_RETRY.countdown(r, &mut rng) <= 600);
        }
        assert_eq!(RetryPolicy::DEFAULT.countdown(0, &mut rng), 180);
        // max_retries=5: retries 1..=5 allowed, the 6th not
        assert!(!INDEX_RETRY.exhausted(4));
        assert!(INDEX_RETRY.exhausted(5));
    }
}
