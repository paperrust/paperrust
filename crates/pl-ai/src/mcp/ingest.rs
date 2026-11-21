//! `add_document`: file/text/URL → normal processing (`POST /api/documents/post_document/`,
//! consumer task with workflows, OCR, assignment, LLM index update after import).
//!
//! Security:
//! * Size: at most `max_upload_bytes` (decoded); base64 is measured by length
//!   even before decoding.
//! * File type: detected from the content ("magic bytes"), not from the name or the agent's claim; only the
//!   types of the normal upload (PDF, images, text/CSV, Office/ODF – the latter only with Tika on
//!   the server, otherwise the server rejects). The server additionally checks itself (libmagic).
//! * URL fetch only with `PAPERRUST_AI_MCP_ALLOW_URL_FETCH=yes`, http(s) only, public
//!   addresses only (same check as for LLM endpoints: [`crate::llm::validate_and_pin`], address
//!   pinned against DNS rebinding), redirects checked individually (at most 3), response
//!   size-limited, time limit.

use std::time::Duration;

use base64::Engine as _;

/// Detected type: (MIME, file extension).
pub type Sniffed = (&'static str, &'static str);

const OOXML: [(&str, &str); 3] = [
    ("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ("xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    ("pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
];
const ODF: [(&str, &str); 3] = [
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
];
const OLE: [(&str, &str); 3] =
    [("doc", "application/msword"), ("xls", "application/vnd.ms-excel"), ("ppt", "application/vnd.ms-powerpoint")];

fn ext_of(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Detect the type from the content; `name` only helps with container formats (ZIP/OLE) and CSV.
/// `None` = not supported.
pub fn sniff(bytes: &[u8], name: &str) -> Option<Sniffed> {
    let ext = ext_of(name);
    let starts = |p: &[u8]| bytes.starts_with(p);
    if starts(b"%PDF-") {
        return Some(("application/pdf", "pdf"));
    }
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some(("image/png", "png"));
    }
    if starts(b"\xFF\xD8\xFF") {
        return Some(("image/jpeg", "jpg"));
    }
    if starts(b"II*\0") || starts(b"MM\0*") {
        return Some(("image/tiff", "tiff"));
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some(("image/gif", "gif"));
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(("image/webp", "webp"));
    }
    if starts(b"PK\x03\x04") {
        // ODF: entry "mimetype" uncompressed at position 30
        if bytes.len() > 38 && &bytes[30..38] == b"mimetype" {
            for (e, m) in ODF {
                if bytes.windows(m.len()).take(200).any(|w| w == m.as_bytes()) {
                    return Some((m, e));
                }
            }
        }
        return OOXML.iter().chain(ODF.iter()).find(|(e, _)| *e == ext).map(|(e, m)| (*m, *e));
    }
    if starts(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
        return OLE.iter().find(|(e, _)| *e == ext).map(|(e, m)| (*m, *e));
    }
    // Text: valid UTF-8 without NUL/control characters except tab/line break/form feed
    let text = std::str::from_utf8(bytes).ok()?;
    if text.is_empty() || text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\x0c')) {
        return None;
    }
    if ext == "csv" {
        return Some(("text/csv", "csv"));
    }
    Some(("text/plain", if ext == "md" || ext == "markdown" { "md" } else { "txt" }))
}

/// File name without path, only harmless characters, at most 100 characters, with a matching extension.
pub fn safe_filename(name: &str, ext: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = match base.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s,
        _ => base,
    };
    let mut clean: String =
        stem.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')' | ',') { c } else { '_' }).collect();
    clean = clean.trim().trim_matches('.').to_string();
    if clean.is_empty() {
        clean = "document".into();
    }
    let clean: String = clean.chars().take(100).collect();
    format!("{clean}.{ext}")
}

/// Decode base64 with a size check upfront. Whitespace is ignored; `data:…;base64,` allowed.
pub fn decode_base64(s: &str, max: usize) -> Result<Vec<u8>, String> {
    let s = match s.find(";base64,") {
        Some(i) if s.starts_with("data:") => &s[i + 8..],
        _ => s,
    };
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() / 4 * 3 > max + 3 {
        return Err(format!("file too large: at most {max} bytes"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(compact.as_bytes()))
        .map_err(|_| "file_base64 is not valid base64".to_string())?;
    if bytes.len() > max {
        return Err(format!("file too large: at most {max} bytes"));
    }
    if bytes.is_empty() {
        return Err("file is empty".into());
    }
    Ok(bytes)
}

/// Result of a URL fetch.
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub final_url: String,
}

/// Fetches a public http(s) URL (SSRF protection as above). `allow_internal` for tests only.
pub async fn fetch_url(url: &str, max: usize, allow_internal: bool) -> Result<Fetched, String> {
    let mut current = url.to_string();
    for _ in 0..=3 {
        let (parsed, pin) = crate::llm::validate_and_pin(&current, allow_internal).await.map_err(|e| match e {
            crate::llm::LlmError::InvalidConfig(m) => m,
            _ => "URL rejected".to_string(),
        })?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("URLs with credentials are not allowed".into());
        }
        let timeout = Duration::from_secs(30);
        let client = match pin {
            Some((host, addr)) => crate::llm::pinned_client(timeout, &host, addr).map_err(|_| "HTTP client error".to_string())?,
            None => reqwest::Client::builder()
                .user_agent(crate::USER_AGENT)
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                // Security review 2026-11-19 F11: no proxy from HTTP(S)_PROXY.
                .no_proxy()
                .build()
                .map_err(|_| "HTTP client error".to_string())?,
        };
        let resp = client.get(parsed.clone()).send().await.map_err(|e| format!("fetch failed: {}", e.without_url()))?;
        let status = resp.status();
        if status.is_redirection() {
            let loc = resp.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()).ok_or("redirect without location")?;
            current = parsed.join(loc).map_err(|_| "invalid redirect target".to_string())?.to_string();
            continue;
        }
        if !status.is_success() {
            return Err(format!("fetch failed: HTTP {}", status.as_u16()));
        }
        let bytes = crate::llm::read_body_limited(resp, max).await.map_err(|e| match e {
            crate::llm::BodyError::TooLarge => format!("file too large: at most {max} bytes"),
            crate::llm::BodyError::Http(_) => "fetch failed while reading the body".to_string(),
        })?;
        if bytes.is_empty() {
            return Err("fetched file is empty".into());
        }
        let filename = parsed
            .path_segments()
            .and_then(|mut s| s.next_back().map(str::to_string))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| parsed.host_str().unwrap_or("download").to_string());
        let mut shown = parsed.clone();
        shown.set_query(None);
        shown.set_fragment(None);
        return Ok(Fetched { bytes, filename, final_url: shown.to_string() });
    }
    Err("too many redirects".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffing() {
        assert_eq!(sniff(b"%PDF-1.7\n...", "x.bin"), Some(("application/pdf", "pdf")));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n....", "a.pdf"), Some(("image/png", "png")));
        assert_eq!(sniff("Grüezi\nmiteinander\x0c".as_bytes(), "n.md"), Some(("text/plain", "md")));
        assert_eq!(sniff(b"a;b\n1;2\n", "t.csv"), Some(("text/csv", "csv")));
        assert_eq!(sniff(b"MZ\x90\x00\x03\x00", "evil.pdf"), None); // Windows program
        assert_eq!(sniff(b"PK\x03\x04rest", "archive.zip"), None);
        assert_eq!(sniff(b"PK\x03\x04rest", "brief.docx").map(|s| s.1), Some("docx"));
        assert_eq!(sniff(b"\x7fELF\x02\x01", "x.txt"), None);
        assert_eq!(sniff(b"", "x.txt"), None);
    }

    #[test]
    fn filenames() {
        assert_eq!(safe_filename("../../etc/passwd", "txt"), "passwd.txt");
        assert_eq!(safe_filename("C:\\x\\Rechnung Mai.PDF", "pdf"), "Rechnung Mai.pdf");
        assert_eq!(safe_filename("", "pdf"), "document.pdf");
        assert_eq!(safe_filename("a\"b\r\n.pdf", "pdf"), "a_b__.pdf");
    }

    #[test]
    fn base64_limits() {
        let b = base64::engine::general_purpose::STANDARD.encode(vec![1u8; 100]);
        assert_eq!(decode_base64(&b, 100).unwrap().len(), 100);
        assert!(decode_base64(&b, 99).unwrap_err().contains("too large"));
        assert!(decode_base64(&format!("data:application/pdf;base64,{b}"), 100).is_ok());
        assert!(decode_base64("!!!", 100).is_err());
        // huge input is rejected without decoding
        assert!(decode_base64(&"A".repeat(10_000), 100).unwrap_err().contains("too large"));
    }

    #[tokio::test]
    async fn ssrf_guard() {
        for u in [
            "http://127.0.0.1/x",
            "http://10.0.0.1/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data",
            "file:///etc/passwd",
            "ftp://example.com/x",
            "http://0.0.0.0/",
        ] {
            assert!(fetch_url(u, 1000, false).await.is_err(), "{u}");
        }
    }
}
