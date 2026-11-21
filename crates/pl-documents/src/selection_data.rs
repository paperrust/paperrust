//! `selection_data` – document counts per correspondent/tag/type/storage path/custom field for a
//! selection of documents.
//!
//! upstream: src/documents/views.py@v3.4.2 (`DocumentViewSet._get_selection_data_for_queryset`,
//! `UnifiedSearchViewSet.list` with `include_selection_data`, `SelectionDataView.post`),
//! src/documents/permissions.py@v3.4.2 (`annotate_document_count_by_ids`).
//!
//! Query shape (PERF-SCALE §5 #1 / Q3, 2026-11-10): **one** query. The selection appears
//! once in the query as `WITH sel AS MATERIALIZED (…)` – either the filtered document query
//! itself as a subquery (list: no ID list over the wire) or `id = ANY($1::int8[])`
//! (explicit IDs: POST, full-text hits). One `GROUP BY` per relation, only over the selection
//! (FK relations directly from `sel`, tags/custom fields via the join table with
//! `document_id IN (SELECT id FROM sel)`), then a right join onto all objects. `COUNT(*)` instead of
//! `COUNT(DISTINCT …)`: `sel` contains each document id at most once (primary key; the
//! sort joins are 1:1), `(document_id, tag_id)` and `(document_id, field_id)` are
//! UNIQUE – the result is identical.
//!
//! Order of the objects (explicit per relation, carried into the output via `row_number()`):
//! * List: FK relations `id` (upstream `Count(distinct)` + `GROUP BY` → GroupAggregate by id),
//!   tags `name` (Meta.ordering), custom fields `created` (Meta.ordering) – each with `id` as
//!   tie-breaker (upstream has none; for equal name/timestamp it depends on the plan).
//! * POST: upstream without ORDER BY (HashAggregate, plan-dependent – docs/contract/pl-documents.md);
//!   PaperRust always returns by `id`. Parity rule: the harness compares the `selected_*` lists
//!   of the POST response sorted by `id` (tests/parity/harness/plharness/normalize.py).

use std::collections::HashMap;

use pl_core::sqlbind::SqlxBinder;
use sea_query::{PostgresQueryBuilder, Query, SelectStatement};
use sea_query_binder::SqlxValues;
use serde_json::{Map, Value, json};
use sqlx::Row;

use crate::db::{ApiResult, a};
use crate::listing::DOC;

/// Which response is built (counting method and order differ, see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// `_get_selection_data_for_queryset` (list, full-text list)
    List,
    /// `SelectionDataView.post`
    Post,
}

/// Output key, object table, kind of counting.
const RELATIONS: [(&str, &str, Rel); 5] = [
    ("selected_correspondents", "documents_correspondent", Rel::Fk("correspondent_id")),
    ("selected_tags", "documents_tag", Rel::Through("documents_document_tags", "tag_id")),
    ("selected_document_types", "documents_documenttype", Rel::Fk("document_type_id")),
    ("selected_storage_paths", "documents_storagepath", Rel::Fk("storage_path_id")),
    ("selected_custom_fields", "documents_customfield", Rel::Through("documents_customfieldinstance", "field_id")),
];

#[derive(Debug, Clone, Copy)]
enum Rel {
    /// Column of `documents_document` (carried along in `sel`)
    Fk(&'static str),
    /// Join table with `document_id` and the object column
    Through(&'static str, &'static str),
}

/// Columns of `sel` other than `id`.
const SEL_FK_COLS: [&str; 3] = ["correspondent_id", "document_type_id", "storage_path_id"];

/// The one counting query (without the `WITH sel …` head). Columns: `rel` (index into [`RELATIONS`]),
/// `id`, `n`, `ord`; sorted by `rel`, `ord`.
fn counts_sql(variant: Variant) -> String {
    objects_sql(variant, |i| match RELATIONS[i].2 {
        Rel::Fk(fk) => format!("SELECT s.{fk} AS k, COUNT(*) AS n FROM sel s WHERE s.{fk} IS NOT NULL GROUP BY s.{fk}"),
        Rel::Through(through, col) => {
            // List: CustomFieldInstance.objects (without deleted); POST: join without manager
            let deleted =
                if variant == Variant::List && through == "documents_customfieldinstance" { " AND j.deleted_at IS NULL" } else { "" };
            format!(
                "SELECT j.{col} AS k, COUNT(*) AS n FROM {through} j \
                 WHERE j.document_id IN (SELECT s.id FROM sel s){deleted} GROUP BY j.{col}"
            )
        }
    })
}

/// `work_mem` only for the counting query (PERF-SCALE §12): for large selections (1 million documents)
/// the materialized selection (≈ 23 MB) and the hash table of the selection ids for the
/// join tables (≈ 40 MB) do not fit into the default of 4 MB; PostgreSQL spilled both to
/// temporary files and split the hash join into 8 passes. `SET LOCAL` only applies to
/// this transaction; small selections still use only what they need.
const SELECTION_WORK_MEM: &str = "SET LOCAL work_mem = '96MB'";

/// Only for counting from the counters ([`for_all_unfiltered`]): the correction sets (versions,
/// exclusions, shared foreign documents) are mostly small compared with the join tables. With
/// the default `random_page_cost = 4` the planner considers 15 000 index accesses more expensive than
/// reading all 3 million field values (measured 1.6 s instead of 0.4 s); 1.1 corresponds to SSD/cache. For
/// large correction sets it still picks the hash join. Only changes plans.
const COUNTER_PLAN_COST: &str = "SET LOCAL random_page_cost = 1.1";

async fn run(db: &sqlx::PgPool, sql: &str, args: Args) -> ApiResult<Value> {
    let mut tx = db.begin().await?;
    sqlx::query(SELECTION_WORK_MEM).execute(&mut *tx).await?;
    let rows = match args {
        Args::Values(v) => sqlx::query_with(sql, v).fetch_all(&mut *tx).await?,
        Args::Ids(ids) => sqlx::query(sql).bind(ids).fetch_all(&mut *tx).await?,
        Args::None => {
            sqlx::query(COUNTER_PLAN_COST).execute(&mut *tx).await?;
            sqlx::query(sql).fetch_all(&mut *tx).await?
        }
    };
    tx.commit().await?;
    let mut lists: Vec<Vec<Value>> = vec![Vec::new(); RELATIONS.len()];
    for r in &rows {
        let rel: i32 = r.try_get("rel")?;
        let id: i64 = r.try_get("id")?;
        let n: i64 = r.try_get("n")?;
        if let Some(l) = usize::try_from(rel).ok().and_then(|i| lists.get_mut(i)) {
            l.push(json!({"id": id, "document_count": n}));
        }
    }
    let mut out = Map::new();
    for ((key, _, _), l) in RELATIONS.iter().zip(lists) {
        out.insert((*key).into(), Value::Array(l));
    }
    Ok(Value::Object(out))
}

enum Args {
    Values(SqlxValues),
    Ids(Vec<i64>),
    None,
}

/// Selection = the filtered document query (`SELECT "documents_document"."id" FROM
/// "documents_document" …`, as `listing::filtered_documents`), ordering is removed.
pub async fn for_query(db: &sqlx::PgPool, q: &SelectStatement, variant: Variant) -> ApiResult<Value> {
    let (sql, values) = sel_from_query(q).build_sqlx(PostgresQueryBuilder);
    let full = format!("WITH sel AS MATERIALIZED ({sql}) {}", counts_sql(variant));
    run(db, &full, Args::Values(values)).await
}

/// `sel` query from the filtered list: without ORDER BY, plus the FK columns.
fn sel_from_query(q: &SelectStatement) -> SelectStatement {
    let mut s = q.clone();
    s.clear_order_by();
    for c in SEL_FK_COLS {
        s.column((a(DOC), a(c)));
    }
    s
}

/// Selection = explicit document ids (`id = ANY($1::int8[])`).
pub async fn for_ids(db: &sqlx::PgPool, ids: &[i64], variant: Variant) -> ApiResult<Value> {
    let full = format!(
        "WITH sel AS MATERIALIZED (SELECT d.id, d.{} FROM documents_document d WHERE d.id = ANY($1::int8[])) {}",
        SEL_FK_COLS.join(", d."),
        counts_sql(variant)
    );
    run(db, &full, Args::Ids(ids.to_vec())).await
}

// ------------------------------------------------------------ "all" without filter: counters

/// Predicate without any restriction (`Pred::True`, also as an empty/nested AND)?
pub fn pred_is_unfiltered(p: &pl_domain::filter::Pred) -> bool {
    use pl_domain::filter::Pred;
    match p {
        Pred::True => true,
        Pred::And(v) => v.iter().all(pred_is_unfiltered),
        _ => false,
    }
}

/// List without filter? (`?search=` and all FilterSet fields have no effect; AST path only.)
pub async fn list_is_unfiltered(db: &sqlx::PgPool, tz: chrono_tz::Tz, params: &pl_core::QueryParams) -> bool {
    let fctx = pl_docfilter::FilterContext::new(tz);
    match pl_docfilter::ast::parse(params, &fctx, db).await {
        Ok(f) => pred_is_unfiltered(&f.pred),
        Err(_) => false,
    }
}

/// `all=true` selection without filter? (no full-text keys, lenient ORM filters have no effect)
pub async fn selection_is_unfiltered(db: &sqlx::PgPool, tz: chrono_tz::Tz, filters: &Map<String, Value>) -> bool {
    if crate::selection_filter::SEARCH_PARAM_NAMES.iter().any(|k| filters.contains_key(*k)) {
        return false;
    }
    let params = crate::selection_filter::orm_params(filters);
    let fctx = pl_docfilter::FilterContext::new(tz);
    match pl_docfilter::ast::parse_filters(&params, &fctx, db, true).await {
        Ok(p) => pred_is_unfiltered(&p),
        Err(_) => false,
    }
}

/// Do the counters of the own data model exist for all five relations (stages 1–3:
/// `paperrust_label_count_live` with kind 6, `paperrust_document_tag`, `paperrust_field_value`)?
/// Cached for 60 s per database.
async fn counters_available(db: &sqlx::PgPool) -> bool {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, bool)>>> = OnceLock::new();
    let key = db.connect_options().get_database().unwrap_or_default().to_string();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, v)) = cache.lock().ok().and_then(|m| m.get(&key).copied())
        && at.elapsed() < Duration::from_secs(60)
    {
        return v;
    }
    let v = sqlx::query_scalar::<_, bool>(
        "SELECT to_regclass('paperrust_label_count_live') IS NOT NULL \
           AND to_regclass('paperrust_field_value') IS NOT NULL \
           AND to_regclass('paperrust_document_tag') IS NOT NULL \
           AND EXISTS (SELECT 1 FROM pg_class WHERE relname = 'paperrust_document' AND relkind = 'r')",
    )
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if let Ok(mut m) = cache.lock() {
        m.insert(key, (Instant::now(), v));
    }
    v
}

/// Kind in the counter for each entry of [`RELATIONS`] (2 tag, 3 correspondent, 4 type, 5 path, 6 field).
const COUNTER_KIND: [i16; 5] = [3, 2, 4, 5, 6];

/// Selection "all visible, non-deleted root documents except `excluded`" (list without filter,
/// `all=true` without filter) via the label counters (PERF-SCALE §13). `None`: counters missing or
/// inactive user → previous path.
///
/// Calculation (same result as counting over `sel`, contract pl-documents "selection all without
/// filter"): `paperrust_label_count_live` counts non-deleted documents **including versions** per
/// label and visibility owner (owner of the root, 0 = none). Visible to the user are
/// the buckets `B` (superuser: all; otherwise `{uid, 0}`) plus shared foreign roots. So
///
/// `count(sel) = Σ_B counter − contribution(live versions in B) − contribution(excluded roots in B)
///             + contribution(shared foreign roots, not excluded)`
///
/// The corrected sets are small (versions, exclusions; shares only for restricted
/// users). Custom fields: the counter counts non-deleted field values = list
/// (`CustomFieldInstance.objects`); POST upstream also counts deleted field values
/// (join without manager) – these are added directly for the selection (partial index
/// `paperrust_field_value_deleted`).
pub async fn for_all_unfiltered(db: &sqlx::PgPool, user: &pl_authz::User, excluded: &[i64], variant: Variant) -> ApiResult<Option<Value>> {
    if !user.is_active || !counters_available(db).await {
        return Ok(None);
    }
    let sql = all_unfiltered_sql(user, excluded, variant);
    Ok(Some(run(db, &sql, Args::None).await?))
}

/// The query for [`for_all_unfiltered`].
fn all_unfiltered_sql(user: &pl_authz::User, excluded: &[i64], variant: Variant) -> String {
    let su = user.is_superuser;
    let uid = i32::try_from(user.id).unwrap_or(-1);
    let excl = if excluded.is_empty() {
        None
    } else {
        let mut v: Vec<i64> = excluded.to_vec();
        v.sort_unstable();
        v.dedup();
        Some(v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","))
    };
    let cols = "d.id, d.correspondent_id, d.document_type_id, d.storage_path_id";
    let in_buckets = |owner: &str| if su { String::new() } else { format!(" AND COALESCE({owner}, 0) IN ({uid}, 0)") };
    // Visibility of a root row `d` (as the list: pl_store::docsql::Visibility)
    let vis = pl_store::docsql::Visibility::for_user(user)
        .root_row("d")
        .map(|e| {
            let s = Query::select().expr(e).to_string(PostgresQueryBuilder);
            format!(" AND ({})", s.trim_start_matches("SELECT ").trim())
        })
        .unwrap_or_default();
    let not_excl = excl.as_ref().map(|l| format!(" AND d.id NOT IN ({l})")).unwrap_or_default();

    let mut adj = vec![format!(
        "SELECT {cols}, -1 AS sgn FROM paperrust_document d{} \
         WHERE d.root_document_id IS NOT NULL AND d.deleted_at IS NULL{}",
        if su { "" } else { " LEFT JOIN paperrust_document r ON r.id = d.root_document_id" },
        in_buckets("r.owner_id")
    )];
    if let Some(l) = &excl {
        adj.push(format!(
            "SELECT {cols}, -1 FROM paperrust_document d \
             WHERE d.id IN ({l}) AND d.root_document_id IS NULL AND d.deleted_at IS NULL{}",
            in_buckets("d.owner_id")
        ));
    }
    if !su {
        // shared foreign roots: of the list's visibility (`Visibility::root_row`:
        // own OR ownerless OR shared) only "shared" remains here. As the sole
        // AND condition it becomes a semi-join over the shares – the planner then knows the
        // order of magnitude (with the OR it estimated hundreds of thousands and hashed all 5 million
        // tag assignments for 15 000 documents: 3.9 s instead of 0.3 s).
        let granted = pl_authz::objects::granted_key_cond(
            user,
            &pl_authz::models::DOCUMENT,
            "view_document",
            sea_query::Expr::col((a("d"), a("id"))).into(),
        );
        let granted = Query::select().expr(granted).to_string(PostgresQueryBuilder);
        let granted = granted.trim_start_matches("SELECT ").trim();
        adj.push(format!(
            "SELECT {cols}, 1 FROM paperrust_document d \
             WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL \
             AND d.owner_id IS NOT NULL AND d.owner_id <> {uid} AND {granted}{not_excl}"
        ));
    }
    let buckets = if su { String::new() } else { format!(" WHERE owner_bucket IN ({uid}, 0)") };
    let rel_of_kind = COUNTER_KIND.iter().enumerate().map(|(i, k)| format!("WHEN {k} THEN {i}")).collect::<Vec<_>>().join(" ");
    let mut cnt = vec![
        format!("SELECT CASE kind {rel_of_kind} END AS rel, k, n FROM lc"),
        "SELECT 0, a.correspondent_id, a.sgn FROM adj a WHERE a.correspondent_id IS NOT NULL".to_string(),
        "SELECT 1, t.tag_id, a.sgn FROM adj a JOIN paperrust_document_tag t ON t.document_id = a.id".to_string(),
        "SELECT 2, a.document_type_id, a.sgn FROM adj a WHERE a.document_type_id IS NOT NULL".to_string(),
        "SELECT 3, a.storage_path_id, a.sgn FROM adj a WHERE a.storage_path_id IS NOT NULL".to_string(),
        "SELECT 4, fv.field_id, a.sgn FROM adj a JOIN paperrust_field_value fv \
         ON fv.document_id = a.id AND fv.deleted_at IS NULL"
            .to_string(),
    ];
    if variant == Variant::Post {
        cnt.push(format!(
            "SELECT 4, fv.field_id, 1 FROM paperrust_field_value fv JOIN paperrust_document d ON d.id = fv.document_id \
             WHERE fv.deleted_at IS NOT NULL AND d.root_document_id IS NULL AND d.deleted_at IS NULL{vis}{not_excl}"
        ));
    }
    let head = format!(
        // = paperrust_label_count_live (counter + not yet folded differences), but in one
        // grouping instead of two (the view additionally groups per bucket: 0.4 s instead of 0.1 s)
        "WITH lc AS (SELECT kind, label_id AS k, SUM(n)::int8 AS n FROM \
         (SELECT kind, label_id, owner_bucket, n FROM paperrust_label_count \
          UNION ALL SELECT kind, label_id, owner_bucket, d FROM paperrust_label_count_delta) lcx{buckets} \
         GROUP BY kind, label_id), \
         adj AS MATERIALIZED ({}), \
         cnt AS ({}), \
         agg AS (SELECT rel, k, SUM(n)::int8 AS n FROM cnt WHERE rel IS NOT NULL GROUP BY rel, k) ",
        adj.join(" UNION ALL "),
        cnt.join(" UNION ALL ")
    );
    format!("{head}{}", objects_sql(variant, |i| format!("SELECT k, n FROM agg WHERE rel = {i}")))
}

/// Right part: all objects per relation with their count from `counts(i)` (columns `k`, `n`),
/// order as [`counts_sql`].
fn objects_sql(variant: Variant, counts: impl Fn(usize) -> String) -> String {
    let mut parts = Vec::new();
    for (i, (_, table, _)) in RELATIONS.iter().enumerate() {
        let order = match (variant, *table) {
            (Variant::List, "documents_tag") => "o.name, o.id",
            (Variant::List, "documents_customfield") => "o.created, o.id",
            _ => "o.id",
        };
        parts.push(format!(
            "SELECT {i} AS rel, o.id::int8 AS id, COALESCE(c.n, 0)::int8 AS n, \
             row_number() OVER (ORDER BY {order}) AS ord \
             FROM {table} o LEFT JOIN ({}) c ON c.k = o.id",
            counts(i)
        ));
    }
    format!("{} ORDER BY rel, ord", parts.join(" UNION ALL "))
}

/// Tests only: counts of a response as `{key: {id: n}}`.
#[doc(hidden)]
pub fn as_maps(v: &Value) -> HashMap<String, HashMap<i64, i64>> {
    let mut out = HashMap::new();
    if let Some(m) = v.as_object() {
        for (k, l) in m {
            let mut inner = HashMap::new();
            for e in l.as_array().into_iter().flatten() {
                if let (Some(id), Some(n)) = (e["id"].as_i64(), e["document_count"].as_i64()) {
                    inner.insert(id, n);
                }
            }
            out.insert(k.clone(), inner);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::{Expr, Order, Query};

    #[test]
    fn list_order_and_deleted_filter() {
        let s = counts_sql(Variant::List);
        assert!(s.contains("ORDER BY o.name, o.id"), "{s}");
        assert!(s.contains("ORDER BY o.created, o.id"), "{s}");
        assert!(s.contains("j.deleted_at IS NULL"), "{s}");
        assert!(!s.contains("DISTINCT"), "{s}");
        assert!(s.ends_with("ORDER BY rel, ord"), "{s}");
        assert_eq!(s.matches("UNION ALL").count(), 4);
    }

    #[test]
    fn post_order_by_id_and_no_manager() {
        let s = counts_sql(Variant::Post);
        assert!(!s.contains("o.name"), "{s}");
        assert!(!s.contains("deleted_at"), "{s}");
        assert_eq!(s.matches("ORDER BY o.id").count(), 5);
    }

    #[test]
    fn sel_from_query_drops_order_and_adds_fk_columns() {
        let q = Query::select()
            .column((a(DOC), a("id")))
            .from(a(DOC))
            .and_where(Expr::col((a(DOC), a("deleted_at"))).is_null())
            .order_by((a(DOC), a("created")), Order::Desc)
            .to_owned();
        let s = sel_from_query(&q).to_string(PostgresQueryBuilder);
        assert!(!s.contains("ORDER BY"), "{s}");
        assert!(s.starts_with(r#"SELECT "documents_document"."id", "documents_document"."correspondent_id", "documents_document"."document_type_id", "documents_document"."storage_path_id" FROM"#), "{s}");
    }

    fn user(id: i64, su: bool) -> pl_authz::User {
        pl_authz::User {
            id,
            username: format!("u{id}"),
            password: String::new(),
            first_name: String::new(),
            last_name: String::new(),
            email: String::new(),
            is_active: true,
            is_staff: false,
            is_superuser: su,
            last_login: None,
            date_joined: chrono::Utc::now(),
            groups: vec![3],
            permissions: Default::default(),
        }
    }

    #[test]
    fn unfiltered_preds() {
        use pl_domain::filter::Pred;
        assert!(pred_is_unfiltered(&Pred::True));
        assert!(pred_is_unfiltered(&Pred::And(vec![])));
        assert!(pred_is_unfiltered(&Pred::And(vec![Pred::True, Pred::And(vec![])])));
        assert!(!pred_is_unfiltered(&Pred::False));
        assert!(!pred_is_unfiltered(&Pred::Or(vec![])));
        assert!(!pred_is_unfiltered(&Pred::Num { col: pl_domain::filter::NumCol::Id, op: pl_domain::filter::CmpOp::Eq, value: 1 }));
    }

    #[test]
    fn all_unfiltered_superuser() {
        let s = all_unfiltered_sql(&user(1, true), &[7, 5, 7], Variant::List);
        // subtract all buckets, versions and exclusions, no shares
        assert!(!s.contains("WHERE owner_bucket"), "{s}");
        assert!(s.contains("d.root_document_id IS NOT NULL AND d.deleted_at IS NULL"), "{s}");
        assert!(s.contains("d.id IN (5,7) AND d.root_document_id IS NULL"), "{s}");
        assert!(!s.contains(", 1 FROM"), "{s}");
        assert!(s.contains("fv.deleted_at IS NULL"), "{s}");
        assert!(!s.contains("fv.deleted_at IS NOT NULL"), "{s}");
        // Order of the list
        assert!(s.contains("ORDER BY o.name, o.id") && s.contains("ORDER BY o.created, o.id"), "{s}");
        assert!(s.ends_with("ORDER BY rel, ord"), "{s}");
        assert!(s.contains("CASE kind WHEN 3 THEN 0 WHEN 2 THEN 1 WHEN 4 THEN 2 WHEN 5 THEN 3 WHEN 6 THEN 4 END"), "{s}");
    }

    #[test]
    fn all_unfiltered_restricted_post() {
        let s = all_unfiltered_sql(&user(22, false), &[], Variant::Post);
        assert!(s.contains("WHERE owner_bucket IN (22, 0)"), "{s}");
        assert!(s.contains("COALESCE(r.owner_id, 0) IN (22, 0)"), "{s}");
        // shared foreign roots added
        assert!(s.contains("d.owner_id IS NOT NULL AND d.owner_id <> 22"), "{s}");
        assert!(s.contains(r#""d"."id""#), "{s}");
        // POST: deleted field values added directly, without exclusion list
        assert!(s.contains("fv.deleted_at IS NOT NULL"), "{s}");
        assert!(!s.contains("NOT IN"), "{s}");
        assert_eq!(s.matches("ORDER BY o.id").count(), 5, "{s}");
    }

    #[test]
    fn maps_helper() {
        let v = json!({"selected_tags": [{"id": 3, "document_count": 2}]});
        assert_eq!(as_maps(&v)["selected_tags"][&3], 2);
    }
}
