//! `prune_audit_logs`: delete audit log entries for objects that no longer exist.
//!
//! upstream: src/documents/management/commands/prune_audit_logs.py (v3.4.2):
//! `LogEntry.objects.all()` (order `-timestamp`), per entry `content_type.model_class()`,
//! `global_objects` if present, otherwise `objects`; `object_id` truthy and object missing → delete.

use sqlx::Row;

use crate::args;
use crate::ctx::Ctx;
use crate::export::descriptor;
use crate::CliResult;

pub async fn command(argv: Vec<String>) -> CliResult {
    args::parse(&argv, &[args::PROGRESS], 0)?;
    let c = Ctx::connect().await?;
    for (name, id) in prune(&c).await? {
        println!("Deleted audit log entry for {name} #{id}");
    }
    Ok(())
}

/// Returns: (model class, object_id) per deleted entry, in iteration order.
pub async fn prune(c: &Ctx) -> CliResult<Vec<(String, i64)>> {
    let d = descriptor::get();
    let mut tx = c.pool.begin().await?;
    let rows = sqlx::query(
        "SELECT l.id, l.object_id, ct.app_label, ct.model FROM auditlog_logentry l
         JOIN django_content_type ct ON ct.id = l.content_type_id ORDER BY l.timestamp DESC",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut deleted = Vec::new();
    for r in rows {
        let id: i32 = r.try_get(0)?;
        let object_id: Option<i64> = r.try_get(1)?;
        let label = format!("{}.{}", r.try_get::<String, _>(2)?, r.try_get::<String, _>(3)?);
        let Some(model) = d.models.get(&label) else {
            // upstream: model_class() is None → AttributeError; skipped here
            tracing::warn!(target: "paperless.management", "audit log entry {id}: model {label} not installed, skipped");
            continue;
        };
        let Some(oid) = object_id.filter(|v| *v != 0) else { continue };
        let exists: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS (SELECT 1 FROM \"{}\" WHERE \"{}\"::text = $1)",
            model.table, model.pk_column
        ))
        .bind(oid.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            sqlx::query("DELETE FROM auditlog_logentry WHERE id = $1").bind(id).execute(&mut *tx).await?;
            let name = if model.object_name.is_empty() { label.clone() } else { model.object_name.clone() };
            deleted.push((name, oid));
        }
    }
    tx.commit().await?;
    Ok(deleted)
}
