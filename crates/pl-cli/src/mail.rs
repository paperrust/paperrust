//! `mail_fetcher`: fetch mail accounts and apply rules.
//!
//! upstream: src/paperless_mail/management/commands/mail_fetcher.py (v3.4.2) →
//! `paperless_mail.tasks.process_mail_accounts()`.
//!
//! The implementation belongs to W6 (pl-mail `fetch::tasks::process_mail_accounts`). pl-cli does not depend on
//! pl-mail; W1 registers `set_mail_fetcher(...)` at startup. Without registration: clear message.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use crate::args;
use crate::{CliError, CliResult};

pub type FetchFuture = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;
pub type MailFetcher = Box<dyn Fn() -> FetchFuture + Send + Sync>;

static FETCHER: OnceLock<MailFetcher> = OnceLock::new();

/// To be called by W1, e.g. `pl_cli::mail::set_mail_fetcher(Box::new(|| Box::pin(async { … })))`.
pub fn set_mail_fetcher(f: MailFetcher) -> bool {
    FETCHER.set(f).is_ok()
}

pub async fn command(argv: Vec<String>) -> CliResult {
    args::parse(&argv, &[], 0)?;
    let Some(f) = FETCHER.get() else {
        return Err(CliError::Command(
            "mail_fetcher is not available in this build: the mail worker (pl-mail, W6) is not registered with pl-cli".into(),
        ));
    };
    match f().await {
        Ok(msg) => {
            if !msg.is_empty() {
                tracing::info!(target: "paperless_mail", "{msg}");
            }
            Ok(())
        }
        Err(e) => Err(CliError::Other(e)),
    }
}
