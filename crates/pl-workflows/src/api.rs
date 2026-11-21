//! Handlers of the three ViewSets.
//! upstream: src/documents/views.py@v3.4.2 (WorkflowTriggerViewSet, WorkflowActionViewSet,
//! WorkflowViewSet), DRF 3.18.3 `mixins.{List,Retrieve,Create,Update,Destroy}ModelMixin`,
//! `generics.GenericAPIView.get_object`, `permissions.DjangoObjectPermissions`.
//!
//! Errors are ready-made HTTP responses (`Response` as `Err`); hence `result_large_err` is off.
#![allow(clippy::result_large_err)]

use crate::drf::{Ctx, VErr};
use crate::load;
use crate::pyfmt::{self, PyInt};
use crate::schema::*;
use crate::serializers;
use crate::write;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use pl_authz::{AuthUser, ObjectModel};
use pl_core::pagination::{self, PageRequest};
use pl_core::{ApiError, AppState, Drf, QueryParams, RequestContext};
use sea_query::{Alias, Order};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

type HR = Result<Response, Response>;

async fn conn(state: &AppState) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, Response> {
    state.db().acquire().await.map_err(db_err)
}

pub const M_TRIGGER: ObjectModel = pl_authz::models::WORKFLOW_TRIGGER;
pub const M_ACTION: ObjectModel = pl_authz::models::WORKFLOW_ACTION;
pub const M_WORKFLOW: ObjectModel = pl_authz::models::WORKFLOW;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Trigger,
    Action,
    Workflow,
}

impl Kind {
    fn model(self) -> &'static ObjectModel {
        match self {
            Kind::Trigger => &M_TRIGGER,
            Kind::Action => &M_ACTION,
            Kind::Workflow => &M_WORKFLOW,
        }
    }
    fn table(self) -> &'static str {
        self.model().table
    }
}

fn e<E: IntoResponse>(err: E) -> Response {
    err.into_response()
}

fn verr(err: VErr) -> Response {
    match err {
        VErr::Detail(d) => ApiError::validation(d).into_response(),
        VErr::Server(s) => ApiError::internal(s).into_response(),
    }
}

fn db_err(err: sqlx::Error) -> Response {
    ApiError::from(err).into_response()
}

/// `get_object_or_404(queryset, pk=<url>)`: not an int → "Not found.", int4 overflow/missing →
/// "No <Model> matches the given query.".
async fn get_object(db: &mut PgConnection, kind: Kind, pk: &str) -> Result<(i64, Map<String, Value>), Response> {
    let id = match pyfmt::parse_int(pk) {
        PyInt::Invalid => return Err(e(ApiError::not_found())),
        PyInt::Overflow(_) => None,
        PyInt::Ok(n) if !(-2_147_483_648..=2_147_483_647).contains(&n) => None,
        PyInt::Ok(n) => Some(n),
    };
    let not_found = || e(ApiError::not_found_model(kind.model().object_name));
    let Some(id) = id else { return Err(not_found()) };
    match load::get_row(db, kind.table(), id).await.map_err(db_err)? {
        Some(row) => Ok((id, row)),
        None => Err(not_found()),
    }
}

/// `request.data` (JSONParser; empty body → `{}`).
fn parse_body(headers: &HeaderMap, body: &Bytes) -> Result<Value, Response> {
    if body.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let ct = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if main != "application/json" {
        return Err(e(ApiError::unsupported_media_type(ct)));
    }
    serde_json::from_slice::<Value>(body)
        .map_err(|err| e(ApiError::json_parse_error(&crate::pyjson::decode_error(body).unwrap_or_else(|| err.to_string()))))
}

/// `Serializer.is_valid` with `data=None` → `{"non_field_errors": ["No data provided"]}`.
fn reject_null(data: &Value) -> Result<(), Response> {
    if data.is_null() {
        return Err(e(ApiError::validation(json!({"non_field_errors": ["No data provided"]}))));
    }
    Ok(())
}

async fn render(db: &mut PgConnection, kind: Kind, rows: &[Map<String, Value>], prefetch: bool) -> Result<Vec<Value>, Response> {
    let out = match kind {
        Kind::Trigger => load::render_triggers(db, rows).await,
        Kind::Action => load::render_actions(db, rows).await,
        Kind::Workflow => load::render_workflows(db, rows, prefetch).await,
    }
    .map_err(db_err)?;
    Ok(out.into_iter().map(Value::Object).collect())
}

async fn render_one(db: &mut PgConnection, kind: Kind, id: i64, prefetch: bool) -> Result<Value, Response> {
    let row = load::get_row(db, kind.table(), id).await.map_err(db_err)?.ok_or_else(|| e(ApiError::internal("saved object vanished")))?;
    let mut v = render(db, kind, &[row], prefetch).await?;
    v.pop().ok_or_else(|| e(ApiError::internal("render failed")))
}

// ------------------------------------------------------------------ list / retrieve

pub async fn list(kind: Kind, state: AppState, ctx: RequestContext, auth: AuthUser, q: QueryParams) -> HR {
    auth.require_model_perms(kind.model()).map_err(e)?;
    let mut conn = conn(&state).await?;
    let db = &mut *conn;
    // Filter only on the workflow list (`WorkflowFilterSet`, see crate::filters); before the
    // page call like DRF `filter_queryset` → `paginate_queryset`.
    let conds = if kind == Kind::Workflow {
        crate::filters::workflow_conditions(&q, &pl_core::filters::FilterContext::new(&ctx, state.tz())).map_err(e)?
    } else {
        Vec::new()
    };
    let req = PageRequest::from_query(&q);
    let total = load::count_where(db, kind.table(), &conds).await.map_err(db_err)?;
    let info = req.resolve(total).map_err(e)?;
    let mut sel = load::select_rows(kind.table());
    for c in &conds {
        sel.and_where(c.clone());
    }
    if kind == Kind::Workflow {
        sel.order_by((Alias::new(T_WORKFLOW), Alias::new("order")), Order::Asc);
    }
    sel.limit(info.page_size).offset(info.offset());
    let rows = load::fetch_rows(db, &sel).await.map_err(db_err)?;
    let results = render(db, kind, &rows, true).await?;
    let all = if ctx.version().includes_all() {
        let ids = load::ids_in_order_where(db, kind.table(), kind == Kind::Workflow, &conds).await.map_err(db_err)?;
        Some(ids.into_iter().map(Value::from).collect())
    } else {
        None
    };
    Ok(Drf::ok(pagination::paginated_body(&ctx, &info, results, all)).into_response())
}

pub async fn retrieve(kind: Kind, state: AppState, auth: AuthUser, pk: String) -> HR {
    auth.require_model_perms(kind.model()).map_err(e)?;
    let mut conn = conn(&state).await?;
    let db = &mut *conn;
    let (_, row) = get_object(db, kind, &pk).await?;
    let mut v = render(db, kind, &[row], true).await?;
    Ok(Drf::ok(v.pop().unwrap_or(Value::Null)).into_response())
}

// ------------------------------------------------------------------ create

pub async fn create(kind: Kind, state: AppState, auth: AuthUser, headers: HeaderMap, body: Bytes) -> HR {
    auth.require_model_perms(kind.model()).map_err(e)?;
    let data = parse_body(&headers, &body)?;
    reject_null(&data)?;
    let mut conn = conn(&state).await?;
    let db = &mut *conn;
    let ctx = Ctx { partial: false };
    let id = match kind {
        Kind::Trigger => {
            let attrs = serializers::validate_trigger(db, ctx, &data, None).await.map_err(verr)?;
            write::model_create(db, true, &attrs).await.map_err(verr)?
        }
        Kind::Action => {
            let attrs = serializers::validate_action(db, ctx, &data).await.map_err(verr)?;
            write::model_create(db, false, &attrs).await.map_err(verr)?
        }
        Kind::Workflow => {
            let input = serializers::validate_workflow(db, ctx, &data, None).await.map_err(verr)?;
            write::workflow_create(db, &input).await.map_err(verr)?
        }
    };
    let v = render_one(db, kind, id, false).await?;
    Ok(Drf::created(v).into_response())
}

// ------------------------------------------------------------------ update

/// `partial_update` override of trigger/action viewset:
/// `"id" in request.data and str(request.data["id"]) != str(kwargs["pk"])`.
fn check_body_id(data: &Value, pk: &str) -> Result<(), Response> {
    let mismatch = match data {
        Value::Object(o) => match o.get("id") {
            Some(v) => pyfmt::py_str(v) != pk,
            None => false,
        },
        // `"id" in <list>` → Elementvergleich; danach `list["id"]` → TypeError (500)
        Value::Array(a) if a.iter().any(|v| v.as_str() == Some("id")) => {
            return Err(e(ApiError::internal("list indices must be integers or slices, not str")));
        }
        Value::Array(_) => false,
        // `"id" in <str>` → Teilstring; danach `str["id"]` → TypeError (500)
        Value::String(s) if s.contains("id") => {
            return Err(e(ApiError::internal("string indices must be integers")));
        }
        Value::String(_) => false,
        // `in` on int/float/bool/None → TypeError (500)
        _ => return Err(e(ApiError::internal("argument is not iterable"))),
    };
    if mismatch {
        return Err(e(ApiError::django_text(StatusCode::BAD_REQUEST, "ID in body does not match URL")));
    }
    Ok(())
}

pub async fn update(kind: Kind, partial: bool, state: AppState, auth: AuthUser, pk: String, headers: HeaderMap, body: Bytes) -> HR {
    auth.require_model_perms(kind.model()).map_err(e)?;
    let data = parse_body(&headers, &body)?;
    if partial && kind != Kind::Workflow {
        check_body_id(&data, &pk)?;
    }
    let mut conn = conn(&state).await?;
    let db = &mut *conn;
    let (id, row) = get_object(db, kind, &pk).await?;
    reject_null(&data)?;
    let ctx = Ctx { partial };
    let saved = match kind {
        Kind::Trigger => {
            let itype = row.get("type").and_then(Value::as_i64);
            let attrs = serializers::validate_trigger(db, ctx, &data, itype).await.map_err(verr)?;
            write::model_update(db, true, &row, &attrs).await.map_err(verr)?
        }
        Kind::Action => {
            let attrs = serializers::validate_action(db, ctx, &data).await.map_err(verr)?;
            write::model_update(db, false, &row, &attrs).await.map_err(verr)?
        }
        Kind::Workflow => {
            let input = serializers::validate_workflow(db, ctx, &data, Some(id)).await.map_err(verr)?;
            write::workflow_update(db, &row, &input).await.map_err(verr)?
        }
    };
    let v = render_one(db, kind, saved, false).await?;
    Ok(Drf::ok(v).into_response())
}

// ------------------------------------------------------------------ destroy

pub async fn destroy(kind: Kind, state: AppState, auth: AuthUser, pk: String) -> HR {
    auth.require_model_perms(kind.model()).map_err(e)?;
    let mut conn = conn(&state).await?;
    let db = &mut *conn;
    let (id, _) = get_object(db, kind, &pk).await?;
    match kind {
        Kind::Trigger => write::delete_triggers(db, &[id]).await,
        Kind::Action => write::delete_actions(db, &[id]).await,
        Kind::Workflow => write::delete_workflow(db, id).await,
    }
    .map_err(verr)?;
    Ok(pl_core::json::drf_no_content())
}

// ------------------------------------------------------------------ axum adapter

pub fn flatten(r: HR) -> Response {
    match r {
        Ok(r) | Err(r) => r,
    }
}

pub type PathPk = Path<String>;
pub type St = State<AppState>;
