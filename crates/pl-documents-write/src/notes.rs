// upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.notes, POST/DELETE branch),
//           POST validation and `format` from paperless-ngx v3.5.1 (#14626):
//           src/documents/serialisers.py (NoteCreateSerializer), views.py (DocumentViewSet.notes),
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

/// `Note.Format` choices (paperless-ngx 3.5, models.py `Note.Format`).
pub const NOTE_FORMATS: [&str; 2] = ["plain", "markdown"];

/// `NoteCreateSerializer` (paperless-ngx v3.5.1 serialisers.py): `note = CharField()` (required,
/// trimmed, not blank, not null, no NUL characters) and `format = ChoiceField(Note.Format.choices,
/// default="plain", required=False)`. Errors as DRF: `{field: [message]}` in declaration order.
pub fn validate_note_create(data: &Value) -> Result<(String, &'static str), Value> {
    let empty = Map::new();
    let m = match data {
        Value::Object(m) => m,
        // an empty body parses to {} upstream (request.data is never None)
        Value::Null => &empty,
        // DRF Serializer with non-dict data
        other => {
            let got = match other {
                Value::Array(_) => "list",
                Value::String(_) => "str",
                Value::Bool(_) => "bool",
                Value::Number(n) if n.is_f64() => "float",
                _ => "int",
            };
            return Err(json!({"non_field_errors": [format!("Invalid data. Expected a dictionary, but got {got}.")]}));
        }
    };
    let mut errors = Map::new();
    let note = match m.get("note") {
        None => {
            errors.insert("note".into(), json!(["This field is required."]));
            None
        }
        Some(Value::Null) => {
            errors.insert("note".into(), json!(["This field may not be null."]));
            None
        }
        Some(v @ (Value::String(_) | Value::Number(_))) => {
            let s = crate::drf::py_str(v);
            let t = s.trim();
            if t.is_empty() {
                errors.insert("note".into(), json!(["This field may not be blank."]));
                None
            } else if t.contains('\0') {
                errors.insert("note".into(), json!(["Null characters are not allowed."]));
                None
            } else {
                Some(t.to_string())
            }
        }
        Some(_) => {
            errors.insert("note".into(), json!(["Not a valid string."]));
            None
        }
    };
    let format = match m.get("format") {
        None => Some(NOTE_FORMATS[0]),
        Some(Value::Null) => {
            errors.insert("format".into(), json!(["This field may not be null."]));
            None
        }
        Some(v) => {
            let s = crate::drf::py_str(v);
            match NOTE_FORMATS.iter().find(|f| **f == s) {
                Some(f) => Some(*f),
                None => {
                    errors.insert("format".into(), json!([format!("\"{s}\" is not a valid choice.")]));
                    None
                }
            }
        }
    };
    match (note, format) {
        (Some(n), Some(f)) if errors.is_empty() => Ok((n, f)),
        _ => Err(Value::Object(errors)),
    }
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
        // DRF's FormParser/MultiPartParser: `request.data[key]` is the last value of the key.
        let data = match crate::form::parse(&ctx.headers, &body).await? {
            Some(form) => {
                let mut m = Map::new();
                for key in ["note", "format"] {
                    if let Some(v) = form.get(key) {
                        m.insert(key.into(), Value::String(v.to_owned()));
                    }
                }
                Value::Object(m)
            }
            None => crate::views::parse_json_body(&ctx.headers, &body)?,
        };
        // 3.5.1: change permission first, then NoteCreateSerializer(...).is_valid(raise_exception=True)
        let Some((_, can_change)) = doc_and_perm(&state, &auth, &pk, "change_document").await? else {
            return Err(ApiError::Core(pl_core::ApiError::not_found()));
        };
        if !can_change {
            return Err(forbidden("Insufficient permissions to create notes"));
        }
        let (note, format) = validate_note_create(&data).map_err(ApiError::Validation)?;
        let result: ApiResult<Value> = async {
            let lctx = log_ctx(&state, &ctx, &auth, peer);
            let media = MediaCfg::from_config(&state.live_config());
            let mut conn = state.db().acquire().await?;
            let created = now_micros();
            let id: i32 = sqlx::query_scalar(
                "INSERT INTO documents_note (note, format, created, document_id, user_id) VALUES ($1, $2, $3, $4, $5) RETURNING id",
            )
            .bind(&note)
            .bind(format)
            .bind(created)
            .bind(doc_id as i32)
            .bind(auth.user().id as i32)
            .fetch_one(&mut *conn)
            .await?;
            if lctx.enabled {
                let n = NoteRow {
                    id: i64::from(id),
                    note: note.clone(),
                    format: format.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_create_validation_like_3_5_1() {
        assert_eq!(validate_note_create(&json!({"note": "  hi  "})), Ok(("hi".into(), "plain")));
        assert_eq!(validate_note_create(&json!({"note": "x", "format": "markdown"})), Ok(("x".into(), "markdown")));
        assert_eq!(validate_note_create(&json!({"note": 5})), Ok(("5".into(), "plain")));
        assert_eq!(validate_note_create(&json!({})), Err(json!({"note": ["This field is required."]})));
        assert_eq!(validate_note_create(&Value::Null), Err(json!({"note": ["This field is required."]})));
        assert_eq!(validate_note_create(&json!({"note": "   "})), Err(json!({"note": ["This field may not be blank."]})));
        assert_eq!(validate_note_create(&json!({"note": null})), Err(json!({"note": ["This field may not be null."]})));
        assert_eq!(validate_note_create(&json!({"note": ["a"]})), Err(json!({"note": ["Not a valid string."]})));
        assert_eq!(validate_note_create(&json!({"note": true})), Err(json!({"note": ["Not a valid string."]})));
        let both = validate_note_create(&json!({"format": "html", "note": ""})).unwrap_err();
        assert_eq!(both, json!({"note": ["This field may not be blank."], "format": ["\"html\" is not a valid choice."]}));
        // declaration order (note before format), independent of the request order
        assert_eq!(both.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()), Some(vec!["note".to_string(), "format".to_string()]));
        assert_eq!(validate_note_create(&json!({"note": "a", "format": null})), Err(json!({"format": ["This field may not be null."]})));
        assert_eq!(validate_note_create(&json!({"note": "a", "format": ""})), Err(json!({"format": ["\"\" is not a valid choice."]})));
        assert_eq!(validate_note_create(&json!({"note": "a\u{0}b"})), Err(json!({"note": ["Null characters are not allowed."]})));
        assert_eq!(
            validate_note_create(&json!([1])),
            Err(json!({"non_field_errors": ["Invalid data. Expected a dictionary, but got list."]}))
        );
    }
}
