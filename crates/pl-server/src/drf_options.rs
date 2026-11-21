//! `OPTIONS` on DRF API views: `SimpleMetadata.determine_metadata` without a per-view handler.
//!
//! Every `/api/` path served by Rust routes gets DRF's metadata answer for `OPTIONS` (unless a
//! route handles `OPTIONS` itself): `{"name", "description", "renders", "parses"}` with the view's
//! `Allow` and `Vary: Accept`, after DRF's authentication (`APIView.initial`, same approximation as
//! the 405 answer in [`crate::routing`]). The view facts come from the upstream URL configuration
//! ([`crate::frontend::urlconf::UPSTREAM`]) plus the tables below, taken from the upstream source.
//!
//! Known differences (docs/contract/pl-server.md): `actions` (field metadata of the serializer for
//! views that accept POST/PUT) is not produced; view-specific permission checks before the
//! metadata (e.g. admin-only views) are not evaluated; PaperRust-only `/api/` routes get a name
//! derived from the path.
//!
//! upstream: rest_framework 3.18.3 metadata.py (`SimpleMetadata`), views.py (`APIView.options`,
//! `get_view_name`, `get_view_description`), utils/formatting.py (`camelcase_to_spaces`, `dedent`),
//! decorators.py (`action`: `name = pretty_name(func.__name__)`, `description = func.__doc__`),
//! routers.py (`suffix` "List"/"Instance"), src/documents/views.py@v3.4.2,
//! src/paperless/views.py@v3.4.2, src/paperless_mail/views.py@v3.4.2.

use serde_json::{Value, json};

use crate::frontend::urlconf::{Kind, UPSTREAM};

/// DRF default renderers (`JSONRenderer`, `BrowsableAPIRenderer`).
const RENDERS: &[&str] = &["application/json", "text/html"];
/// DRF default parsers (`JSONParser`, `FormParser`, `MultiPartParser`).
const PARSES: &[&str] = &["application/json", "application/x-www-form-urlencoded", "multipart/form-data"];
const JSON_ONLY: &[&str] = &["application/json"];
const MULTIPART_ONLY: &[&str] = &["multipart/form-data"];

/// Views with their own `parser_classes` (class attribute, inherited) or action `parser_classes`.
const PARSER_OVERRIDES: &[(&str, &[&str])] = &[
    // DocumentOperationPermissionMixin.parser_classes = (JSONParser,)
    ("BulkEditView", JSON_ONLY),
    ("DeleteDocumentsView", JSON_ONLY),
    ("ReprocessDocumentsView", JSON_ONLY),
    ("RotateDocumentsView", JSON_ONLY),
    ("MergeDocumentsView", JSON_ONLY),
    ("MergeDocumentsAsVersionsView", JSON_ONLY),
    ("EditPdfDocumentsView", JSON_ONLY),
    ("RemovePasswordDocumentsView", JSON_ONLY),
    ("BulkDownloadView", JSON_ONLY),
    ("BulkEditObjectsView", JSON_ONLY),
    ("PostDocumentView", MULTIPART_ONLY),
    ("SelectionDataView", &["multipart/form-data", "application/json"]),
    // @action(parser_classes=[parsers.MultiPartParser])
    ("UnifiedSearchViewSet.update_version", MULTIPART_ONLY),
];

/// Class docstrings (`view.__class__.__doc__`, dedented) and action docstrings
/// (`@action` → `description = func.__doc__`); every other view has none (`""`).
const DESCRIPTIONS: &[(&str, &str)] = &[
    ("DisconnectSocialAccountView", "Disconnects a social account provider from the user account"),
    ("GenerateAuthTokenView", "Generates (or re-generates) an auth token, requires a logged in user\nunlike the default DRF endpoint"),
    ("ProfileView", "User profile view, only available when logged in"),
    ("SocialAccountProvidersView", "List of social account providers"),
    ("TOTPView", "TOTP views"),
    ("StoragePathViewSet.test", "Test storage path against a document"),
    ("TasksViewSet.summary", "Aggregated task statistics per task_type over the last N days (default 30)."),
    ("TasksViewSet.status_counts", "Aggregated task counts for task UI sections."),
    ("TasksViewSet.active", "Currently pending and running tasks (capped at 50)."),
    ("TasksViewSet.run", "Manually dispatch a background task. Superuser only."),
];

/// `@action(name=…)` and the Python function names behind the URL-configuration labels.
const ACTION_NAMES: &[(&str, &str)] = &[("UnifiedSearchViewSet.history", "Audit Trail"), ("UnifiedSearchViewSet.next_asn", "Get Next ASN")];
const ACTION_FUNCS: &[(&str, &str)] = &[("UnifiedSearchViewSet.versions", "delete_version")];

/// What `SimpleMetadata` reports for a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewMeta {
    pub name: String,
    pub description: String,
    pub renders: &'static [&'static str],
    pub parses: &'static [&'static str],
}

impl ViewMeta {
    /// The response body (key order as DRF renders it).
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "renders": self.renders,
            "parses": self.parses,
        })
    }
}

/// `rest_framework.utils.formatting.camelcase_to_spaces`: a space before every uppercase letter
/// that follows a lowercase one or is followed by a non-uppercase character, `_` → space, then
/// Python's `str.title()`.
pub fn camelcase_to_spaces(content: &str) -> String {
    let chars: Vec<char> = content.chars().collect();
    let mut out = String::with_capacity(content.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let after_lower = i > 0 && chars[i - 1].is_ascii_lowercase();
            let before_non_upper = chars.get(i + 1).is_some_and(|n| !n.is_ascii_uppercase());
            if after_lower || before_non_upper {
                out.push(' ');
            }
        }
        out.push(c);
    }
    let joined = out.trim().split('_').collect::<Vec<_>>().join(" ");
    py_title(&joined)
}

/// Python `str.title()`: first cased letter of every run upper, the rest lower.
pub fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_cased = true;
        } else {
            out.push(c);
            prev_cased = false;
        }
    }
    out
}

/// Django `pretty_name`: `_` → space, first letter upper, rest lower (`str.capitalize`).
pub fn pretty_name(name: &str) -> String {
    let s = name.replace('_', " ");
    let mut it = s.chars();
    match it.next() {
        Some(f) => f.to_uppercase().chain(it.flat_map(char::to_lowercase)).collect(),
        None => String::new(),
    }
}

/// `get_view_name` for a class name (`View`, then `ViewSet` stripped).
fn class_view_name(class: &str) -> String {
    let base = class.strip_suffix("View").unwrap_or(class);
    let base = base.strip_suffix("ViewSet").unwrap_or(base);
    camelcase_to_spaces(base)
}

fn lookup<'a>(table: &'a [(&str, &'a str)], key: &str) -> Option<&'a str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// `{id}`/`{pk}` → `{}` (Rust route path → URL-configuration pattern).
fn normalize(path: &str) -> String {
    path.split('/').map(|s| if s.starts_with('{') && s.ends_with('}') { "{}" } else { s }).collect::<Vec<_>>().join("/")
}

/// Metadata of the view behind the Rust route path `route_path` (e.g. `/api/tags/{id}/`).
pub fn view_metadata(route_path: &str) -> ViewMeta {
    let norm = normalize(route_path);
    let upstream = UPSTREAM.iter().filter(|u| u.kind != Kind::Websocket).find(|u| u.pattern.replace("{d}", "{}") == norm);
    let Some(u) = upstream else {
        // PaperRust-only route: name from the last literal path segment.
        let last = norm.split('/').rev().find(|s| !s.is_empty() && *s != "{}").unwrap_or("api");
        return ViewMeta { name: py_title(&last.replace(['_', '-'], " ")), description: String::new(), renders: RENDERS, parses: PARSES };
    };
    let view = u.view;
    let (class, action) = match view.split_once('.') {
        Some((c, a)) => (c, Some(a)),
        None => (view, None),
    };
    let name = match action {
        Some(_) => match lookup(ACTION_NAMES, view) {
            Some(n) => n.to_owned(),
            None => pretty_name(lookup(ACTION_FUNCS, view).unwrap_or(action.unwrap_or_default())),
        },
        None if class.ends_with("ViewSet") && u.kind == Kind::Router => {
            let suffix = if norm.ends_with("{}/") { "Instance" } else { "List" };
            format!("{} {suffix}", class_view_name(class))
        }
        None => class_view_name(class),
    };
    let description = lookup(DESCRIPTIONS, view).or_else(|| lookup(DESCRIPTIONS, class)).unwrap_or("").to_owned();
    let parses = PARSER_OVERRIDES.iter().find(|(k, _)| *k == view || *k == class).map(|(_, p)| *p).unwrap_or(PARSES);
    ViewMeta { name, description, renders: RENDERS, parses }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_like_drf() {
        assert_eq!(camelcase_to_spaces("UnifiedSearch"), "Unified Search");
        assert_eq!(camelcase_to_spaces("TOTP"), "Totp");
        assert_eq!(camelcase_to_spaces("SpectacularAPI"), "Spectacular Api");
        assert_eq!(camelcase_to_spaces("PaperlessObtainAuthToken"), "Paperless Obtain Auth Token");
        assert_eq!(camelcase_to_spaces("UiSettings"), "Ui Settings");
        assert_eq!(class_view_name("SavedViewViewSet"), "Saved View");
        assert_eq!(class_view_name("TagViewSet"), "Tag");
        assert_eq!(class_view_name("BulkEditView"), "Bulk Edit");
        assert_eq!(pretty_name("next_asn"), "Next asn");
        assert_eq!(pretty_name("delete_version"), "Delete version");
    }

    /// Parity run 2026-11-20 F4: `OPTIONS /api/documents/` (UnifiedSearchViewSet list).
    #[test]
    fn documents_list() {
        let m = view_metadata("/api/documents/");
        assert_eq!(
            serde_json::to_string(&m.to_json()).unwrap_or_default(),
            r#"{"name":"Unified Search List","description":"","renders":["application/json","text/html"],"parses":["application/json","application/x-www-form-urlencoded","multipart/form-data"]}"#
        );
    }

    #[test]
    fn views_and_actions() {
        assert_eq!(view_metadata("/api/tags/{id}/").name, "Tag Instance");
        assert_eq!(view_metadata("/api/saved_views/").name, "Saved View List");
        assert_eq!(view_metadata("/api/documents/{id}/history/").name, "Audit Trail");
        assert_eq!(view_metadata("/api/documents/next_asn/").name, "Get Next ASN");
        assert_eq!(view_metadata("/api/documents/{id}/notes/").name, "Notes");
        assert_eq!(view_metadata("/api/documents/{id}/versions/{version_id}/").name, "Delete version");
        let up = view_metadata("/api/documents/{id}/update_version/");
        assert_eq!((up.name.as_str(), up.parses), ("Update version", MULTIPART_ONLY));
        let be = view_metadata("/api/documents/bulk_edit/");
        assert_eq!((be.name.as_str(), be.parses), ("Bulk Edit", JSON_ONLY));
        let p = view_metadata("/api/profile/");
        assert_eq!((p.name.as_str(), p.description.as_str()), ("Profile", "User profile view, only available when logged in"));
        assert_eq!(view_metadata("/api/profile/totp/").name, "Totp");
        let t = view_metadata("/api/storage_paths/test/");
        assert_eq!((t.name.as_str(), t.description.as_str()), ("Test", "Test storage path against a document"));
        assert_eq!(view_metadata("/api/x-ai/review/").name, "Review");
    }
}
