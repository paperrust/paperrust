// upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.update_version, _get_root_doc_for_version_action,
//           _get_version_doc_for_root, delete_version, update_version_label),
//           src/documents/serialisers.py@v3.4.2 (DocumentVersionSerializer, DocumentVersionLabelSerializer,
//           PostDocumentSerializer.validate_document), src/documents/versioning.py@v3.4.2
//           (get_root_document, versions_newest_first), src/documents/permissions.py@v3.4.2
//           (PaperlessObjectPermissions, has_perms_owner_aware), rest_framework/fields.py (FileField,
//           CharField, Field.get_value), rest_framework/utils/encoders.py (JSONEncoder: datetime)
//
//! Document versions:
//!
//! * `POST /api/documents/{id}/update_version/` – upload a new version (multipart, `consume_file`
//!   with `root_document_id` via the PaperRust queue; without it → Django).
//! * `DELETE /api/documents/{id}/versions/{version_id}/` – delete a version.
//! * `PATCH /api/documents/{id}/versions/{version_id}/` – change the version label.

use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use pl_authz::AuthUser;
use pl_authz::models::DOCUMENT;
use pl_authz::perms::{ObjRef, has_perms_owner_aware};
use pl_consumer::db::DocRow;
use pl_core::{AppState, RequestContext};

use crate::actions::{char_field, explicit_actor};
use crate::audit::RequestMeta;
use crate::auditlog;
use crate::docs;
use crate::drf::{self, Errors};
use crate::error::{ApiError, ApiResult};
use crate::files::MediaCfg;
use crate::update::{log_ctx, parse_pk};
use crate::views::Peer;

/// `settings.CONSUMER_PDF_RECOVERABLE_MIME_TYPES`.
const PDF_RECOVERABLE_MIME_TYPES: &[&str] = &["application/octet-stream"];
/// Django `DATA_UPLOAD_MAX_NUMBER_FILES`.
const MAX_NUMBER_FILES: usize = 100;

fn not_found() -> ApiError {
    ApiError::Core(pl_core::ApiError::not_found())
}

fn auth_err(e: pl_authz::AuthError) -> ApiError {
    ApiError::Core(e.into())
}

fn drf_ok(ctx: &RequestContext, v: Value) -> Response {
    pl_core::Drf::ok(v).indent(ctx.negotiation.indent).into_response()
}

/// DRF `JSONEncoder` for an aware `datetime` in UTC: `isoformat()` with `+00:00` → `Z`.
pub fn drf_encode_datetime(t: &DateTime<Utc>) -> String {
    let n = t.naive_utc();
    if n.and_utc().timestamp_subsec_micros() == 0 {
        n.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        n.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
    }
}

/// `Document.objects.select_related(...).get(pk=pk)` + `get_root_document(...)`.
async fn root_for_action(conn: &mut PgConnection, pk: &str) -> ApiResult<DocRow> {
    // Django: get(pk="abc") → ValueError (500); missing → Http404
    let Some(id) = parse_pk(pk) else {
        return Err(ApiError::Uncaught(format!("Field 'id' expected a number but got '{pk}'.")));
    };
    let Some(doc) = docs::load_live(conn, id).await? else { return Err(not_found()) };
    get_root_document(conn, doc).await
}

/// `get_root_document(doc)`: via `doc.root_document` (base manager, also in the trash).
async fn get_root_document(conn: &mut PgConnection, doc: DocRow) -> ApiResult<DocRow> {
    match doc.root_document_id {
        None => Ok(doc),
        Some(r) => Ok(docs::load_global(conn, r).await?.unwrap_or(doc)),
    }
}

/// `_get_version_doc_for_root`.
async fn version_for_root(conn: &mut PgConnection, root: &DocRow, version_id: &str) -> ApiResult<DocRow> {
    let Some(vid) = version_id.parse::<i64>().ok().filter(|v| i32::try_from(*v).is_ok()) else { return Err(not_found()) };
    let Some(v) = docs::load_live(conn, vid).await? else { return Err(not_found()) };
    if v.id != root.id && v.root_document_id != Some(root.id) {
        return Err(not_found());
    }
    Ok(v)
}

async fn owner_aware(state: &AppState, auth: &AuthUser, perm: &str, root: &DocRow) -> ApiResult<bool> {
    has_perms_owner_aware(state.db(), Some(auth.user()), perm, &DOCUMENT, ObjRef { id: root.id, owner_id: root.owner_id })
        .await
        .map_err(auth_err)
}

/// `Document.objects.filter(pk=root.pk).update(modified=now)`.
async fn touch_root(conn: &mut PgConnection, root_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE documents_document SET modified = $2 WHERE id = $1 AND deleted_at IS NULL")
        .bind(root_id as i32)
        .bind(docs::now_micros())
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// `document_updated.send(document=root)`, part 1 with the held connection: "document
/// updated" workflows and WebSocket. Part 2 ([`document_updated_finish`]) fetches connections itself and
/// runs only after the held one has been returned (FINDINGS B2: no second pool connection while
/// one is held).
async fn document_updated(
    state: &AppState,
    conn: &mut PgConnection,
    lctx: &auditlog::LogCtx,
    root_id: i64,
) -> ApiResult<crate::workflows::RunOutcome> {
    let media = MediaCfg::from_config(&state.live_config());
    let outcome = crate::workflows::run_updated(conn, lctx, &media, *state.tz(), root_id).await?;
    crate::hooks::send_websocket_document_updated(state, conn, root_id).await;
    Ok(outcome)
}

/// Part 2 of `document_updated`: enqueue AI tasks, further receivers (pl-ai).
async fn document_updated_finish(state: &AppState, outcome: &crate::workflows::RunOutcome, root_id: i64) {
    crate::workflows::enqueue_outcome(state, outcome).await;
    crate::hooks::document_updated(state, root_id).await;
}

// ------------------------------------------------------------------ DELETE versions/{id}/

/// DELETE /api/documents/{id}/versions/{version_id}/
pub async fn delete_version_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Peer(peer): Peer,
    Path((pk, version_id)): Path<(String, String)>,
) -> Response {
    match delete_version(&state, &ctx, &auth, peer, &pk, &version_id).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn delete_version(
    state: &AppState,
    ctx: &RequestContext,
    auth: &AuthUser,
    peer: Option<std::net::SocketAddr>,
    pk: &str,
    version_id: &str,
) -> ApiResult<Response> {
    // url_path=r"versions/(?P<version_id>\d+)": otherwise no match in Django's URL configuration
    // (upstream falls through to the catch-all index view); PaperRust answers like any other
    // `/api/` path without a route (JSON 404, docs/contract/pl-documents-write.md).
    if !is_version_id(version_id) {
        return Err(no_route());
    }
    auth.context().require_model_perms(&DOCUMENT).map_err(auth_err)?;
    // FINDINGS B2: hold the connection only in sections; permission check, search index and queue
    // fetch their own connections.
    let root = {
        let mut conn = state.db().acquire().await?;
        root_for_action(&mut conn, pk).await?
    };
    if !owner_aware(state, auth, "delete_document", &root).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    let version = {
        let mut conn = state.db().acquire().await?;
        version_for_root(&mut conn, &root, version_id).await?
    };
    if version.id == root.id {
        return Err(ApiError::BadRequestText("Cannot delete the root/original version. Delete the document instead.".into()));
    }
    let lctx = log_ctx(state, ctx, auth, peer);
    let media = MediaCfg::from_config(&state.live_config());
    crate::index::remove(state, &[version.id]).await;
    let meta = RequestMeta::from_request(&ctx.headers, peer);
    let actor = explicit_actor(state, auth.user().id, &meta);
    let (current, outcome) = {
        let mut conn = state.db().acquire().await?;
        // Lock order as when ingesting a version: root document, then version (s3 follow-up
        // to FINDINGS B1); complete transaction, retried on 40P01/40001.
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let r: ApiResult<()> = async {
                let mut tx = sqlx::Connection::begin(&mut *conn).await?;
                crate::update::lock_root_then_version(&mut tx, root.id, version.id).await?;
                crate::softdelete::soft_delete_document(&mut tx, &lctx, &media, version.id, None).await?;
                touch_root(&mut tx, root.id).await?;
                tx.commit().await?;
                Ok(())
            }
            .await;
            match r {
                Err(e) if e.is_retryable() && attempt < pl_consumer::db::MAX_ATTEMPTS => {
                    tokio::time::sleep(pl_consumer::db::retry_backoff(attempt)).await;
                }
                r => break r?,
            }
        }
        if actor.enabled {
            let r = docs::repr(&mut conn, &root).await?;
            let mut ch = Map::new();
            ch.insert("Version Deleted".into(), json!(["None", version.id]));
            auditlog::write(
                &mut conn,
                &actor,
                "document",
                root.id,
                &r,
                auditlog::UPDATE,
                ch,
                Some(json!({"reason": "Version deleted", "version_id": version.id})),
            )
            .await?;
        }
        // versions_newest_first(Document.objects.filter(Q(id=root) | Q(root_document=root))).first()
        let current: Option<i32> = sqlx::query_scalar(
            "SELECT id FROM documents_document WHERE deleted_at IS NULL AND (id = $1 OR root_document_id = $1) \
             ORDER BY version_index DESC NULLS LAST, id DESC LIMIT 1",
        )
        .bind(root.id as i32)
        .fetch_optional(&mut *conn)
        .await?;
        let outcome = document_updated(state, &mut conn, &lctx, root.id).await?;
        (current, outcome)
    };
    crate::index::add_or_update(state, &[root.id]).await;
    document_updated_finish(state, &outcome, root.id).await;
    Ok(drf_ok(ctx, json!({"result": "OK", "current_version_id": current.map(i64::from).unwrap_or(root.id)})))
}

// ------------------------------------------------------------------ PATCH versions/{id}/

/// PATCH /api/documents/{id}/versions/{version_id}/
pub async fn update_version_label_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Peer(peer): Peer,
    Path((pk, version_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Response {
    match update_version_label(&state, &ctx, &auth, peer, &pk, &version_id, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// `\d+` of `url_path=r"versions/(?P<version_id>\d+)"`.
fn is_version_id(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())
}

/// Answer for a path Django's URL configuration does not match: like an `/api/` path without a
/// route in PaperRust (`{"detail": "Not found."}`).
fn no_route() -> ApiError {
    ApiError::Core(pl_core::ApiError::not_found())
}

/// `DocumentVersionLabelSerializer` input from a form body (`CharField.get_value` for HTML input:
/// last value; missing → `empty` → "required"; `""` stays `""` because of `allow_blank`).
fn label_form_data(form: &crate::form::FormData) -> Value {
    let mut m = Map::new();
    if let Some(v) = form.get("version_label") {
        m.insert("version_label".into(), Value::String(v.to_owned()));
    }
    Value::Object(m)
}

/// `DocumentVersionLabelSerializer` (required, allow_blank, allow_null, max_length=64,
/// `validate_version_label`: strip, empty → None).
pub fn validate_label_body(data: &Value) -> Result<Option<String>, Value> {
    let Value::Object(m) = data else {
        return Err(drf::non_field(crate::views::not_a_dict_message(data)));
    };
    let mut errors = Errors::default();
    let label = match m.get("version_label") {
        None => {
            errors.add("version_label", drf::msg(drf::REQUIRED));
            None
        }
        Some(Value::Null) => None,
        Some(v) => match char_field(v, true, Some(64)) {
            Ok(s) => Some(s).filter(|s| !s.is_empty()),
            Err(e) => {
                errors.add("version_label", e);
                None
            }
        },
    };
    if errors.is_empty() { Ok(label) } else { Err(Value::Object(errors.0)) }
}

async fn update_version_label(
    state: &AppState,
    ctx: &RequestContext,
    auth: &AuthUser,
    peer: Option<std::net::SocketAddr>,
    pk: &str,
    version_id: &str,
    body: &axum::body::Bytes,
) -> ApiResult<Response> {
    if !is_version_id(version_id) {
        return Err(no_route());
    }
    auth.context().require_model_perms(&DOCUMENT).map_err(auth_err)?;
    let data = match crate::form::parse(&ctx.headers, body).await? {
        Some(form) => label_form_data(&form),
        None => crate::views::parse_json_body(&ctx.headers, body)?,
    };
    let new_label = validate_label_body(&data).map_err(ApiError::Validation)?;

    // FINDINGS B2: no second pool connection while one is held.
    let root = {
        let mut conn = state.db().acquire().await?;
        root_for_action(&mut conn, pk).await?
    };
    if !owner_aware(state, auth, "change_document", &root).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    let lctx = log_ctx(state, ctx, auth, peer);
    let media = MediaCfg::from_config(&state.live_config());
    let meta = RequestMeta::from_request(&ctx.headers, peer);
    let actor = explicit_actor(state, auth.user().id, &meta);
    let (version, outcome) = {
        let mut conn = state.db().acquire().await?;
        let mut version = version_for_root(&mut conn, &root, version_id).await?;
        let old_label = version.version_label.clone();
        version.version_label = new_label.clone();
        // Lock order as when ingesting a version: root document, then version (s3 follow-up
        // to FINDINGS B1); complete transaction, retried on 40P01/40001.
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let r: ApiResult<()> = async {
                let mut tx = sqlx::Connection::begin(&mut *conn).await?;
                crate::update::lock_root_then_version(&mut tx, root.id, version.id).await?;
                let mut v = version.clone();
                docs::save(&mut tx, &lctx, Some(&media), &mut v, Some(&["version_label"])).await?;
                touch_root(&mut tx, root.id).await?;
                tx.commit().await?;
                Ok(())
            }
            .await;
            match r {
                Err(e) if e.is_retryable() && attempt < pl_consumer::db::MAX_ATTEMPTS => {
                    tokio::time::sleep(pl_consumer::db::retry_backoff(attempt)).await;
                }
                r => break r?,
            }
        }
        if actor.enabled && old_label != new_label {
            let r = docs::repr(&mut conn, &root).await?;
            let mut ch = Map::new();
            ch.insert("Version Label".into(), json!([old_label, new_label]));
            auditlog::write(
                &mut conn,
                &actor,
                "document",
                root.id,
                &r,
                auditlog::UPDATE,
                ch,
                Some(json!({"reason": "Version label updated", "version_id": version.id})),
            )
            .await?;
        }
        let outcome = document_updated(state, &mut conn, &lctx, root.id).await?;
        (version, outcome)
    };
    document_updated_finish(state, &outcome, root.id).await;
    // PaperRust (pl-pdftools §4.1): like `versions[].redacted` in the document
    let redacted = crate::redaction_marks::is_redacted(state.db(), &version.checksum).await;
    Ok(drf_ok(
        ctx,
        json!({
            "id": version.id,
            "added": drf_encode_datetime(&version.added),
            "version_label": new_label,
            "checksum": version.checksum,
            "is_root": version.id == root.id,
            "redacted": redacted,
        }),
    ))
}

// ------------------------------------------------------------------ POST update_version/

/// Uploaded file (already on disk).
struct Upload {
    /// `UploadedFile.name` per Django's `sanitize_file_name`.
    name: String,
    path: std::path::PathBuf,
    size: u64,
}

#[derive(Default)]
struct Form {
    fields: Vec<(String, String)>,
    document: Option<Upload>,
    /// `html.is_html_input` (QueryDict); `false` = empty body without a form content type (`{}`).
    html: bool,
}

impl Form {
    fn get(&self, key: &str) -> Option<&str> {
        self.fields.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// Temporary folder, deleted on drop (except `keep`).
struct Spool {
    dir: std::path::PathBuf,
    keep: bool,
}

impl Drop for Spool {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Django `MultiPartParser.sanitize_file_name` incl. `html.unescape` – the same function as in
/// pl-server (final security review W-N4).
fn sanitize_upload_name(name: &str) -> Option<String> {
    pl_core::safepath::sanitize_file_name(name)
}

enum ReadError {
    Parse,
    Suspicious(&'static str),
    Io(std::io::Error),
}

async fn read_form(mut mp: Multipart, spool: &Spool) -> Result<Form, ReadError> {
    use tokio::io::AsyncWriteExt;
    let limit = pl_core::body::DATA_UPLOAD_MAX_MEMORY_SIZE;
    let mut form = Form { html: true, ..Default::default() };
    let mut field_bytes = 0usize;
    let mut files = 0usize;
    let mut seq = 0usize;
    while let Some(mut field) = mp.next_field().await.map_err(|_| ReadError::Parse)? {
        let Some(name) = field.name().map(str::to_string) else {
            while field.chunk().await.map_err(|_| ReadError::Parse)?.is_some() {}
            continue;
        };
        match field.file_name().map(str::to_string) {
            None => {
                field_bytes += name.len() + 2;
                let mut buf = Vec::new();
                while let Some(c) = field.chunk().await.map_err(|_| ReadError::Parse)? {
                    field_bytes += c.len();
                    if field_bytes > limit {
                        return Err(ReadError::Suspicious("RequestDataTooBig"));
                    }
                    buf.extend_from_slice(&c);
                }
                form.fields.push((name, String::from_utf8_lossy(&buf).into_owned()));
            }
            Some(raw) => {
                let Some(file_name) = sanitize_upload_name(&raw) else {
                    while field.chunk().await.map_err(|_| ReadError::Parse)?.is_some() {}
                    continue;
                };
                files += 1;
                if files > MAX_NUMBER_FILES {
                    return Err(ReadError::Suspicious("TooManyFilesSent"));
                }
                if name != "document" {
                    while field.chunk().await.map_err(|_| ReadError::Parse)?.is_some() {}
                    continue;
                }
                seq += 1;
                let path = spool.dir.join(format!(".paperrust-upload-{seq}.part"));
                let mut f = tokio::fs::File::create(&path).await.map_err(ReadError::Io)?;
                let mut size = 0u64;
                while let Some(c) = field.chunk().await.map_err(|_| ReadError::Parse)? {
                    size += c.len() as u64;
                    f.write_all(&c).await.map_err(ReadError::Io)?;
                }
                f.flush().await.map_err(ReadError::Io)?;
                drop(f);
                if let Some(prev) = form.document.replace(Upload { name: file_name, path, size }) {
                    let _ = std::fs::remove_file(prev.path);
                }
            }
        }
    }
    Ok(form)
}

/// `DocumentVersionSerializer.is_valid()` → `(document, version_label)`.
fn validate_version_form(form: &Form, registry: &pl_parsers::ParserRegistry) -> ApiResult<Result<Option<String>, Value>> {
    let mut errors = Errors::default();
    match &form.document {
        Some(f) if f.size == 0 => errors.add("document", drf::msg("The submitted file is empty.")),
        Some(f) => {
            let detected = pl_parsers::mime::detect_file(&f.path).map_err(|e| ApiError::Uncaught(e.to_string()))?;
            if !registry.is_mime_type_supported(&detected)
                && !(PDF_RECOVERABLE_MIME_TYPES.contains(&detected.as_str()) && f.name.ends_with(".pdf"))
            {
                errors.add("document", drf::msg(format!("File type {detected} not supported")));
            }
        }
        None if form.get("document").is_some() => {
            errors.add("document", drf::msg("The submitted data was not a file. Check the encoding type on the form."))
        }
        None => errors.add("document", drf::msg("No file was submitted.")),
    }
    // version_label: CharField(required=False, allow_blank, allow_null, max_length=64);
    // HTML input: missing → skipped, '' → '' (allow_blank)
    let label = match form.get("version_label") {
        None => None,
        Some(v) => match char_field(&Value::String(v.to_string()), true, Some(64)) {
            Ok(s) => Some(s),
            Err(e) => {
                errors.add("version_label", e);
                None
            }
        },
    };
    Ok(if errors.is_empty() { Ok(label) } else { Err(Value::Object(errors.0)) })
}

/// POST /api/documents/{id}/update_version/
pub async fn update_version_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
    req: Request,
) -> Response {
    match update_version(&state, &ctx, &auth, &pk, req).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

fn server_error() -> ApiError {
    ApiError::Core(pl_core::ApiError::django_text(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Error updating document, check logs for more detail.",
    ))
}

async fn update_version(state: &AppState, ctx: &RequestContext, auth: &AuthUser, pk: &str, req: Request) -> ApiResult<Response> {
    auth.context().require_model_perms(&DOCUMENT).map_err(auth_err)?;
    // consume_file can only be sent via the PaperRust queue (Celery: pickle)
    // pl-server always attaches the queue; without it the server is misconfigured (500).
    let Some(queue) = crate::pdf_edit::pg_queue(state) else {
        return Err(ApiError::Uncaught("update_version: PaperRust task queue not attached".into()));
    };
    let content_type =
        req.headers().get(header::CONTENT_TYPE).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
    let media = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let empty_body = axum::body::HttpBody::size_hint(req.body()).exact() == Some(0);
    let mk_spool = || -> ApiResult<Spool> {
        let p = pl_pdfops::bulk::scratch_path(&state.config().scratch_dir, "x").map_err(|e| ApiError::Uncaught(e.to_string()))?;
        let dir = p.parent().map(std::path::Path::to_path_buf).ok_or_else(|| ApiError::Uncaught("scratch dir".into()))?;
        Ok(Spool { dir, keep: false })
    };
    let mut spool = mk_spool()?;
    let form = if empty_body {
        // Request._parse without stream: form type → empty QueryDict, otherwise {}
        Form { html: media == "multipart/form-data" || media == "application/x-www-form-urlencoded", ..Default::default() }
    } else if media != "multipart/form-data" {
        return Err(ApiError::Core(pl_core::ApiError::unsupported_media_type(&content_type)));
    } else {
        let mp = Multipart::from_request(req, &()).await.map_err(|_| {
            ApiError::Core(pl_core::ApiError::parse_error(Some("Multipart form parse error - Invalid boundary in multipart: None")))
        })?;
        match read_form(mp, &spool).await {
            Ok(f) => f,
            Err(ReadError::Parse) => {
                return Err(ApiError::Core(pl_core::ApiError::parse_error(Some("Multipart form parse error - Invalid multipart body"))));
            }
            Err(ReadError::Suspicious(kind)) => {
                tracing::warn!(target: "django.security", kind, "upload rejected");
                return Err(ApiError::Core(pl_core::ApiError::django_bad_request()));
            }
            Err(ReadError::Io(e)) => return Err(ApiError::Uncaught(format!("upload spooling failed: {e}"))),
        }
    };
    let _ = form.html;
    let registry = pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default());
    let version_label = validate_version_form(&form, &registry)?.map_err(ApiError::Validation)?;
    let Some(file) = form.document else { return Err(ApiError::Uncaught("validated upload without file".into())) };

    let user = auth.user();
    let mut conn = state.db().acquire().await?;
    // Document.objects.get(pk=pk) – DoesNotExist → Http404 (ValueError for a non-numeric pk → 500)
    let root = root_for_action(&mut conn, pk).await?;
    drop(conn);
    if !user.has_perm("documents.change_document") || !owner_aware(state, auth, "change_document", &root).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }

    let r: Result<uuid::Uuid, String> = async {
        let target_name = pl_templating::sanitize_filename(&file.name, "");
        if target_name.is_empty() {
            return Err("file name is empty after sanitizing".to_string());
        }
        let target = spool.dir.join(&target_name);
        std::fs::rename(&file.path, &target).map_err(|e| e.to_string())?;
        // os.utime(temp_file_path, times=(t, t)), t = int(mktime(now))
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(Utc::now().timestamp().max(0) as u64);
        std::fs::File::options()
            .write(true)
            .open(&target)
            .and_then(|f| f.set_times(std::fs::FileTimes::new().set_accessed(now).set_modified(now)))
            .map_err(|e| e.to_string())?;
        let mut input = pl_consumer::ConsumableDocument::new(pl_consumer::DocumentSource::ApiUpload, &target).map_err(|e| e.to_string())?;
        input.root_document_id = Some(root.id);
        let overrides = pl_consumer::DocumentMetadataOverrides {
            version_label: version_label.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
            owner_id: Some(user.id),
            actor_id: Some(user.id),
            ..Default::default()
        };
        let kwargs = pl_consumer::task::consume_kwargs(&input, &overrides)?;
        let opts =
            pl_queue::EnqueueOptions { trigger_source: Some(pl_core::tasks::trigger_source::WEB_UI.to_string()), ..Default::default() };
        queue.enqueue(crate::pdf_edit::CONSUME_FILE, kwargs, opts).await.map_err(|e| e.to_string())
    }
    .await;
    match r {
        Ok(task_id) => {
            spool.keep = true;
            tracing::debug!(target: "paperless.api", "Updated document {} with new version", root.id);
            Ok(drf_ok(ctx, Value::String(task_id.to_string())))
        }
        Err(e) => {
            tracing::warn!(target: "paperless.api", "An error occurred updating document: {e}");
            Err(server_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// W-N4: upload name like Django incl. `html.unescape` (same as pl-server).
    #[test]
    fn upload_name_unescapes_entities() {
        assert_eq!(sanitize_upload_name("a&amp;b.pdf").as_deref(), Some("a&b.pdf"));
        assert_eq!(sanitize_upload_name("C:\\x\\R&#228;chnung.pdf").as_deref(), Some("Rächnung.pdf"));
        assert_eq!(sanitize_upload_name("x/&#46;&#46;"), None);
    }

    #[test]
    fn datetime_like_drf_encoder() {
        let t = Utc.with_ymd_and_hms(2026, 11, 8, 12, 30, 5).unwrap();
        assert_eq!(drf_encode_datetime(&t), "2026-11-08T12:30:05Z");
        let t = t + chrono::Duration::microseconds(1234);
        assert_eq!(drf_encode_datetime(&t), "2026-11-08T12:30:05.001234Z");
    }

    #[test]
    fn label_serializer() {
        assert_eq!(validate_label_body(&json!({"version_label": "  v2 "})), Ok(Some("v2".into())));
        assert_eq!(validate_label_body(&json!({"version_label": ""})), Ok(None));
        assert_eq!(validate_label_body(&json!({"version_label": null})), Ok(None));
        assert_eq!(validate_label_body(&json!({})), Err(json!({"version_label": [drf::REQUIRED]})));
        assert_eq!(
            validate_label_body(&json!({"version_label": "x".repeat(65)})),
            Err(json!({"version_label": ["Ensure this field has no more than 64 characters."]}))
        );
        assert_eq!(validate_label_body(&json!([1])), Err(drf::non_field("Invalid data. Expected a dictionary, but got list.")));
    }

    #[test]
    fn upload_names() {
        assert_eq!(sanitize_upload_name("C:\\x\\a.pdf").as_deref(), Some("a.pdf"));
        assert_eq!(sanitize_upload_name("../.."), None);
        assert_eq!(sanitize_upload_name("dir/f\u{0}.txt").as_deref(), Some("f.txt"));
    }

    #[test]
    fn version_form_validation() {
        let reg = pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default());
        let f = Form { html: true, ..Default::default() };
        assert_eq!(validate_version_form(&f, &reg).ok().and_then(|r| r.err()), Some(json!({"document": ["No file was submitted."]})));
        let f =
            Form { html: true, fields: vec![("document".into(), "x".into()), ("version_label".into(), "y".repeat(70))], document: None };
        assert_eq!(
            validate_version_form(&f, &reg).ok().and_then(|r| r.err()),
            Some(json!({
                "document": ["The submitted data was not a file. Check the encoding type on the form."],
                "version_label": ["Ensure this field has no more than 64 characters."]
            }))
        );
    }
}
