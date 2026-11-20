//! Storage-location hook (PaperRust extension, docs/contract/pl-labels.md §number ranges): if a
//! document gets an ASN that lies in a number range, the hook sets the user field "storage location"
//! (name configurable, created once as a text field) to the storage location of the range.
//!
//! * only with the "multiple number ranges" switch on (`pl_labels::ranges`),
//! * a different value set by hand is never overwritten (`ranges::should_write`),
//! * used by the consumer (ASN from barcode or default) and by the document PATCH (pl-documents-write).
//!
//! Errors do not abort ingestion/saving: the hook runs in its own (nested)
//! transaction, is rolled back on errors and only logged.

use serde_json::Value;
use sqlx::PgConnection;

use crate::audit::AuditLog;
use crate::db;

/// What is to be set: (field id, storage location, existing instance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationPlan {
    pub field_id: i64,
    pub location: String,
    /// Id of an existing (empty or hook-set) instance, otherwise `None` = create new.
    pub instance_id: Option<i64>,
}

/// Plan for `doc_id` with `asn`; `None` = nothing to do. Creates the field if it is missing.
/// `skip_fields`: fields that the caller sets itself in the same request (never overwrite).
pub async fn plan(conn: &mut PgConnection, doc_id: i64, asn: i64, skip_fields: &[i64]) -> Result<Option<LocationPlan>, pl_labels::LabelError> {
    use pl_labels::ranges;
    let Some(t) = ranges::location_target(conn, asn).await? else { return Ok(None) };
    let Some(field_id) = ranges::ensure_location_field(conn, &t.field_name).await? else {
        tracing::warn!(target: "paperrust.asn_location", "custom field {:?} exists with another data type; location not set", t.field_name);
        return Ok(None);
    };
    if skip_fields.contains(&field_id) {
        return Ok(None);
    }
    let cur = ranges::current_value(conn, doc_id, field_id).await?;
    if !ranges::should_write(cur.as_ref().and_then(|c| c.1.as_deref()), &t) {
        return Ok(None);
    }
    Ok(Some(LocationPlan { field_id, location: t.location, instance_id: cur.map(|c| c.0) }))
}

/// Consumer: after `apply_overrides` (document is saved, ASN is fixed).
pub async fn apply_on_consume(conn: &mut PgConnection, audit: &AuditLog, doc_id: i64, asn: i64) {
    let r: Result<(), String> = async {
        let mut tx = sqlx::Connection::begin(&mut *conn).await.map_err(|e| e.to_string())?;
        let res: Result<(), String> = async {
            let Some(p) = plan(&mut tx, doc_id, asn, &[]).await.map_err(|e| e.to_string())? else { return Ok(()) };
            match p.instance_id {
                None => {
                    let Some(f) = db::custom_fields(&mut tx, &[p.field_id]).await.map_err(|e| e.to_string())?.into_iter().next() else {
                        return Ok(());
                    };
                    crate::consumer::create_custom_field_instance(&mut tx, audit, doc_id, &f, &Value::String(p.location.clone()))
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Some(id) => {
                    sqlx::query("UPDATE documents_customfieldinstance SET value_text = $2 WHERE id = $1")
                        .bind(id as i32)
                        .bind(&p.location)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            tracing::info!(target: "paperrust.asn_location", "document {doc_id}: ASN {asn} → location {:?}", p.location);
            Ok(())
        }
        .await;
        match res {
            Ok(()) => tx.commit().await.map_err(|e| e.to_string()),
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }
    .await;
    if let Err(e) = r {
        tracing::warn!(target: "paperrust.asn_location", "document {doc_id}: could not set location for ASN {asn}: {e}");
    }
}
