//! `/rs/api/v2/asn/…` – ASN label generator (PaperRust extension, crate `pl-labels`,
//! contract docs/contract/pl-labels.md).
//!
//! | Method | Path | Response |
//! |---|---|---|
//! | GET | `/asn/presets` | label sheets |
//! | GET | `/asn/config` | prefix, separator string, barcode switches, next ASN, hints, number-range switch |
//! | PUT | `/asn/config` | change number-range switch + name of the storage-location field (superuser) |
//! | GET/POST | `/asn/ranges` | read / create number ranges (create: superuser) |
//! | GET/PUT/PATCH/DELETE | `/asn/ranges/{id}` | one number range (change/delete: superuser) |
//! | POST | `/asn/preview?page=N` | SVG of one page + collisions + hints |
//! | POST | `/asn/labels?allow_collisions=&record=` | PDF (stores the range in `paperrust_asn_print`) |
//! | GET | `/asn/printed?limit=` | printed ranges |
//! | DELETE | `/asn/printed/{id}` | release a range (creator or superuser) |
//! | POST | `/asn/separator` | separator-sheet PDF |
//!
//! Permission: `documents.change_document` (like editing documents; v1 checks model permissions via
//! `PaperlessModelPermissions`, the ASN is assigned via PATCH of a document). Inactive users
//! have no permission (`has_perm`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use pl_core::AppState;
use pl_domain::{DomainError, Principal};
use pl_labels::ranges::{self, ConfigUpdate, RangeConfig, RangeInput, RangePatch, RangeStatus};
use pl_labels::store::{self, Collisions, NextAsn, Reserve};
use pl_labels::{LabelError, LabelJob, SeparatorJob, sheets};
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{V2Error, V2Result};
use crate::http::{Caller, V2Json, json, truthy};

pub const PERM_CHANGE_DOCUMENT: &str = "documents.change_document";

pub fn routes(r: axum::Router<AppState>) -> axum::Router<AppState> {
    r.route("/asn/presets", get(presets))
        .route("/asn/config", get(config).put(put_config))
        .route("/asn/ranges", get(list_ranges).post(create_range))
        .route("/asn/ranges/{id}", get(get_range).put(put_range).patch(patch_range).delete(delete_range))
        .route("/asn/preview", post(preview))
        .route("/asn/labels", post(labels))
        .route("/asn/printed", get(printed))
        .route("/asn/printed/{id}", delete(release))
        .route("/asn/separator", post(separator))
}

struct Perm {
    superuser: bool,
}

fn require(p: &Principal) -> V2Result<Perm> {
    let user = p.ext.downcast_ref::<pl_store::AuthExt>().and_then(|e| e.ctx.user.clone());
    let Some(u) = user else { return Err(V2Error(DomainError::Unauthenticated)) };
    if !u.has_perm(PERM_CHANGE_DOCUMENT) {
        return Err(V2Error(DomainError::Forbidden("You do not have permission to perform this action.".into())));
    }
    Ok(Perm { superuser: u.is_superuser && u.is_active })
}

fn label_err(e: LabelError) -> V2Error {
    match e {
        LabelError::Invalid(f, m) => V2Error(DomainError::field(&f, m)),
        LabelError::Db(e) => V2Error(DomainError::internal(e)),
    }
}

static TABLE_READY: AtomicBool = AtomicBool::new(false);

/// Create `paperrust_asn_print` if `paperrust migrate` has not run yet (expand-only, idempotent).
async fn ensure_table(state: &AppState) -> V2Result<()> {
    if !TABLE_READY.load(Ordering::Relaxed) {
        store::migrate(state.db()).await.map_err(label_err)?;
        TABLE_READY.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Effective barcode settings of the consumer (environment/UI + ApplicationConfiguration,
/// like `BarcodeConfig.__post_init__`: switches apply when not NULL; texts when not empty).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BarcodeInfo {
    pub barcodes_enabled: bool,
    pub asn_barcode_enabled: bool,
    pub retain_split_pages: bool,
    pub prefix: String,
    pub separator_string: String,
}

fn env_bool(key: &str) -> bool {
    pl_core::config::var(key).is_some_and(|v| matches!(v.trim().to_lowercase().as_str(), "yes" | "y" | "1" | "t" | "true"))
}

pub async fn barcode_info(state: &AppState) -> V2Result<BarcodeInfo> {
    let mut i = BarcodeInfo {
        barcodes_enabled: env_bool("PAPERLESS_CONSUMER_ENABLE_BARCODES"),
        asn_barcode_enabled: env_bool("PAPERLESS_CONSUMER_ENABLE_ASN_BARCODE"),
        retain_split_pages: env_bool("PAPERLESS_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES"),
        prefix: pl_core::config::var("PAPERLESS_CONSUMER_ASN_BARCODE_PREFIX").unwrap_or_else(|| "ASN".into()),
        separator_string: pl_core::config::var("PAPERLESS_CONSUMER_BARCODE_STRING").unwrap_or_else(|| "PATCHT".into()),
    };
    type Row = (Option<bool>, Option<bool>, Option<bool>, Option<String>, Option<String>);
    let row: Option<Row> = sqlx::query_as(
        "SELECT barcodes_enabled, barcode_enable_asn, barcode_retain_split_pages, barcode_asn_prefix, barcode_string
           FROM paperless_applicationconfiguration ORDER BY id LIMIT 1",
    )
    .fetch_optional(state.db())
    .await
    .map_err(DomainError::internal)?;
    if let Some((be, ae, rs, prefix, sep)) = row {
        i.barcodes_enabled = be.unwrap_or(i.barcodes_enabled);
        i.asn_barcode_enabled = ae.unwrap_or(i.asn_barcode_enabled);
        i.retain_split_pages = rs.unwrap_or(i.retain_split_pages);
        if let Some(p) = prefix.filter(|p| !p.is_empty()) {
            i.prefix = p;
        }
        if let Some(s) = sep.filter(|s| !s.is_empty()) {
            i.separator_string = s;
        }
    }
    Ok(i)
}

/// Setup hints (for the UI).
fn hints(i: &BarcodeInfo) -> Vec<Value> {
    let mut h = Vec::new();
    if !i.asn_barcode_enabled {
        h.push(json!({"code": "asn_barcode_disabled", "message": "ASN barcodes are not read: enable PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE (Configuration → Barcodes → Enable ASN)."}));
    }
    if !i.barcodes_enabled {
        h.push(json!({"code": "barcode_split_disabled", "message": "Batches are not split at labels or separator sheets: enable PAPERRUST_CONSUMER_ENABLE_BARCODES (Configuration → Barcodes → Enable barcodes)."}));
    }
    if pl_labels::validate_prefix(&i.prefix).is_err() {
        h.push(json!({"code": "prefix_unprintable", "message": "The configured ASN prefix cannot be printed on labels (only printable ASCII, no leading/trailing spaces)."}));
    }
    h
}

async fn next(state: &AppState) -> V2Result<NextAsn> {
    let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
    store::next_asn(&mut c).await.map_err(label_err)
}

async fn collisions(state: &AppState, first: u64, last: u64) -> V2Result<Collisions> {
    let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
    store::collisions(&mut c, first, last).await.map_err(label_err)
}

pub async fn presets(Caller(p): Caller) -> V2Result<Response> {
    require(&p)?;
    Ok(json(&json!({ "items": sheets::presets(), "default": sheets::DEFAULT_PRESET })))
}

async fn range_config(state: &AppState) -> V2Result<RangeConfig> {
    let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
    ranges::get_config(&mut c).await.map_err(label_err)
}

fn require_superuser(perm: &Perm) -> V2Result<()> {
    if perm.superuser { Ok(()) } else { Err(V2Error(DomainError::Forbidden("Only superusers can change number ranges.".into()))) }
}

pub async fn config(State(state): State<AppState>, Caller(p): Caller) -> V2Result<Response> {
    let perm = require(&p)?;
    ensure_table(&state).await?;
    config_body(&state, &perm).await.map(|v| json(&v))
}

/// `PUT /asn/config`: `{ranges_enabled?, location_field_name?}` (superuser) → like GET.
pub async fn put_config(State(state): State<AppState>, Caller(p): Caller, V2Json(upd): V2Json<ConfigUpdate>) -> V2Result<Response> {
    let perm = require(&p)?;
    require_superuser(&perm)?;
    ensure_table(&state).await?;
    let cur = range_config(&state).await?;
    let new = upd.apply(&cur).map_err(label_err)?;
    let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
    ranges::set_config(&mut c, &new).await.map_err(label_err)?;
    drop(c);
    if new != cur {
        tracing::info!(target: "paperrust.asn_labels", "user {} set ASN number ranges enabled={} field={:?}", p.username, new.ranges_enabled, new.location_field_name);
    }
    config_body(&state, &perm).await.map(|v| json(&v))
}

async fn config_body(state: &AppState, perm: &Perm) -> V2Result<Value> {
    let info = barcode_info(state).await?;
    let next = next(state).await?;
    let rc = range_config(state).await?;
    Ok(json!({
        "barcode": info,
        "split_on_asn": info.barcodes_enabled && info.asn_barcode_enabled,
        "next_asn": next,
        "max_asn": pl_labels::ASN_MAX_STORABLE,
        "archive_serial_number_max": pl_labels::ARCHIVE_SERIAL_NUMBER_MAX,
        "default_digits": pl_labels::DEFAULT_DIGITS,
        "default_preset": sheets::DEFAULT_PRESET,
        "max_labels": pl_labels::MAX_LABELS,
        "max_sheets": pl_labels::MAX_PAGES,
        "hints": hints(&info),
        "ranges_enabled": rc.ranges_enabled,
        "location_field_name": rc.location_field_name,
        "can_manage_ranges": perm.superuser,
    }))
}

/// Number range of a job (only when the switch is on).
async fn job_range(state: &AppState, job: &LabelJob) -> V2Result<Option<RangeStatus>> {
    let Some(id) = job.range else { return Ok(None) };
    if !range_config(state).await?.ranges_enabled {
        return Err(V2Error(DomainError::field("range", "Number ranges are switched off.")));
    }
    let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
    match ranges::get(&mut c, id).await.map_err(label_err)? {
        Some(r) => Ok(Some(r)),
        None => Err(V2Error(DomainError::field("range", "Unknown number range."))),
    }
}

async fn resolve(state: &AppState, job: &LabelJob) -> V2Result<(pl_labels::Resolved, BarcodeInfo, Option<RangeStatus>)> {
    let info = barcode_info(state).await?;
    let range = job_range(state, job).await?;
    let start = match (job.start, &range) {
        (Some(s), _) => s,
        (None, Some(rs)) => match rs.next_asn {
            Some(n) => u64::try_from(n).unwrap_or(0),
            None => return Err(range_exhausted(rs)),
        },
        (None, None) => u64::try_from(next(state).await?.suggested).unwrap_or(1),
    };
    let mut r = job.resolve(&info.prefix, start).map_err(label_err)?;
    if let Some(rs) = &range {
        let (f, l) = (rs.range.first_asn as u64, rs.range.last_asn as u64);
        if r.start < f || r.start > l {
            return Err(V2Error(DomainError::field(
                "start",
                format!("The number {} lies outside the range {:?} ({f}–{l}).", r.start, rs.range.name),
            )));
        }
        if r.last() > l {
            return Err(V2Error(DomainError::field(
                "count",
                format!(
                    "The labels {}–{} do not fit into the range {:?} ({f}–{l}): only {} number(s) left from {}.",
                    r.start,
                    r.last(),
                    rs.range.name,
                    l - r.start + 1,
                    r.start
                ),
            )));
        }
        r.caption = Some(ranges::caption(&rs.range));
    }
    Ok((r, info, range))
}

fn range_exhausted(rs: &RangeStatus) -> V2Error {
    V2Error(DomainError::field(
        "range",
        format!(
            "The range {:?} ({}–{}) is exhausted: all numbers are used or printed. Extend it or choose another range.",
            rs.range.name, rs.range.first_asn, rs.range.last_asn
        ),
    ))
}

// ---- Number ranges -------------------------------------------------------------------------

fn parse_id(id: &str) -> V2Result<i64> {
    id.parse().map_err(|_| V2Error(DomainError::NotFound))
}

/// `GET /asn/ranges` → `{enabled, items}` (permission like the other ASN routes).
pub async fn list_ranges(State(state): State<AppState>, Caller(p): Caller) -> V2Result<Response> {
    require(&p)?;
    ensure_table(&state).await?;
    let rc = range_config(&state).await?;
    let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
    let items = ranges::list(&mut c).await.map_err(label_err)?;
    Ok(json(&json!({ "enabled": rc.ranges_enabled, "items": items })))
}

pub async fn get_range(State(state): State<AppState>, Caller(p): Caller, Path(id): Path<String>) -> V2Result<Response> {
    require(&p)?;
    ensure_table(&state).await?;
    let id = parse_id(&id)?;
    let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
    match ranges::get(&mut c, id).await.map_err(label_err)? {
        Some(r) => Ok(json(&r)),
        None => Err(V2Error(DomainError::NotFound)),
    }
}

async fn save_range(state: &AppState, p: &Principal, id: Option<i64>, input: &RangeInput) -> V2Result<RangeStatus> {
    let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
    let Some(r) = ranges::save(&mut c, id, input).await.map_err(label_err)? else {
        return Err(V2Error(DomainError::NotFound));
    };
    tracing::info!(target: "paperrust.asn_labels", "user {} saved ASN number range {} ({} {}–{})", p.username, r.range.id, r.range.code, r.range.first_asn, r.range.last_asn);
    Ok(r)
}

pub async fn create_range(State(state): State<AppState>, Caller(p): Caller, V2Json(input): V2Json<RangeInput>) -> V2Result<Response> {
    require_superuser(&require(&p)?)?;
    ensure_table(&state).await?;
    let r = save_range(&state, &p, None, &input).await?;
    Ok((StatusCode::CREATED, axum::Json(r)).into_response())
}

pub async fn put_range(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(id): Path<String>,
    V2Json(input): V2Json<RangeInput>,
) -> V2Result<Response> {
    require_superuser(&require(&p)?)?;
    ensure_table(&state).await?;
    let id = parse_id(&id)?;
    Ok(json(&save_range(&state, &p, Some(id), &input).await?))
}

pub async fn patch_range(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(id): Path<String>,
    V2Json(patch): V2Json<RangePatch>,
) -> V2Result<Response> {
    require_superuser(&require(&p)?)?;
    ensure_table(&state).await?;
    let id = parse_id(&id)?;
    let cur = {
        let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
        ranges::get(&mut c, id).await.map_err(label_err)?
    };
    let Some(cur) = cur else { return Err(V2Error(DomainError::NotFound)) };
    let input = patch.merge(&cur.range);
    Ok(json(&save_range(&state, &p, Some(id), &input).await?))
}

pub async fn delete_range(State(state): State<AppState>, Caller(p): Caller, Path(id): Path<String>) -> V2Result<Response> {
    require_superuser(&require(&p)?)?;
    ensure_table(&state).await?;
    let id = parse_id(&id)?;
    let mut c = state.db().acquire().await.map_err(DomainError::internal)?;
    if !ranges::delete(&mut c, id).await.map_err(label_err)? {
        return Err(V2Error(DomainError::NotFound));
    }
    tracing::info!(target: "paperrust.asn_labels", "user {} deleted ASN number range {id}", p.username);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn preview(
    State(state): State<AppState>,
    Caller(p): Caller,
    Query(q): Query<HashMap<String, String>>,
    V2Json(job): V2Json<LabelJob>,
) -> V2Result<Response> {
    require(&p)?;
    ensure_table(&state).await?;
    let page: usize = match q.get("page") {
        Some(v) => v.parse().map_err(|_| V2Error(DomainError::field("page", "Expected a page number (0-based).")))?,
        None => 0,
    };
    let (r, info, range) = resolve(&state, &job).await?;
    if page as u32 >= r.pages() {
        return Err(V2Error(DomainError::field("page", format!("The job has {} page(s).", r.pages()))));
    }
    let (svg, mut warnings) = r.preview_svg(page).map_err(label_err)?;
    if r.prefix != info.prefix {
        warnings.push(pl_labels::render::Warning {
            code: "prefix_differs",
            message: format!(
                "The prefix {:?} differs from the configured ASN prefix {:?} – PaperRust will not recognise these labels.",
                r.prefix, info.prefix
            ),
        });
    }
    let c = collisions(&state, r.start, r.last()).await?;
    let plan = r.plan();
    Ok(json(&json!({
        "svg": svg,
        "page": page,
        "pages": r.pages(),
        "per_sheet": r.sheet.per_sheet(),
        "count": r.count,
        "first_asn": r.start,
        "last_asn": r.last(),
        "first_content": r.content(r.start),
        "last_content": r.content(r.last()),
        "page_first_asn": plan.get(page).and_then(|s| s.first()).map(|s| s.1),
        "page_last_asn": plan.get(page).and_then(|s| s.last()).map(|s| s.1),
        "sheet": r.sheet,
        "skip": r.skip,
        "warnings": warnings,
        "collisions": c,
        "range": range,
    })))
}

fn pdf_response(bytes: Vec<u8>, filename: &str, extra: &[(&'static str, String)]) -> Response {
    let mut r = (StatusCode::OK, bytes).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    for (k, v) in extra {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(*k, v);
        }
    }
    r
}

fn collision_response(c: &Collisions) -> Response {
    let body = json!({"error": {
        "code": "asn_collision",
        "message": "Some of these numbers are already used by documents or were printed before. Choose another start number or confirm.",
        "collisions": c,
    }});
    (StatusCode::CONFLICT, axum::Json(body)).into_response()
}

pub async fn labels(
    State(state): State<AppState>,
    Caller(p): Caller,
    Query(q): Query<HashMap<String, String>>,
    V2Json(job): V2Json<LabelJob>,
) -> V2Result<Response> {
    require(&p)?;
    ensure_table(&state).await?;
    let allow = truthy(q.get("allow_collisions").map(String::as_str));
    // Default: store the range; `record=false` e.g. for test prints
    let record = q.get("record").is_none_or(|v| truthy(Some(v)));
    let (r, _, _) = resolve(&state, &job).await?;
    let pdf = r.pdf().map_err(label_err)?;
    let name = format!("asn-labels-{}-{}.pdf", r.content(r.start), r.content(r.last()))
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' { c } else { '_' })
        .collect::<String>();
    let mut extra = vec![("x-paperrust-asn-first", r.start.to_string()), ("x-paperrust-asn-last", r.last().to_string())];
    if record {
        match store::reserve(state.db(), &r, Some(p.user_id), allow).await.map_err(label_err)? {
            Reserve::Conflict(c) => return Ok(collision_response(&c)),
            Reserve::Recorded(range) => {
                tracing::info!(target: "paperrust.asn_labels", "user {} printed ASN labels {}–{} ({} labels, {})", p.username, r.start, r.last(), r.count, r.sheet.id);
                extra.push(("x-paperrust-print-id", range.id.to_string()));
            }
        }
    } else if !allow {
        let c = collisions(&state, r.start, r.last()).await?;
        if !c.is_empty() {
            return Ok(collision_response(&c));
        }
    }
    Ok(pdf_response(pdf, &name, &extra))
}

pub async fn printed(State(state): State<AppState>, Caller(p): Caller, Query(q): Query<HashMap<String, String>>) -> V2Result<Response> {
    require(&p)?;
    ensure_table(&state).await?;
    let limit = q.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(100);
    let items = store::list(state.db(), limit).await.map_err(label_err)?;
    Ok(json(&json!({ "items": items })))
}

pub async fn release(State(state): State<AppState>, Caller(p): Caller, Path(id): Path<String>) -> V2Result<Response> {
    let perm = require(&p)?;
    ensure_table(&state).await?;
    let id: i64 = id.parse().map_err(|_| V2Error(DomainError::NotFound))?;
    let Some(range) = store::get(state.db(), id).await.map_err(label_err)? else {
        return Err(V2Error(DomainError::NotFound));
    };
    if !perm.superuser && range.user_id.map(i64::from) != Some(p.user_id) {
        return Err(V2Error(DomainError::Forbidden("Only the creator or a superuser can release a printed range.".into())));
    }
    store::delete(state.db(), id).await.map_err(label_err)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn separator(State(state): State<AppState>, Caller(p): Caller, V2Json(job): V2Json<SeparatorJob>) -> V2Result<Response> {
    let perm = require(&p)?;
    let info = barcode_info(&state).await?;
    if job.value.as_ref().is_some_and(|v| *v != info.separator_string) && !perm.superuser {
        return Err(V2Error(DomainError::field("value", "Only superusers can print a separator value other than the configured one.")));
    }
    let pdf = job.pdf(&info.separator_string, info.retain_split_pages).map_err(label_err)?;
    Ok(pdf_response(pdf, "separator-sheet.pdf", &[]))
}
