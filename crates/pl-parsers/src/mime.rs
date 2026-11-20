//! MIME detection like libmagic (`magic.from_file(path, mime=True)`), as used by the consumer.
//!
//! upstream: src/documents/consumer.py@v3.4.2 (`magic.from_file(self.working_copy, mime=True)`),
//! python-magic 0.4.27 + libmagic (file 5.46 in the Docker image; reference here: file 5.48).
//!
//! This is **not** a complete libmagic: reproduced are all formats paperless supports (PDF,
//! images, text/CSV, mail, office/ODF/RTF) and the cases in which libmagic classifies similar
//! files *differently* (mail headers, HTML, XML, JSON, CSV heuristics, source code, scripts,
//! OOXML entry order, OLE/CDF), calibrated empirically with
//! `tests/parity/pl-parsers/gen_reference.py` (section `mime_probes`). Everything unknown:
//! text → `text/plain`, otherwise `application/octet-stream`.

use std::io::Read;
use std::path::Path;

/// libmagic reads at most this many bytes (`MAGIC_PARAM_BYTES_MAX`, file ≥ 5.41: 7 MiB).
pub const BYTES_MAX: usize = 7 * 1024 * 1024;

/// MIME type of a file (reads at most [`BYTES_MAX`] bytes).
pub fn detect_file(path: &Path) -> std::io::Result<String> {
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.take(BYTES_MAX as u64).read_to_end(&mut buf)?;
    Ok(detect_bytes(&buf).to_owned())
}

/// MIME type for a file beginning.
pub fn detect_bytes(data: &[u8]) -> &'static str {
    let data = &data[..data.len().min(BYTES_MAX)];
    if data.is_empty() {
        return "inode/x-empty";
    }
    if data.len() == 1 {
        return "application/octet-stream";
    }
    if let Some(m) = binary_magic(data) {
        return m;
    }
    if pdf_after_junk(data) {
        return "application/pdf";
    }
    match text_view(data) {
        Some(text) => text_magic(data, &text),
        None => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// Binary formats (signatures at the start)
// ---------------------------------------------------------------------------

fn binary_magic(d: &[u8]) -> Option<&'static str> {
    if d.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if d.starts_with(b"\x89PNG\r\n\x1a\n") && d.len() >= 16 && &d[12..16] == b"IHDR" {
        return Some("image/png");
    }
    if d.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if d.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if d.starts_with(b"BM") && d.len() >= 18 {
        let hs = u32::from_le_bytes([d[14], d[15], d[16], d[17]]);
        if matches!(hs, 12 | 16 | 40 | 52 | 56 | 64 | 108 | 124) {
            return Some("image/bmp");
        }
    }
    if d.starts_with(b"II*\0")
        || d.starts_with(b"MM\0*")
        || d.starts_with(b"II+\0")
        || d.starts_with(b"MM\0+")
    {
        return Some("image/tiff");
    }
    if d.len() >= 12 && d.starts_with(b"RIFF") {
        match &d[8..12] {
            b"WEBP" => return Some("image/webp"),
            b"WAVE" => return Some("audio/x-wav"),
            b"AVI " => return Some("video/x-msvideo"),
            _ => {}
        }
    }
    if d.len() >= 12 && &d[4..8] == b"ftyp" {
        return Some(match &d[8..12] {
            b"heic" | b"heix" | b"heim" | b"heis" => "image/heic",
            b"hevc" | b"hevx" | b"hevm" | b"hevs" => "image/heic-sequence",
            b"mif1" => "image/heif",
            b"msf1" => "image/heif-sequence",
            b"avif" | b"avis" => "image/avif",
            b"qt  " => "video/quicktime",
            b"M4A " => "audio/x-m4a",
            _ => "video/mp4",
        });
    }
    if d.starts_with(b"PK\x03\x04") {
        return Some(zip_magic(d));
    }
    if d.starts_with(b"PK\x05\x06") {
        return Some("application/zip");
    }
    if d.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1") {
        return Some(cdf_magic(d));
    }
    if d.starts_with(b"\x1f\x8b") {
        return Some("application/gzip");
    }
    if d.starts_with(b"7z\xbc\xaf\x27\x1c") {
        return Some("application/x-7z-compressed");
    }
    if d.starts_with(b"Rar!\x1a\x07") {
        return Some("application/x-rar");
    }
    if d.starts_with(b"MZ") && d.len() >= 64 {
        return Some("application/x-dosexec");
    }
    None
}

// ---------------------------------------------------------------------------
// ZIP: OpenDocument/EPUB ("mimetype" first) and OOXML (libmagic Magdir/msooxml)
// ---------------------------------------------------------------------------

const ODF_TYPES: &[&str] = &[
    "application/vnd.oasis.opendocument.text",
    "application/vnd.oasis.opendocument.text-template",
    "application/vnd.oasis.opendocument.text-web",
    "application/vnd.oasis.opendocument.text-master",
    "application/vnd.oasis.opendocument.text-master-template",
    "application/vnd.oasis.opendocument.graphics",
    "application/vnd.oasis.opendocument.graphics-template",
    "application/vnd.oasis.opendocument.presentation",
    "application/vnd.oasis.opendocument.presentation-template",
    "application/vnd.oasis.opendocument.spreadsheet",
    "application/vnd.oasis.opendocument.spreadsheet-template",
    "application/vnd.oasis.opendocument.chart",
    "application/vnd.oasis.opendocument.chart-template",
    "application/vnd.oasis.opendocument.formula",
    "application/vnd.oasis.opendocument.formula-template",
    "application/vnd.oasis.opendocument.database",
    "application/vnd.oasis.opendocument.image",
    "application/vnd.oasis.opendocument.image-template",
];

fn find(hay: &[u8], needle: &[u8], from: usize, window: usize) -> Option<usize> {
    if from >= hay.len() || needle.is_empty() {
        return None;
    }
    let end = (from + window + needle.len()).min(hay.len());
    hay[from..end]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Search window of the OOXML header search: start positions `[s, s + 10000)`.
const OOXML_WINDOW: usize = 9_999;

/// `msooxml` subroutine: subdirectory of the entry name.
fn ooxml_name(d: &[u8], off: usize) -> Option<&'static str> {
    let s = d.get(off..)?;
    if s.starts_with(b"word/") {
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
    } else if s.starts_with(b"ppt/") {
        Some("application/vnd.openxmlformats-officedocument.presentationml.presentation")
    } else if s.starts_with(b"xl/") {
        Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
    } else if s.starts_with(b"visio/") {
        Some("application/vnd.ms-visio.drawing.main+xml")
    } else if s.starts_with(b"AppxManifest.xml") || s.starts_with(b"AppxBlockMap.xml") {
        Some("application/vnd.ms-appx")
    } else {
        None
    }
}

/// Result of the OOXML rule: `None` = rule does not apply (continue with ODF/ZIP),
/// `Some(None)` = "Microsoft OOXML" without MIME (→ `application/octet-stream`).
fn msooxml(d: &[u8]) -> Option<Option<&'static str>> {
    // >0x1E use msooxml
    if let Some(m) = ooxml_name(d, 0x1e) {
        return Some(Some(m));
    }
    // >>0x1E regex \[Content_Types\]\.xml|_rels/\.rels|docProps|customXml
    // (searched from 0x1E up to the first NUL, like regexec on C strings)
    let region_end = d[0x1e.min(d.len())..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| p + 0x1e)
        .unwrap_or(d.len())
        .min(0x1e + 8192);
    let region = d.get(0x1e..region_end)?;
    // leftmost-first: earliest start wins, end = start + length of the match
    let mut first: Option<(usize, usize)> = None;
    for pat in [
        &b"[Content_Types].xml"[..],
        b"_rels/.rels",
        b"docProps",
        b"customXml",
    ] {
        if let Some(p) = region.windows(pat.len()).position(|w| w == pat)
            && first.is_none_or(|(s, _)| p < s)
        {
            first = Some((p, p + pat.len()));
        }
    }
    let (_, rel_end) = first?;
    let regex_end = 0x1e + rel_end;
    // Empirical (file 5.48, tests/parity/pl-parsers): the search for the 2nd header starts 26 bytes
    // after the end of the regex match; every further search 26 bytes after the name start of the
    // header found. A match must start within the following 10000 bytes.
    let h2 = find(d, b"PK\x03\x04", regex_end + 26, OOXML_WINDOW)?;
    if let Some(m) = ooxml_name(d, h2 + 30) {
        return Some(Some(m));
    }
    let h3 = find(d, b"PK\x03\x04", h2 + 30 + 26, OOXML_WINDOW)?;
    if let Some(m) = ooxml_name(d, h3 + 30) {
        return Some(Some(m));
    }
    let h4 = match find(d, b"PK\x03\x04", h3 + 30 + 26, OOXML_WINDOW) {
        Some(p) => p,
        None => return Some(None),
    };
    Some(ooxml_name(d, h4 + 30))
}

/// PaperRust (test log 2026-11-16 C-073): `%PDF-` not at the start, but within the first
/// 1024 bytes (junk before it, which PDF readers – Acrobat, PDFium, pdf.js – skip). libmagic
/// recognizes `%PDF-` from byte 0 and – only for text files – within the first 1024 bytes; if
/// the junk before it contained binary bytes, the file became `application/octet-stream` and was
/// rejected. Now it counts as a PDF if the header `%PDF-<digit>.<digit>` is also followed by an
/// indirect object (`<n> <n> obj`) (binary files that only happen to contain the character
/// sequence remain what they were).
fn pdf_after_junk(d: &[u8]) -> bool {
    let head = &d[..d.len().min(1024 + 8)];
    let Some(p) = head.windows(8).position(|w| {
        w.starts_with(b"%PDF-") && w[5].is_ascii_digit() && w[6] == b'.' && w[7].is_ascii_digit()
    }) else {
        return false;
    };
    if p == 0 || p > 1024 {
        return false;
    }
    // an indirect object "<number> <number> obj"
    let rest = &d[p..];
    rest.windows(3).enumerate().any(|(i, w)| {
        if w != b"obj" {
            return false;
        }
        let before = &rest[..i];
        let digits_then_space = |s: &[u8]| -> Option<usize> {
            let t = s.len() - s.iter().rev().take_while(|b| b.is_ascii_whitespace()).count();
            let n = s[..t].iter().rev().take_while(|b| b.is_ascii_digit()).count();
            (t < s.len() && n > 0).then_some(t - n)
        };
        digits_then_space(before)
            .and_then(|k| digits_then_space(&before[..k]))
            .is_some()
    })
}

/// PaperRust (test log 2026-11-16 C-008): OOXML package whose main part (`word/`, `xl/`,
/// `ppt/`) is neither among the first entries libmagic looks at, nor found otherwise
/// (e.g. python-docx: `[Content_Types].xml`, `_rels/`, `docProps/…`, then `word/…`). All local
/// file headers and the central directory are checked; the prerequisite is the entry
/// `[Content_Types].xml` (mandatory part of every OPC package), so that ordinary ZIP archives
/// stay ZIP.
fn ooxml_by_entries(d: &[u8]) -> Option<&'static str> {
    let mut names: Vec<&[u8]> = Vec::new();
    let mut collect = |sig: &[u8], name_len_at: usize, name_at: usize| {
        let mut i = 0;
        while let Some(p) = find(d, sig, i, d.len()) {
            let nl = d
                .get(p + name_len_at..p + name_len_at + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
            if let Some(name) = nl.and_then(|nl| d.get(p + name_at..p + name_at + nl)) {
                names.push(name);
            }
            i = p + 4;
        }
    };
    // local file header: name length at +26, name from +30; central directory: +28 / +46
    collect(b"PK\x03\x04", 26, 30);
    collect(b"PK\x01\x02", 28, 46);
    if !names.iter().any(|n| *n == b"[Content_Types].xml") {
        return None;
    }
    for prefix in [&b"word/"[..], b"ppt/", b"xl/"] {
        if names.iter().any(|n| n.starts_with(prefix)) {
            return ooxml_name(prefix, 0);
        }
    }
    None
}

fn zip_magic(d: &[u8]) -> &'static str {
    if let Some(r) = msooxml(d) {
        if let Some(m) = r {
            return m;
        }
        return ooxml_by_entries(d).unwrap_or("application/octet-stream");
    }
    if let Some(m) = ooxml_by_entries(d) {
        return m;
    }
    // OpenDocument / EPUB: first entry "mimetype", uncompressed, content from offset 38
    if d.get(30..38) == Some(b"mimetype") {
        let content = &d[38.min(d.len())..];
        if content.starts_with(b"application/epub+zip") {
            return "application/epub+zip";
        }
        let mut best: Option<&'static str> = None;
        for t in ODF_TYPES {
            if content.starts_with(t.as_bytes()) && best.is_none_or(|b| t.len() > b.len()) {
                best = Some(t);
            }
        }
        if let Some(t) = best {
            return t;
        }
    }
    if d.len() < 30 {
        return "application/octet-stream";
    }
    "application/zip"
}

// ---------------------------------------------------------------------------
// OLE2 / CDF (libmagic readcdf.c)
// ---------------------------------------------------------------------------

fn cdf_magic(d: &[u8]) -> &'static str {
    let cf = match cfb::CompoundFile::open(std::io::Cursor::new(d)) {
        Ok(cf) => cf,
        Err(_) => return "application/x-ole-storage",
    };
    cdf_classify(cf)
}

/// Classification of an opened compound file (also for tests with generated CFB files).
pub fn cdf_classify<F: std::io::Read + std::io::Seek>(
    mut cf: cfb::CompoundFile<F>,
) -> &'static str {
    let mut names: Vec<String> = Vec::new();
    for e in cf.walk() {
        names.push(e.name().to_owned());
    }
    if cf.is_stream("/\u{5}SummaryInformation") {
        let mut buf = Vec::new();
        if let Ok(mut s) = cf.open_stream("/\u{5}SummaryInformation") {
            let _ = (&mut s).take(1 << 20).read_to_end(&mut buf);
        }
        if let Some(app) = summary_app_name(&buf) {
            let a = app.to_lowercase();
            for (pat, mime) in [
                ("word", "application/msword"),
                ("excel", "application/vnd.ms-excel"),
                ("powerpoint", "application/vnd.ms-powerpoint"),
                ("crystal reports", "application/x-rpt"),
                ("advanced installer", "application/vnd.ms-msi"),
                ("installshield", "application/vnd.ms-msi"),
                ("microsoft patch compiler", "application/vnd.ms-msi"),
                ("nant", "application/vnd.ms-msi"),
                ("windows installer", "application/vnd.ms-msi"),
            ] {
                if a.contains(pat) {
                    return mime;
                }
            }
        }
        if names.iter().any(|n| n == "EncryptedPackage") {
            return "application/encrypted";
        }
        return "application/vnd.ms-office";
    }
    // readcdf.c name2mime: patterns in order, name comparison as prefix (cdf_namecmp)
    for (pat, mime) in [
        ("Book", "application/vnd.ms-excel"),
        ("Workbook", "application/vnd.ms-excel"),
        ("WordDocument", "application/msword"),
        ("PowerPoint", "application/vnd.ms-powerpoint"),
        ("DigitalSignature", "application/vnd.ms-msi"),
        ("__substg1.0_", "application/vnd.ms-outlook"),
    ] {
        if names.iter().any(|n| n.starts_with(pat)) {
            return mime;
        }
    }
    "application/x-ole-storage"
}

/// PIDSI_APPNAME (0x12) from a `\x05SummaryInformation` property set.
pub(crate) fn summary_app_name(b: &[u8]) -> Option<String> {
    let rd32 = |o: usize| -> Option<u32> {
        b.get(o..o + 4)
            .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
    };
    let nsets = rd32(24)?;
    if nsets == 0 {
        return None;
    }
    let sec = rd32(28 + 16)? as usize;
    let nprops = rd32(sec + 4)? as usize;
    for i in 0..nprops.min(1024) {
        let pid = rd32(sec + 8 + i * 8)?;
        let off = rd32(sec + 12 + i * 8)? as usize;
        if pid != 0x12 {
            continue;
        }
        let p = sec + off;
        let vt = rd32(p)? & 0xffff;
        let len = rd32(p + 4)? as usize;
        return match vt {
            0x1e => {
                let raw = b.get(p + 8..p + 8 + len.min(4096))?;
                let raw = raw.split(|&c| c == 0).next().unwrap_or(&[]);
                Some(raw.iter().map(|&c| c as char).collect())
            }
            0x1f => {
                let raw = b.get(p + 8..p + 8 + (len * 2).min(8192))?;
                let u: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&c| c != 0)
                    .collect();
                Some(String::from_utf16_lossy(&u))
            }
            _ => None,
        };
    }
    None
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// libmagic `text_chars`: these bytes never occur in text.
fn is_forbidden(b: u8) -> bool {
    matches!(b, 0x00..=0x06 | 0x0e..=0x19 | 0x1c..=0x1f | 0x7f)
}

/// If the data counts as text: the text as UTF-8 (UTF-16 with BOM converted,
/// UTF-8 BOM removed). Otherwise `None` (binary).
fn text_view(d: &[u8]) -> Option<Vec<u8>> {
    if (d.starts_with(b"\xff\xfe") || d.starts_with(b"\xfe\xff")) && d.len().is_multiple_of(2) {
        let be = d[0] == 0xfe;
        let units: Vec<u16> = d[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        if units.iter().all(|&u| u >= 0x80 || !is_forbidden(u as u8)) {
            return Some(String::from_utf16_lossy(&units).into_bytes());
        }
        return None;
    }
    // A single trailing NUL does not bother libmagic (empirical)
    let body = if d.len() > 1 && d[d.len() - 1] == 0 {
        &d[..d.len() - 1]
    } else {
        d
    };
    if body.iter().any(|&b| is_forbidden(b)) {
        return None;
    }
    Some(body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(body).to_vec())
}

fn starts_ci(s: &[u8], p: &[u8]) -> bool {
    s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p)
}

/// Search (case-insensitive, several spaces in the pattern = any amount of whitespace,
/// libmagic `search/N/cW`), start position < `range`.
fn search_ci_w(t: &[u8], pat: &[u8], range: usize) -> bool {
    let limit = t.len().min(range);
    'outer: for start in 0..limit {
        let mut i = start;
        let mut j = 0;
        while j < pat.len() {
            if pat[j] == b' ' {
                if i >= t.len() || !t[i].is_ascii_whitespace() {
                    continue 'outer;
                }
                while i < t.len() && t[i].is_ascii_whitespace() {
                    i += 1;
                }
                j += 1;
                continue;
            }
            if i >= t.len() || !t[i].eq_ignore_ascii_case(&pat[j]) {
                continue 'outer;
            }
            i += 1;
            j += 1;
        }
        return true;
    }
    false
}

fn lines(t: &[u8]) -> impl Iterator<Item = &[u8]> {
    t.split(|&b| b == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
}

fn text_magic(raw: &[u8], t: &[u8]) -> &'static str {
    // file_buffer: JSON and CSV before the text signatures
    if let Some(m) = json_magic(t) {
        return m;
    }
    if csv_parse(t) {
        return "text/csv";
    }
    if raw.starts_with(b"{\\rtf") && raw.iter().all(|&b| b < 0x80) {
        return "text/rtf";
    }
    if starts_ci(t, b"<?xml") && t.len() > 5 && t[5].is_ascii_whitespace() {
        if search_ci_w(t, b"<svg", 4096) {
            return "image/svg+xml";
        }
        if search_ci_w(t, b"xmlns=\"http://www.w3.org/1999/xhtml\"", 4096) {
            return "application/xhtml+xml";
        }
        return "text/xml";
    }
    if t.starts_with(b"<svg") {
        return "image/svg+xml";
    }
    if find(raw, b"%PDF-", 0, 1024).is_some() {
        return "application/pdf";
    }
    if t.starts_with(b"%!") {
        return "application/postscript";
    }
    if starts_ci(t, b"<?php") {
        return "text/x-php";
    }
    if t.starts_with(b"#!")
        && let Some(m) = shebang(t)
    {
        return m;
    }
    if let Some(m) = mail_magic(t) {
        return m;
    }
    if starts_ci(t, b"BEGIN:VCARD") {
        return "text/vcard";
    }
    if t.starts_with(b"BEGIN:VCALENDAR") {
        return "text/calendar";
    }
    for (p, m) in [
        (
            &b"-----BEGIN PGP MESSAGE-----"[..],
            "application/pgp-encrypted",
        ),
        (
            b"-----BEGIN PGP SIGNATURE-----",
            "application/pgp-signature",
        ),
        (
            b"-----BEGIN PGP PUBLIC KEY BLOCK-----",
            "application/pgp-keys",
        ),
        (b"-----BEGIN PGP SIGNED MESSAGE-----", "text/PGP"),
        (b"-----BEGIN CERTIFICATE-----", "application/x-pem-file"),
        (b"-----BEGIN PRIVATE KEY-----", "text/x-ssh-private-key"),
        (
            b"-----BEGIN OPENSSH PRIVATE KEY-----",
            "text/x-ssh-private-key",
        ),
        (b"-----BEGIN RSA PRIVATE KEY-----", "text/x-ssl-private-key"),
        (b"ssh-rsa ", "text/x-ssh-public-key"),
        (b"ssh-ed25519 ", "text/x-ssh-public-key"),
        (b"diff ", "text/x-diff"),
        (b"--- ", "text/x-diff"),
        (b"*** ", "text/x-diff"),
        (b"Index: ", "text/x-diff"),
    ] {
        if t.starts_with(p) {
            return m;
        }
    }
    for pat in [
        &b"<!doctype html"[..],
        b"<head>",
        b"<head ",
        b"<title>",
        b"<title ",
        b"<html>",
        b"<html ",
        b"<script>",
        b"<script ",
        b"<style>",
        b"<style ",
        b"<table>",
        b"<table ",
        b"<a href=",
    ] {
        if search_ci_w(t, pat, 4096) {
            return "text/html";
        }
    }
    if find(t, b"\\documentclass", 0, 4096).is_some()
        || find(t, b"\\section", 0, 4096).is_some()
        || find(t, b"\\chapter", 0, 4096).is_some()
        || t.starts_with(b"\\input")
    {
        return "text/x-tex";
    }
    source_magic(t).unwrap_or("text/plain")
}

fn shebang(t: &[u8]) -> Option<&'static str> {
    let line = lines(t).next().unwrap_or(&[]);
    let line = String::from_utf8_lossy(&line[2..]).into_owned();
    let mut parts = line.split_whitespace();
    let mut prog = parts.next()?.rsplit('/').next().unwrap_or("").to_owned();
    if prog == "env" {
        prog = parts.find(|p| !p.starts_with('-'))?.to_owned();
    }
    let p = prog.as_str();
    let base = p.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    Some(match base {
        "sh" | "bash" | "zsh" | "ksh" | "dash" | "ash" | "csh" | "tcsh" => "text/x-shellscript",
        "python" => "text/x-script.python",
        "perl" => "text/x-perl",
        "ruby" => "text/x-ruby",
        "node" | "nodejs" => "text/javascript",
        "awk" | "gawk" | "nawk" => "text/x-awk",
        "tclsh" | "wish" | "tcl" => "text/x-tcl",
        "lua" => "text/x-lua",
        "php" => "text/x-php",
        _ => return None,
    })
}

fn mail_magic(t: &[u8]) -> Option<&'static str> {
    if starts_ci(t, b"return-path:") || starts_ci(t, b"delivered-to:") {
        return Some("message/rfc822");
    }
    for p in [
        &b"Received:"[..],
        b"From:",
        b"To:",
        b"Cc:",
        b"Date:",
        b"Subject:",
        b"Relay-Version:",
        b"Forward to ",
        b"Pipe to ",
    ] {
        if t.starts_with(p) {
            return Some("message/rfc822");
        }
    }
    if t.starts_with(b"Path:") || t.starts_with(b"Xref:") {
        return Some("message/news");
    }
    if t.starts_with(b"From ") {
        return Some("application/mbox");
    }
    None
}

fn source_magic(t: &[u8]) -> Option<&'static str> {
    let head = &t[..t.len().min(64 * 1024)];
    let mut c = false;
    let mut cpp = false;
    let mut py = false;
    let mut make = false;
    let mut troff = false;
    for l in lines(head) {
        if l.starts_with(b"#include")
            || l.starts_with(b"#define")
            || l.starts_with(b"#ifdef")
            || l.starts_with(b"#ifndef")
            || l.starts_with(b"#pragma")
        {
            c = true;
        }
        let trimmed: &[u8] = {
            let s = l
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .unwrap_or(l.len());
            &l[s..]
        };
        if trimmed.starts_with(b"char *") || trimmed.starts_with(b"char*") {
            c = true;
        }
        for kw in [&b"struct"[..], b"union"] {
            if let Some(rest) = l.strip_prefix(kw)
                && is_c_decl(rest)
            {
                c = true;
            }
        }
        if let Some(rest) = l.strip_prefix(b"namespace")
            && is_c_decl(rest)
        {
            cpp = true;
        }
        if l.starts_with(b"template <") || l.starts_with(b"template<") {
            cpp = true;
        }
        if py_line(l) {
            py = true;
        }
        if l.starts_with(b"all:")
            || l.starts_with(b"CC=")
            || l.starts_with(b"CC =")
            || l.starts_with(b"CFLAGS")
            || l.starts_with(b"LDFLAGS")
            || l.starts_with(b"VPATH")
            || l.starts_with(b"SUBDIRS")
            || l.starts_with(b".PRECIOUS")
        {
            make = true;
        }
        if l.starts_with(b".TH ") || l.starts_with(b".SH ") || l.starts_with(b".\\\"") {
            troff = true;
        }
    }
    if find(head, b"if __name__ == ", 0, head.len()).is_some() {
        py = true;
    }
    if cpp {
        Some("text/x-c++")
    } else if c {
        Some("text/x-c")
    } else if py {
        Some("text/x-script.python")
    } else if make {
        Some("text/x-makefile")
    } else if troff {
        Some("text/troff")
    } else {
        None
    }
}

/// `(struct|union|namespace)\s+\w+\s*\{`
fn is_c_decl(rest: &[u8]) -> bool {
    let mut i = 0;
    if rest.first().is_none_or(|b| !b.is_ascii_whitespace()) {
        return false;
    }
    while i < rest.len() && rest[i].is_ascii_whitespace() {
        i += 1;
    }
    let s = i;
    while i < rest.len() && (rest[i].is_ascii_alphanumeric() || rest[i] == b'_') {
        i += 1;
    }
    if i == s {
        return false;
    }
    while i < rest.len() && rest[i].is_ascii_whitespace() {
        i += 1;
    }
    rest.get(i) == Some(&b'{')
}

fn py_line(l: &[u8]) -> bool {
    let s = String::from_utf8_lossy(l);
    if let Some(r) = s.strip_prefix("def ") {
        // def name(...):
        if let Some(open) = r.find('(') {
            let name = r[..open].trim();
            if !name.is_empty()
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                && r.trim_end().ends_with(':')
            {
                return true;
            }
        }
    }
    if let Some(r) = s.strip_prefix("class ") {
        let r = r.trim_end();
        if r.ends_with(':') {
            let name: String = r
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return true;
            }
        }
    }
    if let Some(r) = s.strip_prefix("from ") {
        let mut it = r.split_whitespace();
        if it.next().is_some() && it.next() == Some("import") && it.next().is_some() {
            return true;
        }
    }
    false
}

/// libmagic `is_csv.c` `csv_parse` (without line limit, empirical file 5.48).
fn csv_parse(t: &[u8]) -> bool {
    let (mut nf, mut tf, mut nl) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while i < t.len() {
        let c = t[i];
        i += 1;
        match c {
            b'"' => {
                // eatquote
                let mut quote = false;
                while i < t.len() {
                    let c2 = t[i];
                    i += 1;
                    if c2 != b'"' {
                        if quote {
                            i -= 1;
                            break;
                        }
                        continue;
                    }
                    if quote {
                        quote = false;
                        continue;
                    }
                    quote = true;
                }
            }
            b',' => nf += 1,
            b'\n' => {
                nl += 1;
                if tf == 0 {
                    if nf == 0 {
                        return false;
                    }
                    tf = nf;
                } else if tf != nf {
                    return false;
                }
                nf = 0;
            }
            _ => {}
        }
    }
    tf != 0 && nl >= 2
}

/// libmagic `is_json.c` (lenient: comma before `}`/`]` allowed). Only object/array at top
/// level; several values separated by line breaks → NDJSON.
fn json_magic(t: &[u8]) -> Option<&'static str> {
    let mut p = JsonP {
        s: t,
        i: 0,
        depth: 0,
    };
    let mut count = 0;
    loop {
        p.ws();
        if p.i >= t.len() {
            break;
        }
        if count > 0 {
            // there must have been a line break between values
            let between = &t[..p.i];
            if !between
                .iter()
                .rev()
                .take_while(|b| b.is_ascii_whitespace())
                .any(|&b| b == b'\n')
            {
                return None;
            }
        }
        match t[p.i] {
            b'{' | b'[' => {}
            _ => return None,
        }
        if !p.value() {
            return None;
        }
        count += 1;
    }
    match count {
        0 => None,
        1 => Some("application/json"),
        _ => Some("application/x-ndjson"),
    }
}

struct JsonP<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl JsonP<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn value(&mut self) -> bool {
        self.ws();
        if self.i >= self.s.len() {
            return false;
        }
        self.depth += 1;
        if self.depth > 256 {
            return false;
        }
        let ok = match self.s[self.i] {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string(),
            b't' => self.lit(b"true"),
            b'f' => self.lit(b"false"),
            b'n' => self.lit(b"null"),
            b'-' | b'0'..=b'9' => self.number(),
            _ => false,
        };
        self.depth -= 1;
        ok
    }
    fn lit(&mut self, l: &[u8]) -> bool {
        if self.s[self.i..].starts_with(l) {
            self.i += l.len();
            true
        } else {
            false
        }
    }
    fn string(&mut self) -> bool {
        self.i += 1;
        while self.i < self.s.len() {
            match self.s[self.i] {
                b'"' => {
                    self.i += 1;
                    return true;
                }
                b'\\' => self.i += 2,
                _ => self.i += 1,
            }
        }
        false
    }
    fn number(&mut self) -> bool {
        let st = self.i;
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        let ds = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        if self.i == ds {
            return false;
        }
        if self.i < self.s.len() && self.s[self.i] == b'.' {
            self.i += 1;
            while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                self.i += 1;
            }
        }
        if self.i < self.s.len() && (self.s[self.i] == b'e' || self.s[self.i] == b'E') {
            self.i += 1;
            if self.i < self.s.len() && (self.s[self.i] == b'+' || self.s[self.i] == b'-') {
                self.i += 1;
            }
            while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                self.i += 1;
            }
        }
        self.i > st
    }
    fn object(&mut self) -> bool {
        self.i += 1;
        loop {
            self.ws();
            if self.i >= self.s.len() {
                return false;
            }
            if self.s[self.i] == b'}' {
                self.i += 1;
                return true;
            }
            if self.s[self.i] != b'"' || !self.string() {
                return false;
            }
            self.ws();
            if self.i >= self.s.len() || self.s[self.i] != b':' {
                return false;
            }
            self.i += 1;
            if !self.value() {
                return false;
            }
            self.ws();
            if self.i >= self.s.len() {
                return false;
            }
            match self.s[self.i] {
                b',' => self.i += 1,
                b'}' => {
                    self.i += 1;
                    return true;
                }
                _ => return false,
            }
        }
    }
    fn array(&mut self) -> bool {
        self.i += 1;
        loop {
            self.ws();
            if self.i >= self.s.len() {
                return false;
            }
            if self.s[self.i] == b']' {
                self.i += 1;
                return true;
            }
            if !self.value() {
                return false;
            }
            self.ws();
            if self.i >= self.s.len() {
                return false;
            }
            match self.s[self.i] {
                b',' => self.i += 1,
                b']' => {
                    self.i += 1;
                    return true;
                }
                _ => return false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        assert_eq!(detect_bytes(b""), "inode/x-empty");
        assert_eq!(detect_bytes(b"a"), "application/octet-stream");
        assert_eq!(detect_bytes(b"%PDF-1.4\n"), "application/pdf");
        assert_eq!(detect_bytes(b"hallo welt\n"), "text/plain");
        assert_eq!(detect_bytes(b"a,b\n1,2\n"), "text/csv");
        assert_eq!(detect_bytes(b"From: a@b\n\nx\n"), "message/rfc822");
        assert_eq!(detect_bytes(b"MIME-Version: 1.0\n\nx\n"), "text/plain");
        assert_eq!(detect_bytes(b"x <title> y\n"), "text/html");
        assert_eq!(detect_bytes(b"{\"a\":1,}\n"), "application/json");
        assert_eq!(detect_bytes(b"Text\x01x\n"), "application/octet-stream");
    }

    /// Minimal ZIP (uncompressed): local headers, central directory, end record.
    fn zip_stored(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let off = out.len() as u32;
            let mut h = Vec::new();
            h.extend_from_slice(b"PK\x03\x04");
            h.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            h.extend_from_slice(&0u32.to_le_bytes()); // crc (irrelevant for detection)
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&h);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&off.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    /// Test log 2026-11-16 C-008: DOCX as produced by python-docx/LibreOffice – `word/` only after
    /// `[Content_Types].xml`, `_rels/` and several `docProps/` entries.
    #[test]
    fn ooxml_with_main_part_late_is_recognized() {
        let ct = vec![b'x'; 1200];
        let thumb = vec![0xffu8; 20_000];
        let docx = zip_stored(&[
            ("[Content_Types].xml", &ct),
            ("_rels/.rels", b"<r/>"),
            ("docProps/core.xml", b"<c/>"),
            ("docProps/app.xml", b"<a/>"),
            ("docProps/thumbnail.jpeg", &thumb),
            ("word/document.xml", b"<w/>"),
            ("word/styles.xml", b"<s/>"),
        ]);
        assert_eq!(detect_bytes(&docx), "application/vnd.openxmlformats-officedocument.wordprocessingml.document");
        let xlsx = zip_stored(&[("[Content_Types].xml", &ct), ("_rels/.rels", b"<r/>"), ("docProps/core.xml", b"<c/>"), ("docProps/app.xml", b"<a/>"), ("xl/workbook.xml", b"<w/>")]);
        assert_eq!(detect_bytes(&xlsx), "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
        // ordinary ZIP with a word/ folder but without [Content_Types].xml: stays ZIP
        let plain = zip_stored(&[("readme.txt", b"hi"), ("word/notes.txt", b"x")]);
        assert_eq!(detect_bytes(&plain), "application/zip");
    }

    /// Test log 2026-11-16 C-073: junk before `%PDF-` (up to 1024 bytes) is a PDF.
    #[test]
    fn pdf_with_junk_before_header() {
        let mut pdf = b"GARBAGE\x00\x01\x02 junk junk\n".to_vec();
        pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n%%EOF\n");
        assert_eq!(detect_bytes(&pdf), "application/pdf");
        let mut far = vec![b' '; 1100];
        far.extend_from_slice(b"%PDF-1.4\n1 0 obj\nendobj\n");
        assert_ne!(detect_bytes(&far), "application/pdf");
        // Binary data that only contains the character sequence (no object): not a PDF
        assert_eq!(detect_bytes(b"\x00\x01\x02 Version %PDF-1.7 im Handbuch, jedes object zaehlt.\n"), "application/octet-stream");
        // plain text with %PDF- in the first 1024 bytes was already PDF in libmagic (text rule)
        assert_eq!(detect_bytes(b"Siehe %PDF-1.7 im Handbuch.\n"), "application/pdf");
    }
}

#[cfg(test)]
mod cdf_tests {
    use std::io::{Cursor, Write};

    use super::*;

    fn cfb_with(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        for (p, d) in streams {
            let mut s = cf.create_stream(p).unwrap();
            s.write_all(d).unwrap();
        }
        cf.flush().unwrap();
        cf.into_inner().into_inner()
    }

    fn summary(app: &str) -> Vec<u8> {
        let mut b = vec![0xfe, 0xff, 0, 0, 0, 0, 0, 0];
        b.extend_from_slice(&[0u8; 16]); // clsid
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&[0u8; 16]); // fmtid
        b.extend_from_slice(&48u32.to_le_bytes()); // Offset of the section
        let mut sec = Vec::new();
        let val_off = 8 + 8;
        let mut val = Vec::new();
        val.extend_from_slice(&0x1eu32.to_le_bytes());
        val.extend_from_slice(&((app.len() + 1) as u32).to_le_bytes());
        val.extend_from_slice(app.as_bytes());
        val.push(0);
        sec.extend_from_slice(&((val_off + val.len()) as u32).to_le_bytes());
        sec.extend_from_slice(&1u32.to_le_bytes());
        sec.extend_from_slice(&0x12u32.to_le_bytes());
        sec.extend_from_slice(&(val_off as u32).to_le_bytes());
        sec.extend(val);
        b.extend(sec);
        b
    }

    #[test]
    fn cdf_classification() {
        let word = cfb_with(&[
            ("/\u{5}SummaryInformation", summary("Microsoft Office Word")),
            ("/WordDocument", vec![0; 600]),
        ]);
        assert_eq!(detect_bytes(&word), "application/msword");
        let xls = cfb_with(&[
            ("/\u{5}SummaryInformation", summary("Microsoft Excel")),
            ("/Workbook", vec![0; 600]),
        ]);
        assert_eq!(detect_bytes(&xls), "application/vnd.ms-excel");
        let unknown_app = cfb_with(&[
            ("/\u{5}SummaryInformation", summary("Foo")),
            ("/WordDocument", vec![0; 600]),
        ]);
        assert_eq!(detect_bytes(&unknown_app), "application/vnd.ms-office");
        let no_summary = cfb_with(&[("/PowerPoint Document", vec![0; 600])]);
        assert_eq!(detect_bytes(&no_summary), "application/vnd.ms-powerpoint");
        let plain = cfb_with(&[("/Foo", vec![0; 10])]);
        assert_eq!(detect_bytes(&plain), "application/x-ole-storage");
    }
}
