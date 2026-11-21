//! App logo: check and storage, shared for v1 (`PATCH /api/config/{pk}/`, pl-system) and v2
//! (`PUT /rs/api/v2/settings/app_logo`, docs/SETTINGS.md §2.8; pl-api-v2 re-exports this module):
//!
//! upstream: src/paperless/models.py@v3.4.2 (`app_logo`: `FileExtensionValidator(jpg, png, gif,
//! svg)`, `upload_to="logo/"`), src/paperless/serialisers.py (`validate_app_logo`: SVG →
//! `reject_dangerous_svg`, otherwise `validate_raster_image`; JPEG/PNG re-encoded),
//! src/paperless/validators.py (allow lists for SVG tags/attributes, dangerous patterns),
//! django/core/files/storage (`get_valid_filename`, `get_available_name`).
//!
//! Deviation (stricter): SVG with `<!DOCTYPE …>` is rejected (lxml would read it without entities).

use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

const ALLOWED_EXT: [&str; 4] = ["jpg", "png", "gif", "svg"];

const SVG_TAGS: &[&str] = &[
    "svg",
    "g",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textpath",
    "style",
    "defs",
    "symbol",
    "use",
    "marker",
    "pattern",
    "mask",
    "lineargradient",
    "radialgradient",
    "stop",
    "clippath",
    "title",
    "desc",
    "metadata",
];

const SVG_ATTRS: &[&str] = &[
    "id",
    "class",
    "style",
    "x",
    "y",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "width",
    "height",
    "x1",
    "y1",
    "x2",
    "y2",
    "dx",
    "dy",
    "points",
    "d",
    "fill",
    "fill-opacity",
    "fill-rule",
    "color",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "vector-effect",
    "clip-rule",
    "overflow",
    "transform",
    "viewbox",
    "preserveaspectratio",
    "opacity",
    "gradienttransform",
    "gradientunits",
    "spreadmethod",
    "fx",
    "fy",
    "fr",
    "offset",
    "stop-color",
    "stop-opacity",
    "clip-path",
    "mask",
    "marker-start",
    "marker-mid",
    "marker-end",
    "markerunits",
    "markerwidth",
    "markerheight",
    "refx",
    "refy",
    "orient",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "text-decoration",
    "letter-spacing",
    "word-spacing",
    "text-rendering",
    "shape-rendering",
    "image-rendering",
    "startoffset",
    "method",
    "spacing",
    "href",
    "xlink:href",
    "xlink:title",
    "patternunits",
    "patterntransform",
    "patterncontentunits",
    "maskunits",
    "maskcontentunits",
    "xmlns",
    "xmlns:xlink",
    "version",
    "type",
    "aria-label",
    "aria-hidden",
    "role",
    "focusable",
];

const DANGEROUS_STYLE: &[&str] = &[
    "javascript:",
    "data:text/html",
    "expression(",
    "import",
    "@import",
    "-moz-binding:",
    "behaviour:",
    "behavior:",
    "vbscript:",
    "data:application/",
];
const DANGEROUS_SCHEMES: &[&str] = &["javascript:", "data:text/html", "vbscript:", "file:", "data:application/"];
const SAFE_PREFIXES: &[&str] = &["#", "/", "./", "../", "data:image/"];

/// Django `get_valid_filename` (+ Basisname).
pub fn valid_filename(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s: String = base.trim().replace(' ', "_").chars().filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    if s.is_empty() || s == "." || s == ".." {
        return None;
    }
    Some(s)
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Kind {
    Svg,
    Png,
    Jpeg,
    Gif,
    Other,
}

fn sniff(data: &[u8]) -> Kind {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Kind::Png;
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Kind::Jpeg;
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Kind::Gif;
    }
    let head = String::from_utf8_lossy(&data[..data.len().min(2048)]).to_ascii_lowercase();
    let t = head.trim_start_matches('\u{feff}').trim_start();
    if (t.starts_with("<?xml") || t.starts_with("<svg") || t.starts_with("<!--")) && t.contains("<svg") {
        return Kind::Svg;
    }
    Kind::Other
}

/// `reject_dangerous_svg`.
pub fn reject_dangerous_svg(data: &[u8]) -> Result<(), String> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_reader(data);
    let mut buf = Vec::new();
    let mut in_style = 0usize;
    let mut style_text = String::new();
    let mut root_seen = false;
    loop {
        let ev = r.read_event_into(&mut buf).map_err(|_| "Invalid SVG file.".to_string())?;
        match ev {
            Event::Eof => break,
            Event::DocType(_) => return Err("Invalid SVG file.".into()),
            Event::Start(ref e) | Event::Empty(ref e) => {
                root_seen = true;
                let local = String::from_utf8_lossy(e.local_name().as_ref()).to_lowercase();
                if !SVG_TAGS.contains(&local.as_str()) {
                    return Err(format!("Disallowed SVG tag: <{local}>"));
                }
                for a in e.attributes() {
                    let a = a.map_err(|_| "Invalid SVG file.".to_string())?;
                    let raw = String::from_utf8_lossy(a.key.as_ref()).to_string();
                    // lxml does not treat namespace declarations as attributes
                    if raw == "xmlns" || raw.starts_with("xmlns:") {
                        continue;
                    }
                    let name = raw.to_lowercase();
                    let name = name.trim();
                    if !SVG_ATTRS.contains(&name) {
                        return Err(format!("Disallowed SVG attribute: {raw}"));
                    }
                    let value =
                        a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(|_| "Invalid SVG file.".to_string())?.to_lowercase();
                    if name == "style"
                        && let Some(p) = DANGEROUS_STYLE.iter().find(|p| value.contains(*p))
                    {
                        return Err(format!("Disallowed pattern in style attribute: {p}"));
                    }
                    if name == "href" || name == "xlink:href" {
                        let v = value.trim();
                        if let Some(s) = DANGEROUS_SCHEMES.iter().find(|s| v.starts_with(*s)) {
                            return Err(format!("Disallowed URI scheme in {raw}: {s}"));
                        }
                        if !v.is_empty() && !SAFE_PREFIXES.iter().any(|p| v.starts_with(p)) {
                            return Err(format!("URI scheme not allowed in {raw}: must be #anchor, relative path, or data:image/*"));
                        }
                    }
                }
                if local == "style" && matches!(ev, Event::Start(_)) {
                    in_style += 1;
                }
            }
            Event::End(ref e) => {
                if String::from_utf8_lossy(e.local_name().as_ref()).eq_ignore_ascii_case("style") && in_style > 0 {
                    in_style -= 1;
                    let t = style_text.to_lowercase();
                    if let Some(p) = DANGEROUS_STYLE.iter().find(|p| t.contains(*p)) {
                        return Err(format!("Disallowed pattern in <style> content: {p}"));
                    }
                    style_text.clear();
                }
            }
            Event::Text(ref t) if in_style > 0 => style_text.push_str(&String::from_utf8_lossy(t.as_ref())),
            Event::CData(ref t) if in_style > 0 => style_text.push_str(&String::from_utf8_lossy(t.as_ref())),
            _ => {}
        }
        buf.clear();
    }
    if !root_seen {
        return Err("Invalid SVG file.".into());
    }
    Ok(())
}

/// Checks and cleans a logo. Returns: (filename, bytes to store).
pub fn validate(name: &str, data: &[u8], max_pixels: Option<i64>) -> Result<(String, Vec<u8>), String> {
    let clean = valid_filename(name).ok_or_else(|| "Invalid file name.".to_string())?;
    let ext = clean.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if !ALLOWED_EXT.contains(&ext.as_str()) {
        return Err(format!("File extension “{ext}” is not allowed. Allowed extensions are: {}.", ALLOWED_EXT.join(", ")));
    }
    if data.is_empty() {
        return Err("The submitted file is empty.".into());
    }
    match sniff(data) {
        Kind::Svg => {
            reject_dangerous_svg(data)?;
            Ok((clean, data.to_vec()))
        }
        kind => {
            let reader = image::ImageReader::new(Cursor::new(data)).with_guessed_format().map_err(|_| "Invalid logo image.".to_string())?;
            let (w, h) = reader.into_dimensions().map_err(|_| "Invalid logo image.".to_string())?;
            if let Some(m) = max_pixels.filter(|m| *m > 0)
                && i64::from(w) * i64::from(h) > m
            {
                return Err("Uploaded logo exceeds the maximum allowed image size.".into());
            }
            let img = image::load_from_memory(data).map_err(|_| "Invalid logo image.".to_string())?;
            let fmt = match kind {
                Kind::Png => image::ImageFormat::Png,
                Kind::Jpeg => image::ImageFormat::Jpeg,
                // GIF only checked, not re-encoded, like upstream
                Kind::Gif => return Ok((clean, data.to_vec())),
                _ => return Err("Invalid logo image.".into()),
            };
            let mut out = Cursor::new(Vec::new());
            let img = if fmt == image::ImageFormat::Jpeg { image::DynamicImage::ImageRgb8(img.to_rgb8()) } else { img };
            img.write_to(&mut out, fmt).map_err(|_| "Invalid logo image.".to_string())?;
            Ok((clean, out.into_inner()))
        }
    }
}

/// `MEDIA_ROOT/<name>` for a stored FileField value (relative paths without `..` only).
pub fn media_path(media_root: &Path, name: &str) -> Option<PathBuf> {
    let rel = Path::new(name);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(media_root.join(rel))
}

/// Stores under `MEDIA_ROOT/logo/` with a free name (Django `get_available_name`: `_<7 characters>`
/// before the extension). Returns: FileField value `logo/<name>` (≤ 100 characters).
pub async fn store(media_root: &Path, name: &str, bytes: &[u8]) -> std::io::Result<String> {
    use rand::Rng;
    let dir = media_root.join("logo");
    tokio::fs::create_dir_all(&dir).await?;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    let max_stem = 100usize.saturating_sub("logo/".len() + ext.len() + 8);
    let stem: String = stem.chars().take(max_stem).collect();
    let mut candidate = format!("{stem}{ext}");
    for _ in 0..100 {
        let path = dir.join(&candidate);
        match tokio::fs::OpenOptions::new().write(true).create_new(true).open(&path).await {
            Ok(mut f) => {
                use tokio::io::AsyncWriteExt;
                f.write_all(bytes).await?;
                f.flush().await?;
                return Ok(format!("logo/{candidate}"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let suffix: String = rand::thread_rng().sample_iter(&rand::distributions::Alphanumeric).take(7).map(char::from).collect();
                candidate = format!("{stem}_{suffix}{ext}");
            }
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free file name for the logo"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_rules_like_upstream() {
        let ok = br##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10"><defs><linearGradient id="g"><stop offset="0" stop-color="#fff"/></linearGradient></defs><rect width="10" height="10" fill="url(#g)"/><use xlink:href="#g"/><style>.a{fill:red}</style></svg>"##;
        assert_eq!(reject_dangerous_svg(ok), Ok(()));
        assert!(reject_dangerous_svg(br#"<svg><script>alert(1)</script></svg>"#).unwrap_err().contains("<script>"));
        assert!(reject_dangerous_svg(br#"<svg onload="x()"/>"#).unwrap_err().contains("onload"));
        assert!(reject_dangerous_svg(br##"<svg><a href="#x"/></svg>"##).unwrap_err().contains("<a>"));
        assert!(reject_dangerous_svg(br#"<svg><use href="javascript:alert(1)"/></svg>"#).unwrap_err().contains("javascript:"));
        assert!(reject_dangerous_svg(br#"<svg><use href="https://evil/x.svg"/></svg>"#).unwrap_err().contains("not allowed"));
        assert!(reject_dangerous_svg(br#"<svg><style>@import url(x)</style></svg>"#).unwrap_err().contains("<style>"));
        assert!(reject_dangerous_svg(br#"<svg style="behavior: url(x)"/>"#).is_err());
        assert!(reject_dangerous_svg(br#"<!DOCTYPE svg [<!ENTITY a "b">]><svg/>"#).is_err());
        assert!(reject_dangerous_svg(b"not xml <").is_err());
    }

    #[test]
    fn raster_and_names() {
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 3).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();
        let (n, b) = validate("my logo (1).png", &png, None).unwrap();
        assert_eq!(n, "my_logo_1.png");
        assert!(b.starts_with(b"\x89PNG"));
        assert!(validate("x.png", &png, Some(11)).unwrap_err().contains("maximum"));
        assert!(validate("x.png", &png, Some(12)).is_ok());
        assert!(validate("x.jpeg", &png, None).unwrap_err().contains("extension"));
        assert!(validate("x.png", b"\x89PNG\r\n\x1a\ngarbage", None).is_err());
        assert!(validate("x.svg", br#"<svg onload="x"/>"#, None).is_err());
        assert_eq!(valid_filename("../../etc/passwd").as_deref(), Some("passwd"));
        assert!(media_path(Path::new("/m"), "../x").is_none());
        assert!(media_path(Path::new("/m"), "logo/a.png").is_some());
    }

    #[tokio::test]
    async fn store_picks_free_name() {
        let dir = std::env::temp_dir().join(format!("pl_api_v2_logo_{}", std::process::id()));
        let a = store(&dir, "a.png", b"1").await.unwrap();
        let b = store(&dir, "a.png", b"2").await.unwrap();
        assert_eq!(a, "logo/a.png");
        assert!(b.starts_with("logo/a_") && b.ends_with(".png") && b.len() == "logo/a_1234567.png".len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
