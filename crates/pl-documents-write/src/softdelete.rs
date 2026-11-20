// upstream: django_softdelete 1.0.23 (SoftDeleteModel.delete/restore, __delete_related_object,
//           __restore_related_one_to_many, SoftDeleteQuerySet.delete), src/documents/models.py@v3.4.2
//           (Document.delete: versions first with the same transaction_id; order of the
//           reverse relations = model order: versions, notes, barcodes, share_links,
//           custom_fields, workflow_runs), src/documents/tasks.py@v3.4.2 (empty_trash),
//           django/db/models/deletion.py (Collector)
//
//! Trash logic for documents: soft-delete cascade, restore, permanent deletion.

use sqlx::{PgConnection, Row};
use uuid::Uuid;

use pl_consumer::db::DocRow;

use crate::auditlog::{self, LogCtx, NoteRow};
use crate::cfi;
use crate::docs::{self, now_micros};
use crate::error::ApiError;
use crate::files::{self, MediaCfg};

/// Fields of the preloaded version (`only(id, added, checksum, version_label, root_document_id,
/// version_index, page_count)`) plus the fields read/set in the cascade.
const PREFETCH_LOADED_FIELDS: &[&str] = &[
    "added", "checksum", "version_label", "root_document", "version_index", "page_count", "deleted_at",
    "restored_at", "transaction_id", "title", "created", "correspondent", "filename", "archive_filename",
    "mime_type",
];

const NOTE_COLS: &str = "id, note, created, document_id, user_id, deleted_at, restored_at, transaction_id";

fn note_from_row(r: &sqlx::postgres::PgRow) -> Result<NoteRow, sqlx::Error> {
    Ok(NoteRow {
        id: i64::from(r.try_get::<i32, _>(0)?),
        note: r.try_get(1)?,
        created: r.try_get(2)?,
        document_id: r.try_get::<Option<i32>, _>(3)?.map(i64::from),
        user_id: r.try_get::<Option<i32>, _>(4)?.map(i64::from),
        deleted_at: r.try_get(5)?,
        restored_at: r.try_get(6)?,
        transaction_id: r.try_get(7)?,
    })
}

pub async fn load_note(conn: &mut PgConnection, id: i64) -> Result<Option<NoteRow>, sqlx::Error> {
    let sql = format!("SELECT {NOTE_COLS} FROM documents_note WHERE id = $1");
    let r = sqlx::query(&sql).bind(id as i32).fetch_optional(&mut *conn).await?;
    r.as_ref().map(note_from_row).transpose()
}

async fn notes_where(conn: &mut PgConnection, where_: &str, doc_id: i64, tid: Option<Uuid>) -> Result<Vec<NoteRow>, sqlx::Error> {
    let sql = format!("SELECT {NOTE_COLS} FROM documents_note WHERE {where_} ORDER BY created, id");
    let rows = sqlx::query(&sql).bind(doc_id as i32).bind(tid).fetch_all(&mut *conn).await?;
    rows.iter().map(note_from_row).collect()
}

/// pre_save audit of a note (old via `Note.objects`, without trash).
async fn note_save_audit(conn: &mut PgConnection, ctx: &LogCtx, n: &NoteRow, only: Option<&[&str]>) -> Result<(), sqlx::Error> {
    if !ctx.enabled {
        return Ok(());
    }
    let old = load_note(conn, n.id).await?.filter(|o| o.deleted_at.is_none());
    let ov = old.as_ref().map(auditlog::note_fields);
    let nv = auditlog::note_fields(n);
    let ch = auditlog::diff(ov.as_ref(), Some(&nv), &[], only);
    auditlog::write_diff(conn, ctx, "note", n.id, &n.note, auditlog::UPDATE, ch).await
}

async fn note_set_delete_fields(conn: &mut PgConnection, n: &NoteRow) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE documents_note SET deleted_at = $2, restored_at = $3, transaction_id = $4 WHERE id = $1")
        .bind(n.id as i32)
        .bind(n.deleted_at)
        .bind(n.restored_at)
        .bind(n.transaction_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// `note.delete()` (soft delete; `then_full_save` = cascade via the document).
pub async fn soft_delete_note(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    n: &NoteRow,
    tid: Uuid,
    then_full_save: bool,
) -> Result<(), sqlx::Error> {
    let mut m = n.clone();
    m.deleted_at = Some(now_micros());
    m.restored_at = None;
    m.transaction_id = Some(tid);
    note_save_audit(conn, ctx, &m, Some(&["deleted_at", "restored_at", "transaction_id"])).await?;
    note_set_delete_fields(conn, &m).await?;
    if then_full_save {
        note_save_audit(conn, ctx, &m, None).await?;
    }
    Ok(())
}

/// Soft delete / restore of the models without auditlog (ShareLink, WorkflowRun).
async fn plain_soft(conn: &mut PgConnection, table: &str, doc_id: i64, tid: Uuid) -> Result<(), sqlx::Error> {
    let sql = format!(
        "UPDATE {table} SET deleted_at = $2, restored_at = NULL, transaction_id = $3 WHERE document_id = $1 AND deleted_at IS NULL"
    );
    sqlx::query(&sql).bind(doc_id as i32).bind(now_micros()).bind(tid).execute(&mut *conn).await?;
    Ok(())
}

async fn plain_restore(conn: &mut PgConnection, table: &str, doc_id: i64, tid: Option<Uuid>) -> Result<(), sqlx::Error> {
    let sql = format!(
        "UPDATE {table} SET deleted_at = NULL, restored_at = $2, transaction_id = NULL \
         WHERE document_id = $1 AND deleted_at IS NOT NULL AND transaction_id IS NOT DISTINCT FROM $3"
    );
    sqlx::query(&sql).bind(doc_id as i32).bind(now_micros()).bind(tid).execute(&mut *conn).await?;
    Ok(())
}

/// `document.delete(transaction_id=…)` (soft-delete cascade, `Document.delete` + `SoftDeleteModel.delete`).
pub async fn soft_delete_document(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    doc_id: i64,
    tid: Option<Uuid>,
) -> Result<(), ApiError> {
    Box::pin(soft_delete_inner(conn, ctx, media, doc_id, tid, false)).await
}

/// Like [`soft_delete_document`] for the instance from `DocumentViewSet.get_object()`: there
/// `versions` are preloaded (prefetch), so the cascade treats the already deleted versions
/// once more as related objects (repeated soft delete + `save()`).
pub async fn soft_delete_document_prefetched(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    doc_id: i64,
) -> Result<(), ApiError> {
    Box::pin(soft_delete_inner(conn, ctx, media, doc_id, None, true)).await
}

async fn soft_delete_inner(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    doc_id: i64,
    tid: Option<Uuid>,
    prefetched: bool,
) -> Result<(), ApiError> {
    let tid = tid.unwrap_or_else(Uuid::new_v4);
    let Some(doc) = docs::load_global(conn, doc_id).await? else { return Ok(()) };
    let mut prefetched_versions: Vec<i32> = Vec::new();
    if doc.root_document_id.is_none() {
        // Document.objects.filter(root_document=self).delete(transaction_id=…) (order -created)
        let versions: Vec<i32> = sqlx::query_scalar(
            "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL ORDER BY created DESC, id ASC",
        )
        .bind(doc_id as i32)
        .fetch_all(&mut *conn)
        .await?;
        if prefetched {
            // Prefetch query (order -created; on a tie in the reference ascending id)
            prefetched_versions = sqlx::query_scalar(
                "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL ORDER BY created DESC, id ASC",
            )
            .bind(doc_id as i32)
            .fetch_all(&mut *conn)
            .await?;
        }
        for v in &versions {
            Box::pin(soft_delete_inner(conn, ctx, media, i64::from(*v), Some(tid), false)).await?;
        }
    }
    // Reverse relation `versions` (only preloaded instances; otherwise already deleted)
    for v in prefetched_versions {
        Box::pin(soft_delete_inner(conn, ctx, media, i64::from(v), Some(tid), false)).await?;
        if let Some(mut vd) = docs::load_global(conn, i64::from(v)).await? {
            // Instance from `.only(...)` of the prefetch: save() saves only loaded fields (plus those
            // read on deletion: str(), file name)
            docs::save(conn, ctx, Some(media), &mut vd, Some(PREFETCH_LOADED_FIELDS)).await?;
        }
    }
    let now = now_micros();
    // notes
    for n in notes_where(conn, "document_id = $1 AND deleted_at IS NULL AND $2::uuid IS NULL", doc_id, None).await? {
        soft_delete_note(conn, ctx, &n, tid, true).await?;
    }
    // barcodes: no SoftDeleteModel → hard deletion (no signals)
    sqlx::query("DELETE FROM documents_documentbarcode WHERE document_id = $1").bind(doc_id as i32).execute(&mut *conn).await?;
    plain_soft(conn, "documents_sharelink", doc_id, tid).await?;
    for c in cfi::live_for_document(conn, doc_id).await? {
        cfi::soft_delete(conn, ctx, media, &c, tid, true).await?;
    }
    plain_soft(conn, "documents_workflowrun", doc_id, tid).await?;
    // self.save(update_fields=['deleted_at', 'restored_at', 'transaction_id'])
    let Some(mut d) = docs::load_global(conn, doc_id).await? else { return Ok(()) };
    d.deleted_at = Some(now);
    d.restored_at = None;
    d.transaction_id = Some(tid);
    docs::save(conn, ctx, Some(media), &mut d, Some(&["deleted_at", "restored_at", "transaction_id"])).await?;
    Ok(())
}

/// `doc.restore(strict=False)` (incl. cascade via `transaction_id`); `then_full_save` = as a
/// related object (`related_object.save()` afterwards).
pub async fn restore_document(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    doc_id: i64,
    tid_override: Option<Option<Uuid>>,
    then_full_save: bool,
) -> Result<(), ApiError> {
    Box::pin(restore_inner(conn, ctx, media, doc_id, tid_override, then_full_save)).await
}

async fn restore_inner(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    doc_id: i64,
    tid_override: Option<Option<Uuid>>,
    then_full_save: bool,
) -> Result<(), ApiError> {
    let Some(doc) = docs::load_global(conn, doc_id).await? else { return Ok(()) };
    let now = now_micros();
    let tid = tid_override.unwrap_or(doc.transaction_id);
    // versions (Document.deleted_objects, order -created)
    let versions: Vec<i32> = sqlx::query_scalar(
        "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NOT NULL \
         AND transaction_id IS NOT DISTINCT FROM $2 ORDER BY created DESC, id ASC",
    )
    .bind(doc_id as i32)
    .bind(tid)
    .fetch_all(&mut *conn)
    .await?;
    for v in versions {
        Box::pin(restore_inner(conn, ctx, media, i64::from(v), Some(tid), true)).await?;
    }
    // notes
    for n in notes_where(
        conn,
        "document_id = $1 AND deleted_at IS NOT NULL AND transaction_id IS NOT DISTINCT FROM $2",
        doc_id,
        tid,
    )
    .await?
    {
        let mut m = n.clone();
        m.deleted_at = None;
        m.restored_at = Some(now_micros());
        m.transaction_id = None;
        note_save_audit(conn, ctx, &m, Some(&["deleted_at", "restored_at", "transaction_id"])).await?;
        note_set_delete_fields(conn, &m).await?;
        note_save_audit(conn, ctx, &m, None).await?;
    }
    plain_restore(conn, "documents_sharelink", doc_id, tid).await?;
    let deleted_cfis: Vec<_> = cfi::deleted_for_document(conn, doc_id)
        .await?
        .into_iter()
        .filter(|c| c.transaction_id == tid)
        .collect();
    for c in deleted_cfis {
        cfi::restore(conn, ctx, media, &c).await?;
    }
    plain_restore(conn, "documents_workflowrun", doc_id, tid).await?;
    let Some(mut d) = docs::load_global(conn, doc_id).await? else { return Ok(()) };
    d.deleted_at = None;
    d.restored_at = Some(now);
    d.transaction_id = None;
    docs::save(conn, ctx, Some(media), &mut d, Some(&["deleted_at", "restored_at", "transaction_id"])).await?;
    if then_full_save {
        docs::save(conn, ctx, Some(media), &mut d, None).await?;
    }
    Ok(())
}

/// `empty_trash(doc_ids)`: hard deletion (collector) incl. cascade, clean up files,
/// remove LogEntries of the deleted documents.
pub async fn empty_trash(conn: &mut PgConnection, ctx: &LogCtx, media: &MediaCfg, doc_ids: &[i64]) -> Result<(), ApiError> {
    let ids32: Vec<i32> = doc_ids.iter().map(|i| *i as i32).collect();
    // Document.deleted_objects.filter(id__in=doc_ids)
    let selected: Vec<i32> =
        sqlx::query_scalar("SELECT id FROM documents_document WHERE id = ANY($1) AND deleted_at IS NOT NULL ORDER BY id")
            .bind(&ids32)
            .fetch_all(&mut *conn)
            .await?;
    if selected.is_empty() {
        return Ok(());
    }
    // Cascade via root_document (all versions, also deleted), recursive
    let mut all: Vec<i32> = selected.clone();
    let mut frontier = selected.clone();
    while !frontier.is_empty() {
        let next: Vec<i32> = sqlx::query_scalar("SELECT id FROM documents_document WHERE root_document_id = ANY($1)")
            .bind(&frontier)
            .fetch_all(&mut *conn)
            .await?;
        frontier = next.into_iter().filter(|i| !all.contains(i)).collect();
        all.extend(frontier.iter().copied());
    }
    all.sort_unstable();
    let mut docs_rows: Vec<(DocRow, String)> = Vec::new();
    for id in &all {
        if let Some(d) = docs::load_global(conn, i64::from(*id)).await? {
            let r = docs::repr(conn, &d).await?;
            docs_rows.push((d, r));
        }
    }
    // collect dependent rows (for the post_delete audit)
    let notes: Vec<NoteRow> = {
        let sql = format!("SELECT {NOTE_COLS} FROM documents_note WHERE document_id = ANY($1) ORDER BY id");
        let rows = sqlx::query(&sql).bind(&all).fetch_all(&mut *conn).await?;
        rows.iter().map(note_from_row).collect::<Result<_, _>>()?
    };
    let mut cfis = Vec::new();
    for id in &all {
        let rows = sqlx::query_scalar::<_, i32>("SELECT id FROM documents_customfieldinstance WHERE document_id = $1")
            .bind(*id)
            .fetch_all(&mut *conn)
            .await?;
        for r in rows {
            if let Some(c) = cfi::load(conn, i64::from(r)).await? {
                cfis.push(c);
            }
        }
    }
    cfis.sort_by_key(|c| c.id);
    let mut cfi_reprs = Vec::new();
    for c in &cfis {
        let f = cfi::field_def(conn, c.field_id).await?;
        cfi_reprs.push(match f {
            Some(f) => auditlog::cfi_repr(&f.name, &f.data_type, f.extra_data.as_ref(), c),
            None => String::new(),
        });
    }
    // Delete (dependents first)
    for t in ["documents_note", "documents_documentbarcode", "documents_sharelink", "documents_customfieldinstance", "documents_workflowrun"] {
        let sql = format!("DELETE FROM {t} WHERE document_id = ANY($1)");
        sqlx::query(&sql).bind(&all).execute(&mut *conn).await?;
    }
    sqlx::query("DELETE FROM documents_document_tags WHERE document_id = ANY($1)").bind(&all).execute(&mut *conn).await?;
    sqlx::query("DELETE FROM documents_sharelinkbundle_documents WHERE document_id = ANY($1)")
        .bind(&all)
        .execute(&mut *conn)
        .await?;
    // Versions before roots
    sqlx::query("DELETE FROM documents_document WHERE id = ANY($1) AND root_document_id IS NOT NULL")
        .bind(&all)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM documents_document WHERE id = ANY($1)").bind(&all).execute(&mut *conn).await?;
    // post_delete (collector: per model descending by pk)
    if ctx.enabled {
        for n in notes.iter().rev() {
            let ch = auditlog::diff(Some(&auditlog::note_fields(n)), None, &[], None);
            auditlog::write_diff(conn, ctx, "note", n.id, &n.note, auditlog::DELETE, ch).await?;
        }
        for (c, r) in cfis.iter().zip(cfi_reprs.iter()).rev() {
            let ch = auditlog::diff(Some(&auditlog::cfi_fields(c)), None, &[], None);
            auditlog::write_diff(conn, ctx, "customfieldinstance", c.id, r, auditlog::DELETE, ch).await?;
        }
    }
    for (d, r) in docs_rows.iter().rev() {
        if ctx.enabled {
            let ch = auditlog::diff(Some(&auditlog::document_fields(d)), None, auditlog::DOCUMENT_REVERSE, None);
            auditlog::write_diff(conn, ctx, "document", d.id, r, auditlog::DELETE, ch).await?;
        }
        files::cleanup_document_deletion(media, d).await;
    }
    if ctx.enabled {
        // LogEntry.objects.filter(content_type=Document, object_id__in=deleted_document_ids).delete()
        let sel64: Vec<i64> = selected.iter().map(|i| i64::from(*i)).collect();
        sqlx::query(
            "DELETE FROM auditlog_logentry WHERE content_type_id = \
             (SELECT id FROM django_content_type WHERE app_label = 'documents' AND model = 'document') \
             AND object_id = ANY($1)",
        )
        .bind(&sel64)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
