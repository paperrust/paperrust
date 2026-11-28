//! Embedded worker: task queue in [`AppState`], task handlers under the upstream task names,
//! worker loop, scheduler (beat) and consumption folder watcher in the `paperrust` process.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (worker settings, beat schedule),
//! src/paperless/settings/custom.py@v3.4.2 (parse_beat_schedule),
//! src/documents/tasks.py@v3.4.2 (task bodies), src/documents/management/commands/
//! document_consumer.py@v3.4.2 (folder watcher), docs/contract/pl-queue.md.
//!
//! * [`queue_config`]: `QueueConfig` from the environment (the only queue is PostgreSQL;
//!   `PAPERRUST_QUEUE`/`PAPERRUST_QUEUE_BACKEND` are ignored, see `pl_core::config::retired_warnings`).
//! * [`setup_queue`]: `PgQueue` + `EventBus` into [`AppState`]
//!   (`state.service::<Arc<dyn TaskQueue>>()`, `state.service::<EventBus>()`, task dispatcher);
//!   the migration `crates/pl-queue/migrations/0001` is applied (idempotent).
//! * [`start_background`]: worker + beat + folder watcher (`--worker`, `--worker-only`);
//!   status events between processes over `LISTEN/NOTIFY`.
//! * [`TaskRegistry`]: handlers by upstream task name; tasks without a Rust port are
//!   registered as "not ported" and fail with a clear `NotImplementedError` (never silently).
//!   Mail tasks: [`register_mail_handlers`] (W6).
//! * AI (feature `ai`, W10): [`register_ai_handlers`] (`llmindex_index`, `apply_ai_suggestions`,
//!   `update_document_in_llm_index`, `remove_document_from_llm_index` + PaperRust tasks
//!   `paperrust.ai.*`), the signal receiver `add_or_update_document_in_llm_index` after
//!   `consume_file` ([`ai_after_consumption`]), the LLM index update of `bulk_update_documents`,
//!   `pl_ai::server::index::AiTasks` → PgQueue, beat entry for the AI trigger tag
//!   ([`ai_beat_entries`]) and the migration `crates/pl-ai/migrations/0001_paperrust_ai.sql`
//!   ([`apply_ai_migration`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use pl_core::AppState;
use pl_core::config::{ConfigError, Env};
use pl_queue::{EventBus, Handlers, PgQueue, QueueConfig, TaskContext, TaskError, TaskHandler, TaskQueue};
use serde_json::{Map, Value};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::routing::MountPlan;

/// How the process runs (`paperrust serve [--worker|--worker-only|--no-worker]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerMode {
    /// HTTP server only (default).
    NoWorker,
    /// HTTP server + worker + beat + folder watcher.
    WithWorker,
    /// Worker + beat + folder watcher, no HTTP server.
    WorkerOnly,
}

impl WorkerMode {
    pub fn runs_worker(self) -> bool {
        matches!(self, Self::WithWorker | Self::WorkerOnly)
    }
    pub fn serves_http(self) -> bool {
        matches!(self, Self::NoWorker | Self::WithWorker)
    }
}

/// Upstream task names.
pub mod names {
    pub const CONSUME_FILE: &str = "documents.tasks.consume_file";
    pub const INDEX_DOCUMENT: &str = "documents.tasks.index_document";
    pub const REMOVE_DOCUMENT_FROM_INDEX: &str = "documents.tasks.remove_document_from_index";
    pub const INDEX_OPTIMIZE: &str = "documents.tasks.index_optimize";
    pub const UPDATE_CONTENT_MAYBE_ARCHIVE: &str = "documents.tasks.update_document_content_maybe_archive_file";
    pub const TRAIN_CLASSIFIER: &str = "documents.tasks.train_classifier";
    pub const SANITY_CHECK: &str = "documents.tasks.sanity_check";
    pub const EMPTY_TRASH: &str = "documents.tasks.empty_trash";
    /// upstream fix a139bdff4 (#14722, after v3.4.2), beat "Clear expired sessions".
    pub const CLEAR_EXPIRED_SESSIONS: &str = "documents.tasks.clear_expired_sessions";
    pub const BULK_UPDATE_DOCUMENTS: &str = "documents.tasks.bulk_update_documents";
    pub const BULK_EDIT_DELETE: &str = "documents.bulk_edit.delete";
    pub const RESTORE_ARCHIVE_SERIAL_NUMBERS: &str = "documents.bulk_edit.restore_archive_serial_numbers_task";
    pub const CHECK_SCHEDULED_WORKFLOWS: &str = "documents.tasks.check_scheduled_workflows";
    pub const PROCESS_MAIL_ACCOUNTS: &str = "paperless_mail.tasks.process_mail_accounts";
    pub const APPLY_MAIL_ACTION: &str = "paperless_mail.mail.apply_mail_action";
    pub const MAIL_ERROR_CALLBACK: &str = "paperless_mail.mail.error_callback";
    pub const SEND_WEBHOOK: &str = "documents.workflows.webhooks.send_webhook";
    pub const LLMINDEX_INDEX: &str = "documents.tasks.llmindex_index";
    pub const APPLY_AI_SUGGESTIONS: &str = "documents.tasks.apply_ai_suggestions";
    pub const UPDATE_DOCUMENT_IN_LLM_INDEX: &str = "documents.tasks.update_document_in_llm_index";
    pub const REMOVE_DOCUMENT_FROM_LLM_INDEX: &str = "documents.tasks.remove_document_from_llm_index";
    /// PaperRust: `document_llmindex compact` as a task (upstream: management command only).
    pub const AI_LLM_INDEX_COMPACT: &str = "paperrust.ai.llm_index_compact";
    /// PaperRust: automatic AI processing of documents carrying the trigger tag (beat).
    pub const AI_PROCESS_TRIGGER_TAG: &str = "paperrust.ai.process_trigger_tag";
    /// PaperRust: automatic AI processing of one document (`document_id`, `trigger`).
    pub const AI_PROCESS_DOCUMENT: &str = "paperrust.ai.process_document";
    /// PaperRust: AI archive backfill job (`job_id`, table `paperrust_ai_job`).
    pub const AI_BACKFILL: &str = "paperrust.ai.backfill";
    /// PaperRust: timeline date extraction for all (stale) documents (`force`), pl-timeline.
    pub const TIMELINE_BACKFILL: &str = "paperrust.timeline.backfill";
}

/// `QueueConfig::from_env`.
pub fn queue_config(env: &Env) -> Result<QueueConfig, ConfigError> {
    QueueConfig::from_env(env)
}

/// Queue and event bus of this process.
#[derive(Clone)]
pub struct QueueSetup {
    pub cfg: QueueConfig,
    pub queue: Arc<dyn TaskQueue>,
    pub events: EventBus,
    pub pg: PgQueue,
}

/// Build the queue, apply the queue migration and hang queue + bus into `state`.
pub async fn setup_queue(state: &AppState, cfg: QueueConfig) -> Result<QueueSetup, String> {
    let events = EventBus::default();
    pl_queue::migrations::apply(state.db()).await.map_err(|e| format!("applying the task queue migration failed: {e}"))?;
    tracing::info!("task queue: PostgreSQL (paperrust_task_queue, migration 0001 applied)");
    let pg = PgQueue::new(state.db().clone(), cfg.max_deliveries);
    let queue: Arc<dyn TaskQueue> = Arc::new(pg.clone());
    state.set_service::<Arc<dyn TaskQueue>>(queue.clone());
    // pl-ai enqueues its tasks (index_document, llmindex_index, update_document_in_llm_index, …)
    // through this sink.
    #[cfg(feature = "ai")]
    state.set_service(pl_ai::server::index::AiTasks(Arc::new(QueueAiSink(queue.clone()))));
    state.set_service::<EventBus>(events.clone());
    // Handler crates send through `pl_core::tasks::send_task` → this dispatcher → PgQueue.
    state.set_service(pl_core::tasks::Dispatcher(Arc::new(QueueDispatch(queue.clone()))));
    Ok(QueueSetup { cfg, queue, events, pg })
}

/// `pl_core::tasks::TaskDispatch` on a `TaskQueue` (W14): `TaskRequest` →
/// `enqueue(name, kwargs, EnqueueOptions{args, trigger_source, headers, countdown, expires,
/// task_id, time_limit})`. The PgQueue creates the `PaperlessTask` row itself.
pub struct QueueDispatch(pub Arc<dyn TaskQueue>);

/// Mapping of [`QueueDispatch`].
pub fn enqueue_options(req: &pl_core::tasks::TaskRequest) -> pl_queue::EnqueueOptions {
    let mut headers = req.headers.clone();
    let trigger_source = match headers.remove("trigger_source") {
        Some(Value::String(s)) => Some(s),
        Some(other) => {
            headers.insert("trigger_source".into(), other);
            None
        }
        None => None,
    };
    pl_queue::EnqueueOptions {
        args: req.args.clone(),
        task_id: req.task_id,
        trigger_source,
        headers,
        countdown: req.countdown,
        expires: req.expires,
        time_limit: req.time_limit.0,
        ..Default::default()
    }
}

impl pl_core::tasks::TaskDispatch for QueueDispatch {
    fn dispatch(&self, req: pl_core::tasks::TaskRequest) -> pl_core::tasks::DispatchFuture<'_> {
        Box::pin(async move {
            let opts = enqueue_options(&req);
            self.0
                .enqueue(&req.name, req.kwargs, opts)
                .await
                .map_err(|e| pl_core::ApiError::internal(format!("enqueue {} failed: {e}", req.name)))
        })
    }
}

/// W1: `execute_password_removal_action` – passwords of the action in order with
/// `bulk_edit.remove_password(update_document=True)` (pl-documents-write), in the background. For the
/// ingestion trigger the action registers itself and runs on "ingestion finished" with the new
/// document (both hooks share the same dependencies).
fn install_password_removal(
    effects: &mut pl_mail::effects::MailWorkflowEffects,
    pool: sqlx::PgPool,
    queue: Arc<dyn pl_queue::TaskQueue>,
    cfg: &pl_core::Config,
) {
    let deps = Arc::new(pl_documents_write::workflow_password::PasswordRemovalDeps::new(
        pool,
        queue,
        pl_documents_write::files::MediaCfg::from_config(cfg),
        cfg.scratch_dir.clone(),
    ));
    let d = deps.clone();
    effects.password_removal =
        Some(Arc::new(move |ctx: &pl_consumer::stages::ActionContext| pl_documents_write::workflow_password::spawn(d.clone(), ctx)));
    effects.consumption_finished = Some(Arc::new(move |doc_id: i64, original_file: &std::path::Path| {
        pl_documents_write::workflow_password::consumption_finished(deps.clone(), doc_id, original_file)
    }));
}

/// Runtime hooks of the handler crates (every mode, before routes serve requests):
/// `pl_documents_write::hooks::set_workflow_effects` = pl-mail's e-mail/webhook/password-removal
/// actions (same implementation the consumer uses), so "Document Updated" workflows run by
/// pl-documents-write send mails/webhooks instead of only logging.
///
/// `pl_documents_write::hooks::set_pdf_ops` = `pl_documents_write::pdf_edit::NativePdfOps` (bulk_edit
/// PDF methods with pl-pdfops; results go to `consume_file` in the PaperRust queue).
///
/// `pl_documents_write::hooks::set_document_updated_hook` (feature `ai`) = receiver
/// `add_or_update_document_in_llm_index` of `document_updated` (PUT/PATCH of a document).
pub fn install_hooks(state: &AppState, setup: &QueueSetup) -> Result<(), String> {
    let mut effects = pl_mail::effects::MailWorkflowEffects::new(state.db().clone(), Some(setup.queue.clone()), state.config())
        .map_err(|e| format!("workflow effects setup failed: {e}"))?;
    install_password_removal(&mut effects, state.db().clone(), setup.queue.clone(), &state.live_config());
    pl_documents_write::hooks::set_workflow_effects(Arc::new(pl_mail::effects::SharedEffects(Arc::new(effects))));
    pl_documents_write::hooks::set_pdf_ops(Arc::new(pl_documents_write::pdf_edit::NativePdfOps));
    // `all=true` selections of bulk_download (pl-files cannot depend on pl-documents).
    state.set_service(pl_files::bulk::SelectionResolver(Arc::new(|st: AppState, user, filters, excluded| {
        Box::pin(async move { pl_documents::selection_filter::resolve_all(&st, &user, filters.as_ref(), &excluded).await })
    })));
    // `llm_index_exists()` for `ApplicationConfigurationViewSet.perform_update` (pl-system).
    #[cfg(feature = "ai")]
    state.set_service(pl_system::config_write::LlmIndexExists(Arc::new(|st: AppState| {
        Box::pin(async move { pl_ai::server::index::llm_index_exists(&st).await.unwrap_or(false) })
    })));
    #[cfg(feature = "ai")]
    pl_documents_write::hooks::set_document_updated_hook(Arc::new(AiDocumentUpdated { queue: setup.queue.clone() }));
    Ok(())
}

/// `add_or_update_document_in_llm_index(sender, document)` on `document_updated`:
/// `if AIConfig().llm_index_enabled: update_document_in_llm_index.apply_async(kwargs={"document": …})`
/// (Rust task: `document_id`).
#[cfg(feature = "ai")]
pub struct AiDocumentUpdated {
    pub queue: Arc<dyn TaskQueue>,
}

#[cfg(feature = "ai")]
impl pl_documents_write::hooks::DocumentUpdatedHook for AiDocumentUpdated {
    fn document_updated<'a>(
        &'a self,
        state: &'a AppState,
        document_id: i64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let Some(cfg) = ai_config(state).await else { return };
            if !cfg.llm_index_enabled() {
                return;
            }
            if let Err(e) = self
                .queue
                .enqueue(names::UPDATE_DOCUMENT_IN_LLM_INDEX, document_kwargs(document_id), pl_queue::EnqueueOptions::default())
                .await
            {
                tracing::error!(target: "paperless.handlers", document_id, error = %e, "could not queue update_document_in_llm_index");
            }
        })
    }
}

/// Apply `crates/pl-ai/migrations/0001_paperrust_ai.sql` (expand-only, idempotent): the worker
/// (review list, audit) and the AI routes (`ai.*`: review, run, MCP) use its tables. Returns
/// whether it was applied.
#[cfg(feature = "ai")]
pub async fn apply_ai_migration(state: &AppState, _plan: &MountPlan) -> Result<bool, String> {
    pl_ai::server::migrations::apply(state.db()).await.map_err(|e| format!("applying the AI migration 0001_paperrust_ai failed: {e}"))?;
    tracing::info!("AI tables: migration 0001_paperrust_ai applied (paperrust_taxonomy_alias, paperrust_ai_audit, paperrust_ai_review)");
    Ok(true)
}

/// Without feature `ai` there is nothing to migrate.
#[cfg(not(feature = "ai"))]
pub async fn apply_ai_migration(_state: &AppState, _plan: &MountPlan) -> Result<bool, String> {
    Ok(false)
}

/// [`pl_ai::server::index::AiTaskSink`] on the PaperRust queue.
#[cfg(feature = "ai")]
pub struct QueueAiSink(pub Arc<dyn TaskQueue>);

#[cfg(feature = "ai")]
impl pl_ai::server::index::AiTaskSink for QueueAiSink {
    fn enqueue(&self, name: &str, kwargs: Map<String, Value>, trigger_source: &str) -> futures::future::BoxFuture<'_, Result<(), String>> {
        let name = name.to_string();
        let opts = pl_queue::EnqueueOptions::trigger(trigger_source);
        Box::pin(async move { self.0.enqueue(&name, kwargs, opts).await.map(|_| ()).map_err(|e| e.to_string()) })
    }
}

/// Everything a handler factory needs.
#[derive(Clone)]
pub struct WorkerContext {
    pub state: AppState,
    pub queue: Arc<dyn TaskQueue>,
    pub events: EventBus,
    pub cfg: QueueConfig,
}

/// Handlers by task name plus the set of tasks with a working Rust port (beat only schedules
/// those).
#[derive(Default, Clone)]
pub struct TaskRegistry {
    handlers: Handlers,
    ported: BTreeSet<String>,
    not_ported: BTreeSet<String>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a working handler (upstream retry policy from `pl_queue::registry`).
    pub fn register(mut self, name: &str, handler: impl TaskHandler + 'static) -> Self {
        // Read-only mode after a ransomware alarm (pl-guard): writing tasks fail.
        self.handlers = self.handlers.register(name, pl_guard::readonly::gate(name, handler));
        self.not_ported.remove(name);
        self.ported.insert(name.to_string());
        self
    }

    /// Register a task without Rust port: it is claimed and fails with
    /// `NotImplementedError` (visible in `/api/tasks/`) instead of waiting forever.
    pub fn not_ported(mut self, name: &str, missing: &'static str) -> Self {
        let task = name.to_string();
        self.handlers = self.handlers.register(name, move |_ctx: TaskContext| {
            let task = task.clone();
            async move { Err(not_ported_error(&task, missing)) }
        });
        self.ported.remove(name);
        self.not_ported.insert(name.to_string());
        self
    }

    pub fn is_ported(&self, name: &str) -> bool {
        self.ported.contains(name)
    }

    pub fn ported(&self) -> impl Iterator<Item = &str> {
        self.ported.iter().map(String::as_str)
    }

    pub fn not_ported_names(&self) -> impl Iterator<Item = &str> {
        self.not_ported.iter().map(String::as_str)
    }

    pub fn into_handlers(self) -> Handlers {
        self.handlers
    }

    /// Let another crate register `names` on the raw [`Handlers`] (e.g.
    /// `pl_mail::fetch::register_mail_handlers`); they count as ported.
    pub fn with_handlers(mut self, names: &[&str], f: impl FnOnce(Handlers) -> Handlers) -> Self {
        self.handlers = f(std::mem::take(&mut self.handlers));
        for n in names {
            self.not_ported.remove(*n);
            self.ported.insert((*n).to_string());
        }
        self
    }
}

/// Error of a task without Rust port.
pub fn not_ported_error(task: &str, missing: &str) -> TaskError {
    TaskError::fail("NotImplementedError", format!("{task} is not ported to PaperRust yet (noch nicht portiert): {missing}"))
}

/// Positional (`args[i]`) or keyword argument of a task.
fn task_arg<'a>(ctx: &'a TaskContext, index: usize, name: &str) -> Option<&'a Value> {
    ctx.kwargs.get(name).or_else(|| ctx.args.get(index))
}

fn int_arg(ctx: &TaskContext, index: usize, name: &str) -> Result<i64, TaskError> {
    match task_arg(ctx, index, name) {
        Some(v) => v.as_i64().ok_or_else(|| TaskError::fail("TypeError", format!("{name} must be an integer"))),
        None => {
            Err(TaskError::fail("TypeError", format!("{}() missing 1 required positional argument: '{name}'", short_name(&ctx.task_name))))
        }
    }
}

fn short_name(task: &str) -> &str {
    task.rsplit('.').next().unwrap_or(task)
}

fn indexer_error(e: pl_indexer::IndexerError) -> TaskError {
    match e {
        // autoretry_for=(SearchIndexLockError,), max_retries=5, retry_backoff=60, jitter
        pl_indexer::IndexerError::LockExhausted { .. } => TaskError::autoretry("SearchIndexLockError", e.to_string()),
        // FINDINGS B8: transient file/IO errors of the index (Windows: "access denied", file
        // already exists) have already been retried several times by the indexer; afterwards do not
        // fail for good but run the task again later (do not lose the index change).
        ref t if t.is_transient() => TaskError::retry("OSError", e.to_string(), Some(60.0)),
        pl_indexer::IndexerError::Io(_) | pl_indexer::IndexerError::MissingIndexDir(_) => TaskError::fail("OSError", e.to_string()),
        other => TaskError::fail("ValueError", other.to_string()),
    }
}

/// All handlers of this worker.
pub async fn build_registry(ctx: &WorkerContext) -> Result<TaskRegistry, String> {
    let reg = register_document_handlers(TaskRegistry::new(), ctx).await?;
    let reg = register_mail_handlers(reg, ctx)?;
    let reg = register_guard_handlers(reg, ctx);
    Ok(register_ai_handlers(reg, ctx))
}

/// Encrypted backup (pl-guard, schedule `PAPERRUST_BACKUP_CRON`).
pub fn register_guard_handlers(reg: TaskRegistry, ctx: &WorkerContext) -> TaskRegistry {
    let st = ctx.state.clone();
    reg.register(pl_guard::TASK_BACKUP, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let trigger = c.headers.get("trigger_source").and_then(Value::as_str).unwrap_or("manual").to_string();
            pl_guard::backup_task(&st, &trigger).await.map_err(|e| TaskError::fail("BackupError", e))
        }
    })
}

/// `documents.tasks.*` handlers (pl-consumer, pl-indexer, pl-classifier).
/// Ingestion building blocks with the current settings (environment + UI + ApplicationConfiguration).
pub async fn build_consumer(ctx: &WorkerContext) -> Result<pl_consumer::Consumer, String> {
    let pool = ctx.state.db().clone();
    let mut consumer =
        pl_consumer::Consumer::from_env(pool.clone(), ctx.events.clone()).await.map_err(|e| format!("consumer setup failed: {e}"))?;
    consumer.queue = Some(ctx.queue.clone());
    // Workflow actions e-mail / webhook / password removal (pl-mail, W6) instead of logging only.
    // Live configuration (SMTP etc. from the web UI, docs/SETTINGS.md WP4b).
    let mut effects = pl_mail::effects::MailWorkflowEffects::new(pool.clone(), Some(ctx.queue.clone()), &ctx.state.live_config())
        .map_err(|e| format!("workflow effects setup failed: {e}"))?;
    // W1: workflow action "remove password" (bulk_edit.remove_password as a new version).
    install_password_removal(&mut effects, pool.clone(), ctx.queue.clone(), &ctx.state.live_config());
    consumer.effects = Arc::new(pl_mail::effects::SharedEffects(Arc::new(effects)));
    // W15: clear_document_caches (Redis + LLM suggestions) and the full soft-delete cascade.
    consumer.caches = Arc::new(RedisDocumentCaches { state: ctx.state.clone() });
    consumer.deleter =
        Arc::new(DocumentsWriteSoftDelete { media: pl_documents_write::files::MediaCfg::from_config(&ctx.state.live_config()) });
    // Workflows new: ensure tables, attach the AI condition to pl-ai (without `ai`: ignored, marked).
    pl_automation::init(&ctx.state).await;
    // AI text recognition (pl-core `vision`, implementation pl-ai); unchanged without feature `ai`.
    #[cfg(feature = "ai")]
    {
        let vision: Arc<dyn pl_core::vision::VisionOcr> = Arc::new(AiVisionOcr { state: ctx.state.clone() });
        ctx.state.set_service(pl_core::vision::VisionOcrService(vision.clone()));
        consumer.vision = Some(vision);
    }
    if let Err(e) = consumer.settings.ensure_directories() {
        tracing::warn!(error = %e, "cannot create media/scratch directories");
    }
    // tests/chaos C5: clean up working folders of aborted ingestions (hard abort) – only
    // folders older than a task's hard time limit + 10 min.
    let age =
        std::time::Duration::from_secs_f64(queue_config(&ctx.state.config().env).map(|c| c.time_limit_secs).unwrap_or(1800.0) + 600.0);
    tokio::spawn(pl_consumer::consumer::scratch_sweeper(consumer.settings.scratch_dir.clone(), age));
    Ok(consumer)
}

/// A2: `document_index reindex --if-needed` (pl-indexer `rebuild(if_needed=true)`).
async fn reindex_if_needed(ix: &pl_indexer::Indexer, pool: &sqlx::PgPool) {
    let opts = pl_indexer::RebuildOptions { if_needed: true, ..Default::default() };
    match ix.rebuild(pool, &opts).await {
        Ok(s) if s.skipped_up_to_date => tracing::debug!(target: "paperless.index", "Search index is up to date."),
        Ok(s) => tracing::info!(target: "paperless.index", "Search index rebuilt at startup ({} documents)", s.documents),
        Err(e) => tracing::error!(target: "paperless.index", "Search index rebuild at startup failed: {e}"),
    }
}

pub async fn register_document_handlers(reg: TaskRegistry, ctx: &WorkerContext) -> Result<TaskRegistry, String> {
    let pool = ctx.state.db().clone();
    let consumer = Arc::new(build_consumer(ctx).await?);
    // AI write paths (workflow action, automatic processing, review apply/undo) through pl-consumer
    // docops: django-auditlog entries like Document.save()/add_nested_tags/tags.remove.
    #[cfg(feature = "ai")]
    ctx.state.set_service(pl_ai::server::workflow::AiDocWriter(Arc::new(ConsumerDocWriter {
        audit_enabled: consumer.settings.audit_log_enabled,
        settings: consumer.settings.clone(),
    })));
    let indexer = Arc::new(pl_indexer::Indexer::new(pl_indexer::IndexerConfig::from_env()));
    // A2: upstream init-search-index `document_index reindex --if-needed` at every start:
    // rebuild if schema/language (SEARCH_LANGUAGE) do not match, the index is missing or an
    // earlier rebuild was aborted. In the background, so that the worker accepts tasks immediately.
    {
        let (ix, p) = (indexer.clone(), pool.clone());
        tokio::spawn(async move { reindex_if_needed(&ix, &p).await });
    }
    let classifier_cfg = Arc::new(ClassifierTaskConfig::from_consumer(&consumer.settings, ctx.state.config()));

    // Settings from the next document on (docs/SETTINGS.md WP4a).
    let build_ctx = ctx.clone();
    let build: crate::settings_live::BuildConsumer = Arc::new(move || {
        let c = build_ctx.clone();
        Box::pin(async move { build_consumer(&c).await })
    });
    let consume = Arc::new(crate::settings_live::LiveConsume::new(ctx.state.clone(), consumer.clone(), build));
    let (st, q) = (ctx.state.clone(), ctx.queue.clone());
    let reg = reg.register(names::CONSUME_FILE, move |c: TaskContext| {
        let (consume, st, q) = (consume.clone(), st.clone(), q.clone());
        async move {
            let result = consume.call(c).await?;
            // Receivers of document_consumption_finished that live outside pl-consumer (AI).
            if let Some(id) = result.get("document_id").and_then(Value::as_i64) {
                ai_after_consumption(&st, q.as_ref(), id).await;
                // PaperRust UI: detect deadlines in the content (PAPERRUST_TIMELINE_EXTRACT_DATES)
                pl_timeline::refresh::after_consumption(st.db(), id, pl_core::config::var).await;
            }
            Ok(result)
        }
    });

    let (ix, p) = (indexer.clone(), pool.clone());
    let reg = reg.register(names::INDEX_DOCUMENT, move |ctx: TaskContext| {
        let (ix, p) = (ix.clone(), p.clone());
        async move {
            let id = int_arg(&ctx, 0, "document_id")?;
            ix.index_document_task(&p, id).await.map_err(indexer_error)?;
            Ok(Value::Null)
        }
    });
    let ix = indexer.clone();
    let reg = reg.register(names::REMOVE_DOCUMENT_FROM_INDEX, move |ctx: TaskContext| {
        let ix = ix.clone();
        async move {
            let id = int_arg(&ctx, 0, "doc_id")?;
            ix.remove_document_task(id).await.map_err(indexer_error)?;
            Ok(Value::Null)
        }
    });
    let ix = indexer.clone();
    let reg = reg.register(names::INDEX_OPTIMIZE, move |_ctx: TaskContext| {
        let ix = ix.clone();
        async move {
            ix.optimize();
            Ok(Value::Null)
        }
    });
    let (cc, p) = (classifier_cfg.clone(), pool.clone());
    let reg = reg.register(names::TRAIN_CLASSIFIER, move |_ctx: TaskContext| {
        let (cc, p) = (cc.clone(), p.clone());
        async move { train_classifier(&p, &cc).await.map(Value::String) }
    });
    let (ix, st, cons) = (indexer.clone(), ctx.state.clone(), consumer.clone());
    let reg = reg.register(names::BULK_UPDATE_DOCUMENTS, move |ctx: TaskContext| {
        let (ix, st, cons) = (ix.clone(), st.clone(), cons.clone());
        async move { bulk_update_documents(&st, &cons, &ix, &ctx).await }
    });
    // update_document_content_maybe_archive_file(document_id, *, remote_ocr=False) (W15)
    let (st, q, cons) = (ctx.state.clone(), ctx.queue.clone(), consumer.clone());
    let reg = reg.register(names::UPDATE_CONTENT_MAYBE_ARCHIVE, move |c: TaskContext| {
        let (st, q, cons) = (st.clone(), q.clone(), cons.clone());
        async move {
            let id = int_arg(&c, 0, "document_id")?;
            let remote_ocr = c.kwargs.get("remote_ocr").is_some_and(pl_documents_write::drf::py_truthy);
            // PaperRust: "AI text recognition" when reprocessing (only set if chosen)
            let ai_ocr = c.kwargs.get("ai_ocr").is_some_and(pl_documents_write::drf::py_truthy);
            let outcome = cons
                .update_document_content_maybe_archive_file_with(id, remote_ocr, ai_ocr)
                .await
                .map_err(|e| TaskError::fail(e.error_type(), e.to_string()))?;
            // upstream: llm_index_add_or_update_document(document) after successful saving
            if outcome == pl_consumer::reprocess::ReprocessOutcome::Updated {
                ai_after_reprocess(&st, q.as_ref(), id).await;
                pl_timeline::refresh::after_consumption(st.db(), id, pl_core::config::var).await;
            }
            Ok(Value::Null)
        }
    });
    // PaperRust: run_workflows_for_documents(document_ids, workflow_id, *, filter_documents=True)
    // (manual workflow run, bulk_edit `run_workflow`; idea from paperless-ngx dev #14704)
    let cons = consumer.clone();
    let reg = reg.register(pl_consumer::manual::RUN_WORKFLOWS_FOR_DOCUMENTS, move |c: TaskContext| {
        let cons = cons.clone();
        async move {
            // PaperRust: `match_trigger` (catch-up runs with AI condition, "apply retroactively")
            let k = pl_consumer::manual::parse_kwargs_ext(&c.kwargs).map_err(|e| TaskError::fail("TypeError", e))?;
            cons.run_workflows_for_documents_as(&k.document_ids, k.workflow_id, k.filter_documents, k.match_trigger)
                .await
                .map_err(|e| TaskError::fail("OperationalError", e.to_string()))?;
            Ok(Value::Null)
        }
    });
    let cons = consumer.clone();
    let reg = reg.register(names::CHECK_SCHEDULED_WORKFLOWS, move |_c: TaskContext| {
        let cons = cons.clone();
        async move {
            cons.check_scheduled_workflows().await.map_err(|e| TaskError::fail("OperationalError", e.to_string()))?;
            Ok(Value::Null)
        }
    });
    let st = ctx.state.clone();
    let reg = reg.register(names::SANITY_CHECK, move |c: TaskContext| {
        let st = st.clone();
        async move { sanity_check(&st, raise_on_error_arg(&c)).await.map(Value::String) }
    });
    let st = ctx.state.clone();
    let reg = reg.register(names::TIMELINE_BACKFILL, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let force = c.kwargs.get("force").is_some_and(pl_documents_write::drf::py_truthy);
            if !pl_timeline::refresh::enabled(&pl_core::config::var) {
                return Ok(Value::String("timeline date extraction is disabled".into()));
            }
            let dp = pl_timeline::extract::DateParse::from_lookup(pl_core::config::var);
            match pl_timeline::refresh::backfill(st.db(), &dp, force).await {
                Ok(Some(r)) => Ok(serde_json::to_value(r).unwrap_or(Value::Null)),
                Ok(None) => Ok(Value::String("a backfill is already running".into())),
                Err(e) => Err(TaskError::fail("OperationalError", e.to_string())),
            }
        }
    });
    let reg = register_documents_write_handlers(reg, &ctx.state);
    // upstream fix a139bdff4 (#14722): documents.tasks.clear_expired_sessions() (pl-accounts)
    let st = ctx.state.clone();
    let reg = reg.register(names::CLEAR_EXPIRED_SESSIONS, move |_c: TaskContext| {
        let st = st.clone();
        async move {
            pl_accounts::session::clear_expired_sessions(st.db(), chrono::Utc::now())
                .await
                .map_err(|e| TaskError::fail("OperationalError", e.to_string()))?;
            Ok(Value::Null)
        }
    });
    // W14: documents.tasks.build_share_link_bundle(bundle_id) (pl-fields-views)
    let st = ctx.state.clone();
    let reg = reg.register(pl_fields_views::bundle_task::TASK_NAME, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let fail = |f: pl_fields_views::bundle_task::TaskFailure| TaskError::fail(&f.exception_type, f.message);
            let id = pl_fields_views::bundle_task::bundle_id_arg(&c.args, &c.kwargs).map_err(fail)?;
            pl_fields_views::bundle_task::build_share_link_bundle(&st, id).await.map_err(fail)?;
            Ok(Value::Null)
        }
    });
    // A1: documents.tasks.cleanup_expired_share_link_bundles() (Beat SHARE_LINK_BUNDLE_CLEANUP_CRON)
    let st = ctx.state.clone();
    let reg = reg.register(pl_queue::maintenance::CLEANUP_EXPIRED_SHARE_LINK_BUNDLES, move |_c: TaskContext| {
        let st = st.clone();
        async move {
            let dir = st.config().share_link_bundle_dir.clone();
            pl_queue::maintenance::cleanup_expired_share_link_bundles(st.db(), &dir)
                .await
                .map_err(|e| TaskError::fail("OperationalError", e.to_string()))?;
            Ok(Value::Null)
        }
    });
    Ok(reg)
}

/// After `update_document_content_maybe_archive_file`: with `llm_index_enabled`
/// enqueue `update_document_in_llm_index` (upstream synchronously in the task).
#[cfg(feature = "ai")]
async fn ai_after_reprocess(state: &AppState, queue: &dyn TaskQueue, document_id: i64) {
    let Some(cfg) = ai_config(state).await else { return };
    if cfg.llm_index_enabled()
        && let Err(e) =
            queue.enqueue(names::UPDATE_DOCUMENT_IN_LLM_INDEX, document_kwargs(document_id), pl_queue::EnqueueOptions::default()).await
    {
        tracing::error!(target: "paperless.tasks", document_id, error = %e, "could not queue update_document_in_llm_index");
    }
}

#[cfg(not(feature = "ai"))]
async fn ai_after_reprocess(_state: &AppState, _queue: &dyn TaskQueue, _document_id: i64) {}

/// Tasks of pl-documents-write: `documents.tasks.empty_trash(doc_ids=None)` (beat "Empty trash"),
/// `documents.bulk_edit.delete(doc_ids)` (chord body / `link` of the PDF methods) and
/// `documents.bulk_edit.restore_archive_serial_numbers_task(backup, *args)` (their errback).
pub fn register_documents_write_handlers(reg: TaskRegistry, state: &AppState) -> TaskRegistry {
    let st = state.clone();
    let reg = reg.register(names::EMPTY_TRASH, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let ids = pl_documents_write::tasks::doc_ids_arg(&c.args, &c.kwargs, "doc_ids").map_err(|e| TaskError::fail("TypeError", e))?;
            pl_documents_write::tasks::empty_trash(&st, ids.as_deref()).await;
            Ok(Value::Null)
        }
    });
    let st = state.clone();
    let reg = reg.register(names::BULK_EDIT_DELETE, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let ids = pl_documents_write::tasks::doc_ids_arg(&c.args, &c.kwargs, "doc_ids")
                .map_err(|e| TaskError::fail("TypeError", e))?
                .ok_or_else(|| TaskError::fail("TypeError", "delete() missing 1 required positional argument: 'doc_ids'"))?;
            pl_documents_write::tasks::bulk_delete(&st, &ids).await;
            Ok(Value::String("OK".into()))
        }
    });
    // upstream dev 60a35edee: large bulk metadata changes in the background (bulk_task)
    let st = state.clone();
    let reg = reg.register(pl_documents_write::bulk_task::TASK_NAME, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let started = std::time::Instant::now();
            let r = pl_documents_write::bulk_task::execute(&st, &c.kwargs).await.map_err(|(t, m)| TaskError::fail(&t, m))?;
            tracing::info!(target: "paperless.tasks", "execute_bulk_operation: {} in {} ms", r, started.elapsed().as_millis());
            Ok(r)
        }
    });
    // `process_cf_select_update` (docs/STANDALONE-GAPS.md §4.3): PaperRust sends `custom_field_id`.
    let st = state.clone();
    let reg = reg.register(pl_fields_views::celery::PROCESS_CF_SELECT_UPDATE, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let id = c
                .kwargs
                .get("custom_field_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| TaskError::fail("TypeError", "process_cf_select_update() missing 'custom_field_id'"))?;
            pl_documents_write::tasks::process_cf_select_update(&st, id).await.map_err(|e| TaskError::fail("Exception", e))?;
            Ok(Value::Null)
        }
    });
    let st = state.clone();
    reg.register(names::RESTORE_ARCHIVE_SERIAL_NUMBERS, move |c: TaskContext| {
        let st = st.clone();
        async move {
            pl_documents_write::tasks::restore_asn_task(st.db(), &c.args, &c.kwargs).await.map_err(|(t, m)| TaskError::fail(&t, m))?;
            Ok(Value::Null)
        }
    })
}

/// Mail tasks of pl-mail (W6): `paperless_mail.tasks.process_mail_accounts`,
/// `paperless_mail.mail.apply_mail_action`, `paperless_mail.mail.error_callback` and the workflow
/// webhook task `documents.workflows.webhooks.send_webhook` (docs/contract/pl-mail-worker.md).
pub fn register_mail_handlers(reg: TaskRegistry, ctx: &WorkerContext) -> Result<TaskRegistry, String> {
    let core = ctx.state.config();
    let worker = Arc::new(pl_mail::fetch::MailWorker::new(
        ctx.state.db().clone(),
        ctx.queue.clone(),
        pl_mail::fetch::MailWorkerConfig::from_core(core),
    ));
    let webhooks = Arc::new(pl_mail::effects::webhook::WebhookSettings::from_env(&core.env)?);
    Ok(reg.with_handlers(&[names::PROCESS_MAIL_ACCOUNTS, names::APPLY_MAIL_ACTION, names::MAIL_ERROR_CALLBACK, names::SEND_WEBHOOK], |h| {
        pl_mail::fetch::register_mail_handlers(h, worker, webhooks)
    }))
}

// ------------------------------------------------------------------ train_classifier

/// Settings of `train_classifier` (model file, key, language).
#[derive(Debug, Clone)]
pub struct ClassifierTaskConfig {
    pub model_file: std::path::PathBuf,
    pub key: Vec<u8>,
    pub language: Option<pl_classifier::Language>,
}

impl ClassifierTaskConfig {
    /// Same model file and key as the consumer's `FileClassifier` (pl-consumer native.rs);
    /// language like `settings.CLASSIFIER_LANGUAGE` (`OCR_LANGUAGE.split("+")[0]`).
    pub fn from_consumer(s: &pl_consumer::ConsumerSettings, core: &pl_core::Config) -> Self {
        let ocr = core.env.get("PAPERLESS_OCR_LANGUAGE").unwrap_or("eng");
        Self {
            model_file: s.classifier_model_file.clone(),
            key: s.secret_key.clone().into_bytes(),
            language: pl_classifier::Language::from_ocr_language(ocr),
        }
    }
}

/// `documents.tasks.train_classifier` (upstream: tasks.py::train_classifier,
/// classifier.py::DocumentClassifier.train – data gathering is the caller's part, see
/// docs/contract/pl-classifier.md §3).
pub async fn train_classifier(pool: &sqlx::PgPool, cfg: &ClassifierTaskConfig) -> Result<String, TaskError> {
    let db = |e: sqlx::Error| TaskError::fail("OperationalError", e.to_string());
    // Tag/DocumentType/Correspondent/StoragePath .filter(matching_algorithm=MATCH_AUTO).exists()
    let (any_auto,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM documents_tag WHERE matching_algorithm = 6) \
         OR EXISTS (SELECT 1 FROM documents_documenttype WHERE matching_algorithm = 6) \
         OR EXISTS (SELECT 1 FROM documents_correspondent WHERE matching_algorithm = 6) \
         OR EXISTS (SELECT 1 FROM documents_storagepath WHERE matching_algorithm = 6)",
    )
    .fetch_one(pool)
    .await
    .map_err(db)?;
    if !any_auto {
        let result = "No automatic matching items, not training".to_string();
        tracing::info!(target: "paperless.tasks", "{result}");
        if cfg.model_file.exists() {
            tracing::info!(target: "paperless.tasks", "Removing {} so it won't be used", cfg.model_file.display());
            let _ = std::fs::remove_file(&cfg.model_file);
        }
        return Ok(result);
    }
    // Document.objects.exclude(tags__is_inbox_tag=True).order_by("pk") with the AUTO labels
    // (raw SQL: joins with CASE, no sea-query form needed for the read-only worker query).
    type LabelRow = (i32, chrono::DateTime<chrono::Utc>, Option<i32>, Option<i32>, Option<i32>);
    let rows: Vec<LabelRow> = sqlx::query_as(
        "SELECT d.id, d.modified, \
                CASE WHEN c.matching_algorithm = 6 THEN c.id END, \
                CASE WHEN t.matching_algorithm = 6 THEN t.id END, \
                CASE WHEN s.matching_algorithm = 6 THEN s.id END \
         FROM documents_document d \
         LEFT JOIN documents_correspondent c ON c.id = d.correspondent_id \
         LEFT JOIN documents_documenttype t ON t.id = d.document_type_id \
         LEFT JOIN documents_storagepath s ON s.id = d.storage_path_id \
         WHERE d.deleted_at IS NULL AND NOT EXISTS ( \
             SELECT 1 FROM documents_document_tags dt JOIN documents_tag it ON it.id = dt.tag_id \
             WHERE dt.document_id = d.id AND it.is_inbox_tag) \
         ORDER BY d.id",
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let tag_rows: Vec<(i32, i32)> = sqlx::query_as(
        "SELECT dt.document_id, dt.tag_id FROM documents_document_tags dt \
         JOIN documents_tag t ON t.id = dt.tag_id WHERE t.matching_algorithm = 6 \
         ORDER BY dt.document_id, dt.tag_id",
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let mut tags: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    for (d, t) in tag_rows {
        tags.entry(i64::from(d)).or_default().push(i64::from(t));
    }
    let docs: Vec<pl_classifier::model::TrainingDoc> = rows
        .into_iter()
        .map(|(id, modified, c, t, s)| pl_classifier::model::TrainingDoc {
            pk: i64::from(id),
            modified_us: modified.timestamp_micros(),
            correspondent: c.map(i64::from),
            document_type: t.map(i64::from),
            storage_path: s.map(i64::from),
            tags: tags.remove(&i64::from(id)).unwrap_or_default(),
        })
        .collect();
    let cfg = cfg.clone();
    let pool = pool.clone();
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || -> Result<String, TaskError> {
        let mut classifier = pl_classifier::model::load_classifier(&cfg.model_file, &cfg.key, false).ok().flatten().unwrap_or_default();
        // Document.objects.only("content").in_bulk(chunk): deleted meanwhile -> "".
        let content = |pks: &[i64]| -> Vec<Option<String>> {
            let ids: Vec<i32> = pks.iter().filter_map(|p| i32::try_from(*p).ok()).collect();
            let fetched: Result<Vec<(i32, String)>, sqlx::Error> = handle.block_on(
                sqlx::query_as("SELECT id, content FROM documents_document WHERE deleted_at IS NULL AND id = ANY($1)")
                    .bind(ids)
                    .fetch_all(&pool),
            );
            let mut map: std::collections::HashMap<i64, String> =
                fetched.unwrap_or_default().into_iter().map(|(i, c)| (i64::from(i), c)).collect();
            pks.iter().map(|p| map.remove(p)).collect()
        };
        let trained = classifier
            .train(docs, cfg.language, content, &pl_classifier::model::TrainOptions::default(), None)
            .map_err(|e| TaskError::fail("ValueError", e.to_string()))?;
        if trained {
            tracing::info!(target: "paperless.tasks", "Saving updated classifier model to {}...", cfg.model_file.display());
            classifier.save(&cfg.model_file, &cfg.key).map_err(|e| TaskError::fail("OSError", e.to_string()))?;
            Ok("Training completed successfully".into())
        } else {
            tracing::debug!(target: "paperless.tasks", "Training data unchanged.");
            Ok("Training data unchanged".into())
        }
    })
    .await
    .map_err(|e| TaskError::fail("Panic", e.to_string()))?
}

// ------------------------------------------------------------------ bulk_update_documents

/// `documents.tasks.bulk_update_documents(document_ids)`: per document (default manager, `-created`)
/// `clear_document_caches` + `document_updated` (workflows "Document Updated", `skip_ai_index`) +
/// `post_save` (file name) via `Consumer::bulk_update_signals` (W15), afterwards search index
/// (`batch.add_or_update_ids`) and – with `llm_index_enabled` – `update_llm_index(document_ids)`.
async fn bulk_update_documents(
    state: &AppState,
    consumer: &pl_consumer::Consumer,
    ix: &pl_indexer::Indexer,
    ctx: &TaskContext,
) -> Result<Value, TaskError> {
    let ids: Vec<i64> = match task_arg(ctx, 0, "document_ids") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_i64).collect(),
        Some(_) => return Err(TaskError::fail("TypeError", "document_ids must be a list of integers")),
        None => {
            return Err(TaskError::fail("TypeError", "bulk_update_documents() missing 1 required positional argument: 'document_ids'"));
        }
    };
    // PaperRust (PERF-SCALE §10): bundled follow-up, same effect as the single path;
    // `PAPERRUST_BULK_UPDATE=single` switches back to the single path.
    let started = std::time::Instant::now();
    let single = pl_core::config::var("PAPERRUST_BULK_UPDATE").is_some_and(|v| v.trim().eq_ignore_ascii_case("single"));
    if single {
        consumer.bulk_update_signals_single(&ids).await
    } else {
        consumer.bulk_update_signals(&ids).await.map(|st| {
            tracing::info!(
                target: "paperless.tasks",
                documents = st.documents,
                workflow_matched = st.workflow_matched,
                workflow_deferred = st.workflow_deferred,
                moves_skipped = st.moves_skipped,
                filename_unchanged = st.filename_unchanged,
                filename_slow = st.filename_slow,
                "bulk_update_documents: signals done in {} ms",
                started.elapsed().as_millis()
            );
        })
    }
    .map_err(|e| TaskError::fail("OperationalError", e.to_string()))?;
    let pool = state.db();
    // FINDINGS B8: whole write operation with retry on transient file errors
    ix.add_or_update_ids_task(pool, &ids).await.map_err(indexer_error)?;
    // if ai_config.llm_index_enabled: update_llm_index(rebuild=False, document_ids=document_ids)
    #[cfg(feature = "ai")]
    llm_index_update_ids(state, &ids).await?;
    Ok(Value::Null)
}

// ------------------------------------------------------------------ caches / soft delete (W15)

/// `clear_document_caches(document_id)` (documents/caching.py): `DEL` of the Django cache keys
/// `suggest`, `metadata`, `thumbnail_modified` (`pl_consumer::signals::document_cache_keys`), with
/// feature `ai` additionally the LLM suggestions (`pl_ai::server::cache::invalidate`). Errors (no
/// Redis) are logged.
pub struct RedisDocumentCaches {
    pub state: AppState,
}

impl pl_consumer::signals::DocumentCaches for RedisDocumentCaches {
    fn clear_document_caches(&self, document_id: i64) -> futures::future::BoxFuture<'_, ()> {
        Box::pin(async move {
            // W23: Django's `default` cache via AppState::cache() – Redis `DEL` (shared with
            // Django) or the process-local cache (never a Redis connection attempt).
            let keys = pl_consumer::signals::document_cache_keys("", document_id);
            let names: Vec<&str> = keys.iter().map(|k| k.strip_prefix(":1:").unwrap_or(k)).collect();
            if let Err(e) = self.state.cache().delete_many(&names).await {
                tracing::warn!(target: "paperless.caching", document_id, error = %e.status(), "clearing document caches failed");
            }
            #[cfg(feature = "ai")]
            pl_ai::server::cache::invalidate(&self.state, document_id).await;
        })
    }

    /// Bundled: one `DEL` with all keys of the block.
    fn clear_many<'a>(&'a self, document_ids: &'a [i64]) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            let keys: Vec<String> = document_ids
                .iter()
                .flat_map(|id| pl_consumer::signals::document_cache_keys("", *id))
                .map(|k| k.strip_prefix(":1:").map(str::to_string).unwrap_or(k))
                .collect();
            let names: Vec<&str> = keys.iter().map(String::as_str).collect();
            if !names.is_empty()
                && let Err(e) = self.state.cache().delete_many(&names).await
            {
                tracing::warn!(target: "paperless.caching", documents = document_ids.len(), error = %e.status(), "clearing document caches failed");
            }
            #[cfg(feature = "ai")]
            for id in document_ids {
                pl_ai::server::cache::invalidate(&self.state, *id).await;
            }
        })
    }
}

/// `document.delete()` of the workflow action "move to trash" via the full
/// soft-delete cascade of pl-documents-write (with auditlog of the related objects).
pub struct DocumentsWriteSoftDelete {
    pub media: pl_documents_write::files::MediaCfg,
}

impl pl_consumer::workflows::DocumentDeleter for DocumentsWriteSoftDelete {
    fn soft_delete<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        audit: &'a pl_consumer::audit::AuditLog,
        doc_id: i64,
    ) -> futures::future::BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            let log = pl_documents_write::auditlog::LogCtx { enabled: audit.enabled, actor_id: audit.actor_id, ..Default::default() };
            pl_documents_write::softdelete::soft_delete_document(conn, &log, &self.media, doc_id, Some(uuid::Uuid::new_v4()))
                .await
                .map_err(|e| sqlx::Error::Protocol(format!("{e:?}")))
        })
    }
}

// ------------------------------------------------------------------ sanity_check

/// `documents.tasks.sanity_check(*, raise_on_error=True)`: `check_sanity()` (pl-cli),
/// `SanityCheckMessages.log_messages()` (logger `paperless.sanity_checker`), summary.
/// PaperRust Z2: `PAPERRUST_INTEGRITY_*` (incremental, JSON report, log line `paperless.integrity`).
pub async fn sanity_check(state: &AppState, raise_on_error: bool) -> Result<String, TaskError> {
    let c = pl_cli::ctx::Ctx { cfg: state.config().clone(), pool: state.db().clone() };
    let opts = pl_cli::sanity::Options::for_task(&state.config().env);
    let (m, _, _) = pl_cli::sanity::run(&c, &opts, "task").await.map_err(|e| TaskError::fail("RuntimeError", e.to_string()))?;
    log_sanity_messages(state, &m).await;
    let summary = sanity_summary(&m);
    match summary {
        SanitySummary::Clean(s) | SanitySummary::Issues(s) => Ok(s),
        SanitySummary::Errors(s) if raise_on_error => Err(TaskError::fail("SanityCheckFailedException", s)),
        SanitySummary::Errors(s) => Ok(s),
    }
}

/// Result text of `sanity_check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SanitySummary {
    Clean(String),
    Issues(String),
    Errors(String),
}

/// Summary like upstream `documents.tasks.sanity_check`.
pub fn sanity_summary(m: &pl_cli::sanity::Messages) -> SanitySummary {
    if !m.has_error && !m.has_warning && !m.has_info {
        return SanitySummary::Clean("No issues detected.".into());
    }
    let mut parts = Vec::new();
    if m.document_error_count > 0 {
        parts.push(format!("{} document(s) with errors", m.document_error_count));
    }
    if m.document_warning_count > 0 {
        parts.push(format!("{} document(s) with warnings", m.document_warning_count));
    }
    if m.document_info_count > 0 {
        parts.push(format!("{} document(s) with infos", m.document_info_count));
    }
    if m.global_warning_count > 0 {
        parts.push(format!("{} global warning(s)", m.global_warning_count));
    }
    let summary = format!("{} found.", parts.join(", "));
    if m.has_error { SanitySummary::Errors(format!("{summary} Check logs for details.")) } else { SanitySummary::Issues(summary) }
}

/// `SanityCheckMessages.log_messages()`.
async fn log_sanity_messages(state: &AppState, m: &pl_cli::sanity::Messages) {
    if m.order.is_empty() {
        tracing::info!(target: "paperless.sanity_checker", "Sanity checker detected no issues.");
        return;
    }
    let pks: Vec<i32> = m.order.iter().flatten().filter_map(|p| i32::try_from(*p).ok()).collect();
    let titles: std::collections::HashMap<i64, String> = if pks.is_empty() {
        Default::default()
    } else {
        // Document.global_objects (also trash)
        sqlx::query_as::<_, (i32, String)>("SELECT id, title FROM documents_document WHERE id = ANY($1)")
            .bind(&pks)
            .fetch_all(state.db())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(i, t)| (i64::from(i), t))
            .collect()
    };
    for pk in &m.order {
        if let Some(p) = pk {
            let title = titles.get(p).map(String::as_str).unwrap_or("Unknown");
            tracing::info!(target: "paperless.sanity_checker", "Detected following issue(s) with document #{p}, titled {title}");
        }
        for msg in m.messages.get(pk).map(Vec::as_slice).unwrap_or_default() {
            match msg.level {
                pl_cli::sanity::ERROR => tracing::error!(target: "paperless.sanity_checker", "{}", msg.message),
                pl_cli::sanity::WARNING => tracing::warn!(target: "paperless.sanity_checker", "{}", msg.message),
                _ => tracing::info!(target: "paperless.sanity_checker", "{}", msg.message),
            }
        }
    }
}

/// Keyword argument `raise_on_error` (default `True`, Python truth value).
pub fn raise_on_error_arg(ctx: &TaskContext) -> bool {
    ctx.kwargs.get("raise_on_error").is_none_or(pl_documents_write::drf::py_truthy)
}

// ------------------------------------------------------------------ AI (pl-ai, W10)

/// `AIConfig()` for worker hooks; an invalid configuration is logged and treated as "AI off"
/// (Django would refuse to start with it).
#[cfg(feature = "ai")]
async fn ai_config(state: &AppState) -> Option<pl_ai::config::AiConfig> {
    match pl_ai::server::effective_config(state).await {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(target: "paperless.tasks", error = %e, "AI configuration unavailable; AI hooks skipped");
            None
        }
    }
}

/// `AiTaskError` → `TaskError` (`autoretry` = Celery `autoretry_for`).
#[cfg(feature = "ai")]
pub fn ai_task_error(e: pl_ai::server::index::AiTaskError) -> TaskError {
    if e.autoretry { TaskError::autoretry(e.error_type, e.message) } else { TaskError::fail(e.error_type, e.message) }
}

#[cfg(feature = "ai")]
fn api_task_error(e: pl_core::ApiError) -> TaskError {
    TaskError::fail("RuntimeError", e.to_string())
}

/// Receivers of `document_consumption_finished` for AI (documents/apps.py order: after
/// `add_to_index`): `add_or_update_document_in_llm_index` →
/// `update_document_in_llm_index.apply_async(kwargs={"document": document})` (Rust: document id),
/// then the PaperRust automatic processing (`PAPERRUST_AI_AUTO_MODE` ≠ off, trigger `consumed`).
/// Enqueue errors are logged (the consumption itself succeeded).
#[cfg(feature = "ai")]
pub async fn ai_after_consumption(state: &AppState, queue: &dyn TaskQueue, document_id: i64) {
    let Some(cfg) = ai_config(state).await else { return };
    if cfg.llm_index_enabled()
        && let Err(e) =
            queue.enqueue(names::UPDATE_DOCUMENT_IN_LLM_INDEX, document_kwargs(document_id), pl_queue::EnqueueOptions::default()).await
    {
        tracing::error!(target: "paperless.handlers", document_id, error = %e, "could not queue update_document_in_llm_index");
    }
    if cfg.ai_enabled && cfg.rs.auto_mode != pl_ai::config::AutoMode::Off {
        let mut kw = document_kwargs(document_id);
        kw.insert("trigger".into(), Value::from("consumed"));
        if let Err(e) = queue.enqueue(names::AI_PROCESS_DOCUMENT, kw, pl_queue::EnqueueOptions::trigger("system")).await {
            tracing::error!(target: "paperless.handlers", document_id, error = %e, "could not queue AI processing");
        }
    }
}

/// Without feature `ai` there are no AI receivers.
#[cfg(not(feature = "ai"))]
pub async fn ai_after_consumption(_state: &AppState, _queue: &dyn TaskQueue, _document_id: i64) {}

/// [`pl_core::vision::VisionOcr`] on pl-ai (`server::vision::{plan, transcribe, record}`, KI-Abschrift):
/// mode ("Aus"/"Nur unsichere Stellen"/"Ganze Seite, wenn unsicher"/"Immer", legacy `vision_when`),
/// sensitivity, image model, image size, page limits and daily budget come from the AI options
/// (`paperrust_ai_option` / `PAPERRUST_AI_VISION_*`); `record` stores the transcript and the label note.
#[cfg(feature = "ai")]
pub struct AiVisionOcr {
    pub state: AppState,
}

#[cfg(feature = "ai")]
impl pl_core::vision::VisionOcr for AiVisionOcr {
    fn plan(&self, requested: bool) -> pl_core::vision::VisionFuture<'_, Option<pl_core::vision::VisionPlan>> {
        Box::pin(async move { pl_ai::server::vision::plan(&self.state, requested).await })
    }

    fn recognize(
        &self,
        request: pl_core::vision::VisionRequest,
    ) -> pl_core::vision::VisionFuture<'_, Result<Option<pl_core::vision::VisionResponse>, String>> {
        Box::pin(async move {
            let r = pl_ai::server::vision::transcribe(&self.state, request).await?;
            if let Some(r) = &r {
                let skipped = r.items.iter().filter(|i| i.status == pl_core::vision::ItemStatus::Skipped).count();
                if skipped > 0 {
                    tracing::info!(target: "paperless.consumer", "AI transcription: {skipped} page(s)/crop(s) not sent (page limit or daily budget)");
                }
            }
            Ok(r)
        })
    }

    fn record(&self, document_id: i64, transcript: pl_core::vision::Transcript) -> pl_core::vision::VisionFuture<'_, ()> {
        Box::pin(async move { pl_ai::server::vision::record(&self.state, document_id, transcript).await })
    }
}

/// [`pl_ai::server::workflow::DocWriter`] on pl-consumer `docops` (one transaction per call):
/// `document.save(update_fields=[…, "modified"])` with the django-auditlog `LogEntry`,
/// `add_nested_tags` / `tags.remove` with the m2m entries. `get_or_create` stays with
/// `SqlDocWriter` (tree update via pl-objects; no auditlog entry for the new object). Custom fields go
/// through pl-documents-write `cfi` (auditlog, file name). The audit actor
/// is always `None` (the trait carries no user). W5: after `save`/`add_nested_tags`/`remove_tags`
/// the receivers `post_save`/`m2m_changed` → `update_filename_and_move_files` run in the same
/// transaction (pl-consumer `files`).
///
/// Transactional path (`supports_transaction() == true`, used by the AI review/automation/undo):
/// the `*_tx` methods write on the caller's connection with the same auditlog entries and never
/// begin/commit themselves; the file move runs only in [`DocWriter::after_commit`] (own
/// transaction after the caller's commit), so a rollback never leaves files moved. Custom fields
/// on that path use the deferred pl-documents-write variants (`cfi::update_or_create_deferred`,
/// `cfi::soft_delete_deferred`), which skip the `post_save` file move.
///
/// [`DocWriter::after_commit`]: pl_ai::server::workflow::DocWriter::after_commit
#[cfg(feature = "ai")]
pub struct ConsumerDocWriter {
    pub audit_enabled: bool,
    pub settings: Arc<pl_consumer::settings::ConsumerSettings>,
}

/// Error text of a pl-documents-write `cfi` call for the `DocWriter` trait (no internals beyond
/// what the previous local mirror returned: the message or the database error).
#[cfg(feature = "ai")]
fn cfi_err(e: pl_documents_write::error::ApiError) -> String {
    use pl_documents_write::error::ApiError;
    match e {
        ApiError::Uncaught(m) | ApiError::BadRequestText(m) => m,
        ApiError::Database(e) => e.to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(feature = "ai")]
impl ConsumerDocWriter {
    fn audit(&self) -> pl_consumer::audit::AuditLog {
        pl_consumer::audit::AuditLog { enabled: self.audit_enabled, actor_id: None }
    }
}

#[cfg(feature = "ai")]
async fn load_instance(conn: &mut sqlx::PgConnection, doc_id: i64) -> Result<pl_consumer::docops::DocInstance, String> {
    match pl_consumer::db::load_document(conn, doc_id).await.map_err(|e| e.to_string())? {
        Some(row) => Ok(pl_consumer::docops::DocInstance::from_db(row)),
        None => Err(format!("document {doc_id} does not exist")),
    }
}

/// `update_fields` for [`pl_ai::server::workflow::DocChanges`] (always with `modified`).
#[cfg(feature = "ai")]
pub fn apply_doc_changes(doc: &mut pl_consumer::docops::DocInstance, c: &pl_ai::server::workflow::DocChanges) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if let Some(t) = &c.title {
        doc.row.title = t.clone();
        fields.push("title");
    }
    if let Some(v) = c.correspondent_id {
        doc.row.correspondent_id = v;
        fields.push("correspondent");
    }
    if let Some(v) = c.document_type_id {
        doc.row.document_type_id = v;
        fields.push("document_type");
    }
    if let Some(v) = c.storage_path_id {
        doc.row.storage_path_id = v;
        fields.push("storage_path");
    }
    if let Some(d) = c.created {
        doc.row.created = d;
        doc.created_attr = pl_consumer::audit::CreatedAttr::Date(d);
        fields.push("created");
    }
    fields.push("modified");
    fields
}

#[cfg(feature = "ai")]
impl pl_ai::server::workflow::DocWriter for ConsumerDocWriter {
    fn save<'a>(
        &'a self,
        state: &'a AppState,
        doc_id: i64,
        c: &'a pl_ai::server::workflow::DocChanges,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut tx = state.db().begin().await.map_err(|e| e.to_string())?;
            let mut doc = load_instance(&mut tx, doc_id).await?;
            let fields = apply_doc_changes(&mut doc, c);
            pl_consumer::docops::save(&mut tx, &self.audit(), &mut doc, Some(&fields)).await.map_err(|e| e.to_string())?;
            pl_consumer::files::update_filename_and_move_files(&mut tx, &self.settings, doc_id).await.map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())
        })
    }

    fn add_nested_tags<'a>(
        &'a self,
        state: &'a AppState,
        doc_id: i64,
        tag_ids: &'a [i64],
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut tx = state.db().begin().await.map_err(|e| e.to_string())?;
            let doc = load_instance(&mut tx, doc_id).await?;
            pl_consumer::docops::add_nested_tags(&mut tx, &self.audit(), &doc, tag_ids).await.map_err(|e| e.to_string())?;
            pl_consumer::files::update_filename_and_move_files(&mut tx, &self.settings, doc_id).await.map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())
        })
    }

    fn remove_tags<'a>(
        &'a self,
        state: &'a AppState,
        doc_id: i64,
        tag_ids: &'a [i64],
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut tx = state.db().begin().await.map_err(|e| e.to_string())?;
            let doc = load_instance(&mut tx, doc_id).await?;
            pl_consumer::docops::remove_tags(&mut tx, &self.audit(), &doc, tag_ids).await.map_err(|e| e.to_string())?;
            pl_consumer::files::update_filename_and_move_files(&mut tx, &self.settings, doc_id).await.map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())
        })
    }

    fn get_or_create<'a>(
        &'a self,
        state: &'a AppState,
        cat: pl_ai::server::data::Cat,
        name: &'a str,
        owner: Option<i64>,
    ) -> futures::future::BoxFuture<'a, Result<(i64, bool), String>> {
        Box::pin(async move { pl_ai::server::workflow::SqlDocWriter.get_or_create(state, cat, name, owner).await })
    }

    /// `CustomFieldInstance.objects.update_or_create(...)` over pl-documents-write `cfi` (auditlog
    /// entry, `update_filename_and_move_files` via `after_custom_field_saved`). The field type comes
    /// from the database; a mismatch with the caller's `data_type` is rejected.
    fn set_custom_field<'a>(
        &'a self,
        state: &'a AppState,
        doc_id: i64,
        field_id: i64,
        data_type: &'a str,
        value: &'a Value,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let media = pl_documents_write::files::MediaCfg::from_config(&state.live_config());
            let log = pl_documents_write::auditlog::LogCtx { enabled: self.audit_enabled, ..Default::default() };
            let mut tx = state.db().begin().await.map_err(|e| e.to_string())?;
            let field = pl_documents_write::cfi::field_def(&mut tx, field_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("custom field {field_id} does not exist"))?;
            if field.data_type != data_type {
                return Err(format!("custom field {field_id} has type {}, not {data_type}", field.data_type));
            }
            pl_documents_write::cfi::update_or_create(&mut tx, &log, &media, doc_id, &field, value).await.map_err(|e| format!("{e:?}"))?;
            tx.commit().await.map_err(|e| e.to_string())
        })
    }

    /// Soft delete of the live instance (`SoftDeleteModel.delete`, auditlog, file name); no instance → no-op.
    fn remove_custom_field<'a>(
        &'a self,
        state: &'a AppState,
        doc_id: i64,
        field_id: i64,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let media = pl_documents_write::files::MediaCfg::from_config(&state.live_config());
            let log = pl_documents_write::auditlog::LogCtx { enabled: self.audit_enabled, ..Default::default() };
            let mut tx = state.db().begin().await.map_err(|e| e.to_string())?;
            if let Some(c) = pl_documents_write::cfi::live_for(&mut tx, doc_id, field_id).await.map_err(|e| e.to_string())? {
                pl_documents_write::cfi::soft_delete(&mut tx, &log, &media, &c, uuid::Uuid::new_v4(), false)
                    .await
                    .map_err(|e| format!("{e:?}"))?;
            }
            tx.commit().await.map_err(|e| e.to_string())
        })
    }

    // ---- transactional path: caller's connection, no begin/commit, files only after the commit ----

    fn supports_transaction(&self) -> bool {
        true
    }

    /// `save` on the caller's connection: `Document.save(update_fields)` with auditlog, no file move.
    fn save_tx<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        doc_id: i64,
        c: &'a pl_ai::server::workflow::DocChanges,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut doc = load_instance(conn, doc_id).await?;
            let fields = apply_doc_changes(&mut doc, c);
            pl_consumer::docops::save(conn, &self.audit(), &mut doc, Some(&fields)).await.map_err(|e| e.to_string())
        })
    }

    fn add_nested_tags_tx<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        doc_id: i64,
        tag_ids: &'a [i64],
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let doc = load_instance(conn, doc_id).await?;
            pl_consumer::docops::add_nested_tags(conn, &self.audit(), &doc, tag_ids).await.map(|_| ()).map_err(|e| e.to_string())
        })
    }

    fn remove_tags_tx<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        doc_id: i64,
        tag_ids: &'a [i64],
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let doc = load_instance(conn, doc_id).await?;
            pl_consumer::docops::remove_tags(conn, &self.audit(), &doc, tag_ids).await.map_err(|e| e.to_string())
        })
    }

    /// Same SQL as the pool variant (`SqlDocWriter`), on the caller's connection.
    fn get_or_create_tx<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        cat: pl_ai::server::data::Cat,
        name: &'a str,
        owner: Option<i64>,
    ) -> futures::future::BoxFuture<'a, Result<(i64, bool), String>> {
        Box::pin(pl_ai::server::workflow::sql_get_or_create(conn, cat, name, owner))
    }

    /// `update_or_create` with auditlog on the caller's connection; the field type comes from the
    /// database (mismatch rejected). File name: [`Self::after_commit`].
    fn set_custom_field_tx<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        doc_id: i64,
        field_id: i64,
        data_type: &'a str,
        value: &'a Value,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let log = pl_documents_write::auditlog::LogCtx { enabled: self.audit_enabled, ..Default::default() };
            let field = pl_documents_write::cfi::field_def(conn, field_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("custom field {field_id} does not exist"))?;
            if field.data_type != data_type {
                return Err(format!("custom field {field_id} has type {}, not {data_type}", field.data_type));
            }
            pl_documents_write::cfi::update_or_create_deferred(conn, &log, doc_id, &field, value).await.map(|_| ()).map_err(cfi_err)
        })
    }

    /// Soft delete of the live instance with auditlog on the caller's connection; none → no-op.
    fn remove_custom_field_tx<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        doc_id: i64,
        field_id: i64,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let log = pl_documents_write::auditlog::LogCtx { enabled: self.audit_enabled, ..Default::default() };
            let Some(c) = pl_documents_write::cfi::live_for(conn, doc_id, field_id).await.map_err(|e| e.to_string())? else {
                return Ok(());
            };
            pl_documents_write::cfi::soft_delete_deferred(conn, &log, &c, uuid::Uuid::new_v4()).await.map_err(cfi_err)
        })
    }

    /// After the caller's commit: `post_save`/`m2m_changed` → `update_filename_and_move_files` in
    /// a fresh transaction (move errors are logged and rolled back inside, as upstream).
    fn after_commit<'a>(&'a self, state: &'a AppState, doc_id: i64) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut tx = state.db().begin().await.map_err(|e| e.to_string())?;
            pl_consumer::files::update_filename_and_move_files(&mut tx, &self.settings, doc_id).await.map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())
        })
    }
}

/// LLM index part of `bulk_update_documents`.
#[cfg(feature = "ai")]
async fn llm_index_update_ids(state: &AppState, ids: &[i64]) -> Result<(), TaskError> {
    let Some(cfg) = ai_config(state).await else { return Ok(()) };
    if cfg.llm_index_enabled() {
        pl_ai::server::index::update_llm_index(state, &cfg, false, Some(ids.to_vec())).await.map_err(ai_task_error)?;
    }
    Ok(())
}

/// `llmindex_index(rebuild=False)` keyword argument.
pub fn rebuild_arg(ctx: &TaskContext) -> Result<bool, TaskError> {
    match ctx.kwargs.get("rebuild") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(TaskError::fail("TypeError", "rebuild must be a boolean")),
    }
}

/// `trigger` of `paperrust.ai.process_document` (default `consumed`).
pub fn trigger_arg(ctx: &TaskContext) -> Result<String, TaskError> {
    match ctx.kwargs.get("trigger").and_then(Value::as_str) {
        None => Ok("consumed".into()),
        Some(t @ ("consumed" | "tag" | "workflow" | "manual")) => Ok(t.to_string()),
        Some(other) => Err(TaskError::fail("ValueError", format!("unknown trigger '{other}'"))),
    }
}

/// AI tasks (pl-ai). Without feature `ai` the upstream names are registered as "not ported".
#[cfg(feature = "ai")]
pub fn register_ai_handlers(reg: TaskRegistry, ctx: &WorkerContext) -> TaskRegistry {
    use pl_ai::server::{index, review, workflow};
    let st = ctx.state.clone();
    let reg = reg.register(names::LLMINDEX_INDEX, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let rebuild = rebuild_arg(&c)?;
            index::llmindex_index(&st, rebuild).await.map_err(ai_task_error)
        }
    });
    let st = ctx.state.clone();
    let reg = reg.register(names::APPLY_AI_SUGGESTIONS, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let action_id = int_arg(&c, 0, "action_id")?;
            let document_id = int_arg(&c, 1, "document_id")?;
            workflow::apply_ai_suggestions(&st, action_id, document_id).await.map_err(ai_task_error)
        }
    });
    let st = ctx.state.clone();
    let reg = reg.register(names::UPDATE_DOCUMENT_IN_LLM_INDEX, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let id = int_arg(&c, 0, "document_id")?;
            index::update_document_in_llm_index(&st, id).await.map_err(ai_task_error)?;
            Ok(Value::Null)
        }
    });
    let st = ctx.state.clone();
    let reg = reg.register(names::REMOVE_DOCUMENT_FROM_LLM_INDEX, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let id = int_arg(&c, 0, "document_id")?;
            index::remove_document_from_llm_index(&st, id).await.map_err(ai_task_error)?;
            Ok(Value::Null)
        }
    });
    let st = ctx.state.clone();
    let reg = reg.register(names::AI_LLM_INDEX_COMPACT, move |_c: TaskContext| {
        let st = st.clone();
        async move {
            index::llm_index_compact(&st).await.map_err(ai_task_error)?;
            Ok(Value::Null)
        }
    });
    let st = ctx.state.clone();
    let reg = reg.register(names::AI_PROCESS_TRIGGER_TAG, move |_c: TaskContext| {
        let st = st.clone();
        async move { review::process_trigger_tag(&st).await.map_err(api_task_error) }
    });
    let st = ctx.state.clone();
    let reg = reg.register(names::AI_PROCESS_DOCUMENT, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let id = int_arg(&c, 0, "document_id")?;
            let trigger = trigger_arg(&c)?;
            let cfg = pl_ai::server::effective_config(&st).await.map_err(api_task_error)?;
            if !cfg.ai_enabled || cfg.rs.auto_mode == pl_ai::config::AutoMode::Off {
                return Ok(Value::Null);
            }
            review::run(&st, &[id], cfg.rs.auto_mode, &trigger, None).await.map_err(api_task_error)
        }
    });
    let st = ctx.state.clone();
    reg.register(names::AI_BACKFILL, move |c: TaskContext| {
        let st = st.clone();
        async move {
            let id = int_arg(&c, 0, "job_id")?;
            pl_ai::server::jobs::run_job_reporting(&st, id).await.map_err(|e| TaskError::fail("AiJobError", e))?;
            Ok(Value::Null)
        }
    })
}

/// Without feature `ai` the upstream AI tasks fail clearly instead of waiting forever.
#[cfg(not(feature = "ai"))]
pub fn register_ai_handlers(reg: TaskRegistry, _ctx: &WorkerContext) -> TaskRegistry {
    const MISSING: &str = "built without feature `ai`";
    reg.not_ported(names::LLMINDEX_INDEX, MISSING)
        .not_ported(names::APPLY_AI_SUGGESTIONS, MISSING)
        .not_ported(names::UPDATE_DOCUMENT_IN_LLM_INDEX, MISSING)
        .not_ported(names::REMOVE_DOCUMENT_FROM_LLM_INDEX, MISSING)
}

/// PaperRust beat entry for the AI trigger tag (`PAPERRUST_AI_AUTO_TRIGGER_TAG`): only when a
/// trigger tag is set and `PAPERRUST_AI_AUTO_MODE` is not `off`. Schedule
/// `PAPERRUST_AI_TRIGGER_TAG_CRON` (celery crontab like the `PAPERLESS_*_TASK_CRON` variables,
/// default every 5 minutes, `disable` turns it off), expires after 4 minutes.
#[cfg(feature = "ai")]
pub fn ai_beat_entries(env: &Env) -> Result<Vec<pl_queue::beat::BeatEntry>, String> {
    let vars: std::collections::HashMap<String, String> = env.vars().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let Ok(cfg) = pl_ai::config::AiConfig::from_env_map(&vars) else { return Ok(Vec::new()) };
    if cfg.rs.auto_trigger_tag.is_none() || cfg.rs.auto_mode == pl_ai::config::AutoMode::Off {
        return Ok(Vec::new());
    }
    let value = env.get("PAPERRUST_AI_TRIGGER_TAG_CRON").unwrap_or("*/5 * * * *");
    if value == "disable" {
        return Ok(Vec::new());
    }
    let schedule = pl_queue::cron::Crontab::from_paperless(value).map_err(|e| format!("PAPERRUST_AI_TRIGGER_TAG_CRON: {e}"))?;
    let mut headers = Map::new();
    headers.insert("trigger_source".into(), Value::String("scheduled".into()));
    Ok(vec![pl_queue::beat::BeatEntry {
        name: "PaperRust: process AI trigger tag".into(),
        task: names::AI_PROCESS_TRIGGER_TAG.into(),
        schedule,
        expires: 4.0 * 60.0,
        headers,
    }])
}

/// Without feature `ai` there is no AI beat entry.
#[cfg(not(feature = "ai"))]
pub fn ai_beat_entries(_env: &Env) -> Result<Vec<pl_queue::beat::BeatEntry>, String> {
    Ok(Vec::new())
}

// ------------------------------------------------------------------ background tasks

/// Running background tasks of this process.
pub struct Background {
    set: JoinSet<()>,
}

impl Background {
    /// Wait for all background tasks (after the stop signal).
    pub async fn join(mut self) {
        while self.set.join_next().await.is_some() {}
    }
}

/// Start worker, beat and folder watcher (worker modes) and the status relay between processes.
pub async fn start_background(
    state: &AppState,
    setup: &QueueSetup,
    mode: WorkerMode,
    stop: watch::Receiver<bool>,
) -> Result<Background, String> {
    let mut set = JoinSet::new();
    // Keep read-only mode (pl-guard) up to date from the DB in every process.
    set.spawn(pl_guard::readonly::refresher(state.db().clone(), stop.clone()));
    if mode.serves_http() {
        // SMTP intake for devices (pl-scan): only listens with PAPERRUST_SMTP_INTAKE=on.
        set.spawn(pl_scan::smtp::run_if_enabled(state.clone(), stop.clone()));
        // L3: keep the search index up to date for permissions/trash (docs/contract/pl-search.md "L3").
        set.spawn(crate::search_sync::run(state.clone(), stop.clone()));
    }
    if mode.runs_worker() {
        // Sync sequencer (pl-sync, docs/SYNC.md §2): numbers captured changes; idles while the
        // migration pl_sync.0001_paperrust_sync is not applied. Worker processes run it. Several
        // processes are safe (advisory lock in paperrust_sync_sequence()).
        // Restarted with new values when PAPERRUST_SYNC_* change in the web UI (docs/SETTINGS.md WP4c).
        // FINDINGS B2: own secondary pool (LISTEN + numbering), not from the general pool.
        let pool = pl_core::dbconn::side_pool(state.db(), 2);
        let start: crate::settings_live::Start = Box::new(move |s| {
            let env = Env::from_process();
            let cfg = pl_sync::SyncConfig::from_lookup(|k| env.get(k).map(str::to_string));
            let pool = pool.clone();
            Ok(Box::pin(async move { pl_sync::sequencer::run(pool, cfg, s).await }))
        });
        let keys = crate::settings_live::SYNC_KEYS.to_vec();
        set.spawn(crate::settings_live::restartable(state.clone(), "sync sequencer", keys, start, stop.clone()));
    }
    let pg = setup.pg.clone();
    if mode.serves_http() {
        // Events of separate `--worker-only` processes (NOTIFY) -> local bus -> WebSocket.
        let (pool, bus, stop) = (state.db().clone(), setup.events.clone(), stop.clone());
        set.spawn(async move {
            if let Err(e) = pl_queue::events::relay_notifications(pool, bus, stop).await {
                tracing::error!(error = %e, "status relay (LISTEN paperrust_status) stopped");
            }
        });
    }
    if !mode.runs_worker() {
        return Ok(Background { set });
    }
    let ctx = WorkerContext { state: state.clone(), queue: setup.queue.clone(), events: setup.events.clone(), cfg: setup.cfg.clone() };
    let registry = build_registry(&ctx).await?;
    tracing::info!(
        ported = ?registry.ported().collect::<Vec<_>>(),
        not_ported = ?registry.not_ported_names().collect::<Vec<_>>(),
        "worker task handlers"
    );

    // Beat: only tasks with a working handler (others would fail on every run). Rebuilt with the
    // new crontabs when they change in the web UI (docs/SETTINGS.md WP4c); an invalid schedule at
    // startup still fails the start like upstream.
    let ported: std::collections::HashSet<String> = registry.ported().map(String::from).collect();
    let beat_entries = move |log: bool| -> Result<Vec<pl_queue::beat::BeatEntry>, String> {
        let env = Env::from_process();
        let mut entries = pl_queue::beat::parse_beat_schedule(&env).map_err(|e| e.to_string())?;
        entries.extend(ai_beat_entries(&env)?);
        entries.extend(pl_guard::backup_beat_entry(|k| env.get(k).map(str::to_string))?);
        let (entries, skipped): (Vec<_>, Vec<_>) = entries.into_iter().partition(|e| ported.contains(&e.task));
        if log {
            for e in &skipped {
                tracing::warn!(entry = %e.name, task = %e.task, "beat entry skipped: task not ported to Rust");
            }
        }
        Ok(entries)
    };
    beat_entries(true)?;
    let (db, queue, tz) = (state.db().clone(), setup.queue.clone(), setup.cfg.tz);
    let start: crate::settings_live::Start = Box::new(move |s| {
        let beat =
            pl_queue::beat::Beat::new(beat_entries(false)?, Arc::new(pl_queue::beat::PgBeatStore::new(db.clone())), queue.clone(), tz);
        Ok(Box::pin(async move { beat.run(s).await }))
    });
    set.spawn(crate::settings_live::restartable(state.clone(), "scheduler", crate::settings_live::cron_keys(), start, stop.clone()));

    // Worker pool: restarted (running tasks finish first) when TASK_WORKERS / PAPERRUST_QUEUE_*
    // change in the web UI (docs/SETTINGS.md WP4c). The first run uses the registry built above.
    let first = std::sync::Mutex::new(Some(registry.into_handlers()));
    let (wctx, events, base_cfg) = (ctx.clone(), setup.events.clone(), setup.cfg.clone());
    let start: crate::settings_live::Start = Box::new(move |s| {
        let handlers = match first.lock() {
            Ok(mut g) => g.take(),
            Err(p) => p.into_inner().take(),
        };
        let (ctx, pg, events, base_cfg) = (wctx.clone(), pg.clone(), events.clone(), base_cfg.clone());
        Ok(Box::pin(async move {
            let handlers = match handlers {
                Some(h) => h,
                None => match build_registry(&ctx).await {
                    Ok(r) => r.into_handlers(),
                    Err(e) => {
                        tracing::error!(error = %e, "worker pool not restarted");
                        return;
                    }
                },
            };
            let cfg = queue_config(&Env::from_process()).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "invalid queue settings, keeping the previous ones");
                base_cfg
            });
            // FINDINGS B2: claim, heartbeat/lease, completion and retries run over its own
            // secondary pool (one connection per running task + claim/reap + reserve). They starved
            // on the general pool under load: heartbeat failed → lease lost →
            // task aborted and run again.
            let ctrl = pl_core::dbconn::side_pool(pg.pool(), (cfg.concurrency as u32).saturating_add(3));
            let pg = PgQueue::new(ctrl, cfg.max_deliveries);
            pl_queue::Worker::new(pg, handlers, cfg, events).run(s).await
        }))
    });
    let keys = crate::settings_live::WORKER_KEYS.to_vec();
    set.spawn(crate::settings_live::restartable(state.clone(), "worker pool", keys, start, stop.clone()));

    // Ransomware guard (pl-guard): only in the worker process, restarted when PAPERRUST_GUARD_* changes.
    let st = state.clone();
    let start: crate::settings_live::Start = Box::new(move |s| {
        let st = st.clone();
        Ok(Box::pin(async move { pl_guard::db::run_guard(st, s).await }))
    });
    set.spawn(crate::settings_live::restartable(
        state.clone(),
        "ransomware guard",
        pl_guard::config::GUARD_KEYS.to_vec(),
        start,
        stop.clone(),
    ));

    // Restarted when CONSUMER_POLLING_INTERVAL/RECURSIVE/… change in the web UI (docs/SETTINGS.md WP4a).
    let (st, queue) = (state.clone(), setup.queue.clone());
    let start: crate::settings_live::Start = Box::new(move |s| {
        let w = folder_watcher(&st, queue.clone())?;
        Ok(Box::pin(async move { w.run(false, s).await }))
    });
    let keys = crate::settings_live::WATCHER_KEYS.to_vec();
    set.spawn(crate::settings_live::restartable(state.clone(), "consumption folder watcher", keys, start, stop.clone()));

    if !mode.serves_http() {
        // Separate worker process: forward status events to the web processes (NOTIFY).
        let (pool, mut rx, mut s) = (state.db().clone(), setup.events.subscribe(), stop.clone());
        set.spawn(async move {
            loop {
                tokio::select! {
                    _ = s.changed() => if *s.borrow() { break },
                    m = rx.recv() => match m {
                        Ok(p) => if let Err(e) = pl_queue::events::notify(&pool, &p).await {
                            tracing::warn!(error = %e, "status NOTIFY failed");
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => tracing::warn!(n, "status events dropped"),
                        Err(_) => break,
                    },
                }
            }
        });
    }
    Ok(Background { set })
}

/// `document_consumer` (watch mode) feeding `consume_file` into the queue.
fn folder_watcher(state: &AppState, queue: Arc<dyn TaskQueue>) -> Result<pl_consumer::watcher::FolderWatcher, String> {
    let s = pl_consumer::ConsumerSettings::from_env().map_err(|e| e.0)?;
    let directory = pl_consumer::watcher::FolderWatcher::check_directory(&s.consumption_dir)?;
    Ok(pl_consumer::watcher::FolderWatcher {
        opts: pl_consumer::watcher::WatchOptions {
            directory,
            recursive: s.consumer_recursive,
            subdirs_as_tags: s.consumer_subdirs_as_tags,
            polling_interval: s.consumer_polling_interval,
            stability_delay: s.consumer_stability_delay,
            rescan_interval: std::time::Duration::from_secs(300),
        },
        filter: pl_consumer::watcher::ConsumerFilter::with_registry(&s.consumer_ignore_patterns, &s.consumer_ignore_dirs),
        pool: state.db().clone(),
        sink: Arc::new(pl_consumer::watcher::QueueSink(queue)),
    })
}

/// kwargs helper for tests and callers: `{"document_id": id}`.
pub fn document_kwargs(id: i64) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("document_id".into(), Value::from(id));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W14: TaskRequest → EnqueueOptions of the PgQueue dispatcher.
    #[test]
    fn dispatch_maps_task_request() {
        let id = uuid::Uuid::new_v4();
        let mut req = pl_core::tasks::TaskRequest::new("documents.tasks.sanity_check")
            .arg(1)
            .kwarg("raise_on_error", false)
            .trigger_source("manual")
            .header("x", 2);
        req.countdown = Some(5.0);
        req.expires = Some(60.0);
        req.task_id = Some(id);
        req.time_limit = (Some(30.0), None);
        let o = enqueue_options(&req);
        assert_eq!(o.args, vec![serde_json::json!(1)]);
        assert_eq!(o.trigger_source.as_deref(), Some("manual"));
        assert_eq!(o.headers, serde_json::json!({"x": 2}).as_object().unwrap().clone());
        assert_eq!((o.countdown, o.expires, o.task_id, o.time_limit), (Some(5.0), Some(60.0), Some(id), Some(30.0)));
        assert_eq!(o.all_headers()["trigger_source"], "manual");
    }

    fn ctx(task: &str, args: Vec<Value>, kwargs: Map<String, Value>) -> TaskContext {
        TaskContext {
            task_id: "t".into(),
            task_name: task.into(),
            args,
            kwargs,
            headers: Map::new(),
            retries: 0,
            events: EventBus::default(),
            threads_per_worker: 1,
        }
    }

    #[test]
    fn queue_ignores_retired_backend_values() {
        assert!(queue_config(&Env::from_pairs([("PAPERRUST_QUEUE", "postgres")])).is_ok());
        assert!(queue_config(&Env::from_pairs([("PAPERRUST_QUEUE", "celery")])).is_ok());
        assert!(queue_config(&Env::from_pairs([("PAPERRUST_QUEUE", "postgres"), ("PAPERRUST_QUEUE_BACKEND", "celery")])).is_ok());
    }

    #[test]
    fn args_positional_or_keyword() {
        let c = ctx(names::INDEX_DOCUMENT, vec![Value::from(7)], Map::new());
        assert_eq!(int_arg(&c, 0, "document_id").unwrap(), 7);
        let c = ctx(names::INDEX_DOCUMENT, vec![], document_kwargs(9));
        assert_eq!(int_arg(&c, 0, "document_id").unwrap(), 9);
        let c = ctx(names::INDEX_DOCUMENT, vec![], Map::new());
        let e = int_arg(&c, 0, "document_id").unwrap_err();
        assert_eq!(e.error_type, "TypeError");
        assert_eq!(e.message, "index_document() missing 1 required positional argument: 'document_id'");
    }

    #[tokio::test]
    async fn not_ported_fails_clearly() {
        let reg = TaskRegistry::new().not_ported(names::SANITY_CHECK, "x");
        assert!(!reg.is_ported(names::SANITY_CHECK));
        let reg = reg.register(names::SANITY_CHECK, |_c: TaskContext| async { Ok(Value::Null) });
        assert!(reg.is_ported(names::SANITY_CHECK));
        assert_eq!(reg.not_ported_names().count(), 0);
        let e = not_ported_error(names::EMPTY_TRASH, "hard delete");
        assert_eq!(e.error_type, "NotImplementedError");
        assert!(e.message.contains("noch nicht portiert"));
        assert_eq!(names::PROCESS_MAIL_ACCOUNTS, pl_mail::fetch::TASK_PROCESS_MAIL_ACCOUNTS);
        assert_eq!(names::APPLY_MAIL_ACTION, pl_mail::fetch::TASK_APPLY_MAIL_ACTION);
        assert_eq!(names::MAIL_ERROR_CALLBACK, pl_mail::fetch::TASK_ERROR_CALLBACK);
        assert_eq!(names::SEND_WEBHOOK, pl_mail::effects::webhook::TASK_SEND_WEBHOOK);
        let reg = TaskRegistry::new().not_ported(names::PROCESS_MAIL_ACCOUNTS, "x").with_handlers(&[names::PROCESS_MAIL_ACCOUNTS], |h| h);
        assert!(reg.is_ported(names::PROCESS_MAIL_ACCOUNTS));
    }

    #[test]
    fn ai_task_args() {
        let mut kw = Map::new();
        assert!(!rebuild_arg(&ctx(names::LLMINDEX_INDEX, vec![], kw.clone())).unwrap());
        kw.insert("rebuild".into(), Value::Bool(true));
        assert!(rebuild_arg(&ctx(names::LLMINDEX_INDEX, vec![], kw.clone())).unwrap());
        kw.insert("rebuild".into(), Value::from("yes"));
        assert_eq!(rebuild_arg(&ctx(names::LLMINDEX_INDEX, vec![], kw)).unwrap_err().error_type, "TypeError");
        let mut kw = document_kwargs(5);
        #[cfg(feature = "ai")]
        assert_eq!(names::AI_BACKFILL, pl_ai::server::jobs::BACKFILL_TASK);
        assert_eq!(trigger_arg(&ctx(names::AI_PROCESS_DOCUMENT, vec![], kw.clone())).unwrap(), "consumed");
        kw.insert("trigger".into(), Value::from("tag"));
        assert_eq!(trigger_arg(&ctx(names::AI_PROCESS_DOCUMENT, vec![], kw.clone())).unwrap(), "tag");
        kw.insert("trigger".into(), Value::from("x"));
        assert!(trigger_arg(&ctx(names::AI_PROCESS_DOCUMENT, vec![], kw)).is_err());
        // apply_ai_suggestions.delay_on_commit(action_id=…, document_id=…) or positional
        let mut kw = Map::new();
        kw.insert("action_id".into(), Value::from(2));
        kw.insert("document_id".into(), Value::from(9));
        let c = ctx(names::APPLY_AI_SUGGESTIONS, vec![], kw);
        assert_eq!((int_arg(&c, 0, "action_id").unwrap(), int_arg(&c, 1, "document_id").unwrap()), (2, 9));
        let c = ctx(names::APPLY_AI_SUGGESTIONS, vec![Value::from(3), Value::from(4)], Map::new());
        assert_eq!((int_arg(&c, 0, "action_id").unwrap(), int_arg(&c, 1, "document_id").unwrap()), (3, 4));
        // task names = pl-queue registry (retry policy, PaperlessTask type)
        assert_eq!(pl_queue::registry::tracked_task_type(names::LLMINDEX_INDEX), Some("llm_index"));
        let d = pl_queue::registry::task_def(names::APPLY_AI_SUGGESTIONS).unwrap();
        assert_eq!(d.autoretry.unwrap().max_retries, Some(3));
        assert!(pl_queue::registry::task_def(names::UPDATE_DOCUMENT_IN_LLM_INDEX).is_some());
        assert!(pl_queue::registry::task_def(names::REMOVE_DOCUMENT_FROM_LLM_INDEX).is_some());
        let beat = pl_queue::beat::DEFAULT_ENTRIES.iter().find(|e| e.3 == names::LLMINDEX_INDEX).unwrap();
        assert_eq!((beat.0, beat.1, beat.2), ("Rebuild LLM index", "PAPERLESS_LLM_INDEX_TASK_CRON", "10 2 * * *"));
    }

    #[cfg(feature = "ai")]
    #[test]
    fn ai_trigger_tag_beat_entry() {
        let none = ai_beat_entries(&Env::from_pairs([("PAPERRUST_AI_AUTO_TRIGGER_TAG", "ai")])).unwrap();
        assert!(none.is_empty(), "auto mode off → no entry");
        let env = Env::from_pairs([("PAPERRUST_AI_AUTO_TRIGGER_TAG", "ai"), ("PAPERRUST_AI_AUTO_MODE", "suggest")]);
        let e = ai_beat_entries(&env).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].task, names::AI_PROCESS_TRIGGER_TAG);
        assert_eq!(e[0].headers["trigger_source"], "scheduled");
        assert_eq!(e[0].schedule, pl_queue::cron::Crontab::from_paperless("*/5 * * * *").unwrap());
        let env = Env::from_pairs([
            ("PAPERRUST_AI_AUTO_TRIGGER_TAG", "ai"),
            ("PAPERRUST_AI_AUTO_MODE", "apply"),
            ("PAPERRUST_AI_TRIGGER_TAG_CRON", "disable"),
        ]);
        assert!(ai_beat_entries(&env).unwrap().is_empty());
        let env = Env::from_pairs([
            ("PAPERRUST_AI_AUTO_TRIGGER_TAG", "ai"),
            ("PAPERRUST_AI_AUTO_MODE", "apply"),
            ("PAPERRUST_AI_TRIGGER_TAG_CRON", "bogus"),
        ]);
        assert!(ai_beat_entries(&env).is_err());
    }

    #[cfg(feature = "ai")]
    #[tokio::test]
    async fn ai_sink_enqueues_on_queue() {
        use pl_ai::server::index::AiTaskSink;
        use std::sync::Mutex;
        type Sent = (String, Map<String, Value>, pl_queue::EnqueueOptions);
        #[derive(Default)]
        struct Rec(Mutex<Vec<Sent>>);
        impl TaskQueue for Rec {
            fn enqueue<'a>(
                &'a self,
                n: &'a str,
                k: Map<String, Value>,
                o: pl_queue::EnqueueOptions,
            ) -> futures::future::BoxFuture<'a, Result<uuid::Uuid, pl_queue::QueueError>> {
                self.0.lock().unwrap().push((n.into(), k, o));
                Box::pin(async { Ok(uuid::Uuid::nil()) })
            }
            fn status(&self, _: uuid::Uuid) -> futures::future::BoxFuture<'_, Result<Option<pl_queue::TaskInfo>, pl_queue::QueueError>> {
                Box::pin(async { Ok(None) })
            }
        }
        let rec = Arc::new(Rec::default());
        let sink = QueueAiSink(rec.clone());
        let mut kw = Map::new();
        kw.insert("rebuild".into(), Value::Bool(false));
        sink.enqueue(names::LLMINDEX_INDEX, kw, "system").await.unwrap();
        let got = rec.0.lock().unwrap();
        assert_eq!(got[0].0, names::LLMINDEX_INDEX);
        assert_eq!(got[0].1["rebuild"], Value::Bool(false));
        assert_eq!(got[0].2.trigger_source.as_deref(), Some("system"));
    }

    #[test]
    fn modes() {
        assert!(WorkerMode::WithWorker.runs_worker() && WorkerMode::WithWorker.serves_http());
        assert!(WorkerMode::WorkerOnly.runs_worker() && !WorkerMode::WorkerOnly.serves_http());
        assert!(!WorkerMode::NoWorker.runs_worker() && WorkerMode::NoWorker.serves_http());
    }
}
