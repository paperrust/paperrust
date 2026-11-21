//! `/rs/api/v2/life-areas` – life areas (contract docs/contract/pl-api-v2.md,
//! "PaperRust-Oberfläche"). Own areas per user, instance default (superuser writes),
//! suggestions from existing data (never stored).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use pl_core::AppState;
use pl_domain::{DomainError, Principal};
use pl_timeline::areas::{AreaWrite, Scope, suggest};
use pl_timeline::store;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::d2::{self, tl};
use crate::error::{V2Error, V2Result};
use crate::http::{Caller, V2Json, json, json_with_rev};

pub fn routes(r: axum::Router<AppState>) -> axum::Router<AppState> {
    r.route("/life-areas", get(list).put(replace).post(create).delete(delete_scope))
        .route("/life-areas/suggestions", get(suggestions))
        .route("/life-areas/{uuid}", get(one).patch(patch).delete(delete_one))
}

fn scope_of(q: &HashMap<String, String>) -> V2Result<Scope> {
    Scope::parse(q.get("scope").map(String::as_str)).map_err(tl)
}

/// Write permission for a scope (default only superuser).
fn may_write(p: &Principal, scope: Scope) -> V2Result<()> {
    let u = d2::user(p)?;
    if !u.is_active {
        return Err(V2Error(DomainError::Forbidden("Inactive user.".into())));
    }
    if scope == Scope::Instance && !u.is_superuser {
        return Err(V2Error(DomainError::Forbidden("Only superusers can change the default life areas.".into())));
    }
    Ok(())
}

pub async fn list(State(state): State<AppState>, Caller(p): Caller, Query(q): Query<HashMap<String, String>>) -> V2Result<Response> {
    d2::ensure(&state).await?;
    let pool = state.db();
    let (items, source) = match q.get("scope") {
        None => store::effective_areas(pool, p.user_id).await.map_err(tl)?,
        Some(_) => {
            let s = scope_of(&q)?;
            (store::list_areas(pool, s, p.user_id).await.map_err(tl)?, s.as_str())
        }
    };
    Ok(json(&json!({ "items": items, "source": source })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceBody {
    items: Vec<AreaWrite>,
}

pub async fn replace(
    State(state): State<AppState>,
    Caller(p): Caller,
    Query(q): Query<HashMap<String, String>>,
    V2Json(body): V2Json<ReplaceBody>,
) -> V2Result<Response> {
    let scope = scope_of(&q)?;
    may_write(&p, scope)?;
    d2::ensure(&state).await?;
    let vis = d2::visibility(&p)?;
    let mut vals = Vec::with_capacity(body.items.len());
    for (i, w) in body.items.iter().enumerate() {
        let v = w.values(i).map_err(|e| prefix(e, i))?;
        store::check_rules_visible(state.db(), &v.rules, &vis).await.map_err(|e| prefix(e, i))?;
        vals.push((w.uuid, v));
    }
    let items = store::replace_areas(state.db(), scope, p.user_id, &vals).await.map_err(tl)?;
    Ok(json(&json!({ "items": items, "source": scope.as_str() })))
}

/// Field error of a list entry as `items[i].field`.
fn prefix(e: pl_timeline::TimelineError, i: usize) -> V2Error {
    match e {
        pl_timeline::TimelineError::Invalid(f, m) => V2Error(DomainError::field(&format!("items[{i}].{f}"), m)),
        other => tl(other),
    }
}

pub async fn create(
    State(state): State<AppState>,
    Caller(p): Caller,
    Query(q): Query<HashMap<String, String>>,
    V2Json(w): V2Json<AreaWrite>,
) -> V2Result<Response> {
    let scope = scope_of(&q)?;
    may_write(&p, scope)?;
    d2::ensure(&state).await?;
    let pos = store::list_areas(state.db(), scope, p.user_id).await.map_err(tl)?.len();
    let v = w.values(pos).map_err(tl)?;
    store::check_rules_visible(state.db(), &v.rules, &d2::visibility(&p)?).await.map_err(tl)?;
    let a = store::insert_area(state.db(), scope, p.user_id, &v).await.map_err(tl)?;
    Ok(json_with_rev(StatusCode::CREATED, &a, None))
}

pub async fn delete_scope(
    State(state): State<AppState>,
    Caller(p): Caller,
    Query(q): Query<HashMap<String, String>>,
) -> V2Result<Response> {
    let scope = scope_of(&q)?;
    may_write(&p, scope)?;
    d2::ensure(&state).await?;
    store::delete_scope(state.db(), scope, p.user_id).await.map_err(tl)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Visible area (own or default), otherwise 404.
async fn visible_area(state: &AppState, p: &Principal, uuid: &str) -> V2Result<pl_timeline::areas::Area> {
    let u = Uuid::parse_str(uuid).map_err(|_| V2Error(DomainError::NotFound))?;
    d2::ensure(state).await?;
    let a = store::get_area(state.db(), u).await.map_err(tl)?.ok_or(V2Error(DomainError::NotFound))?;
    match a.user_id {
        Some(owner) if owner != p.user_id => Err(V2Error(DomainError::NotFound)),
        _ => Ok(a),
    }
}

pub async fn one(State(state): State<AppState>, Caller(p): Caller, Path(uuid): Path<String>) -> V2Result<Response> {
    let a = visible_area(&state, &p, &uuid).await?;
    Ok(json(&a))
}

pub async fn patch(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(uuid): Path<String>,
    V2Json(w): V2Json<AreaWrite>,
) -> V2Result<Response> {
    let a = visible_area(&state, &p, &uuid).await?;
    may_write(&p, a.scope)?;
    if w.uuid.is_some_and(|u| u != a.uuid) {
        return Err(V2Error(DomainError::field("uuid", "Cannot be changed.")));
    }
    let v = w.patch(&a).map_err(tl)?;
    if w.rules.is_some() {
        store::check_rules_visible(state.db(), &v.rules, &d2::visibility(&p)?).await.map_err(tl)?;
    }
    let a = store::update_area(state.db(), a.id, &v).await.map_err(tl)?;
    Ok(json(&a))
}

pub async fn delete_one(State(state): State<AppState>, Caller(p): Caller, Path(uuid): Path<String>) -> V2Result<Response> {
    let a = visible_area(&state, &p, &uuid).await?;
    may_write(&p, a.scope)?;
    store::delete_area(state.db(), a.id).await.map_err(tl)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn suggestions(State(state): State<AppState>, Caller(p): Caller, Query(q): Query<HashMap<String, String>>) -> V2Result<Response> {
    d2::require_view_documents(&p)?;
    let english = match q.get("lang").map(|s| s.trim()) {
        None | Some("de") | Some("") => false,
        Some("en") => true,
        Some(_) => return Err(V2Error(DomainError::field("lang", "Expected de or en."))),
    };
    let vis = d2::visibility(&p)?;
    let cands = store::candidates(state.db(), &vis).await.map_err(tl)?;
    let docs = store::doc_keys(state.db(), &vis).await.map_err(tl)?;
    Ok(json(&json!({ "items": suggest(&cands, &docs, english) })))
}
