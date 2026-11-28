//! `/rs/api/v2/timeline…` – timeline, "Upcoming", done/snooze, reminders,
//! mapping of the date fields, backfill (contract docs/contract/pl-api-v2.md,
//! "PaperRust-Oberfläche"). Documents exactly like the document list (`pl_store::visible_ids_sql`).

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use chrono::NaiveDate;
use pl_core::AppState;
use pl_domain::{DomainError, Principal};
use pl_timeline::build::{self, LaneArea, TimelineQuery, item_id, parse_item_id};
use pl_timeline::extract::DateParse;
use pl_timeline::fields::{self, FieldKinds};
use pl_timeline::store::{self, LoadSpec, Visibility};
use pl_timeline::{DateKind, refresh};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::d2::{self, tl};
use crate::error::{V2Error, V2Result};
use crate::http::{Caller, V2Json, json, json_with_rev, truthy};

/// Re-detect at most this many outdated documents per request.
pub const STALE_PER_REQUEST: i64 = 200;

pub fn routes(r: axum::Router<AppState>) -> axum::Router<AppState> {
    r.route("/timeline", get(timeline))
        .route("/timeline/upcoming", get(upcoming))
        .route("/timeline/items/{id}/state", put(set_state).delete(clear_state))
        .route("/timeline/reminders", get(reminders).post(add_reminder))
        .route("/timeline/reminders/{id}", delete(delete_reminder))
        .route("/timeline/config", get(config))
        .route("/timeline/backfill", post(backfill))
}

/// Everything needed for a query.
struct Ctx {
    vis: Visibility,
    fields: FieldKinds,
    names: HashMap<i64, String>,
    extract: bool,
}

async fn ctx(state: &AppState, p: &Principal) -> V2Result<Ctx> {
    d2::require_view_documents(p)?;
    d2::ensure(state).await?;
    let vis = d2::visibility(p)?;
    let all = store::custom_fields(state.db()).await.map_err(tl)?;
    let (fields, warnings) = fields::resolve(&all, d2::setting(pl_timeline::SETTING_FIELDS).as_deref());
    for w in warnings {
        tracing::warn!(target: "paperrust.timeline", "{}: {w}", pl_timeline::SETTING_FIELDS);
    }
    let names = all.into_iter().map(|f| (f.id, f.name)).collect();
    let extract = refresh::enabled(&d2::setting);
    if extract {
        let dp = DateParse::from_lookup(d2::setting);
        if let Err(e) = refresh::refresh_stale(state.db(), &dp, Some(&vis.documents), STALE_PER_REQUEST).await {
            tracing::warn!(target: "paperrust.timeline", error = %e, "refreshing stale timeline dates failed");
        }
    }
    Ok(Ctx { vis, fields, names, extract })
}

/// Effective ranges with resolved rules.
async fn lane_areas(state: &AppState, p: &Principal) -> V2Result<(Vec<LaneArea>, &'static str)> {
    let (areas, source) = store::effective_areas(state.db(), p.user_id).await.map_err(tl)?;
    let mut out = Vec::with_capacity(areas.len());
    for a in areas {
        let rules = store::resolve_rules(state.db(), &a.rules).await.map_err(tl)?;
        out.push(LaneArea { area: a, rules });
    }
    Ok((out, source))
}

fn lanes_filter(q: &HashMap<String, String>) -> V2Result<Option<HashSet<String>>> {
    let Some(v) = q.get("areas").filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let mut out = HashSet::new();
    for part in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if part == "other" {
            out.insert(part.to_string());
        } else {
            let u =
                uuid::Uuid::parse_str(part).map_err(|_| V2Error(DomainError::field("areas", "Expected area UUIDs and/or \"other\".")))?;
            out.insert(u.to_string());
        }
    }
    Ok(Some(out))
}

fn kinds_filter(q: &HashMap<String, String>) -> V2Result<Option<HashSet<DateKind>>> {
    let Some(v) = q.get("kinds").filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    v.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|k| DateKind::parse(k).ok_or_else(|| V2Error(DomainError::field("kinds", format!("Unknown kind {k:?}.")))))
        .collect::<V2Result<HashSet<_>>>()
        .map(Some)
}

async fn load_items(
    state: &AppState,
    p: &Principal,
    c: &Ctx,
    created: Option<(NaiveDate, NaiveDate)>,
) -> V2Result<(Vec<build::Item>, HashMap<i64, build::DocRow>)> {
    let spec = LoadSpec { user_id: p.user_id, created_range: created, fields: &c.fields, field_names: &c.names, content_dates: c.extract };
    let (docs, rows) = store::load(state.db(), &c.vis, &spec).await.map_err(tl)?;
    let ids: Vec<i64> = docs.keys().copied().collect();
    let states = store::states(state.db(), p.user_id, &ids).await.map_err(tl)?;
    Ok((build::items(&docs, &rows, &states), docs))
}

pub async fn timeline(State(state): State<AppState>, Caller(p): Caller, Query(q): Query<HashMap<String, String>>) -> V2Result<Response> {
    let today = d2::today(&state);
    let from = match q.get("from") {
        Some(v) => d2::parse_date("from", v)?,
        None => today - chrono::Duration::days(365),
    };
    let to = match q.get("to") {
        Some(v) => d2::parse_date("to", v)?,
        None => today + chrono::Duration::days(365),
    };
    if to < from {
        return Err(V2Error(DomainError::field("to", "Must not be before from.")));
    }
    if (to - from).num_days() > 3660 {
        return Err(V2Error(DomainError::field("to", "The range may span at most 3660 days.")));
    }
    let kinds = kinds_filter(&q)?;
    let lanes = lanes_filter(&q)?;
    let limit = d2::parse_int("limit", q.get("limit"), 2000, 1, 10000)? as usize;
    let c = ctx(&state, &p).await?;
    let (areas, source) = lane_areas(&state, &p).await?;
    let created = kinds.as_ref().is_none_or(|k| k.contains(&DateKind::Created)).then_some((from, to));
    let (items, docs) = load_items(&state, &p, &c, created).await?;
    let tq = TimelineQuery { from, to, kinds, lanes, limit };
    let (lanes, truncated) = build::lanes(items, &docs, &areas, &tq);
    Ok(json(&json!({
        "from": from,
        "to": to,
        "today": today,
        "areas_source": source,
        "extract_dates": c.extract,
        "truncated": truncated,
        "lanes": lanes,
    })))
}

pub async fn upcoming(State(state): State<AppState>, Caller(p): Caller, Query(q): Query<HashMap<String, String>>) -> V2Result<Response> {
    let today = d2::today(&state);
    let days = d2::parse_int("days", q.get("days"), 60, 1, 366)?;
    let overdue = d2::parse_int("overdue_days", q.get("overdue_days"), 30, 0, 366)?;
    let include_done = truthy(q.get("include_done").map(String::as_str));
    let lanes = lanes_filter(&q)?;
    let c = ctx(&state, &p).await?;
    let (areas, _) = lane_areas(&state, &p).await?;
    let (items, docs) = load_items(&state, &p, &c, None).await?;
    let up = build::upcoming(items, &docs, &areas, today, days, overdue, lanes.as_ref(), include_done);
    Ok(json(&json!({ "today": today, "days": days, "overdue_days": overdue, "items": up })))
}

/// Entry id → (document id) if visible, otherwise 404.
async fn item_doc(state: &AppState, p: &Principal, id: &str) -> V2Result<i64> {
    d2::require_view_documents(p)?;
    d2::ensure(state).await?;
    let (u, _, _) = parse_item_id(id).ok_or(V2Error(DomainError::NotFound))?;
    store::visible_document(state.db(), &d2::visibility(p)?, u).await.map_err(tl)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateBody {
    #[serde(default)]
    done: Option<bool>,
    #[serde(default, deserialize_with = "double")]
    snoozed_until: Option<Option<String>>,
    #[serde(default)]
    snooze_days: Option<i64>,
}

fn double<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

pub async fn set_state(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(id): Path<String>,
    V2Json(b): V2Json<StateBody>,
) -> V2Result<Response> {
    let doc = item_doc(&state, &p, &id).await?;
    let snoozed: Option<Option<NaiveDate>> = match (b.snoozed_until, b.snooze_days) {
        (Some(_), Some(_)) => {
            return Err(V2Error(DomainError::field("snooze_days", "Use either snoozed_until or snooze_days.")));
        }
        (Some(None), None) => Some(None),
        (Some(Some(s)), None) => Some(Some(d2::parse_date("snoozed_until", &s)?)),
        (None, Some(n)) if (1..=366).contains(&n) => Some(Some(d2::today(&state) + chrono::Duration::days(n))),
        (None, Some(_)) => {
            return Err(V2Error(DomainError::field("snooze_days", "Expected 1–366.")));
        }
        (None, None) => None,
    };
    if b.done.is_none() && snoozed.is_none() {
        return Err(V2Error(DomainError::invalid("Nothing to change: send done, snoozed_until or snooze_days.")));
    }
    let st = store::set_state(state.db(), p.user_id, &id, doc, b.done, snoozed).await.map_err(tl)?;
    Ok(json(&json!({ "id": id, "done": st.done_at.is_some(), "done_at": st.done_at, "snoozed_until": st.snoozed_until })))
}

pub async fn clear_state(State(state): State<AppState>, Caller(p): Caller, Path(id): Path<String>) -> V2Result<Response> {
    item_doc(&state, &p, &id).await?;
    store::clear_state(state.db(), p.user_id, &id).await.map_err(tl)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn reminder_json(r: &store::Reminder, uuid: uuid::Uuid, title: &str) -> Value {
    json!({
        "id": r.id,
        "item_id": item_id(uuid, r.kind, r.date),
        "document": uuid,
        "legacy_id": r.document_id,
        "title": title,
        "kind": r.kind,
        "date": r.date,
        "note": r.note,
        "created": r.created,
    })
}

pub async fn reminders(State(state): State<AppState>, Caller(p): Caller) -> V2Result<Response> {
    d2::require_view_documents(&p)?;
    d2::ensure(&state).await?;
    let list = store::list_reminders(state.db(), p.user_id, &d2::visibility(&p)?).await.map_err(tl)?;
    let ids: Vec<i64> = list.iter().map(|r| r.document_id).collect();
    let t = store::titles(state.db(), &ids).await.map_err(tl)?;
    let items: Vec<Value> = list.iter().filter_map(|r| t.get(&r.document_id).map(|(u, title)| reminder_json(r, *u, title))).collect();
    Ok(json(&json!({ "items": items })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReminderBody {
    pub document: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub date: String,
    #[serde(default)]
    pub note: Option<String>,
}

pub async fn add_reminder(State(state): State<AppState>, Caller(p): Caller, V2Json(b): V2Json<ReminderBody>) -> V2Result<Response> {
    d2::require_view_documents(&p)?;
    d2::ensure(&state).await?;
    let kind = match b.kind.as_deref() {
        None => DateKind::Due,
        Some(k) => DateKind::parse(k)
            .filter(|k| *k != DateKind::Created)
            .ok_or_else(|| V2Error(DomainError::field("kind", "Expected due, cancel_by, valid_from, valid_until or warranty_until.")))?,
    };
    let date = d2::parse_date("date", &b.date)?;
    let note = b.note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    if note.as_ref().is_some_and(|n| n.chars().count() > 500) {
        return Err(V2Error(DomainError::field("note", "At most 500 characters.")));
    }
    let u = uuid::Uuid::parse_str(b.document.trim()).map_err(|_| V2Error(DomainError::field("document", "Expected a document UUID.")))?;
    let doc = store::visible_document(state.db(), &d2::visibility(&p)?, u).await.map_err(tl)?;
    let (r, created) = store::add_reminder(state.db(), p.user_id, doc, kind, date, note.as_deref()).await.map_err(tl)?;
    let t = store::titles(state.db(), &[doc]).await.map_err(tl)?;
    let title = t.get(&doc).map(|x| x.1.clone()).unwrap_or_default();
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    Ok(json_with_rev(status, &reminder_json(&r, u, &title), None))
}

pub async fn delete_reminder(State(state): State<AppState>, Caller(p): Caller, Path(id): Path<String>) -> V2Result<Response> {
    d2::require_view_documents(&p)?;
    d2::ensure(&state).await?;
    let id: i64 = id.parse().map_err(|_| V2Error(DomainError::NotFound))?;
    if !store::delete_reminder(state.db(), p.user_id, id).await.map_err(tl)? {
        return Err(V2Error(DomainError::NotFound));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn config(State(state): State<AppState>, Caller(p): Caller) -> V2Result<Response> {
    d2::require_view_documents(&p)?;
    d2::ensure(&state).await?;
    let all = store::custom_fields(state.db()).await.map_err(tl)?;
    let (kinds, warnings) = fields::resolve(&all, d2::setting(pl_timeline::SETTING_FIELDS).as_deref());
    let ids: Vec<i64> = all.iter().map(|f| f.id).collect();
    let u = store::uuids(state.db(), store::KIND_CUSTOM_FIELD, &ids).await.map_err(tl)?;
    let show = d2::visibility(&p)?.custom_field_names;
    let fields: Vec<Value> = all
        .iter()
        .filter(|f| f.data_type == "date" || f.data_type == "monetary")
        .map(|f| {
            let m = kinds.dates.get(&f.id);
            json!({
                "uuid": u.get(&f.id),
                "legacy_id": f.id,
                "name": if show { Some(&f.name) } else { None },
                "data_type": f.data_type,
                "kind": m.map(|x| x.0.as_str()).or_else(|| kinds.amount.contains(&f.id).then_some("amount")),
                "source": m.map(|x| x.1),
            })
        })
        .collect();
    let stats = store::extraction_stats(state.db()).await.map_err(tl)?;
    let running = refresh::backfill_running_any(state.db()).await.map_err(tl)?;
    Ok(json(&json!({
        "extract_dates": refresh::enabled(&d2::setting),
        "kinds": DateKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        "deadline_kinds": DateKind::DEADLINES.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        "fields": fields,
        "amount_fields": kinds.amount.iter().filter_map(|i| u.get(i)).collect::<Vec<_>>(),
        "setting_warnings": warnings,
        "extraction": stats,
        "backfill_running": running,
    })))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BackfillBody {
    #[serde(default)]
    force: bool,
}

pub async fn backfill(State(state): State<AppState>, Caller(p): Caller, body: axum::body::Bytes) -> V2Result<Response> {
    let u = d2::user(&p)?;
    if !(u.is_active && u.is_superuser) {
        return Err(V2Error(DomainError::Forbidden("Only superusers can start the backfill.".into())));
    }
    if !refresh::enabled(&d2::setting) {
        return Err(V2Error(DomainError::invalid(format!("Date extraction is disabled ({}).", pl_timeline::SETTING_EXTRACT))));
    }
    d2::ensure(&state).await?;
    let force = if body.iter().all(u8::is_ascii_whitespace) {
        false
    } else {
        serde_json::from_slice::<BackfillBody>(&body).map_err(|e| V2Error(DomainError::invalid(format!("Invalid JSON: {e}"))))?.force
    };
    // Take the cluster-wide advisory lock *before* answering: of two concurrent calls (in this or another
    // process) exactly one gets 202 "started", the other 409; the spawned task owns the lock.
    let Some(lock) = refresh::try_lock_backfill(state.db()).await.map_err(tl)? else {
        return Err(V2Error(DomainError::Conflict("A backfill is already running.".into())));
    };
    let pool = state.db().clone();
    tokio::spawn(async move {
        let dp = DateParse::from_lookup(d2::setting);
        if let Err(e) = refresh::backfill_locked(&pool, &dp, force, lock).await {
            tracing::error!(target: "paperrust.timeline", error = %e, "timeline backfill failed");
        }
    });
    Ok(json_with_rev(StatusCode::ACCEPTED, &json!({ "started": true }), None))
}
