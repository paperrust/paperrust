// upstream: src/documents/views.py@v3.4.2 (SharedLinkView.get, serve_file)
//           src/documents/models.py@v3.4.2 (ShareLink, ShareLinkBundle.absolute_file_path)
//           src/paperless/urls.py@v3.4.2 (^share/(?P<slug>\w+)/?$)

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use chrono::Utc;
use pl_core::AppState;

use crate::compat;
use crate::config;
use crate::db;
use crate::docfile::{self, Ctx, ServeError};
use crate::naming;
use crate::resolve;
use crate::serve;

/// Python `\w+` (Unicode word characters).
fn is_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// The language Django's LocaleMiddleware would activate (for the translated texts of the
/// 202/503 bundle answers, [`crate::share_i18n`]). Review 1 N1: same negotiation as Django
/// (`pl_core::locale`, cookie name with `PAPERLESS_COOKIE_PREFIX`, `Accept-Language` q-sorted).
fn active_language(headers: &HeaderMap, language_cookie_name: &str) -> &'static str {
    pl_core::locale::language_from_headers(headers, language_cookie_name)
}

/// GET /share/{slug}[/]
pub async fn shared_link(State(state): State<AppState>, Path(slug): Path<String>, headers: HeaderMap) -> Response {
    if !is_slug(&slug) {
        // No URL pattern matches upstream -> Django 404 page.
        return compat::django_404();
    }
    match inner(&state, &slug, &headers).await {
        Ok(r) => r,
        Err(e) => compat::internal("shared link", e),
    }
}

async fn inner(state: &AppState, slug: &str, headers: &HeaderMap) -> Result<Response, sqlx::Error> {
    let cx = Ctx::new(state);
    let db = cx.db;
    let login = &cx.cfg.login_url;
    let now = Utc::now();

    if let Some(link) = db::share_link(db, slug).await? {
        if link.expiration.is_some_and(|e| e < now) {
            return Ok(compat::redirect(&format!("{login}?sharelink_expired=1")));
        }
        // share_link.document: FK access through the base manager (trash included).
        let Some(doc) = db::load_doc(db, link.document_id, true).await? else {
            return Ok(compat::redirect(&format!("{login}?sharelink_notfound=1")));
        };
        let context = resolve::root_of(db, &doc).await?;
        let use_archive = link.file_version == "archive" && doc.has_archive_version();
        return Ok(match docfile::serve_file(cx, &doc, &context, use_archive, "inline", false).await {
            Ok(r) => r,
            Err(ServeError::NotFound) => compat::redirect(&format!("{login}?sharelink_notfound=1")),
            Err(ServeError::Db(e)) => return Err(e),
            Err(ServeError::Io(e)) => compat::internal("shared link io", e),
        });
    }

    let Some(bundle) = db::share_link_bundle(db, slug).await? else {
        return Ok(compat::redirect(&format!("{login}?sharelink_notfound=1")));
    };
    if bundle.expiration.is_some_and(|e| e < now) {
        return Ok(compat::redirect(&format!("{login}?sharelink_expired=1")));
    }
    let lang = active_language(headers, &state.config().language_cookie_name);
    if bundle.status == "pending" || bundle.status == "processing" {
        return Ok(compat::plain(StatusCode::ACCEPTED, crate::share_i18n::preparing(lang)));
    }
    let path = bundle_absolute_path(&cx.cfg.share_link_bundle_dir, &bundle.file_path);
    let opened = match (&bundle.status[..], path) {
        ("failed", _) | (_, None) => None,
        (_, Some(p)) => serve::open(Some(&p)).await.ok(),
    };
    let Some(opened) = opened else {
        return Ok(compat::plain(StatusCode::SERVICE_UNAVAILABLE, crate::share_i18n::unavailable(lang)));
    };
    let mut r = docfile::file_response(opened, "application/zip");
    let short: String = bundle.slug.chars().take(12).collect();
    let cd = naming::bundle_disposition(&format!("paperrust-share-{short}.zip"));
    if let Ok(v) = HeaderValue::from_str(&cd) {
        r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(r)
}

/// `ShareLinkBundle.absolute_file_path`: relative, resolved, inside the bundle directory.
fn bundle_absolute_path(dir: &std::path::Path, file_path: &str) -> Option<std::path::PathBuf> {
    if file_path.is_empty() {
        return None;
    }
    let rel = std::path::Path::new(file_path);
    if rel.is_absolute() || file_path.starts_with('/') {
        return None;
    }
    let bundle_dir = config::resolve(dir);
    let abs = config::resolve(&bundle_dir.join(rel));
    if !abs.starts_with(&bundle_dir) {
        return None;
    }
    Some(abs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert!(is_slug("abc_123"));
        assert!(is_slug("123notaslug"));
        assert!(!is_slug("a-b"));
        assert!(!is_slug(""));
    }

    #[test]
    fn languages() {
        let c = "django_language";
        let mut h = HeaderMap::new();
        assert_eq!(active_language(&h, c), "en-us");
        h.insert(header::ACCEPT_LANGUAGE, HeaderValue::from_static("de-CH,de;q=0.9"));
        assert_eq!(active_language(&h, c), "de-de");
        assert_eq!(crate::share_i18n::unavailable(active_language(&h, c)), "Das Freigabelink-Paket ist nicht verfügbar.");
        h.insert(header::COOKIE, HeaderValue::from_static("a=b; django_language=en-us"));
        assert_eq!(active_language(&h, c), "en-us");
        // Review 1 N1: q-sorted, not the first entry; prefixed cookie name
        let mut h = HeaderMap::new();
        h.insert(header::ACCEPT_LANGUAGE, HeaderValue::from_static("en;q=0.1, de;q=0.9"));
        assert_ne!(active_language(&h, c), "en-us");
        h.insert(header::COOKIE, HeaderValue::from_static("pl_django_language=en-us"));
        assert_ne!(active_language(&h, c), "en-us");
        assert_eq!(active_language(&h, "pl_django_language"), "en-us");
    }
}
