//! Write paths via the v1 handlers (transition, ARCHITECTURE-V2 §1: reuse of parity-checked
//! logic only **behind** the core interface).
//!
//! `pl-server` passes a [`LegacyRouter`] with the v1 routes (`tags.create`,
//! `documents.partial_update`, …). A v2 write access is translated here into an internal v1 request with
//! the **same login context** (no second login, CSRF has already been checked);
//! validation, permissions, audit log, workflows, index and file names thus remain identical to v1.
//! The v1 response is evaluated only for success/error, the result is re-read from the DB.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt as _;
use pl_domain::model::*;
use pl_domain::{DomainError, DomainResult, Kind, Principal};
use serde_json::{Map, Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

use crate::{Store, auth, documents, ident, objects};

/// Router with the v1 write handlers (state and `Authz` extension already set).
#[derive(Clone)]
pub struct LegacyRouter(axum::Router);

impl LegacyRouter {
    pub fn new(router: axum::Router) -> Self {
        LegacyRouter(router)
    }

    /// The internal v1 router (for in-process read accesses, e.g. `/ask` in pl-api-v2).
    pub fn router(&self) -> axum::Router {
        self.0.clone()
    }
}

/// v1 path of the list of a kind.
fn v1_base(kind: Kind) -> DomainResult<&'static str> {
    match kind {
        Kind::Tag => Ok("/api/tags/"),
        Kind::Correspondent => Ok("/api/correspondents/"),
        Kind::DocumentType => Ok("/api/document_types/"),
        other => Err(DomainError::NotImplemented(format!("writing {} is not available in API v2 yet", other.plural()))),
    }
}

/// One internal v1 request; returns the JSON body on 2xx.
async fn call(store: &Store, p: &Principal, method: Method, path: &str, body: Option<Value>) -> DomainResult<Value> {
    let Some(router) = store.legacy.clone() else {
        return Err(DomainError::NotImplemented("write path not mounted".into()));
    };
    let ext = auth::ext(p)?;
    let mut ctx = (*ext.ctx).clone();
    ctx.http_method = method.clone();
    let bytes = body.map(|b| serde_json::to_vec(&b).unwrap_or_default()).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost")
        .header(header::ACCEPT, "application/json; version=10")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .map_err(DomainError::internal)?;
    req.extensions_mut().insert(Arc::new(ctx));
    if let Some(peer) = ext.peer {
        req.extensions_mut().insert(axum::extract::ConnectInfo(peer));
    }
    let resp = match router.0.oneshot(req).await {
        Ok(r) => r,
        Err(e) => match e {},
    };
    if pl_core::markers::is_delegation(&resp) {
        return Err(DomainError::NotImplemented("this change is only supported by the classic API".into()));
    }
    let status = resp.status();
    let raw = resp.into_body().collect().await.map_err(DomainError::internal)?.to_bytes();
    let v: Value = if raw.is_empty() { Value::Null } else { serde_json::from_slice(&raw).unwrap_or(Value::Null) };
    if status.is_success() {
        return Ok(v);
    }
    Err(map_error(status, v))
}

/// v1 error → domain error (field names mapped to v2).
fn map_error(status: StatusCode, v: Value) -> DomainError {
    let detail = v.get("detail").and_then(Value::as_str).map(str::to_string);
    match status {
        StatusCode::BAD_REQUEST => {
            let mut fields = Map::new();
            let mut message = "Invalid input.".to_string();
            match v {
                Value::Object(m) => {
                    for (k, val) in m {
                        if k == "non_field_errors"
                            && let Some(first) = val.as_array().and_then(|a| a.first()).and_then(Value::as_str)
                        {
                            message = first.to_string();
                        }
                        let key = match k.as_str() {
                            "archive_serial_number" => "asn",
                            "match" => "matching.pattern",
                            "matching_algorithm" => "matching.algorithm",
                            "is_insensitive" => "matching.is_insensitive",
                            other => other,
                        };
                        fields.insert(key.to_string(), val);
                    }
                }
                Value::Array(a) => {
                    if let Some(first) = a.first().and_then(Value::as_str) {
                        message = first.to_string();
                    }
                }
                _ => {}
            }
            DomainError::Invalid { message, fields }
        }
        StatusCode::UNAUTHORIZED => DomainError::Unauthenticated,
        StatusCode::FORBIDDEN => {
            DomainError::Forbidden(detail.unwrap_or_else(|| "You do not have permission to perform this action.".into()))
        }
        StatusCode::NOT_FOUND => DomainError::NotFound,
        StatusCode::CONFLICT => DomainError::Conflict(detail.unwrap_or_default()),
        s => DomainError::Internal(format!("v1 write path answered {s}: {v}")),
    }
}

fn check_rev(if_match: Option<i64>, current: i64) -> DomainResult<()> {
    match if_match {
        Some(r) if r != current => Err(DomainError::RevMismatch { current }),
        _ => Ok(()),
    }
}

async fn object_body(store: &Store, kind: Kind, w: &ObjectWrite) -> DomainResult<Value> {
    let mut m = Map::new();
    if let Some(n) = &w.name {
        m.insert("name".into(), json!(n));
    }
    if let Some(mw) = &w.matching {
        if let Some(alg) = &mw.algorithm {
            let code = matching_code(alg).ok_or_else(|| {
                DomainError::field("matching.algorithm", format!("Unknown algorithm \"{alg}\" ({}).", MATCHING_ALGORITHMS.join(", ")))
            })?;
            m.insert("matching_algorithm".into(), json!(code));
        }
        if let Some(pat) = &mw.pattern {
            m.insert("match".into(), json!(pat));
        }
        if let Some(i) = mw.is_insensitive {
            m.insert("is_insensitive".into(), json!(i));
        }
    }
    let only = |field: &str, allowed: bool| -> DomainResult<()> {
        if allowed { Ok(()) } else { Err(DomainError::field(field, format!("Not a field of {}.", kind.as_str()))) }
    };
    if let Some(c) = &w.color {
        only("color", kind == Kind::Tag)?;
        m.insert("color".into(), json!(c));
    }
    if let Some(b) = w.is_inbox_tag {
        only("is_inbox_tag", kind == Kind::Tag)?;
        m.insert("is_inbox_tag".into(), json!(b));
    }
    if let Some(parent) = &w.parent {
        only("parent", kind == Kind::Tag)?;
        let v = match parent {
            Some(u) => json!(ident::ref_ids(&store.db, Kind::Tag, "parent", &[*u]).await?[0]),
            None => Value::Null,
        };
        m.insert("parent".into(), v);
    }
    if let Some(path) = &w.path {
        only("path", kind == Kind::StoragePath)?;
        m.insert("path".into(), json!(path));
    }
    if let Some(owner) = &w.owner {
        let v = match owner {
            Some(u) => json!(ident::ref_ids(&store.db, Kind::User, "owner", &[*u]).await?[0]),
            None => Value::Null,
        };
        m.insert("owner".into(), v);
    }
    Ok(Value::Object(m))
}

async fn reload(store: &Store, p: &Principal, kind: Kind, id: i64) -> DomainResult<Object> {
    let mut v = objects::by_legacy_ids(store, p, kind, &[id]).await?;
    v.pop().and_then(|(_, o)| o).ok_or(DomainError::NotFound)
}

pub(crate) async fn create(store: &Store, p: &Principal, kind: Kind, w: &ObjectWrite) -> DomainResult<Object> {
    let base = v1_base(kind)?;
    let body = object_body(store, kind, w).await?;
    let v = call(store, p, Method::POST, base, Some(body)).await?;
    let id = v.get("id").and_then(Value::as_i64).ok_or_else(|| DomainError::Internal("v1 create without id".into()))?;
    reload(store, p, kind, id).await
}

/// Visible? (404 before any revision check, so that `412` reveals nothing.)
async fn visible(store: &Store, p: &Principal, kind: Kind, uuid: Uuid) -> DomainResult<(i64, i64)> {
    let (id, rev) = ident::resolve_one(&store.db, kind, uuid).await?;
    let user = auth::user(p)?;
    crate::perm::require(user, crate::perm::spec(kind).view_perm)?;
    match objects::by_legacy_ids(store, p, kind, &[id]).await?.pop() {
        Some((_, Some(_))) => Ok((id, rev)),
        _ => Err(DomainError::NotFound),
    }
}

pub(crate) async fn update(
    store: &Store,
    p: &Principal,
    kind: Kind,
    uuid: Uuid,
    if_match: Option<i64>,
    w: &ObjectWrite,
) -> DomainResult<Object> {
    let base = v1_base(kind)?;
    let (id, rev) = visible(store, p, kind, uuid).await?;
    check_rev(if_match, rev)?;
    let body = object_body(store, kind, w).await?;
    call(store, p, Method::PATCH, &format!("{base}{id}/"), Some(body)).await?;
    reload(store, p, kind, id).await
}

pub(crate) async fn delete(store: &Store, p: &Principal, kind: Kind, uuid: Uuid, if_match: Option<i64>) -> DomainResult<()> {
    let base = v1_base(kind)?;
    let (id, rev) = visible(store, p, kind, uuid).await?;
    check_rev(if_match, rev)?;
    call(store, p, Method::DELETE, &format!("{base}{id}/"), None).await?;
    Ok(())
}

pub(crate) async fn patch_document(
    store: &Store,
    p: &Principal,
    uuid: Uuid,
    if_match: Option<i64>,
    patch: &DocumentPatch,
) -> DomainResult<Document> {
    let user = auth::user(p)?;
    let (id, rev) = documents::visible_one(store, user, uuid).await?;
    check_rev(if_match, rev)?;
    let db = &store.db;
    let mut m = Map::new();
    if let Some(t) = &patch.title {
        m.insert("title".into(), json!(t));
    }
    if let Some(c) = &patch.created {
        m.insert("created".into(), json!(c.to_string()));
    }
    for (field, kind, val) in [
        ("correspondent", Kind::Correspondent, &patch.correspondent),
        ("document_type", Kind::DocumentType, &patch.document_type),
        ("storage_path", Kind::StoragePath, &patch.storage_path),
        ("owner", Kind::User, &patch.owner),
    ] {
        if let Some(v) = val {
            let j = match v {
                Some(u) => json!(ident::ref_ids(db, kind, field, &[*u]).await?[0]),
                None => Value::Null,
            };
            m.insert(field.into(), j);
        }
    }
    if let Some(tags) = &patch.tags {
        m.insert("tags".into(), json!(ident::ref_ids(db, Kind::Tag, "tags", tags).await?));
    }
    if let Some(asn) = &patch.asn {
        m.insert("archive_serial_number".into(), json!(asn));
    }
    if let Some(cfs) = &patch.custom_fields {
        let fields: Vec<Uuid> = cfs.iter().map(|c| c.field).collect();
        let ids = ident::ref_ids(db, Kind::CustomField, "custom_fields", &fields).await?;
        let mut out = Vec::new();
        for (c, fid) in cfs.iter().zip(ids) {
            // document links: UUIDs → IDs
            let value = match &c.value {
                Value::Array(a) if a.iter().all(|x| x.as_str().is_some_and(|s| Uuid::parse_str(s).is_ok())) && !a.is_empty() => {
                    let us: Vec<Uuid> = a.iter().filter_map(|x| x.as_str().and_then(|s| Uuid::parse_str(s).ok())).collect();
                    json!(ident::ref_ids(db, Kind::Document, "custom_fields", &us).await?)
                }
                other => other.clone(),
            };
            out.push(json!({"field": fid, "value": value}));
        }
        m.insert("custom_fields".into(), Value::Array(out));
    }
    call(store, p, Method::PATCH, &format!("/api/documents/{id}/"), Some(Value::Object(m))).await?;
    documents::get_by_id(store, user, id).await
}

pub(crate) async fn add_note(store: &Store, p: &Principal, doc: Uuid, text: &str) -> DomainResult<Vec<Note>> {
    let user = auth::user(p)?;
    let (id, _) = documents::visible_one(store, user, doc).await?;
    call(store, p, Method::POST, &format!("/api/documents/{id}/notes/"), Some(json!({"note": text}))).await?;
    documents::notes_by_id(store, id).await
}

pub(crate) async fn delete_note(store: &Store, p: &Principal, doc: Uuid, note: i64) -> DomainResult<Vec<Note>> {
    let user = auth::user(p)?;
    let (id, _) = documents::visible_one(store, user, doc).await?;
    call(store, p, Method::DELETE, &format!("/api/documents/{id}/notes/?id={note}"), None).await?;
    documents::notes_by_id(store, id).await
}

pub(crate) async fn set_ui_settings(store: &Store, p: &Principal, settings: &Value) -> DomainResult<Value> {
    if !settings.is_object() {
        return Err(DomainError::field("settings", "Must be an object."));
    }
    call(store, p, Method::POST, "/api/ui_settings/", Some(json!({"settings": settings}))).await?;
    objects::ui_settings(store, p).await
}
