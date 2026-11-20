//! Workflows with PaperRust extensions: overview, read, save together (ngx workflow +
//! "else" branch + AI condition in **one** transaction). The ngx part goes through the same validation and
//! the same write steps as `/api/workflows/` (pl-workflows `serializers`/`write`), the else branch
//! through the same action validation (`validate_actions`) and `upsert_actions`.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use pl_authz::User;
use pl_consumer::workflow_ext::{self, AiConditionDef};
use pl_core::AppState;
use pl_workflows::drf::Ctx;
use pl_workflows::schema::{T_ACTION, T_WORKFLOW};
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};

use crate::common::{self, ADD_WORKFLOW, CHANGE_WORKFLOW, Names, VIEW_WORKFLOW, Who};
use crate::error::{AErr, AResult, json as json_resp};
use crate::summary;

/// Workflow in ngx representation + else actions + AI condition (without summary).
pub struct Loaded {
    pub workflow: Value,
    pub else_actions: Vec<Value>,
    pub ai: Option<AiConditionDef>,
}

/// Load (`None` = does not exist).
pub async fn load(conn: &mut PgConnection, id: i64) -> AResult<Option<Loaded>> {
    let Some(row) = pl_workflows::load::get_row(conn, T_WORKFLOW, id).await? else { return Ok(None) };
    let wf = pl_workflows::load::render_workflows(conn, &[row], true).await?.into_iter().next().map(Value::Object).unwrap_or(Value::Null);
    let else_ids = workflow_ext::else_action_ids(conn, id).await?;
    let mut rows = pl_workflows::load::rows_by_ids(conn, T_ACTION, &else_ids).await?;
    rows.sort_by_key(|r| else_ids.iter().position(|x| Some(*x) == r.get("id").and_then(Value::as_i64)).unwrap_or(usize::MAX));
    let else_actions: Vec<Value> = pl_workflows::load::render_actions(conn, &rows).await?.into_iter().map(Value::Object).collect();
    let ai = workflow_ext::load_ai_conditions(conn, &[id]).await?.remove(&id);
    Ok(Some(Loaded { workflow: wf, else_actions, ai }))
}

/// Response shape `{"workflow","else_actions","ai_condition","ai","summary"}`.
pub async fn render(conn: &mut PgConnection, user: &User, l: &Loaded) -> AResult<Value> {
    let summary = summary::summarize(conn, user, &l.workflow, &l.else_actions, l.ai.as_ref()).await?;
    let (available, enabled) = summary::ai_status().await;
    Ok(json!({
        "workflow": l.workflow,
        "else_actions": l.else_actions,
        "ai_condition": l.ai,
        "ai": {"available": available, "enabled": enabled},
        "summary": summary,
    }))
}

/// GET /workflows – overview: all workflows (order `order`, id) with summary, "runs this
/// week" and last run.
pub async fn list(State(st): State<AppState>, who: Who) -> Result<Response, AErr> {
    let user = who.require(&[VIEW_WORKFLOW])?;
    workflow_ext::ensure_pool(st.db()).await?;
    let mut conn = st.db().acquire().await?;
    let rows = pl_workflows::load::fetch_rows(
        &mut conn,
        pl_workflows::load::select_rows(T_WORKFLOW)
            .order_by((sea_query::Alias::new(T_WORKFLOW), sea_query::Alias::new("order")), sea_query::Order::Asc)
            .order_by((sea_query::Alias::new(T_WORKFLOW), sea_query::Alias::new("id")), sea_query::Order::Asc),
    )
    .await?;
    let wfs: Vec<Value> = pl_workflows::load::render_workflows(&mut conn, &rows, true).await?.into_iter().map(Value::Object).collect();
    let ids: Vec<i64> = wfs.iter().filter_map(|w| w.get("id").and_then(Value::as_i64)).collect();
    let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    // Else actions of all workflows
    let links = sqlx::query("SELECT workflow_id, workflowaction_id FROM paperrust_workflow_else_action WHERE workflow_id = ANY($1)")
        .bind(&ids32)
        .fetch_all(&mut *conn)
        .await?;
    let mut else_ids: Vec<(i64, i64)> = Vec::new();
    for r in links {
        else_ids.push((i64::from(r.try_get::<i32, _>(0)?), i64::from(r.try_get::<i32, _>(1)?)));
    }
    let all_else: Vec<i64> = else_ids.iter().map(|(_, a)| *a).collect();
    let mut else_rows = pl_workflows::load::rows_by_ids(&mut conn, T_ACTION, &all_else).await?;
    else_rows.sort_by_key(|r| (r.get("order").and_then(Value::as_i64).unwrap_or(0), r.get("id").and_then(Value::as_i64).unwrap_or(0)));
    let else_rendered: Vec<Value> = pl_workflows::load::render_actions(&mut conn, &else_rows).await?.into_iter().map(Value::Object).collect();
    let mut ai = workflow_ext::load_ai_conditions(&mut conn, &ids).await?;
    // Runs this week (from Monday 00:00 in PAPERLESS_TIME_ZONE) and last run
    let week_start = week_start_utc(*st.tz(), chrono::Utc::now());
    let counts = sqlx::query(
        "SELECT workflow_id, count(*) FILTER (WHERE run_at >= $2 AND result <> 'skipped'), max(id) \
         FROM paperrust_workflow_run WHERE workflow_id = ANY($1) GROUP BY workflow_id",
    )
    .bind(&ids32)
    .bind(week_start)
    .fetch_all(&mut *conn)
    .await?;
    let mut week: std::collections::HashMap<i64, (i64, Option<i64>)> = std::collections::HashMap::new();
    for r in counts {
        week.insert(i64::from(r.try_get::<i32, _>(0)?), (r.try_get::<i64, _>(1)?, r.try_get::<Option<i64>, _>(2)?));
    }
    let last_ids: Vec<i64> = week.values().filter_map(|(_, l)| *l).collect();
    let last_rows = sqlx::query("SELECT id, workflow_id, run_at, result, code FROM paperrust_workflow_run WHERE id = ANY($1)")
        .bind(&last_ids)
        .fetch_all(&mut *conn)
        .await?;
    let mut last: std::collections::HashMap<i64, Value> = std::collections::HashMap::new();
    for r in last_rows {
        let at: chrono::DateTime<chrono::Utc> = r.try_get(2)?;
        last.insert(
            i64::from(r.try_get::<i32, _>(1)?),
            json!({"id": r.try_get::<i64, _>(0)?, "at": at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true), "result": r.try_get::<String, _>(3)?, "code": r.try_get::<String, _>(4)?}),
        );
    }
    let mut names = Names::default();
    for w in &wfs {
        let id = w.get("id").and_then(Value::as_i64).unwrap_or(0);
        let mine: Vec<Value> = else_of(&else_ids, &else_rendered, id);
        summary::collect(&mut names, w, &mine);
        summary::collect_ai(&mut names, ai.get(&id));
    }
    names.resolve(&mut conn, user).await?;
    let state = summary::ai_status().await;
    let mut items = Vec::with_capacity(wfs.len());
    for w in &wfs {
        let id = w.get("id").and_then(Value::as_i64).unwrap_or(0);
        let mine = else_of(&else_ids, &else_rendered, id);
        let cond = ai.remove(&id);
        let (n_week, _) = week.get(&id).copied().unwrap_or((0, None));
        items.push(json!({
            "id": id,
            "name": w.get("name").cloned().unwrap_or(Value::Null),
            "order": w.get("order").cloned().unwrap_or(Value::Null),
            "enabled": w.get("enabled").cloned().unwrap_or(Value::Null),
            "runs_this_week": n_week,
            "last_run": last.remove(&id),
            "has_else": !mine.is_empty(),
            "summary": summary::build(&names, w, &mine, cond.as_ref(), state),
        }));
    }
    Ok(json_resp(StatusCode::OK, &json!({"items": items, "week_start": week_start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)})))
}

fn else_of(links: &[(i64, i64)], rendered: &[Value], wf: i64) -> Vec<Value> {
    rendered
        .iter()
        .filter(|a| {
            let aid = a.get("id").and_then(Value::as_i64).unwrap_or(0);
            links.iter().any(|(w, x)| *w == wf && *x == aid)
        })
        .cloned()
        .collect()
}

/// Monday 00:00 of the current week in `tz`, as UTC.
pub fn week_start_utc(tz: chrono_tz::Tz, now: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    use chrono::{Datelike, TimeZone};
    let local = now.with_timezone(&tz);
    let monday = local.date_naive() - chrono::Duration::days(i64::from(local.weekday().num_days_from_monday()));
    let midnight = monday.and_hms_opt(0, 0, 0).unwrap_or_default();
    tz.from_local_datetime(&midnight).earliest().map(|d| d.with_timezone(&chrono::Utc)).unwrap_or(now)
}

/// GET /workflows/{id}
pub async fn get_one(State(st): State<AppState>, who: Who, Path(id): Path<String>) -> Result<Response, AErr> {
    let user = who.require(&[VIEW_WORKFLOW])?;
    let id = common::path_id(&id)?;
    workflow_ext::ensure_pool(st.db()).await?;
    let mut conn = st.db().acquire().await?;
    let l = load(&mut conn, id).await?.ok_or(AErr::NotFound)?;
    Ok(json_resp(StatusCode::OK, &render(&mut conn, user, &l).await?))
}

/// POST /workflows
pub async fn create(State(st): State<AppState>, who: Who, headers: HeaderMap, body: Bytes) -> Result<Response, AErr> {
    let user = who.require(&[ADD_WORKFLOW])?;
    let data = common::body_object(&headers, &body)?;
    workflow_ext::ensure_pool(st.db()).await?;
    let mut tx = st.db().begin().await?;
    let id = save(&mut tx, data, None, false).await?;
    tx.commit().await?;
    let mut conn = st.db().acquire().await?;
    let l = load(&mut conn, id).await?.ok_or_else(|| AErr::Internal("saved workflow vanished".into()))?;
    Ok(json_resp(StatusCode::CREATED, &render(&mut conn, user, &l).await?))
}

/// PUT/PATCH /workflows/{id}
pub async fn update(st: AppState, who: Who, id: String, headers: HeaderMap, body: Bytes, partial: bool) -> Result<Response, AErr> {
    let user = who.require(&[CHANGE_WORKFLOW])?;
    let id = common::path_id(&id)?;
    let data = common::body_object(&headers, &body)?;
    workflow_ext::ensure_pool(st.db()).await?;
    let mut tx = st.db().begin().await?;
    if pl_workflows::load::get_row(&mut tx, T_WORKFLOW, id).await?.is_none() {
        return Err(AErr::NotFound);
    }
    save(&mut tx, data, Some(id), partial).await?;
    tx.commit().await?;
    let mut conn = st.db().acquire().await?;
    let l = load(&mut conn, id).await?.ok_or(AErr::NotFound)?;
    Ok(json_resp(StatusCode::OK, &render(&mut conn, user, &l).await?))
}

pub async fn put(State(st): State<AppState>, who: Who, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> Result<Response, AErr> {
    update(st, who, id, headers, body, false).await
}

pub async fn patch(State(st): State<AppState>, who: Who, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> Result<Response, AErr> {
    update(st, who, id, headers, body, true).await
}

/// Take extension fields from the body (the rest is the ngx workflow).
pub struct Extensions {
    /// `None` = not in the body (for PATCH: leave unchanged).
    pub else_actions: Option<Value>,
    /// `None` = not in the body; `Some(None)` = delete.
    pub ai_condition: Option<Option<Value>>,
}

pub fn split_extensions(data: &mut Map<String, Value>) -> Extensions {
    let else_actions = data.remove("else_actions");
    let ai_condition = data.remove("ai_condition").map(|v| if v.is_null() { None } else { Some(v) });
    Extensions { else_actions, ai_condition }
}

/// Save in the caller transaction (create: `id = None`). Returns: workflow id.
pub async fn save(tx: &mut PgConnection, mut data: Map<String, Value>, id: Option<i64>, partial: bool) -> AResult<i64> {
    let ext = split_extensions(&mut data);
    let input = pl_workflows::serializers::validate_workflow(tx, Ctx { partial }, &Value::Object(data), id).await?;
    let wf_id = match id {
        None => pl_workflows::write::workflow_create(tx, &input).await?,
        Some(id) => {
            let row = pl_workflows::load::get_row(tx, T_WORKFLOW, id).await?.ok_or(AErr::NotFound)?;
            pl_workflows::write::workflow_update(tx, &row, &input).await?
        }
    };
    save_extensions(tx, wf_id, ext, id.is_none() || !partial).await?;
    Ok(wf_id)
}

/// Save else branch and AI condition. `full` (create/PUT): missing fields = clear.
pub async fn save_extensions(tx: &mut PgConnection, wf_id: i64, ext: Extensions, full: bool) -> AResult<()> {
    workflow_ext::ensure_conn(tx).await?;
    let wf32 = i32::try_from(wf_id).map_err(|_| AErr::NotFound)?;
    let else_v = match ext.else_actions {
        Some(v) => Some(v),
        None if full => Some(Value::Array(Vec::new())),
        None => None,
    };
    if let Some(v) = else_v {
        let attrs = match pl_workflows::serializers::validate_actions(tx, Some(&v)).await {
            Ok(a) => a,
            Err(pl_workflows::drf::VErr::Detail(d)) => {
                let mut fields = Map::new();
                fields.insert("else_actions".into(), d);
                return Err(AErr::Invalid { message: "Invalid input.".into(), fields });
            }
            Err(e) => return Err(e.into()),
        };
        let old = workflow_ext::else_action_ids(tx, wf_id).await?;
        // only accept ids if they belong to this else branch (do not hijack foreign actions)
        let attrs: Vec<pl_workflows::serializers::Attrs> = attrs
            .into_iter()
            .map(|mut a| {
                if a.get("id").and_then(Value::as_i64).is_some_and(|i| !old.contains(&i)) {
                    a.remove("id");
                }
                a
            })
            .collect();
        let new_ids = pl_workflows::write::upsert_actions(tx, &attrs, false).await?;
        sqlx::query("DELETE FROM paperrust_workflow_else_action WHERE workflow_id = $1").bind(wf32).execute(&mut *tx).await?;
        let ids32: Vec<i32> = new_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
        sqlx::query("INSERT INTO paperrust_workflow_else_action (workflow_id, workflowaction_id) SELECT $1, x FROM unnest($2::int[]) x ON CONFLICT DO NOTHING")
            .bind(wf32)
            .bind(&ids32)
            .execute(&mut *tx)
            .await?;
        let gone: Vec<i64> = old.into_iter().filter(|o| !new_ids.contains(o)).collect();
        if !gone.is_empty() {
            pl_workflows::write::delete_actions(tx, &gone).await?;
        }
    }
    let ai_v = match ext.ai_condition {
        Some(v) => Some(v),
        None if full => Some(None),
        None => None,
    };
    if let Some(v) = ai_v {
        let cond = match v {
            None => None,
            Some(v) => Some(parse_ai_condition(tx, v).await?),
        };
        let json_v = match &cond {
            Some(c) => serde_json::to_value(c).map_err(|e| AErr::Internal(e.to_string()))?,
            None => Value::Null,
        };
        sqlx::query(
            "INSERT INTO paperrust_workflow_ext (workflow_id, ai_condition, updated_at) VALUES ($1, $2, now()) \
             ON CONFLICT (workflow_id) DO UPDATE SET ai_condition = EXCLUDED.ai_condition, updated_at = now()",
        )
        .bind(wf32)
        .bind(if json_v.is_null() { None } else { Some(json_v) })
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

async fn parse_ai_condition(conn: &mut PgConnection, v: Value) -> AResult<AiConditionDef> {
    let field_err = |m: &str| {
        let mut f = Map::new();
        f.insert("ai_condition".into(), json!([m]));
        AErr::Invalid { message: "Invalid input.".into(), fields: f }
    };
    let c: AiConditionDef = serde_json::from_value(v).map_err(|_| field_err("Expected {\"field\", \"object_id\", \"min_confidence\"}."))?;
    c.validate().map_err(|e| field_err(&e))?;
    let sql = format!("SELECT EXISTS (SELECT 1 FROM {} WHERE id = $1)", c.object_table());
    let ok: bool = sqlx::query_scalar(&sql).bind(i32::try_from(c.object_id).unwrap_or(-1)).fetch_one(&mut *conn).await?;
    if !ok {
        return Err(field_err(&format!("Invalid pk \"{}\" - object does not exist.", c.object_id)));
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn week_start_is_monday_midnight_local() {
        use chrono::TimeZone;
        let tz = chrono_tz::Europe::Zurich;
        // Wednesday 2026-11-18 10:00 UTC → Monday 2026-11-16 00:00 Zurich = 2026-11-15 23:00 UTC
        let now = chrono::Utc.with_ymd_and_hms(2026, 11, 18, 10, 0, 0).unwrap();
        assert_eq!(week_start_utc(tz, now), chrono::Utc.with_ymd_and_hms(2026, 11, 15, 23, 0, 0).unwrap());
        // Monday just after midnight local
        let now = chrono::Utc.with_ymd_and_hms(2026, 11, 15, 23, 30, 0).unwrap();
        assert_eq!(week_start_utc(tz, now), chrono::Utc.with_ymd_and_hms(2026, 11, 15, 23, 0, 0).unwrap());
    }

    #[test]
    fn extensions_are_split() {
        let mut m: Map<String, Value> = serde_json::from_value(json!({"name": "x", "else_actions": [], "ai_condition": null})).unwrap();
        let e = split_extensions(&mut m);
        assert_eq!(e.else_actions, Some(json!([])));
        assert_eq!(e.ai_condition, Some(None));
        assert!(!m.contains_key("else_actions"));
        let mut m: Map<String, Value> = serde_json::from_value(json!({"name": "x"})).unwrap();
        let e = split_extensions(&mut m);
        assert!(e.else_actions.is_none() && e.ai_condition.is_none());
    }
}
