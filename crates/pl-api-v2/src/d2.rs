//! Shared parts of the PaperRust UI (design 2): user, visibility, errors, "today".

use chrono::{NaiveDate, Utc};
use pl_authz::User;
use pl_core::AppState;
use pl_domain::{DomainError, Kind, Principal};
use pl_timeline::TimelineError;
use pl_timeline::store::Visibility;

use crate::error::{V2Error, V2Result};

pub const PERM_VIEW_DOCUMENT: &str = "documents.view_document";

/// Logged-in user (from the login context).
pub fn user(p: &Principal) -> V2Result<&User> {
    p.ext
        .downcast_ref::<pl_store::AuthExt>()
        .and_then(|e| e.ctx.user())
        .ok_or(V2Error(DomainError::Unauthenticated))
}

/// Active user with `documents.view_document` (like the document list).
pub fn require_view_documents(p: &Principal) -> V2Result<&User> {
    let u = user(p)?;
    if !(u.is_active && u.has_perm(PERM_VIEW_DOCUMENT)) {
        return Err(V2Error(DomainError::Forbidden(
            "You do not have permission to perform this action.".into(),
        )));
    }
    Ok(u)
}

/// Visibility for pl-timeline (subqueries from pl-store).
pub fn visibility(p: &Principal) -> V2Result<Visibility> {
    let u = user(p)?;
    Ok(Visibility {
        documents: pl_store::visible_ids_sql(p, Kind::Document)?,
        tags: pl_store::visible_ids_sql(p, Kind::Tag)?,
        correspondents: pl_store::visible_ids_sql(p, Kind::Correspondent)?,
        document_types: pl_store::visible_ids_sql(p, Kind::DocumentType)?,
        storage_paths: pl_store::visible_ids_sql(p, Kind::StoragePath)?,
        custom_field_names: u.is_active && u.has_perm("documents.view_customfield"),
    })
}

/// pl-timeline error → v2.
pub fn tl(e: TimelineError) -> V2Error {
    match e {
        TimelineError::Invalid(f, m) => V2Error(DomainError::field(&f, m)),
        TimelineError::NotFound => V2Error(DomainError::NotFound),
        TimelineError::Forbidden(m) => V2Error(DomainError::Forbidden(m)),
        TimelineError::Db(e) => V2Error(DomainError::internal(e)),
    }
}

/// Create the page tables (once per process).
pub async fn ensure(state: &AppState) -> V2Result<()> {
    pl_timeline::store::ensure(state.db()).await.map_err(tl)
}

/// Today in `PAPERLESS_TIME_ZONE`.
pub fn today(state: &AppState) -> NaiveDate {
    Utc::now().with_timezone(&state.config().tz).date_naive()
}

/// Read a setting (environment + UI, live).
pub fn setting(key: &str) -> Option<String> {
    pl_core::config::var(key)
}

/// `YYYY-MM-DD`.
pub fn parse_date(field: &str, v: &str) -> V2Result<NaiveDate> {
    NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d")
        .map_err(|_| V2Error(DomainError::field(field, "Expected a date YYYY-MM-DD.")))
}

/// Integer in range.
pub fn parse_int(
    field: &str,
    v: Option<&String>,
    default: i64,
    min: i64,
    max: i64,
) -> V2Result<i64> {
    let Some(v) = v else { return Ok(default) };
    match v.trim().parse::<i64>() {
        Ok(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(V2Error(DomainError::field(
            field,
            format!("Expected an integer {min}–{max}."),
        ))),
    }
}
