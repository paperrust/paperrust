//! PaperRust branding without a frontend fork: only **logo, name and accent colour** change,
//! everything else stays exactly as upstream. Contract: docs/contract/pl-server.md
//! ("Branding"), docs/BRANDING.md ("Umsetzung ohne Fork").
//!
//! - Name/logo use upstream's own hooks `PAPERLESS_APP_TITLE` / `PAPERLESS_APP_LOGO`
//!   (upstream: src/paperless/settings/__init__.py@v3.4.2 APP_TITLE/APP_LOGO,
//!   src/documents/views.py@v3.4.2 UiSettingsView, src/documents/context_processors.py@v3.4.2).
//!   When branding is on and a variable is *unset*, PaperRust's own `pl_core::Config` gets
//!   "PaperRust" / `/__rs/brand/logo.svg` (Rust route `ui_settings.get`). Django reads its own
//!   environment (see docs/OPERATIONS.md).
//! - Accent colour: one `<style id="paperrust-brand">` before `</head>` of HTML navigations
//!   (Angular index and the Django account pages), overriding the CSS variables
//!   `--pngx-primary` / `--pngx-primary-lightness`
//!   (upstream: src-ui/src/theme.scss@v3.4.2, src/documents/static/base.css@v3.4.2).
//!   The per-user theme colour (src-ui/src/app/services/settings.service.ts@v3.4.2
//!   `updateAppearanceSettings`: inline style on `<html>` + body class `primary-light|dark`)
//!   keeps precedence: inline style beats `:root`, and the dark-mode rules are scoped to
//!   `body:not(.primary-light):not(.primary-dark)` exactly like upstream's.
//!   The Light UI under `/rs/app/` has a CSP without `'unsafe-inline'`: there the same CSS comes
//!   as `<link rel="stylesheet" id="paperrust-brand" href="/__rs/brand/accent/v1/<h>-<s>-<l>/<variant>.css">`;
//!   the colour is part of the path, so the file is stateless and immutable (test protocol A-331/D-029).
//! - Own assets under `/__rs/brand/` (embedded from docs/design/assets, favicon.ico rasterised
//!   in Rust from the favicon.svg geometry).
//!
//! `PAPERRUST_BRANDING=off` disables all of it: the server is byte-transparent as before.

use std::sync::LazyLock;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use hyper::body::Body as _;

use crate::config::ConfigError;

pub const BRAND_PREFIX: &str = "/__rs/brand/";
pub const APP_TITLE: &str = "PaperRust";
pub const LOGO_PATH: &str = "/__rs/brand/logo.svg";
pub const FAVICON_ICO_PATH: &str = "/__rs/brand/favicon.ico";
pub const FAVICON_SVG_PATH: &str = "/__rs/brand/favicon.svg";
/// Rust `#B7410E` (docs/design/assets/README.md).
pub const DEFAULT_ACCENT: [u8; 3] = [0xB7, 0x41, 0x0E];
/// WCAG 2.x AA for normal text.
pub const MIN_CONTRAST: f64 = 4.5;
/// Button text in light mode: `--pngx-primary-text-contrast: var(--bs-light)` = `#f8f9fa`
/// (slightly darker than white, so checking it also covers pure white).
pub const LIGHT_BUTTON_TEXT: [u8; 3] = [0xF8, 0xF9, 0xFA];
/// Button text in dark mode: `--bs-body-color` = `$text-color-dark-bg` `#abb2bf` (theme.scss).
pub const DARK_BUTTON_TEXT: [u8; 3] = [0xAB, 0xB2, 0xBF];
/// Accent stylesheets: `/__rs/brand/accent/v1/<h>-<s>-<l>/{app,accounts}.css` (bump `v1` when the
/// generated CSS changes, the files are cached as immutable).
pub const ACCENT_CSS_PREFIX: &str = "/__rs/brand/accent/v1/";
/// The Light UI's index pages reference their assets under this path.
const LIGHT_UI_MARKER: &[u8] = b"\"/rs/app/";
/// HTML bodies larger than this are passed through unchanged.
const MAX_HTML: u64 = 4 * 1024 * 1024;

pub const LOGO_SVG: &[u8] = include_bytes!("../../../docs/design/assets/paperrust-logo.svg");
pub const LOGO_INVERSE_SVG: &[u8] =
    include_bytes!("../../../docs/design/assets/paperrust-logo-inverse.svg");
pub const LOGO_MONO_SVG: &[u8] =
    include_bytes!("../../../docs/design/assets/paperrust-logo-mono.svg");
pub const APP_ICON_SVG: &[u8] =
    include_bytes!("../../../docs/design/assets/paperrust-app-icon.svg");
pub const FAVICON_SVG: &[u8] = include_bytes!("../../../docs/design/assets/favicon.svg");

/// favicon.ico (16/32/48, 32-bit BMP entries), rasterised once from the favicon.svg geometry.
pub static FAVICON_ICO: LazyLock<Vec<u8>> = LazyLock::new(|| ico::build(&[16, 32, 48]));

// ------------------------------------------------------------------ colour

/// Accent colour as written into the CSS (integer H, S%, L%).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accent {
    /// Hue in degrees (0..360).
    pub h: u16,
    /// Saturation in percent.
    pub s: u8,
    /// Lightness in percent, already lowered for >= 4.5:1 against the button text.
    pub l: u8,
    /// Lightness of the given colour before the contrast adjustment.
    pub l_input: u8,
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).ok();
    match s.len() {
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
        6 => Some([
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ]),
        _ => None,
    }
}

/// (h degrees, s 0..1, l 0..1)
pub fn rgb_to_hsl(rgb: [u8; 3]) -> (f64, f64, f64) {
    let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (h, s, l)
}

/// CSS `hsl()` to 8-bit sRGB (as a browser renders it).
pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r, g, b].map(|v| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// WCAG 2.x relative luminance.
pub fn relative_luminance(rgb: [u8; 3]) -> f64 {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = rgb;
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio (>= 1).
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

impl Accent {
    /// Round to integer H/S/L and lower L until the button background has >= 4.5:1 against
    /// the light-mode button text (`#f8f9fa`, therefore also against white).
    pub fn from_rgb(rgb: [u8; 3]) -> Self {
        let (h, s, l) = rgb_to_hsl(rgb);
        let h = (h.round() as u16) % 360;
        let s = (s * 100.0).round().clamp(0.0, 100.0) as u8;
        let l_input = (l * 100.0).round().clamp(0.0, 100.0) as u8;
        let mut a = Accent { h, s, l: l_input, l_input };
        while a.l > 0 && a.contrast_with(LIGHT_BUTTON_TEXT) < MIN_CONTRAST {
            a.l -= 1;
        }
        a
    }

    /// Rendered colour at lightness `l + delta` percent (clamped like CSS).
    pub fn rgb_at(&self, delta: i16) -> [u8; 3] {
        let l = (i16::from(self.l) + delta).clamp(0, 100);
        hsl_to_rgb(
            f64::from(self.h),
            f64::from(self.s) / 100.0,
            f64::from(l) / 100.0,
        )
    }

    pub fn rgb(&self) -> [u8; 3] {
        self.rgb_at(0)
    }

    pub fn contrast_with(&self, text: [u8; 3]) -> f64 {
        contrast(self.rgb(), text)
    }

    fn hex_at(&self, delta: i16) -> String {
        let [r, g, b] = self.rgb_at(delta);
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

// ------------------------------------------------------------------ config

#[derive(Debug, Clone)]
pub struct Branding {
    pub enabled: bool,
    pub accent: Accent,
    css_angular: String,
    css_accounts: String,
    etag_suffix: String,
}

fn invalid(var: &'static str, value: &str, reason: &str) -> ConfigError {
    ConfigError::Invalid {
        var,
        value: value.to_owned(),
        reason: reason.to_owned(),
    }
}

impl Branding {
    pub fn new(enabled: bool, accent_rgb: [u8; 3]) -> Self {
        let accent = Accent::from_rgb(accent_rgb);
        let css_angular = css_angular(&accent);
        let css_accounts = css_accounts(&accent);
        let etag_suffix = format!(
            "-prb{:08x}",
            fnv1a(format!("{css_angular}{css_accounts}").as_bytes())
        );
        Self {
            enabled,
            accent,
            css_angular,
            css_accounts,
            etag_suffix,
        }
    }

    pub fn off() -> Self {
        Self::new(false, DEFAULT_ACCENT)
    }

    /// `PAPERRUST_BRANDING` (on/off, default on), `PAPERRUST_ACCENT` (hex, default `#B7410E`).
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let enabled = match get("PAPERRUST_BRANDING") {
            None => true,
            Some(v) => match v.trim().to_ascii_lowercase().as_str() {
                "" | "on" | "yes" | "true" | "1" | "y" | "t" => true,
                "off" | "no" | "false" | "0" | "n" | "f" => false,
                _ => return Err(invalid("PAPERRUST_BRANDING", &v, "expected on or off")),
            },
        };
        let accent = match get("PAPERRUST_ACCENT") {
            None => DEFAULT_ACCENT,
            Some(v) if v.trim().is_empty() => DEFAULT_ACCENT,
            Some(v) => parse_hex(&v).ok_or_else(|| {
                invalid("PAPERRUST_ACCENT", &v, "expected a hex colour like #B7410E")
            })?,
        };
        Ok(Self::new(enabled, accent))
    }

    /// Name and logo for the Rust `ui_settings.get` route (pl-auth-api reads `pl_core::Config`).
    /// Only *unset* variables are filled: an explicitly empty value keeps upstream's
    /// "no custom branding" meaning.
    pub fn apply_ui_defaults(&self, core: &mut pl_core::Config) {
        if !self.enabled {
            return;
        }
        if core.app_title.is_none() {
            core.app_title = Some(APP_TITLE.to_owned());
        }
        if core.app_logo.is_none() {
            core.app_logo = Some(LOGO_PATH.to_owned());
        }
        // Account pages and mails: "PaperRust sign in", "[PaperRust] …" (pl-accounts).
        core.product_name = Some(pl_core::brand::PRODUCT_NAME.to_owned());
    }

    /// The `<style>` element for a page (Angular index or Django account page).
    pub fn style_element(&self, angular: bool) -> String {
        let css = if angular {
            &self.css_angular
        } else {
            &self.css_accounts
        };
        format!("<style id=\"paperrust-brand\">{css}</style>")
    }

    /// The `<link>` element for pages under a CSP without `'unsafe-inline'` (Light UI).
    pub fn link_element(&self, angular: bool) -> String {
        format!(
            "<link rel=\"stylesheet\" id=\"paperrust-brand\" href=\"{}\">",
            stylesheet_path(&self.accent, angular)
        )
    }

    /// GET navigation to an HTML page of the frontend or the account pages?
    /// API, files, static assets, WebSocket, admin and internal paths never qualify.
    pub fn is_candidate(&self, method: &Method, path: &str, headers: &HeaderMap) -> bool {
        if !self.enabled || method != Method::GET || is_excluded_path(path) {
            return false;
        }
        if headers.contains_key(header::RANGE) || headers.contains_key(header::UPGRADE) {
            return false;
        }
        let get = |n: &str| {
            headers
                .get_all(n)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect::<Vec<_>>()
                .join(",")
                .to_ascii_lowercase()
        };
        let dest = get("sec-fetch-dest");
        if !dest.is_empty() {
            return dest.trim() == "document";
        }
        get("accept").contains("text/html")
    }

    /// Insert the style (exactly once) and point the favicon links to `/__rs/brand/`.
    /// `None`: nothing to do (no `</head>`, or already branded).
    pub fn inject(&self, html: &[u8]) -> Option<Vec<u8>> {
        let head_end = find_ci(html, b"</head>", 0)?;
        if find_ci(html, b"id=\"paperrust-brand\"", 0).is_some() {
            return None;
        }
        let angular = find_ci(html, b"<pngx-root", 0).is_some();
        // Light UI (assets under /rs/app/): CSP `default-src 'self'` forbids inline <style>
        let light_ui = find_ci(&html[..head_end], LIGHT_UI_MARKER, 0).is_some();
        let (head, icon_links) = rewrite_icon_links(&html[..head_end]);
        let mut out = Vec::with_capacity(html.len() + 1024);
        out.extend_from_slice(&head);
        if light_ui {
            out.extend_from_slice(self.link_element(angular).as_bytes());
        } else {
            out.extend_from_slice(self.style_element(angular).as_bytes());
        }
        if icon_links == 0 {
            out.extend_from_slice(
                format!("<link rel=\"icon\" type=\"image/x-icon\" href=\"{FAVICON_ICO_PATH}\">")
                    .as_bytes(),
            );
        }
        out.extend_from_slice(
            format!("<link rel=\"icon\" type=\"image/svg+xml\" href=\"{FAVICON_SVG_PATH}\">")
                .as_bytes(),
        );
        out.extend_from_slice(&html[head_end..]);
        Some(out)
    }

    /// Apply the injection to an uncompressed response (Rust or proxied). Responses that are
    /// not a successful, inline, uncompressed HTML document of known size stay untouched.
    pub async fn inject_response(&self, resp: Response) -> Response {
        if !html_eligible(&resp) {
            return resp;
        }
        let (mut parts, body) = resp.into_parts();
        let content = match BodyExt::collect(body).await {
            Ok(c) => c.to_bytes(),
            Err(e) => {
                tracing::warn!(error = %e, "html body failed before branding");
                let mut r = Response::new(Body::from("Bad Gateway"));
                *r.status_mut() = StatusCode::BAD_GATEWAY;
                r.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; charset=utf-8"),
                );
                return r;
            }
        };
        let Some(new) = self.inject(&content) else {
            return Response::from_parts(parts, Body::from(content));
        };
        parts
            .headers
            .insert(header::CONTENT_LENGTH, HeaderValue::from(new.len()));
        let etag = parts
            .headers
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .and_then(|e| {
                let e = e.trim();
                e.strip_suffix('"')
                    .map(|stem| format!("{stem}{}\"", self.etag_suffix))
            })
            .and_then(|e| HeaderValue::from_str(&e).ok());
        if let Some(e) = etag {
            parts.headers.insert(header::ETAG, e);
        }
        parts.headers.remove("content-md5");
        Response::from_parts(parts, Body::from(new))
    }
}

fn is_excluded_path(path: &str) -> bool {
    const EXACT_OR_DIR: &[&str] = &[
        "/api", "/ws", "/share", "/static", "/assets", "/logo", "/fetch", "/admin", "/media",
        "/__rs",
    ];
    path == "/favicon.ico"
        || EXACT_OR_DIR.iter().any(|p| {
            path.strip_prefix(p)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
}

fn html_eligible(resp: &Response) -> bool {
    let h = resp.headers();
    let ct_html = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.trim().to_ascii_lowercase().starts_with("text/html"));
    let encoded = h
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .any(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"));
    let attachment = h
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("attachment"));
    let size_ok = resp
        .body()
        .size_hint()
        .exact()
        .is_some_and(|n| n <= MAX_HTML);
    resp.status() == StatusCode::OK && ct_html && !encoded && !attachment && size_ok
}

/// Light mode: `:root` (inline style of the per-user theme colour on `<html>` wins).
/// Dark mode: upstream switches the *default* green to lightness 31% on body and keeps the
/// navbar "og green" (theme.scss `[data-bs-theme=dark|auto] body:not(.primary-light):not(.primary-dark)`);
/// the same selectors (same specificity, later in the document) set the accent there, with the
/// same lightness as in light mode (better contrast to the dark-mode button text than +10%).
fn css_angular(a: &Accent) -> String {
    let vars = format!(
        "--pngx-primary:{},{}%;--pngx-primary-lightness:{}%",
        a.h, a.s, a.l
    );
    let b = "body:not(.primary-light):not(.primary-dark)";
    format!(
        ":root{{{vars}}}\
         [data-bs-theme=\"dark\"] {b},[data-bs-theme=\"dark\"] {b} .navbar.bg-primary{{{vars}}}\
         @media (prefers-color-scheme:dark){{[data-bs-theme=\"auto\"] {b},[data-bs-theme=\"auto\"] {b} .navbar.bg-primary,[data-bs-theme=\"auto\"] {b} .navbar.bg-primary .dropdown-menu{{{vars}}}}}"
    )
}

/// Django account pages (base.html + static/base.css): there `--pngx-primary` is a full
/// colour on `body`, hover/active and the button border colours are fixed greens.
fn css_accounts(a: &Accent) -> String {
    let [r, g, bl] = a.rgb();
    let [hr, hg, hb] = a.rgb_at(-5);
    format!(
        "body{{--pngx-primary:{p};--pngx-primary-hover:{h};--pngx-primary-active:{ac};\
         --bs-link-color-rgb:{r}, {g}, {bl};--bs-link-hover-color-rgb:{hr}, {hg}, {hb}}}\
         .btn.btn-primary{{--bs-btn-hover-border-color:{ac};--bs-btn-active-border-color:{b11}}}",
        p = a.hex_at(0),
        h = a.hex_at(-5),
        ac = a.hex_at(-8),
        b11 = a.hex_at(-11),
    )
}

// ------------------------------------------------------------------ html helpers

fn find_ci(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Attribute spans of a start tag: (name, value range in `tag`).
fn attributes(tag: &[u8]) -> Vec<(String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    // skip "<link"
    let mut i = tag.iter().position(|b| b.is_ascii_whitespace()).unwrap_or(tag.len());
    let end = tag.len().saturating_sub(1); // '>'
    while i < end {
        while i < end && (tag[i].is_ascii_whitespace() || tag[i] == b'/') {
            i += 1;
        }
        let ns = i;
        while i < end && !tag[i].is_ascii_whitespace() && tag[i] != b'=' && tag[i] != b'>' {
            i += 1;
        }
        if ns == i {
            i += 1;
            continue;
        }
        let name = String::from_utf8_lossy(&tag[ns..i]).to_ascii_lowercase();
        while i < end && tag[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < end && tag[i] == b'=' {
            i += 1;
            while i < end && tag[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < end && (tag[i] == b'"' || tag[i] == b'\'') {
                let q = tag[i];
                let vs = i + 1;
                let ve = tag[vs..end].iter().position(|&b| b == q).map_or(end, |p| vs + p);
                out.push((name, vs..ve));
                i = ve + 1;
            } else {
                let vs = i;
                while i < end && !tag[i].is_ascii_whitespace() {
                    i += 1;
                }
                out.push((name, vs..i));
            }
        } else {
            out.push((name, i..i));
        }
    }
    out
}

/// Point every `<link rel="… icon …">` (rel token `icon`, e.g. `icon`, `shortcut icon`; not
/// `apple-touch-icon`) to the PaperRust favicon. Returns the new head and the number of links.
fn rewrite_icon_links(head: &[u8]) -> (Vec<u8>, usize) {
    let mut out = Vec::with_capacity(head.len() + 64);
    let mut pos = 0;
    let mut count = 0;
    while let Some(start) = find_ci(head, b"<link", pos) {
        let Some(len) = head[start..].iter().position(|&b| b == b'>') else {
            break;
        };
        let tag = &head[start..=start + len];
        out.extend_from_slice(&head[pos..start]);
        let attrs = attributes(tag);
        let is_icon = attrs.iter().any(|(n, r)| {
            n == "rel"
                && String::from_utf8_lossy(&tag[r.clone()])
                    .split_ascii_whitespace()
                    .any(|t| t.eq_ignore_ascii_case("icon"))
        });
        match attrs.iter().find(|(n, _)| n == "href").filter(|_| is_icon) {
            Some((_, r)) => {
                count += 1;
                out.extend_from_slice(&tag[..r.start]);
                out.extend_from_slice(FAVICON_ICO_PATH.as_bytes());
                out.extend_from_slice(&tag[r.end..]);
            }
            None => out.extend_from_slice(tag),
        }
        pos = start + len + 1;
    }
    out.extend_from_slice(&head[pos..]);
    (out, count)
}

fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |h, &b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

// ------------------------------------------------------------------ assets

/// Embedded asset for a `/__rs/brand/<name>` path.
pub fn asset(path: &str) -> Option<(&'static [u8], &'static str)> {
    let name = path.strip_prefix(BRAND_PREFIX)?;
    let svg = "image/svg+xml";
    Some(match name {
        "logo.svg" => (LOGO_SVG, svg),
        "logo-inverse.svg" => (LOGO_INVERSE_SVG, svg),
        "logo-mono.svg" => (LOGO_MONO_SVG, svg),
        "app-icon.svg" => (APP_ICON_SVG, svg),
        "favicon.svg" => (FAVICON_SVG, svg),
        "favicon.ico" => (FAVICON_ICO.as_slice(), "image/x-icon"),
        _ => return None,
    })
}

/// `GET|HEAD /__rs/brand/<asset>` (any peer; only when branding is on – otherwise the path
/// answers 404 like every other unknown `/__rs/` path).
pub fn asset_response(method: &Method, path: &str) -> Option<Response> {
    if method != Method::GET && method != Method::HEAD {
        return None;
    }
    if let Some(css) = accent_stylesheet(path) {
        return Some(stylesheet_response(method, css));
    }
    let (bytes, ct) = asset(path)?;
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    let mut r = Response::new(body);
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"),
    );
    h.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{:08x}\"", fnv1a(bytes)))
            .unwrap_or_else(|_| HeaderValue::from_static("\"0\"")),
    );
    Some(r)
}

/// Path of the accent stylesheet (the colour is encoded in the path → stateless, immutable).
pub fn stylesheet_path(a: &Accent, angular: bool) -> String {
    let variant = if angular { "app" } else { "accounts" };
    format!("{ACCENT_CSS_PREFIX}{}-{}-{}/{variant}.css", a.h, a.s, a.l)
}

/// CSS for `/__rs/brand/accent/v1/<h>-<s>-<l>/{app,accounts}.css`; `None` for anything else
/// (out-of-range or non-numeric values included).
pub fn accent_stylesheet(path: &str) -> Option<String> {
    let rest = path.strip_prefix(ACCENT_CSS_PREFIX)?;
    let (hsl, file) = rest.split_once('/')?;
    let mut it = hsl.split('-');
    let num = |v: Option<&str>| -> Option<u16> {
        let v = v?;
        if v.is_empty() || v.len() > 3 || !v.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        v.parse().ok()
    };
    let (h, sat, l) = (num(it.next())?, num(it.next())?, num(it.next())?);
    if it.next().is_some() || h >= 360 || sat > 100 || l > 100 {
        return None;
    }
    let a = Accent {
        h,
        s: u8::try_from(sat).ok()?,
        l: u8::try_from(l).ok()?,
        l_input: u8::try_from(l).ok()?,
    };
    match file {
        "app.css" => Some(css_angular(&a)),
        "accounts.css" => Some(css_accounts(&a)),
        _ => None,
    }
}

fn stylesheet_response(method: &Method, css: String) -> Response {
    let len = css.len();
    let etag = format!("\"{:08x}\"", fnv1a(css.as_bytes()));
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(css)
    };
    let mut r = Response::new(body);
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"0\"")),
    );
    r
}

/// Pure-Rust rasteriser for the three shapes of `favicon.svg` (viewBox 0 0 32 32) and an ICO
/// writer (32-bit BMP entries with alpha, empty AND mask). The geometry is checked against
/// the SVG source in the tests, so a changed favicon.svg fails the build's tests.
pub mod ico {
    pub const ROUND_RECT: (f64, f64, f64, f64, f64) = (1.0, 1.0, 30.0, 30.0, 7.0);
    pub const PAPER_SVG: &str = "M9 6 H19 L23 10 V25 Q23 26 22 26 H10 Q9 26 9 25 Z";
    pub const CORNER_SVG: &str = "M19 6 V10 H23 Z";
    pub const IRON: [u8; 3] = [0x1F, 0x1A, 0x17];
    pub const PAPER: [u8; 3] = [0xF4, 0xEF, 0xE6];
    pub const CORNER: [u8; 3] = [0xC2, 0x55, 0x1A];

    fn quad(p0: (f64, f64), c: (f64, f64), p1: (f64, f64), out: &mut Vec<(f64, f64)>) {
        for i in 1..=8 {
            let t = f64::from(i) / 8.0;
            let u = 1.0 - t;
            out.push((
                u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
                u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
            ));
        }
    }

    fn paper() -> Vec<(f64, f64)> {
        let mut p = vec![(9.0, 6.0), (19.0, 6.0), (23.0, 10.0), (23.0, 25.0)];
        quad((23.0, 25.0), (23.0, 26.0), (22.0, 26.0), &mut p);
        p.push((10.0, 26.0));
        quad((10.0, 26.0), (9.0, 26.0), (9.0, 25.0), &mut p);
        p
    }

    fn corner() -> Vec<(f64, f64)> {
        vec![(19.0, 6.0), (19.0, 10.0), (23.0, 10.0)]
    }

    fn in_poly(poly: &[(f64, f64)], x: f64, y: f64) -> bool {
        let mut inside = false;
        let mut j = poly.len() - 1;
        for i in 0..poly.len() {
            let (xi, yi) = poly[i];
            let (xj, yj) = poly[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    fn in_round_rect(x: f64, y: f64) -> bool {
        let (rx, ry, w, h, r) = ROUND_RECT;
        if x < rx || y < ry || x > rx + w || y > ry + h {
            return false;
        }
        let cx = x.clamp(rx + r, rx + w - r);
        let cy = y.clamp(ry + r, ry + h - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    }

    /// RGBA pixels, top-down rows, 4x4 supersampling.
    pub fn render(n: u32) -> Vec<[u8; 4]> {
        let (paper, corner) = (paper(), corner());
        let scale = 32.0 / f64::from(n);
        let mut px = Vec::with_capacity((n * n) as usize);
        for py in 0..n {
            for pxx in 0..n {
                let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
                for sy in 0..4 {
                    for sx in 0..4 {
                        let x = (f64::from(pxx) + (f64::from(sx) + 0.5) / 4.0) * scale;
                        let y = (f64::from(py) + (f64::from(sy) + 0.5) / 4.0) * scale;
                        let c = if in_poly(&corner, x, y) {
                            Some(CORNER)
                        } else if in_poly(&paper, x, y) {
                            Some(PAPER)
                        } else if in_round_rect(x, y) {
                            Some(IRON)
                        } else {
                            None
                        };
                        if let Some([cr, cg, cb]) = c {
                            r += f64::from(cr);
                            g += f64::from(cg);
                            b += f64::from(cb);
                            a += 1.0;
                        }
                    }
                }
                if a == 0.0 {
                    px.push([0, 0, 0, 0]);
                } else {
                    px.push([
                        (r / a).round() as u8,
                        (g / a).round() as u8,
                        (b / a).round() as u8,
                        (a / 16.0 * 255.0).round() as u8,
                    ]);
                }
            }
        }
        px
    }

    fn bmp_entry(n: u32) -> Vec<u8> {
        let px = render(n);
        let mask_row = n.div_ceil(32) * 4;
        let mut d = Vec::new();
        for v in [40u32, n, 2 * n] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&1u16.to_le_bytes()); // planes
        d.extend_from_slice(&32u16.to_le_bytes()); // bpp
        d.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        d.extend_from_slice(&(n * n * 4 + mask_row * n).to_le_bytes());
        d.extend_from_slice(&[0u8; 16]);
        for y in (0..n).rev() {
            for x in 0..n {
                let [r, g, b, a] = px[(y * n + x) as usize];
                d.extend_from_slice(&[b, g, r, a]);
            }
        }
        d.extend(std::iter::repeat_n(0u8, (mask_row * n) as usize));
        d
    }

    pub fn build(sizes: &[u32]) -> Vec<u8> {
        let entries: Vec<Vec<u8>> = sizes.iter().map(|&n| bmp_entry(n)).collect();
        let mut out = Vec::new();
        out.extend_from_slice(&[0, 0, 1, 0]);
        out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * sizes.len() as u32;
        for (&n, e) in sizes.iter().zip(&entries) {
            let dim = if n >= 256 { 0 } else { n as u8 };
            out.extend_from_slice(&[dim, dim, 0, 0]);
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            out.extend_from_slice(&(e.len() as u32).to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            offset += e.len() as u32;
        }
        for e in entries {
            out.extend_from_slice(&e);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_accent_contrast() {
        let a = Accent::from_rgb(DEFAULT_ACCENT);
        assert_eq!((a.h, a.s, a.l_input), (18, 86, 39));
        assert!(a.contrast_with([255, 255, 255]) >= MIN_CONTRAST);
        assert!(a.contrast_with(LIGHT_BUTTON_TEXT) >= MIN_CONTRAST);
        // #B7410E already passes: lightness is not lowered.
        assert_eq!(a.l, a.l_input);
        // Hover/active are darker, i.e. even more contrast.
        assert!(contrast(a.rgb_at(-5), LIGHT_BUTTON_TEXT) > a.contrast_with(LIGHT_BUTTON_TEXT));
    }

    #[test]
    fn light_accent_gets_darkened() {
        let a = Accent::from_rgb(parse_hex("#E07A3F").unwrap());
        assert!(a.l < a.l_input);
        assert!(a.contrast_with(LIGHT_BUTTON_TEXT) >= MIN_CONTRAST);
        // The step before would not have passed (smallest change).
        let prev = Accent { l: a.l + 1, ..a };
        assert!(prev.contrast_with(LIGHT_BUTTON_TEXT) < MIN_CONTRAST);
        let y = Accent::from_rgb(parse_hex("ffff00").unwrap());
        assert!(y.contrast_with(LIGHT_BUTTON_TEXT) >= MIN_CONTRAST);
    }

    #[test]
    fn contrast_reference_values() {
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 1e-9);
        assert!((contrast([255, 255, 255], [255, 255, 255]) - 1.0).abs() < 1e-9);
        // upstream green #17541f against #f8f9fa
        assert!(contrast([0x17, 0x54, 0x1f], LIGHT_BUTTON_TEXT) > 8.0);
        assert_eq!(hsl_to_rgb(128.0, 0.57, 0.21), [0x17, 0x54, 0x1f]);
    }

    #[test]
    fn parse_colours() {
        assert_eq!(parse_hex("#B7410E"), Some([0xB7, 0x41, 0x0E]));
        assert_eq!(parse_hex("b7410e"), Some([0xB7, 0x41, 0x0E]));
        assert_eq!(parse_hex("#fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("#ggg"), None);
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("rot"), None);
    }

    #[test]
    fn config_from_env() {
        let b = Branding::from_lookup(&|_| None).unwrap();
        assert!(b.enabled);
        assert_eq!(b.accent, Accent::from_rgb(DEFAULT_ACCENT));
        let off = Branding::from_lookup(&|k| (k == "PAPERRUST_BRANDING").then(|| "OFF".into()))
            .unwrap();
        assert!(!off.enabled);
        assert!(Branding::from_lookup(&|k| (k == "PAPERRUST_BRANDING").then(|| "vielleicht".into())).is_err());
        assert!(Branding::from_lookup(&|k| (k == "PAPERRUST_ACCENT").then(|| "#zz0000".into())).is_err());
        let blue = Branding::from_lookup(&|k| (k == "PAPERRUST_ACCENT").then(|| "#0d6efd".into()))
            .unwrap();
        assert_eq!(blue.accent.h, 216);
    }

    #[test]
    fn ui_defaults_only_when_unset() {
        let mut core = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([(
            "PAPERLESS_SECRET_KEY",
            "x",
        )]))
        .unwrap();
        Branding::off().apply_ui_defaults(&mut core);
        assert_eq!(core.app_title, None);
        Branding::new(true, DEFAULT_ACCENT).apply_ui_defaults(&mut core);
        assert_eq!(core.app_title.as_deref(), Some("PaperRust"));
        assert_eq!(core.app_logo.as_deref(), Some("/__rs/brand/logo.svg"));
        core.app_title = Some(String::new());
        core.app_logo = Some("/logo/x.png".into());
        Branding::new(true, DEFAULT_ACCENT).apply_ui_defaults(&mut core);
        assert_eq!(core.app_title.as_deref(), Some(""));
        assert_eq!(core.app_logo.as_deref(), Some("/logo/x.png"));
    }

    fn hdr(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn candidates_only_html_navigation() {
        let b = Branding::new(true, DEFAULT_ACCENT);
        let nav = hdr(&[("accept", "text/html,application/xhtml+xml,*/*;q=0.8")]);
        let fetch = hdr(&[("accept", "text/html"), ("sec-fetch-dest", "empty")]);
        let doc = hdr(&[("sec-fetch-dest", "document"), ("accept", "*/*")]);
        let json = hdr(&[("accept", "application/json")]);
        assert!(b.is_candidate(&Method::GET, "/", &nav));
        assert!(b.is_candidate(&Method::GET, "/dashboard", &doc));
        assert!(b.is_candidate(&Method::GET, "/accounts/login/", &nav));
        assert!(!b.is_candidate(&Method::GET, "/", &fetch));
        assert!(!b.is_candidate(&Method::GET, "/", &json));
        assert!(!b.is_candidate(&Method::POST, "/accounts/login/", &nav));
        assert!(!b.is_candidate(&Method::HEAD, "/", &nav));
        for p in [
            "/api/", "/api", "/api/documents/", "/share/abc", "/static/x.css", "/logo/", "/logo",
            "/ws/status/", "/admin/", "/favicon.ico", "/__rs/health", "/fetch/doc/1", "/assets/x",
        ] {
            assert!(!b.is_candidate(&Method::GET, p, &nav), "{p}");
        }
        assert!(b.is_candidate(&Method::GET, "/logs", &nav));
        let ranged = hdr(&[("accept", "text/html"), ("range", "bytes=0-1")]);
        assert!(!b.is_candidate(&Method::GET, "/", &ranged));
        assert!(!Branding::off().is_candidate(&Method::GET, "/", &nav));
    }

    const ANGULAR: &str = "<!doctype html>\n<html lang=\"en\" data-bs-theme=\"auto\">\n<head>\n\t<meta charset=\"utf-8\">\n\t<title>Paperless-ngx</title>\n\t<base href=\"/\">\n\t<link rel=\"icon\" type=\"image/x-icon\" href=\"favicon.ico\">\n\t<link rel=\"manifest\" href=\"/static/frontend/en-US/manifest.webmanifest\" crossorigin=\"use-credentials\">\n\t<link rel=\"stylesheet\" href=\"/static/frontend/en-US/styles.css\">\n\t<link rel=\"apple-touch-icon\" href=\"/static/frontend/en-US/apple-touch-icon.png\">\n</head>\n<body>\n\t<pngx-root></pngx-root>\n</body>\n</html>\n";

    #[test]
    fn inject_angular_once() {
        let b = Branding::new(true, DEFAULT_ACCENT);
        let out = String::from_utf8(b.inject(ANGULAR.as_bytes()).unwrap()).unwrap();
        assert_eq!(out.matches("id=\"paperrust-brand\"").count(), 1);
        assert!(out.contains(":root{--pngx-primary:18,86%;--pngx-primary-lightness:39%}"));
        let style = out.find("<style id=\"paperrust-brand\">").unwrap();
        assert!(style > out.find("styles.css").unwrap(), "after the stylesheet");
        assert!(style < out.find("</head>").unwrap());
        assert!(out.contains("<link rel=\"icon\" type=\"image/x-icon\" href=\"/__rs/brand/favicon.ico\">"));
        assert!(!out.contains("href=\"favicon.ico\""));
        assert!(out.contains("apple-touch-icon.png"), "apple-touch-icon unchanged");
        assert!(out.contains("/static/frontend/en-US/manifest.webmanifest"));
        // dark-mode rules are scoped like upstream: user theme colour keeps precedence
        assert!(out.contains("[data-bs-theme=\"dark\"] body:not(.primary-light):not(.primary-dark) .navbar.bg-primary"));
        // body after </head> untouched
        let tail = &ANGULAR[ANGULAR.find("</head>").unwrap()..];
        assert!(out.ends_with(tail));
        // second pass: nothing (exactly once)
        assert!(b.inject(out.as_bytes()).is_none());
    }

    #[test]
    fn inject_accounts_page() {
        let b = Branding::new(true, DEFAULT_ACCENT);
        let page = "<html lang=\"en\"><HEAD><title>Login</title><link href=\"/static/base.css\" rel=\"stylesheet\"></HEAD><body class=\"text-center\">x</body></html>";
        let out = String::from_utf8(b.inject(page.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("body{--pngx-primary:#b9410e;"));
        assert!(!out.contains(":root{"));
        assert!(out.contains("<link rel=\"icon\" type=\"image/x-icon\" href=\"/__rs/brand/favicon.ico\">"));
        assert!(out.contains("</HEAD><body"));
        assert!(b.inject(b"<html><body>no head</body></html>").is_none());
    }

    #[test]
    fn inject_light_ui_as_stylesheet_link() {
        // Light UI (CSP default-src 'self' without 'unsafe-inline'): <link>, no inline <style> (A-331)
        let b = Branding::new(true, DEFAULT_ACCENT);
        let page = "<!doctype html><html><head><meta charset=\"utf-8\"><link rel=\"icon\" href=\"/favicon.ico\"><script type=\"module\" crossorigin src=\"/rs/app/assets/index-1.js\"></script><link rel=\"stylesheet\" crossorigin href=\"/rs/app/assets/index-1.css\"></head><body><pngx-root></pngx-root></body></html>";
        let out = String::from_utf8(b.inject(page.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("<style"));
        let link = "<link rel=\"stylesheet\" id=\"paperrust-brand\" href=\"/__rs/brand/accent/v1/18-86-39/app.css\">";
        assert_eq!(out.matches(link).count(), 1, "{out}");
        assert!(out.find(link).unwrap() > out.find("index-1.css").unwrap(), "after the app stylesheet");
        assert!(b.inject(out.as_bytes()).is_none());
        // the linked file has exactly the CSS the inline variant would carry
        assert_eq!(accent_stylesheet("/__rs/brand/accent/v1/18-86-39/app.css").unwrap(), b.css_angular);
        assert_eq!(accent_stylesheet("/__rs/brand/accent/v1/18-86-39/accounts.css").unwrap(), b.css_accounts);
    }

    #[test]
    fn accent_stylesheet_route() {
        for bad in [
            "/__rs/brand/accent/v1/360-50-50/app.css",
            "/__rs/brand/accent/v1/10-101-50/app.css",
            "/__rs/brand/accent/v1/10-50-50-1/app.css",
            "/__rs/brand/accent/v1/10-50/app.css",
            "/__rs/brand/accent/v1/a-50-50/app.css",
            "/__rs/brand/accent/v1/+1-50-50/app.css",
            "/__rs/brand/accent/v1/10-50-50/x.css",
            "/__rs/brand/accent/v1/10-50-50/app.css/x",
            "/__rs/brand/accent/v2/10-50-50/app.css",
        ] {
            assert!(accent_stylesheet(bad).is_none(), "{bad}");
        }
        let r = asset_response(&Method::GET, "/__rs/brand/accent/v1/18-86-39/app.css").unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()[header::CONTENT_TYPE], "text/css; charset=utf-8");
        assert!(r.headers()[header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));
        let head = asset_response(&Method::HEAD, "/__rs/brand/accent/v1/18-86-39/app.css").unwrap();
        assert_eq!(head.headers()[header::CONTENT_LENGTH], r.headers()[header::CONTENT_LENGTH]);
        assert!(asset_response(&Method::POST, "/__rs/brand/accent/v1/18-86-39/app.css").is_none());
    }

    #[test]
    fn icon_link_variants() {
        let (h, n) = rewrite_icon_links(b"<link href='a.ico' rel='shortcut icon'><LINK REL=icon HREF=b.ico /><link rel=\"apple-touch-icon\" href=\"c.png\">");
        assert_eq!(n, 2);
        let h = String::from_utf8(h).unwrap();
        assert_eq!(
            h,
            "<link href='/__rs/brand/favicon.ico' rel='shortcut icon'><LINK REL=icon HREF=/__rs/brand/favicon.ico /><link rel=\"apple-touch-icon\" href=\"c.png\">"
        );
    }

    #[test]
    fn assets_and_ico() {
        let svg = std::str::from_utf8(FAVICON_SVG).unwrap();
        assert!(svg.contains(ico::PAPER_SVG) && svg.contains(ico::CORNER_SVG));
        assert!(svg.contains("x=\"1\" y=\"1\" width=\"30\" height=\"30\" rx=\"7\" fill=\"#1F1A17\""));
        assert!(svg.contains("#F4EFE6") && svg.contains("#C2551A"));
        let ico = FAVICON_ICO.as_slice();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 3, 0]);
        let sizes: Vec<u8> = (0..3).map(|i| ico[6 + 16 * i]).collect();
        assert_eq!(sizes, vec![16, 32, 48]);
        let px = ico::render(32);
        assert_eq!(px[0][3], 0, "corner transparent");
        assert_eq!(px[16 * 32 + 16], [0xF4, 0xEF, 0xE6, 255], "centre is paper");
        assert_eq!(px[8 * 32 + 20], [0xC2, 0x55, 0x1A, 255], "folded corner rust");
        assert_eq!(px[16 * 32 + 3], [0x1F, 0x1A, 0x17, 255], "iron frame");
        for p in ["logo.svg", "logo-inverse.svg", "logo-mono.svg", "app-icon.svg", "favicon.svg", "favicon.ico"] {
            assert!(asset(&format!("/__rs/brand/{p}")).is_some(), "{p}");
        }
        assert!(asset("/__rs/brand/../health").is_none());
    }
}
