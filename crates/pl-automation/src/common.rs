//! Shared: sign-in, JSON body, permissions, resolving names, visible documents.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::FromRequestParts;
use axum::http::header::CONTENT_TYPE;
use axum::http::request::Parts;
use axum::http::HeaderMap;
use pl_authz::{MaybeUser, User};
use sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder as _;
use serde_json::{Map, Value};
use sqlx::{PgConnection, Row};

use crate::error::{AErr, AResult};

/// Signed-in, active user (session+CSRF, token, device token – like v1/v2 via pl-authz).
pub struct Who(pub Arc<User>);

impl<S: Send + Sync> FromRequestParts<S> for Who {
    type Rejection = AErr;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let m = MaybeUser::from_request_parts(parts, state).await.map_err(AErr::from)?;
        match m.0.user.clone() {
            Some(u) if u.is_active => Ok(Who(u)),
            Some(_) => Err(AErr::Forbidden("User inactive or deleted.".into())),
            None => Err(AErr::Unauthenticated),
        }
    }
}

impl Who {
    pub fn user(&self) -> &User {
        &self.0
    }
    /// All permissions (`app.codename`), otherwise 403.
    pub fn require(&self, perms: &[&str]) -> AResult<&User> {
        if self.0.has_perms(perms) { Ok(&self.0) } else { Err(AErr::forbidden()) }
    }
    /// At least one of the permissions, otherwise 403.
    pub fn require_any(&self, perms: &[&str]) -> AResult<&User> {
        if perms.iter().any(|p| self.0.has_perm(p)) { Ok(&self.0) } else { Err(AErr::forbidden()) }
    }
}

pub const VIEW_WORKFLOW: &str = "documents.view_workflow";
pub const ADD_WORKFLOW: &str = "documents.add_workflow";
pub const CHANGE_WORKFLOW: &str = "documents.change_workflow";
pub const VIEW_DOCUMENT: &str = "documents.view_document";
pub const CHANGE_DOCUMENT: &str = "documents.change_document";

/// Largest accepted JSON body (1 MiB, like v2).
const MAX_JSON: usize = 1024 * 1024;

/// JSON object from the body (`Content-Type: application/json`; empty → `{}`).
pub fn body_object(headers: &HeaderMap, body: &Bytes) -> AResult<Map<String, Value>> {
    if body.is_empty() {
        return Ok(Map::new());
    }
    let ct = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if !(main == "application/json" || main.ends_with("+json")) {
        return Err(AErr::invalid("Content-Type must be application/json."));
    }
    if body.len() > MAX_JSON {
        return Err(AErr::invalid("Request body too large."));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(AErr::invalid("Expected a JSON object.")),
        Err(e) => Err(AErr::invalid(format!("Invalid JSON: {e}"))),
    }
}

/// Integer from a path segment (otherwise 404).
pub fn path_id(s: &str) -> AResult<i64> {
    s.parse::<i64>().ok().filter(|v| (1..=i64::from(i32::MAX)).contains(v)).ok_or(AErr::NotFound)
}

/// Optional integer of a field (error with field names).
pub fn opt_int(m: &Map<String, Value>, k: &str) -> AResult<Option<i64>> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| AErr::field(k, "A valid integer is required.")),
    }
}

/// Optional truth value of a field.
pub fn opt_bool(m: &Map<String, Value>, k: &str) -> AResult<Option<bool>> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(AErr::field(k, "Must be a valid boolean.")),
    }
}

/// List of integers of a field.
pub fn int_list(m: &Map<String, Value>, k: &str) -> AResult<Option<Vec<i64>>> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_i64().ok_or_else(|| AErr::field(k, "A valid integer is required.")))
            .collect::<AResult<Vec<i64>>>()
            .map(Some),
        Some(_) => Err(AErr::field(k, "Expected a list of items.")),
    }
}

/// May `user` do `perm` (`view_document`/`change_document`) on these documents – the ids they may.
pub async fn permitted_docs(conn: &mut PgConnection, user: &User, perm: &str, ids: &[i64]) -> AResult<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sub = pl_authz::objects::permitted_document_ids(Some(user), perm, false);
    let (sql, values) = Query::select()
        .column(Alias::new("id"))
        .from(Alias::new("documents_document"))
        .and_where(Expr::col(Alias::new("id")).is_in(ids.iter().filter_map(|i| i32::try_from(*i).ok())))
        .and_where(Expr::col(Alias::new("id")).in_subquery(sub))
        .build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(&mut *conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(i64::from(r.try_get::<i32, _>(0)?));
    }
    Ok(out)
}

/// Do all (non-deleted) documents or their roots belong to `user` or to nobody
/// (like pl-documents-write `perms::owns_all_roots`)?
pub async fn owns_all_docs(conn: &mut PgConnection, user: &User, ids: &[i64]) -> AResult<bool> {
    let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let foreign: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM documents_document d          LEFT JOIN documents_document r ON r.id = d.root_document_id          WHERE d.id = ANY($1) AND d.deleted_at IS NULL          AND (CASE WHEN d.root_document_id IS NULL THEN d.owner_id ELSE r.owner_id END) IS NOT NULL          AND (CASE WHEN d.root_document_id IS NULL THEN d.owner_id ELSE r.owner_id END) <> $2)",
    )
    .bind(ids32)
    .bind(i32::try_from(user.id).unwrap_or(i32::MAX))
    .fetch_one(&mut *conn)
    .await?;
    Ok(!foreign)
}

/// The last `limit` visible root documents (not in the trash), newest first
/// (`added DESC, id DESC`).
pub async fn latest_visible_docs(conn: &mut PgConnection, user: &User, limit: u64) -> AResult<Vec<i64>> {
    let sub = pl_authz::objects::permitted_document_ids(Some(user), "view_document", false);
    let (sql, values) = Query::select()
        .column(Alias::new("id"))
        .from(Alias::new("documents_document"))
        .and_where(Expr::col(Alias::new("root_document_id")).is_null())
        .and_where(Expr::col(Alias::new("deleted_at")).is_null())
        .and_where(Expr::col(Alias::new("id")).in_subquery(sub))
        .order_by(Alias::new("added"), Order::Desc)
        .order_by(Alias::new("id"), Order::Desc)
        .limit(limit)
        .build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(&mut *conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(i64::from(r.try_get::<i32, _>(0)?));
    }
    Ok(out)
}

/// Object kinds whose names the summary resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameKind {
    Tag,
    Correspondent,
    DocumentType,
    StoragePath,
    CustomField,
    User,
    Group,
    MailRule,
}

impl NameKind {
    fn sql(self) -> (&'static str, &'static str) {
        match self {
            NameKind::Tag => ("documents_tag", "name"),
            NameKind::Correspondent => ("documents_correspondent", "name"),
            NameKind::DocumentType => ("documents_documenttype", "name"),
            NameKind::StoragePath => ("documents_storagepath", "name"),
            NameKind::CustomField => ("documents_customfield", "name"),
            NameKind::User => ("auth_user", "username"),
            NameKind::Group => ("auth_group", "name"),
            NameKind::MailRule => ("paperless_mail_mailrule", "name"),
        }
    }
    /// Ownable model (visibility via pl-authz) or model permission.
    fn visibility(self) -> Visibility {
        match self {
            NameKind::Tag => Visibility::Owned(&pl_authz::models::TAG, "view_tag"),
            NameKind::Correspondent => Visibility::Owned(&pl_authz::models::CORRESPONDENT, "view_correspondent"),
            NameKind::DocumentType => Visibility::Owned(&pl_authz::models::DOCUMENT_TYPE, "view_documenttype"),
            NameKind::StoragePath => Visibility::Owned(&pl_authz::models::STORAGE_PATH, "view_storagepath"),
            NameKind::CustomField => Visibility::Perm("documents.view_customfield"),
            NameKind::User => Visibility::Perm("auth.view_user"),
            NameKind::Group => Visibility::Perm("auth.view_group"),
            NameKind::MailRule => Visibility::Perm("paperless_mail.view_mailrule"),
        }
    }
}

enum Visibility {
    Owned(&'static pl_authz::ObjectModel, &'static str),
    Perm(&'static str),
}

/// Names of collected ids, only for objects `user` may see (otherwise `None` → id only).
#[derive(Default)]
pub struct Names {
    want: HashMap<NameKind, Vec<i64>>,
    got: HashMap<(NameKind, i64), String>,
}

impl Names {
    pub fn want(&mut self, k: NameKind, id: i64) {
        self.want.entry(k).or_default().push(id);
    }
    pub fn get(&self, k: NameKind, id: i64) -> Option<&str> {
        self.got.get(&(k, id)).map(String::as_str)
    }
    /// One query per kind.
    pub async fn resolve(&mut self, conn: &mut PgConnection, user: &User) -> AResult<()> {
        let want = std::mem::take(&mut self.want);
        for (k, mut ids) in want {
            ids.sort_unstable();
            ids.dedup();
            let (table, col) = k.sql();
            let mut q = Query::select();
            q.column(Alias::new("id"))
                .column(Alias::new(col))
                .from(Alias::new(table))
                .and_where(Expr::col(Alias::new("id")).is_in(ids.iter().filter_map(|i| i32::try_from(*i).ok())));
            match k.visibility() {
                Visibility::Owned(model, perm) => {
                    if !pl_authz::objects::user_is_active_superuser(Some(user)) {
                        let sub = pl_authz::objects::permitted_object_ids(Some(user), model, perm, &Default::default());
                        q.and_where(Expr::col(Alias::new("id")).in_subquery(sub));
                    }
                }
                Visibility::Perm(p) => {
                    if !user.has_perm(p) {
                        continue;
                    }
                }
            }
            let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
            for r in sqlx::query_with(&sql, values).fetch_all(&mut *conn).await? {
                self.got.insert((k, i64::from(r.try_get::<i32, _>(0)?)), r.try_get(1)?);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_lists() {
        assert_eq!(path_id("5").unwrap(), 5);
        assert!(path_id("x").is_err());
        assert!(path_id("0").is_err());
        assert!(path_id("99999999999").is_err());
        let m: Map<String, Value> = serde_json::from_value(serde_json::json!({"a": [1, 2], "b": "x", "c": true})).unwrap();
        assert_eq!(int_list(&m, "a").unwrap(), Some(vec![1, 2]));
        assert!(int_list(&m, "b").is_err());
        assert_eq!(opt_bool(&m, "c").unwrap(), Some(true));
        assert_eq!(opt_int(&m, "zz").unwrap(), None);
    }

    #[test]
    fn body_rules() {
        let mut h = HeaderMap::new();
        assert!(body_object(&h, &Bytes::new()).unwrap().is_empty());
        assert!(body_object(&h, &Bytes::from_static(b"{}")).is_err());
        h.insert(CONTENT_TYPE, "application/json".parse().unwrap());
        assert!(body_object(&h, &Bytes::from_static(b"[1]")).is_err());
        assert!(body_object(&h, &Bytes::from_static(b"{\"a\":1}")).is_ok());
    }
}
