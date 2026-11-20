//! Conversation history of `/ask` per user (`paperrust_ask_conversation`, `paperrust_ask_message`).

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::store::ensure;
use crate::{Result, TimelineError};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conversation {
    #[serde(skip)]
    pub id: i64,
    pub uuid: Uuid,
    pub title: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub message_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Message {
    pub uuid: Uuid,
    pub role: String,
    pub content: String,
    pub citations: Value,
    pub actions: Value,
    pub meta: Value,
    pub created: DateTime<Utc>,
}

/// Title from the first question (at most 80 characters).
pub fn title_of(question: &str) -> String {
    let flat = question.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 80 {
        flat
    } else {
        format!("{}…", flat.chars().take(79).collect::<String>())
    }
}

/// Fetch own conversation (`Some(uuid)`, otherwise `NotFound`) or create a new one. → (id, uuid)
pub async fn open(
    pool: &PgPool,
    user_id: i64,
    conversation: Option<Uuid>,
    question: &str,
) -> Result<(i64, Uuid)> {
    ensure(pool).await?;
    if let Some(u) = conversation {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM paperrust_ask_conversation WHERE uuid = $1 AND user_id = $2",
        )
        .bind(u)
        .bind(user_id as i32)
        .fetch_optional(pool)
        .await?;
        return id.map(|i| (i, u)).ok_or(TimelineError::NotFound);
    }
    let (id, u): (i64, Uuid) = sqlx::query_as("INSERT INTO paperrust_ask_conversation (user_id, title) VALUES ($1, $2) RETURNING id, uuid")
        .bind(user_id as i32)
        .bind(title_of(question))
        .fetch_one(pool)
        .await?;
    Ok((id, u))
}

/// Append message (updates `updated`); `uuid` given (e.g. already announced in the
/// stream) or new. → (uuid, created)
#[allow(clippy::too_many_arguments)]
pub async fn add(
    pool: &PgPool,
    conv: i64,
    uuid: Option<Uuid>,
    role: &str,
    content: &str,
    citations: &Value,
    actions: &Value,
    meta: &Value,
) -> Result<(Uuid, DateTime<Utc>)> {
    let mut tx = pool.begin().await?;
    let r: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO paperrust_ask_message (conversation_id, uuid, role, content, citations, actions, meta)
         VALUES ($1, COALESCE($2, gen_random_uuid()), $3, $4, $5, $6, $7) RETURNING uuid, created",
    )
    .bind(conv)
    .bind(uuid)
    .bind(role)
    .bind(content)
    .bind(citations)
    .bind(actions)
    .bind(meta)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE paperrust_ask_conversation SET updated = now() WHERE id = $1")
        .bind(conv)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(r)
}

fn conv_from(r: &sqlx::postgres::PgRow) -> Result<Conversation> {
    Ok(Conversation {
        id: r.try_get("id")?,
        uuid: r.try_get("uuid")?,
        title: r.try_get("title")?,
        created: r.try_get("created")?,
        updated: r.try_get("updated")?,
        message_count: r.try_get("message_count")?,
    })
}

const CONV_COLS: &str = "c.id, c.uuid, c.title, c.created, c.updated, (SELECT count(*) FROM paperrust_ask_message m WHERE m.conversation_id = c.id) AS message_count";

fn cursor_of(c: &Conversation) -> String {
    format!("{}.{}", c.updated.timestamp_micros(), c.id)
}

fn parse_cursor(s: &str) -> Option<(DateTime<Utc>, i64)> {
    let (t, i) = s.split_once('.')?;
    Some((
        DateTime::from_timestamp_micros(t.parse().ok()?)?,
        i.parse().ok()?,
    ))
}

/// Own conversations, most recently active first. → (page, next cursor)
pub async fn list(
    pool: &PgPool,
    user_id: i64,
    limit: i64,
    cursor: Option<&str>,
) -> Result<(Vec<Conversation>, Option<String>)> {
    ensure(pool).await?;
    let after = match cursor {
        Some(c) => {
            Some(parse_cursor(c).ok_or_else(|| crate::invalid("cursor", "Invalid cursor."))?)
        }
        None => None,
    };
    let rows = sqlx::query(&format!(
        "SELECT {CONV_COLS} FROM paperrust_ask_conversation c WHERE c.user_id = $1
            AND ($2::timestamptz IS NULL OR (date_trunc('microseconds', c.updated), c.id) < ($2, $3))
          ORDER BY c.updated DESC, c.id DESC LIMIT $4"
    ))
    .bind(user_id as i32)
    .bind(after.map(|a| a.0))
    .bind(after.map(|a| a.1).unwrap_or(0))
    .bind(limit + 1)
    .fetch_all(pool)
    .await?;
    let mut items: Vec<Conversation> = rows.iter().map(conv_from).collect::<Result<_>>()?;
    let next = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(cursor_of)
    } else {
        None
    };
    Ok((items, next))
}

/// Own conversation with messages.
pub async fn get(
    pool: &PgPool,
    user_id: i64,
    uuid: Uuid,
) -> Result<Option<(Conversation, Vec<Message>)>> {
    ensure(pool).await?;
    let Some(r) = sqlx::query(&format!(
        "SELECT {CONV_COLS} FROM paperrust_ask_conversation c WHERE c.uuid = $1 AND c.user_id = $2"
    ))
    .bind(uuid)
    .bind(user_id as i32)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let c = conv_from(&r)?;
    let msgs = sqlx::query("SELECT uuid, role, content, citations, actions, meta, created FROM paperrust_ask_message WHERE conversation_id = $1 ORDER BY id")
        .bind(c.id)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|m| {
            Ok(Message {
                uuid: m.try_get("uuid")?,
                role: m.try_get("role")?,
                content: m.try_get("content")?,
                citations: m.try_get("citations")?,
                actions: m.try_get("actions")?,
                meta: m.try_get("meta")?,
                created: m.try_get("created")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some((c, msgs)))
}

/// Delete own conversation (`false` = not found).
pub async fn delete(pool: &PgPool, user_id: i64, uuid: Uuid) -> Result<bool> {
    ensure(pool).await?;
    let n = sqlx::query("DELETE FROM paperrust_ask_conversation WHERE uuid = $1 AND user_id = $2")
        .bind(uuid)
        .bind(user_id as i32)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// Delete all own conversations.
pub async fn delete_all(pool: &PgPool, user_id: i64) -> Result<u64> {
    ensure(pool).await?;
    Ok(
        sqlx::query("DELETE FROM paperrust_ask_conversation WHERE user_id = $1")
            .bind(user_id as i32)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_cursor() {
        assert_eq!(
            title_of("  Wann  ist\n die Rechnung fällig? "),
            "Wann ist die Rechnung fällig?"
        );
        let long = "x".repeat(200);
        assert_eq!(title_of(&long).chars().count(), 80);
        let c = Conversation {
            id: 7,
            uuid: Uuid::nil(),
            title: "t".into(),
            created: Utc::now(),
            updated: Utc::now(),
            message_count: 0,
        };
        let (t, i) = parse_cursor(&cursor_of(&c)).unwrap();
        assert_eq!((t.timestamp_micros(), i), (c.updated.timestamp_micros(), 7));
        assert!(parse_cursor("nope").is_none());
    }
}
