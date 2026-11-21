// upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.download, preview, thumb,
//           file_response, _resolve_request_and_root_doc)
//           src/documents/conditionals.py@v3.4.2 (preview_etag, preview_last_modified,
//           thumbnail_etag, thumbnail_last_modified)
//
// Routes are wrapped with `pl_core::route::drf_view(.., ALLOW_READ_ONLY)` (Allow + Vary: Accept
// on every response, like DRF's finalize_response).

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pl_authz::models::DOCUMENT;
use pl_authz::{AuthError, AuthUser, User};
use pl_core::AppState;

use crate::compat;
use crate::conditional;
use crate::db::{self, DocRow};
use crate::docfile::{self, Ctx, ServeError};
use crate::naming;
use crate::perms;
use crate::resolve::{self, Params, VersionError};
use crate::serve;

/// A response the view *raised* (DRF exception handling), as opposed to one it returned.
/// Raised responses bypass `condition()` / `cache_control()` post-processing.
pub enum Outcome {
    Returned(Response),
    Raised(Response),
}

impl Outcome {
    fn into_response(self) -> Response {
        match self {
            Outcome::Returned(r) | Outcome::Raised(r) => r,
        }
    }
}

/// DRF `initial()`: authentication + `IsAuthenticated` + `PaperlessObjectPermissions.has_permission`
/// (GET/HEAD → `documents.view_document`).
#[allow(clippy::result_large_err)]
fn drf_check(auth: Result<AuthUser, AuthError>) -> Result<AuthUser, Response> {
    let auth = auth.map_err(IntoResponse::into_response)?;
    auth.require_model_perms(&DOCUMENT).map_err(IntoResponse::into_response)?;
    Ok(auth)
}

/// DRF router pk: lookup regex `[^/.]+`; non-integers 500 upstream, 404 here (Known difference 2).
fn parse_pk(id: &str) -> Option<i64> {
    match resolve::py_int(id) {
        Ok(Some(v)) => Some(v),
        _ => None,
    }
}

enum StepError {
    Db(sqlx::Error),
    Auth(AuthError),
}

impl From<sqlx::Error> for StepError {
    fn from(e: sqlx::Error) -> Self {
        StepError::Db(e)
    }
}

impl From<AuthError> for StepError {
    fn from(e: AuthError) -> Self {
        StepError::Auth(e)
    }
}

fn step_error(context: &str, e: StepError) -> Outcome {
    match e {
        StepError::Db(e) => Outcome::Raised(compat::internal(context, e)),
        StepError::Auth(e) => Outcome::Raised(e.into_response()),
    }
}

/// `_resolve_request_and_root_doc(pk, request, include_deleted=True)` + object permission.
async fn request_and_root(cx: Ctx<'_>, user: &User, pk: Option<i64>) -> Result<Result<(DocRow, DocRow), Outcome>, StepError> {
    let Some(pk) = pk else {
        return Ok(Err(Outcome::Raised(compat::drf_not_found())));
    };
    let Some(request_doc) = db::load_doc(cx.db, pk, true).await? else {
        return Ok(Err(Outcome::Raised(compat::drf_not_found())));
    };
    let root = resolve::root_of(cx.db, &request_doc).await?;
    if !perms::has_view_on_root(cx.db, user, root.id, root.owner_id).await? {
        return Ok(Err(Outcome::Returned(compat::insufficient_permissions())));
    }
    Ok(Ok((request_doc, root)))
}

/// `_get_effective_file_doc` with the view's error mapping.
async fn effective(cx: Ctx<'_>, request_doc: &DocRow, root: &DocRow, params: &Params) -> Result<Result<DocRow, Outcome>, StepError> {
    Ok(match resolve::effective_file_doc(cx.db, request_doc, root, params).await? {
        Ok(d) => Ok(d),
        Err(VersionError::Invalid) => Err(Outcome::Raised(compat::drf_detail(StatusCode::NOT_FOUND, "Invalid version parameter"))),
        Err(VersionError::NotFound) => Err(Outcome::Raised(compat::drf_not_found())),
    })
}

fn map_serve(e: ServeError) -> Outcome {
    match e {
        ServeError::NotFound => Outcome::Raised(compat::drf_not_found()),
        ServeError::Db(e) => Outcome::Raised(compat::internal("serve_file db", e)),
        ServeError::Io(e) => Outcome::Raised(compat::internal("serve_file io", e)),
    }
}

/// The body shared by download and preview (`file_response` / `preview`).
async fn file_view(cx: Ctx<'_>, user: &User, pk: Option<i64>, params: &Params, disposition: &str, follow_formatting: bool) -> Outcome {
    let step = async {
        let (request_doc, root) = match request_and_root(cx, user, pk).await? {
            Ok(v) => v,
            Err(o) => return Ok(o),
        };
        let file_doc = match effective(cx, &request_doc, &root, params).await? {
            Ok(d) => d,
            Err(o) => return Ok(o),
        };
        let context = resolve::root_of(cx.db, &file_doc).await?;
        let use_archive = !params.original_requested() && file_doc.has_archive_version();
        Ok::<_, StepError>(match docfile::serve_file(cx, &file_doc, &context, use_archive, disposition, follow_formatting).await {
            Ok(r) => Outcome::Returned(r),
            Err(e) => map_serve(e),
        })
    };
    match step.await {
        Ok(o) => o,
        Err(e) => step_error("file view", e),
    }
}

/// GET /api/documents/{id}/download/
pub async fn download(
    State(state): State<AppState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    auth: Result<AuthUser, AuthError>,
) -> Response {
    let auth = match drf_check(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let params = Params::parse(raw.as_deref());
    file_view(Ctx::new(&state), auth.user(), parse_pk(&id), &params, "attachment", params.follow_formatting()).await.into_response()
}

/// Run `cache_control(no_cache=True)` + `condition()` around a view: compute validators,
/// maybe short-circuit, post-process.
async fn conditional_view<F, Fut>(
    method: &Method,
    headers: &HeaderMap,
    etag: Option<String>,
    last_modified: Option<i64>,
    view: F,
) -> Response
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Outcome>,
{
    let etag = etag.map(|e| conditional::quote_etag(&e));
    let pre = conditional::evaluate(method, headers, etag.as_deref(), last_modified);
    let outcome = match conditional::precondition_response(&pre) {
        Some(r) => Outcome::Returned(r),
        None => view().await,
    };
    match outcome {
        Outcome::Returned(r) => conditional::finish(r, method, etag.as_deref(), last_modified),
        Outcome::Raised(r) => r,
    }
}

/// GET /api/documents/{id}/preview/
pub async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    method: Method,
    headers: HeaderMap,
    auth: Result<AuthUser, AuthError>,
) -> Response {
    let auth = match drf_check(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let cx = Ctx::new(&state);
    let params = Params::parse(raw.as_deref());
    let pk = parse_pk(&id);
    let eff = match pk {
        Some(pk) => match resolve::effective_by_pk(cx.db, pk, &params).await {
            Ok(d) => d,
            Err(e) => return compat::internal("preview etag", e),
        },
        None => None,
    };
    // preview_etag / preview_last_modified
    let etag = eff.as_ref().and_then(|d| if params.original_requested() { Some(d.checksum.clone()) } else { d.archive_checksum.clone() });
    let lm = eff.as_ref().map(|d| d.modified.timestamp());
    conditional_view(&method, &headers, etag, lm, || file_view(cx, auth.user(), pk, &params, "inline", false)).await
}

/// GET /api/documents/{id}/thumb/
pub async fn thumb(
    State(state): State<AppState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    method: Method,
    headers: HeaderMap,
    auth: Result<AuthUser, AuthError>,
) -> Response {
    let auth = match drf_check(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let cx = Ctx::new(&state);
    let params = Params::parse(raw.as_deref());
    let pk = parse_pk(&id);
    let eff = match pk {
        Some(pk) => match resolve::effective_by_pk(cx.db, pk, &params).await {
            Ok(d) => d,
            Err(e) => return compat::internal("thumb etag", e),
        },
        None => None,
    };
    // thumbnail_etag / thumbnail_last_modified (mtime, UTC, truncated to seconds)
    let etag = eff.as_ref().map(|d| d.checksum.clone());
    let lm = match eff.as_ref() {
        Some(d) => tokio::fs::metadata(docfile::thumbnail_path(cx, d))
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64),
        None => None,
    };
    let user = auth.user();
    conditional_view(&method, &headers, etag, lm, || async move {
        let step = async {
            let (request_doc, root) = match request_and_root(cx, user, pk).await? {
                Ok(v) => v,
                Err(o) => return Ok(o),
            };
            let file_doc = match effective(cx, &request_doc, &root, &params).await? {
                Ok(d) => d,
                Err(o) => return Ok(o),
            };
            let path = docfile::thumbnail_path(cx, &file_doc);
            Ok::<_, StepError>(match serve::open(Some(&path)).await {
                Ok(f) => {
                    let mut r = docfile::file_response(f, "image/webp");
                    // FileResponse derives Content-Disposition from the file name.
                    let name = format!("{:07}.webp", file_doc.id);
                    if let Some(cd) = naming::django_content_disposition(false, &name)
                        && let Ok(v) = HeaderValue::from_str(&cd)
                    {
                        r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
                    }
                    Outcome::Returned(r)
                }
                Err(e) => map_serve(e.into()),
            })
        };
        match step.await {
            Ok(o) => o,
            Err(e) => step_error("thumb", e),
        }
    })
    .await
}
