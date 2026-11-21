//! PostgreSQL storage for aliases, AI audit and MCP scopes (`paperrust_taxonomy_alias`,
//! `paperrust_ai_audit`, migration `migrations/0001_paperrust_ai.sql`; `paperrust_ai_mcp_scope`,
//! migration `migrations/0002_paperrust_ai_mcp_scope.sql`).
//!
//! As long as the migration is not applied (phase 1: no own tables):
//! read aliases → empty; store alias → error "alias storage not installed";
//! audit → entry in the log (`target = "pl_ai::audit"`, JSON), no error.
//!
//! SQL: simple parameterized statements (all values bound, nothing concatenated); sea-query
//! brings no advantage here (own tables, no dynamic filter).

use futures::future::BoxFuture;
use sqlx::{PgPool, Row as _};

use crate::mcp::api::SideStore;
use crate::mcp::backend::{AuditEntry, BResult, BackendError};
use crate::taxonomy::{Alias, TaxonomyKind};

/// Storage in PostgreSQL.
pub struct PgSideStore {
    pool: PgPool,
}

impl PgSideStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn undefined_table(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("42P01"))
}

fn kind_from(s: &str) -> Option<TaxonomyKind> {
    TaxonomyKind::parse(s)
}

impl SideStore for PgSideStore {
    fn aliases(&self, kind: TaxonomyKind) -> BoxFuture<'_, BResult<Vec<Alias>>> {
        Box::pin(async move {
            let r = sqlx::query("SELECT kind, target_id, alias, alias_key FROM paperrust_taxonomy_alias WHERE kind = $1 ORDER BY id")
                .bind(kind.as_str())
                .fetch_all(&self.pool)
                .await;
            match r {
                Ok(rows) => Ok(rows
                    .iter()
                    .filter_map(|row| {
                        Some(Alias {
                            kind: kind_from(row.try_get::<String, _>("kind").ok()?.as_str())?,
                            target_id: i64::from(row.try_get::<i32, _>("target_id").ok()?),
                            alias: row.try_get("alias").ok()?,
                            alias_key: row.try_get("alias_key").ok()?,
                            owner_id: None,
                        })
                    })
                    .collect()),
                Err(e) if undefined_table(&e) => Ok(Vec::new()),
                Err(e) => {
                    tracing::error!(target: "pl_ai", error = %e, "alias query failed");
                    Err(BackendError::Internal)
                }
            }
        })
    }

    fn add_alias(&self, alias: Alias, actor_id: Option<i64>) -> BoxFuture<'_, BResult<bool>> {
        Box::pin(async move {
            let target = i32::try_from(alias.target_id).map_err(|_| BackendError::Invalid("target_id out of range".into()))?;
            let actor = actor_id.and_then(|a| i32::try_from(a).ok());
            let r = sqlx::query(
                "INSERT INTO paperrust_taxonomy_alias (kind, target_id, alias, alias_key, source, created_by_id) \
                 VALUES ($1, $2, $3, $4, 'mcp', $5) ON CONFLICT (kind, alias_key, target_id) DO NOTHING",
            )
            .bind(alias.kind.as_str())
            .bind(target)
            .bind(&alias.alias)
            .bind(&alias.alias_key)
            .bind(actor)
            .execute(&self.pool)
            .await;
            match r {
                Ok(done) => Ok(done.rows_affected() > 0),
                Err(e) if undefined_table(&e) => Err(BackendError::Invalid(
                    "alias storage not installed (migration crates/pl-ai/migrations/0001_paperrust_ai.sql pending)".into(),
                )),
                Err(e) => {
                    tracing::error!(target: "pl_ai", error = %e, "alias insert failed");
                    Err(BackendError::Internal)
                }
            }
        })
    }

    fn audit(&self, e: AuditEntry) -> BoxFuture<'_, BResult<()>> {
        Box::pin(async move {
            let run: Option<uuid::Uuid> = e.run_id.as_deref().and_then(|r| uuid::Uuid::parse_str(r).ok());
            let r = sqlx::query(
                "INSERT INTO paperrust_ai_audit (actor_id, channel, token_hint, run_id, action, object_kind, object_id, \
                 forced, reason, candidates, before, after, confidence) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            )
            .bind(e.actor_id.and_then(|a| i32::try_from(a).ok()))
            .bind(e.channel)
            .bind(&e.token_hint)
            .bind(run)
            .bind(e.action)
            .bind(&e.object_kind)
            .bind(e.object_id.and_then(|a| i32::try_from(a).ok()))
            .bind(e.forced)
            .bind(&e.reason)
            .bind(&e.candidates)
            .bind(&e.before)
            .bind(&e.after)
            .bind(e.confidence)
            .execute(&self.pool)
            .await;
            match r {
                Ok(_) => Ok(()),
                Err(err) => {
                    // Fallback: audit into the log (never lose it).
                    tracing::warn!(
                        target: "pl_ai::audit",
                        entry = %serde_json::to_string(&e).unwrap_or_default(),
                        table_missing = undefined_table(&err),
                        "ai audit (db write failed, logged instead)"
                    );
                    Ok(())
                }
            }
        })
    }

    fn token_scopes(&self, token_key: &str, user_id: Option<i64>) -> BoxFuture<'_, BResult<Option<Vec<String>>>> {
        let token_key = token_key.to_string();
        Box::pin(async move {
            let r = sqlx::query(
                "SELECT scopes FROM paperrust_ai_mcp_scope                  WHERE token_key = $1 OR (token_key IS NULL AND user_id = $2)                  ORDER BY (token_key IS NOT NULL) DESC LIMIT 1",
            )
            .bind(&token_key)
            .bind(user_id.and_then(|u| i32::try_from(u).ok()))
            .fetch_optional(&self.pool)
            .await;
            match r {
                Ok(Some(row)) => {
                    let s: String = row.try_get("scopes").map_err(|_| BackendError::Internal)?;
                    Ok(Some(split_scopes(&s)))
                }
                Ok(None) => Ok(None),
                // Table missing = nobody has assigned scopes → all enabled groups.
                Err(e) if undefined_table(&e) => Ok(None),
                Err(e) => {
                    // fail-closed: error → tool call fails
                    tracing::error!(target: "pl_ai", error = %e, "mcp scope query failed");
                    Err(BackendError::Internal)
                }
            }
        })
    }
}

fn split_scopes(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect()
}

/// A scope entry (managed via `/api/x-ai/mcp-scopes/`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ScopeRow {
    pub id: i64,
    /// First 8 characters of the token hash (recognition, not a secret).
    pub token_key_prefix: Option<String>,
    pub user_id: Option<i64>,
    pub scopes: Vec<String>,
    pub label: Option<String>,
    pub created: String,
}

impl PgSideStore {
    /// All scope entries.
    pub async fn list_scopes(&self) -> Result<Vec<ScopeRow>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, token_key, user_id, scopes, label, to_char(created AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS c              FROM paperrust_ai_mcp_scope ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| ScopeRow {
                id: r.try_get("id").unwrap_or_default(),
                token_key_prefix: r.try_get::<Option<String>, _>("token_key").ok().flatten().map(|k| k.chars().take(8).collect()),
                user_id: r.try_get::<Option<i32>, _>("user_id").ok().flatten().map(i64::from),
                scopes: split_scopes(&r.try_get::<String, _>("scopes").unwrap_or_default()),
                label: r.try_get("label").ok().flatten(),
                created: r.try_get("c").unwrap_or_default(),
            })
            .collect())
    }

    /// Creates an entry or replaces it (per token or per user). `scopes` must have been validated.
    pub async fn upsert_scope(
        &self,
        token_key: Option<&str>,
        user_id: Option<i64>,
        scopes: &[&str],
        label: Option<&str>,
        by: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        let list = scopes.join(",");
        let by = by.and_then(|b| i32::try_from(b).ok());
        let row = match token_key {
            Some(k) => {
                sqlx::query(
                    "INSERT INTO paperrust_ai_mcp_scope (token_key, scopes, label, created_by_id) VALUES ($1, $2, $3, $4)                      ON CONFLICT (token_key) WHERE token_key IS NOT NULL DO UPDATE SET scopes = excluded.scopes, label = excluded.label                      RETURNING id",
                )
                .bind(k)
                .bind(&list)
                .bind(label)
                .bind(by)
                .fetch_one(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "INSERT INTO paperrust_ai_mcp_scope (user_id, scopes, label, created_by_id) VALUES ($1, $2, $3, $4)                      ON CONFLICT (user_id) WHERE token_key IS NULL DO UPDATE SET scopes = excluded.scopes, label = excluded.label                      RETURNING id",
                )
                .bind(user_id.and_then(|u| i32::try_from(u).ok()))
                .bind(&list)
                .bind(label)
                .bind(by)
                .fetch_one(&self.pool)
                .await?
            }
        };
        row.try_get("id")
    }

    /// Removes an entry; `false` = not present.
    pub async fn delete_scope(&self, id: i64) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query("DELETE FROM paperrust_ai_mcp_scope WHERE id = $1").bind(id).execute(&self.pool).await?.rows_affected() > 0)
    }

    /// Checks whether the audit table has an entry (tests/diagnostics): count per action.
    pub async fn audit_count(&self, action: &str, actor_id: Option<i64>) -> Result<i64, sqlx::Error> {
        let r = sqlx::query("SELECT count(*)::bigint AS n FROM paperrust_ai_audit WHERE action = $1 AND actor_id IS NOT DISTINCT FROM $2 AND channel = 'mcp'")
            .bind(action)
            .bind(actor_id.and_then(|u| i32::try_from(u).ok()))
            .fetch_one(&self.pool)
            .await?;
        r.try_get("n")
    }
}
