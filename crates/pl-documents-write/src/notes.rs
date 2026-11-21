// upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.notes, POST/DELETE branch),
//           src/documents/permissions.py@v3.4.2 (PaperlessNotePermissions, has_perms_owner_aware),
//           django-auditlog 3.4.1 (LogEntryManager.log_create, log_create receiver for Note)
//
//! POST/DELETE `/api/documents/{id}/notes/`.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use pl_authz::AuthUser;
use pl_authz::models::DOCUMENT;
use pl_authz::perms::{NOTE_PERMISSIONS, ObjRef, has_perms_owner_aware};
use pl_core::{AppState, RequestContext};
use pl_documents::{SerializeAction, SerializeOpts};

use crate::auditlog::{self, NoteRow};
use crate::docs::{self, now_micros};
use crate::error::{ApiError, ApiResult};
use crate::files::MediaCfg;
use crate::softdelete;
use crate::update::{log_ctx, parse_pk};
use crate::views::Peer;

fn forbidden(msg: &str) -> ApiError {
    ApiError::Core(pl_core::ApiError::django_text(StatusCode::FORBIDDEN, msg))
}

/// `Document.objects…get(pk=pk)` + `has_perms_owner_aware(user, perm, doc)` (via the root).
async fn doc_and_perm(state: &AppState, auth: &AuthUser, pk: &str, perm: &str) -> ApiResult<Option<(i64, bool)>> {
    let Some(id) = parse_pk(pk) else { return Err(ApiError::Core(pl_core::ApiError::not_found())) };
    let mut conn = state.db().acquire().await?;
    let Some(d) = docs::load_live(&mut conn, id).await? else { return Ok(None) };
    let (auth_id, owner) = match d.root_document_id {
        Some(r) => {
            let o: Option<Option<i32>> = sqlx::query_scalar("SELECT owner_id FROM documents_document WHERE id = $1")
                .bind(r as i32)
                .fetch_optional(&mut *conn)
                .await?;
            (r, o.flatten().map(i64::from))
        }
        None => (d.id, d.owner_id),
    };
    // FINDINGS B2: return the connection before the permission check fetches its own from the pool
    drop(conn);
    let ok = has_perms_owner_aware(state.db(), Some(auth.user()), perm, &DOCUMENT, ObjRef { id: auth_id, owner_id: owner })
        .await
        .map_err(|e| ApiError::Core(e.into()))?;
    Ok(Some((d.id, ok)))
}

async fn notes_json(state: &AppState, auth: &AuthUser, doc_id: i64) -> ApiResult<Value> {
    let opts = SerializeOpts {
        fields: Some(vec!["notes".into()]),
        truncate_content: false,
        full_perms: false,
        action: SerializeAction::Other,
        viewset_queryset: true,
    };
    let mut v = pl_documents::serialize_documents(state, auth.user(), &[doc_id], &opts).await?;
    Ok(v.pop().and_then(|o| o.get("notes").cloned()).unwrap_or(Value::Array(vec![])))
}

fn ok(ctx: &RequestContext, v: Value) -> Response {
    pl_core::Drf::ok(v).indent(ctx.negotiation.indent).into_response()
}

/// POST /api/documents/{id}/notes/
pub async fn notes_create_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Peer(peer): Peer,
    Path(pk): Path<String>,
    body: Bytes,
) -> Response {
    let r: ApiResult<Response> = async {
        auth.context().check_perms_by_method(NOTE_PERMISSIONS).map_err(|e| ApiError::Core(e.into()))?;
        let Some((doc_id, can_view)) = doc_and_perm(&state, &auth, &pk, "view_document").await? else {
            return Err(ApiError::Core(pl_core::ApiError::not_found()));
        };
        if !can_view {
            return Err(forbidden("Insufficient permissions to view notes"));
        }
        // DRF's FormParser/MultiPartParser: `request.data["note"]` is the last value of the key.
        let data = match crate::form::parse(&ctx.headers, &body).await? {
            Some(form) => {
                let mut m = Map::new();
                if let Some(n) = form.get("note") {
                    m.insert("note".into(), Value::String(n.to_owned()));
                }
                Value::Object(m)
            }
            None => crate::views::parse_json_body(&ctx.headers, &body)?,
        };
        let result: ApiResult<Value> = async {
            let Some((_, can_change)) = doc_and_perm(&state, &auth, &pk, "change_document").await? else {
                return Err(ApiError::Uncaught("gone".into()));
            };
            if !can_change {
                return Err(forbidden("Insufficient permissions to create notes"));
            }
            // request.data["note"] (KeyError → except → {"error": …})
            let note_v = match &data {
                Value::Object(m) => m.get("note").cloned(),
                _ => None,
            };
            let Some(note_v) = note_v else { return Err(ApiError::Uncaught("KeyError: 'note'".into())) };
            let note = match &note_v {
                Value::String(s) => s.clone(),
                Value::Null => return Err(ApiError::Uncaught("null note".into())),
                other => crate::drf::py_str(other),
            };
            let lctx = log_ctx(&state, &ctx, &auth, peer);
            let media = MediaCfg::from_config(&state.live_config());
            let mut conn = state.db().acquire().await?;
            let created = now_micros();
            let id: i32 =
                sqlx::query_scalar("INSERT INTO documents_note (note, created, document_id, user_id) VALUES ($1, $2, $3, $4) RETURNING id")
                    .bind(&note)
                    .bind(created)
                    .bind(doc_id as i32)
                    .bind(auth.user().id as i32)
                    .fetch_one(&mut *conn)
                    .await?;
            if lctx.enabled {
                let n = NoteRow {
                    id: i64::from(id),
                    note: note.clone(),
                    created,
                    document_id: Some(doc_id),
                    user_id: Some(auth.user().id),
                    deleted_at: None,
                    restored_at: None,
                    transaction_id: None,
                };
                let ch = auditlog::diff(None, Some(&auditlog::note_fields(&n)), &[], None);
                auditlog::write_diff(&mut conn, &lctx, "note", n.id, &n.note, auditlog::CREATE, ch).await?;
                if let Some(d) = docs::load_global(&mut conn, doc_id).await? {
                    let r = docs::repr(&mut conn, &d).await?;
                    let mut ch = Map::new();
                    ch.insert("Note Added".into(), json!(["None", id]));
                    auditlog::write(&mut conn, &lctx, "document", doc_id, &r, auditlog::UPDATE, ch, None).await?;
                }
            }
            if let Some(mut d) = docs::load_global(&mut conn, doc_id).await? {
                docs::save(&mut conn, &lctx, Some(&media), &mut d, Some(&["modified"])).await?;
            }
            drop(conn);
            crate::index::add_or_update(&state, &[doc_id]).await;
            notes_json(&state, &auth, doc_id).await
        }
        .await;
        match result {
            Ok(v) => Ok(ok(&ctx, v)),
            Err(ApiError::Core(e)) if e.status() == StatusCode::FORBIDDEN => Err(ApiError::Core(e)),
            Err(e) => {
                tracing::warn!(target: "paperless.api", "An error occurred saving note: {e:?}");
                Ok(ok(&ctx, json!({"error": "Error saving note, check logs for more detail."})))
            }
        }
    }
    .await;
    match r {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// DELETE /api/documents/{id}/notes/?id=…
pub async fn notes_destroy_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Peer(peer): Peer,
    Path(pk): Path<String>,
) -> Response {
    let r: ApiResult<Response> = async {
        auth.context().check_perms_by_method(NOTE_PERMISSIONS).map_err(|e| ApiError::Core(e.into()))?;
        let Some((doc_id, can_view)) = doc_and_perm(&state, &auth, &pk, "view_document").await? else {
            return Err(ApiError::Core(pl_core::ApiError::not_found()));
        };
        if !can_view {
            return Err(forbidden("Insufficient permissions to view notes"));
        }
        let Some((_, can_change)) = doc_and_perm(&state, &auth, &pk, "change_document").await? else {
            return Err(ApiError::Core(pl_core::ApiError::not_found()));
        };
        if !can_change {
            return Err(forbidden("Insufficient permissions to delete notes"));
        }
        let note_id = ctx.query.get("id").unwrap_or("");
        if note_id.is_empty() {
            return Err(ApiError::Validation(json!({"id": "This field is required."})));
        }
        let Some(note_id) = crate::drf::py_int_from_str(note_id) else {
            return Err(ApiError::Validation(json!({"id": "A valid integer is required."})));
        };
        let lctx = log_ctx(&state, &ctx, &auth, peer);
        let media = MediaCfg::from_config(&state.live_config());
        let mut conn = state.db().acquire().await?;
        // get_object_or_404(Note, id=…, document=doc) – Note.objects (without trash)
        let note = softdelete::load_note(&mut conn, note_id).await?.filter(|n| n.deleted_at.is_none() && n.document_id == Some(doc_id));
        let Some(note) = note else {
            return Err(ApiError::Core(pl_core::ApiError::not_found_model("Note")));
        };
        if lctx.enabled
            && let Some(d) = docs::load_global(&mut conn, doc_id).await?
        {
            let r = docs::repr(&mut conn, &d).await?;
            let mut ch = Map::new();
            ch.insert("Note Deleted".into(), json!([note.id, "None"]));
            auditlog::write(&mut conn, &lctx, "document", doc_id, &r, auditlog::UPDATE, ch, None).await?;
        }
        softdelete::soft_delete_note(&mut conn, &lctx, &note, uuid::Uuid::new_v4(), false).await?;
        if let Some(mut d) = docs::load_global(&mut conn, doc_id).await? {
            docs::save(&mut conn, &lctx, Some(&media), &mut d, Some(&["modified"])).await?;
        }
        drop(conn);
        crate::index::add_or_update(&state, &[doc_id]).await;
        Ok(ok(&ctx, notes_json(&state, &auth, doc_id).await?))
    }
    .await;
    match r {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}
