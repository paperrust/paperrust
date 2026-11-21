//! `POST /api/documents/email/` and (deprecated) `POST /api/documents/{id}/email/`.
//!
//! upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.email_documents, email_document:
//! `permission_classes=[IsAuthenticated, ViewDocumentsPermissions]`), src/documents/serialisers.py@v3.4.2
//! (EmailSerializer ← DocumentListSerializer: `documents`, `addresses`, `subject`, `message`,
//! `use_archive_version`; validate_addresses, validate_documents), src/documents/mail.py@v3.4.2
//! (send_email, EmailAttachment), src/documents/models.py@v3.4.2 (source_path, archive_path,
//! get_public_filename). Sending: [`crate::effects::smtp`] (Django SMTP backend).
//!
//! `email_document` upstream does `request.data.copy().setlist("documents", [pk])`: with a JSON body
//! `request.data` is a `dict` without `setlist` → AttributeError → 500. PaperRust accepts JSON objects
//! like forms (Known differences, B-313).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use pl_authz::AuthUser;
use pl_authz::perms::VIEW_DOCUMENTS_PERMISSIONS;
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{ApiError, AppState, Drf, RequestContext};
use pl_files::db::DocRow;
use pl_files::docfile::{self, Ctx};
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Map, Value, json};

use crate::drf::{self, CharOpts, Errors, Raw};
use crate::effects::smtp::{self, EmailAttachment, EmailSettings};

const MSG_ERROR: &str = "Error emailing documents, check logs for more detail.";

/// Validated data of `EmailSerializer`.
#[derive(Debug, Clone, PartialEq)]
pub struct EmailRequest {
    pub documents: Vec<i64>,
    /// `",".join(address_list)` from `validate_addresses`.
    pub addresses: String,
    pub subject: String,
    pub message: String,
    pub use_archive_version: bool,
}

/// Django `EmailValidator` (same rules as `pl_auth_api::http::is_valid_email`).
pub fn is_valid_email(value: &str) -> bool {
    if value.is_empty() || value.len() > 320 {
        return false;
    }
    let Some((user, domain)) = value.rsplit_once('@') else {
        return false;
    };
    if user.is_empty() || domain.is_empty() {
        return false;
    }
    let user_ok = if user.starts_with('"') && user.ends_with('"') && user.len() >= 2 {
        true
    } else {
        user.split('.').all(|atom| !atom.is_empty() && atom.chars().all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+/=?^_`{|}~-".contains(c)))
    };
    if !user_ok {
        return false;
    }
    if domain.starts_with('[') && domain.ends_with(']') {
        return domain[1..domain.len() - 1].parse::<std::net::IpAddr>().is_ok();
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return domain == "localhost";
    }
    let tld = labels.last().copied().unwrap_or("");
    let tld_ok = (tld.len() >= 2 && tld.chars().all(|c| c.is_alphabetic() || c == '-') && !tld.starts_with('-') && !tld.ends_with('-'))
        || (tld.starts_with("xn--") && tld.len() > 4);
    tld_ok
        && labels[..labels.len() - 1].iter().all(|l| {
            !l.is_empty()
                && l.chars().count() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
}

/// `EmailSerializer.validate_addresses`.
pub fn validate_addresses(addresses: &str) -> Result<String, Value> {
    let list: Vec<&str> = addresses.split(',').map(|a| a.trim()).collect();
    for a in &list {
        if !is_valid_email(a) {
            return Err(json!([format!("Invalid email address: {a}")]));
        }
    }
    Ok(list.join(","))
}

fn char_required(data: &Map<String, Value>, key: &str, errors: &mut Errors) -> Option<String> {
    let r = match drf::raw(data, key) {
        Raw::Missing => Err(drf::err(drf::MSG_REQUIRED)),
        Raw::Null => Err(drf::err(drf::MSG_NULL)),
        Raw::Val(v) => drf::char_field(v, CharOpts { allow_blank: false, max_length: None }),
    };
    match r {
        Ok(s) => Some(s),
        Err(e) => {
            errors.add(key, e);
            None
        }
    }
}

/// `EmailSerializer(data=...).is_valid()`. `Err` = 400 body (field errors in declaration order).
pub async fn validate(db: &sqlx::PgPool, data: &Value) -> Result<Result<EmailRequest, Value>, ApiError> {
    let Value::Object(data) = data else {
        return Ok(Err(drf::non_dict_error(data)));
    };
    let mut errors = Errors::default();
    // documents: ListField(child=IntegerField(), required=True) + validate_documents
    let documents = match drf::raw(data, "documents") {
        Raw::Missing => {
            errors.add("documents", drf::err(drf::MSG_REQUIRED));
            None
        }
        Raw::Null => {
            errors.add("documents", drf::err(drf::MSG_NULL));
            None
        }
        Raw::Val(v) => match pl_files::bulk::drf_int_list(v) {
            Ok(ids) => {
                if pl_files::db::count_existing(db, &ids).await? as usize != ids.len() {
                    errors.add("documents", drf::err("Some documents in documents don't exist or were specified twice."));
                    None
                } else if ids.is_empty() {
                    errors.add("documents", drf::err("At least one document is required"));
                    None
                } else {
                    Some(ids)
                }
            }
            Err(e) => {
                errors.add("documents", e);
                None
            }
        },
    };
    let addresses = char_required(data, "addresses", &mut errors).and_then(|a| match validate_addresses(&a) {
        Ok(a) => Some(a),
        Err(e) => {
            errors.add("addresses", e);
            None
        }
    });
    let subject = char_required(data, "subject", &mut errors);
    let message = char_required(data, "message", &mut errors);
    // BooleanField(default=True)
    let use_archive_version = match drf::raw(data, "use_archive_version") {
        Raw::Missing => Some(true),
        Raw::Null => {
            errors.add("use_archive_version", drf::err(drf::MSG_NULL));
            None
        }
        Raw::Val(v) => match drf::boolean_field(v) {
            Ok(b) => Some(b),
            Err(e) => {
                errors.add("use_archive_version", e);
                None
            }
        },
    };
    if !errors.is_empty() {
        return Ok(Err(errors.into_value()));
    }
    match (documents, addresses, subject, message, use_archive_version) {
        (Some(documents), Some(addresses), Some(subject), Some(message), Some(use_archive_version)) => {
            Ok(Ok(EmailRequest { documents, addresses, subject, message, use_archive_version }))
        }
        _ => Err(ApiError::internal("EmailSerializer: incomplete validated data")),
    }
}

/// `documents.exclude(id__in=permitted_document_ids(user)).exists()`.
async fn has_unpermitted(db: &sqlx::PgPool, user: &pl_authz::User, ids: &[i64]) -> Result<bool, ApiError> {
    let permitted = pl_authz::objects::permitted_document_ids(Some(user), "view_document", false);
    let q = Query::select()
        .expr(Expr::val(1))
        .from(Alias::new("documents_document"))
        .and_where(Expr::col(Alias::new("id")).is_in(ids.iter().copied()))
        .and_where(Expr::col(Alias::new("deleted_at")).is_null())
        .and_where(Expr::col(Alias::new("id")).not_in_subquery(permitted))
        .limit(1)
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    Ok(sqlx::query_with(&sql, values).fetch_optional(db).await?.is_some())
}

/// Attachments in `Document.objects.filter(pk__in=...)` order (Meta ordering `-created`).
async fn attachments(cx: Ctx<'_>, docs: &[DocRow], use_archive: bool) -> Result<Vec<EmailAttachment>, String> {
    let mut out = Vec::with_capacity(docs.len());
    for doc in docs {
        let archive = use_archive && doc.has_archive_version();
        let path = if archive { docfile::archive_path(cx, doc) } else { docfile::source_path(cx, doc).await.map_err(|e| e.to_string())? };
        let Some(path) = path else { return Err(format!("unsafe path for document {}", doc.id)) };
        // Root owns metadata for all versions
        let root = match doc.root_document_id {
            Some(r) => pl_files::db::load_doc(cx.db, r, true).await.map_err(|e| e.to_string())?,
            None => None,
        };
        let context = root.as_ref().unwrap_or(doc);
        let friendly = docfile::public_filename(cx, doc, context, archive, 0).await.map_err(|e| e.to_string())?;
        out.push(EmailAttachment { path, mime_type: doc.mime_type.clone(), friendly_name: friendly });
    }
    Ok(out)
}

/// `email_documents` from the validated data (permissions, attachments, sending).
pub async fn email_documents(state: &AppState, user: &pl_authz::User, data: &Value) -> Result<Response, ApiError> {
    let db = state.db();
    let req = match validate(db, data).await? {
        Ok(r) => r,
        Err(body) => return Err(ApiError::validation(body)),
    };
    if has_unpermitted(db, user, &req.documents).await? {
        return Err(ApiError::django_text(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }
    let docs = pl_files::db::load_docs_ordered(db, &req.documents).await?;
    let addresses: Vec<String> = req.addresses.split(',').map(|a| a.trim().to_string()).collect();
    // SMTP live from the web UI (docs/SETTINGS.md WP4b).
    let settings = EmailSettings::from_core(&state.live_config());
    let sent = async {
        let att = attachments(Ctx::new(state), &docs, req.use_archive_version).await?;
        let mail = smtp::build_message(&settings, &req.subject, &req.message, &addresses, &att)?;
        smtp::send(&settings, &mail).await.map_err(|e| e.to_string())
    }
    .await;
    match sent {
        Ok(_) => {
            tracing::debug!(
                target: "paperless.api",
                "Sent documents {:?} via email to {:?}",
                docs.iter().map(|d| d.id).collect::<Vec<_>>(),
                addresses
            );
            Ok(Drf::ok(json!({"message": "Email sent"})).into_response())
        }
        Err(e) => {
            tracing::warn!(target: "paperless.api", "An error occurred emailing documents: {e}");
            Err(send_failed())
        }
    }
}

/// Sending failed (no SMTP, server unreachable, attachment missing). upstream:
/// `HttpResponseServerError` (500); PaperRust: 400 with the same text (B-313, Known differences),
/// because a 500 looks like a server crash. The reason is only in the log.
fn send_failed() -> ApiError {
    ApiError::django_text(StatusCode::BAD_REQUEST, MSG_ERROR)
}

/// DRF `JSONParser` (empty body → `{}`).
fn parse_json(ctx: &RequestContext, body: &[u8]) -> Result<Value, ApiError> {
    let ct = ctx.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if body.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    if main != "application/json" {
        return Err(ApiError::unsupported_media_type(ct));
    }
    serde_json::from_slice(body).map_err(|e| ApiError::json_body_parse_error(body, e))
}

fn respond(r: Result<Response, ApiError>) -> Response {
    match r {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// `POST /api/documents/email/`.
pub async fn email_documents_handler(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Response {
    respond(
        async {
            let user = auth.require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS)?.clone();
            let data = parse_json(&ctx, &body)?;
            email_documents(&state, &user, &data).await
        }
        .await,
    )
}

/// QueryDict of a form body as `request.data` (last value per key, like
/// `QueryDict.__getitem__`); `documents` = `[pk]` (`setlist`).
pub fn form_data_with_pk(body: &[u8], pk: &str) -> Value {
    let mut m = Map::new();
    for (k, v) in url::form_urlencoded::parse(body) {
        m.insert(k.into_owned(), Value::String(v.into_owned()));
    }
    m.insert("documents".into(), json!([pk]));
    Value::Object(m)
}

/// `POST /api/documents/{id}/email/` (deprecated, API v9).
pub async fn email_document_handler(
    State(state): State<AppState>,
    Path(pk): Path<String>,
    ctx: RequestContext,
    auth: AuthUser,
    body: Bytes,
) -> Response {
    respond(
        async {
            let user = auth.require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS)?.clone();
            let ct = ctx.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
            let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            let data = if main == "application/x-www-form-urlencoded" {
                form_data_with_pk(&body, &pk)
            } else {
                // JSON (or empty): upstream `request.data` is a dict → `.setlist` missing →
                // AttributeError → 500. PaperRust: treat JSON object like a form, otherwise 400.
                match parse_json(&ctx, &body)? {
                    Value::Object(mut m) => {
                        m.insert("documents".into(), json!([pk]));
                        Value::Object(m)
                    }
                    _ => return Err(ApiError::validation(json!({"non_field_errors": ["Invalid data. Expected a dictionary."]}))),
                }
            };
            email_documents(&state, &user, &data).await
        }
        .await,
    )
}

pub fn routes() -> Vec<pl_core::ApiRoute> {
    use axum::http::Method;
    use axum::routing::post;
    use pl_core::ApiRoute;
    use pl_core::route::{ALLOW_POST_ONLY, drf_view};
    vec![
        ApiRoute::new(
            "documents.email",
            Method::POST,
            "/api/documents/email/",
            "src/documents/views.py::DocumentViewSet.email_documents",
            drf_view(post(email_documents_handler), ALLOW_POST_ONLY),
        ),
        ApiRoute::new(
            "documents.email_document",
            Method::POST,
            "/api/documents/{id}/email/",
            "src/documents/views.py::DocumentViewSet.email_document (deprecated)",
            drf_view(post(email_document_handler), ALLOW_POST_ONLY),
        )
        .accepts_form(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_like_upstream() {
        assert_eq!(validate_addresses(" a@example.com , b@example.org"), Ok("a@example.com,b@example.org".into()));
        assert_eq!(validate_addresses("bad, x@y.de"), Err(json!(["Invalid email address: bad"])));
        assert_eq!(validate_addresses("a@b.c"), Err(json!(["Invalid email address: a@b.c"])));
        assert_eq!(validate_addresses("x@y.de,"), Err(json!(["Invalid email address: "])));
        assert!(is_valid_email("root@localhost"));
    }

    #[test]
    fn form_pk_overrides_documents() {
        let v = form_data_with_pk(b"documents=9&addresses=a%40b.de&subject=s&subject=t", "5");
        assert_eq!(v, json!({"documents": ["5"], "addresses": "a@b.de", "subject": "t"}));
    }

    /// B-313: sending without SMTP is not a server error (upstream 500).
    #[test]
    fn send_failure_is_400() {
        assert_eq!(send_failed().into_response().status(), StatusCode::BAD_REQUEST);
    }
}
