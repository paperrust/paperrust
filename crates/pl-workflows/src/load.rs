//! Loading and representing (to_representation) triggers, actions and workflows.
//! upstream: src/documents/serialisers.py@v3.4.2 (Workflow*Serializer.to_representation via
//! ModelSerializer), src/documents/views.py@v3.4.2 (WorkflowViewSet.queryset with Prefetch),
//! django-multiselectfield 1.0.1 (`MultiSelectField.to_python`), DRF `MultipleChoiceField.to_representation`.

use crate::schema::*;
use sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query, SelectStatement};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};
use std::collections::HashMap;

/// Whole row as JSON (`row_to_json(<table>)`, column names = Django columns).
/// Raw SQL fragment because sea-query has no whole-row reference.
fn row_json_expr(table: &str) -> sea_query::SimpleExpr {
    Expr::cust(format!("row_to_json(\"{table}\")"))
}

pub async fn fetch_rows(db: &mut PgConnection, q: &SelectStatement) -> Result<Vec<Map<String, Value>>, sqlx::Error> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(&mut *db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let v: sqlx::types::Json<Value> = r.try_get(0)?;
        if let Value::Object(o) = v.0 {
            out.push(o);
        }
    }
    Ok(out)
}

/// Rows of a table by ids (in the order of the ids).
pub async fn rows_by_ids(db: &mut PgConnection, table: &'static str, ids: &[i64]) -> Result<Vec<Map<String, Value>>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let q = Query::select()
        .expr(row_json_expr(table))
        .from(Alias::new(table))
        .and_where(Expr::col((Alias::new(table), Alias::new("id"))).is_in(ids.iter().copied()))
        .to_owned();
    let rows = fetch_rows(db, &q).await?;
    let mut by_id: HashMap<i64, Map<String, Value>> = HashMap::new();
    for r in rows {
        if let Some(id) = r.get("id").and_then(Value::as_i64) {
            by_id.insert(id, r);
        }
    }
    Ok(ids.iter().filter_map(|i| by_id.remove(i)).collect())
}

/// m2m values: owner id → [target ids] in the Meta order of the target model.
pub async fn load_m2m(db: &mut PgConnection, m: &M2m, owner_ids: &[i64]) -> Result<HashMap<i64, Vec<i64>>, sqlx::Error> {
    let mut out: HashMap<i64, Vec<i64>> = HashMap::new();
    if owner_ids.is_empty() {
        return Ok(out);
    }
    // SELECT through.own, target.id FROM target INNER JOIN through ON target.id = through.other
    // WHERE through.own IN (...) [ORDER BY target.name|created]  (like the Django prefetch)
    let mut q = Query::select();
    q.expr(Expr::col((Alias::new(m.through), Alias::new(m.own))).cast_as(Alias::new("bigint")))
        .expr(Expr::col((Alias::new(m.target), Alias::new("id"))).cast_as(Alias::new("bigint")))
        .from(Alias::new(m.target))
        .inner_join(
            Alias::new(m.through),
            Expr::col((Alias::new(m.target), Alias::new("id")))
                .equals((Alias::new(m.through), Alias::new(m.other))),
        )
        .and_where(Expr::col((Alias::new(m.through), Alias::new(m.own))).is_in(owner_ids.iter().copied()));
    match m.order {
        M2mOrder::Name => {
            q.order_by((Alias::new(m.target), Alias::new("name")), Order::Asc);
        }
        M2mOrder::Created => {
            q.order_by((Alias::new(m.target), Alias::new("created")), Order::Asc);
        }
        M2mOrder::Unordered => {}
    }
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    for r in sqlx::query_with(&sql, values).fetch_all(&mut *db).await? {
        let own: i64 = r.try_get(0)?;
        let id: i64 = r.try_get(1)?;
        out.entry(own).or_default().push(id);
    }
    Ok(out)
}

/// `MultiSelectField.to_python` + `MultipleChoiceField.to_representation`.
pub fn sources_repr(v: Option<&Value>) -> Value {
    let s = match v {
        Some(Value::String(s)) => s.as_str(),
        _ => "",
    };
    if s.is_empty() {
        return json!([]);
    }
    let mut out: Vec<Value> = Vec::new();
    for item in s.split(',') {
        let val = match SOURCES.iter().find(|c| c.to_string() == item) {
            Some(c) => json!(c),
            None => json!(item),
        };
        if !out.contains(&val) {
            out.push(val);
        }
    }
    Value::Array(out)
}

fn scalar(row: &Map<String, Value>, col: &str) -> Value {
    row.get(col).cloned().unwrap_or(Value::Null)
}

/// Representation of a row by field list.
async fn render(
    db: &mut PgConnection,
    fields: &'static [Field],
    rows: &[Map<String, Value>],
) -> Result<Vec<Map<String, Value>>, sqlx::Error> {
    let ids: Vec<i64> = rows.iter().filter_map(|r| r.get("id").and_then(Value::as_i64)).collect();
    let mut m2m: HashMap<&'static str, HashMap<i64, Vec<i64>>> = HashMap::new();
    for (name, m) in m2m_fields(fields) {
        m2m.insert(name, load_m2m(db, &m, &ids).await?);
    }
    // nested objects (email / webhook)
    let mut nested: HashMap<&'static str, HashMap<i64, Map<String, Value>>> = HashMap::new();
    for fld in fields {
        if let Col::Nested(fk) = fld.col {
            let (table, nfields) = nested_def(fld.spec.name);
            let nids: Vec<i64> = rows.iter().filter_map(|r| r.get(fk).and_then(Value::as_i64)).collect();
            let nrows = rows_by_ids(db, table, &nids).await?;
            let mut map = HashMap::new();
            for nr in nrows {
                let id = nr.get("id").and_then(Value::as_i64).unwrap_or_default();
                map.insert(id, render_plain(nfields, &nr));
            }
            nested.insert(fld.spec.name, map);
        }
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.get("id").and_then(Value::as_i64).unwrap_or_default();
        let mut o = Map::new();
        for fld in fields {
            let v = match fld.col {
                Col::Pk => json!(id),
                Col::Scalar("sources", _) => sources_repr(row.get("sources")),
                Col::Scalar(c, _) => scalar(row, c),
                Col::M2m(_) => {
                    let ids = m2m.get(fld.spec.name).and_then(|m| m.get(&id)).cloned().unwrap_or_default();
                    json!(ids)
                }
                Col::Nested(fk) => match row.get(fk).and_then(Value::as_i64) {
                    Some(nid) => nested
                        .get(fld.spec.name)
                        .and_then(|m| m.get(&nid))
                        .cloned()
                        .map(Value::Object)
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                },
            };
            o.insert(fld.spec.name.to_string(), v);
        }
        out.push(o);
    }
    Ok(out)
}

pub fn nested_def(name: &str) -> (&'static str, &'static [Field]) {
    if name == "email" { (T_EMAIL, EMAIL_FIELDS) } else { (T_WEBHOOK, WEBHOOK_FIELDS) }
}

fn render_plain(fields: &'static [Field], row: &Map<String, Value>) -> Map<String, Value> {
    let mut o = Map::new();
    for fld in fields {
        let v = match fld.col {
            Col::Pk => scalar(row, "id"),
            Col::Scalar(c, _) => scalar(row, c),
            _ => Value::Null,
        };
        o.insert(fld.spec.name.to_string(), v);
    }
    o
}

pub async fn render_triggers(db: &mut PgConnection, rows: &[Map<String, Value>]) -> Result<Vec<Map<String, Value>>, sqlx::Error> {
    render(db, TRIGGER_FIELDS, rows).await
}

pub async fn render_actions(db: &mut PgConnection, rows: &[Map<String, Value>]) -> Result<Vec<Map<String, Value>>, sqlx::Error> {
    render(db, ACTION_FIELDS, rows).await
}

/// Child rows of a workflow via the m2m table.
/// `ordered_actions`: viewset prefetch (`ORDER BY order, pk`), otherwise unsorted.
async fn children(
    db: &mut PgConnection,
    table: &'static str,
    through: &'static str,
    other: &'static str,
    wf_ids: &[i64],
    ordered: bool,
) -> Result<Vec<(i64, Map<String, Value>)>, sqlx::Error> {
    if wf_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = Query::select();
    q.expr(Expr::col((Alias::new(through), Alias::new("workflow_id"))).cast_as(Alias::new("bigint")))
        .expr(row_json_expr(table))
        .from(Alias::new(table))
        .inner_join(
            Alias::new(through),
            Expr::col((Alias::new(table), Alias::new("id"))).equals((Alias::new(through), Alias::new(other))),
        )
        .and_where(Expr::col((Alias::new(through), Alias::new("workflow_id"))).is_in(wf_ids.iter().copied()));
    if ordered {
        q.order_by((Alias::new(table), Alias::new("order")), Order::Asc)
            .order_by((Alias::new(table), Alias::new("id")), Order::Asc);
    }
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let mut out = Vec::new();
    for r in sqlx::query_with(&sql, values).fetch_all(&mut *db).await? {
        let wf: i64 = r.try_get(0)?;
        let v: sqlx::types::Json<Value> = r.try_get(1)?;
        if let Value::Object(o) = v.0 {
            out.push((wf, o));
        }
    }
    Ok(out)
}

/// Represent workflows. `prefetch = true`: like the viewset queryset (list/retrieve),
/// `false`: like `serializer.data` after create/update (no sorting of the actions).
pub async fn render_workflows(
    db: &mut PgConnection,
    rows: &[Map<String, Value>],
    prefetch: bool,
) -> Result<Vec<Map<String, Value>>, sqlx::Error> {
    let ids: Vec<i64> = rows.iter().filter_map(|r| r.get("id").and_then(Value::as_i64)).collect();
    let trig = children(db, T_TRIGGER, T_WF_TRIGGERS, "workflowtrigger_id", &ids, false).await?;
    let act = children(db, T_ACTION, T_WF_ACTIONS, "workflowaction_id", &ids, prefetch).await?;
    let trig_rows: Vec<Map<String, Value>> = trig.iter().map(|(_, r)| r.clone()).collect();
    let act_rows: Vec<Map<String, Value>> = act.iter().map(|(_, r)| r.clone()).collect();
    let trig_r = render_triggers(db, &trig_rows).await?;
    let act_r = render_actions(db, &act_rows).await?;
    let mut t_by: HashMap<i64, Vec<Value>> = HashMap::new();
    for ((wf, _), r) in trig.iter().zip(trig_r) {
        t_by.entry(*wf).or_default().push(Value::Object(r));
    }
    let mut a_by: HashMap<i64, Vec<Value>> = HashMap::new();
    for ((wf, _), r) in act.iter().zip(act_r) {
        a_by.entry(*wf).or_default().push(Value::Object(r));
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.get("id").and_then(Value::as_i64).unwrap_or_default();
        let mut o = Map::new();
        o.insert("id".into(), json!(id));
        o.insert("name".into(), scalar(row, "name"));
        o.insert("order".into(), scalar(row, "order"));
        o.insert("enabled".into(), scalar(row, "enabled"));
        o.insert("triggers".into(), Value::Array(t_by.remove(&id).unwrap_or_default()));
        o.insert("actions".into(), Value::Array(a_by.remove(&id).unwrap_or_default()));
        out.push(o);
    }
    Ok(out)
}

/// `SELECT row_to_json(t) FROM t [ORDER BY …] LIMIT/OFFSET`
pub fn select_rows(table: &'static str) -> SelectStatement {
    Query::select().expr(row_json_expr(table)).from(Alias::new(table)).to_owned()
}

pub async fn count(db: &mut PgConnection, table: &'static str) -> Result<i64, sqlx::Error> {
    let (sql, values) = Query::select()
        .expr(Expr::col(sea_query::Asterisk).count())
        .from(Alias::new(table))
        .build_sqlx(PostgresQueryBuilder);
    let r = sqlx::query_with(&sql, values).fetch_one(&mut *db).await?;
    r.try_get::<i64, _>(0)
}

pub async fn ids_in_order(db: &mut PgConnection, table: &'static str, ordered: bool) -> Result<Vec<i64>, sqlx::Error> {
    ids_in_order_where(db, table, ordered, &[]).await
}

/// Like [`count`], with filter conditions (workflow list).
pub async fn count_where(db: &mut PgConnection, table: &'static str, conds: &[sea_query::SimpleExpr]) -> Result<i64, sqlx::Error> {
    let mut q = Query::select();
    q.expr(Expr::col(sea_query::Asterisk).count()).from(Alias::new(table));
    for c in conds {
        q.and_where(c.clone());
    }
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let r = sqlx::query_with(&sql, values).fetch_one(&mut *db).await?;
    r.try_get::<i64, _>(0)
}

/// Like [`ids_in_order`], with filter conditions (workflow list).
pub async fn ids_in_order_where(
    db: &mut PgConnection,
    table: &'static str,
    ordered: bool,
    conds: &[sea_query::SimpleExpr],
) -> Result<Vec<i64>, sqlx::Error> {
    let mut q = Query::select();
    q.expr(Expr::col((Alias::new(table), Alias::new("id"))).cast_as(Alias::new("bigint"))).from(Alias::new(table));
    for c in conds {
        q.and_where(c.clone());
    }
    if ordered {
        q.order_by((Alias::new(table), Alias::new("order")), Order::Asc);
    }
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(&mut *db).await?;
    rows.iter().map(|r| r.try_get::<i64, _>(0)).collect()
}

pub async fn get_row(db: &mut PgConnection, table: &'static str, id: i64) -> Result<Option<Map<String, Value>>, sqlx::Error> {
    Ok(rows_by_ids(db, table, &[id]).await?.into_iter().next())
}
