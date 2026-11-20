// upstream: src/documents/views.py@v3.4.2 (TrashView.queryset, TrashView._TrashPermittedObjectsFilter,
//           TrashView.get → ListModelMixin.list), src/documents/filters.py@v3.4.2
//           (PermittedObjectsFilter, include_granted=False), src/paperless/views.py (StandardPagination)
//
//! Trash list (GET /api/trash/): query + permissions here, serialization via
//! `pl_documents::serialize_documents` (without ViewSet prefetch), pagination via `pl_core::pagination`.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use pl_authz::AuthUser;
use pl_authz::perms::TRASH_PERMISSIONS;
use pl_core::pagination::{self, PageRequest};
use pl_core::{AppState, Drf, RequestContext};
use pl_documents::{SerializeAction, SerializeOpts};
use sea_query::{Alias, Expr, Order, Query, SelectStatement};
use serde_json::Value;

use crate::user::CurrentUser;

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// `Document.deleted_objects.exclude(root_document_id__in=Document.deleted_objects.values("id"))`
/// + `_TrashPermittedObjectsFilter`, column `id`, without sorting.
pub fn trash_select(user: &CurrentUser) -> SelectStatement {
    let deleted_ids = Query::select()
        .column(a("id"))
        .from(a("documents_document"))
        .and_where(Expr::col(a("deleted_at")).is_not_null())
        .to_owned();
    Query::select()
        .column((a("documents_document"), a("id")))
        .from(a("documents_document"))
        .and_where(Expr::col((a("documents_document"), a("deleted_at"))).is_not_null())
        // Django: NOT (root_document_id IN (...) AND root_document_id IS NOT NULL)
        .cond_where(
            sea_query::Cond::any()
                .add(Expr::col((a("documents_document"), a("root_document_id"))).is_null())
                .add(
                    Expr::col((a("documents_document"), a("root_document_id")))
                        .not_in_subquery(deleted_ids),
                ),
        )
        .cond_where(pl_authz::objects::permitted_objects_filter(
            Some(user),
            &pl_authz::models::DOCUMENT,
            "documents_document",
            &pl_authz::objects::FilterOpts {
                include_granted: false,
                perm_codename: None,
                parent_field: Some(pl_authz::models::DOCUMENT_PARENT_FIELD),
            },
        ))
        .to_owned()
}

/// GET /api/trash/ – `TrashView.get` (`ListModelMixin.list` with `DocumentSerializer`).
pub async fn trash_list_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
) -> Result<Response, pl_core::ApiError> {
    // (IsAuthenticated, TrashPermissions): GET → documents.view_document
    let user = auth.require_perms_by_method(TRASH_PERMISSIONS)?;
    let db = state.db();
    // Meta.ordering = ["-created"] (no unique secondary sort, like upstream)
    let mut q = trash_select(user);
    q.order_by((a("documents_document"), a("created")), Order::Desc);
    let req = PageRequest::from_query(&ctx.query);
    let (rows, info) = pagination::fetch_page(db, &q, &req).await?;
    let ids: Vec<i64> = rows
        .iter()
        .map(|r| pl_documents::db::i64_col(r, "id"))
        .collect::<Result<_, _>>()?;
    // PassUserMixin.get_serializer: only `full_perms` (no fields/truncate_content);
    // own queryset without prefetch → own content/page count; no action == "retrieve".
    let opts = SerializeOpts {
        fields: None,
        truncate_content: false,
        full_perms: pl_authz::owned::parse_full_perms(ctx.query.get("full_perms")),
        action: SerializeAction::Other,
        viewset_queryset: false,
    };
    let results = pl_documents::serialize_documents(&state, user, &ids, &opts).await?;
    let all = if ctx.version().includes_all() {
        let all =
            pagination::fetch_all_ids(db, &q, Expr::col((a("documents_document"), a("id"))).into())
                .await?;
        Some(all.into_iter().map(Value::from).collect())
    } else {
        None
    };
    let body = pagination::paginated_body(&ctx, &info, results, all);
    Ok(Drf::ok(body).indent(ctx.negotiation.indent).into_response())
}
