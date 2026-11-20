//! Mail fetching without Python: `process_mail_accounts`, `apply_mail_action`, `error_callback` for the
//! own queue (pl-queue). Contract: docs/contract/pl-mail-worker.md.
//!
//! upstream: src/paperless_mail/mail.py@v3.4.2, src/paperless_mail/tasks.py@v3.4.2,
//! src/paperless_mail/oauth.py@v3.4.2.
//!
//! Entry point for the worker (W1): [`register_mail_handlers`].

pub mod client;
pub mod criteria;
pub mod db;
pub mod generator;
pub mod gpg;
pub mod handler;
pub mod message;
pub mod tasks;

use std::path::PathBuf;
use std::sync::Arc;

use chrono_tz::Tz;
use sqlx::PgPool;

pub use handler::{AccountRun, MailError};
pub use tasks::{apply_mail_action, error_callback, process_mail_accounts};

pub const TASK_PROCESS_MAIL_ACCOUNTS: &str = "paperless_mail.tasks.process_mail_accounts";
pub const TASK_APPLY_MAIL_ACTION: &str = "paperless_mail.mail.apply_mail_action";
pub const TASK_ERROR_CALLBACK: &str = "paperless_mail.mail.error_callback";

/// Settings of the mail fetch.
#[derive(Debug, Clone)]
pub struct MailWorkerConfig {
    /// `PAPERLESS_TIME_ZONE` (`date.today()`, `make_aware`).
    pub time_zone: Tz,
    /// `PAPERLESS_SCRATCH_DIR`.
    pub scratch_dir: PathBuf,
    /// Connection (`PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS`, `PAPERLESS_EMAIL_CERTIFICATE_LOCATION`).
    pub connect: client::ConnectOptions,
    /// OAuth endpoints/client data (`PAPERLESS_GMAIL_OAUTH_*`, `PAPERLESS_OUTLOOK_OAUTH_*`).
    pub mail: crate::ctx::MailConfig,
    /// `PAPERLESS_AUDIT_LOG_ENABLED` (create correspondent).
    pub audit_log_enabled: bool,
    /// M1: `EMAIL_ENABLE_GPG_DECRYPTOR` / `EMAIL_GNUPG_HOME` (MailMessageDecryptor).
    pub gpg: gpg::GpgConfig,
}

impl MailWorkerConfig {
    pub fn from_core(core: &pl_core::Config) -> Self {
        let mail = crate::ctx::MailConfig::from_core(core);
        Self {
            time_zone: core.tz,
            scratch_dir: core.scratch_dir.clone(),
            connect: client::ConnectOptions {
                allow_internal_hosts: core.email_allow_internal_hosts,
                certificate_file: core.email_certificate_file.clone(),
                timeout: mail.imap_timeout,
                extra_roots: Vec::new(),
            },
            mail,
            audit_log_enabled: core.audit_log_enabled,
            gpg: gpg::GpgConfig::from_core(core),
        }
    }
}

/// Shared state of the mail tasks.
pub struct MailWorker {
    pub pool: PgPool,
    /// For `chord(consume_file…, apply_mail_action).on_error(error_callback)`.
    pub queue: Arc<dyn pl_queue::TaskQueue>,
    pub config: MailWorkerConfig,
    /// `is_mime_type_supported` (same parsers as the Rust consumer).
    pub parsers: pl_parsers::ParserRegistry,
}

impl MailWorker {
    pub fn new(pool: PgPool, queue: Arc<dyn pl_queue::TaskQueue>, config: MailWorkerConfig) -> Self {
        Self { pool, queue, config, parsers: pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default()) }
    }
}

/// Registers the mail tasks (`process_mail_accounts`, `apply_mail_action`, `error_callback`)
/// and the webhook task of the workflows (`documents.workflows.webhooks.send_webhook`, with the
/// upstream retry rule from pl-queue).
///
/// The schedule ("Check all e-mail accounts", `PAPERLESS_EMAIL_TASK_CRON`) is already in
/// `pl_queue::beat::DEFAULT_ENTRIES`; the beat process enqueues `process_mail_accounts`.
pub fn register_mail_handlers(
    handlers: pl_queue::Handlers,
    worker: Arc<MailWorker>,
    webhooks: Arc<crate::effects::webhook::WebhookSettings>,
) -> pl_queue::Handlers {
    let w1 = worker.clone();
    let w2 = worker.clone();
    let w3 = worker;
    handlers
        .register(TASK_PROCESS_MAIL_ACCOUNTS, move |ctx: pl_queue::TaskContext| {
            let w = w1.clone();
            async move { tasks::process_mail_accounts_task(w, ctx).await }
        })
        .register(TASK_APPLY_MAIL_ACTION, move |ctx: pl_queue::TaskContext| {
            let w = w2.clone();
            async move { tasks::apply_mail_action_task(w, ctx).await }
        })
        .register(TASK_ERROR_CALLBACK, move |ctx: pl_queue::TaskContext| {
            let w = w3.clone();
            async move { tasks::error_callback_task(w, ctx).await }
        })
        .register(crate::effects::webhook::TASK_SEND_WEBHOOK, move |ctx: pl_queue::TaskContext| {
            let s = webhooks.clone();
            async move { crate::effects::webhook::send_webhook_task(s, ctx).await }
        })
}
