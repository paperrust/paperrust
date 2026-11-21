//! Mail as trigger (docs/WORKFLOWS-NEU.md topic 11): a mail rule and the workflows with
//! ingestion trigger for this rule (`filter_mailrule`) as **one** flow. Internally it stays
//! mail rule + workflow (ngx-compatible); both are saved in one transaction via the same
//! functions as `/api/mail_rules/` (pl-mail `rules::save_in`) and `/api/workflows/` (pl-workflows).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use pl_authz::User;
use pl_consumer::workflow_ext;
use pl_core::AppState;
use pl_mail::ctx::MailUser;
use pl_mail::perms::MAIL_RULE;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};

use crate::common::{self, ADD_WORKFLOW, CHANGE_WORKFLOW, VIEW_WORKFLOW, Who};
use crate::error::{AErr, AResult, from_mail, json as json_resp};

/// Source "Mail" (`DocumentSource.MailFetch`).
const SOURCE_MAIL: i64 = 3;

fn mail_user(u: &User) -> AResult<MailUser> {
    MailUser::from_authz(u).ok_or(AErr::Unauthenticated)
}

async fn require_rule_perm(st: &AppState, mu: &MailUser, method: &Method) -> AResult<()> {
    pl_mail::perms::require_model_perm(st.db(), mu, method, MAIL_RULE).await.map_err(from_mail)
}

/// Workflows whose triggers filter this mail rule (by `order`, id).
pub async fn linked_workflows(conn: &mut PgConnection, rule_id: i64) -> AResult<Vec<i64>> {
    let ids: Vec<i32> = sqlx::query_scalar(
        "SELECT w.id FROM documents_workflow w WHERE EXISTS (SELECT 1 FROM documents_workflow_triggers wt \
         JOIN documents_workflowtrigger t ON t.id = wt.workflowtrigger_id WHERE wt.workflow_id = w.id AND t.filter_mailrule_id = $1) \
         ORDER BY w.\"order\", w.id",
    )
    .bind(i32::try_from(rule_id).unwrap_or(-1))
    .fetch_all(&mut *conn)
    .await?;
    Ok(ids.into_iter().map(i64::from).collect())
}

/// Fix the ingestion triggers of the workflow draft to the rule: if there is none, one is added;
/// existing ones get `filter_mailrule = rule` and the source "Mail" (if sources are restricted).
pub fn bind_to_rule(data: &mut Map<String, Value>, rule_id: i64) {
    let triggers = data.entry("triggers").or_insert_with(|| Value::Array(Vec::new()));
    if !triggers.is_array() {
        return; // the workflow validation reports the error
    }
    let Some(list) = triggers.as_array_mut() else { return };
    let mut has = false;
    for t in list.iter_mut() {
        let Some(o) = t.as_object_mut() else { continue };
        if o.get("type").and_then(Value::as_i64) != Some(1) {
            continue;
        }
        has = true;
        o.insert("filter_mailrule".into(), json!(rule_id));
        if let Some(Value::Array(src)) = o.get_mut("sources")
            && !src.is_empty()
            && !src.iter().any(|s| s.as_i64() == Some(SOURCE_MAIL))
        {
            src.push(json!(SOURCE_MAIL));
        }
    }
    if !has {
        list.push(json!({
            "type": 1,
            "sources": [SOURCE_MAIL],
            "filter_mailrule": rule_id,
            "matching_algorithm": 0,
            "match": "",
            "is_insensitive": true,
        }));
    }
}

async fn render_flow(st: &AppState, conn: &mut PgConnection, user: &User, mu: &MailUser, rule: &pl_mail::rules::RuleRow) -> AResult<Value> {
    let rule_v = pl_mail::rules::render(st.db(), mu, false, rule).await.map_err(from_mail)?;
    let mut wfs = Vec::new();
    for id in linked_workflows(conn, i64::from(rule.id)).await? {
        if let Some(l) = crate::workflows::load(conn, id).await? {
            wfs.push(crate::workflows::render(conn, user, &l).await?);
        }
    }
    Ok(json!({"mail_rule": rule_v, "workflows": wfs}))
}

/// GET /mail-flows – visible mail rules with their workflows (short list).
pub async fn list(State(st): State<AppState>, who: Who) -> Result<Response, AErr> {
    let user = who.require(&[VIEW_WORKFLOW])?;
    let mu = mail_user(user)?;
    require_rule_perm(&st, &mu, &Method::GET).await?;
    let mut conn = st.db().acquire().await?;
    let all: Vec<i32> = sqlx::query_scalar("SELECT id FROM paperless_mail_mailrule ORDER BY \"order\", id").fetch_all(&mut *conn).await?;
    let visible = pl_mail::perms::permitted_among(st.db(), &mu, MAIL_RULE, "view_mailrule", &all).await.map_err(from_mail)?;
    let ids: Vec<i32> = all.into_iter().filter(|i| visible.contains(i)).collect();
    let rows = sqlx::query(
        "SELECT r.id, r.name, r.enabled, r.account_id, \
           COALESCE((SELECT json_agg(json_build_object('id', w.id, 'name', w.name, 'enabled', w.enabled) ORDER BY w.\"order\", w.id) \
             FROM documents_workflow w WHERE EXISTS (SELECT 1 FROM documents_workflow_triggers wt JOIN documents_workflowtrigger t \
             ON t.id = wt.workflowtrigger_id WHERE wt.workflow_id = w.id AND t.filter_mailrule_id = r.id)), '[]'::json) \
         FROM paperless_mail_mailrule r WHERE r.id = ANY($1) ORDER BY r.\"order\", r.id",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let mut items = Vec::new();
    for r in rows {
        items.push(json!({
            "mail_rule": {"id": r.try_get::<i32, _>(0)?, "name": r.try_get::<String, _>(1)?, "enabled": r.try_get::<bool, _>(2)?, "account": r.try_get::<i32, _>(3)?},
            "workflows": r.try_get::<Value, _>(4)?,
        }));
    }
    Ok(json_resp(StatusCode::OK, &json!({"items": items})))
}

/// GET /mail-flows/{rule}
pub async fn get_one(State(st): State<AppState>, who: Who, Path(id): Path<String>) -> Result<Response, AErr> {
    let user = who.require(&[VIEW_WORKFLOW])?;
    let mu = mail_user(user)?;
    require_rule_perm(&st, &mu, &Method::GET).await?;
    let id = common::path_id(&id)?;
    let rule = pl_mail::rules::load_for(st.db(), &mu, &Method::GET, id as i32).await.map_err(from_mail)?;
    workflow_ext::ensure_pool(st.db()).await?;
    let mut conn = st.db().acquire().await?;
    Ok(json_resp(StatusCode::OK, &render_flow(&st, &mut conn, user, &mu, &rule).await?))
}

/// POST /mail-flows `{"mail_rule", "workflows": [...]}`
pub async fn create(State(st): State<AppState>, who: Who, headers: HeaderMap, body: Bytes) -> Result<Response, AErr> {
    let b = common::body_object(&headers, &body)?;
    let wfs = workflows_of(&b)?;
    let user = who.require(&[VIEW_WORKFLOW])?;
    if !wfs.is_empty() {
        who.require(&[ADD_WORKFLOW])?;
    }
    let mu = mail_user(user)?;
    require_rule_perm(&st, &mu, &Method::POST).await?;
    let rule_data = rule_of(&b)?;
    workflow_ext::ensure_pool(st.db()).await?;
    let mut tx = st.db().begin().await?;
    let rule = pl_mail::rules::save_in(&mut tx, st.db(), &mu, &rule_data, None, false).await.map_err(prefix_mail)?;
    save_workflows(&mut tx, i64::from(rule.id), wfs, &[]).await?;
    tx.commit().await?;
    let mut conn = st.db().acquire().await?;
    Ok(json_resp(StatusCode::CREATED, &render_flow(&st, &mut conn, user, &mu, &rule).await?))
}

/// PUT/PATCH /mail-flows/{rule} `{"mail_rule"?, "workflows"?, "delete_workflow_ids"?}`
pub async fn update(st: AppState, who: Who, id: String, headers: HeaderMap, body: Bytes, partial: bool) -> Result<Response, AErr> {
    let b = common::body_object(&headers, &body)?;
    let wfs = workflows_of(&b)?;
    let delete = common::int_list(&b, "delete_workflow_ids")?.unwrap_or_default();
    let user = who.require(&[VIEW_WORKFLOW])?;
    if wfs.iter().any(|w| w.get("id").and_then(Value::as_i64).is_some()) {
        who.require(&[CHANGE_WORKFLOW])?;
    }
    if wfs.iter().any(|w| w.get("id").and_then(Value::as_i64).is_none()) {
        who.require(&[ADD_WORKFLOW])?;
    }
    if !delete.is_empty() {
        who.require(&["documents.delete_workflow"])?;
    }
    let mu = mail_user(user)?;
    let method = if partial { Method::PATCH } else { Method::PUT };
    require_rule_perm(&st, &mu, &method).await?;
    let id = common::path_id(&id)?;
    let row = pl_mail::rules::load_for(st.db(), &mu, &method, id as i32).await.map_err(from_mail)?;
    workflow_ext::ensure_pool(st.db()).await?;
    let mut tx = st.db().begin().await?;
    let rule = match b.get("mail_rule") {
        Some(_) => pl_mail::rules::save_in(&mut tx, st.db(), &mu, &rule_of(&b)?, Some(row), partial).await.map_err(prefix_mail)?,
        None if partial => row,
        None => return Err(AErr::field("mail_rule", "This field is required.")),
    };
    save_workflows(&mut tx, i64::from(rule.id), wfs, &delete).await?;
    tx.commit().await?;
    let mut conn = st.db().acquire().await?;
    Ok(json_resp(StatusCode::OK, &render_flow(&st, &mut conn, user, &mu, &rule).await?))
}

pub async fn put(State(st): State<AppState>, who: Who, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> Result<Response, AErr> {
    update(st, who, id, headers, body, false).await
}

pub async fn patch(
    State(st): State<AppState>,
    who: Who,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AErr> {
    update(st, who, id, headers, body, true).await
}

fn rule_of(b: &Map<String, Value>) -> AResult<Map<String, Value>> {
    match b.get("mail_rule") {
        Some(Value::Object(m)) => Ok(m.clone()),
        None => Err(AErr::field("mail_rule", "This field is required.")),
        Some(_) => Err(AErr::field("mail_rule", "Expected an object.")),
    }
}

fn workflows_of(b: &Map<String, Value>) -> AResult<Vec<Map<String, Value>>> {
    match b.get("workflows") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => {
            a.iter().map(|w| w.as_object().cloned().ok_or_else(|| AErr::field("workflows", "Expected a list of objects."))).collect()
        }
        Some(_) => Err(AErr::field("workflows", "Expected a list of objects.")),
    }
}

/// Error of the mail rule under `mail_rule`.
fn prefix_mail(e: pl_mail::error::ApiError) -> AErr {
    match from_mail(e) {
        AErr::Invalid { message, fields } => {
            let mut f = Map::new();
            f.insert("mail_rule".into(), Value::Object(fields));
            AErr::Invalid { message, fields: f }
        }
        other => other,
    }
}

/// Save the workflows of the flow (errors under `workflows.<index>`), then delete.
async fn save_workflows(tx: &mut PgConnection, rule_id: i64, wfs: Vec<Map<String, Value>>, delete: &[i64]) -> AResult<()> {
    let linked = linked_workflows(tx, rule_id).await?;
    for d in delete {
        if !linked.contains(d) {
            return Err(AErr::field("delete_workflow_ids", &format!("Workflow {d} is not part of this mail flow.")));
        }
    }
    for (i, mut w) in wfs.into_iter().enumerate() {
        let id = w.remove("id").and_then(|v| v.as_i64());
        if id.is_some_and(|i| delete.contains(&i)) {
            return Err(AErr::field("delete_workflow_ids", "A workflow cannot be saved and deleted at once."));
        }
        if let Some(id) = id
            && pl_workflows::load::get_row(tx, pl_workflows::schema::T_WORKFLOW, id).await?.is_none()
        {
            return Err(AErr::field("workflows", &format!("Workflow {id} does not exist.")));
        }
        bind_to_rule(&mut w, rule_id);
        if let Err(e) = crate::workflows::save(tx, w, id, false).await {
            return Err(match e {
                AErr::Invalid { message, fields } => {
                    let mut inner = Map::new();
                    inner.insert(i.to_string(), Value::Object(fields));
                    let mut f = Map::new();
                    f.insert("workflows".into(), Value::Object(inner));
                    AErr::Invalid { message, fields: f }
                }
                other => other,
            });
        }
    }
    for d in delete {
        pl_workflows::write::delete_workflow(tx, *d).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_adds_or_fixes_consumption_trigger() {
        let mut w: Map<String, Value> = serde_json::from_value(json!({"name": "x", "triggers": [{"type": 2}]})).unwrap();
        bind_to_rule(&mut w, 7);
        let ts = w["triggers"].as_array().unwrap();
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[1]["type"], 1);
        assert_eq!(ts[1]["filter_mailrule"], 7);
        assert_eq!(ts[1]["sources"], json!([3]));
        let mut w: Map<String, Value> =
            serde_json::from_value(json!({"triggers": [{"type": 1, "sources": [1], "filter_mailrule": 2}]})).unwrap();
        bind_to_rule(&mut w, 7);
        assert_eq!(w["triggers"][0]["filter_mailrule"], 7);
        assert_eq!(w["triggers"][0]["sources"], json!([1, 3]));
        let mut w: Map<String, Value> = serde_json::from_value(json!({})).unwrap();
        bind_to_rule(&mut w, 1);
        assert_eq!(w["triggers"][0]["filter_mailrule"], 1);
        // wrong type stays for the validation
        let mut w: Map<String, Value> = serde_json::from_value(json!({"triggers": "x"})).unwrap();
        bind_to_rule(&mut w, 1);
        assert_eq!(w["triggers"], "x");
    }
}
