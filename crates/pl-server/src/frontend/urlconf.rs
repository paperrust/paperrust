//! The upstream URL configuration as data: which paths Django answers with a *specific* view
//! (everything else falls through to the catch-all `IndexView`), and which of those patterns
//! have no active Rust route (startup message without Django).
//!
//! upstream: src/paperless/urls.py@v3.4.2 (`urlpatterns`, `api_router`), list cross-checked with
//! `manage.py show_urls` of the reference (2026-11-08). DRF `DefaultRouter`: list `^p/$`, detail
//! `^p/{lookup}/$` with lookup `[^/.]+`, actions, plus format-suffix variants
//! `^p\.(?P<format>[a-z0-9]+)/?$`. allauth provider URLs (`build_provider_urlpatterns`) only
//! exist for installed providers; matched generically here.
//!
//! Approximations (see docs/contract/pl-server.md, Known differences): `{}` matches any segment
//! (Django: `\d+`/`\w+`/`[-:\w]+` per pattern) except where noted, and `re_path` patterns without
//! `$` are prefixes.

/// How a pattern matches a (decoded) request path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Whole path (segments; `{}` = one non-empty segment).
    Exact,
    /// `re_path` without `$`: the path starts with the pattern.
    Prefix,
    /// DRF router route: `{}` excludes `.`; format-suffix variant accepted.
    Router,
    /// Channels websocket route: never matched by HTTP (a plain GET falls through to the
    /// index); upgrade requests are decided before ([`crate::frontend::fallback`]).
    Websocket,
}

/// One upstream URL pattern.
#[derive(Debug, Clone, Copy)]
pub struct UpstreamUrl {
    pub pattern: &'static str,
    pub kind: Kind,
    /// upstream view (for the startup message / report).
    pub view: &'static str,
}

const fn e(pattern: &'static str, view: &'static str) -> UpstreamUrl {
    UpstreamUrl { pattern, kind: Kind::Exact, view }
}
const fn p(pattern: &'static str, view: &'static str) -> UpstreamUrl {
    UpstreamUrl { pattern, kind: Kind::Prefix, view }
}
const fn r(pattern: &'static str, view: &'static str) -> UpstreamUrl {
    UpstreamUrl { pattern, kind: Kind::Router, view }
}

/// All upstream patterns with a specific view (not `IndexView`).
pub const UPSTREAM: &[UpstreamUrl] = &[
    e("/api/", "RedirectView(schema/view/)"),
    e("/api/auth/login/", "allauth LoginView"),
    e("/api/auth/logout/", "allauth LogoutView"),
    e("/api/search/", "GlobalSearchView"),
    p("/api/search/autocomplete/", "SearchAutoCompleteView"),
    p("/api/statistics/", "StatisticsView"),
    p("/api/documents/post_document/", "PostDocumentView"),
    p("/api/documents/bulk_edit/", "BulkEditView"),
    p("/api/documents/delete/", "DeleteDocumentsView"),
    p("/api/documents/reprocess/", "ReprocessDocumentsView"),
    p("/api/documents/rotate/", "RotateDocumentsView"),
    p("/api/documents/merge/", "MergeDocumentsView"),
    p("/api/documents/merge_as_versions/", "MergeDocumentsAsVersionsView"),
    p("/api/documents/edit_pdf/", "EditPdfDocumentsView"),
    p("/api/documents/remove_password/", "RemovePasswordDocumentsView"),
    p("/api/documents/bulk_download/", "BulkDownloadView"),
    p("/api/documents/selection_data/", "SelectionDataView"),
    p("/api/documents/chat/", "ChatStreamingView"),
    p("/api/bulk_edit_objects/", "BulkEditObjectsView"),
    p("/api/remote_version/", "RemoteVersionView"),
    p("/api/ui_settings/", "UiSettingsView"),
    e("/api/token/", "PaperlessObtainAuthTokenView"),
    e("/api/profile/", "ProfileView"),
    e("/api/profile/generate_auth_token/", "GenerateAuthTokenView"),
    e("/api/profile/disconnect_social_account/", "DisconnectSocialAccountView"),
    e("/api/profile/social_account_providers/", "SocialAccountProvidersView"),
    e("/api/profile/totp/", "TOTPView"),
    p("/api/status/", "SystemStatusView"),
    p("/api/trash/", "TrashView"),
    p("/api/oauth/callback/", "OauthCallbackView"),
    e("/api/schema/", "SpectacularAPIView"),
    p("/api/schema/view/", "SpectacularSwaggerView"),
    p("/api/auth/headless/", "allauth headless (app/browser API)"),
    // DefaultRouter
    r("/api/correspondents/", "CorrespondentViewSet"),
    r("/api/correspondents/{}/", "CorrespondentViewSet"),
    r("/api/document_types/", "DocumentTypeViewSet"),
    r("/api/document_types/{}/", "DocumentTypeViewSet"),
    r("/api/documents/", "UnifiedSearchViewSet"),
    r("/api/documents/{}/", "UnifiedSearchViewSet"),
    r("/api/documents/{}/ai_suggestions/", "UnifiedSearchViewSet.ai_suggestions"),
    r("/api/documents/{}/download/", "UnifiedSearchViewSet.download"),
    r("/api/documents/{}/email/", "UnifiedSearchViewSet.email_document"),
    r("/api/documents/{}/history/", "UnifiedSearchViewSet.history"),
    r("/api/documents/{}/metadata/", "UnifiedSearchViewSet.metadata"),
    r("/api/documents/{}/notes/", "UnifiedSearchViewSet.notes"),
    r("/api/documents/{}/preview/", "UnifiedSearchViewSet.preview"),
    r("/api/documents/{}/root/", "UnifiedSearchViewSet.root"),
    r("/api/documents/{}/share_links/", "UnifiedSearchViewSet.share_links"),
    r("/api/documents/{}/suggestions/", "UnifiedSearchViewSet.suggestions"),
    r("/api/documents/{}/thumb/", "UnifiedSearchViewSet.thumb"),
    r("/api/documents/{}/update_version/", "UnifiedSearchViewSet.update_version"),
    r("/api/documents/{}/versions/{}/", "UnifiedSearchViewSet.versions"),
    r("/api/documents/email/", "UnifiedSearchViewSet.email_documents"),
    r("/api/documents/next_asn/", "UnifiedSearchViewSet.next_asn"),
    r("/api/logs/", "LogViewSet"),
    r("/api/logs/{}/", "LogViewSet"),
    r("/api/tags/", "TagViewSet"),
    r("/api/tags/{}/", "TagViewSet"),
    r("/api/saved_views/", "SavedViewViewSet"),
    r("/api/saved_views/{}/", "SavedViewViewSet"),
    r("/api/storage_paths/", "StoragePathViewSet"),
    r("/api/storage_paths/{}/", "StoragePathViewSet"),
    r("/api/storage_paths/test/", "StoragePathViewSet.test"),
    r("/api/tasks/", "TasksViewSet"),
    r("/api/tasks/{}/", "TasksViewSet"),
    r("/api/tasks/acknowledge/", "TasksViewSet.acknowledge"),
    r("/api/tasks/active/", "TasksViewSet.active"),
    r("/api/tasks/run/", "TasksViewSet.run"),
    r("/api/tasks/status_counts/", "TasksViewSet.status_counts"),
    r("/api/tasks/summary/", "TasksViewSet.summary"),
    r("/api/users/", "UserViewSet"),
    r("/api/users/{}/", "UserViewSet"),
    r("/api/users/{}/deactivate_totp/", "UserViewSet.deactivate_totp"),
    r("/api/groups/", "GroupViewSet"),
    r("/api/groups/{}/", "GroupViewSet"),
    r("/api/mail_accounts/", "MailAccountViewSet"),
    r("/api/mail_accounts/{}/", "MailAccountViewSet"),
    r("/api/mail_accounts/{}/process/", "MailAccountViewSet.process"),
    r("/api/mail_accounts/test/", "MailAccountViewSet.test"),
    r("/api/mail_rules/", "MailRuleViewSet"),
    r("/api/mail_rules/{}/", "MailRuleViewSet"),
    r("/api/share_link_bundles/", "ShareLinkBundleViewSet"),
    r("/api/share_link_bundles/{}/", "ShareLinkBundleViewSet"),
    r("/api/share_link_bundles/{}/rebuild/", "ShareLinkBundleViewSet.rebuild"),
    r("/api/share_links/", "ShareLinkViewSet"),
    r("/api/share_links/{}/", "ShareLinkViewSet"),
    r("/api/workflow_triggers/", "WorkflowTriggerViewSet"),
    r("/api/workflow_triggers/{}/", "WorkflowTriggerViewSet"),
    r("/api/workflow_actions/", "WorkflowActionViewSet"),
    r("/api/workflow_actions/{}/", "WorkflowActionViewSet"),
    r("/api/workflows/", "WorkflowViewSet"),
    r("/api/workflows/{}/", "WorkflowViewSet"),
    r("/api/custom_fields/", "CustomFieldViewSet"),
    r("/api/custom_fields/{}/", "CustomFieldViewSet"),
    r("/api/config/", "ApplicationConfigurationViewSet"),
    r("/api/config/{}/", "ApplicationConfigurationViewSet"),
    r("/api/processed_mail/", "ProcessedMailViewSet"),
    r("/api/processed_mail/{}/", "ProcessedMailViewSet"),
    r("/api/processed_mail/bulk_delete/", "ProcessedMailViewSet.bulk_delete"),
    // outside /api/
    e("/share/{}", "SharedLinkView"),
    e("/share/{}/", "SharedLinkView"),
    e("/favicon.ico", "FaviconView"),
    p("/admin/", "django.contrib.admin"),
    e("/fetch/doc/{d}", "RedirectView(api/documents/<pk>/download/)"),
    e("/fetch/thumb/{d}", "RedirectView(api/documents/<pk>/thumb/)"),
    e("/fetch/preview/{d}", "RedirectView(api/documents/<pk>/preview/)"),
    p("/assets/{}", "RedirectView(static/frontend/en-US/assets/<path>)"),
    e("/logo", "serve_logo"),
    p("/logo/", "serve_logo"),
    e("/accounts/login/", "allauth LoginView"),
    e("/accounts/logout/", "allauth LogoutView"),
    e("/accounts/signup/", "allauth SignupView"),
    e("/accounts/account_inactive/", "allauth AccountInactiveView"),
    e("/accounts/password/reset/", "allauth PasswordResetView"),
    e("/accounts/password/reset/done/", "allauth PasswordResetDoneView"),
    e("/accounts/password/reset/key/done/", "allauth PasswordResetFromKeyDoneView"),
    e("/accounts/confirm-email/{}/", "allauth ConfirmEmailView"),
    p("/accounts/password/reset/key/{}/", "allauth PasswordResetFromKeyView"),
    e("/accounts/3rdparty/login/cancelled/", "allauth LoginCancelledView"),
    e("/accounts/3rdparty/login/error/", "allauth LoginErrorView"),
    e("/accounts/3rdparty/signup/", "allauth socialaccount SignupView"),
    e("/accounts/oidc/{}/login/", "allauth openid_connect login"),
    e("/accounts/oidc/{}/login/callback/", "allauth openid_connect callback"),
    e("/accounts/{}/login/", "allauth provider login (other social providers)"),
    e("/accounts/{}/login/callback/", "allauth provider callback (other social providers)"),
    e("/accounts/2fa/authenticate/", "allauth mfa AuthenticateView"),
    UpstreamUrl { pattern: "/ws/status/", kind: Kind::Websocket, view: "channels StatusConsumer (websocket)" },
];

fn seg_matches(pat: &str, seg: &str, router: bool) -> bool {
    match pat {
        "{}" => !seg.is_empty() && !(router && seg.contains('.')),
        "{d}" => !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()),
        _ => pat == seg,
    }
}

fn segments_match(pattern: &str, path: &str, router: bool) -> bool {
    let ps: Vec<&str> = pattern.split('/').collect();
    let xs: Vec<&str> = path.split('/').collect();
    ps.len() == xs.len() && ps.iter().zip(&xs).all(|(p, x)| seg_matches(p, x, router))
}

/// `^p\.(?P<format>[a-z0-9]+)/?$` of a router pattern `.../p/`.
fn format_suffix_match(pattern: &str, path: &str) -> bool {
    let Some(base) = pattern.strip_suffix('/') else { return false };
    let path = path.strip_suffix('/').unwrap_or(path);
    let Some((stem, fmt)) = path.rsplit_once('.') else { return false };
    !fmt.is_empty()
        && fmt.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && !fmt.contains('/')
        && segments_match(base, stem, true)
}

impl UpstreamUrl {
    /// Does Django resolve the (decoded) `path` to this pattern?
    pub fn matches(&self, path: &str) -> bool {
        match self.kind {
            Kind::Exact => segments_match(self.pattern, path, false),
            Kind::Websocket => false,
            Kind::Router => segments_match(self.pattern, path, true) || format_suffix_match(self.pattern, path),
            Kind::Prefix => {
                // The last pattern segment takes the rest of the path: "" = anything,
                // "{}" = at least one character (`<path:path>`).
                let ps: Vec<&str> = self.pattern.split('/').collect();
                let xs: Vec<&str> = path.splitn(ps.len(), '/').collect();
                if xs.len() != ps.len() {
                    return false;
                }
                let n = ps.len() - 1;
                ps[..n].iter().zip(&xs[..n]).all(|(p, x)| seg_matches(p, x, false))
                    && match ps[n] {
                        "" => true,
                        "{}" => !xs[n].is_empty(),
                        lit => lit == xs[n],
                    }
            }
        }
    }
}

/// Django resolves `path` (decoded, as `request.path_info`) to a specific view, i.e. *not* to
/// the catch-all `IndexView`.
pub fn is_upstream_view(path: &str) -> bool {
    UPSTREAM.iter().any(|u| u.matches(path))
}

/// Prefix views that PaperRust serves itself (all `re_path` patterns without `$` and without
/// placeholders, except the unported allauth headless API and `/admin/`).
pub fn prefix_view_base(path: &str) -> Option<&'static str> {
    UPSTREAM
        .iter()
        .filter(|u| u.kind == Kind::Prefix && !u.pattern.contains('{'))
        .filter(|u| !matches!(u.pattern, "/api/auth/headless/" | "/admin/" | "/logo/"))
        .find(|u| path.len() > u.pattern.len() && path.starts_with(u.pattern))
        // a more specific exact/router pattern wins in Django's order only for listed paths;
        // the prefix views precede the router in `urlpatterns`
        .map(|u| u.pattern)
}

/// Without Django: rewrite `/api/statistics/anything` → `/api/statistics/` (query kept) so the
/// Rust route answers like Django's `re_path(r"^statistics/", …)`. `request.path` seen by the
/// handler is the base path (Known difference: absolute URLs built from it).
pub fn rewrite_prefix_view(mut req: axum::extract::Request) -> axum::extract::Request {
    let path = req.uri().path();
    if path.contains('%') {
        return req;
    }
    let Some(base) = prefix_view_base(path) else { return req };
    let pq = match req.uri().query() {
        Some(q) => format!("{base}?{q}"),
        None => base.to_string(),
    };
    if let Ok(pq) = pq.parse::<axum::http::uri::PathAndQuery>() {
        let mut parts = req.uri().clone().into_parts();
        parts.path_and_query = Some(pq);
        if let Ok(u) = axum::http::Uri::from_parts(parts) {
            *req.uri_mut() = u;
        }
    }
    req
}

/// Request extension: the path had a DRF format suffix no renderer serves (e.g. `/api/tags/1.5/`
/// = pk `1`, format `5`). DRF's content negotiation raises `Http404` in `APIView.initial`, before
/// authentication → `{"detail": "Not found."}` ([`crate::body::gate`] answers it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RejectFormat;

/// Formats of the default renderers (`JSONRenderer.format = "json"`, `BrowsableAPIRenderer.format
/// = "api"`). PaperRust has no browsable API: `.api` is answered as JSON (Known difference).
const RENDERER_FORMATS: &[&str] = &["json", "api"];

/// `(path without the suffix, with trailing slash, format)` if `path` resolves only to the
/// format-suffix variant `^p\.(?P<format>[a-z0-9]+)/?$` of an upstream router route.
pub fn format_suffix(path: &str) -> Option<(String, String)> {
    if !path.starts_with("/api/") || !path.contains('.') {
        return None;
    }
    let routers = || UPSTREAM.iter().filter(|u| u.kind == Kind::Router);
    if routers().any(|u| segments_match(u.pattern, path, true)) {
        return None;
    }
    if !routers().any(|u| format_suffix_match(u.pattern, path)) {
        return None;
    }
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    let (stem, fmt) = trimmed.rsplit_once('.')?;
    Some((format!("{stem}/"), fmt.to_owned()))
}

/// Without Django: a DRF format suffix on a router route is resolved here – `.json`/`.api` →
/// the route without the suffix (query kept); any other format → same route, marked
/// [`RejectFormat`] (DRF 404). Paths with percent-escapes are left alone (like
/// [`rewrite_prefix_view`]).
pub fn rewrite_format_suffix(mut req: axum::extract::Request) -> axum::extract::Request {
    let path = req.uri().path();
    if path.contains('%') {
        return req;
    }
    let Some((stem, fmt)) = format_suffix(path) else { return req };
    if !RENDERER_FORMATS.contains(&fmt.as_str()) {
        req.extensions_mut().insert(RejectFormat);
    }
    let pq = match req.uri().query() {
        Some(q) => format!("{stem}?{q}"),
        None => stem,
    };
    if let Ok(pq) = pq.parse::<axum::http::uri::PathAndQuery>() {
        let mut parts = req.uri().clone().into_parts();
        parts.path_and_query = Some(pq);
        if let Ok(u) = axum::http::Uri::from_parts(parts) {
            *req.uri_mut() = u;
        }
    }
    req
}

/// `{x}` → `{}`, `{*x}` → `{*}` (Rust route paths).
fn normalize_rust(path: &str) -> String {
    path.split('/')
        .map(|s| {
            if s.starts_with("{*") {
                "{*}"
            } else if s.starts_with('{') && s.ends_with('}') {
                "{}"
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Upstream patterns without an active Rust route (method-independent; a pattern counts as
/// covered when at least one Rust route has the same path). Prefix patterns are covered by any
/// Rust route below them.
pub fn unported<'a>(active_paths: impl IntoIterator<Item = &'a str>) -> Vec<UpstreamUrl> {
    let rust: Vec<String> = active_paths.into_iter().map(normalize_rust).collect();
    UPSTREAM
        .iter()
        .filter(|u| {
            let pat = u.pattern.replace("{d}", "{}");
            !rust.iter().any(|r| match u.kind {
                Kind::Prefix => {
                    let base = pat.trim_end_matches("{}");
                    r.starts_with(base)
                }
                _ => *r == pat,
            })
        })
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_views_rewritten() {
        assert_eq!(prefix_view_base("/api/statistics/x"), Some("/api/statistics/"));
        assert_eq!(prefix_view_base("/api/statistics/"), None);
        assert_eq!(prefix_view_base("/api/oauth/callback/abc/"), Some("/api/oauth/callback/"));
        assert_eq!(prefix_view_base("/api/documents/bulk_edit/x"), Some("/api/documents/bulk_edit/"));
        assert_eq!(prefix_view_base("/logo/logo/x.svg"), None);
        assert_eq!(prefix_view_base("/api/auth/headless/x"), None);
        assert_eq!(prefix_view_base("/api/tags/1/"), None);
    }

    #[test]
    fn index_fallthrough() {
        for p in ["/", "/dashboard", "/documents/12/details", "/api/foo/", "/api", "/static/x.js", "/accounts/foo/",
            "/admin", "/fetch/doc/abc", "/assets/", "/media/x", "/api/tags/1/2/3/", "/ws/status/"] {
            assert!(!is_upstream_view(p), "{p}");
        }
    }

    #[test]
    fn specific_views() {
        for p in ["/api/", "/api/tags/", "/api/tags/5/", "/api/tags.json", "/api/tags/5.json/", "/api/documents/merge_as_versions/",
            "/api/statistics/x", "/admin/", "/admin/auth/user/", "/fetch/doc/12", "/assets/logo.svg", "/assets/a/b.svg",
            "/logo", "/logo/", "/logo/x.png", "/share/abc", "/share/abc/", "/favicon.ico", "/accounts/login/",
            "/accounts/password/reset/key/1-abc/", "/accounts/oidc/x/login/callback/", "/accounts/google/login/",
            "/api/auth/headless/browser/v1/config", "/api/documents/1/versions/2/"] {
            assert!(is_upstream_view(p), "{p}");
        }
    }

    /// Parity run 2026-11-20 F5: `/api/tags/1.5/` = pk 1 with format "5".
    #[test]
    fn format_suffixes() {
        assert_eq!(format_suffix("/api/tags/1.5/"), Some(("/api/tags/1/".into(), "5".into())));
        assert_eq!(format_suffix("/api/tags/5.json/"), Some(("/api/tags/5/".into(), "json".into())));
        assert_eq!(format_suffix("/api/tags.json"), Some(("/api/tags/".into(), "json".into())));
        assert_eq!(format_suffix("/api/documents/3/notes.api"), Some(("/api/documents/3/notes/".into(), "api".into())));
        assert_eq!(format_suffix("/api/tags/5/"), None);
        assert_eq!(format_suffix("/api/tags/1.X/"), None, "format must be [a-z0-9]+");
        assert_eq!(format_suffix("/api/statistics.json"), None, "not a router route");
        assert_eq!(format_suffix("/static/a.json"), None);
        let req = |p: &str| axum::http::Request::builder().uri(p).body(axum::body::Body::empty()).unwrap_or_default();
        let r = rewrite_format_suffix(req("/api/tags/1.5/?x=1"));
        assert_eq!((r.uri().path_and_query().map(|p| p.as_str()), r.extensions().get::<RejectFormat>().is_some()), (Some("/api/tags/1/?x=1"), true));
        let r = rewrite_format_suffix(req("/api/tags/2.json/"));
        assert_eq!((r.uri().path(), r.extensions().get::<RejectFormat>().is_some()), ("/api/tags/2/", false));
    }

    #[test]
    fn unported_list() {
        let missing = unported(["/api/tags/", "/api/tags/{id}/", "/logo/{*filename}", "/api/statistics/"]);
        let pats: Vec<&str> = missing.iter().map(|u| u.pattern).collect();
        assert!(!pats.contains(&"/api/tags/"));
        assert!(!pats.contains(&"/api/tags/{}/"));
        assert!(!pats.contains(&"/logo/"));
        assert!(!pats.contains(&"/api/statistics/"));
        assert!(pats.contains(&"/admin/"));
        assert!(pats.contains(&"/api/documents/merge_as_versions/"));
    }
}
