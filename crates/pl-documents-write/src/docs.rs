// upstream: django/db/models/base.py (Model.save with update_fields: pre_save → auditlog.log_update,
//           auto_now only for saved fields, post_save), src/documents/models.py@v3.4.2
//           (Document.__str__, SoftDeleteModel manager: objects = not deleted, global_objects = all),
//           src/documents/signals/handlers.py@v3.4.2 (post_save receiver update_filename_and_move_files)
//
//! Document instances like the Django models: loading, `str()`, `save(update_fields=…)` with auditlog
//! and file name receiver.

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

use pl_consumer::db::{DocRow, load_document};

use crate::auditlog::{self, LogCtx};
use crate::files::{self, MediaCfg};

pub use pl_consumer::db::now_micros;

/// `Document.global_objects.get(pk=…)` (incl. trash).
pub async fn load_global(conn: &mut PgConnection, id: i64) -> Result<Option<DocRow>, sqlx::Error> {
    load_document(conn, id).await
}

/// `Document.objects.filter(pk=…).first()` (without trash).
pub async fn load_live(conn: &mut PgConnection, id: i64) -> Result<Option<DocRow>, sqlx::Error> {
    Ok(load_document(conn, id).await?.filter(|d| d.deleted_at.is_none()))
}

pub async fn correspondent_name(conn: &mut PgConnection, id: Option<i64>) -> Result<Option<String>, sqlx::Error> {
    let Some(id) = id else { return Ok(None) };
    sqlx::query_scalar("SELECT name FROM documents_correspondent WHERE id = $1").bind(id as i32).fetch_optional(&mut *conn).await
}

/// `str(document)`.
pub async fn repr(conn: &mut PgConnection, d: &DocRow) -> Result<String, sqlx::Error> {
    let c = correspondent_name(conn, d.correspondent_id).await?;
    Ok(auditlog::document_repr(d.created, c.as_deref(), &d.title))
}

/// `UPDATE documents_document SET <fields> WHERE id = …` (field names like Django `update_fields`).
pub async fn update_columns(conn: &mut PgConnection, d: &DocRow, fields: &[&str]) -> Result<(), sqlx::Error> {
    let mut sets: Vec<String> = Vec::new();
    let mut q_parts: Vec<&str> = Vec::new();
    for f in fields {
        let col = match *f {
            "correspondent" => "correspondent_id",
            "document_type" => "document_type_id",
            "storage_path" => "storage_path_id",
            "owner" => "owner_id",
            "root_document" => "root_document_id",
            x => x,
        };
        if !q_parts.contains(&col) {
            q_parts.push(col);
        }
    }
    for (i, c) in q_parts.iter().enumerate() {
        sets.push(format!("\"{c}\" = ${}", i + 2));
    }
    if sets.is_empty() {
        return Ok(());
    }
    let sql = format!("UPDATE documents_document SET {} WHERE id = $1", sets.join(", "));
    let mut q = sqlx::query(&sql).bind(d.id as i32);
    for c in &q_parts {
        q = match *c {
            "title" => q.bind(d.title.clone()),
            "content" => q.bind(d.content.clone()),
            "archive_checksum" => q.bind(d.archive_checksum.clone()),
            "page_count" => q.bind(d.page_count.map(|v| v as i32)),
            "created" => q.bind(d.created),
            "modified" => q.bind(d.modified),
            "filename" => q.bind(d.filename.clone()),
            "archive_filename" => q.bind(d.archive_filename.clone()),
            "archive_serial_number" => q.bind(d.archive_serial_number),
            "correspondent_id" => q.bind(d.correspondent_id.map(|v| v as i32)),
            "document_type_id" => q.bind(d.document_type_id.map(|v| v as i32)),
            "storage_path_id" => q.bind(d.storage_path_id.map(|v| v as i32)),
            "owner_id" => q.bind(d.owner_id.map(|v| v as i32)),
            "root_document_id" => q.bind(d.root_document_id.map(|v| v as i32)),
            "version_label" => q.bind(d.version_label.clone()),
            "version_index" => q.bind(d.version_index.map(|v| v as i32)),
            "deleted_at" => q.bind(d.deleted_at),
            "restored_at" => q.bind(d.restored_at),
            "transaction_id" => q.bind(d.transaction_id),
            other => return Err(sqlx::Error::Protocol(format!("unknown document field {other}"))),
        };
    }
    q.execute(&mut *conn).await?;
    Ok(())
}

/// Concrete, storable fields (`Document._meta.concrete_fields` without `id`, `content_length`).
pub const ALL_FIELDS: &[&str] = &[
    "deleted_at",
    "restored_at",
    "transaction_id",
    "title",
    "content",
    "archive_checksum",
    "page_count",
    "created",
    "modified",
    "filename",
    "archive_filename",
    "archive_serial_number",
    "correspondent",
    "document_type",
    "storage_path",
    "owner",
    "root_document",
    "version_label",
    "version_index",
];

/// `document.save(update_fields=…)`.
///
/// 1. `pre_save` → auditlog `log_update` (old = `Document.objects`, i.e. without trash; if
///    the document is missing there, auditlog compares with "no instance").
/// 2. `auto_now` for `modified` (only if saved).
/// 3. UPDATE.
/// 4. `post_save` → `update_filename_and_move_files` (if `media` is set).
pub async fn save(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: Option<&MediaCfg>,
    doc: &mut DocRow,
    update_fields: Option<&[&str]>,
) -> Result<(), crate::error::ApiError> {
    if ctx.enabled {
        let old = load_live(conn, doc.id).await?;
        let new_vals = auditlog::document_fields(doc);
        let old_vals = old.as_ref().map(auditlog::document_fields);
        let ch = auditlog::diff(old_vals.as_ref(), Some(&new_vals), auditlog::DOCUMENT_REVERSE, update_fields);
        if ch.is_some() {
            let r = repr(conn, doc).await?;
            auditlog::write_diff(conn, ctx, "document", doc.id, &r, auditlog::UPDATE, ch).await?;
        }
    }
    // do not write editable=False fields that never change here (relevant only for the diff)
    let fields: Vec<&str> = update_fields
        .map(|f| f.iter().copied().filter(|x| !matches!(*x, "added" | "checksum" | "mime_type" | "original_filename")).collect())
        .unwrap_or_else(|| ALL_FIELDS.to_vec());
    if fields.contains(&"modified") {
        doc.modified = now_micros();
    }
    update_columns(conn, doc, &fields).await?;
    if let Some(m) = media {
        files::update_filename_and_move_files(conn, m, doc.id).await?;
        // update_filename_and_move_files calls instance.refresh_from_db()
        if let Some(fresh) = load_global(conn, doc.id).await? {
            *doc = fresh;
        }
    }
    Ok(())
}

/// `Document.objects.filter(pk__in=…).update(modified=now)`.
pub async fn touch_modified(conn: &mut PgConnection, ids: &[i64]) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    // Lock in id order (FINDINGS B4: otherwise deadlock with the sorting sync trigger)
    pl_consumer::db::touch_modified_sorted(conn, ids, now_micros()).await?;
    Ok(())
}

/// Tags of a document (ids, ascending).
pub async fn tag_ids(conn: &mut PgConnection, doc_id: i64) -> Result<Vec<i64>, sqlx::Error> {
    let v: Vec<i32> = sqlx::query_scalar("SELECT tag_id FROM documents_document_tags WHERE document_id = $1 ORDER BY tag_id")
        .bind(doc_id as i32)
        .fetch_all(&mut *conn)
        .await?;
    Ok(v.into_iter().map(i64::from).collect())
}

/// Tag names of the ids by `Tag.Meta.ordering = ("name",)`.
pub async fn tag_names(conn: &mut PgConnection, ids: &[i64]) -> Result<Vec<String>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    sqlx::query_scalar("SELECT name FROM documents_tag WHERE id = ANY($1) ORDER BY name, id").bind(&ids32).fetch_all(&mut *conn).await
}

/// All tag names (`post_clear` logs `Tag.objects.all()`).
pub async fn all_tag_names(conn: &mut PgConnection) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT name FROM documents_tag ORDER BY name, id").fetch_all(&mut *conn).await
}

/// Ancestors (root→parent) and descendants of a tag via the parent edge
/// (`get_ancestors_pks()` / `get_descendants_pks()`, upstream #14612); `None` without a tag.
pub async fn tag_tree(conn: &mut PgConnection, tag_id: i64) -> Result<Option<(Vec<i64>, Vec<i64>)>, sqlx::Error> {
    let Some(anc) = pl_consumer::db::tag_ancestors(conn, tag_id).await? else { return Ok(None) };
    let desc = pl_consumer::db::tag_descendants(conn, tag_id).await?;
    Ok(Some((anc, desc)))
}

/// `document.tags.add(*ids)` – adds missing ones; m2m signals (auditlog + file name) only if
/// `ids` is not empty (Django sends `pre_add`/`post_add` even for an empty `pk_set`).
pub async fn tags_add(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: Option<&MediaCfg>,
    doc_id: i64,
    ids: &[i64],
) -> Result<(), crate::error::ApiError> {
    if ids.is_empty() {
        return Ok(());
    }
    let have = tag_ids(conn, doc_id).await?;
    let mut missing: Vec<i64> = ids.iter().copied().filter(|t| !have.contains(t)).collect();
    missing.sort_unstable();
    missing.dedup();
    for t in &missing {
        sqlx::query("INSERT INTO documents_document_tags (document_id, tag_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(doc_id as i32)
            .bind(*t as i32)
            .execute(&mut *conn)
            .await?;
    }
    // pre_add + post_add → update_filename_and_move_files (twice, idempotent)
    if let Some(m) = media {
        files::update_filename_and_move_files(conn, m, doc_id).await?;
    }
    if ctx.enabled && !missing.is_empty() {
        let names = tag_names(conn, &missing).await?;
        if let Some(d) = load_global(conn, doc_id).await? {
            let r = repr(conn, &d).await?;
            auditlog::tags_m2m(conn, ctx, doc_id, &r, "add", &names).await?;
        }
    }
    if let Some(m) = media {
        files::update_filename_and_move_files(conn, m, doc_id).await?;
    }
    Ok(())
}

/// `document.tags.remove(*ids)` – auditlog logs all requested ids (`pk_set`).
pub async fn tags_remove(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: Option<&MediaCfg>,
    doc_id: i64,
    ids: &[i64],
) -> Result<(), crate::error::ApiError> {
    if ids.is_empty() {
        return Ok(());
    }
    if let Some(m) = media {
        files::update_filename_and_move_files(conn, m, doc_id).await?;
    }
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    sqlx::query("DELETE FROM documents_document_tags WHERE document_id = $1 AND tag_id = ANY($2)")
        .bind(doc_id as i32)
        .bind(&ids32)
        .execute(&mut *conn)
        .await?;
    if ctx.enabled {
        let names = tag_names(conn, ids).await?;
        if let Some(d) = load_global(conn, doc_id).await? {
            let r = repr(conn, &d).await?;
            auditlog::tags_m2m(conn, ctx, doc_id, &r, "delete", &names).await?;
        }
    }
    if let Some(m) = media {
        files::update_filename_and_move_files(conn, m, doc_id).await?;
    }
    Ok(())
}

/// `document.tags.clear()` – auditlog logs `Tag.objects.all()`.
pub async fn tags_clear(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: Option<&MediaCfg>,
    doc_id: i64,
) -> Result<(), crate::error::ApiError> {
    if let Some(m) = media {
        files::update_filename_and_move_files(conn, m, doc_id).await?;
    }
    sqlx::query("DELETE FROM documents_document_tags WHERE document_id = $1").bind(doc_id as i32).execute(&mut *conn).await?;
    if ctx.enabled {
        let names = all_tag_names(conn).await?;
        if let Some(d) = load_global(conn, doc_id).await? {
            let r = repr(conn, &d).await?;
            auditlog::tags_m2m(conn, ctx, doc_id, &r, "delete", &names).await?;
        }
    }
    if let Some(m) = media {
        files::update_filename_and_move_files(conn, m, doc_id).await?;
    }
    Ok(())
}

/// `document.tags.set(objs)`: `remove(*old)` + `add(*new)`.
pub async fn tags_set(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: Option<&MediaCfg>,
    doc_id: i64,
    wanted: &[i64],
) -> Result<(), crate::error::ApiError> {
    let have = tag_ids(conn, doc_id).await?;
    let remove: Vec<i64> = have.iter().copied().filter(|t| !wanted.contains(t)).collect();
    let mut add: Vec<i64> = Vec::new();
    for t in wanted {
        if !have.contains(t) && !add.contains(t) {
            add.push(*t);
        }
    }
    tags_remove(conn, ctx, media, doc_id, &remove).await?;
    tags_add(conn, ctx, media, doc_id, &add).await?;
    Ok(())
}

/// Timestamp with microseconds (like Python/PostgreSQL).
pub fn now() -> DateTime<Utc> {
    now_micros()
}
