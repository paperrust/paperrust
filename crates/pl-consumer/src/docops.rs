// Document operations with the side effects of the Django models (auditlog pre_save/post_save,
// m2m_changed, auto_now).
//
// upstream: src/documents/models.py@v3.4.2 (Document.save via Model.save: modified auto_now,
//           Document.add_nested_tags, Document.__str__), django.db.models.fields.related_descriptors
//           (ManyRelatedManager._add_items/_remove_items/clear → m2m_changed), auditlog receivers.

use sea_query::{Expr, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder as _;
use sqlx::PgConnection;

use crate::audit::{AuditLog, CreatedAttr, document_repr};
use crate::db::{self, DocRow, a, now_micros};

/// A Python `Document` instance: fields + state of the `created` attribute.
#[derive(Debug, Clone)]
pub struct DocInstance {
    pub row: DocRow,
    pub created_attr: CreatedAttr,
}

impl DocInstance {
    /// Fresh from the DB (`Document.objects.get(pk=...)`).
    pub fn from_db(row: DocRow) -> Self {
        let created_attr = CreatedAttr::Date(row.created);
        Self { row, created_attr }
    }

    /// `str(document)` (needs the correspondent name).
    pub async fn repr(&self, conn: &mut PgConnection) -> Result<String, sqlx::Error> {
        let c = match self.row.correspondent_id {
            Some(id) => db::object_name(conn, "documents_correspondent", id).await?,
            None => None,
        };
        Ok(document_repr(&self.created_attr, c.as_deref(), &self.row.title))
    }
}

/// `document.save(update_fields=...)`: audit (difference DB → instance), `modified = now()`
/// (auto_now; with `update_fields` only if `modified` is in it), DB update.
pub async fn save(
    conn: &mut PgConnection,
    audit: &AuditLog,
    doc: &mut DocInstance,
    update_fields: Option<&[&str]>,
) -> Result<(), sqlx::Error> {
    let Some(old) = db::load_document(conn, doc.row.id).await? else {
        return Err(sqlx::Error::RowNotFound);
    };
    let touch_modified = update_fields.is_none_or(|f| f.contains(&"modified"));
    if touch_modified {
        doc.row.modified = now_micros();
    }
    let corr = match doc.row.correspondent_id {
        Some(id) => db::object_name(conn, "documents_correspondent", id).await?,
        None => None,
    };
    audit.document_updated(conn, &old, &doc.row, &doc.created_attr, update_fields, corr.as_deref()).await?;
    const ALL: &[&str] = &[
        "title", "content", "archive_checksum", "page_count", "created", "modified", "filename", "archive_filename",
        "archive_serial_number", "correspondent", "document_type", "storage_path", "owner", "version_label",
        "deleted_at", "transaction_id",
    ];
    let fields: Vec<&str> = match update_fields {
        Some(f) => f.to_vec(),
        None => ALL.to_vec(),
    };
    db::update_document_fields(conn, &doc.row, &fields).await
}

/// `document.add_nested_tags(tags)`: tags + ancestors; m2m audit for the newly added ones.
pub async fn add_nested_tags(
    conn: &mut PgConnection,
    audit: &AuditLog,
    doc: &DocInstance,
    tag_ids: &[i64],
) -> Result<Vec<i64>, sqlx::Error> {
    let all = db::with_ancestors(conn, tag_ids).await?;
    if all.is_empty() {
        return Ok(Vec::new());
    }
    let added = db::add_tags(conn, doc.row.id, &all).await?;
    if !added.is_empty() {
        let repr = doc.repr(conn).await?;
        let names: Vec<String> = added.iter().map(|(_, n)| n.clone()).collect();
        audit.tags_added(conn, doc.row.id, &repr, &names).await?;
    }
    Ok(added.into_iter().map(|(i, _)| i).collect())
}

/// `document.tags.remove(*ids)` (m2m `post_remove` with all requested IDs).
pub async fn remove_tags(conn: &mut PgConnection, audit: &AuditLog, doc: &DocInstance, tag_ids: &[i64]) -> Result<(), sqlx::Error> {
    if tag_ids.is_empty() {
        return Ok(());
    }
    let d = Query::delete()
        .from_table(a("documents_document_tags"))
        .and_where(Expr::col(a("document_id")).eq(doc.row.id))
        .and_where(Expr::col(a("tag_id")).is_in(tag_ids.iter().map(|i| *i as i32)))
        .to_owned();
    crate::lookup::forget_tags(doc.row.id);
    let (sql, v) = d.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, v).execute(&mut *conn).await?;
    let names = tag_names(conn, Some(tag_ids)).await?;
    m2m_delete(conn, audit, doc, &names).await
}

/// `document.tags.clear()` (auditlog logs `Tag.objects.all()` in the process).
pub async fn clear_tags(conn: &mut PgConnection, audit: &AuditLog, doc: &DocInstance) -> Result<(), sqlx::Error> {
    let d = Query::delete()
        .from_table(a("documents_document_tags"))
        .and_where(Expr::col(a("document_id")).eq(doc.row.id))
        .to_owned();
    crate::lookup::forget_tags(doc.row.id);
    let (sql, v) = d.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, v).execute(&mut *conn).await?;
    let names = tag_names(conn, None).await?;
    m2m_delete(conn, audit, doc, &names).await
}

async fn m2m_delete(conn: &mut PgConnection, audit: &AuditLog, doc: &DocInstance, names: &[String]) -> Result<(), sqlx::Error> {
    if names.is_empty() {
        return Ok(());
    }
    let repr = doc.repr(conn).await?;
    let mut ch = serde_json::Map::new();
    ch.insert("tags".into(), serde_json::json!({"type": "m2m", "operation": "delete", "objects": names}));
    audit.write(conn, "document", doc.row.id, &repr, crate::audit::ACTION_UPDATE, ch, None).await
}

async fn tag_names(conn: &mut PgConnection, ids: Option<&[i64]>) -> Result<Vec<String>, sqlx::Error> {
    let mut q = Query::select();
    q.column(a("name")).from(a("documents_tag")).order_by(a("name"), sea_query::Order::Asc);
    if let Some(ids) = ids {
        q.and_where(Expr::col(a("id")).is_in(ids.iter().map(|i| *i as i32)));
    }
    let rows = db::fetch_all(conn, &q).await?;
    use sqlx::Row as _;
    rows.iter().map(|r| r.try_get::<String, _>(0)).collect()
}
