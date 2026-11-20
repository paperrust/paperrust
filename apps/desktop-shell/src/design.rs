//! UI switcher (docs/DESIGN2.md §6) in the tray: classic light UI under `/rs/app/…`,
//! PaperRust design under `/rs/app/m/…`. The main window switches to the same route of the other
//! UI; `?design=…` lets the web UI store the choice per user in ui_settings.

use tauri::Url;

/// Menu IDs.
pub const MODERN_ID: &str = "design-modern";
pub const CLASSIC_ID: &str = "design-classic";
pub const MODERN_LABEL: &str = "Neue Oberfläche (PaperRust-Design)";
pub const CLASSIC_LABEL: &str = "Klassische Oberfläche";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Design {
    Classic,
    Modern,
}

impl Design {
    /// `"modern"` / `"classic"` (ui_settings, `?design=`, `app.toml`); anything else → `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "modern" => Some(Self::Modern),
            "classic" => Some(Self::Classic),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modern => "modern",
            Self::Classic => "classic",
        }
    }
}

/// `?design=modern|classic` of a server page (tray, Angular user menu, links) – the choice that
/// the shell remembers for the next start. Local app pages do not count.
pub fn from_query(u: &Url) -> Option<Design> {
    if !matches!(u.scheme(), "http" | "https" | "prapi") || is_app_local(u) {
        return None;
    }
    u.query_pairs()
        .find(|(k, _)| k == "design")
        .and_then(|(_, v)| Design::parse(&v))
}

/// Start address of the main window: `base` (server root, `…/` – Angular UI) or, with the
/// PaperRust design chosen, `<base>rs/app/m/`. Subpaths (`https://host/paperless/`) are
/// kept; an unreadable address stays unchanged.
pub fn start_url(base: &str, design: Design) -> String {
    if design == Design::Classic {
        return base.to_string();
    }
    let Ok(mut u) = Url::parse(base) else {
        return base.to_string();
    };
    if !u.path().ends_with('/') {
        let p = format!("{}/", u.path());
        u.set_path(&p);
    }
    u.set_query(None);
    u.set_fragment(None);
    u.join("rs/app/m/").map(|j| j.to_string()).unwrap_or_else(|_| base.to_string())
}

/// Target URL for `target` starting from the current page of the main window. `None` if the
/// current page is not a server page (local pages of the app: first start, error, settings).
pub fn switch_url(current: &Url, target: Design) -> Option<Url> {
    if !matches!(current.scheme(), "http" | "https" | "prapi") || is_app_local(current) {
        return None;
    }
    let path = current.path();
    // Segment-exact: `/rs/app/mail` is a classic route, not `/rs/app/m` + `ail`
    let seg = |p: &str, pre: &str| -> Option<String> {
        let r = p.strip_prefix(pre)?;
        (r.is_empty() || r.starts_with('/')).then(|| r.trim_start_matches('/').to_string())
    };
    let rest = seg(path, "/rs/app/m")
        .or_else(|| seg(path, "/rs/app"))
        // Angular UI at `/` or login: entry of the chosen UI
        .unwrap_or_default();
    // Routes of the modern UI only → entry of the classic one
    let rest = if target == Design::Classic && ["inbox", "assistant"].iter().any(|r| rest == *r || rest.starts_with(&format!("{r}/"))) {
        "dashboard".to_string()
    } else {
        rest
    };
    let (base, flag) = match target {
        Design::Modern => ("/rs/app/m/", "modern"),
        Design::Classic => ("/rs/app/", "classic"),
    };
    let mut u = current.clone();
    u.set_path(&format!("{base}{rest}"));
    u.set_query(Some(&format!("design={flag}")));
    u.set_fragment(None);
    Some(u)
}

/// Pages from the app package (tauri://localhost or http://tauri.localhost on Windows).
fn is_app_local(u: &Url) -> bool {
    u.scheme() == "tauri" || u.host_str() == Some("tauri.localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn same_route_other_design() {
        let m = switch_url(&u("http://127.0.0.1:8000/rs/app/documents/12/details?x=1"), Design::Modern).unwrap();
        assert_eq!(m.as_str(), "http://127.0.0.1:8000/rs/app/m/documents/12/details?design=modern");
        let c = switch_url(&m, Design::Classic).unwrap();
        assert_eq!(c.as_str(), "http://127.0.0.1:8000/rs/app/documents/12/details?design=classic");
        let p = switch_url(&u("http://prapi.localhost/rs/app/m/inbox"), Design::Classic).unwrap();
        assert_eq!(p.as_str(), "http://prapi.localhost/rs/app/dashboard?design=classic");
        let mail = switch_url(&u("http://127.0.0.1:8000/rs/app/mail"), Design::Modern).unwrap();
        assert_eq!(mail.path(), "/rs/app/m/mail");
    }

    #[test]
    fn start_url_follows_last_choice() {
        // local (supervisor reports `http://127.0.0.1:<port>/`)
        assert_eq!(start_url("http://127.0.0.1:58123/", Design::Classic), "http://127.0.0.1:58123/");
        assert_eq!(start_url("http://127.0.0.1:58123/", Design::Modern), "http://127.0.0.1:58123/rs/app/m/");
        assert_eq!(start_url("http://127.0.0.1:58123", Design::Modern), "http://127.0.0.1:58123/rs/app/m/");
        // remote: base with subpath is kept
        assert_eq!(
            start_url("https://docs.example.ch/paperless/", Design::Modern),
            "https://docs.example.ch/paperless/rs/app/m/"
        );
        assert_eq!(
            start_url("https://docs.example.ch/paperless", Design::Modern),
            "https://docs.example.ch/paperless/rs/app/m/"
        );
        assert_eq!(start_url("kaputt", Design::Modern), "kaputt");
    }

    #[test]
    fn design_choice_from_query() {
        assert_eq!(from_query(&u("http://127.0.0.1:8000/rs/app/m/?design=modern")), Some(Design::Modern));
        assert_eq!(from_query(&u("http://127.0.0.1:8000/rs/app/x?a=1&design=classic")), Some(Design::Classic));
        assert_eq!(from_query(&u("http://127.0.0.1:8000/?design=bunt")), None);
        assert_eq!(from_query(&u("http://127.0.0.1:8000/dashboard")), None);
        assert_eq!(from_query(&u("http://tauri.localhost/index.html?design=modern")), None);
        // tray target carries the choice itself
        let t = switch_url(&u("http://127.0.0.1:8000/dashboard"), Design::Modern).unwrap();
        assert_eq!(from_query(&t), Some(Design::Modern));
        assert_eq!(Design::parse(" Modern "), Some(Design::Modern));
        assert_eq!(Design::Classic.as_str(), "classic");
    }

    #[test]
    fn angular_and_local_pages() {
        let a = switch_url(&u("https://paper.example.ch/documents?tags=1"), Design::Modern).unwrap();
        assert_eq!(a.as_str(), "https://paper.example.ch/rs/app/m/?design=modern");
        assert!(switch_url(&u("http://tauri.localhost/index.html"), Design::Modern).is_none());
        assert!(switch_url(&u("tauri://localhost/settings.html"), Design::Classic).is_none());
    }
}
