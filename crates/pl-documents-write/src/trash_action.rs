// upstream: src/documents/views.py@v3.4.2 (TrashView.post), src/documents/serialisers.py@v3.4.2
//           (TrashSerializer), src/documents/permissions.py@v3.4.2 (TrashPermissions,
//           permitted_document_ids), src/documents/tasks.py@v3.4.2 (empty_trash),
//           django_softdelete 1.0.23 (restore)
//
//! POST /api/trash/ (`restore` / `empty`).

use axum::body::Bytes;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};

use pl_authz::AuthUser;
use pl_authz::perms::TRASH_PERMISSIONS;
use pl_core::{AppState, RequestContext};

use crate::drf::{self, NULL, REQUIRED, msg};
use crate::error::{ApiError, ApiResult};
use crate::files::MediaCfg;
use crate::update::log_ctx;
use crate::views::Peer;

/// `TrashSerializer` input from a form body (`Field.get_value` for HTML input): `documents` =
/// `getlist`/`parse_html_list` (strings, converted by the `IntegerField` children), `action` = last
/// value (missing → `empty` → "required").
fn form_data(form: &crate::form::FormData) -> Value {
    let mut m = Map::new();
    if let Some(docs) = form.list_value("documents") {
        m.insert("documents".into(), docs);
    }
    if let Some(a) = form.get("action") {
        m.insert("action".into(), Value::String(a.to_owned()));
    }
    Value::Object(m)
}

/// POST /api/trash/
pub async fn trash_action_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Peer(peer): Peer,
    body: Bytes,
) -> Response {
    match inner(&state, &ctx, &auth, peer, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn inner(state: &AppState, ctx: &RequestContext, auth: &AuthUser, peer: Option<std::net::SocketAddr>, body: &Bytes) -> ApiResult<Response> {
    let user = auth.require_perms_by_method(TRASH_PERMISSIONS).map_err(|e| ApiError::Core(e.into()))?.clone();
    let data = match crate::form::parse(&ctx.headers, body).await? {
        Some(form) => form_data(&form),
        None => crate::views::parse_json_body(&ctx.headers, body)?,
    };
    let Value::Object(data) = data else {
        return Err(ApiError::Validation(json!({"non_field_errors": [crate::views::not_a_dict_message(&data)]})));
    };
    let mut conn = state.db().acquire().await?;
    // TrashSerializer: documents, action
    let mut errors = Map::new();
    let mut doc_ids: Option<Vec<i64>> = None;
    if let Some(v) = data.get("documents") {
        match drf::int_list(v) {
            Err(e) => {
                errors.insert("documents".into(), e);
            }
            Ok(ids) => {
                let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
                let count: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM documents_document WHERE id = ANY($1) AND deleted_at IS NOT NULL")
                        .bind(&ids32)
                        .fetch_one(&mut *conn)
                        .await?;
                if count as usize != ids.len() {
                    errors.insert("documents".into(), msg("Some documents in the list have not yet been deleted."));
                } else {
                    doc_ids = Some(ids);
                }
            }
        }
    }
    let mut action: Option<String> = None;
    match data.get("action") {
        None => {
            errors.insert("action".into(), msg(REQUIRED));
        }
        Some(Value::Null) => {
            errors.insert("action".into(), msg(NULL));
        }
        Some(v) => {
            let s = drf::py_str(v);
            if s == "restore" || s == "empty" {
                action = Some(s);
            } else {
                errors.insert("action".into(), msg(format!("\"{s}\" is not a valid choice.")));
            }
        }
    }
    if !errors.is_empty() {
        return Err(ApiError::Validation(Value::Object(errors)));
    }
    // docs = global_objects.filter(id__in=doc_ids) or filtered trash list
    let a = |s: &str| Alias::new(s);
    let docs: Vec<i64> = match &doc_ids {
        Some(ids) => ids.clone(),
        None => {
            let mut q = crate::trash::trash_select(&user);
            q.order_by((a("documents_document"), a("created")), Order::Desc);
            let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
            let rows: Vec<(i32,)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
            rows.into_iter().map(|(i,)| i64::from(i)).collect()
        }
    };
    // docs.exclude(id__in=permitted_document_ids(user, "delete_document", include_deleted=True)).exists()
    if !docs.is_empty() {
        let permitted = pl_authz::objects::permitted_document_ids(Some(&user), "delete_document", true);
        let q = Query::select()
            .expr(Expr::val(1))
            .from(a("documents_document"))
            .and_where(Expr::col(a("id")).is_in(docs.iter().copied()))
            .and_where(Expr::col(a("id")).not_in_subquery(permitted))
            .limit(1)
            .to_owned();
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        if sqlx::query_with(&sql, values).fetch_optional(&mut *conn).await?.is_some() {
            return Err(ApiError::ForbiddenText("Insufficient permissions"));
        }
    }
    let media = MediaCfg::from_config(&state.live_config());
    let lctx = log_ctx(state, ctx, auth, peer);
    let action = action.unwrap_or_default();
    let mut out_ids = doc_ids.clone();
    if action == "restore" {
        let Some(ids) = doc_ids.as_ref() else {
            return Err(ApiError::Uncaught("TypeError: object of type 'NoneType' has no len()".into()));
        };
        // self.get_queryset().filter(id__in=doc_ids) (without permission filter, order -created)
        let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
        let restored: Vec<i32> = sqlx::query_scalar(
            "SELECT d.id FROM documents_document d WHERE d.deleted_at IS NOT NULL AND d.id = ANY($1) \
             AND (d.root_document_id IS NULL OR d.root_document_id NOT IN (SELECT id FROM documents_document WHERE deleted_at IS NOT NULL)) \
             ORDER BY d.created DESC",
        )
        .bind(&ids32)
        .fetch_all(&mut *conn)
        .await?;
        if restored.len() != ids.len() {
            return Err(ApiError::Validation(json!({"documents": ["Restore the root document instead of one of its versions."]})));
        }
        for id in &restored {
            crate::softdelete::restore_document(&mut conn, &lctx, &media, i64::from(*id), None, false).await?;
        }
        drop(conn);
        if !restored.is_empty() {
            let r: Vec<i64> = restored.iter().map(|i| i64::from(*i)).collect();
            crate::index::add_or_update(state, &r).await;
        }
    } else {
        let ids = doc_ids.clone().unwrap_or_else(|| docs.clone());
        if let Err(e) = crate::softdelete::empty_trash(&mut conn, &lctx, &media, &ids).await {
            tracing::error!(target: "paperless.tasks", "Error while emptying trash: {e:?}");
        }
        out_ids = Some(ids);
    }
    let body = json!({"result": "OK", "doc_ids": out_ids});
    Ok(pl_core::Drf::ok(body).indent(ctx.negotiation.indent).into_response())
}

#[cfg(test)]
mod form_tests {
    use super::*;
    use crate::form::FormData;

    fn fd(pairs: &[(&str, &str)]) -> FormData {
        FormData(pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect())
    }

    #[test]
    fn trash_form_like_drf() {
        assert_eq!(
            form_data(&fd(&[("documents", "1"), ("documents", "2"), ("action", "empty"), ("action", "restore")])),
            json!({"documents": ["1", "2"], "action": "restore"})
        );
        assert_eq!(form_data(&fd(&[("documents[0]", "7")])), json!({"documents": ["7"]}));
        assert_eq!(form_data(&fd(&[])), json!({}));
    }
}
