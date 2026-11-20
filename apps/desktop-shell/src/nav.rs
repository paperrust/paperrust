//! Navigation allow list by exact origin (scheme + host + port, REMOTE-APP §5).
//!
//! - own pages (`tauri://localhost`, `http(s)://tauri.localhost`) and `about:` always;
//! - mode 1: exactly `http://127.0.0.1:<app port>`;
//! - mode 2a: the profile's origin + login origins confirmed by the user (SSO/OIDC);
//! - everything else is opened by the system browser – except a redirect **from the login page**
//!   of the server (`…/accounts/…`) to a foreign https origin: the shell asks about that
//!   ("Anmeldung über <Origin> erlauben?"), because OIDC/SSO otherwise would not work in the window.

use tauri::Url;

pub fn is_local_page(u: &Url) -> bool {
    // Own pages: tauri://localhost (macOS/Linux), http(s)://tauri.localhost (Windows).
    u.scheme() == "tauri" || u.host_str() == Some("tauri.localhost")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NavPolicy {
    origins: Vec<String>,
    /// Base path of the server (login page = `<base>accounts/`), mode 2a only.
    login_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavDecision {
    Allow,
    /// Open in the system browser (http/https/mailto) or discard.
    External,
    /// Foreign login origin: ask the user.
    AskSso(String),
}

impl NavPolicy {
    /// Nothing allowed yet (own pages only).
    pub fn none() -> Self {
        Self::default()
    }

    pub fn local(port: u16) -> Self {
        if port == 0 {
            return Self::none();
        }
        Self {
            origins: vec![format!("http://127.0.0.1:{port}")],
            login_prefix: None,
        }
    }

    /// `base` = profile address (`https://host[:port]/[path/]`), `extra` = confirmed SSO origins.
    pub fn remote(base: &str, extra: &[String]) -> Self {
        let Ok(b) = Url::parse(base) else {
            return Self::none();
        };
        let mut origins = vec![b.origin().ascii_serialization()];
        origins.extend(extra.iter().cloned());
        Self {
            origins,
            login_prefix: Some(format!("{}accounts/", b.as_str())),
        }
    }

    pub fn allows(&self, u: &Url) -> bool {
        if is_local_page(u) || u.scheme() == "about" {
            return true;
        }
        matches!(u.scheme(), "http" | "https")
            && self
                .origins
                .iter()
                .any(|o| *o == u.origin().ascii_serialization())
    }

    /// `current` = page that the window is currently showing (source of the navigation).
    pub fn decide(&self, current: Option<&Url>, target: &Url) -> NavDecision {
        if self.allows(target) {
            return NavDecision::Allow;
        }
        if let (Some(prefix), Some(cur)) = (&self.login_prefix, current)
            && target.scheme() == "https"
            && target.host_str().is_some()
            && cur.as_str().starts_with(prefix.as_str())
        {
            return NavDecision::AskSso(target.origin().ascii_serialization());
        }
        NavDecision::External
    }
}

/// Mode 1, automatic login: is `u` the login page of the local server
/// (`http://127.0.0.1:<port>/accounts/login/` or `/api/auth/login/`)? Then target after
/// login: `next` of the page if relative (`/…`, no `//`, no `\`), otherwise `/`. The server
/// checks `next` once more (pl-accounts `desktop::safe_next`).
pub fn local_login_next(u: &Url, port: u16) -> Option<String> {
    if port == 0
        || u.scheme() != "http"
        || u.host_str() != Some("127.0.0.1")
        || u.port() != Some(port)
        || !matches!(u.path(), "/accounts/login/" | "/api/auth/login/")
    {
        return None;
    }
    let next = u
        .query_pairs()
        .find(|(k, _)| k == "next")
        .map(|(_, v)| v.into_owned())
        .filter(|n| n.starts_with('/') && !n.starts_with("//") && !n.contains('\\'))
        .unwrap_or_else(|| "/".into());
    Some(next)
}

/// Loop protection for automatic login (e.g. when the session does not hold): at most
/// [`ReloginGuard::MAX`] automatic logins per [`ReloginGuard::WINDOW`].
#[derive(Debug, Default)]
pub struct ReloginGuard {
    recent: Vec<std::time::Instant>,
}

impl ReloginGuard {
    pub const MAX: usize = 3;
    pub const WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

    pub fn allow(&mut self, now: std::time::Instant) -> bool {
        self.recent.retain(|t| now.duration_since(*t) < Self::WINDOW);
        if self.recent.len() >= Self::MAX {
            return false;
        }
        self.recent.push(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_page_detection() {
        let n = |s: &str| local_login_next(&u(s), 58_080);
        assert_eq!(n("http://127.0.0.1:58080/accounts/login/").as_deref(), Some("/"));
        assert_eq!(
            n("http://127.0.0.1:58080/accounts/login/?next=%2Frs%2Fapp%2Fdocuments%3Ftags%3D1").as_deref(),
            Some("/rs/app/documents?tags=1")
        );
        assert_eq!(n("http://127.0.0.1:58080/api/auth/login/?next=/x").as_deref(), Some("/x"));
        // foreign target in next → start page
        assert_eq!(n("http://127.0.0.1:58080/accounts/login/?next=https://evil.example/").as_deref(), Some("/"));
        assert_eq!(n("http://127.0.0.1:58080/accounts/login/?next=//evil.example/").as_deref(), Some("/"));
        assert_eq!(n("http://127.0.0.1:58080/accounts/login/?next=/%5Cevil").as_deref(), Some("/"));
        // only the login page, only the own port/host
        assert!(n("http://127.0.0.1:58080/accounts/logout/").is_none());
        assert!(n("http://127.0.0.1:58080/rs/app/").is_none());
        assert!(n("http://127.0.0.1:58081/accounts/login/").is_none());
        assert!(n("http://localhost:58080/accounts/login/").is_none());
        assert!(n("https://127.0.0.1:58080/accounts/login/").is_none());
        assert!(local_login_next(&u("http://127.0.0.1:58080/accounts/login/"), 0).is_none());
    }

    #[test]
    fn relogin_guard_limits_loops() {
        let mut g = ReloginGuard::default();
        let t0 = std::time::Instant::now();
        assert!(g.allow(t0));
        assert!(g.allow(t0));
        assert!(g.allow(t0));
        assert!(!g.allow(t0));
        assert!(g.allow(t0 + ReloginGuard::WINDOW));
    }

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn own_pages_always() {
        for p in [
            NavPolicy::none(),
            NavPolicy::local(58_080),
            NavPolicy::remote("https://a.example/", &[]),
        ] {
            assert!(p.allows(&u("http://tauri.localhost/index.html")));
            assert!(p.allows(&u("https://tauri.localhost/settings.html")));
            assert!(p.allows(&u("tauri://localhost/index.html")));
            assert!(p.allows(&u("about:blank")));
        }
    }

    #[test]
    fn local_mode_is_exactly_app_port() {
        // as before P1 (mode 1 unchanged)
        let p = NavPolicy::local(58_080);
        assert!(p.allows(&u("http://127.0.0.1:58080/dashboard")));
        assert!(!NavPolicy::local(0).allows(&u("http://127.0.0.1:58080/")));
        assert!(!p.allows(&u("http://127.0.0.1:5432/")));
        assert!(!p.allows(&u("http://localhost:58080/")));
        assert!(!p.allows(&u("https://127.0.0.1:58080/")));
        assert!(!p.allows(&u("https://docs.paperless-ngx.com/")));
        assert_eq!(
            p.decide(
                Some(&u("http://127.0.0.1:58080/accounts/login/")),
                &u("https://idp.example/")
            ),
            NavDecision::External
        );
    }

    #[test]
    fn remote_mode_exact_origin() {
        let p = NavPolicy::remote("https://docs.example.ch/", &[]);
        assert!(p.allows(&u("https://docs.example.ch/dashboard")));
        assert!(p.allows(&u("https://docs.example.ch:443/x")));
        // same host, different port/scheme → rejected
        assert!(!p.allows(&u("https://docs.example.ch:8443/")));
        assert!(!p.allows(&u("http://docs.example.ch/")));
        assert!(!p.allows(&u("https://evil.docs.example.ch/")));
        assert!(!p.allows(&u("https://docs.example.ch.evil.com/")));
        assert!(!p.allows(&u("http://127.0.0.1:58080/")));
        assert!(!p.allows(&u("file:///C:/Windows/")));
        // subpath: origin counts
        let p = NavPolicy::remote("https://example.ch/paperless/", &[]);
        assert!(p.allows(&u("https://example.ch/paperless/documents")));
        assert_eq!(p.origins, ["https://example.ch".to_string()]);
    }

    #[test]
    fn sso_only_after_confirmation() {
        let login = u("https://docs.example.ch/accounts/oidc/keycloak/login/?process=login");
        let idp = u("https://sso.example.ch/realms/x/auth?client_id=1");
        let p = NavPolicy::remote("https://docs.example.ch/", &[]);
        assert!(!p.allows(&idp));
        assert_eq!(
            p.decide(Some(&login), &idp),
            NavDecision::AskSso("https://sso.example.ch".into())
        );
        // not from the login page (e.g. link in a document) → system browser
        assert_eq!(
            p.decide(
                Some(&u("https://docs.example.ch/documents/1/details")),
                &idp
            ),
            NavDecision::External
        );
        assert_eq!(p.decide(None, &idp), NavDecision::External);
        // never http IdP
        assert_eq!(
            p.decide(Some(&login), &u("http://sso.example.ch/")),
            NavDecision::External
        );
        // allowed after confirmation
        let p = NavPolicy::remote(
            "https://docs.example.ch/",
            &["https://sso.example.ch".into()],
        );
        assert_eq!(p.decide(Some(&login), &idp), NavDecision::Allow);
        assert!(!p.allows(&u("https://sso.example.ch:8443/")));
    }
}
