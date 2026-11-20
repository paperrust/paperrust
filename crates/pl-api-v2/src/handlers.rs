//! Handlers of the object and document endpoints.

use std::collections::HashMap;

use axum::extract::{Path, Query};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use pl_domain::model::*;
use pl_domain::query::DocQuery;
use pl_domain::{DomainError, Kind};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::V2;
use crate::error::{V2Error, V2Result};
use crate::http::{Caller, OptCaller, V2Json, if_match, json, json_with_rev, truthy};

pub(crate) fn parse_uuid(s: &str) -> V2Result<Uuid> {
    Uuid::parse_str(s).map_err(|_| V2Error(DomainError::NotFound))
}

fn parse_limit(s: Option<&String>) -> V2Result<i64> {
    match s {
        None => Ok(0),
        Some(v) => match v.parse::<i64>() {
            Ok(n) if (1..=1000).contains(&n) => Ok(n),
            _ => Err(V2Error(DomainError::field("limit", "Must be between 1 and 1000."))),
        },
    }
}

// ------------------------------------------------------------------------------ server, me

pub async fn server(v2: V2, OptCaller(p): OptCaller) -> V2Result<Response> {
    let mut features = vec!["device_tokens", "files_range", "documents_query"];
    if v2.feed.is_some() {
        features.push("sync");
    }
    let mut body = json!({ "product": "paperrust", "api": 2, "features": features });
    if p.is_some() {
        body["version"] = json!(pl_core::brand::PAPERRUST_VERSION);
        match &v2.feed {
            Some(f) => {
                let m = f.meta().await?;
                body["server_uuid"] = json!(m.server_uuid);
                body["min_cursor"] = json!(pl_domain::repo::format_feed_cursor(m.min_cursor));
                body["perm_epoch"] = json!(m.perm_epoch);
            }
            None => {
                body["server_uuid"] = Value::Null;
                body["min_cursor"] = Value::Null;
                body["perm_epoch"] = Value::Null;
            }
        }
    }
    Ok(json(&body))
}

pub async fn me(v2: V2, Caller(p): Caller) -> V2Result<Response> {
    Ok(json(&v2.repo.me(&p).await?))
}

pub async fn get_ui_settings(v2: V2, Caller(p): Caller) -> V2Result<Response> {
    Ok(json(&json!({ "settings": v2.repo.ui_settings(&p).await? })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiSettingsBody {
    settings: Value,
}

pub async fn put_ui_settings(v2: V2, Caller(p): Caller, V2Json(b): V2Json<UiSettingsBody>) -> V2Result<Response> {
    Ok(json(&json!({ "settings": v2.repo.set_ui_settings(&p, &b.settings).await? })))
}

// ------------------------------------------------------------------------------ Objekte

pub async fn list_objects(
    v2: V2,
    kind: Kind,
    Caller(p): Caller,
    q: HashMap<String, String>,
) -> V2Result<Response> {
    let req = ListRequest {
        cursor: q.get("cursor").cloned(),
        limit: parse_limit(q.get("limit"))?,
        sort: q.get("sort").cloned(),
        name_contains: q.get("name_contains").cloned(),
        count: truthy(q.get("count").map(String::as_str)),
        document_count: truthy(q.get("document_count").map(String::as_str)),
        sharing: q.get("include").is_some_and(|i| i.split(',').any(|x| x.trim() == "sharing")),
    };
    Ok(json(&v2.repo.list(&p, kind, &req).await?))
}

pub async fn get_object(v2: V2, kind: Kind, Caller(p): Caller, uuid: String, q: HashMap<String, String>) -> V2Result<Response> {
    let uuid = parse_uuid(&uuid)?;
    let sharing = q.get("include").is_some_and(|i| i.split(',').any(|x| x.trim() == "sharing"));
    let o = v2.repo.get(&p, kind, uuid, sharing).await?;
    let rev = o.meta().rev;
    Ok(json_with_rev(StatusCode::OK, &o, Some(rev)))
}

pub async fn create_object(v2: V2, kind: Kind, Caller(p): Caller, w: ObjectWrite) -> V2Result<Response> {
    let o = v2.repo.create(&p, kind, &w).await?;
    let rev = o.meta().rev;
    Ok(json_with_rev(StatusCode::CREATED, &o, Some(rev)))
}

pub async fn update_object(
    v2: V2,
    kind: Kind,
    Caller(p): Caller,
    uuid: String,
    headers: HeaderMap,
    w: ObjectWrite,
) -> V2Result<Response> {
    let uuid = parse_uuid(&uuid)?;
    let o = v2.repo.update(&p, kind, uuid, if_match(&headers)?, &w).await?;
    let rev = o.meta().rev;
    Ok(json_with_rev(StatusCode::OK, &o, Some(rev)))
}

pub async fn delete_object(v2: V2, kind: Kind, Caller(p): Caller, uuid: String, headers: HeaderMap) -> V2Result<Response> {
    let uuid = parse_uuid(&uuid)?;
    v2.repo.delete(&p, kind, uuid, if_match(&headers)?).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchBody {
    kind: String,
    uuids: Vec<Uuid>,
    #[serde(default)]
    include: Vec<String>,
}

pub async fn batch(v2: V2, Caller(p): Caller, V2Json(b): V2Json<BatchBody>) -> V2Result<Response> {
    let kind = Kind::parse(&b.kind).ok_or_else(|| V2Error(DomainError::field("kind", format!("Unknown kind \"{}\".", b.kind))))?;
    if b.uuids.len() > 500 {
        return Err(V2Error(DomainError::field("uuids", "At most 500 per request.")));
    }
    let include = DocInclude::parse(b.include.iter().map(String::as_str))?;
    let (items, missing) = v2.repo.batch(&p, kind, &b.uuids, include).await?;
    Ok(json(&json!({ "items": items, "missing": missing })))
}

// ------------------------------------------------------------------------------ Dokumente

pub async fn list_documents(v2: V2, Caller(p): Caller, Query(q): Query<HashMap<String, String>>) -> V2Result<Response> {
    let dq = DocQuery {
        filter: None,
        sort: q.get("sort").cloned(),
        limit: match q.get("limit") {
            None => None,
            Some(_) => Some(parse_limit(q.get("limit"))?),
        },
        cursor: q.get("cursor").cloned(),
        include: q.get("include").map(|i| i.split(',').map(|s| s.trim().to_string()).collect()).unwrap_or_default(),
        count: truthy(q.get("count").map(String::as_str)),
    };
    Ok(json(&v2.repo.query_documents(&p, &dq).await?))
}

pub async fn query_documents(v2: V2, Caller(p): Caller, V2Json(dq): V2Json<DocQuery>) -> V2Result<Response> {
    Ok(json(&v2.repo.query_documents(&p, &dq).await?))
}

pub async fn get_document(
    v2: V2,
    Caller(p): Caller,
    Path(uuid): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> V2Result<Response> {
    let uuid = parse_uuid(&uuid)?;
    let include = DocInclude::parse(q.get("include").map(|s| s.as_str()).unwrap_or("").split(','))?;
    let d = v2.repo.get_document(&p, uuid, include).await?;
    let rev = d.meta.rev;
    Ok(json_with_rev(StatusCode::OK, &d, Some(rev)))
}

pub async fn patch_document(
    v2: V2,
    Caller(p): Caller,
    Path(uuid): Path<String>,
    headers: HeaderMap,
    V2Json(patch): V2Json<DocumentPatch>,
) -> V2Result<Response> {
    let uuid = parse_uuid(&uuid)?;
    let d = v2.repo.patch_document(&p, uuid, if_match(&headers)?, &patch).await?;
    let rev = d.meta.rev;
    Ok(json_with_rev(StatusCode::OK, &d, Some(rev)))
}

pub async fn notes(v2: V2, Caller(p): Caller, Path(uuid): Path<String>) -> V2Result<Response> {
    Ok(json(&json!({ "items": v2.repo.notes(&p, parse_uuid(&uuid)?).await? })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteBody {
    note: String,
}

pub async fn add_note(
    v2: V2,
    Caller(p): Caller,
    Path(uuid): Path<String>,
    V2Json(b): V2Json<NoteBody>,
) -> V2Result<Response> {
    let items = v2.repo.add_note(&p, parse_uuid(&uuid)?, &b.note).await?;
    Ok(json_with_rev(StatusCode::CREATED, &json!({ "items": items }), None))
}

pub async fn delete_note(v2: V2, Caller(p): Caller, Path((uuid, note)): Path<(String, i64)>) -> V2Result<Response> {
    Ok(json(&json!({ "items": v2.repo.delete_note(&p, parse_uuid(&uuid)?, note).await? })))
}

pub async fn file(
    v2: V2,
    Caller(p): Caller,
    Path((uuid, variant)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> V2Result<Response> {
    let uuid = parse_uuid(&uuid)?;
    let variant = FileVariant::parse(&variant).ok_or(V2Error(DomainError::NotFound))?;
    let f = v2.repo.file(&p, uuid, variant).await?;
    crate::files::serve(&method, &headers, f).await
}

pub async fn not_found() -> Response {
    V2Error(DomainError::NotFound).into_response()
}
