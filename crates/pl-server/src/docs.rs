//! The user documentation (`docs/user/`, German, and `docs/user/en/`, English) as HTML under
//! `/rs/docs/` – PaperRust extension, no upstream view. The Markdown is compiled into the binary,
//! so server, Docker image and desktop app serve the docs of their own version, offline.
//!
//! Route ids (activate in `routes.toml`): `rs.docs.index` (`GET /rs/docs/`) and `rs.docs.page`
//! (`GET /rs/docs/{*path}`).
//!
//! * `/rs/docs/de/<page>.md`, `/rs/docs/en/<page>.md` (also `.html` or without extension): the
//!   rendered page (`text/html`). Links between the pages are rewritten to these URLs (relative,
//!   so a script-name prefix keeps working); links out of `docs/user/` go to the repository.
//! * `/rs/docs/` and `/rs/docs/<page>.md` (no language): `302` to the language of the UI – the
//!   language cookie (`django_language`, set by the UI's display-language setting), else
//!   `Accept-Language` (Django's `get_language_from_request`): German → `de/`, otherwise `en/`.
//!   The UI's doc links (`frontend/doc-links.json`, base `rs/docs/`) use this form.
//! * `PAPERRUST_DOCS_URL` (optional, an http(s) URL ending in `/` with the layout of
//!   `docs/user/`): every docs URL redirects there instead (e.g. a published online manual).
//! * No login needed (public manual; nothing instance-specific).

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use pl_core::{ApiRoute, AppState};
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd};

/// Language of a docs page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::De => "de",
            Lang::En => "en",
        }
    }

    fn parse(s: &str) -> Option<Lang> {
        match s {
            "de" => Some(Lang::De),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    /// Directory of the Markdown sources in the repository.
    fn repo_dir(self) -> &'static str {
        match self {
            Lang::De => "docs/user",
            Lang::En => "docs/user/en",
        }
    }

    /// Docs language for a Django language code (`de`, `de-ch`, `en-us`, `fr` …).
    pub fn for_language(code: &str) -> Lang {
        if code == "de" || code.starts_with("de-") || code.starts_with("de_") { Lang::De } else { Lang::En }
    }
}

macro_rules! pages {
    ($($name:literal),* $(,)?) => {
        /// Page names (file stem) – identical in both languages.
        pub const PAGES: &[&str] = &[$($name),*];
        const DE: &[&str] = &[$(include_str!(concat!("../../../docs/user/", $name, ".md"))),*];
        const EN: &[&str] = &[$(include_str!(concat!("../../../docs/user/en/", $name, ".md"))),*];
    };
}

pages!(
    "README",
    "getting-started",
    "installation",
    "umstieg",
    "migration",
    "configuration",
    "usage",
    "mail",
    "workflows",
    "automatik",
    "firmen",
    "custom-fields",
    "permissions",
    "storage-paths",
    "advanced",
    "asn-labels",
    "pdf-tools",
    "scanner",
    "ai",
    "api",
    "administration",
    "integrity",
    "ransomware",
    "security",
    "troubleshooting",
    "faq",
);

/// Markdown source of `page` (file stem) in `lang`.
pub fn source(lang: Lang, page: &str) -> Option<&'static str> {
    let i = PAGES.iter().position(|p| *p == page)?;
    Some(match lang {
        Lang::De => DE[i],
        Lang::En => EN[i],
    })
}

pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute::new("rs.docs.index", Method::GET, "/rs/docs/", "PaperRust extension (user documentation)", get(index)),
        ApiRoute::new("rs.docs.page", Method::GET, "/rs/docs/{*path}", "PaperRust extension (user documentation)", get(page)),
    ]
}

/// What a path below `/rs/docs/` resolves to.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// Render `page` in `lang`.
    Page(Lang, &'static str),
    /// Redirect (relative `Location`) – `None` language: negotiate, then `<lang>/<rest>`.
    Redirect(Option<Lang>, String),
    NotFound,
}

fn page_name(file: &str) -> Option<&'static str> {
    let stem = file.strip_suffix(".md").or_else(|| file.strip_suffix(".html")).unwrap_or(file);
    PAGES.iter().copied().find(|p| *p == stem)
}

/// Resolve the part after `/rs/docs/`.
pub fn resolve(path: &str) -> Target {
    let path = path.trim_start_matches('/');
    match path.split_once('/') {
        Some((l, rest)) => match Lang::parse(l) {
            Some(lang) if rest.is_empty() => Target::Redirect(Some(lang), "README.md".into()),
            Some(lang) => page_name(rest).map_or(Target::NotFound, |p| Target::Page(lang, p)),
            None => Target::NotFound,
        },
        None => match Lang::parse(path) {
            // `/rs/docs/de` → `de/README.md` (relative to `/rs/docs/`)
            Some(lang) => Target::Redirect(None, format!("{}/README.md", lang.code())),
            None if path.is_empty() => Target::Redirect(None, String::new()),
            None => page_name(path).map_or(Target::NotFound, |p| Target::Redirect(None, format!("{p}.md"))),
        },
    }
}

/// `PAPERRUST_DOCS_URL` target for `page` in `lang`.
pub fn online_url(base: &str, lang: Lang, page: &str) -> String {
    let prefix = if lang == Lang::En { "en/" } else { "" };
    format!("{base}{prefix}{page}.md")
}

fn redirect(location: &str) -> Response {
    let mut r = StatusCode::FOUND.into_response();
    if let Ok(v) = HeaderValue::from_str(location) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

fn online_base(st: &AppState) -> Option<String> {
    st.config()
        .env
        .get("PAPERRUST_DOCS_URL")
        .map(str::trim)
        .filter(|u| (u.starts_with("https://") || u.starts_with("http://")) && u.ends_with('/'))
        .map(str::to_string)
}

fn negotiated(st: &AppState, headers: &HeaderMap) -> Lang {
    Lang::for_language(pl_core::request::language_from_request(headers, &st.config().language_cookie_name))
}

async fn index(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let lang = negotiated(&st, &headers);
    match online_base(&st) {
        Some(base) => redirect(&online_url(&base, lang, "README")),
        None => redirect(&format!("{}/README.md", lang.code())),
    }
}

async fn page(State(st): State<AppState>, headers: HeaderMap, UrlPath(path): UrlPath<String>) -> Response {
    let target = resolve(&path);
    let online = online_base(&st);
    match target {
        Target::Page(lang, page) => match online {
            Some(base) => redirect(&online_url(&base, lang, page)),
            None => html_response(StatusCode::OK, render_page(lang, page)),
        },
        Target::Redirect(Some(lang), rest) => match online {
            Some(base) => redirect(&online_url(&base, lang, "README")),
            None => redirect(&rest),
        },
        Target::Redirect(None, rest) => {
            let lang = negotiated(&st, &headers);
            match (online, page_name(&rest)) {
                (Some(base), Some(p)) => redirect(&online_url(&base, lang, p)),
                (Some(base), None) => redirect(&online_url(&base, lang, "README")),
                // `/rs/docs/de` → `de/README.md`: already language-qualified
                (None, _) if rest.contains('/') => redirect(&rest),
                (None, _) => redirect(&format!("{}/{}", lang.code(), if rest.is_empty() { "README.md" } else { &rest })),
            }
        }
        Target::NotFound => html_response(StatusCode::NOT_FOUND, not_found_page(negotiated(&st, &headers))),
    }
}

fn html_response(status: StatusCode, body: String) -> Response {
    let mut r = (status, body).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=60"));
    h.insert("X-Content-Type-Options", HeaderValue::from_static("nosniff"));
    r
}

// ---------------------------------------------------------------------------------------------
// Rendering

/// Normalise `a/b/../c` (no leading `/`); `None` when it climbs above the repository root.
fn normalise(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    Some(out.join("/"))
}

/// Rewrite a link of a page in `lang`: relative `.md` links to docs pages → served URL
/// (relative to `/rs/docs/<lang>/`); other relative links → the repository on GitHub;
/// absolute URLs, `#anchors`, `mailto:` unchanged.
pub fn rewrite_link(lang: Lang, dest: &str) -> String {
    if dest.is_empty() || dest.starts_with('#') || dest.starts_with('/') || dest.contains("://") || dest.contains(':') {
        return dest.to_string();
    }
    let (path, frag) = match dest.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (dest, None),
    };
    let frag = frag.map(|f| format!("#{f}")).unwrap_or_default();
    let Some(resolved) = normalise(&format!("{}/{path}", lang.repo_dir())) else {
        return dest.to_string();
    };
    for (l, dir) in [(Lang::En, "docs/user/en/"), (Lang::De, "docs/user/")] {
        if let Some(file) = resolved.strip_prefix(dir)
            && !file.contains('/')
            && let Some(p) = page_name(file).filter(|_| file.ends_with(".md"))
        {
            return if l == lang { format!("{p}.md{frag}") } else { format!("../{}/{p}.md{frag}", l.code()) };
        }
    }
    format!("{}/blob/main/{resolved}{frag}", pl_core::brand::REPO_URL)
}

/// GitHub-like heading slug (lower case, spaces → `-`, punctuation dropped).
pub fn slug(text: &str) -> String {
    text.trim()
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                Some(c.to_lowercase().next().unwrap_or(c))
            } else if c == ' ' {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

/// Markdown → HTML body (links rewritten, headings with ids); returns `(title, body)`.
pub fn render_markdown(lang: Lang, md: &str) -> (String, String) {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES;
    let mut events: Vec<Event> = Parser::new_ext(md, opts)
        .map(|e| match e {
            Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
                let dest = CowStr::from(rewrite_link(lang, &dest_url));
                Event::Start(Tag::Link { link_type, dest_url: dest, title, id })
            }
            Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
                let dest = CowStr::from(rewrite_link(lang, &dest_url));
                Event::Start(Tag::Image { link_type, dest_url: dest, title, id })
            }
            e => e,
        })
        .collect();
    // heading ids + title (first h1)
    let mut title = String::new();
    let mut used = std::collections::HashSet::new();
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::Heading { level, id: None, .. }) = &events[i] {
            let level = *level;
            let mut text = String::new();
            let mut j = i + 1;
            while j < events.len() && !matches!(events[j], Event::End(TagEnd::Heading(_))) {
                if let Event::Text(t) | Event::Code(t) = &events[j] {
                    text.push_str(t);
                }
                j += 1;
            }
            if title.is_empty() && level == pulldown_cmark::HeadingLevel::H1 {
                title = text.trim().to_string();
            }
            let base = slug(&text);
            let mut s = base.clone();
            let mut n = 1;
            while !used.insert(s.clone()) {
                s = format!("{base}-{n}");
                n += 1;
            }
            if let Event::Start(Tag::Heading { id, .. }) = &mut events[i] {
                *id = Some(CowStr::from(s));
            }
        }
        i += 1;
    }
    let mut body = String::new();
    pulldown_cmark::html::push_html(&mut body, events.into_iter());
    (title, body)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const STYLE: &str = r#"
:root{--bg:#fff;--fg:#1d1d1f;--muted:#5f6368;--line:#e3e3e6;--code:#f4f4f6;--accent:#B7410E}
@media (prefers-color-scheme:dark){:root{--bg:#17181a;--fg:#e8e8ea;--muted:#a0a3a8;--line:#2e3034;--code:#222428;--accent:#e0703f}}
*{box-sizing:border-box}html{-webkit-text-size-adjust:100%}
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.6 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
header{border-bottom:1px solid var(--line);padding:.6rem 1rem;display:flex;gap:1rem;align-items:center;flex-wrap:wrap}
header .brand{font-weight:600;color:var(--accent);text-decoration:none}header nav{margin-left:auto;display:flex;gap:1rem}
main{max-width:56rem;margin:0 auto;padding:1rem 1rem 4rem}
a{color:var(--accent)}h1,h2,h3{line-height:1.25}h2{border-bottom:1px solid var(--line);padding-bottom:.2rem;margin-top:2rem}
code{background:var(--code);padding:.1em .3em;border-radius:4px;font-size:.9em}
pre{background:var(--code);padding:.8rem;border-radius:6px;overflow:auto}pre code{padding:0;background:none}
table{border-collapse:collapse;display:block;overflow-x:auto;margin:1rem 0}th,td{border:1px solid var(--line);padding:.35rem .6rem;text-align:left;vertical-align:top}
blockquote{margin:1rem 0;padding:.2rem 1rem;border-left:4px solid var(--line);color:var(--muted)}
img{max-width:100%}

/* Sprung aus der App (?-Symbol → Anker): Stelle etwa mittig zeigen und kurz hervorheben, ohne Skript */
:target{scroll-margin-top:38vh}
h1:target,h2:target,h3:target,h4:target,p:target,li:target,tr:has(:target),li:has(> :target),p:has(> :target),dt:target,dd:target{animation:pr-hl 2.6s ease-out}
@keyframes pr-hl{0%,35%{background:color-mix(in srgb,var(--accent) 22%,transparent);box-shadow:0 0 0 3px color-mix(in srgb,var(--accent) 45%,transparent)}100%{background:transparent;box-shadow:0 0 0 3px transparent}}
@media (prefers-reduced-motion:reduce){h1:target,h2:target,h3:target,h4:target,p:target,li:target,tr:has(:target),li:has(> :target),p:has(> :target){animation:none;background:color-mix(in srgb,var(--accent) 15%,transparent)}}
"#;

/// Complete HTML page of `page` in `lang`.
pub fn render_page(lang: Lang, page: &str) -> String {
    let md = source(lang, page).unwrap_or_default();
    let (title, body) = render_markdown(lang, md);
    let (home, other, other_label) = match lang {
        Lang::De => ("Übersicht", Lang::En, "English"),
        Lang::En => ("Contents", Lang::De, "Deutsch"),
    };
    let title = if title.is_empty() { page.to_string() } else { title };
    format!(
        "<!doctype html>\n<html lang=\"{code}\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title} – PaperRust</title>\n<style>{STYLE}</style>\n</head>\n<body>\n\
<header><a class=\"brand\" href=\"README.md\">PaperRust</a><nav><a href=\"README.md\">{home}</a>\
<a href=\"../{other}/{page}.md\" hreflang=\"{other}\">{other_label}</a></nav></header>\n\
<main>\n{body}</main>\n</body>\n</html>\n",
        code = lang.code(),
        title = esc(&title),
        other = other.code(),
    )
}

fn not_found_page(lang: Lang) -> String {
    let (h, t) = match lang {
        Lang::De => ("Seite nicht gefunden", "Zur Übersicht der Dokumentation"),
        Lang::En => ("Page not found", "Documentation contents"),
    };
    format!(
        "<!doctype html>\n<html lang=\"{c}\"><head><meta charset=\"utf-8\"><title>{h} – PaperRust</title>\
<style>{STYLE}</style></head><body><main><h1>{h}</h1><p><a href=\"/rs/docs/{c}/README.md\">{t}</a></p></main></body></html>\n",
        c = lang.code()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_docs_page_is_embedded_in_both_languages() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/user");
        for (dir, skip) in [(root.clone(), &["LINKMAP.md"][..]), (root.join("en"), &[][..])] {
            let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| n.ends_with(".md") && !skip.contains(&n.as_str()))
                .map(|n| n.trim_end_matches(".md").to_string())
                .collect();
            on_disk.sort();
            let mut embedded: Vec<String> = PAGES.iter().map(|s| s.to_string()).collect();
            embedded.sort();
            assert_eq!(on_disk, embedded, "docs pages in {} – update pages!() in docs.rs", dir.display());
        }
    }

    #[test]
    fn resolve_paths() {
        assert_eq!(resolve("de/usage.md"), Target::Page(Lang::De, "usage"));
        assert_eq!(resolve("en/usage.html"), Target::Page(Lang::En, "usage"));
        assert_eq!(resolve("en/custom-fields"), Target::Page(Lang::En, "custom-fields"));
        assert_eq!(resolve("usage.md"), Target::Redirect(None, "usage.md".into()));
        assert_eq!(resolve("de/"), Target::Redirect(Some(Lang::De), "README.md".into()));
        assert_eq!(resolve("en"), Target::Redirect(None, "en/README.md".into()));
        assert_eq!(resolve("de/LINKMAP.md"), Target::NotFound);
        assert_eq!(resolve("de/../../etc/passwd"), Target::NotFound);
        assert_eq!(resolve("fr/usage.md"), Target::NotFound);
        assert_eq!(resolve("nope.md"), Target::NotFound);
    }

    #[test]
    fn link_rewriting() {
        assert_eq!(rewrite_link(Lang::De, "usage.md#papierkorb"), "usage.md#papierkorb");
        assert_eq!(rewrite_link(Lang::De, "en/usage.md"), "../en/usage.md");
        assert_eq!(rewrite_link(Lang::En, "../usage.md#x"), "../de/usage.md#x");
        assert_eq!(rewrite_link(Lang::En, "mail.md"), "mail.md");
        assert_eq!(rewrite_link(Lang::De, "../DESKTOP.md"), format!("{}/blob/main/docs/DESKTOP.md", pl_core::brand::REPO_URL));
        assert_eq!(rewrite_link(Lang::En, "../../../README.md#x"), format!("{}/blob/main/README.md#x", pl_core::brand::REPO_URL));
        assert_eq!(rewrite_link(Lang::De, "https://example.org/a.md"), "https://example.org/a.md");
        assert_eq!(rewrite_link(Lang::De, "#anker"), "#anker");
        assert_eq!(rewrite_link(Lang::De, "mailto:a@b"), "mailto:a@b");
    }

    #[test]
    fn renders_tables_anchors_and_title() {
        let (title, body) =
            render_markdown(Lang::De, "# Titel\n\n<a id=\"x\"></a>\n## Zwei Wörter\n\n| a | b |\n|---|---|\n| 1 | [u](usage.md) |\n");
        assert_eq!(title, "Titel");
        assert!(body.contains("<h2 id=\"zwei-wörter\">"), "{body}");
        assert!(body.contains("<table>") && body.contains("<a href=\"usage.md\">u</a>"), "{body}");
        assert!(body.contains("<a id=\"x\"></a>"));
        let page = render_page(Lang::En, "usage");
        assert!(page.starts_with("<!doctype html>") && page.contains("<html lang=\"en\">"));
        assert!(page.contains("href=\"../de/usage.md\""));
    }

    #[test]
    fn language_choice() {
        assert_eq!(Lang::for_language("de"), Lang::De);
        assert_eq!(Lang::for_language("de-ch"), Lang::De);
        assert_eq!(Lang::for_language("en-us"), Lang::En);
        assert_eq!(Lang::for_language("fr"), Lang::En);
        assert_eq!(online_url("https://x.org/docs/user/", Lang::En, "usage"), "https://x.org/docs/user/en/usage.md");
    }
}
