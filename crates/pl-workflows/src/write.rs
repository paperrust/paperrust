//! Writing like Django/DRF: `ModelSerializer.create/update`, `QuerySet.update_or_create`,
//! `ManyRelatedManager.set`, `Model.delete` (Collector), `WorkflowSerializer.update_triggers_and_actions`
//! and `prune_triggers_and_actions`.
//! upstream: src/documents/serialisers.py@v3.4.2 (WorkflowSerializer), Django 5.2
//! (`db/models/query.py::update_or_create`, `fields/related_descriptors.py::set/_add_items`,
//! `base.py::Model._save_table`), DRF 3.18.3 (`ModelSerializer.create/update`).
//! No enclosing transaction (paperless has no ATOMIC_REQUESTS).

use crate::drf::VErr;
use crate::schema::*;
use crate::serializers::{self, Attrs, WorkflowInput};
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query, SimpleExpr};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value};
use sqlx::{PgConnection, Row};

type Cols = Vec<(&'static str, ColTy, Value)>;

fn bind(ty: ColTy, v: &Value) -> SimpleExpr {
    let val: sea_query::Value = match ty {
        ColTy::Int => sea_query::Value::BigInt(v.as_i64()),
        ColTy::Bool => sea_query::Value::Bool(v.as_bool()),
        ColTy::Str => sea_query::Value::String(v.as_str().map(|s| Box::new(s.to_string()))),
        ColTy::Json => {
            if v.is_null() {
                sea_query::Value::Json(None)
            } else {
                sea_query::Value::Json(Some(Box::new(v.clone())))
            }
        }
    };
    let e = SimpleExpr::Value(val);
    match ty {
        // Column type smallint/integer: cast explicitly so that `$n::int8` fits
        ColTy::Int => e.cast_as(Alias::new("integer")),
        ColTy::Json => e.cast_as(Alias::new("jsonb")),
        _ => e,
    }
}

/// Column values from validated attributes (without m2m, without pk, without nested objects).
fn scalar_cols(fields: &'static [Field], attrs: &Attrs) -> Cols {
    let mut out = Vec::new();
    for f in fields {
        if let Col::Scalar(col, ty) = f.col
            && let Some(v) = attrs.get(f.spec.name) {
                let v = if col == "sources" { sources_db(v) } else { v.clone() };
                out.push((col, ty, v));
            }
    }
    out
}

/// `normalize_workflow_trigger_sources` + `MultiSelectField.get_prep_value`: `",".join(str(s))`.
fn sources_db(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::String(
            a.iter().map(crate::pyfmt::py_str).collect::<Vec<_>>().join(","),
        ),
        other => other.clone(),
    }
}

fn overlay(defaults: Cols, cols: &Cols) -> Cols {
    let mut out = defaults;
    for (c, ty, v) in cols {
        if let Some(e) = out.iter_mut().find(|e| e.0 == *c) {
            e.2 = v.clone();
        } else {
            out.push((c, *ty, v.clone()));
        }
    }
    out
}

pub async fn insert(db: &mut PgConnection, table: &'static str, cols: &Cols, id: Option<i64>) -> Result<i64, VErr> {
    let mut q = Query::insert();
    q.into_table(Alias::new(table));
    let mut names: Vec<Alias> = Vec::new();
    let mut vals: Vec<SimpleExpr> = Vec::new();
    if let Some(id) = id {
        names.push(Alias::new("id"));
        vals.push(SimpleExpr::Value(id.into()).cast_as(Alias::new("integer")));
    }
    for (c, ty, v) in cols {
        names.push(Alias::new(*c));
        vals.push(bind(*ty, v));
    }
    q.columns(names);
    q.values(vals).map_err(|e| VErr::Server(format!("insert build: {e}")))?;
    q.returning(Query::returning().expr(Expr::col(Alias::new("id")).cast_as(Alias::new("bigint"))));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_one(&mut *db).await?;
    Ok(row.try_get::<i64, _>(0)?)
}

/// UPDATE … WHERE id = $id; returns the number of affected rows.
pub async fn update(db: &mut PgConnection, table: &'static str, id: i64, cols: &Cols) -> Result<u64, VErr> {
    if cols.is_empty() {
        // Django saves anyway (UPDATE without change) – does the row exist?
        return Ok(crate::drf::row_exists(db, table, id).await? as u64);
    }
    let mut q = Query::update();
    q.table(Alias::new(table));
    for (c, ty, v) in cols {
        q.value(Alias::new(*c), bind(*ty, v));
    }
    q.and_where(Expr::col(Alias::new("id")).eq(id));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    Ok(sqlx::query_with(&sql, values).execute(&mut *db).await?.rows_affected())
}

/// Django `Model.save()`: pk None → INSERT; otherwise UPDATE, on 0 rows INSERT with pk.
async fn save_full(db: &mut PgConnection, table: &'static str, pk: Option<i64>, cols: &Cols) -> Result<i64, VErr> {
    match pk {
        None => insert(db, table, cols, None).await,
        Some(id) => {
            if update(db, table, id, cols).await? == 0 {
                insert(db, table, cols, Some(id)).await
            } else {
                Ok(id)
            }
        }
    }
}

/// `update_or_create(id=…, defaults=…)`: existing row → set only the `defaults` columns,
/// otherwise INSERT (model defaults + `defaults`, possibly with explicit id).
async fn update_or_create(
    db: &mut PgConnection,
    table: &'static str,
    model_defaults: Cols,
    id: Option<i64>,
    cols: &Cols,
) -> Result<i64, VErr> {
    if let Some(id) = id {
        if !(-2_147_483_648..=2_147_483_647).contains(&id) {
            return Err(VErr::Server("id out of range".into()));
        }
        if crate::drf::row_exists(db, table, id).await? {
            update(db, table, id, cols).await?;
            return Ok(id);
        }
        return insert(db, table, &overlay(model_defaults, cols), Some(id)).await;
    }
    insert(db, table, &overlay(model_defaults, cols), None).await
}

// ------------------------------------------------------------------ m2m

/// Iteration order of a CPython `set` of ints, built by `add` in this order
/// (`Objects/setobject.c`, CPython 3.12: LINEAR_PROBES=9, PERTURB_SHIFT=5, resize at fill*5 >= mask*3).
pub fn py_set_order(values: &[i64]) -> Vec<i64> {
    const LINEAR_PROBES: usize = 9;
    const PERTURB_SHIFT: u32 = 5;
    fn hash(v: i64) -> u64 {
        // hash(int) for |v| < 2**61-1; hash(-1) == -2
        let h = if v == -1 { -2 } else { v };
        h as u64
    }
    fn insert_clean(table: &mut [Option<i64>], mask: usize, key: i64) {
        let h = hash(key);
        let mut perturb = h;
        let mut i = (h as usize) & mask;
        loop {
            if table[i].is_none() {
                table[i] = Some(key);
                return;
            }
            if i + LINEAR_PROBES <= mask {
                for j in 1..=LINEAR_PROBES {
                    if table[i + j].is_none() {
                        table[i + j] = Some(key);
                        return;
                    }
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb as usize)) & mask;
        }
    }
    let mut mask: usize = 7;
    let mut table: Vec<Option<i64>> = vec![None; 8];
    let mut used = 0usize;
    for &v in values {
        if table.contains(&Some(v)) {
            continue;
        }
        insert_clean(&mut table, mask, v);
        used += 1;
        if used * 5 >= mask * 3 {
            let minused = if used > 50000 { used * 2 } else { used * 4 };
            let mut newsize = 8usize;
            while newsize <= minused {
                newsize <<= 1;
            }
            let old = std::mem::replace(&mut table, vec![None; newsize]);
            mask = newsize - 1;
            for k in old.into_iter().flatten() {
                insert_clean(&mut table, mask, k);
            }
        }
    }
    table.into_iter().flatten().collect()
}

/// `ManyRelatedManager.set(ids)` (clear=False): add missing ones, remove surplus ones.
pub async fn m2m_set(db: &mut PgConnection, m: &M2m, owner: i64, ids: &[i64]) -> Result<(), VErr> {
    let (sql, values) = Query::select()
        .expr(Expr::col(Alias::new(m.other)).cast_as(Alias::new("bigint")))
        .from(Alias::new(m.through))
        .and_where(Expr::col(Alias::new(m.own)).eq(owner))
        .build_sqlx(PostgresQueryBuilder);
    let mut old: Vec<i64> = Vec::new();
    for r in sqlx::query_with(&sql, values).fetch_all(&mut *db).await? {
        old.push(r.try_get::<i64, _>(0)?);
    }
    let mut new_objs = Vec::new();
    for id in ids {
        if let Some(pos) = old.iter().position(|o| o == id) {
            old.remove(pos);
        } else {
            new_objs.push(*id);
        }
    }
    if !old.is_empty() {
        let (sql, values) = Query::delete()
            .from_table(Alias::new(m.through))
            .and_where(Expr::col(Alias::new(m.own)).eq(owner))
            .and_where(Expr::col(Alias::new(m.other)).is_in(old.iter().copied()))
            .build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values).execute(&mut *db).await?;
    }
    m2m_add(db, m.through, m.own, m.other, owner, &new_objs).await
}

/// `_add_items` (fast add: `bulk_create(..., ignore_conflicts=True)` in Set-Reihenfolge).
async fn m2m_add(
    db: &mut PgConnection,
    through: &'static str,
    own: &'static str,
    other: &'static str,
    owner: i64,
    ids: &[i64],
) -> Result<(), VErr> {
    let ordered = py_set_order(ids);
    if ordered.is_empty() {
        return Ok(());
    }
    let mut q = Query::insert();
    q.into_table(Alias::new(through)).columns([Alias::new(own), Alias::new(other)]);
    for id in ordered {
        q.values([
            SimpleExpr::Value(owner.into()).cast_as(Alias::new("integer")),
            SimpleExpr::Value(id.into()).cast_as(Alias::new("integer")),
        ])
        .map_err(|e| VErr::Server(format!("m2m build: {e}")))?;
    }
    q.on_conflict(sea_query::OnConflict::new().do_nothing().to_owned());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *db).await?;
    Ok(())
}

fn ids_of(v: &Value) -> Vec<i64> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default()
}

async fn set_m2m_from_attrs(db: &mut PgConnection, fields: &'static [Field], owner: i64, attrs: &Attrs) -> Result<(), VErr> {
    for (name, m) in m2m_fields(fields) {
        if let Some(v) = attrs.get(name)
            && !v.is_null() {
                m2m_set(db, &m, owner, &ids_of(v)).await?;
            }
    }
    Ok(())
}

// ------------------------------------------------------------------ Deleting

async fn exec_delete(db: &mut PgConnection, table: &'static str, col: &'static str, ids: &[i64]) -> Result<(), VErr> {
    if ids.is_empty() {
        return Ok(());
    }
    let (sql, values) = Query::delete()
        .from_table(Alias::new(table))
        .and_where(Expr::col(Alias::new(col)).is_in(ids.iter().copied()))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *db).await?;
    Ok(())
}

pub async fn delete_triggers(db: &mut PgConnection, ids: &[i64]) -> Result<(), VErr> {
    for (_, m) in m2m_fields(TRIGGER_FIELDS) {
        exec_delete(db, m.through, m.own, ids).await?;
    }
    exec_delete(db, T_WF_TRIGGERS, "workflowtrigger_id", ids).await?;
    exec_delete(db, T_TRIGGER, "id", ids).await
}

pub async fn delete_actions(db: &mut PgConnection, ids: &[i64]) -> Result<(), VErr> {
    for (_, m) in m2m_fields(ACTION_FIELDS) {
        exec_delete(db, m.through, m.own, ids).await?;
    }
    exec_delete(db, T_WF_ACTIONS, "workflowaction_id", ids).await?;
    exec_delete(db, T_ACTION, "id", ids).await
}

pub async fn delete_workflow(db: &mut PgConnection, id: i64) -> Result<(), VErr> {
    // PaperRust: actions of the "else" branch belong only to this workflow → delete them too
    // (the link, history and extra rows follow via ON DELETE CASCADE).
    let else_ids = paperrust_else_action_ids(db, Some(id)).await?;
    if !else_ids.is_empty() {
        delete_actions(db, &else_ids).await?;
    }
    exec_delete(db, T_WF_RUN, "workflow_id", &[id]).await?;
    exec_delete(db, T_WF_TRIGGERS, "workflow_id", &[id]).await?;
    exec_delete(db, T_WF_ACTIONS, "workflow_id", &[id]).await?;
    exec_delete(db, T_WORKFLOW, "id", &[id]).await
}

/// ids of a table without a reference from `ref_table.ref_col`.
async fn orphan_ids(db: &mut PgConnection, table: &'static str, ref_table: &'static str, ref_col: &'static str) -> Result<Vec<i64>, VErr> {
    let sub = Query::select()
        .expr(Expr::val(1))
        .from(Alias::new(ref_table))
        .and_where(Expr::col((Alias::new(ref_table), Alias::new(ref_col))).equals((Alias::new(table), Alias::new("id"))))
        .to_owned();
    let (sql, values) = Query::select()
        .expr(Expr::col((Alias::new(table), Alias::new("id"))).cast_as(Alias::new("bigint")))
        .from(Alias::new(table))
        .and_where(Expr::exists(sub).not())
        .build_sqlx(PostgresQueryBuilder);
    let mut out = Vec::new();
    for r in sqlx::query_with(&sql, values).fetch_all(&mut *db).await? {
        out.push(r.try_get::<i64, _>(0)?);
    }
    Ok(out)
}

/// PaperRust: ids of the actions in `paperrust_workflow_else_action` ("else" branch, pl-consumer
/// `workflow_ext`) – of one workflow or of all. Without the table: none.
pub async fn paperrust_else_action_ids(db: &mut PgConnection, workflow: Option<i64>) -> Result<Vec<i64>, VErr> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('public.paperrust_workflow_else_action') IS NOT NULL")
        .fetch_one(&mut *db)
        .await?;
    if !exists {
        return Ok(Vec::new());
    }
    let ids: Vec<i32> = match workflow.and_then(|w| i32::try_from(w).ok()) {
        Some(w) => {
            sqlx::query_scalar("SELECT workflowaction_id FROM paperrust_workflow_else_action WHERE workflow_id = $1")
                .bind(w)
                .fetch_all(&mut *db)
                .await?
        }
        None if workflow.is_some() => Vec::new(),
        None => sqlx::query_scalar("SELECT workflowaction_id FROM paperrust_workflow_else_action").fetch_all(&mut *db).await?,
    };
    Ok(ids.into_iter().map(i64::from).collect())
}

/// `prune_triggers_and_actions`
///
/// PaperRust: actions of an "else" branch are not attached to `documents_workflow_actions`, but are
/// not orphaned – they stay.
pub async fn prune(db: &mut PgConnection) -> Result<(), VErr> {
    let t = orphan_ids(db, T_TRIGGER, T_WF_TRIGGERS, "workflowtrigger_id").await?;
    delete_triggers(db, &t).await?;
    let keep = paperrust_else_action_ids(db, None).await?;
    let a: Vec<i64> = orphan_ids(db, T_ACTION, T_WF_ACTIONS, "workflowaction_id").await?.into_iter().filter(|id| !keep.contains(id)).collect();
    delete_actions(db, &a).await?;
    let e = orphan_ids(db, T_EMAIL, T_ACTION, "email_id").await?;
    exec_delete(db, T_EMAIL, "id", &e).await?;
    let w = orphan_ids(db, T_WEBHOOK, T_ACTION, "webhook_id").await?;
    exec_delete(db, T_WEBHOOK, "id", &w).await
}

// ------------------------------------------------------------------ Trigger/action directly

fn has_nested_write(attrs: &Attrs) -> bool {
    ["email", "webhook"].iter().any(|k| attrs.get(*k).is_some_and(|v| v.is_object() || v.is_array()))
}

fn id_attr(attrs: &Attrs) -> Option<Option<i64>> {
    attrs.get("id").map(Value::as_i64)
}

/// `ModelSerializer.create` for `/api/workflow_triggers/` or `/api/workflow_actions/`.
pub async fn model_create(db: &mut PgConnection, is_trigger: bool, attrs: &Attrs) -> Result<i64, VErr> {
    if !is_trigger && has_nested_write(attrs) {
        return Err(VErr::Server("The `.create()` method does not support writable nested fields by default.".into()));
    }
    let (table, fields, defaults) = if is_trigger {
        (T_TRIGGER, TRIGGER_FIELDS, trigger_defaults())
    } else {
        (T_ACTION, ACTION_FIELDS, action_defaults())
    };
    let mut cols = scalar_cols(fields, attrs);
    if !is_trigger {
        nested_null_cols(attrs, &mut cols);
    }
    let id = insert(db, table, &overlay(defaults, &cols), id_attr(attrs).flatten()).await?;
    set_m2m_from_attrs(db, fields, id, attrs).await?;
    Ok(id)
}

/// `"email": null` → `email_id = NULL`.
fn nested_null_cols(attrs: &Attrs, cols: &mut Cols) {
    for (k, col) in [("email", "email_id"), ("webhook", "webhook_id")] {
        if attrs.get(k).is_some_and(Value::is_null) {
            cols.push((col, ColTy::Int, Value::Null));
        }
    }
}

/// `ModelSerializer.update` (setattr + `instance.save()` + m2m `set`). Returns the pk of the
/// saved instance (can change through an `id` field, like upstream).
pub async fn model_update(
    db: &mut PgConnection,
    is_trigger: bool,
    row: &Map<String, Value>,
    attrs: &Attrs,
) -> Result<i64, VErr> {
    if !is_trigger && has_nested_write(attrs) {
        return Err(VErr::Server("The `.update()` method does not support writable nested fields by default.".into()));
    }
    let (table, fields, defaults) = if is_trigger {
        (T_TRIGGER, TRIGGER_FIELDS, trigger_defaults())
    } else {
        (T_ACTION, ACTION_FIELDS, action_defaults())
    };
    // current values of the instance
    let current: Cols = defaults
        .into_iter()
        .map(|(c, ty, d)| (c, ty, row.get(c).cloned().unwrap_or(d)))
        .collect();
    let mut cols = scalar_cols(fields, attrs);
    if !is_trigger {
        nested_null_cols(attrs, &mut cols);
    }
    let full = overlay(current, &cols);
    let old_pk = row.get("id").and_then(Value::as_i64);
    let pk = match id_attr(attrs) {
        Some(new) => new,
        None => old_pk,
    };
    let pk = save_full(db, table, pk, &full).await?;
    set_m2m_from_attrs(db, fields, pk, attrs).await?;
    Ok(pk)
}

// ------------------------------------------------------------------ Workflow

/// `WorkflowSerializer.update_triggers_and_actions`
async fn update_triggers_and_actions(
    db: &mut PgConnection,
    wf: i64,
    triggers: Option<&[Attrs]>,
    actions: Option<&[Attrs]>,
    is_create: bool,
) -> Result<(), VErr> {
    let mut set_triggers = Vec::new();
    let mut set_actions = Vec::new();
    if let Some(ts) = triggers {
        for t in ts {
            let cols = scalar_cols(TRIGGER_FIELDS, t);
            let id = t.get("id").and_then(Value::as_i64);
            let tid = update_or_create(db, T_TRIGGER, trigger_defaults(), id, &cols).await?;
            set_m2m_from_attrs(db, TRIGGER_FIELDS, tid, t).await?;
            set_triggers.push(tid);
        }
    }
    if let Some(acts) = actions {
        set_actions = upsert_actions(db, acts, is_create).await?;
    }
    let wf_trig = M2m { through: T_WF_TRIGGERS, own: "workflow_id", other: "workflowtrigger_id", target: T_TRIGGER, order: M2mOrder::Unordered };
    let wf_act = M2m { through: T_WF_ACTIONS, own: "workflow_id", other: "workflowaction_id", target: T_ACTION, order: M2mOrder::Unordered };
    if triggers.is_some() {
        m2m_set(db, &wf_trig, wf, &set_triggers).await?;
    }
    if actions.is_some() {
        m2m_set(db, &wf_act, wf, &set_actions).await?;
    }
    Ok(())
}

/// Create or update actions as in `WorkflowSerializer.update_triggers_and_actions`
/// (`order` = position, nested email/webhook, m2m). `is_create` discards ids sent along.
/// Returns: action ids in order. PaperRust also uses this for the "else" branch
/// (pl-automation) so that its actions are stored exactly like ngx actions.
pub async fn upsert_actions(db: &mut PgConnection, acts: &[Attrs], is_create: bool) -> Result<Vec<i64>, VErr> {
    let mut set_actions = Vec::with_capacity(acts.len());
    for (index, a) in acts.iter().enumerate() {
        let mut a = a.clone();
        if is_create {
            a.remove("id");
        }
        let mut cols = scalar_cols(ACTION_FIELDS, &a);
        cols.push(("order", ColTy::Int, Value::from(index as i64)));
        let id = a.get("id").and_then(Value::as_i64);
        let aid = update_or_create(db, T_ACTION, action_defaults(), id, &cols).await?;
        for (name, table, defaults, fk) in [
            ("email", T_EMAIL, email_defaults(), "email_id"),
            ("webhook", T_WEBHOOK, webhook_defaults(), "webhook_id"),
        ] {
            if let Some(Value::Object(data)) = a.get(name) {
                serializers::revalidate_nested(name, data)?;
                let fields = if name == "email" { EMAIL_FIELDS } else { WEBHOOK_FIELDS };
                let ncols = scalar_cols(fields, data);
                let nid = data.get("id").and_then(Value::as_i64);
                let nid = update_or_create(db, table, defaults, nid, &ncols).await?;
                update(db, T_ACTION, aid, &vec![(fk, ColTy::Int, Value::from(nid))]).await?;
            }
        }
        set_m2m_from_attrs(db, ACTION_FIELDS, aid, &a).await?;
        set_actions.push(aid);
    }
    Ok(set_actions)
}

pub async fn workflow_create(db: &mut PgConnection, input: &WorkflowInput) -> Result<i64, VErr> {
    let cols = scalar_cols(WORKFLOW_FIELDS, &input.attrs);
    let id = insert(db, T_WORKFLOW, &overlay(workflow_defaults(), &cols), None).await?;
    update_triggers_and_actions(db, id, input.triggers.as_deref(), input.actions.as_deref(), true).await?;
    Ok(id)
}

pub async fn workflow_update(db: &mut PgConnection, row: &Map<String, Value>, input: &WorkflowInput) -> Result<i64, VErr> {
    let id = row.get("id").and_then(Value::as_i64).ok_or_else(|| VErr::Server("workflow without id".into()))?;
    let current: Cols = workflow_defaults()
        .into_iter()
        .map(|(c, ty, d)| (c, ty, row.get(c).cloned().unwrap_or(d)))
        .collect();
    let cols = scalar_cols(WORKFLOW_FIELDS, &input.attrs);
    save_full(db, T_WORKFLOW, Some(id), &overlay(current, &cols)).await?;
    update_triggers_and_actions(db, id, input.triggers.as_deref(), input.actions.as_deref(), false).await?;
    prune(db).await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_order_small() {
        assert_eq!(py_set_order(&[3, 1, 2]), vec![1, 2, 3]);
        // 9 % 8 == 1 → before 2
        assert_eq!(py_set_order(&[2, 9]), vec![9, 2]);
        assert_eq!(py_set_order(&[1, 1, 1]), vec![1]);
    }

    #[test]
    fn set_order_resize() {
        // 5 elements → resize to 32 slots (5*5 >= 7*3), then ascending modulo 32
        assert_eq!(py_set_order(&[40, 1, 2, 3, 4]), vec![1, 2, 3, 4, 40]);
        assert_eq!(py_set_order(&[33, 1, 2, 3, 4]), vec![33, 2, 3, 4, 1]);
        assert_eq!(py_set_order(&[17, 9, 1, 25, 33, 41, 2]), vec![33, 1, 2, 9, 41, 17, 25]);
    }

    #[test]
    fn sources() {
        assert_eq!(sources_db(&serde_json::json!([2, 1])), Value::String("2,1".into()));
        assert_eq!(sources_db(&serde_json::json!([])), Value::String(String::new()));
    }
}
