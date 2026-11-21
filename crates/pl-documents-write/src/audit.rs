// upstream: src/documents/views.py@v3.4.2 (BulkEditView._snapshot_field, BulkEditView.post),
//           auditlog/models.py 3.4.1 (LogEntryManager.log_create), auditlog/middleware.py, auditlog/context.py
//
//! Manual auditlog entries for bulk_edit.

use std::collections::BTreeMap;

use axum::http::HeaderMap;
use chrono::{NaiveDate, Utc};
use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use sqlx::PgConnection;

use crate::error::ApiError;
use crate::user::{CurrentUser, document_content_type_id};

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// Data from the request that the auditlog middleware attaches to every entry.
#[derive(Clone, Debug, Default)]
pub struct RequestMeta {
    pub remote_addr: Option<String>,
    pub remote_port: Option<i32>,
    pub cid: Option<String>,
}

impl RequestMeta {
    /// `AuditlogMiddleware._get_remote_addr/_get_remote_port` + `set_cid`.
    pub fn from_request(headers: &HeaderMap, peer: Option<std::net::SocketAddr>) -> Self {
        let header = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
        // Review M4/M5: centrally in pl-core – only a valid IP / port 0..=65535, otherwise NULL.
        let (remote_addr, remote_port) = pl_core::client::audit_remote(headers, peer);
        RequestMeta { remote_addr, remote_port, cid: header("x-correlation-id") }
    }
}

/// Which snapshot for a method (`MODIFIED_FIELD_BY_METHOD`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Snapshot {
    Column(&'static str),
    Tags,
    CustomFields,
}

impl Snapshot {
    pub fn field_name(self) -> &'static str {
        match self {
            Snapshot::Column(c) => match c {
                "correspondent_id" => "correspondent",
                "document_type_id" => "document_type",
                "storage_path_id" => "storage_path",
                other => other,
            },
            Snapshot::Tags => "tags",
            Snapshot::CustomFields => "custom_fields",
        }
    }
}

/// `_snapshot_field(doc_ids, field)` → `{doc_id: value}`.
pub async fn snapshot(conn: &mut PgConnection, ids: &[i64], what: Snapshot) -> Result<BTreeMap<i64, Value>, ApiError> {
    let mut out = BTreeMap::new();
    match what {
        Snapshot::Tags => {
            for id in ids {
                out.insert(*id, json!([]));
            }
            let q = Query::select()
                .columns([a("document_id"), a("tag_id")])
                .from(a("documents_document_tags"))
                .and_where(Expr::col(a("document_id")).is_in(ids.iter().copied()))
                .order_by(a("tag_id"), Order::Asc)
                .to_owned();
            let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
            let rows: Vec<(i32, i32)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
            for (doc, tag) in rows {
                if let Some(Value::Array(list)) = out.get_mut(&i64::from(doc)) {
                    list.push(json!(tag));
                }
            }
        }
        Snapshot::CustomFields => {
            for id in ids {
                out.insert(*id, json!([]));
            }
            let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
            let rows: Vec<(i32, i32)> = sqlx::query_as(
                "SELECT document_id, id FROM documents_customfieldinstance WHERE document_id = ANY($1) AND deleted_at IS NULL ORDER BY id",
            )
            .bind(&ids32)
            .fetch_all(&mut *conn)
            .await?;
            for (doc, pk) in rows {
                if let Some(Value::Array(list)) = out.get_mut(&i64::from(doc)) {
                    list.push(json!(pk));
                }
            }
        }
        Snapshot::Column("deleted_at") => {
            let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
            let rows: Vec<i32> = sqlx::query_scalar("SELECT id FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL")
                .bind(&ids32)
                .fetch_all(&mut *conn)
                .await?;
            for id in rows {
                out.insert(i64::from(id), Value::Null);
            }
        }
        Snapshot::Column(col) => {
            // Document.objects → not deleted
            let q = Query::select()
                .column(a("id"))
                .expr(Expr::col(a(col)))
                .from(a("documents_document"))
                .and_where(Expr::col(a("id")).is_in(ids.iter().copied()))
                .and_where(Expr::col(a("deleted_at")).is_null())
                .to_owned();
            let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
            if col == "checksum" {
                let rows: Vec<(i32, Option<String>)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
                for (id, v) in rows {
                    out.insert(i64::from(id), json!(v));
                }
            } else {
                let rows: Vec<(i32, Option<i32>)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
                for (id, v) in rows {
                    out.insert(i64::from(id), json!(v));
                }
            }
        }
    }
    Ok(out)
}

/// `str(document)` = `created.isoformat()` [+ " " + correspondent] [+ " " + title].
pub fn document_repr(created: NaiveDate, correspondent: Option<&str>, title: &str) -> String {
    let mut s = created.format("%Y-%m-%d").to_string();
    if let Some(c) = correspondent {
        s.push(' ');
        s.push_str(c);
    }
    if !title.is_empty() {
        s.push(' ');
        s.push_str(title);
    }
    s
}

/// Write one UPDATE entry per document from `Document.objects.filter(pk__in=ids)`.
#[allow(clippy::too_many_arguments)]
pub async fn log_bulk_edit(
    conn: &mut PgConnection,
    user: &CurrentUser,
    meta: &RequestMeta,
    ids: &[i64],
    field: &str,
    old: &BTreeMap<i64, Value>,
    new: &BTreeMap<i64, Value>,
    method_name: &str,
) -> Result<(), ApiError> {
    let ctype = document_content_type_id(conn).await?;
    let q = Query::select()
        .column((a("d"), a("id")))
        .column((a("d"), a("created")))
        .column((a("c"), a("name")))
        .column((a("d"), a("title")))
        .from_as(a("documents_document"), a("d"))
        .join_as(
            sea_query::JoinType::LeftJoin,
            a("documents_correspondent"),
            a("c"),
            Expr::col((a("c"), a("id"))).equals((a("d"), a("correspondent_id"))),
        )
        .and_where(Expr::col((a("d"), a("id"))).is_in(ids.iter().copied()))
        .and_where(Expr::col((a("d"), a("deleted_at"))).is_null())
        .order_by((a("d"), a("created")), Order::Desc)
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let docs: Vec<(i32, NaiveDate, Option<String>, String)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;

    let now = Utc::now();
    for (id, created, corr, title) in docs {
        let id = i64::from(id);
        let changes = json!({ field: [old.get(&id).cloned().unwrap_or(Value::Null), new.get(&id).cloned().unwrap_or(Value::Null)] });
        let remote_addr: sea_query::SimpleExpr = match &meta.remote_addr {
            Some(addr) => Expr::val(addr.clone()).cast_as(a("inet")),
            None => Expr::val(Option::<String>::None).cast_as(a("inet")),
        };
        let mut ins = Query::insert();
        ins.into_table(a("auditlog_logentry")).columns([
            a("content_type_id"),
            a("object_pk"),
            a("object_id"),
            a("object_repr"),
            a("serialized_data"),
            a("action"),
            a("changes_text"),
            a("changes"),
            a("actor_id"),
            a("cid"),
            a("remote_addr"),
            a("remote_port"),
            a("timestamp"),
            a("additional_data"),
            a("actor_email"),
        ]);
        ins.values([
            ctype.into(),
            id.to_string().into(),
            id.into(),
            document_repr(created, corr.as_deref(), &title).into(),
            Expr::val(Option::<serde_json::Value>::None).into(),
            1i16.into(),
            "".into(),
            changes.into(),
            user.id.into(),
            meta.cid.clone().into(),
            remote_addr,
            meta.remote_port.into(),
            now.into(),
            json!({"reason": format!("Bulk edit: {method_name}")}).into(),
            // actor set explicitly → _set_actor sets no e-mail
            Option::<String>::None.into(),
        ])
        .map_err(|e| ApiError::Uncaught(format!("sea-query: {e}")))?;
        let (sql, values) = ins.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values).execute(&mut *conn).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr() {
        let d = NaiveDate::from_ymd_opt(2024, 3, 5).unwrap();
        assert_eq!(document_repr(d, Some("ACME"), "Rechnung"), "2024-03-05 ACME Rechnung");
        assert_eq!(document_repr(d, None, ""), "2024-03-05");
    }

    #[test]
    fn remote_addr_from_xff() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "10.0.0.1:443, 1.2.3.4".parse().unwrap());
        h.insert("x-forwarded-port", "8443".parse().unwrap());
        let m = RequestMeta::from_request(&h, None);
        assert_eq!(m.remote_addr.as_deref(), Some("10.0.0.1"));
        assert_eq!(m.remote_port, Some(8443));
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "[::1]:80".parse().unwrap());
        assert_eq!(RequestMeta::from_request(&h, None).remote_addr.as_deref(), Some("::1"));
    }
}
