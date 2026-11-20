//! Gateway pages for mode 2b: light UI under `/rs/app/` (from the packed
//! `frontend-light/dist`, SPA fallback) and an own login page under `/accounts/login/`
//! (user/password/2FA → `/local/login` → device token in the keychain). The light UI redirects
//! on 401 to `/accounts/login/?next=…` – in the window everything thus stays under the prapi origin, the
//! login page of the server is never loaded.

use std::path::Path;

use crate::gateway::{GwRequest, GwResponse};

pub const PREFIX: &str = "/rs/app";

/// CSP of the own pages (as pl-server `/rs/app/`).
pub const CSP: &str = "default-src 'self'; img-src 'self' blob: data:; worker-src 'self' blob:";

fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "bcmap" | "pfb" => "application/octet-stream",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn page(status: u16, ct: &str, cache: &str, body: Vec<u8>) -> GwResponse {
    GwResponse {
        status,
        headers: vec![
            ("content-type".into(), ct.into()),
            ("cache-control".into(), cache.into()),
            ("content-security-policy".into(), CSP.into()),
            ("x-content-type-options".into(), "nosniff".into()),
            ("x-paperrust-source".into(), "local".into()),
        ],
        body,
    }
}

/// Windows device name (`CON`, `nul.txt`, `COM1` …), like `pl_core::safepath::is_windows_device_name`.
fn is_device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        s => {
            let mut it = s.chars();
            let head: String = it.by_ref().take(3).collect();
            let tail: Vec<char> = it.collect();
            (head == "COM" || head == "LPT")
                && matches!(tail.as_slice(), [c] if c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
        }
    }
}

/// Resolve the decoded app path relative to `dir` (security review 2026-11-19 F8); rules like
/// `pl_core::safepath::safe_join`, here strict on all platforms: `/` and `\` separate, no
/// absolute path/drive/UNC, no `..`, no `:` (stream), no NUL, no device names, no
/// dot/space at the end. `None` = escape or not allowed → 404.
/// Own version, because pl-sync-client deliberately does without pl-core (axum/sqlx).
fn app_file(dir: &Path, rel: &str) -> Option<std::path::PathBuf> {
    if rel.contains('\0') || rel.starts_with(['/', '\\']) {
        return None;
    }
    let mut p = dir.to_path_buf();
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            x => {
                if x.contains(':') || x.ends_with(['.', ' ']) || is_device_name(x) {
                    return None;
                }
                let mut comps = Path::new(x).components();
                match (comps.next(), comps.next()) {
                    (Some(std::path::Component::Normal(_)), None) => p.push(x),
                    _ => return None,
                }
            }
        }
    }
    Some(p)
}

/// `/rs/app/…` from `dir` (without `..`); unknown paths outside `assets/` → `index.html`.
pub async fn serve_app(dir: Option<&Path>, req: &GwRequest) -> GwResponse {
    let Some(dir) = dir else {
        return GwResponse::error(404, "no_app", "The app UI is not installed (light/ missing).", "local");
    };
    if req.path == PREFIX {
        return redirect("/rs/app/");
    }
    if !matches!(req.method.to_ascii_uppercase().as_str(), "GET" | "HEAD") {
        return GwResponse::error(405, "method_not_allowed", "Method not allowed.", "local");
    }
    let rel = req.path.trim_start_matches("/rs/app/");
    let decoded = percent_encoding::percent_decode_str(rel).decode_utf8_lossy().to_string();
    // Security review 2026-11-19 F8: only via app_file (before, `/rs/app//etc/passwd` or
    // on Windows `%5CWindows%5Cwin.ini`/UNC led out of the app folder via `dir.join(absolute)`)
    let file = if decoded.is_empty() { None } else { app_file(dir, &decoded) };
    let safe = decoded.is_empty() || file.is_some();
    if let Some(p) = file
        && p.is_file()
        && let Ok(b) = tokio::fs::read(&p).await
    {
        let cache = if decoded.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        return page(200, content_type(&decoded), cache, b);
    }
    if decoded.starts_with("assets/") || !safe {
        return GwResponse::error(404, "not_found", "Not found.", "local");
    }
    match tokio::fs::read(dir.join("index.html")).await {
        Ok(b) => page(200, "text/html; charset=utf-8", "no-cache", b),
        Err(_) => GwResponse::error(404, "no_app", "The app UI is not installed.", "local"),
    }
}

/// Paths of the classic (Angular) UI that the light UI links to (`/documents/12/notes`,
/// `/settings`, `/mail` …): everything that is neither app nor API nor core function.
pub fn is_classic_path(path: &str) -> bool {
    const RESERVED: [&str; 6] = ["/rs/", "/api/", "/local/", "/accounts/", "/ws/", "/static/"];
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy().to_ascii_lowercase();
    path.starts_with('/')
        && path != PREFIX
        && !RESERVED.iter().any(|r| decoded.starts_with(r))
        && !decoded.contains("//")
        && !decoded.contains('\\')
        && !decoded.split('/').any(|s| s == ".." || s == "." || s.contains(':'))
}

/// CSP of the notice page (no script, only embedded CSS).
const CLASSIC_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'";

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// Notice page for links into the classic UI (mode 2b). `server_url` = target on the server,
/// when online (the link opens the system browser, because the server origin is not
/// allowed in the app window); `None` = offline → "only available online" (503).
///
/// Deliberately **no** passing through of the page: the classic UI is an Angular app with
/// a session cookie; via prapi, server HTML would only run with `CSP: sandbox` without scripts (empty) and
/// without login (cookies are discarded).
pub fn classic_page(server_url: Option<&str>) -> GwResponse {
    let body = match server_url {
        Some(u) => format!(
            "<h1>Klassische Oberfläche</h1>\
             <p>Diese Ansicht gibt es nur in der klassischen Oberfläche des Servers. Sie öffnet sich im \
             Browser (dort ggf. anmelden).</p>\
             <p lang=\"en\">This view is only available in the classic interface; it opens in your browser.</p>\
             <p><a class=\"btn\" href=\"{0}\">Im Browser öffnen</a> <a href=\"/rs/app/\">Zurück zur App</a></p>",
            html_escape(u)
        ),
        None => "<h1>Nur online verfügbar</h1>\
                 <p>Diese Ansicht gibt es nur in der klassischen Oberfläche des Servers. Der Server ist gerade \
                 nicht erreichbar (oder Sie sind abgemeldet) – im Offline-Bestand gibt es sie nicht.</p>\
                 <p lang=\"en\">Only available online: this view needs the server's classic interface.</p>\
                 <p><a class=\"btn\" href=\"/rs/app/\">Zurück zur App</a></p>"
            .to_string(),
    };
    let html = format!(
        "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>PaperRust</title>\
         <style>{LOGIN_CSS}a{{color:#b7410e}}.btn{{display:inline-block;background:#b7410e;color:#fff;\
         padding:.5rem 1rem;border-radius:.3rem;text-decoration:none;margin-right:1rem}}</style>\
         </head><body><main>{body}</main></body></html>"
    );
    GwResponse {
        status: if server_url.is_some() { 200 } else { 503 },
        headers: vec![
            ("content-type".into(), "text/html; charset=utf-8".into()),
            ("cache-control".into(), "no-store".into()),
            ("content-security-policy".into(), CLASSIC_CSP.into()),
            ("x-content-type-options".into(), "nosniff".into()),
            ("x-paperrust-source".into(), "local".into()),
        ],
        body: html.into_bytes(),
    }
}

pub fn redirect(to: &str) -> GwResponse {
    GwResponse {
        status: 302,
        headers: vec![("location".into(), to.into()), ("cache-control".into(), "no-store".into()), ("x-paperrust-source".into(), "local".into())],
        body: vec![],
    }
}

/// Like `encodeURIComponent`.
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'(' | b')' => (c as char).to_string(),
            _ => format!("%{c:02X}"),
        })
        .collect()
}

/// Return target only within the app (no foreign host, no `//`).
pub fn safe_next(next: &str) -> &str {
    if next.starts_with("/rs/app/") && !next.contains("//") && !next.contains('\\') { next } else { "/rs/app/" }
}

pub fn attachment(name: &str) -> String {
    format!("attachment; filename*=UTF-8''{}", encode(name))
}

const LOGIN_HTML: &str = r#"<!doctype html>
<html lang="de">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>PaperRust – Anmelden</title>
<link rel="stylesheet" href="/accounts/login.css">
</head>
<body>
<main>
  <h1>PaperRust</h1>
  <p id="hint">Anmelden am Server. Das Passwort wird nicht gespeichert; diese App erhält ein eigenes, widerrufbares Geräte-Token.</p>
  <form id="f">
    <label>Benutzername <input id="u" name="username" autocomplete="username" required autofocus></label>
    <label>Passwort <input id="p" name="password" type="password" autocomplete="current-password" required></label>
    <label id="code-row" hidden>Code (Zwei-Faktor) <input id="c" name="code" inputmode="numeric" autocomplete="one-time-code"></label>
    <p id="err" role="alert"></p>
    <button type="submit" id="go">Anmelden</button>
  </form>
</main>
<script src="/accounts/login.js"></script>
</body>
</html>
"#;

const LOGIN_CSS: &str = "body{font-family:system-ui,sans-serif;background:#f4f4f4;margin:0}main{max-width:22rem;margin:10vh auto;background:#fff;padding:2rem;border-radius:.5rem;box-shadow:0 1px 4px #0002}h1{color:#b7410e;margin-top:0}label{display:block;margin:.8rem 0;font-size:.9rem}input{display:block;width:100%;box-sizing:border-box;padding:.5rem;margin-top:.25rem}button{background:#b7410e;color:#fff;border:0;padding:.6rem 1.2rem;border-radius:.3rem;cursor:pointer}#err{color:#b00020;min-height:1.2em}#hint{color:#555;font-size:.85rem}@media (prefers-color-scheme:dark){body{background:#1e1e1e;color:#eee}main{background:#2a2a2a}#hint{color:#aaa}}";

const LOGIN_JS: &str = r#""use strict";
(function () {
  const $ = (id) => document.getElementById(id);
  const q = new URLSearchParams(location.search);
  let next = q.get("next") || "/rs/app/";
  if (!next.startsWith("/rs/app/") || next.includes("//")) next = "/rs/app/";
  $("f").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    $("err").textContent = "";
    $("go").disabled = true;
    const body = { username: $("u").value, password: $("p").value, code: $("c").value || null };
    try {
      const r = await fetch("/local/login", { method: "POST", headers: { "Content-Type": "application/json", Accept: "application/json" }, body: JSON.stringify(body) });
      if (r.ok) { location.replace(next); return; }
      const d = await r.json().catch(() => ({}));
      const msg = (d.error && d.error.message) || ("HTTP " + r.status);
      if (/MFA|code/i.test(msg)) $("code-row").hidden = false;
      $("err").textContent = r.status === 503 ? "Server nicht erreichbar." : msg;
    } catch (e) {
      $("err").textContent = String(e);
    } finally {
      $("p").value = "";
      $("go").disabled = false;
    }
  });
})();
"#;

pub fn login_page() -> GwResponse {
    page(200, "text/html; charset=utf-8", "no-store", LOGIN_HTML.as_bytes().to_vec())
}

pub fn login_js() -> GwResponse {
    page(200, "text/javascript; charset=utf-8", "no-cache", LOGIN_JS.as_bytes().to_vec())
}

pub fn login_css() -> GwResponse {
    page(200, "text/css; charset=utf-8", "no-cache", LOGIN_CSS.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> GwRequest {
        GwRequest { method: "GET".into(), path: path.into(), ..Default::default() }
    }

    #[tokio::test]
    async fn serves_app_with_spa_fallback() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("assets")).unwrap();
        std::fs::write(d.path().join("index.html"), "<html>app</html>").unwrap();
        std::fs::write(d.path().join("assets/a-1.js"), "x").unwrap();
        let r = serve_app(Some(d.path()), &get("/rs/app/assets/a-1.js")).await;
        assert_eq!(r.status, 200);
        assert!(r.header("cache-control").unwrap().contains("immutable"));
        assert_eq!(r.header("content-type"), Some("text/javascript; charset=utf-8"));
        let r = serve_app(Some(d.path()), &get("/rs/app/documents/12/details")).await;
        assert_eq!(r.body, b"<html>app</html>");
        assert_eq!(r.header("content-security-policy"), Some(CSP));
        assert_eq!(serve_app(Some(d.path()), &get("/rs/app/assets/missing.js")).await.status, 404);
        assert_eq!(serve_app(Some(d.path()), &get("/rs/app/..%2F..%2Fsecret")).await.status, 404);
        assert_eq!(serve_app(Some(d.path()), &get("/rs/app")).await.status, 302);
        assert_eq!(serve_app(None, &get("/rs/app/")).await.status, 404);
    }

    /// Security review 2026-11-19 F8: after percent decoding, no absolute path, no
    /// drive, no Windows root path (`%5C…`) and no UNC path may lead out of the app folder.
    #[tokio::test]
    async fn app_paths_never_leave_app_dir() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("light");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("index.html"), "<html>app</html>").unwrap();
        let secret = t.path().join("secret.txt");
        std::fs::write(&secret, "geheim").unwrap();
        let abs = std::path::absolute(&secret).unwrap().to_string_lossy().into_owned();
        let enc = |s: &str| s.replace('\\', "%5C").replace(':', "%3A");
        let mut paths = vec![
            format!("/rs/app/{abs}"),
            format!("/rs/app/{}", enc(&abs)),
            "/rs/app//etc/passwd".to_string(),
            "/rs/app/%2Fetc%2Fpasswd".to_string(),
            "/rs/app/%5CWindows%5Cwin.ini".to_string(),
            "/rs/app/%5C%5Clocalhost%5Cc$%5CWindows%5Cwin.ini".to_string(),
            "/rs/app/C%3A%5CWindows%5Cwin.ini".to_string(),
            "/rs/app/..%5Csecret.txt".to_string(),
            "/rs/app/index.html%3A%3A%24DATA".to_string(),
            "/rs/app/CON".to_string(),
            "/rs/app/nul.txt".to_string(),
            "/rs/app/index.html.".to_string(),
            "/rs/app/a%00b".to_string(),
        ];
        // Windows root path without drive (same drive as the app folder)
        if let Some((_, rest)) = abs.split_once(':') {
            paths.push(format!("/rs/app/{}", enc(rest)));
        }
        for p in &paths {
            let r = serve_app(Some(&app), &get(p)).await;
            assert_eq!(r.status, 404, "{p}");
            assert_ne!(r.body, b"geheim", "{p}");
        }
        // normal paths still allowed
        assert_eq!(serve_app(Some(&app), &get("/rs/app/index.html")).await.status, 200);
        assert_eq!(serve_app(Some(&app), &get("/rs/app/documents//12")).await.body, b"<html>app</html>");
    }

    #[test]
    fn classic_paths() {
        for p in ["/documents/12/details", "/documents", "/settings", "/mail", "/", "/saved-views/3", "/documents%2012"] {
            assert!(is_classic_path(p), "{p}");
        }
        for p in ["/rs/app/", "/rs/app", "/rs/api/v2/x", "/api/documents/", "/local/status", "/accounts/login/", "/ws/status/",
                  "/%61pi/token/", "/a/../api/", "//evil.example", "/x\\y", "/c:/x", "documents"] {
            assert!(!is_classic_path(p), "{p}");
        }
    }

    #[test]
    fn classic_page_online_links_to_server_offline_explains() {
        let r = classic_page(Some("http://127.0.0.1:8100/documents/1/notes?x=\"<y>"));
        assert_eq!(r.status, 200);
        let b = String::from_utf8(r.body.clone()).unwrap();
        assert!(b.contains("href=\"http://127.0.0.1:8100/documents/1/notes?x=&quot;&lt;y&gt;\""));
        assert!(!b.contains("<script"));
        assert!(r.header("content-security-policy").unwrap().starts_with("default-src 'none'"));
        let r = classic_page(None);
        assert_eq!(r.status, 503);
        assert!(String::from_utf8(r.body).unwrap().contains("Nur online verfügbar"));
    }

    #[test]
    fn next_is_kept_inside_the_app() {
        assert_eq!(safe_next("/rs/app/documents?x=1"), "/rs/app/documents?x=1");
        assert_eq!(safe_next("https://evil.example/"), "/rs/app/");
        assert_eq!(safe_next("/rs/app//evil.example"), "/rs/app/");
        assert_eq!(encode("/rs/app/?a=1"), "%2Frs%2Fapp%2F%3Fa%3D1");
    }
}
