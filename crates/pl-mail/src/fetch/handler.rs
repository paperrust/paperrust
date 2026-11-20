//! `MailAccountHandler` (Python-free).
//!
//! upstream: src/paperless_mail/mail.py@v3.4.2 (MailAccountHandler.handle_mail_account,
//! _handle_mail_rule, _handle_message, _process_attachments, _process_eml, _get_title,
//! _get_correspondent, _get_uid_validity, _record_processed_without_consumption,
//! queue_consumption_tasks, MAIL_FETCH_BATCH_SIZE, PROCESSED_UID_QUERY_BATCH_SIZE).
//!
//! Deviations: see docs/contract/pl-mail-worker.md ("Known differences").

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use serde_json::{Map, Value, json};
use unicode_normalization::UnicodeNormalization;

use super::client::{ImapError, MailBox};
use super::criteria::{self, ACTION_MOVE, MailAction, RuleFilter};
use super::db::{self, Account, NewProcessed, Rule};
use super::message::FetchedMessage;
use super::{MailWorker, TASK_APPLY_MAIL_ACTION, TASK_ERROR_CALLBACK};

pub const MAIL_FETCH_BATCH_SIZE: usize = 500;
pub const PROCESSED_UID_QUERY_BATCH_SIZE: usize = 10_000;

pub const SCOPE_ATTACHMENTS_ONLY: i16 = 1;
pub const SCOPE_EML_ONLY: i16 = 2;
pub const SCOPE_EVERYTHING: i16 = 3;
pub const ATTACHMENT_TYPE_ATTACHMENTS_ONLY: i16 = 1;
pub const TITLE_FROM_SUBJECT: i16 = 1;
pub const TITLE_FROM_FILENAME: i16 = 2;
pub const TITLE_NONE: i16 = 3;
pub const CORRESPONDENT_FROM_NOTHING: i16 = 1;
pub const CORRESPONDENT_FROM_EMAIL: i16 = 2;
pub const CORRESPONDENT_FROM_NAME: i16 = 3;
pub const CORRESPONDENT_FROM_CUSTOM: i16 = 4;

/// `MailError` (logged by `process_mail_accounts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailError(pub String);

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Error for a single rule/message (upstream: any exception → `log.exception`).
#[derive(Debug, Clone)]
pub enum HandleError {
    Mail(String),
    Other(String),
}

impl std::fmt::Display for HandleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandleError::Mail(m) | HandleError::Other(m) => f.write_str(m),
        }
    }
}

impl From<ImapError> for HandleError {
    fn from(e: ImapError) -> Self {
        match e {
            ImapError::Mail(m) => HandleError::Mail(m),
            other => HandleError::Other(other.to_string()),
        }
    }
}

impl From<sqlx::Error> for HandleError {
    fn from(e: sqlx::Error) -> Self {
        HandleError::Other(format!("DatabaseError: {e}"))
    }
}

/// Python `Path(name).stem` (POSIX).
pub fn path_stem(p: &str) -> String {
    let trimmed = p.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or("");
    if name == "." || name == ".." {
        return if name == ".." { "..".into() } else { String::new() };
    }
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// `tempfile.mkdtemp(prefix="paperless-mail-", dir=SCRATCH_DIR)`.
fn mkdtemp(scratch: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(scratch)?;
    for _ in 0..100 {
        let p = scratch.join(format!("paperless-mail-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("No usable temporary directory name found"))
}

/// `tempfile.mkstemp(prefix="paperless-mail-", dir=SCRATCH_DIR, suffix=".eml")`.
fn mkstemp_eml(scratch: &Path, data: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(scratch)?;
    for _ in 0..100 {
        let p = scratch.join(format!("paperless-mail-{}.eml", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(mut f) => {
                f.write_all(data)?;
                return Ok(p);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("No usable temporary file name found"))
}

/// Python `datetime.isoformat()` for a date with a fixed offset (second offsets like LMT).
pub fn py_isoformat(d: &chrono::DateTime<chrono::FixedOffset>) -> String {
    let base = d.naive_local().format("%Y-%m-%dT%H:%M:%S").to_string();
    let us = d.timestamp_subsec_micros();
    let frac = if us > 0 { format!(".{us:06}") } else { String::new() };
    let off = d.offset().local_minus_utc();
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    let tz = if s > 0 { format!("{sign}{h:02}:{m:02}:{s:02}") } else { format!("{sign}{h:02}:{m:02}") };
    format!("{base}{frac}{tz}")
}

/// Counterpart of [`py_isoformat`] (also `Z`, naive values → `None`).
pub fn parse_py_isoformat(s: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(d);
    }
    // ±HH:MM:SS offset
    let (dt, off) = s.split_at(s.len().checked_sub(9)?);
    let sign = match off.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let parts: Vec<i32> = off[1..].split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let secs = sign * (parts.first()? * 3600 + parts.get(1)? * 60 + parts.get(2)?);
    let naive = chrono::NaiveDateTime::parse_from_str(dt, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
    let fo = chrono::FixedOffset::east_opt(secs)?;
    naive.and_local_timezone(fo).single()
}

pub struct AccountRun<'a> {
    pub worker: &'a MailWorker,
    /// `consumed_messages` (folder, uid) across all rules of an account.
    consumed: HashSet<(String, String)>,
    uid_validity: Option<String>,
    /// IMAP commands of this run (diagnostics/parity).
    pub transcript: Vec<String>,
}

impl<'a> AccountRun<'a> {
    pub fn new(worker: &'a MailWorker) -> Self {
        Self { worker, consumed: HashSet::new(), uid_validity: None, transcript: Vec::new() }
    }

    /// `handle_mail_account(account)`: number of files put into the queue.
    pub async fn handle_mail_account(&mut self, mut account: Account) -> Result<i64, MailError> {
        tracing::debug!(target: "paperless_mail", "Processing mail account {}", account.name);
        let cfg = &self.worker.config;
        let port = u16::try_from(account.imap_port.unwrap_or(0)).unwrap_or(0);
        let mut mb = match MailBox::connect(&account.imap_server, port, account.imap_security, &cfg.connect).await {
            Ok(m) => m,
            Err(ImapError::Mail(m)) => return Err(MailError(m)),
            Err(e) => {
                tracing::error!(target: "paperless_mail", "Error while retrieving mailbox {}: {e}", account.name);
                return Ok(0);
            }
        };
        let result = self.with_mailbox(&mut mb, &mut account).await;
        let logged_out = mb.logout().await;
        self.transcript.append(&mut mb.transcript);
        match result {
            Ok(n) if logged_out => Ok(n),
            Ok(n) => {
                tracing::error!(target: "paperless_mail", "Error while retrieving mailbox {}: logout without BYE", account.name);
                Ok(n)
            }
            Err(HandleError::Mail(m)) if logged_out => Err(MailError(m)),
            Err(e) => {
                tracing::error!(target: "paperless_mail", "Error while retrieving mailbox {}: {e}", account.name);
                Ok(0)
            }
        }
    }

    async fn with_mailbox(&mut self, mb: &mut MailBox, account: &mut Account) -> Result<i64, HandleError> {
        let w = self.worker;
        let mut total: i64 = 0;
        if account.is_token && account.expiration.is_some_and(|e| e < Utc::now()) {
            let stored = crate::oauth::StoredAccount {
                id: account.id,
                account_type: account.account_type,
                refresh_token: account.refresh_token.clone(),
            };
            match crate::oauth::refresh_account_oauth_token(&w.pool, &stored, &w.config.mail).await {
                Ok(Some(_)) => db::reload_account(&w.pool, account).await?,
                Ok(None) => return Ok(total),
                Err(e) => return Err(HandleError::Other(format!("KeyError: {e:?}"))),
            }
        }
        let supports_gmail_labels = mb.capabilities.iter().any(|c| c == "X-GM-EXT-1");
        let supports_auth_plain = mb.capabilities.iter().any(|c| c == "AUTH=PLAIN");
        tracing::debug!(target: "paperless_mail", "GMAIL Label Support: {supports_gmail_labels}");
        tracing::debug!(target: "paperless_mail", "AUTH=PLAIN Support: {supports_auth_plain}");
        mb.login(&account.username, &account.password, account.is_token).await?;
        let rules = db::rules_of(&w.pool, account.id).await?;
        tracing::debug!(target: "paperless_mail", "Account {}: Processing {} rule(s)", account.name, rules.len());
        for rule in rules {
            if !rule.enabled {
                tracing::debug!(target: "paperless_mail", "Rule {}: Skipping disabled rule", rule.display());
                continue;
            }
            match self.handle_mail_rule(mb, &rule, account, supports_gmail_labels).await {
                Ok(n) => {
                    total += n;
                    if total > 0 && rule.stop_processing {
                        tracing::debug!(target: "paperless_mail", "Rule {}: Stopping processing rules due to stop_processing flag", rule.display());
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(target: "paperless_mail", "Rule {}: Error while processing rule: {e}", rule.display());
                    // connection broken (imaplib abort): all further commands fail as well
                }
            }
        }
        Ok(total)
    }

    async fn get_uid_validity(&self, mb: &mut MailBox, folder: &str) -> Option<String> {
        match mb.folder_status_uidvalidity(folder).await {
            Ok(Some(v)) => Some(v.to_string()),
            Ok(None) => None,
            Err(ImapError::Status { message, .. }) => {
                tracing::warn!(target: "paperless_mail", "Server does not support retrieving UIDVALIDITY for folder {folder}: {message}");
                None
            }
            Err(e) => {
                tracing::warn!(target: "paperless_mail", "Unable to retrieve UIDVALIDITY for folder {folder}: {e}");
                None
            }
        }
    }

    async fn handle_mail_rule(&mut self, mb: &mut MailBox, rule: &Rule, account: &Account, gmail: bool) -> Result<i64, HandleError> {
        let w = self.worker;
        let mut folders = vec![rule.folder.clone()];
        if rule.action == ACTION_MOVE {
            folders.insert(0, rule.action_parameter.clone().unwrap_or_else(|| "None".into()));
        }
        for folder in &folders {
            tracing::debug!(target: "paperless_mail", "Rule {}: Selecting folder {folder}", rule.display());
            match mb.folder_set(folder).await {
                Ok(()) => {}
                Err(ImapError::Status { kind: "MailboxFolderSelectError", .. }) => {
                    tracing::error!(target: "paperless_mail", "Unable to access folder {folder}, attempting folder listing");
                    match mb.folder_list().await {
                        Ok(list) => {
                            for f in list {
                                tracing::info!(target: "paperless_mail", "Located folder: {f}");
                            }
                        }
                        Err(e) => tracing::error!(target: "paperless_mail", "Exception during folder listing, unable to provide list folders: {e}"),
                    }
                    return Err(HandleError::Mail(format!("Rule {}: Folder {folder} does not exist in account {}", rule.display(), account.name)));
                }
                Err(e) => return Err(e.into()),
            }
        }
        self.uid_validity = self.get_uid_validity(mb, &rule.folder).await;

        let action = criteria::get_rule_action(rule.action, rule.action_parameter.as_deref(), gmail)
            .map_err(|e| HandleError::Other(e.to_string()))?;
        let filter = RuleFilter {
            maximum_age: rule.maximum_age,
            filter_from: rule.filter_from.clone(),
            filter_to: rule.filter_to.clone(),
            filter_subject: rule.filter_subject.clone(),
            filter_body: rule.filter_body.clone(),
        };
        let today = Utc::now().with_timezone(&w.config.time_zone).date_naive();
        let crit = criteria::make_criterias(&filter, &action, today);
        tracing::debug!(target: "paperless_mail", "Rule {}: Searching folder with criteria {crit}", rule.display());

        let encoded = encode_criteria(&crit, &account.character_set)
            .map_err(|_| HandleError::Mail(format!("Rule {}: Error while searching folder {}", rule.display(), rule.folder)))?;
        let all_uids: Vec<String> = match mb.uids(&encoded, Some(&account.character_set)).await {
            Ok(u) => u,
            Err(_) => return Err(HandleError::Mail(format!("Rule {}: Error while searching folder {}", rule.display(), rule.folder))),
        };
        let all: HashSet<String> = all_uids.into_iter().collect();
        let all_list: Vec<String> = all.iter().cloned().collect();
        let mut processed: HashSet<String> = HashSet::new();
        for chunk in all_list.chunks(PROCESSED_UID_QUERY_BATCH_SIZE) {
            processed.extend(db::processed_uids(&w.pool, rule.id, &rule.folder, chunk, self.uid_validity.as_deref()).await?);
        }
        let mut new_uids: Vec<String> = all.difference(&processed).cloned().collect();
        if new_uids.is_empty() {
            tracing::debug!(target: "paperless_mail", "Rule {}: No new mail matching criteria {crit}", rule.display());
            return Ok(0);
        }
        // sorted(new_uids, key=int)
        new_uids.sort_by_key(|u| u.parse::<u128>().unwrap_or(u128::MAX));

        let mut mails_processed = 0;
        let mut total = 0;
        let mut seen: HashSet<(String, String)> = HashSet::new();
        // M.fetch is a generator: errors when fetching a batch only show up on iteration
        // (then no MailError, but the IMAP exception).
        for chunk in new_uids.chunks(MAIL_FETCH_BATCH_SIZE) {
            let pairs = mb.fetch_bulk(chunk, MAIL_FETCH_BATCH_SIZE).await?;
            if pairs.is_empty() {
                break;
            }
            for pair in pairs {
                let message = FetchedMessage::from_fetch_items(&pair);
                let uid = message.uid_str();
                let key = (rule.folder.clone(), uid.clone());
                if seen.contains(&key) {
                    tracing::debug!(target: "paperless_mail", "Skipping duplicate fetched mail '{uid}' subject '{}' from '{}'.", message.subject(), message.from_());
                    continue;
                }
                seen.insert(key.clone());
                if self.consumed.contains(&key) {
                    tracing::debug!(target: "paperless_mail", "Skipping mail '{uid}' subject '{}' from '{}', already queued by a previous rule in this run.", message.subject(), message.from_());
                    continue;
                }
                let already = db::processed_uids(&w.pool, rule.id, &rule.folder, std::slice::from_ref(&uid), self.uid_validity.as_deref()).await?;
                if !already.is_empty() {
                    tracing::debug!(target: "paperless_mail", "Skipping mail '{uid}' subject '{}' from '{}', already processed.", message.subject(), message.from_());
                    continue;
                }
                match self.handle_message(&message, rule).await {
                    Ok(n) => {
                        if n > 0 {
                            self.consumed.insert(key);
                        }
                        total += n;
                        mails_processed += 1;
                    }
                    Err(e) => {
                        tracing::error!(target: "paperless_mail", "Rule {}: Error while processing mail {uid}: {e}", rule.display());
                    }
                }
            }
        }
        tracing::debug!(target: "paperless_mail", "Rule {}: Processed {mails_processed} matching mail(s)", rule.display());
        let _ = action;
        Ok(total)
    }

    async fn handle_message(&mut self, message: &FetchedMessage, rule: &Rule) -> Result<i64, HandleError> {
        // _preprocess_message: MailMessageDecryptor (M1)
        let decrypted = super::gpg::decrypt_if_needed(&self.worker.config.gpg, &message.mail, &message.raw)
            .await
            .map_err(HandleError::Other)?
            .map(|raw| message.with_raw(raw));
        let message = decrypted.as_ref().unwrap_or(message);
        let atts = message.attachments();
        let mut processed = 0;
        if atts.is_empty() && rule.consumption_scope == SCOPE_ATTACHMENTS_ONLY {
            self.record_processed_without_consumption(message, rule).await?;
            return Ok(processed);
        }
        tracing::debug!(
            target: "paperless_mail",
            "Rule {}: Processing mail {} from {} with {} attachment(s)",
            rule.display(),
            message.subject(),
            message.from_(),
            atts.len()
        );
        let w = self.worker;
        let tag_ids = db::rule_tag_ids(&w.pool, rule.id).await?;
        if rule.consumption_scope == SCOPE_EML_ONLY || rule.consumption_scope == SCOPE_EVERYTHING {
            processed += self.process_eml(message, rule, &tag_ids).await?;
        }
        if rule.consumption_scope == SCOPE_ATTACHMENTS_ONLY || rule.consumption_scope == SCOPE_EVERYTHING {
            processed += self.process_attachments(message, &atts, rule, &tag_ids).await?;
        }
        Ok(processed)
    }

    async fn record_processed_without_consumption(&self, message: &FetchedMessage, rule: &Rule) -> Result<(), HandleError> {
        let subject = message.subject();
        let uid = message.uid_str();
        let p = NewProcessed {
            owner_id: rule.owner_id,
            rule_id: rule.id,
            folder: &rule.folder,
            uid: &uid,
            uid_validity: self.uid_validity.as_deref(),
            subject: &subject,
            received: message.date_aware(self.worker.config.time_zone),
            status: "PROCESSED_WO_CONSUMPTION",
            error: None,
        };
        db::get_or_create_processed(&self.worker.pool, &p).await?;
        Ok(())
    }

    async fn get_correspondent(&self, message: &FetchedMessage, rule: &Rule) -> Result<Option<i64>, HandleError> {
        let w = self.worker;
        let audit = w.config.audit_log_enabled;
        let from_name = |name: String| async move { db::correspondent_from_name(&w.pool, &name, audit).await.map_err(HandleError::Other) };
        match rule.assign_correspondent_from {
            CORRESPONDENT_FROM_NOTHING => Ok(None),
            CORRESPONDENT_FROM_EMAIL => from_name(message.from_()).await,
            CORRESPONDENT_FROM_NAME => match message.from_name() {
                Some(n) if !n.is_empty() => from_name(n).await,
                _ => from_name(message.from_()).await,
            },
            CORRESPONDENT_FROM_CUSTOM => Ok(rule.assign_correspondent_id.map(i64::from)),
            _ => Err(HandleError::Other("Unknown correspondent selector".into())),
        }
    }

    fn get_title(message: &FetchedMessage, filename: &str, rule: &Rule) -> Result<Option<String>, HandleError> {
        match rule.assign_title_from {
            TITLE_FROM_SUBJECT => Ok(Some(nfc(&message.subject()))),
            TITLE_FROM_FILENAME => Ok(Some(nfc(&path_stem(filename)))),
            TITLE_NONE => Ok(None),
            _ => Err(HandleError::Other("Unknown title selector.".into())),
        }
    }

    async fn process_attachments(
        &mut self,
        message: &FetchedMessage,
        atts: &[super::message::Attachment],
        rule: &Rule,
        tag_ids: &[i64],
    ) -> Result<i64, HandleError> {
        let w = self.worker;
        let mut processed = 0;
        let mut consume_tasks = Vec::new();
        for att in atts {
            if att.content_disposition != "attachment" && rule.attachment_type == ATTACHMENT_TYPE_ATTACHMENTS_ONLY {
                tracing::debug!(target: "paperless_mail", "Rule {}: Skipping attachment {} with content disposition {}", rule.display(), att.filename, att.content_disposition);
                continue;
            }
            if !criteria::filename_inclusion_matches(rule.filter_attachment_filename_include.as_deref(), &att.filename) {
                tracing::debug!(target: "paperless_mail", "Rule {}: Skipping attachment {} does not match pattern {:?}", rule.display(), att.filename, rule.filter_attachment_filename_include);
                continue;
            } else if criteria::filename_exclusion_matches(rule.filter_attachment_filename_exclude.as_deref(), &att.filename) {
                tracing::debug!(target: "paperless_mail", "Rule {}: Skipping attachment {} does match pattern {:?}", rule.display(), att.filename, rule.filter_attachment_filename_exclude);
                continue;
            }
            let correspondent = self.get_correspondent(message, rule).await?;
            let title = Self::get_title(message, &att.filename, rule)?;
            // magic.from_buffer(att.payload, mime=True)
            let mime_type = pl_parsers::mime::detect_bytes(&att.payload);
            if w.parsers.is_mime_type_supported(mime_type) {
                tracing::info!(target: "paperless_mail", "Rule {}: Consuming attachment {} from mail {} from {}", rule.display(), att.filename, message.subject(), message.from_());
                let temp_dir = mkdtemp(&w.config.scratch_dir).map_err(|e| HandleError::Other(e.to_string()))?;
                let attachment_name = pl_templating::sanitize_filename(&nfc(&att.filename), "");
                let temp_filename =
                    if attachment_name.is_empty() { temp_dir.join("no-name-attachment") } else { temp_dir.join(&attachment_name) };
                std::fs::write(&temp_filename, &att.payload).map_err(|e| HandleError::Other(e.to_string()))?;
                let input = consumable(&temp_filename, rule.id, mime_type);
                let overrides = pl_consumer::DocumentMetadataOverrides {
                    title,
                    filename: Some(attachment_name),
                    correspondent_id: correspondent,
                    document_type_id: rule.assign_document_type_id.map(i64::from),
                    tag_ids: Some(tag_ids.to_vec()),
                    owner_id: if rule.assign_owner_from_rule { rule.owner_id.map(i64::from) } else { None },
                    ..Default::default()
                };
                consume_tasks.push(consume_signature(&input, &overrides)?);
                processed += 1;
            } else {
                tracing::debug!(target: "paperless_mail", "Rule {}: Skipping attachment {} since guessed mime type {mime_type} is not supported by PaperRust", rule.display(), att.filename);
            }
        }
        if !consume_tasks.is_empty() {
            self.queue_consumption_tasks(consume_tasks, rule, message).await?;
        } else {
            self.record_processed_without_consumption(message, rule).await?;
        }
        Ok(processed)
    }

    async fn process_eml(&mut self, message: &FetchedMessage, rule: &Rule, tag_ids: &[i64]) -> Result<i64, HandleError> {
        let w = self.worker;
        let bytes = super::generator::eml_bytes(&message.mail.msg).map_err(|e| HandleError::Other(e.0))?;
        let temp_filename = mkstemp_eml(&w.config.scratch_dir, &bytes).map_err(|e| HandleError::Other(e.to_string()))?;
        let correspondent = self.get_correspondent(message, rule).await?;
        let subject = message.subject();
        tracing::info!(target: "paperless_mail", "Rule {}: Consuming eml from mail {subject} from {}", rule.display(), message.from_());
        let mime = pl_parsers::mime::detect_file(&temp_filename).unwrap_or_default();
        let input = consumable(&temp_filename, rule.id, &mime);
        let overrides = pl_consumer::DocumentMetadataOverrides {
            title: Some(subject.clone()),
            filename: Some(pl_templating::sanitize_filename(&nfc(&format!("{subject}.eml")), "")),
            correspondent_id: correspondent,
            document_type_id: rule.assign_document_type_id.map(i64::from),
            tag_ids: Some(tag_ids.to_vec()),
            // upstream: here without the `assign_owner_from_rule` check
            owner_id: rule.owner_id.map(i64::from),
            ..Default::default()
        };
        let sig = consume_signature(&input, &overrides)?;
        self.queue_consumption_tasks(vec![sig], rule, message).await?;
        Ok(1)
    }

    /// `queue_consumption_tasks`: `chord(consume_tasks, apply_mail_action.s(...)).on_error(error_callback.s(...))`.
    async fn queue_consumption_tasks(&self, consume_tasks: Vec<pl_queue::Signature>, rule: &Rule, message: &FetchedMessage) -> Result<(), HandleError> {
        let mut kw = Map::new();
        kw.insert("rule_id".into(), json!(rule.id));
        kw.insert("message_uid".into(), json!(message.uid));
        kw.insert("message_subject".into(), json!(message.subject()));
        // naive stays naive (apply_mail_action makes it aware); here already aware in TIME_ZONE
        kw.insert("message_date".into(), json!(py_isoformat(&message.date_aware(self.worker.config.time_zone))));
        kw.insert("uid_validity".into(), self.uid_validity.clone().map(Value::from).unwrap_or(Value::Null));
        let body = pl_queue::Signature::new(TASK_APPLY_MAIL_ACTION, kw.clone());
        let on_error = pl_queue::Signature::new(TASK_ERROR_CALLBACK, kw);
        self.worker
            .queue
            .enqueue_chord(consume_tasks, body, Some(on_error))
            .await
            .map_err(|e| HandleError::Other(format!("queue: {e}")))?;
        Ok(())
    }
}

/// `str(criteria).encode(charset)` (Python codec; unknown/not encodable → error).
pub fn encode_criteria(crit: &str, charset: &str) -> Result<Vec<u8>, String> {
    let label = charset.trim().to_ascii_lowercase().replace('_', "-");
    if matches!(label.as_str(), "us-ascii" | "ascii" | "646" | "us") {
        if crit.is_ascii() {
            return Ok(crit.as_bytes().to_vec());
        }
        return Err("'ascii' codec can't encode".into());
    }
    if matches!(label.as_str(), "utf-8" | "utf8" | "u8" | "utf") {
        return Ok(crit.as_bytes().to_vec());
    }
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).ok_or_else(|| format!("unknown encoding: {charset}"))?;
    let (out, _, had_errors) = enc.encode(crit);
    if had_errors {
        return Err(format!("'{charset}' codec can't encode"));
    }
    Ok(out.into_owned())
}

fn consumable(path: &Path, rule_id: i32, mime: &str) -> pl_consumer::ConsumableDocument {
    pl_consumer::ConsumableDocument {
        source: pl_consumer::data::DocumentSourceRepr(pl_consumer::DocumentSource::MailFetch),
        original_file: path.to_path_buf(),
        root_document_id: None,
        original_path: None,
        mailrule_id: Some(i64::from(rule_id)),
        mime_type: mime.to_string(),
    }
}

fn consume_signature(input: &pl_consumer::ConsumableDocument, overrides: &pl_consumer::DocumentMetadataOverrides) -> Result<pl_queue::Signature, HandleError> {
    let kwargs = pl_consumer::task::consume_kwargs(input, overrides).map_err(HandleError::Other)?;
    Ok(pl_queue::Signature::new(pl_consumer::task::TASK_NAME, kwargs).with_trigger("email_consume"))
}

/// Shared data for tasks (Arc, so handlers are `'static`).
pub type SharedWorker = Arc<MailWorker>;

/// Run the `MailAction` after consumption (`action.post_consume`).
pub async fn post_consume(mb: &mut MailBox, action: &MailAction, uid: &str, parameter: Option<&str>) -> Result<(), ImapError> {
    match action {
        MailAction::Delete => mb.delete(uid).await,
        MailAction::MarkRead => mb.flag(uid, &["\\Seen".into()], true).await,
        MailAction::Move => mb.move_to(uid, parameter.unwrap_or("None")).await,
        MailAction::Flag => mb.flag(uid, &["\\Flagged".into()], true).await,
        MailAction::Tag { color, keyword, gmail } => {
            if *gmail {
                mb.gmail_label(uid, keyword.as_deref()).await
            } else if let Some(color) = color {
                // remove all $MailFlagBits (upstream: Python set, random order; sorted here)
                let mut all: Vec<String> =
                    criteria::APPLE_MAIL_TAG_COLORS.iter().flat_map(|(_, f)| f.iter().map(|s| s.to_string())).collect();
                all.sort();
                all.dedup();
                mb.flag(uid, &all, false).await?;
                let set: Vec<String> = criteria::APPLE_MAIL_TAG_COLORS
                    .iter()
                    .find(|(c, _)| c == color)
                    .map(|(_, f)| f.iter().map(|s| s.to_string()).collect())
                    .unwrap_or_default();
                mb.flag(uid, &set, true).await?;
                mb.flag(uid, &["\\Flagged".into()], true).await
            } else if let Some(k) = keyword {
                mb.flag(uid, std::slice::from_ref(k), true).await
            } else {
                Err(ImapError::Mail("No keyword specified.".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems() {
        assert_eq!(path_stem("invoice.pdf"), "invoice");
        assert_eq!(path_stem("archive.tar.gz"), "archive.tar");
        assert_eq!(path_stem(".bashrc"), ".bashrc");
        assert_eq!(path_stem("noext"), "noext");
        assert_eq!(path_stem("a/b.txt"), "b");
        assert_eq!(path_stem("trailing."), "trailing.");
        assert_eq!(path_stem(""), "");
    }

    #[test]
    fn isoformat_roundtrip() {
        let d = chrono::DateTime::parse_from_rfc3339("2025-01-06T08:00:00+01:00").unwrap();
        assert_eq!(py_isoformat(&d), "2025-01-06T08:00:00+01:00");
        let lmt = chrono::FixedOffset::east_opt(34 * 60 + 8).unwrap();
        let d2 = chrono::NaiveDate::from_ymd_opt(1900, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap().and_local_timezone(lmt).unwrap();
        assert_eq!(py_isoformat(&d2), "1900-01-01T00:00:00+00:34:08");
        assert_eq!(parse_py_isoformat("1900-01-01T00:00:00+00:34:08"), Some(d2));
        assert_eq!(parse_py_isoformat("2025-01-06T08:00:00+01:00"), Some(d));
    }

    #[test]
    fn criteria_charsets() {
        assert_eq!(encode_criteria("(SUBJECT \"ä\")", "UTF-8").unwrap(), "(SUBJECT \"ä\")".as_bytes());
        assert!(encode_criteria("(SUBJECT \"ä\")", "US-ASCII").is_err());
        assert_eq!(encode_criteria("(SUBJECT \"ä\")", "iso-8859-1").unwrap(), b"(SUBJECT \"\xe4\")");
    }
}
