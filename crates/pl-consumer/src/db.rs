// Database access of the consumer (Django tables of upstream v3.4.2, schema reference/schema/postgres.sql).
//
// upstream: src/documents/models.py@v3.4.2 (Document, Tag.get_ancestors_pks, Document.add_nested_tags,
//           CustomFieldInstance.TYPE_TO_DATA_STORE_NAME_MAP), src/documents/matching.py@v3.4.2
//           (candidate selection in match_*), src/documents/permissions.py@v3.4.2 (permitted_object_ids)
//
// SQL via sea-query (dialect-neutral); every function runs on one connection (usually the
// transaction of the ingestion).

use chrono::{DateTime, NaiveDate, Utc};
use sea_query::{Alias, Expr, Func, Order, PostgresQueryBuilder, Query, SelectStatement, SimpleExpr};
use pl_core::sqlbind::SqlxBinder as _;
use serde_json::Value;
use sqlx::{PgConnection, Row};

pub(crate) fn a(s: &str) -> Alias {
    Alias::new(s)
}

pub(crate) fn col(t: &str, c: &str) -> Expr {
    Expr::col((a(t), a(c)))
}

/// Microsecond-precise time (like Python/PostgreSQL).
pub fn now_micros() -> DateTime<Utc> {
    trunc_micros(Utc::now())
}

pub fn trunc_micros(t: DateTime<Utc>) -> DateTime<Utc> {
    let ns = t.timestamp_subsec_nanos();
    t - chrono::Duration::nanoseconds(i64::from(ns % 1000))
}

pub(crate) async fn fetch_all(conn: &mut PgConnection, q: &SelectStatement) -> Result<Vec<sqlx::postgres::PgRow>, sqlx::Error> {
    let (sql, vals) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, vals).fetch_all(conn).await
}

pub(crate) async fn fetch_optional(
    conn: &mut PgConnection,
    q: &SelectStatement,
) -> Result<Option<sqlx::postgres::PgRow>, sqlx::Error> {
    let (sql, vals) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, vals).fetch_optional(conn).await
}

pub(crate) async fn ids(conn: &mut PgConnection, q: &SelectStatement) -> Result<Vec<i64>, sqlx::Error> {
    let rows = fetch_all(conn, q).await?;
    rows.iter().map(|r| r.try_get::<i32, _>(0).map(i64::from)).collect()
}

/// Create `documents_document_tags` rows `(document_id, tag_id)` (`bulk_create`, blocks of 5000).
///
/// The **only** place that builds this INSERT (ingestion, workflows, bulk_edit): sqlx remembers
/// prepared statements per connection by SQL text. If a second path binds the same text
/// with other types (formerly bulk_edit `int8`, consumer `int4`), it fails with
/// `incorrect binary data format in bind parameter` or `insufficient data left in message`
/// (tests/stress/BEFUNDE.md B3). Both columns are `integer` → always `i32`.
/// `ignore_conflicts` = `bulk_create(…, ignore_conflicts=True)` (`ON CONFLICT DO NOTHING`).
pub async fn insert_document_tags(conn: &mut PgConnection, pairs: &[(i64, i64)], ignore_conflicts: bool) -> Result<(), sqlx::Error> {
    for chunk in pairs.chunks(5000) {
        let mut ins = Query::insert();
        ins.into_table(a("documents_document_tags")).columns([a("document_id"), a("tag_id")]);
        let mut rows = 0usize;
        for (d, t) in chunk {
            let (Ok(d), Ok(t)) = (i32::try_from(*d), i32::try_from(*t)) else {
                // ids outside int4 cannot exist
                continue;
            };
            ins.values_panic([d.into(), t.into()]);
            rows += 1;
        }
        if rows == 0 {
            continue;
        }
        if ignore_conflicts {
            ins.on_conflict(sea_query::OnConflict::new().do_nothing().to_owned());
        }
        let (sql, v) = ins.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, v).execute(&mut *conn).await?;
    }
    Ok(())
}

/// Lock the document rows of `ids` in **ascending id order** (`FOR NO KEY UPDATE`), like
/// the sync trigger `paperrust_sync_bump` (pl-sync 0003). Whoever changes several document rows in one
/// transaction locks them up front this way; two transactions then wait for each other instead of
/// deadlocking (FINDINGS B4). Only useful inside a transaction (in autocommit the lock ends with
/// the statement → there [`touch_modified_sorted`]).
pub async fn lock_documents_sorted(conn: &mut PgConnection, ids: &[i64]) -> Result<(), sqlx::Error> {
    let ids32 = ids32_sorted(ids);
    if ids32.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "SELECT id FROM {} WHERE id = ANY($1) ORDER BY id FOR NO KEY UPDATE",
        pl_authz::grants::document_row_table()
    );
    sqlx::query(&sql).bind(&ids32).execute(&mut *conn).await?;
    Ok(())
}

/// `Document.objects.filter(pk__in=ids).update(modified=at)` with row locks in id order
/// in **one** statement (also valid in autocommit). A plain
/// `UPDATE … WHERE id = ANY($1)` locks in table order and deadlocks with the
/// sorting sync trigger of a concurrent bulk change (FINDINGS B4).
pub async fn touch_modified_sorted(conn: &mut PgConnection, ids: &[i64], at: DateTime<Utc>) -> Result<u64, sqlx::Error> {
    let ids32 = ids32_sorted(ids);
    if ids32.is_empty() {
        return Ok(0);
    }
    let t = pl_authz::grants::document_row_table();
    let sql = format!(
        "UPDATE {t} x SET modified = $2 FROM (SELECT y.id FROM {t} y WHERE y.id = ANY($1) AND y.deleted_at IS NULL \
         ORDER BY y.id FOR NO KEY UPDATE) s WHERE x.id = s.id"
    );
    Ok(sqlx::query(&sql).bind(&ids32).bind(at).execute(&mut *conn).await?.rows_affected())
}

fn ids32_sorted(ids: &[i64]) -> Vec<i32> {
    let mut v: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Deadlock (`40P01`) or serialization failure (`40001`): PostgreSQL aborted the transaction;
/// a complete retry is permissible.
pub fn is_retryable(e: &sqlx::Error) -> bool {
    matches!(e.as_database_error().and_then(|d| d.code()).as_deref(), Some("40P01" | "40001"))
}

/// Wait before retry `attempt` (1, 2, …): 50 ms, 100 ms, 200 ms … with some jitter.
pub fn retry_backoff(attempt: u32) -> std::time::Duration {
    let base = 50u64 << attempt.saturating_sub(1).min(5);
    let jitter = u64::from(std::process::id() ^ chrono::Utc::now().timestamp_subsec_micros()) % 25;
    std::time::Duration::from_millis(base + jitter)
}

/// Maximum number of attempts for [`is_retryable`] errors.
pub const MAX_ATTEMPTS: u32 = 3;

/// A `documents_document` record (fields that the consumer reads/writes).
#[derive(Debug, Clone, PartialEq)]
pub struct DocRow {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub mime_type: String,
    pub checksum: String,
    pub archive_checksum: Option<String>,
    pub page_count: Option<i64>,
    pub created: NaiveDate,
    pub modified: DateTime<Utc>,
    pub added: DateTime<Utc>,
    pub filename: Option<String>,
    pub archive_filename: Option<String>,
    pub original_filename: Option<String>,
    pub archive_serial_number: Option<i64>,
    pub correspondent_id: Option<i64>,
    pub document_type_id: Option<i64>,
    pub storage_path_id: Option<i64>,
    pub owner_id: Option<i64>,
    pub root_document_id: Option<i64>,
    pub version_label: Option<String>,
    pub version_index: Option<i64>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub restored_at: Option<DateTime<Utc>>,
    pub transaction_id: Option<uuid::Uuid>,
}

const DOC_COLS: &[&str] = &[
    "id", "title", "content", "mime_type", "checksum", "archive_checksum", "page_count", "created", "modified", "added",
    "filename", "archive_filename", "original_filename", "archive_serial_number", "correspondent_id",
    "document_type_id", "storage_path_id", "owner_id", "root_document_id", "version_label", "version_index",
    "deleted_at", "restored_at", "transaction_id",
];

fn opt_i(r: &sqlx::postgres::PgRow, c: &str) -> Result<Option<i64>, sqlx::Error> {
    Ok(r.try_get::<Option<i32>, _>(c)?.map(i64::from))
}

fn doc_from_row(r: &sqlx::postgres::PgRow) -> Result<DocRow, sqlx::Error> {
    Ok(DocRow {
        id: i64::from(r.try_get::<i32, _>("id")?),
        title: r.try_get("title")?,
        content: r.try_get("content")?,
        mime_type: r.try_get("mime_type")?,
        checksum: r.try_get("checksum")?,
        archive_checksum: r.try_get("archive_checksum")?,
        page_count: opt_i(r, "page_count")?,
        created: r.try_get("created")?,
        modified: r.try_get("modified")?,
        added: r.try_get("added")?,
        filename: r.try_get("filename")?,
        archive_filename: r.try_get("archive_filename")?,
        original_filename: r.try_get("original_filename")?,
        archive_serial_number: opt_i(r, "archive_serial_number")?,
        correspondent_id: opt_i(r, "correspondent_id")?,
        document_type_id: opt_i(r, "document_type_id")?,
        storage_path_id: opt_i(r, "storage_path_id")?,
        owner_id: opt_i(r, "owner_id")?,
        root_document_id: opt_i(r, "root_document_id")?,
        version_label: r.try_get("version_label")?,
        version_index: opt_i(r, "version_index")?,
        deleted_at: r.try_get("deleted_at")?,
        restored_at: r.try_get("restored_at")?,
        transaction_id: r.try_get("transaction_id")?,
    })
}

/// `Document.global_objects.get(pk=...)` (incl. trash).
pub async fn load_document(conn: &mut PgConnection, id: i64) -> Result<Option<DocRow>, sqlx::Error> {
    if let Some(d) = crate::lookup::doc(id) {
        return Ok(Some(d));
    }
    let mut q = Query::select();
    q.from(a("documents_document"));
    for c in DOC_COLS {
        q.column(a(c));
    }
    q.and_where(Expr::col(a("id")).eq(id));
    match fetch_optional(conn, &q).await? {
        Some(r) => {
            let d = doc_from_row(&r)?;
            crate::lookup::set_doc(&d);
            Ok(Some(d))
        }
        None => Ok(None),
    }
}

/// Several documents (incl. trash) in one query; order undefined.
pub async fn load_documents(conn: &mut PgConnection, ids: &[i64]) -> Result<Vec<DocRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let sql = format!("SELECT {} FROM documents_document WHERE id = ANY($1)", DOC_COLS.join(", "));
    let rows = sqlx::query(&sql).bind(ids32).fetch_all(&mut *conn).await?;
    rows.iter().map(doc_from_row).collect()
}

/// Insert a new document; returns the ID.
pub async fn insert_document(conn: &mut PgConnection, d: &DocRow) -> Result<i64, sqlx::Error> {
    let cols: Vec<Alias> = DOC_COLS.iter().filter(|c| **c != "id").map(|c| a(c)).collect();
    let vals: Vec<SimpleExpr> = vec![
        d.title.clone().into(),
        d.content.clone().into(),
        d.mime_type.clone().into(),
        d.checksum.clone().into(),
        d.archive_checksum.clone().into(),
        d.page_count.map(|v| v as i32).into(),
        d.created.into(),
        d.modified.into(),
        d.added.into(),
        d.filename.clone().into(),
        d.archive_filename.clone().into(),
        d.original_filename.clone().into(),
        d.archive_serial_number.map(|v| v as i32).into(),
        d.correspondent_id.map(|v| v as i32).into(),
        d.document_type_id.map(|v| v as i32).into(),
        d.storage_path_id.map(|v| v as i32).into(),
        d.owner_id.map(|v| v as i32).into(),
        d.root_document_id.map(|v| v as i32).into(),
        d.version_label.clone().into(),
        d.version_index.map(|v| v as i32).into(),
        d.deleted_at.into(),
        d.restored_at.into(),
        d.transaction_id.into(),
    ];
    let mut q = Query::insert();
    q.into_table(a("documents_document")).columns(cols);
    q.values(vals).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    q.returning_col(a("id"));
    let (sql, v) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, v).fetch_one(&mut *conn).await?;
    Ok(i64::from(row.try_get::<i32, _>(0)?))
}

/// `UPDATE documents_document SET ... WHERE id = ...` for the named fields.
pub async fn update_document_fields(conn: &mut PgConnection, d: &DocRow, fields: &[&str]) -> Result<(), sqlx::Error> {
    if fields.is_empty() {
        return Ok(());
    }
    let mut q = Query::update();
    // only columns of the narrow row → directly on the native table (without view trigger)
    let wide = fields.iter().any(|f| matches!(*f, "content" | "archive_checksum" | "filename" | "archive_filename"));
    q.table(a(if wide { "documents_document" } else { pl_authz::grants::document_row_table() }));
    for f in fields {
        let v: SimpleExpr = match *f {
            "title" => d.title.clone().into(),
            "content" => d.content.clone().into(),
            "archive_checksum" => d.archive_checksum.clone().into(),
            "page_count" => d.page_count.map(|v| v as i32).into(),
            "created" => d.created.into(),
            "modified" => d.modified.into(),
            "filename" => d.filename.clone().into(),
            "archive_filename" => d.archive_filename.clone().into(),
            "archive_serial_number" => d.archive_serial_number.map(|v| v as i32).into(),
            "correspondent_id" | "correspondent" => d.correspondent_id.map(|v| v as i32).into(),
            "document_type_id" | "document_type" => d.document_type_id.map(|v| v as i32).into(),
            "storage_path_id" | "storage_path" => d.storage_path_id.map(|v| v as i32).into(),
            "owner_id" | "owner" => d.owner_id.map(|v| v as i32).into(),
            "version_label" => d.version_label.clone().into(),
            "deleted_at" => d.deleted_at.into(),
            "restored_at" => d.restored_at.into(),
            "transaction_id" => d.transaction_id.into(),
            other => return Err(sqlx::Error::Protocol(format!("unknown document field {other}"))),
        };
        let c = match *f {
            "correspondent" => "correspondent_id",
            "document_type" => "document_type_id",
            "storage_path" => "storage_path_id",
            "owner" => "owner_id",
            x => x,
        };
        q.value(a(c), v);
    }
    q.and_where(Expr::col(a("id")).eq(d.id));
    let (sql, v) = q.build_sqlx(PostgresQueryBuilder);
    crate::lookup::forget_doc(d.id);
    sqlx::query_with(&sql, v).execute(&mut *conn).await?;
    Ok(())
}

/// Duplikate (`Document.global_objects.filter(Q(checksum=c) | Q(archive_checksum=c)).order_by("-created")`):
/// (id, title, original_filename, filename, deleted_at)
pub struct DuplicateRow {
    pub id: i64,
    pub title: String,
    pub original_filename: Option<String>,
    pub filename: Option<String>,
    pub in_trash: bool,
}

pub async fn find_duplicates(conn: &mut PgConnection, checksum: &str) -> Result<Vec<DuplicateRow>, sqlx::Error> {
    let q = Query::select()
        .columns([a("id"), a("title"), a("original_filename"), a("filename"), a("deleted_at")])
        .from(a("documents_document"))
        .cond_where(
            sea_query::Cond::any()
                .add(Expr::col(a("checksum")).eq(checksum))
                .add(Expr::col(a("archive_checksum")).eq(checksum)),
        )
        .order_by(a("created"), Order::Desc)
        .to_owned();
    let rows = fetch_all(conn, &q).await?;
    rows.iter()
        .map(|r| {
            Ok(DuplicateRow {
                id: i64::from(r.try_get::<i32, _>("id")?),
                title: r.try_get("title")?,
                original_filename: r.try_get("original_filename")?,
                filename: r.try_get("filename")?,
                in_trash: r.try_get::<Option<DateTime<Utc>>, _>("deleted_at")?.is_some(),
            })
        })
        .collect()
}

/// `Document.global_objects.filter(archive_serial_number=asn)` → first hit: (id, in trash)
pub async fn find_asn(conn: &mut PgConnection, asn: i64) -> Result<Option<(i64, bool)>, sqlx::Error> {
    if asn > i64::from(i32::MAX) || asn < i64::from(i32::MIN) {
        // Column is integer – larger values cannot exist.
        return Ok(None);
    }
    let q = Query::select()
        .columns([a("id"), a("deleted_at")])
        .from(a("documents_document"))
        .and_where(Expr::col(a("archive_serial_number")).eq(asn as i32))
        .order_by(a("id"), Order::Asc)
        .limit(1)
        .to_owned();
    Ok(match fetch_optional(conn, &q).await? {
        Some(r) => Some((
            i64::from(r.try_get::<i32, _>("id")?),
            r.try_get::<Option<DateTime<Utc>>, _>("deleted_at")?.is_some(),
        )),
        None => None,
    })
}

/// Name of an object (`str(obj)`), `None` if not present.
pub async fn object_name(conn: &mut PgConnection, table: &str, id: i64) -> Result<Option<String>, sqlx::Error> {
    if let Some(n) = crate::lookup::name(table, id) {
        return Ok(Some(n));
    }
    let q = Query::select().column(a("name")).from(a(table)).and_where(Expr::col(a("id")).eq(id)).to_owned();
    Ok(match fetch_optional(conn, &q).await? {
        Some(r) => {
            let n: String = r.try_get("name")?;
            crate::lookup::set_name(table, id, &n);
            Some(n)
        }
        None => None,
    })
}

pub async fn exists(conn: &mut PgConnection, table: &str, id: i64) -> Result<bool, sqlx::Error> {
    if crate::lookup::exists(table, id) {
        return Ok(true);
    }
    let q = Query::select().column(a("id")).from(a(table)).and_where(Expr::col(a("id")).eq(id)).to_owned();
    let found = fetch_optional(conn, &q).await?.is_some();
    if found {
        crate::lookup::set_exists(table, id);
    }
    Ok(found)
}

pub async fn username(conn: &mut PgConnection, id: i64) -> Result<Option<String>, sqlx::Error> {
    if let Some(n) = crate::lookup::user(id) {
        return Ok(Some(n));
    }
    let q = Query::select().column(a("username")).from(a("auth_user")).and_where(Expr::col(a("id")).eq(id)).to_owned();
    Ok(match fetch_optional(conn, &q).await? {
        Some(r) => {
            let n: String = r.try_get("username")?;
            crate::lookup::set_user(id, &n);
            Some(n)
        }
        None => None,
    })
}

/// Tag ancestors (`Tag.get_ancestors_pks()`): from the root to the parent, via the
/// parent edge `tn_parent_id` (adjacency list since `pl_objects.0001_paperrust_tag_adjacency`,
/// upstream #14612). `None` if the tag does not exist. Cycle-safe (depth ≤ 64, abort at the
/// first repeated node).
pub async fn tag_ancestors(conn: &mut PgConnection, tag_id: i64) -> Result<Option<Vec<i64>>, sqlx::Error> {
    if let Some(v) = crate::lookup::ancestors(tag_id) {
        return Ok(Some(v));
    }
    let rows: Vec<(i32, i32)> = sqlx::query_as(
        "WITH RECURSIVE up(id, parent, depth) AS ( \
           SELECT id, tn_parent_id, 0 FROM documents_tag WHERE id = $1 \
           UNION ALL \
           SELECT t.id, t.tn_parent_id, up.depth + 1 FROM documents_tag t JOIN up ON t.id = up.parent WHERE up.depth < 64) \
         SELECT id, depth FROM up ORDER BY depth",
    )
    .bind(tag_id as i32)
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let mut out: Vec<i64> = Vec::new();
    for (id, depth) in rows {
        let id = i64::from(id);
        if depth == 0 {
            continue;
        }
        if id == tag_id || out.contains(&id) {
            break;
        }
        out.push(id);
    }
    out.reverse();
    crate::lookup::set_ancestors(tag_id, &out);
    Ok(Some(out))
}

/// All descendants of the tags `tag_ids` without the tags themselves (`get_descendants_pks()`), by id.
pub async fn tags_descendants(conn: &mut PgConnection, tag_ids: &[i64]) -> Result<Vec<i64>, sqlx::Error> {
    if tag_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i32> = tag_ids.iter().map(|i| *i as i32).collect();
    // UNION (not UNION ALL) removes repetitions → also terminates with faulty cycles.
    let rows: Vec<(i32,)> = sqlx::query_as(
        "WITH RECURSIVE down(id) AS ( \
           SELECT id FROM documents_tag WHERE tn_parent_id = ANY($1) \
           UNION \
           SELECT t.id FROM documents_tag t JOIN down ON t.tn_parent_id = down.id) \
         SELECT id FROM down WHERE id <> ALL($1) ORDER BY id",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|(i,)| i64::from(i)).collect())
}

pub async fn tag_descendants(conn: &mut PgConnection, tag_id: i64) -> Result<Vec<i64>, sqlx::Error> {
    tags_descendants(conn, &[tag_id]).await
}

/// Tags of a document: (id, name), order `name` (Tag.Meta.ordering).
pub async fn document_tags(conn: &mut PgConnection, doc_id: i64) -> Result<Vec<(i64, String)>, sqlx::Error> {
    if let Some(v) = crate::lookup::doc_tags(doc_id) {
        return Ok(v);
    }
    let q = Query::select()
        .column((a("t"), a("id")))
        .column((a("t"), a("name")))
        .from_as(a("documents_tag"), a("t"))
        .inner_join(a("documents_document_tags"), col("documents_document_tags", "tag_id").equals((a("t"), a("id"))))
        .and_where(col("documents_document_tags", "document_id").eq(doc_id))
        .order_by((a("t"), a("name")), Order::Asc)
        .order_by((a("t"), a("id")), Order::Asc)
        .to_owned();
    let rows = fetch_all(conn, &q).await?;
    let out: Vec<(i64, String)> =
        rows.iter().map(|r| Ok((i64::from(r.try_get::<i32, _>(0)?), r.try_get(1)?))).collect::<Result<_, sqlx::Error>>()?;
    crate::lookup::set_doc_tags(doc_id, &out);
    Ok(out)
}

/// `document.tags.add(*Tag.objects.filter(id__in=ids))`: adds only missing ones and returns the
/// actually added (id, name) sorted by name (for the m2m audit).
pub async fn add_tags(conn: &mut PgConnection, doc_id: i64, tag_ids: &[i64]) -> Result<Vec<(i64, String)>, sqlx::Error> {
    if tag_ids.is_empty() {
        return Ok(Vec::new());
    }
    let existing: Vec<i64> = document_tags(conn, doc_id).await?.into_iter().map(|(i, _)| i).collect();
    let q = Query::select()
        .columns([a("id"), a("name")])
        .from(a("documents_tag"))
        .and_where(Expr::col(a("id")).is_in(tag_ids.iter().map(|i| *i as i32)))
        .order_by(a("name"), Order::Asc)
        .order_by(a("id"), Order::Asc)
        .to_owned();
    let rows = fetch_all(conn, &q).await?;
    // `self.tags.add(*Tag.objects.filter(id__in=…))`: objs in name order (Meta.ordering) →
    // `_get_target_ids` builds a `set` → `.difference(existing)` → bulk_create in set order.
    let mut by_name = Vec::with_capacity(rows.len());
    for r in &rows {
        by_name.push((i64::from(r.try_get::<i32, _>(0)?), r.try_get::<String, _>(1)?));
    }
    let ids: Vec<i64> = by_name.iter().map(|(i, _)| *i).collect();
    // one statement for all missing tags (bulk_create); row order = order of the set,
    // so that the assignment ids are assigned as upstream
    let missing: Vec<i64> = pl_core::pyset::missing_targets_order(&ids).into_iter().filter(|id| !existing.contains(id)).collect();
    if !missing.is_empty() {
        let pairs: Vec<(i64, i64)> = missing.iter().map(|id| (doc_id, *id)).collect();
        crate::lookup::forget_tags(doc_id);
        // ON CONFLICT DO NOTHING like Django `_add_items` (`bulk_create(ignore_conflicts=True)` with an
        // automatically created join table): two workflow runs on the same document (e.g.
        // "apply retroactively" twice) both read "missing" → otherwise duplicate key (FINDINGS N4b)
        insert_document_tags(conn, &pairs, true).await?;
    }
    // m2m audit: still sorted by name
    Ok(by_name.into_iter().filter(|(i, _)| !existing.contains(i)).collect())
}

/// Tags incl. ancestors (`add_nested_tags`: `Tag.get_hierarchy().with_ancestors(...)`), only
/// existing tags, by id. One query for all tags.
pub async fn with_ancestors(conn: &mut PgConnection, tag_ids: &[i64]) -> Result<Vec<i64>, sqlx::Error> {
    if tag_ids.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(v) = crate::lookup::ancestor_set(tag_ids) {
        return Ok(v);
    }
    let ids: Vec<i32> = tag_ids.iter().map(|i| *i as i32).collect();
    let rows: Vec<(i32,)> = sqlx::query_as(
        "WITH RECURSIVE up(id, parent) AS ( \
           SELECT id, tn_parent_id FROM documents_tag WHERE id = ANY($1) \
           UNION \
           SELECT t.id, t.tn_parent_id FROM documents_tag t JOIN up ON t.id = up.parent) \
         SELECT DISTINCT id FROM up ORDER BY id",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<i64> = rows.into_iter().map(|(i,)| i64::from(i)).collect();
    // remember only complete answers (a missing tag could be created later)
    if tag_ids.iter().all(|t| out.contains(t)) {
        crate::lookup::set_ancestor_set(tag_ids, &out);
    }
    Ok(out)
}

/// `Tag.objects.get(pk=...)` for each ID – `DoesNotExist` text like Django.
pub async fn require_ids(conn: &mut PgConnection, table: &str, model: &str, ids: &[i64]) -> Result<(), String> {
    for id in ids {
        match exists(conn, table, *id).await {
            Ok(true) => {}
            Ok(false) => return Err(format!("{model} matching query does not exist.")),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

/// Custom-Field-Definition.
#[derive(Debug, Clone)]
pub struct CustomFieldDef {
    pub id: i64,
    pub name: String,
    pub data_type: String,
    pub extra_data: Option<Value>,
}

pub async fn custom_fields(conn: &mut PgConnection, ids: &[i64]) -> Result<Vec<CustomFieldDef>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let q = Query::select()
        .columns([a("id"), a("name"), a("data_type"), a("extra_data")])
        .from(a("documents_customfield"))
        .and_where(Expr::col(a("id")).is_in(ids.iter().map(|i| *i as i32)))
        .order_by(a("created"), Order::Asc)
        .order_by(a("id"), Order::Asc)
        .to_owned();
    let rows = fetch_all(conn, &q).await?;
    rows.iter()
        .map(|r| {
            Ok(CustomFieldDef {
                id: i64::from(r.try_get::<i32, _>(0)?),
                name: r.try_get(1)?,
                data_type: r.try_get(2)?,
                extra_data: r.try_get(3)?,
            })
        })
        .collect()
}

/// `CustomFieldInstance.TYPE_TO_DATA_STORE_NAME_MAP`.
pub fn value_field_name(data_type: &str) -> Option<&'static str> {
    Some(match data_type {
        "string" => "value_text",
        "url" => "value_url",
        "date" => "value_date",
        "boolean" => "value_bool",
        "integer" => "value_int",
        "float" => "value_float",
        "monetary" => "value_monetary",
        "documentlink" => "value_document_ids",
        "select" => "value_select",
        "longtext" => "value_long_text",
        _ => return None,
    })
}

/// Value for the DB column of the type.
pub fn custom_value_expr(data_type: &str, v: &Value) -> Result<SimpleExpr, String> {
    let bad = || format!("Invalid value {v} for custom field type {data_type}");
    if v.is_null() {
        return Ok(match data_type {
            "boolean" => Option::<bool>::None.into(),
            "integer" => Option::<i32>::None.into(),
            "float" => Option::<f64>::None.into(),
            "date" => Option::<NaiveDate>::None.into(),
            "documentlink" => Option::<Value>::None.into(),
            _ => Option::<String>::None.into(),
        });
    }
    Ok(match data_type {
        "boolean" => v.as_bool().ok_or_else(bad)?.into(),
        "integer" => (v.as_i64().ok_or_else(bad)? as i32).into(),
        "float" => v.as_f64().ok_or_else(bad)?.into(),
        "date" => {
            let s = v.as_str().ok_or_else(bad)?;
            NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| bad())?.into()
        }
        "documentlink" => v.clone().into(),
        "select" => match v {
            Value::String(s) => s.clone().into(),
            other => other.to_string().into(),
        },
        _ => match v {
            Value::String(s) => s.clone().into(),
            other => other.to_string().into(),
        },
    })
}

/// Candidates for `match_*`: `Model.objects.filter(id__in=permitted_object_ids(user, Model, perm))`
/// or `Model.objects.all()` without owner, in model order (`name`).
pub async fn matchables(
    conn: &mut PgConnection,
    model: &pl_authz::ObjectModel,
    class_name: &str,
    owner: Option<&pl_authz::User>,
) -> Result<Vec<pl_matching::Matchable>, sqlx::Error> {
    let mut q = Query::select();
    q.columns([a("id"), a("name"), a("match"), a("matching_algorithm"), a("is_insensitive")])
        .from(a(model.table))
        .order_by(a("name"), Order::Asc)
        .order_by(a("id"), Order::Asc);
    if let Some(u) = owner {
        let perm = format!("view_{}", model.model_name);
        let sub = pl_authz::objects::permitted_object_ids(Some(u), model, &perm, &pl_authz::objects::PermOpts::default());
        q.and_where(Expr::col(a("id")).in_subquery(sub));
    }
    let rows = fetch_all(conn, &q).await?;
    rows.iter()
        .map(|r| {
            Ok(pl_matching::Matchable {
                id: i64::from(r.try_get::<i32, _>(0)?),
                class_name: class_name.to_string(),
                name: r.try_get(1)?,
                match_text: r.try_get(2)?,
                matching_algorithm: i64::from(r.try_get::<i16, _>(3)?),
                is_insensitive: r.try_get(4)?,
            })
        })
        .collect()
}

/// `restrict_queryset_to_visible(Tag.objects.filter(is_inbox_tag=True), owner, "view_tag")`.
pub async fn inbox_tags(conn: &mut PgConnection, owner: Option<&pl_authz::User>) -> Result<Vec<i64>, sqlx::Error> {
    let mut q = Query::select();
    q.column((a("t"), a("id")))
        .from_as(a("documents_tag"), a("t"))
        .and_where(col("t", "is_inbox_tag").eq(true))
        .order_by((a("t"), a("name")), Order::Asc);
    q.cond_where(pl_authz::objects::restrict_to_visible(owner, &pl_authz::models::TAG, "view_tag", "t"));
    ids(conn, &q).await
}

/// ContentType-ID (`django_content_type`).
pub async fn content_type_id(conn: &mut PgConnection, app_label: &str, model: &str) -> Result<Option<i32>, sqlx::Error> {
    if let Some(v) = crate::lookup::ctype(app_label, model) {
        return Ok(Some(v));
    }
    let q = Query::select()
        .column(a("id"))
        .from(a("django_content_type"))
        .and_where(Expr::col(a("app_label")).eq(app_label))
        .and_where(Expr::col(a("model")).eq(model))
        .to_owned();
    Ok(match fetch_optional(conn, &q).await? {
        Some(r) => {
            let v: i32 = r.try_get(0)?;
            crate::lookup::set_ctype(app_label, model, v);
            Some(v)
        }
        None => None,
    })
}

/// `get_or_create(name__iexact=part, defaults={"name": part})` for tags (subfolders as tags).
pub async fn get_or_create_tag(conn: &mut PgConnection, name: &str) -> Result<i64, sqlx::Error> {
    let q = Query::select()
        .column(a("id"))
        .from(a("documents_tag"))
        .and_where(Expr::expr(Func::upper(Expr::col(a("name")))).eq(Func::upper(Expr::val(name))))
        .order_by(a("id"), Order::Asc)
        .limit(1)
        .to_owned();
    if let Some(r) = fetch_optional(conn, &q).await? {
        return Ok(i64::from(r.try_get::<i32, _>(0)?));
    }
    // New root tag with model defaults (Tag: color #a6cee3, matching_algorithm ANY=1,
    // is_insensitive True, is_inbox_tag False; no parent).
    let ins = Query::insert()
        .into_table(a("documents_tag"))
        .columns([a("name"), a("match"), a("matching_algorithm"), a("is_insensitive"), a("color"), a("is_inbox_tag")])
        .values_panic([name.into(), "".into(), 1i16.into(), true.into(), "#a6cee3".into(), false.into()])
        .returning_col(a("id"))
        .to_owned();
    let (sql, v) = ins.build_sqlx(PostgresQueryBuilder);
    let r = sqlx::query_with(&sql, v).fetch_one(&mut *conn).await?;
    Ok(i64::from(r.try_get::<i32, _>(0)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micros() {
        let t = DateTime::parse_from_rfc3339("2024-01-01T00:00:00.123456789Z").unwrap().with_timezone(&Utc);
        assert_eq!(trunc_micros(t).timestamp_subsec_nanos(), 123_456_000);
    }

    #[test]
    fn value_fields() {
        assert_eq!(value_field_name("monetary"), Some("value_monetary"));
        assert_eq!(value_field_name("nope"), None);
        assert!(custom_value_expr("integer", &Value::from("x")).is_err());
        assert!(custom_value_expr("date", &Value::from("2024-02-03")).is_ok());
    }
}
