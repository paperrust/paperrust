// upstream: src/documents/models.py@v3.4.2 (Document.__str__, get_public_filename, file_type)
//           src/documents/parsers.py@v3.4.2 (get_default_file_extension)
//           src/paperless/parsers/{text,tesseract,mail,tika,remote}.py@v3.4.2 (_SUPPORTED_MIME_TYPES)
//           src/documents/views.py@v3.4.2 (serve_file: Content-Disposition)
//           pathvalidate 3.3.1 (_filename.py::FileNameSanitizer, platform "universal")
//           django 5.2 (utils/http.py::content_disposition_header)

use unicode_normalization::UnicodeNormalization;

pub const MAX_PUBLIC_FILENAME_BYTES: usize = 255;

/// `get_default_file_extension(mime_type)`.
///
/// Parser tables first (registry order does not matter: the tables do not overlap except
/// for `image/*`, where the remote parser wins with score 20 when configured), then
/// Python's built-in `mimetypes.guess_extension` table (strict, defaults only).
pub fn default_file_extension(mime_type: &str, remote_ocr_active: bool) -> &'static str {
    if remote_ocr_active && let Some(e) = lookup(REMOTE_TYPES, mime_type) {
        return e;
    }
    for table in [TEXT_TYPES, TESSERACT_TYPES, MAIL_TYPES, TIKA_TYPES] {
        if let Some(e) = lookup(table, mime_type) {
            return e;
        }
    }
    lookup(PY_MIMETYPES, mime_type).unwrap_or("")
}

fn lookup(table: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

const TEXT_TYPES: &[(&str, &str)] = &[
    ("text/plain", ".txt"),
    ("text/csv", ".csv"),
    ("application/csv", ".csv"),
];
const TESSERACT_TYPES: &[(&str, &str)] = &[
    ("application/pdf", ".pdf"),
    ("image/jpeg", ".jpg"),
    ("image/png", ".png"),
    ("image/tiff", ".tif"),
    ("image/gif", ".gif"),
    ("image/bmp", ".bmp"),
    ("image/webp", ".webp"),
    ("image/heic", ".heic"),
];
const REMOTE_TYPES: &[(&str, &str)] = &[
    ("application/pdf", ".pdf"),
    ("image/png", ".png"),
    ("image/jpeg", ".jpg"),
    ("image/tiff", ".tiff"),
    ("image/bmp", ".bmp"),
    ("image/gif", ".gif"),
    ("image/webp", ".webp"),
];
const MAIL_TYPES: &[(&str, &str)] = &[("message/rfc822", ".eml")];
// The tika parser only scores when tika is enabled; when it is disabled Python falls back to
// mimetypes, which (with Debian's /etc/mime.types) yields the same extensions.
const TIKA_TYPES: &[(&str, &str)] = &[
    ("application/msword", ".doc"),
    ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", ".docx"),
    ("application/vnd.ms-excel", ".xls"),
    ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", ".xlsx"),
    ("application/vnd.ms-powerpoint", ".ppt"),
    ("application/vnd.openxmlformats-officedocument.presentationml.presentation", ".pptx"),
    ("application/vnd.openxmlformats-officedocument.presentationml.slideshow", ".ppsx"),
    ("application/vnd.oasis.opendocument.presentation", ".odp"),
    ("application/vnd.oasis.opendocument.spreadsheet", ".ods"),
    ("application/vnd.oasis.opendocument.text", ".odt"),
    ("application/vnd.oasis.opendocument.graphics", ".odg"),
    ("text/rtf", ".rtf"),
];
// Generated from CPython 3.12 `mimetypes.MimeTypes(filenames=()).guess_extension(t)`.
const PY_MIMETYPES: &[(&str, &str)] = &[
    ("application/json", ".json"), ("application/manifest+json", ".webmanifest"),
    ("application/msword", ".doc"), ("application/n-quads", ".nq"),
    ("application/n-triples", ".nt"), ("application/octet-stream", ".bin"),
    ("application/oda", ".oda"), ("application/pdf", ".pdf"),
    ("application/pkcs7-mime", ".p7c"), ("application/postscript", ".ps"),
    ("application/trig", ".trig"), ("application/vnd.apple.mpegurl", ".m3u"),
    ("application/vnd.ms-excel", ".xls"), ("application/vnd.ms-powerpoint", ".ppt"),
    ("application/wasm", ".wasm"), ("application/x-bcpio", ".bcpio"),
    ("application/x-cpio", ".cpio"), ("application/x-csh", ".csh"),
    ("application/x-dvi", ".dvi"), ("application/x-gtar", ".gtar"),
    ("application/x-hdf", ".hdf"), ("application/x-hdf5", ".h5"),
    ("application/x-latex", ".latex"), ("application/x-mif", ".mif"),
    ("application/x-netcdf", ".cdf"), ("application/x-pkcs12", ".p12"),
    ("application/x-pn-realaudio", ".ram"), ("application/x-python-code", ".pyc"),
    ("application/x-sh", ".sh"), ("application/x-shar", ".shar"),
    ("application/x-shockwave-flash", ".swf"), ("application/x-sv4cpio", ".sv4cpio"),
    ("application/x-sv4crc", ".sv4crc"), ("application/x-tar", ".tar"),
    ("application/x-tcl", ".tcl"), ("application/x-tex", ".tex"),
    ("application/x-texinfo", ".texi"), ("application/x-troff", ".roff"),
    ("application/x-troff-man", ".man"), ("application/x-troff-me", ".me"),
    ("application/x-troff-ms", ".ms"), ("application/x-ustar", ".ustar"),
    ("application/x-wais-source", ".src"), ("application/xml", ".xsl"),
    ("application/zip", ".zip"), ("audio/3gpp", ".3gp"), ("audio/3gpp2", ".3g2"),
    ("audio/aac", ".aac"), ("audio/basic", ".au"), ("audio/mpeg", ".mp3"),
    ("audio/opus", ".opus"), ("audio/x-aiff", ".aif"), ("audio/x-pn-realaudio", ".ra"),
    ("audio/x-wav", ".wav"), ("image/avif", ".avif"), ("image/bmp", ".bmp"),
    ("image/gif", ".gif"), ("image/heic", ".heic"), ("image/heif", ".heif"),
    ("image/ief", ".ief"), ("image/jpeg", ".jpg"), ("image/png", ".png"),
    ("image/svg+xml", ".svg"), ("image/tiff", ".tiff"), ("image/vnd.microsoft.icon", ".ico"),
    ("image/x-cmu-raster", ".ras"), ("image/x-portable-anymap", ".pnm"),
    ("image/x-portable-bitmap", ".pbm"), ("image/x-portable-graymap", ".pgm"),
    ("image/x-portable-pixmap", ".ppm"), ("image/x-rgb", ".rgb"),
    ("image/x-xbitmap", ".xbm"), ("image/x-xpixmap", ".xpm"),
    ("image/x-xwindowdump", ".xwd"), ("message/rfc822", ".eml"), ("text/css", ".css"),
    ("text/csv", ".csv"), ("text/html", ".html"), ("text/javascript", ".js"),
    ("text/markdown", ".md"), ("text/n3", ".n3"), ("text/plain", ".txt"),
    ("text/richtext", ".rtx"), ("text/tab-separated-values", ".tsv"), ("text/vtt", ".vtt"),
    ("text/x-python", ".py"), ("text/x-rst", ".rst"), ("text/x-setext", ".etx"),
    ("text/x-sgml", ".sgm"), ("text/x-vcard", ".vcf"), ("text/xml", ".xml"),
    ("video/mp4", ".mp4"), ("video/mpeg", ".mpeg"), ("video/quicktime", ".mov"),
    ("video/webm", ".webm"), ("video/x-msvideo", ".avi"), ("video/x-sgi-movie", ".movie"),
];

/// `Document.__str__` of the context (root) document.
pub fn document_str(created: chrono::NaiveDate, correspondent: Option<&str>, title: &str) -> String {
    let mut res = created.format("%Y-%m-%d").to_string();
    if let Some(c) = correspondent {
        res.push(' ');
        res.push_str(c);
    }
    if !title.is_empty() {
        res.push(' ');
        res.push_str(title);
    }
    res
}

/// Truncate UTF-8 to at most `max` bytes, dropping a trailing partial character
/// (Python: `s.encode()[:max].decode(errors="ignore")`).
pub fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `Document.get_public_filename(archive=..., counter=...)`.
pub fn public_filename(doc_str: &str, extension: &str, counter: u32) -> String {
    let tail = if counter != 0 {
        format!("_{counter:02}{extension}")
    } else {
        extension.to_string()
    };
    let head_bytes = MAX_PUBLIC_FILENAME_BYTES.saturating_sub(tail.len());
    let head = truncate_utf8(doc_str, head_bytes);
    sanitize_filename(&format!("{head}{tail}"), "-", MAX_PUBLIC_FILENAME_BYTES)
}

fn is_invalid_universal_char(c: char) -> bool {
    let u = c as u32;
    // unprintable ASCII (not in string.printable): 0x00-0x08, 0x0e-0x1f, 0x7f
    (u <= 0x08 || (0x0e..=0x1f).contains(&u) || u == 0x7f)
        || matches!(
            c,
            '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' | '\\'
        )
}

fn is_reserved(name: &str) -> bool {
    let up = name.to_uppercase();
    const BASE: &[&str] = &["CON", "PRN", "AUX", "CLOCK$", "NUL", ":"];
    if BASE.contains(&up.as_str()) {
        return true;
    }
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = up.strip_prefix(prefix) {
            let mut chars = rest.chars();
            if let (Some(c), None) = (chars.next(), chars.next())
                && (c.is_ascii_digit() || matches!(c, '\u{00B9}' | '\u{00B2}' | '\u{00B3}'))
            {
                return true;
            }
        }
    }
    false
}

/// pathvalidate `BaseValidator.__extract_root_name`.
fn root_name(path: &str) -> String {
    if path == "." || path == ".." {
        return path.to_string();
    }
    if path.starts_with("...") {
        return path.to_string();
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    // re.match(r"([^\.]+)") – must match at position 0
    let root: String = base.chars().take_while(|c| *c != '.').collect();
    root
}

/// `pathvalidate.sanitize_filename(value, replacement_text, max_len)` on the universal platform.
pub fn sanitize_filename(value: &str, replacement: &str, max_len: usize) -> String {
    // validate_pathtype(allow_whitespaces=False): empty / whitespace-only -> null handler ""
    if value.trim().is_empty() {
        return String::new();
    }
    let mut s = String::with_capacity(value.len());
    for c in value.chars() {
        if is_invalid_universal_char(c) {
            s.push_str(replacement);
        } else {
            s.push(c);
        }
    }
    let s = truncate_utf8(&s, max_len).to_string();

    // One validation pass; only the first error is handled.
    if s.trim().is_empty() {
        return String::new(); // NULL_NAME -> return_null_string
    }
    // FOUND_ABS_PATH / length errors cannot occur after sanitising (no '/', '\\', ':').
    let root = root_name(&s);
    let base = s.rsplit('/').next().unwrap_or(&s).to_string();
    if is_reserved(&root) || is_reserved(&base) {
        if root == "." || root == ".." {
            return s;
        }
        let replacement_word = format!("{root}_");
        if root.is_empty() {
            return s;
        }
        return s.replace(&root, &replacement_word);
    }
    // universal invalid chars already removed; windows rules:
    if s == "." || s == ".." {
        return s;
    }
    let last = s.chars().last();
    let first = s.chars().next();
    if matches!(last, Some(' ') | Some('.')) || first == Some(' ') {
        let mut t = s.trim_start_matches(' ').trim_end_matches(' ').to_string();
        if t != "." && t != ".." {
            t = t.trim_end_matches([' ', '.']).to_string();
        }
        return t;
    }
    s
}

/// `urllib.parse.quote(s)` with the default `safe="/"`.
pub fn url_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b;
        if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-' | b'~' | b'/') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// `serve_file` Content-Disposition (RFC 5987 form with ASCII fallback).
pub fn serve_file_disposition(disposition: &str, filename: &str) -> String {
    let replaced = filename.replace(',', "_");
    let normalized: String = replaced
        .nfkd()
        .filter(|c| c.is_ascii())
        .collect::<String>()
        .replace(['\\', '"'], "_");
    format!(
        "{disposition}; filename=\"{normalized}\"; filename*=utf-8''{}",
        url_quote(filename)
    )
}

/// Django `content_disposition_header(as_attachment, filename)` as used by `FileResponse`.
pub fn django_content_disposition(as_attachment: bool, filename: &str) -> Option<String> {
    if !filename.is_empty() {
        let disposition = if as_attachment { "attachment" } else { "inline" };
        let quotable = filename
            .chars()
            .all(|c| c == '\t' || (' '..='~').contains(&c));
        let expr = if quotable {
            format!(
                "filename=\"{}\"",
                filename.replace('\\', "\\\\").replace('"', "\\\"")
            )
        } else {
            format!("filename*=utf-8''{}", url_quote(filename))
        };
        Some(format!("{disposition}; {expr}"))
    } else if as_attachment {
        Some("attachment".to_string())
    } else {
        None
    }
}

/// `SharedLinkView` bundle disposition (note the single quotes, as upstream).
pub fn bundle_disposition(download_name: &str) -> String {
    let normalized: String = download_name.nfkd().filter(|c| c.is_ascii()).collect();
    format!(
        "attachment; filename='{normalized}'; filename*=utf-8''{}",
        url_quote(download_name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn upstream_injection_test() {
        // test_api_documents.py::test_download_filename_normalization_does_not_inject_parameters
        let s = document_str(
            NaiveDate::from_ymd_opt(2020, 1, 2).unwrap(),
            None,
            "file.doc\u{ff02}; x=\u{ff02}\u{ff3c}",
        );
        let name = public_filename(&s, ".pdf", 0);
        assert_eq!(
            serve_file_disposition("attachment", &name),
            "attachment; filename=\"2020-01-02 file.doc_; x=__.pdf\"; \
             filename*=utf-8''2020-01-02%20file.doc%EF%BC%82%3B%20x%3D%EF%BC%82%EF%BC%BC.pdf"
        );
    }

    #[test]
    fn public_names() {
        let d = NaiveDate::from_ymd_opt(2021, 1, 1).unwrap();
        let s = document_str(d, None, "document A");
        assert_eq!(public_filename(&s, ".pdf", 0), "2021-01-01 document A.pdf");
        assert_eq!(public_filename(&s, ".pdf", 1), "2021-01-01 document A_01.pdf");
        let s = document_str(d, Some("ACME"), "a/b:c");
        assert_eq!(public_filename(&s, ".pdf", 0), "2021-01-01 ACME a-b-c.pdf");
        let s = document_str(d, None, "");
        assert_eq!(public_filename(&s, ".pdf", 12), "2021-01-01_12.pdf");
    }

    #[test]
    fn long_title_is_cut_before_tail() {
        let d = NaiveDate::from_ymd_opt(2021, 1, 1).unwrap();
        let title = "ä".repeat(300);
        let s = document_str(d, None, &title);
        let n = public_filename(&s, ".pdf", 3);
        assert!(n.len() <= 255);
        assert!(n.ends_with("_03.pdf"));
    }

    #[test]
    fn sanitize_rules() {
        assert_eq!(sanitize_filename("con.txt", "-", 255), "con_.txt");
        assert_eq!(sanitize_filename("abc. ", "-", 255), "abc");
        assert_eq!(sanitize_filename(" abc", "-", 255), "abc");
        assert_eq!(sanitize_filename("   ", "-", 255), "");
        assert_eq!(sanitize_filename("a\u{0}b", "-", 255), "a-b");
    }

    #[test]
    fn quote_and_django_cd() {
        assert_eq!(url_quote("a b/ü~"), "a%20b/%C3%BC~");
        assert_eq!(
            django_content_disposition(false, "0000001.webp").unwrap(),
            "inline; filename=\"0000001.webp\""
        );
        assert_eq!(
            django_content_disposition(true, "ä.png").unwrap(),
            "attachment; filename*=utf-8''%C3%A4.png"
        );
        assert_eq!(
            bundle_disposition("paperless-share-abc.zip"),
            "attachment; filename='paperless-share-abc.zip'; filename*=utf-8''paperless-share-abc.zip"
        );
    }

    #[test]
    fn extensions() {
        assert_eq!(default_file_extension("image/tiff", false), ".tif");
        assert_eq!(default_file_extension("image/tiff", true), ".tiff");
        assert_eq!(default_file_extension("application/pdf", false), ".pdf");
        assert_eq!(default_file_extension("application/x-unknown", false), "");
    }
}
