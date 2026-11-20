//! Database access of the mail fetch (accounts, rules, ProcessedMail, correspondents).
//!
//! upstream: src/paperless_mail/models.py@v3.4.2 (MailAccount, MailRule, ProcessedMail),
//! src/paperless_mail/mail.py@v3.4.2 (_correspondent_from_name, _record_processed_without_consumption,
//! apply_mail_action, error_callback), src/paperless_mail/tasks.py@v3.4.2 (process_mail_accounts),
//! django-auditlog (entry when creating a correspondent).

use chrono::{DateTime, FixedOffset, Utc};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};

/// `MailAccount`.
#[derive(Debug, Clone)]
pub struct Account {
    pub id: i32,
    pub name: String,
    pub imap_server: String,
    pub imap_port: Option<i32>,
    pub imap_security: i16,
    pub username: String,
    pub password: String,
    pub character_set: String,
    pub is_token: bool,
    pub account_type: i16,
    pub refresh_token: Option<String>,
    pub expiration: Option<DateTime<Utc>>,
}

const ACCOUNT_COLS: &str = "id, name, imap_server, imap_port, imap_security, username, password, character_set, \
    is_token, account_type, refresh_token, expiration";

fn account_from(r: &sqlx::postgres::PgRow) -> Result<Account, sqlx::Error> {
    Ok(Account {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        imap_server: r.try_get("imap_server")?,
        imap_port: r.try_get("imap_port")?,
        imap_security: r.try_get("imap_security")?,
        username: r.try_get("username")?,
        password: r.try_get("password")?,
        character_set: r.try_get("character_set")?,
        is_token: r.try_get("is_token")?,
        account_type: r.try_get("account_type")?,
        refresh_token: r.try_get("refresh_token")?,
        expiration: r.try_get("expiration")?,
    })
}

/// `MailAccount.objects.filter(pk__in=ids)` or `.all()` (no Meta ordering → by id).
pub async fn accounts(pool: &PgPool, ids: Option<&[i32]>) -> Result<Vec<Account>, sqlx::Error> {
    let rows = match ids {
        Some(ids) => {
            sqlx::query(&format!("SELECT {ACCOUNT_COLS} FROM paperless_mail_mailaccount WHERE id = ANY($1) ORDER BY id"))
                .bind(ids)
                .fetch_all(pool)
                .await?
        }
        None => sqlx::query(&format!("SELECT {ACCOUNT_COLS} FROM paperless_mail_mailaccount ORDER BY id")).fetch_all(pool).await?,
    };
    rows.iter().map(account_from).collect()
}

pub async fn account(pool: &PgPool, id: i32) -> Result<Option<Account>, sqlx::Error> {
    let r = sqlx::query(&format!("SELECT {ACCOUNT_COLS} FROM paperless_mail_mailaccount WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    r.as_ref().map(account_from).transpose()
}

/// `MailRule.objects.filter(account=account, enabled=True).exists()`.
pub async fn has_enabled_rules(pool: &PgPool, account_id: i32) -> Result<bool, sqlx::Error> {
    let (b,): (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM paperless_mail_mailrule WHERE account_id = $1 AND enabled)")
            .bind(account_id)
            .fetch_one(pool)
            .await?;
    Ok(b)
}

/// `MailRule`.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: i32,
    pub name: String,
    pub account_id: i32,
    pub account_name: String,
    pub enabled: bool,
    pub folder: String,
    pub filter_from: Option<String>,
    pub filter_to: Option<String>,
    pub filter_subject: Option<String>,
    pub filter_body: Option<String>,
    pub filter_attachment_filename_include: Option<String>,
    pub filter_attachment_filename_exclude: Option<String>,
    pub maximum_age: i32,
    pub action: i16,
    pub action_parameter: Option<String>,
    pub assign_title_from: i16,
    pub assign_correspondent_from: i16,
    pub assign_correspondent_id: Option<i32>,
    pub assign_document_type_id: Option<i32>,
    pub assign_owner_from_rule: bool,
    pub order: i16,
    pub attachment_type: i16,
    pub consumption_scope: i16,
    pub owner_id: Option<i32>,
    pub stop_processing: bool,
}

impl Rule {
    /// `str(rule)` = `"{account.name}.{name}"`.
    pub fn display(&self) -> String {
        format!("{}.{}", self.account_name, self.name)
    }
}

const RULE_COLS: &str = "r.id, r.name, r.account_id, a.name AS account_name, r.enabled, r.folder, r.filter_from, r.filter_to, \
    r.filter_subject, r.filter_body, r.filter_attachment_filename_include, r.filter_attachment_filename_exclude, \
    r.maximum_age, r.action, r.action_parameter, r.assign_title_from, r.assign_correspondent_from, \
    r.assign_correspondent_id, r.assign_document_type_id, r.assign_owner_from_rule, r.\"order\", r.attachment_type, \
    r.consumption_scope, r.owner_id, r.stop_processing";

fn rule_from(r: &sqlx::postgres::PgRow) -> Result<Rule, sqlx::Error> {
    let max_age: i16 = r.try_get("maximum_age")?;
    Ok(Rule {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        account_id: r.try_get("account_id")?,
        account_name: r.try_get("account_name")?,
        enabled: r.try_get("enabled")?,
        folder: r.try_get("folder")?,
        filter_from: r.try_get("filter_from")?,
        filter_to: r.try_get("filter_to")?,
        filter_subject: r.try_get("filter_subject")?,
        filter_body: r.try_get("filter_body")?,
        filter_attachment_filename_include: r.try_get("filter_attachment_filename_include")?,
        filter_attachment_filename_exclude: r.try_get("filter_attachment_filename_exclude")?,
        maximum_age: i32::from(max_age),
        action: r.try_get("action")?,
        action_parameter: r.try_get("action_parameter")?,
        assign_title_from: r.try_get("assign_title_from")?,
        assign_correspondent_from: r.try_get("assign_correspondent_from")?,
        assign_correspondent_id: r.try_get("assign_correspondent_id")?,
        assign_document_type_id: r.try_get("assign_document_type_id")?,
        assign_owner_from_rule: r.try_get("assign_owner_from_rule")?,
        order: r.try_get("order")?,
        attachment_type: r.try_get("attachment_type")?,
        consumption_scope: r.try_get("consumption_scope")?,
        owner_id: r.try_get("owner_id")?,
        stop_processing: r.try_get("stop_processing")?,
    })
}

/// `account.rules.order_by("order")` (by id on equal order – undefined upstream).
pub async fn rules_of(pool: &PgPool, account_id: i32) -> Result<Vec<Rule>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT {RULE_COLS} FROM paperless_mail_mailrule r JOIN paperless_mail_mailaccount a ON a.id = r.account_id \
         WHERE r.account_id = $1 ORDER BY r.\"order\", r.id"
    ))
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(rule_from).collect()
}

pub async fn rule(pool: &PgPool, id: i32) -> Result<Option<Rule>, sqlx::Error> {
    let r = sqlx::query(&format!(
        "SELECT {RULE_COLS} FROM paperless_mail_mailrule r JOIN paperless_mail_mailaccount a ON a.id = r.account_id WHERE r.id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    r.as_ref().map(rule_from).transpose()
}

/// `[tag.id for tag in rule.assign_tags.all()]` (tag ordering by name).
pub async fn rule_tag_ids(pool: &PgPool, rule_id: i32) -> Result<Vec<i64>, sqlx::Error> {
    let rows: Vec<(i32,)> = sqlx::query_as(
        "SELECT t.id FROM paperless_mail_mailrule_assign_tags rt JOIN documents_tag t ON t.id = rt.tag_id \
         WHERE rt.mailrule_id = $1 ORDER BY t.name, t.id",
    )
    .bind(rule_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(i,)| i64::from(i)).collect())
}

/// UIDs from `uid_chunk` that are already processed (with UIDVALIDITY filter).
pub async fn processed_uids(pool: &PgPool, rule_id: i32, folder: &str, uids: &[String], uid_validity: Option<&str>) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = match uid_validity {
        Some(v) => {
            sqlx::query_as(
                "SELECT uid FROM paperless_mail_processedmail WHERE rule_id = $1 AND folder = $2 AND uid = ANY($3) \
                 AND (uid_validity = $4 OR uid_validity IS NULL)",
            )
            .bind(rule_id)
            .bind(folder)
            .bind(uids)
            .bind(v)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as("SELECT uid FROM paperless_mail_processedmail WHERE rule_id = $1 AND folder = $2 AND uid = ANY($3)")
                .bind(rule_id)
                .bind(folder)
                .bind(uids)
                .fetch_all(pool)
                .await?
        }
    };
    Ok(rows.into_iter().map(|(u,)| u).collect())
}

/// A ProcessedMail entry.
#[derive(Debug, Clone)]
pub struct NewProcessed<'a> {
    pub owner_id: Option<i32>,
    pub rule_id: i32,
    pub folder: &'a str,
    pub uid: &'a str,
    pub uid_validity: Option<&'a str>,
    pub subject: &'a str,
    pub received: DateTime<FixedOffset>,
    pub status: &'a str,
    pub error: Option<&'a str>,
}

/// `ProcessedMail.objects.create(...)` (`processed = timezone.now()`).
pub async fn create_processed(pool: &PgPool, p: &NewProcessed<'_>) -> Result<i32, sqlx::Error> {
    let (id,): (i32,) = sqlx::query_as(
        "INSERT INTO paperless_mail_processedmail (owner_id, rule_id, folder, uid, uid_validity, subject, received, processed, status, error) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(p.owner_id)
    .bind(p.rule_id)
    .bind(p.folder)
    .bind(p.uid)
    .bind(p.uid_validity)
    .bind(super::message::py_truncate(p.subject, 256))
    .bind(p.received)
    .bind(now_micros())
    .bind(p.status)
    .bind(p.error)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// `ProcessedMail.objects.get_or_create(rule, folder, uid, uid_validity, defaults=…)`.
/// `MultipleObjectsReturned` → error.
pub async fn get_or_create_processed(pool: &PgPool, p: &NewProcessed<'_>) -> Result<(), sqlx::Error> {
    let n: i64 = match p.uid_validity {
        Some(v) => {
            sqlx::query_scalar(
                "SELECT count(*) FROM (SELECT 1 FROM paperless_mail_processedmail WHERE rule_id = $1 AND folder = $2 AND uid = $3 \
                 AND uid_validity = $4 LIMIT 21) s",
            )
            .bind(p.rule_id)
            .bind(p.folder)
            .bind(p.uid)
            .bind(v)
            .fetch_one(pool)
            .await?
        }
        None => {
            sqlx::query_scalar(
                "SELECT count(*) FROM (SELECT 1 FROM paperless_mail_processedmail WHERE rule_id = $1 AND folder = $2 AND uid = $3 \
                 AND uid_validity IS NULL LIMIT 21) s",
            )
            .bind(p.rule_id)
            .bind(p.folder)
            .bind(p.uid)
            .fetch_one(pool)
            .await?
        }
    };
    if n > 1 {
        return Err(sqlx::Error::Protocol(format!("get() returned more than one ProcessedMail -- it returned {n}!")));
    }
    if n == 0 {
        create_processed(pool, p).await?;
    }
    Ok(())
}

/// `timezone.now()` with microseconds like Postgres.
pub fn now_micros() -> DateTime<Utc> {
    pl_consumer::db::now_micros()
}

/// `Correspondent.objects.get_or_create(name=name, defaults={"match": name, "matching_algorithm": MATCH_LITERAL})`.
/// `Ok(None)` = `DatabaseError` (logged); `Err` = other exception (MultipleObjectsReturned).
pub async fn correspondent_from_name(pool: &PgPool, name: &str, audit_enabled: bool) -> Result<Option<i64>, String> {
    let found: Result<Vec<(i32,)>, sqlx::Error> =
        sqlx::query_as("SELECT id FROM documents_correspondent WHERE name = $1 LIMIT 21").bind(name).fetch_all(pool).await;
    match found {
        Err(e) => {
            tracing::error!(target: "paperless_mail", "Error while retrieving correspondent {name}: {e}");
            return Ok(None);
        }
        Ok(v) if v.len() > 1 => {
            return Err(format!("get() returned more than one Correspondent -- it returned {}!", v.len()));
        }
        Ok(v) if v.len() == 1 => return Ok(Some(i64::from(v[0].0))),
        Ok(_) => {}
    }
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(target: "paperless_mail", "Error while retrieving correspondent {name}: {e}");
            return Ok(None);
        }
    };
    let ins: Result<(i32,), sqlx::Error> = sqlx::query_as(
        "INSERT INTO documents_correspondent (name, match, matching_algorithm, is_insensitive, owner_id) \
         VALUES ($1, $1, 3, true, NULL) RETURNING id",
    )
    .bind(name)
    .fetch_one(&mut *tx)
    .await;
    let id = match ins {
        Ok((id,)) => id,
        Err(e) => {
            let _ = tx.rollback().await;
            // IntegrityError → read again (get_or_create); otherwise DatabaseError
            if let Ok(Some((id,))) = sqlx::query_as::<_, (i32,)>("SELECT id FROM documents_correspondent WHERE name = $1 LIMIT 1")
                .bind(name)
                .fetch_optional(pool)
                .await
            {
                return Ok(Some(i64::from(id)));
            }
            tracing::error!(target: "paperless_mail", "Error while retrieving correspondent {name}: {e}");
            return Ok(None);
        }
    };
    if audit_enabled {
        let mut changes = Map::new();
        changes.insert("id".into(), json!(["None", id.to_string()]));
        changes.insert("name".into(), json!(["None", name]));
        changes.insert("match".into(), json!(["None", name]));
        changes.insert("owner".into(), json!(["None", "None"]));
        changes.insert("documents".into(), json!(["None", "documents.Document.None"]));
        changes.insert("is_insensitive".into(), json!(["None", "True"]));
        changes.insert("matching_algorithm".into(), json!(["None", "3"]));
        let audit = pl_consumer::audit::AuditLog { enabled: true, actor_id: None };
        if let Err(e) = audit.write(&mut tx, "correspondent", i64::from(id), name, pl_consumer::audit::ACTION_CREATE, changes, None).await {
            let _ = tx.rollback().await;
            tracing::error!(target: "paperless_mail", "Error while retrieving correspondent {name}: {e}");
            return Ok(None);
        }
    }
    if let Err(e) = tx.commit().await {
        tracing::error!(target: "paperless_mail", "Error while retrieving correspondent {name}: {e}");
        return Ok(None);
    }
    let _ = Value::Null;
    Ok(Some(i64::from(id)))
}

/// Re-read the account after OAuth refresh (`account.refresh_from_db()`).
pub async fn reload_account(pool: &PgPool, acc: &mut Account) -> Result<(), sqlx::Error> {
    if let Some(a) = account(pool, acc.id).await? {
        *acc = a;
    }
    Ok(())
}
