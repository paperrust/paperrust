//! Workflow actions "e-mail" and "webhook" as `pl_consumer::stages::WorkflowEffects`.
//!
//! upstream: src/documents/workflows/actions.py@v3.4.2 (build_workflow_action_context,
//! execute_email_action, execute_webhook_action), src/documents/mail.py@v3.4.2 (send_email),
//! src/documents/workflows/webhooks.py@v3.4.2 (send_webhook).
//!
//! Two paths:
//! * **Consumer** (`email_in`/`webhook_in`, triggers "consumption started" and "document
//!   added"): runs with the connection of the open consumption transaction. Context, templates
//!   and attachment are built **immediately** like upstream (overrides or document instance at
//!   the time of the action); only the sending (SMTP) or the webhook without a queue continues
//!   in the background, so the transaction does not wait on the network.
//! * **Synchronous** (`email`/`webhook`, e.g. pl-documents-write "document updated"): secure the
//!   file immediately, then wait in the background for the commit, load the context and send.
//!
//! Webhooks are sent via the queue like upstream (`send_webhook` with retries); without a
//! queue directly with the same retry rule.

pub mod context;
pub mod smtp;
pub mod webhook;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono_tz::Tz;
use serde_json::{Map, Value};
use sqlx::{PgConnection, PgPool};

use pl_consumer::stages::{ActionContext as ConsumerCtx, ActionInput, BoxFuture, WorkflowEffects};
use pl_matching::workflow::{TRIGGER_DOCUMENT_UPDATED, TRIGGER_SCHEDULED};

use context::ActionContext;
use smtp::{EmailAttachment, EmailSettings, OutgoingMail};
use webhook::{WebhookFile, WebhookRequest, WebhookSettings};

/// Settings of the workflow effects.
#[derive(Debug, Clone)]
pub struct EffectsConfig {
    pub time_zone: Tz,
    /// `PAPERLESS_URL` (empty if not set).
    pub paperless_url: String,
    /// `BASE_URL`
    pub base_url: String,
    /// `ORIGINALS_DIR`
    pub originals_dir: PathBuf,
    /// Scratch area for attachments (`SCRATCH_DIR`).
    pub scratch_dir: PathBuf,
    /// How long to wait for the document commit (synchronous path only).
    pub commit_wait: Duration,
}

impl EffectsConfig {
    pub fn from_core(core: &pl_core::Config) -> Self {
        Self {
            time_zone: core.tz,
            paperless_url: core.paperless_url.clone().unwrap_or_default(),
            base_url: core.base_url.clone(),
            originals_dir: core.media_root.join("documents").join("originals"),
            scratch_dir: core.scratch_dir.clone(),
            commit_wait: Duration::from_secs(120),
        }
    }
}

/// Callback for "remove password".
pub type PasswordRemoval = Arc<dyn Fn(&ConsumerCtx) + Send + Sync>;

/// Callback "consumption finished" (`WorkflowEffects::consumption_finished`: document ID, input file).
pub type ConsumptionFinished = Arc<dyn Fn(i64, &Path) + Send + Sync>;

/// E-mail/webhook actions with external effect.
pub struct MailWorkflowEffects {
    pub pool: PgPool,
    pub queue: Option<Arc<dyn pl_queue::TaskQueue>>,
    pub email: Arc<EmailSettings>,
    pub webhook: Arc<WebhookSettings>,
    pub config: Arc<EffectsConfig>,
    /// Remove password is not part of this module (pl-pdfops); default: log only.
    pub password_removal: Option<PasswordRemoval>,
    /// Receiver of "consumption finished" (e.g. "remove password" for the consumption trigger).
    pub consumption_finished: Option<ConsumptionFinished>,
}

/// `WorkflowActionEmail`.
#[derive(Debug, Clone)]
pub struct EmailAction {
    pub subject: String,
    pub body: String,
    pub to: String,
    pub include_document: bool,
}

/// `WorkflowActionWebhook`.
#[derive(Debug, Clone)]
pub struct WebhookAction {
    pub url: String,
    pub use_params: bool,
    pub as_json: bool,
    pub params: Option<Value>,
    pub body: Option<String>,
    pub headers: Option<Value>,
    pub include_document: bool,
}

pub async fn load_email_action<'e, E: sqlx::PgExecutor<'e>>(ex: E, action_id: i64) -> Result<Option<EmailAction>, sqlx::Error> {
    let r: Option<(String, String, String, bool)> = sqlx::query_as(
        "SELECT e.subject, e.body, e.\"to\", e.include_document FROM documents_workflowaction a \
         JOIN documents_workflowactionemail e ON e.id = a.email_id WHERE a.id = $1",
    )
    .bind(action_id as i32)
    .fetch_optional(ex)
    .await?;
    Ok(r.map(|(subject, body, to, include_document)| EmailAction { subject, body, to, include_document }))
}

pub async fn load_webhook_action<'e, E: sqlx::PgExecutor<'e>>(ex: E, action_id: i64) -> Result<Option<WebhookAction>, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let r: Option<(String, bool, bool, Option<Value>, Option<String>, Option<Value>, bool)> = sqlx::query_as(
        "SELECT w.url, w.use_params, w.as_json, w.params, w.body, w.headers, w.include_document \
         FROM documents_workflowaction a JOIN documents_workflowactionwebhook w ON w.id = a.webhook_id WHERE a.id = $1",
    )
    .bind(action_id as i32)
    .fetch_optional(ex)
    .await?;
    Ok(r.map(|(url, use_params, as_json, params, body, headers, include_document)| WebhookAction {
        url,
        use_params,
        as_json,
        params,
        body,
        headers,
        include_document,
    }))
}

/// Copy of the file while it still exists (deleted after sending).
fn snapshot_file(src: &Path, scratch: &Path) -> Option<PathBuf> {
    if !src.is_file() {
        return None;
    }
    let dir = scratch.join(format!("paperrust-wf-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).ok()?;
    let dst = dir.join(src.file_name()?);
    std::fs::copy(src, &dst).ok()?;
    Some(dst)
}

fn cleanup(p: &Option<PathBuf>) {
    if let Some(p) = p
        && let Some(dir) = p.parent()
    {
        let _ = std::fs::remove_dir_all(dir);
    }
}

impl MailWorkflowEffects {
    pub fn new(pool: PgPool, queue: Option<Arc<dyn pl_queue::TaskQueue>>, core: &pl_core::Config) -> Result<Self, String> {
        let env = pl_core::config::Env::from_process();
        Ok(Self {
            pool,
            queue,
            email: Arc::new(EmailSettings::from_core(core)),
            webhook: Arc::new(WebhookSettings::from_env(&env)?),
            config: Arc::new(EffectsConfig::from_core(core)),
            password_removal: None,
            consumption_finished: None,
        })
    }

    /// Synchronous path: load context – stored document (waits for the commit)
    /// or consumption (only title/file known).
    async fn context(&self, ctx: &ConsumerCtx, original: Option<&Path>) -> Result<Option<ActionContext>, String> {
        let cfg = &self.config;
        match ctx.document_id {
            Some(id) => {
                let deadline = tokio::time::Instant::now() + cfg.commit_wait;
                loop {
                    let mut conn = self.pool.acquire().await.map_err(|e| e.to_string())?;
                    let c = context::for_document(&mut conn, id, cfg.time_zone, &cfg.paperless_url, &cfg.base_url, &cfg.originals_dir)
                        .await
                        .map_err(|e| e.to_string())?;
                    if c.is_some() {
                        return Ok(c);
                    }
                    drop(conn);
                    if tokio::time::Instant::now() >= deadline {
                        tracing::warn!(target: "paperless.workflows.actions", "Document {id} not committed in time, skipping action {}", ctx.workflow_action_id);
                        return Ok(None);
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
            None => {
                let mime = original.and_then(|p| pl_parsers::mime::detect_file(p).ok()).unwrap_or_default();
                Ok(Some(context::for_consumption(&ctx.title, &ctx.original_file, &mime, cfg.time_zone)))
            }
        }
    }

    /// Consumer path: `build_workflow_action_context(document, overrides)` via the caller's
    /// connection (sees the not yet committed document).
    pub async fn context_in(&self, conn: &mut PgConnection, input: &ActionInput) -> Result<Option<ActionContext>, String> {
        let cfg = &self.config;
        let e = |e: sqlx::Error| e.to_string();
        if let Some(o) = &input.overrides {
            return context::for_overrides(conn, o, &input.ctx.original_file, &input.mime_type, cfg.time_zone).await.map(Some).map_err(e);
        }
        if let Some(d) = &input.document {
            return context::for_row(conn, d, cfg.time_zone, &cfg.paperless_url, &cfg.base_url, &cfg.originals_dir)
                .await
                .map(Some)
                .map_err(e);
        }
        match input.ctx.document_id {
            Some(id) => {
                context::for_document(conn, id, cfg.time_zone, &cfg.paperless_url, &cfg.base_url, &cfg.originals_dir).await.map_err(e)
            }
            None => Ok(Some(context::for_consumption(&input.ctx.title, &input.ctx.original_file, &input.mime_type, cfg.time_zone))),
        }
    }

    /// Subject, text, attachment → finished message (`execute_email_action` up to before `send_email`).
    /// `original_file` = attachment for consumption/"added" (upstream `elif original_file:`).
    fn build_email(
        &self,
        action: &EmailAction,
        c: &ActionContext,
        ctx: &ConsumerCtx,
        original_file: Option<&Path>,
    ) -> Result<OutgoingMail, String> {
        let subject = if action.subject.is_empty() {
            String::new()
        } else {
            // syntax error → None → `"\r" in None` → TypeError
            c.render(&action.subject)?.ok_or("argument of type 'NoneType' is not iterable")?
        };
        // EmailMessage(body=None) → ""
        let body = if action.body.is_empty() { String::new() } else { c.render(&action.body)?.unwrap_or_default() };
        let mut attachments = Vec::new();
        if action.include_document {
            let is_doc = ctx.document_id.is_some();
            if (ctx.trigger_type == TRIGGER_DOCUMENT_UPDATED || ctx.trigger_type == TRIGGER_SCHEDULED) && is_doc {
                if let Some(src) = &c.source_path {
                    let friendly = if c.current_filename.is_empty() {
                        src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    } else {
                        file_name(&c.current_filename)
                    };
                    attachments.push(EmailAttachment { path: src.clone(), mime_type: c.mime_type.clone(), friendly_name: friendly });
                }
            } else if let Some(p) = original_file {
                let friendly = if c.current_filename.is_empty() {
                    file_name(&ctx.original_file.to_string_lossy())
                } else {
                    file_name(&c.current_filename)
                };
                attachments.push(EmailAttachment { path: p.to_path_buf(), mime_type: c.mime_type.clone(), friendly_name: friendly });
            }
        }
        let to: Vec<String> = action.to.split(',').map(str::to_string).collect();
        smtp::build_message(&self.email, &subject, &body, &to, &attachments)
    }

    async fn send_mail(&self, mail: &OutgoingMail, to: &str) -> Result<(), String> {
        let n = smtp::send(&self.email, mail).await.map_err(|e| e.to_string())?;
        tracing::debug!(target: "paperless.workflows.actions", "Sent {n} notification email(s) to {to}");
        Ok(())
    }

    /// `execute_email_action` (synchronous path).
    pub async fn execute_email(&self, ctx: &ConsumerCtx, snapshot: Option<PathBuf>) {
        if !self.email.enabled {
            tracing::error!(target: "paperless.workflows.actions", "Email backend has not been configured, cannot send email notifications");
            return;
        }
        let r = self.execute_email_inner(ctx, snapshot.as_deref()).await;
        if let Err(e) = r {
            tracing::error!(target: "paperless.workflows.actions", "Error occurred sending notification email: {e}");
        }
    }

    async fn execute_email_inner(&self, ctx: &ConsumerCtx, snapshot: Option<&Path>) -> Result<(), String> {
        let Some(action) = load_email_action(&self.pool, ctx.workflow_action_id).await.map_err(|e| e.to_string())? else {
            return Err("'NoneType' object has no attribute 'subject'".into());
        };
        let Some(c) = self.context(ctx, snapshot).await? else { return Ok(()) };
        let mail = self.build_email(&action, &c, ctx, snapshot)?;
        self.send_mail(&mail, &action.to).await
    }

    /// `execute_email_action` (consumer path): build the message in the transaction, send in the
    /// background. `None` = nothing to send (error already logged).
    pub async fn prepare_email_in(&self, conn: &mut PgConnection, input: &ActionInput) -> Option<(OutgoingMail, String)> {
        if !self.email.enabled {
            tracing::error!(target: "paperless.workflows.actions", "Email backend has not been configured, cannot send email notifications");
            return None;
        }
        let r: Result<Option<(OutgoingMail, String)>, String> = async {
            let ctx = &input.ctx;
            let Some(action) = load_email_action(&mut *conn, ctx.workflow_action_id).await.map_err(|e| e.to_string())? else {
                return Err("'NoneType' object has no attribute 'subject'".into());
            };
            let Some(c) = self.context_in(conn, input).await? else { return Ok(None) };
            let mail = self.build_email(&action, &c, ctx, Some(&ctx.original_file))?;
            Ok(Some((mail, action.to)))
        }
        .await;
        match r {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(target: "paperless.workflows.actions", "Error occurred sending notification email: {e}");
                None
            }
        }
    }

    /// `execute_webhook_action` → request (not yet sent), context and file from `c`/`file`.
    fn webhook_request(
        &self,
        action: WebhookAction,
        c: &ActionContext,
        ctx: &ConsumerCtx,
        file: Option<&Path>,
    ) -> Result<WebhookRequest, String> {
        let mut data = Value::Object(Map::new());
        if action.use_params {
            if let Some(Value::Object(params)) = &action.params {
                let mut m = Map::new();
                for (k, v) in params {
                    let text = match v {
                        Value::String(s) => s.clone(),
                        other => {
                            tracing::error!(target: "paperless.workflows.actions", "Error occurred parsing webhook params: Can't compile non template nodes: {other}");
                            break;
                        }
                    };
                    match c.render(&text) {
                        Ok(r) => {
                            m.insert(k.clone(), r.map(Value::String).unwrap_or(Value::Null));
                        }
                        Err(e) => {
                            tracing::error!(target: "paperless.workflows.actions", "Error occurred parsing webhook params: {e}");
                            break;
                        }
                    }
                }
                data = Value::Object(m);
            }
        } else if let Some(body) = action.body.as_deref().filter(|b| !b.is_empty()) {
            data = c.render(body)?.map(Value::String).unwrap_or(Value::Null);
        }
        let mut headers = Vec::new();
        if let Some(Value::Object(h)) = &action.headers {
            for (k, v) in h {
                headers.push((k.clone(), crate::pyval::to_str(v)));
            }
        }
        let mut wfile = None;
        if action.include_document {
            // upstream: original_file.open("rb") – if the file is missing, the action fails
            let missing = || format!("[Errno 2] No such file or directory: '{}'", ctx.original_file.display());
            let path = file.ok_or_else(missing)?;
            let content = std::fs::read(path).map_err(|_| missing())?;
            let name = if c.filename.is_empty() { file_name(&ctx.original_file.to_string_lossy()) } else { c.filename.clone() };
            wfile = Some(WebhookFile { name, content, mime: c.mime_type.clone() });
        }
        Ok(WebhookRequest { url: action.url, data, headers, file: wfile, as_json: action.as_json })
    }

    /// `execute_webhook_action` → request (synchronous path; `snapshot` = secured file).
    pub async fn build_webhook(&self, ctx: &ConsumerCtx, snapshot: Option<&Path>) -> Result<Option<WebhookRequest>, String> {
        let Some(action) = load_webhook_action(&self.pool, ctx.workflow_action_id).await.map_err(|e| e.to_string())? else {
            return Err("'NoneType' object has no attribute 'use_params'".into());
        };
        let Some(c) = self.context(ctx, snapshot).await? else { return Ok(None) };
        self.webhook_request(action, &c, ctx, snapshot).map(Some)
    }

    /// `execute_webhook_action` → request (consumer path, in the transaction; file read immediately).
    pub async fn build_webhook_in(&self, conn: &mut PgConnection, input: &ActionInput) -> Result<Option<WebhookRequest>, String> {
        let ctx = &input.ctx;
        let Some(action) = load_webhook_action(&mut *conn, ctx.workflow_action_id).await.map_err(|e| e.to_string())? else {
            return Err("'NoneType' object has no attribute 'use_params'".into());
        };
        let Some(c) = self.context_in(conn, input).await? else { return Ok(None) };
        self.webhook_request(action, &c, ctx, Some(&ctx.original_file)).map(Some)
    }

    /// `send_webhook.apply_async(...)`: queue, without a queue directly (with retries).
    async fn dispatch_webhook(&self, req: WebhookRequest) {
        match &self.queue {
            Some(q) => match q.enqueue(webhook::TASK_SEND_WEBHOOK, req.to_kwargs(), pl_queue::EnqueueOptions::default()).await {
                Ok(_) => tracing::debug!(target: "paperless.workflows.actions", "Webhook to {} queued", req.url),
                Err(e) => tracing::error!(target: "paperless.workflows.actions", "Error occurred sending webhook: {e}"),
            },
            None => {
                let _ = webhook::send_with_retries(&req, &self.webhook).await;
            }
        }
    }

    pub async fn execute_webhook(&self, ctx: &ConsumerCtx, snapshot: Option<PathBuf>) {
        match self.build_webhook(ctx, snapshot.as_deref()).await {
            Ok(Some(r)) => self.dispatch_webhook(r).await,
            Ok(None) => {}
            Err(e) => tracing::error!(target: "paperless.workflows.actions", "Error occurred sending webhook: {e}"),
        }
    }

    fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn(fut);
            }
            Err(_) => tracing::error!(target: "paperless.workflows.actions", "No async runtime for workflow action"),
        }
    }
}

fn file_name(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

/// Shared parts for `Arc<MailWorkflowEffects>` (spawn needs `'static`).
#[derive(Clone)]
pub struct SharedEffects(pub Arc<MailWorkflowEffects>);

impl WorkflowEffects for SharedEffects {
    fn email(&self, ctx: &ConsumerCtx) {
        let snap = snapshot_file(&ctx.original_file, &self.0.config.scratch_dir);
        let me = self.0.clone();
        let ctx = ctx.clone();
        self.0.spawn(async move {
            me.execute_email(&ctx, snap.clone()).await;
            cleanup(&snap);
        });
    }

    fn webhook(&self, ctx: &ConsumerCtx) {
        let snap = snapshot_file(&ctx.original_file, &self.0.config.scratch_dir);
        let me = self.0.clone();
        let ctx = ctx.clone();
        self.0.spawn(async move {
            me.execute_webhook(&ctx, snap.clone()).await;
            cleanup(&snap);
        });
    }

    fn password_removal(&self, ctx: &ConsumerCtx) {
        match &self.0.password_removal {
            Some(f) => f(ctx),
            None => tracing::warn!(
                target: "paperless.workflows",
                "Password removal action {} not executed (not supported by PaperRust worker yet)",
                ctx.workflow_action_id
            ),
        }
    }

    fn consumption_finished(&self, doc_id: i64, original_file: &Path) {
        if let Some(f) = &self.0.consumption_finished {
            f(doc_id, original_file);
        }
    }

    fn email_in<'a>(&'a self, conn: &'a mut PgConnection, input: &'a ActionInput) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // Build the message (incl. attachment) now; only the sending runs in the background.
            if let Some((mail, to)) = self.0.prepare_email_in(conn, input).await {
                let me = self.0.clone();
                self.0.spawn(async move {
                    if let Err(e) = me.send_mail(&mail, &to).await {
                        tracing::error!(target: "paperless.workflows.actions", "Error occurred sending notification email: {e}");
                    }
                });
            }
        })
    }

    fn webhook_in<'a>(&'a self, conn: &'a mut PgConnection, input: &'a ActionInput) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            match self.0.build_webhook_in(conn, input).await {
                Ok(Some(req)) => {
                    // apply_async: enqueue or send in the background. FINDINGS B2: never wait here –
                    // the caller holds `conn` (often in a transaction), enqueueing takes its own
                    // connection from the same pool (two at once per request → pool empty).
                    let me = self.0.clone();
                    self.0.spawn(async move { me.dispatch_webhook(req).await });
                }
                Ok(None) => {}
                Err(e) => tracing::error!(target: "paperless.workflows.actions", "Error occurred sending webhook: {e}"),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pl_consumer::DocumentMetadataOverrides;

    fn effects(pool: PgPool) -> MailWorkflowEffects {
        let env = pl_core::config::Env::from_pairs([("PAPERLESS_WEBHOOKS_ALLOW_INTERNAL_REQUESTS", "true")]);
        MailWorkflowEffects {
            pool,
            queue: None,
            email: Arc::new(EmailSettings {
                host: "127.0.0.1".into(),
                port: 1,
                host_user: "u".into(),
                host_password: "p".into(),
                from_email: "paperless@example.org".into(),
                use_tls: false,
                use_ssl: false,
                timeout: Duration::from_secs(1),
                enabled: true,
                extra_roots: Vec::new(),
            }),
            webhook: Arc::new(WebhookSettings::from_env(&env).unwrap()),
            config: Arc::new(EffectsConfig {
                time_zone: chrono_tz::Europe::Zurich,
                paperless_url: "http://paperless.example".into(),
                base_url: "/".into(),
                originals_dir: PathBuf::from("originals"),
                scratch_dir: std::env::temp_dir(),
                commit_wait: Duration::from_secs(1),
            }),
            password_removal: None,
            consumption_finished: None,
        }
    }

    /// Consumer path in an open (rolled back) transaction: consumption context from the
    /// overrides (correspondent, created, custom field, `doc_id = ""`, title = path) and
    /// document instance in memory (unsaved correspondent), file read immediately.
    #[tokio::test]
    #[ignore = "braucht die eigene DB-Kopie pl_w9_test (PL_W9_TEST_DB=1, aus paperless_snapshot)"]
    async fn in_transaction_context_with_db() {
        if std::env::var("PL_W9_TEST_DB").is_err() {
            return;
        }
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect("postgres://paperless:paperless@127.0.0.1:54329/pl_w9_test")
            .await
            .unwrap();
        let fx = effects(pool.clone());
        let mut tx = pool.begin().await.unwrap();
        let wh: i32 = sqlx::query_scalar(
            "INSERT INTO documents_workflowactionwebhook (url, use_params, as_json, body, include_document) \
             VALUES ('http://127.0.0.1:9/h', false, false, $1, true) RETURNING id",
        )
        .bind("{{ correspondent }}|{{ doc_id }}|{{ created }}|{{ doc_title }}|{{ original_filename }}")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let action: i32 = sqlx::query_scalar(
            "INSERT INTO documents_workflowaction (type, \"order\", remove_all_tags, remove_all_document_types, \
             remove_all_correspondents, remove_all_storage_paths, remove_all_owners, remove_all_permissions, \
             remove_all_custom_fields, webhook_id, ai_create_missing, ai_overwrite_existing) \
             VALUES (4, 0, false, false, false, false, false, false, false, $1, false, false) RETURNING id",
        )
        .bind(wh)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let corr: (i32, String) =
            sqlx::query_as("SELECT id, name FROM documents_correspondent ORDER BY id LIMIT 1").fetch_one(&mut *tx).await.unwrap();
        let dir = std::env::temp_dir().join(format!("pl-w9-fx-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("scan.pdf");
        std::fs::write(&file, b"%PDF-1.4 test").unwrap();
        let ctx = ConsumerCtx {
            workflow_action_id: i64::from(action),
            action_type: 4,
            document_id: None,
            title: String::new(),
            original_file: file.clone(),
            trigger_type: pl_matching::workflow::TRIGGER_CONSUMPTION,
        };
        let overrides = DocumentMetadataOverrides {
            correspondent_id: Some(i64::from(corr.0)),
            created: chrono::NaiveDate::from_ymd_opt(2020, 5, 6),
            ..Default::default()
        };
        let input = ActionInput { ctx: ctx.clone(), overrides: Some(overrides), document: None, mime_type: "application/pdf".into() };
        let req = fx.build_webhook_in(&mut tx, &input).await.unwrap().unwrap();
        let s = req.data.as_str().unwrap().to_string();
        assert_eq!(s, format!("{}||2020-05-06|{}|scan", corr.1, file.display()));
        let f = req.file.unwrap();
        assert_eq!(
            (f.name.as_str(), f.content.as_slice(), f.mime.as_str()),
            (file.to_string_lossy().as_ref(), &b"%PDF-1.4 test"[..], "application/pdf")
        );

        // "Document added": instance in memory with a (still) unsaved correspondent
        let mut row = pl_consumer::db::load_document(&mut tx, 1).await.unwrap().unwrap();
        row.correspondent_id = Some(i64::from(corr.0));
        row.title = "Neu im Speicher".into();
        let input = ActionInput {
            ctx: ConsumerCtx { document_id: Some(1), trigger_type: pl_matching::workflow::TRIGGER_DOCUMENT_ADDED, ..ctx },
            overrides: None,
            mime_type: row.mime_type.clone(),
            document: Some(row),
        };
        let req = fx.build_webhook_in(&mut tx, &input).await.unwrap().unwrap();
        let s = req.data.as_str().unwrap().to_string();
        assert!(s.starts_with(&format!("{}|1|", corr.1)), "{s}");
        assert!(s.contains("|Neu im Speicher|"), "{s}");
        tx.rollback().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
