//! `/static/…` like WhiteNoise (`whitenoise.middleware.WhiteNoiseMiddleware`, position 2 of
//! upstream's MIDDLEWARE: only SecurityMiddleware and the compression middleware wrap it).
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (`STATIC_ROOT` = `PAPERLESS_STATICDIR`,
//! `WHITENOISE_STATIC_PREFIX = "/static/"`, `CompressedStaticFilesStorage`),
//! whitenoise@6 base.py (`update_files_dictionary`, `is_compressed_variant`, `get_static_file`,
//! `add_mime_headers`, `add_cache_headers`), responders.py (`StaticFile`: alternatives sorted by
//! size, `\bgzip\b`/`\bbr\b` against Accept-Encoding, ETag `"{mktime(Last-Modified):x}-{size:x}"`,
//! 304 via If-None-Match (exact) / If-Modified-Since, single byte range, 405/416),
//! middleware.py (`max_age` 60, 0 with DEBUG; `immutable_file_test` is never true for the
//! non-manifest storage).
//!
//! The file list is scanned once at startup (`WHITENOISE_AUTOREFRESH` = DEBUG is not
//! reproduced: new files need a restart, like upstream without DEBUG).
//!
//! PaperRust storage (F1, `frontend/tools/static-dedupe.mjs`): `<STATIC_ROOT>/.paperrust/static.json`
//! lists *virtual* files – byte-identical copies stored once (`alias`) and per-locale `main.js` stored
//! as a copy/insert delta against `en-US` (`delta`). They are served exactly as if the file were on
//! disk (same headers, ETag from size + mtime of the stored file, `.gz`/`.br` alternatives).
//! `.paperrust/` itself is never served.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, NaiveDateTime, TimeZone};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::django_mw::{SecurityOnly, header_str};

/// URL prefix (`WHITENOISE_STATIC_PREFIX`).
pub const PREFIX: &str = "/static/";

#[derive(Debug, Clone)]
struct Alt {
    encoding: Option<&'static str>,
    src: Src,
    size: u64,
}

/// Where the bytes of a served file come from.
#[derive(Debug, Clone)]
enum Src {
    /// A file on disk (also the stored copy behind an `alias` entry).
    File(PathBuf),
    /// Rebuilt from a delta (`.paperrust/static.json` `delta`).
    Delta(Arc<Delta>),
}

/// `delta` entry of the storage manifest.
#[derive(Debug)]
struct Delta {
    key: String,
    base: PathBuf,
    base_size: u64,
    file: PathBuf,
    size: u64,
    fnv1a32: u32,
    cache: Arc<DeltaCache>,
}

/// The last few rebuilt files (a `main.js` is ~3 MB; normally one or two locales are in use).
#[derive(Debug, Default)]
struct DeltaCache(Mutex<std::collections::VecDeque<(String, bytes::Bytes)>>);

const DELTA_CACHE_ENTRIES: usize = 3;

impl DeltaCache {
    fn get(&self, key: &str) -> Option<bytes::Bytes> {
        let mut q = self.0.lock().ok()?;
        let i = q.iter().position(|(k, _)| k == key)?;
        let e = q.remove(i)?;
        let b = e.1.clone();
        q.push_front(e);
        Some(b)
    }

    fn put(&self, key: &str, b: bytes::Bytes) {
        if let Ok(mut q) = self.0.lock() {
            q.retain(|(k, _)| k != key);
            q.push_front((key.to_owned(), b));
            q.truncate(DELTA_CACHE_ENTRIES);
        }
    }
}

fn fnv1a32(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(*b)).wrapping_mul(0x0100_0193))
}

/// Apply a `PRD1` delta (`static-dedupe.mjs`): `0x01 off:u32 len:u32` copies from `base`,
/// `0x02 len:u32 bytes` inserts.
fn apply_delta(base: &[u8], delta: &[u8], size_hint: usize) -> Result<Vec<u8>, &'static str> {
    // reserve at most what the inputs can produce (a broken manifest must not allocate gigabytes)
    let cap = size_hint.min(base.len().saturating_mul(2).saturating_add(delta.len()));
    let rest = delta.strip_prefix(b"PRD1").ok_or("not a PRD1 delta")?;
    let u32_at = |b: &[u8], i: usize| -> Result<usize, &'static str> {
        let s = b.get(i..i + 4).ok_or("truncated delta")?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
    };
    let mut out = Vec::with_capacity(cap);
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            1 => {
                let (off, len) = (u32_at(rest, i + 1)?, u32_at(rest, i + 5)?);
                out.extend_from_slice(base.get(off..off.checked_add(len).ok_or("bad copy")?).ok_or("copy outside base")?);
                i += 9;
            }
            2 => {
                let len = u32_at(rest, i + 1)?;
                out.extend_from_slice(rest.get(i + 5..i + 5 + len).ok_or("truncated insert")?);
                i += 5 + len;
            }
            _ => return Err("unknown delta op"),
        }
        if out.len() > size_hint {
            return Err("delta output too large");
        }
    }
    Ok(out)
}

impl Delta {
    /// Rebuilt content (cached), checked against size and FNV-1a from the manifest.
    async fn bytes(self: &Arc<Self>) -> Result<bytes::Bytes, String> {
        if let Some(b) = self.cache.get(&self.key) {
            return Ok(b);
        }
        let d = self.clone();
        let b = tokio::task::spawn_blocking(move || -> Result<bytes::Bytes, String> {
            let base = std::fs::read(&d.base).map_err(|e| format!("delta base: {e}"))?;
            if base.len() as u64 != d.base_size {
                return Err("delta base changed".into());
            }
            let delta = std::fs::read(&d.file).map_err(|e| format!("delta file: {e}"))?;
            let out = apply_delta(&base, &delta, d.size as usize).map_err(str::to_owned)?;
            if out.len() as u64 != d.size || fnv1a32(&out) != d.fnv1a32 {
                return Err("delta result does not match the manifest".into());
            }
            Ok(bytes::Bytes::from(out))
        })
        .await
        .map_err(|e| format!("delta task: {e}"))??;
        self.cache.put(&self.key, b.clone());
        Ok(b)
    }
}

/// One served file (`whitenoise.responders.StaticFile`).
#[derive(Debug)]
pub struct StaticFile {
    /// Headers without Content-Length/Content-Encoding, in whitenoise order.
    headers: Vec<(HeaderName, String)>,
    etag: String,
    /// `parsedate(Last-Modified)` fields (naive, GMT values).
    last_modified: Option<NaiveDateTime>,
    alternatives: Vec<Alt>,
}

/// All files below `STATIC_ROOT`, keyed by URL (`/static/…`).
#[derive(Debug, Default)]
pub struct StaticFiles {
    files: HashMap<String, Arc<StaticFile>>,
    pub root: PathBuf,
}

/// whitenoise `MediaTypes.get_type`.
pub fn media_type(path: &Path) -> &'static str {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let find = |k: &str| super::media_types::TYPES.iter().find(|(n, _)| *n == k).map(|(_, t)| *t);
    if let Some(t) = find(&name) {
        return t;
    }
    // os.path.splitext: last dot, leading dots do not count
    let trimmed = name.trim_start_matches('.');
    let ext = match trimmed.rfind('.') {
        Some(i) => &trimmed[i..],
        None => "",
    };
    find(ext).unwrap_or("application/octet-stream")
}

/// `email.utils.formatdate(t, usegmt=True)`.
fn formatdate(secs: i64) -> Option<(String, NaiveDateTime)> {
    let dt = DateTime::from_timestamp(secs, 0)?;
    Some((dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string(), dt.naive_utc()))
}

/// `int(time.mktime(parsedate(last_modified)))`: the GMT fields read as *local* time
/// (Django sets `TZ = TIME_ZONE`).
fn mktime_local(naive: NaiveDateTime, tz: &chrono_tz::Tz) -> i64 {
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) => t.timestamp(),
        chrono::LocalResult::Ambiguous(a, _) => a.timestamp(),
        // DST gap: mktime moves forward by the gap
        chrono::LocalResult::None => tz
            .from_local_datetime(&(naive + chrono::Duration::hours(1)))
            .earliest()
            .map(|t| t.timestamp() - 3600)
            .unwrap_or_else(|| naive.and_utc().timestamp()),
    }
}

fn hex_i64(v: i64) -> String {
    if v < 0 { format!("-{:x}", v.unsigned_abs()) } else { format!("{v:x}") }
}

impl StaticFile {
    fn new(path: &Path, size: u64, mtime: Option<i64>, alternatives: Vec<Alt>, max_age: Option<u64>, tz: &chrono_tz::Tz) -> Self {
        let mut headers: Vec<(HeaderName, String)> = Vec::new();
        let mt = media_type(path);
        let ct = if mt.starts_with("text/") { format!("{mt}; charset=\"utf-8\"") } else { mt.to_owned() };
        headers.push((header::CONTENT_TYPE, ct));
        if let Some(age) = max_age {
            headers.push((header::CACHE_CONTROL, format!("max-age={age}, public")));
        }
        headers.push((header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".to_owned()));
        if alternatives.len() > 1 {
            headers.push((header::VARY, "Accept-Encoding".to_owned()));
        }
        let mut last_modified = None;
        let mut etag = String::new();
        if let Some((lm, naive)) = mtime.filter(|t| *t != 0).and_then(formatdate) {
            headers.push((header::LAST_MODIFIED, lm));
            last_modified = Some(naive);
            etag = format!("\"{}-{:x}\"", hex_i64(mktime_local(naive, tz)), size);
            headers.push((header::ETAG, etag.clone()));
        }
        let mut alternatives = alternatives;
        // sorted(files.items(), key=size): stable, primary file first among equals
        alternatives.sort_by_key(|a| a.size);
        Self { headers, etag, last_modified, alternatives }
    }

    fn base_headers(&self, out: &mut HeaderMap) {
        for (k, v) in &self.headers {
            if let Ok(v) = HeaderValue::from_str(v) {
                out.append(k.clone(), v);
            }
        }
    }

    fn not_modified_response(&self) -> Response {
        // NOT_MODIFIED_HEADERS order: Cache-Control, Content-Location, Date, ETag, Expires, Vary
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        for name in [header::CACHE_CONTROL, header::ETAG, header::VARY] {
            if let Some((_, v)) = self.headers.iter().find(|(k, _)| *k == name)
                && let Ok(v) = HeaderValue::from_str(v)
            {
                r.headers_mut().insert(name, v);
            }
        }
        r
    }

    fn is_not_modified(&self, req: &HeaderMap) -> bool {
        if let Some(prev) = header_str(req, &header::IF_NONE_MATCH) {
            return prev == self.etag;
        }
        let Some(lm) = self.last_modified else { return false };
        match header_str(req, &header::IF_MODIFIED_SINCE).and_then(|v| parsedate(&v)) {
            Some(t) => t >= lm,
            None => false,
        }
    }

    fn choose(&self, req: &HeaderMap) -> Option<&Alt> {
        let mut ae = header_str(req, &header::ACCEPT_ENCODING).unwrap_or_default();
        if ae == "*" {
            ae.clear();
        }
        self.alternatives.iter().find(|a| match a.encoding {
            None => true,
            Some(enc) => contains_word(&ae, enc),
        })
    }

    /// `StaticFile.get_response(method, request_headers)` (+ file body).
    pub async fn respond(&self, method: &Method, req: &HeaderMap) -> Response {
        let mut resp = self.respond_inner(method, req).await;
        resp.extensions_mut().insert(SecurityOnly);
        resp.extensions_mut().insert(crate::compress::Streaming);
        resp
    }

    async fn respond_inner(&self, method: &Method, req: &HeaderMap) -> Response {
        if *method != Method::GET && *method != Method::HEAD {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
            r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            return r;
        }
        if self.is_not_modified(req) {
            return self.not_modified_response();
        }
        let Some(alt) = self.choose(req) else {
            return pl_core::ApiError::internal("no file alternative").into_response();
        };
        let mut headers = HeaderMap::new();
        self.base_headers(&mut headers);
        let mut file = None;
        let mut mem: Option<bytes::Bytes> = None;
        if *method != Method::HEAD {
            match &alt.src {
                Src::File(path) => match tokio::fs::File::open(path).await {
                    Ok(f) => file = Some(f),
                    Err(e) => return pl_core::ApiError::internal(format!("static file vanished: {e}")).into_response(),
                },
                Src::Delta(d) => match d.bytes().await {
                    Ok(b) => mem = Some(b),
                    Err(e) => {
                        tracing::error!(file = %d.key, error = %e, "static delta file cannot be rebuilt");
                        return pl_core::ApiError::internal(format!("static file broken: {e}")).into_response();
                    }
                },
            }
        }
        let range = header_str(req, &header::RANGE).and_then(|r| parse_byte_range(&r));
        if let Some((start, end)) = range {
            let size = alt.size as i64;
            let start = if start < 0 { (start + size).max(0) } else { start };
            let end = match end {
                None => size - 1,
                Some(e) => e.min(size - 1),
            };
            if start >= end {
                let mut r = Response::new(Body::empty());
                *r.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                if let Ok(v) = HeaderValue::from_str(&format!("bytes */{size}")) {
                    r.headers_mut().insert(header::CONTENT_RANGE, v);
                }
                return r;
            }
            if let Some(enc) = alt.encoding {
                headers.append(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
            }
            if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")) {
                headers.append(header::CONTENT_RANGE, v);
            }
            let len = (end - start + 1) as u64;
            headers.append(header::CONTENT_LENGTH, HeaderValue::from(len));
            let body = match (file, mem) {
                (Some(mut f), _) => {
                    if let Err(e) = f.seek(SeekFrom::Start(start as u64)).await {
                        return pl_core::ApiError::internal(format!("seek: {e}")).into_response();
                    }
                    Body::from_stream(ReaderStream::new(f.take(len)))
                }
                (None, Some(b)) => Body::from(b.slice(start as usize..start as usize + len as usize)),
                (None, None) => Body::empty(),
            };
            let mut r = Response::new(body);
            *r.status_mut() = StatusCode::PARTIAL_CONTENT;
            *r.headers_mut() = headers;
            return r;
        }
        headers.append(header::CONTENT_LENGTH, HeaderValue::from(alt.size));
        if let Some(enc) = alt.encoding {
            headers.append(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
        }
        let body = match (file, mem) {
            (Some(f), _) => Body::from_stream(ReaderStream::new(f)),
            (None, Some(b)) => Body::from(b),
            (None, None) => Body::empty(),
        };
        let mut r = Response::new(body);
        *r.headers_mut() = headers;
        r
    }
}

/// Python `re.search(rf"\b{word}\b", s)` for an ASCII word.
fn contains_word(s: &str, word: &str) -> bool {
    let is_w = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    s.match_indices(word).any(|(i, _)| {
        let before = s[..i].chars().next_back();
        let after = s[i + word.len()..].chars().next();
        !is_w(before) && !is_w(after)
    })
}

/// `StaticFile.parse_byte_range`: `Some((start, end))`; `None` = ValueError (header ignored).
fn parse_byte_range(h: &str) -> Option<(i64, Option<i64>)> {
    let (units, spec) = h.trim().split_once('=')?;
    if units != "bytes" {
        return None;
    }
    let (s, e) = spec.trim().split_once('-')?;
    let int = |v: &str| v.trim().parse::<i64>().ok();
    if s.is_empty() {
        // Security review final S-N1: `bytes=--9223372036854775808` must not overflow (Python: +2^63 → 416; i64::MAX likewise)
        Some((int(e)?.checked_neg().unwrap_or(i64::MAX), None))
    } else {
        let start = int(s)?;
        let end = if e.is_empty() { None } else { Some(int(e)?) };
        Some((start, end))
    }
}

/// `email.utils.parsedate` (RFC 2822 date, zone ignored): naive date-time fields.
pub fn parsedate(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    // optional "Day," prefix
    let s = match s.split_once(',') {
        Some((day, rest)) if day.trim().len() == 3 => rest.trim(),
        _ => s,
    };
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let (d, m, y, t) = (parts[0], parts[1], parts[2], parts[3]);
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
        .iter()
        // Security review final S-N1: no byte slice (header values can contain non-ASCII,
        // `[..3]` in the middle of a character would be a panic)
        .position(|n| m.get(..3).is_some_and(|p| p.eq_ignore_ascii_case(n)))? as u32
        + 1;
    let day: u32 = d.parse().ok()?;
    let mut year: i32 = y.parse().ok()?;
    if y.len() <= 2 {
        year += if year > 68 { 1900 } else { 2000 };
    }
    let tp: Vec<&str> = t.split(':').collect();
    let (hh, mm, ss) = match tp.as_slice() {
        [h, m] => (h.parse().ok()?, m.parse().ok()?, 0),
        [h, m, s] => (h.parse().ok()?, m.parse().ok()?, s.parse().ok()?),
        _ => return None,
    };
    chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hh, mm, ss)
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, std::fs::Metadata)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        // os.scandir: is_dir() follows symlinks; files are stat()ed (following links).
        // Without a link the entry's own metadata is the same, and on Windows it comes from the
        // directory listing itself instead of one CreateFile per file (O1: ~20 000 files in
        // STATIC_ROOT took 6–9 s at startup, now ~0.5 s). Links (incl. junctions) are followed.
        let meta = match entry.file_type() {
            Ok(t) if !t.is_symlink() => entry.metadata(),
            _ => std::fs::metadata(&p),
        };
        match meta {
            Ok(m) if m.is_dir() => walk(&p, out),
            Ok(m) => out.push((p, m)),
            Err(_) => {}
        }
    }
}

fn mtime_secs(m: &std::fs::Metadata) -> Option<i64> {
    let t = m.modified().ok()?;
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => Some(d.as_secs() as i64),
        Err(e) => Some(-(e.duration().as_secs_f64().ceil() as i64)),
    }
}

/// PaperRust storage directory below `STATIC_ROOT` (never served).
const META_DIR: &str = ".paperrust";
const MANIFEST_FORMAT: &str = "paperrust-static/1";

#[derive(serde::Deserialize)]
struct Manifest {
    format: String,
    #[serde(default)]
    alias: Vec<AliasGroup>,
    #[serde(default)]
    delta: HashMap<String, DeltaEntry>,
}

#[derive(serde::Deserialize)]
struct AliasGroup {
    dir: String,
    to: String,
    files: Vec<String>,
}

#[derive(serde::Deserialize)]
struct DeltaEntry {
    base: String,
    base_size: u64,
    file: String,
    size: u64,
    fnv1a32: String,
}

/// `root` + a manifest path (`/`-separated, relative, no `.`/`..`/drive/backslash).
fn manifest_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for c in rel.split('/') {
        if c.is_empty() || c == "." || c == ".." || c.contains(['\\', ':']) {
            return None;
        }
        p.push(c);
    }
    Some(p)
}

/// Virtual files of `<root>/.paperrust/static.json` (none without manifest). Entries whose stored
/// file is missing, or that collide with a real file, are skipped with a warning.
fn virtual_files(root: &Path, real: &HashMap<PathBuf, &std::fs::Metadata>) -> Vec<(PathBuf, u64, Option<i64>, Src)> {
    let mpath = root.join(META_DIR).join("static.json");
    let Ok(text) = std::fs::read_to_string(&mpath) else { return Vec::new() };
    let m: Manifest = match serde_json::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(file = %mpath.display(), error = %e, "static storage manifest unreadable; deduplicated files are missing");
            return Vec::new();
        }
    };
    if m.format != MANIFEST_FORMAT {
        tracing::error!(file = %mpath.display(), format = %m.format, "unknown static storage manifest format; deduplicated files are missing");
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for g in &m.alias {
        for f in &g.files {
            let (Some(p), Some(t)) = (manifest_path(root, &format!("{}{f}", g.dir)), manifest_path(root, &format!("{}{f}", g.to))) else {
                skipped += 1;
                continue;
            };
            match real.get(&t) {
                Some(meta) if !real.contains_key(&p) => out.push((p, meta.len(), mtime_secs(meta), Src::File(t))),
                // precompressed variants removed on purpose (desktop package ships without .gz/.br)
                None if f.ends_with(".gz") || f.ends_with(".br") => {}
                _ => skipped += 1,
            }
        }
    }
    let cache = Arc::new(DeltaCache::default());
    let mut keys: Vec<&String> = m.delta.keys().collect();
    keys.sort();
    for key in keys {
        let d = &m.delta[key];
        let paths = (manifest_path(root, key), manifest_path(root, &d.base), manifest_path(root, &d.file));
        let (Some(p), Some(base), Some(file)) = paths else {
            skipped += 1;
            continue;
        };
        let file_meta = std::fs::metadata(&file).ok().filter(|m| m.is_file());
        let (Some(fm), Ok(fnv)) = (file_meta, u32::from_str_radix(&d.fnv1a32, 16)) else {
            skipped += 1;
            continue;
        };
        if real.contains_key(&p) || !real.contains_key(&base) {
            skipped += 1;
            continue;
        }
        let delta = Delta {
            key: key.clone(),
            base,
            base_size: d.base_size,
            file,
            size: d.size,
            fnv1a32: fnv,
            cache: cache.clone(),
        };
        out.push((p, d.size, mtime_secs(&fm), Src::Delta(Arc::new(delta))));
    }
    if skipped > 0 {
        tracing::warn!(file = %mpath.display(), skipped, "static storage manifest: entries without stored file (or shadowed by a real file) skipped");
    }
    out
}

impl StaticFiles {
    /// `WhiteNoise.add_files(STATIC_ROOT, prefix="/static/")` (missing directory → empty, warning).
    pub fn scan(root: &Path, max_age: Option<u64>, tz: &chrono_tz::Tz) -> Self {
        let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
        let mut entries = Vec::new();
        if root.is_dir() {
            walk(&root, &mut entries);
        } else {
            tracing::warn!(dir = %root.display(), "No directory at STATIC_ROOT (PAPERRUST_STATICDIR); /static/ serves nothing");
        }
        // (path, size, mtime, source): real files except `.paperrust/`, then the manifest's virtual files
        let meta_dir = root.join(META_DIR);
        let visible: Vec<&(PathBuf, std::fs::Metadata)> = entries.iter().filter(|(p, _)| !p.starts_with(&meta_dir)).collect();
        let real: HashMap<PathBuf, &std::fs::Metadata> = visible.iter().map(|(p, m)| (p.clone(), m)).collect();
        let mut all: Vec<(PathBuf, u64, Option<i64>, Src)> =
            visible.iter().map(|(p, m)| (p.clone(), m.len(), mtime_secs(m), Src::File(p.clone()))).collect();
        all.extend(virtual_files(&root, &real));
        let stats: HashMap<PathBuf, usize> = all.iter().enumerate().map(|(i, (p, ..))| (p.clone(), i)).collect();
        let mut files = HashMap::new();
        for (path, size, mtime, src) in &all {
            let s = path.to_string_lossy();
            if (s.ends_with(".gz") || s.ends_with(".br")) && stats.contains_key(&PathBuf::from(&s[..s.len() - 3])) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&root) else { continue };
            let url = format!("{PREFIX}{}", rel.to_string_lossy().replace('\\', "/"));
            let mut alts = vec![Alt { encoding: None, src: src.clone(), size: *size }];
            for enc in ["gzip", "br"] {
                let alt_path = PathBuf::from(format!("{s}.{}", if enc == "gzip" { "gz" } else { "br" }));
                if let Some(&i) = stats.get(&alt_path) {
                    alts.push(Alt { encoding: Some(enc), src: all[i].3.clone(), size: all[i].1 });
                }
            }
            let f = StaticFile::new(path, *size, *mtime, alts, max_age, tz);
            files.insert(url, Arc::new(f));
        }
        Self { files, root }
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Lookup by decoded request path (`request.path_info`).
    pub fn get(&self, path: &str) -> Option<Arc<StaticFile>> {
        self.files.get(path).cloned()
    }
}

/// The `STATIC_ROOT` scan running in the background (O1: the server listens before the file list
/// is complete; only `/static/…` requests wait for it, API requests do not).
#[derive(Clone)]
pub struct StaticScan(futures::future::Shared<futures::future::BoxFuture<'static, Arc<StaticFiles>>>);

impl StaticScan {
    /// Start [`StaticFiles::scan`] on the blocking pool (needs a Tokio runtime).
    pub fn spawn(root: PathBuf, max_age: Option<u64>, tz: chrono_tz::Tz) -> Self {
        use futures::FutureExt as _;
        let task = tokio::task::spawn_blocking(move || {
            let t0 = std::time::Instant::now();
            let files = StaticFiles::scan(&root, max_age, &tz);
            tracing::info!(dir = %files.root.display(), files = files.len(), ms = t0.elapsed().as_millis() as u64, "static files (STATIC_ROOT) scanned");
            Arc::new(files)
        });
        Self(
            async move {
                task.await.unwrap_or_else(|e| {
                    tracing::error!(error = %e, "static files scan failed; /static/ serves nothing");
                    Arc::new(StaticFiles::default())
                })
            }
            .boxed()
            .shared(),
        )
    }

    /// The file list (waits for the scan the first time).
    pub async fn files(&self) -> Arc<StaticFiles> {
        self.0.clone().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_types_like_whitenoise() {
        assert_eq!(media_type(Path::new("a/main.js")), "text/javascript");
        assert_eq!(media_type(Path::new("a/X.CSS")), "text/css");
        assert_eq!(media_type(Path::new("m.webmanifest")), "application/octet-stream");
        assert_eq!(media_type(Path::new("crossdomain.xml")), "text/x-cross-domain-policy");
        assert_eq!(media_type(Path::new(".htaccess")), "application/octet-stream");
        assert_eq!(media_type(Path::new("noext")), "application/octet-stream");
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_byte_range("bytes=0-9"), Some((0, Some(9))));
        assert_eq!(parse_byte_range("bytes=-5"), Some((-5, None)));
        assert_eq!(parse_byte_range("bytes=5-"), Some((5, None)));
        assert_eq!(parse_byte_range("bytes=5"), None);
        assert_eq!(parse_byte_range("items=0-1"), None);
        assert_eq!(parse_byte_range("bytes=0-1,3-4"), None);
        // S-N1: no overflow when negating
        assert_eq!(parse_byte_range("bytes=--9223372036854775808"), Some((i64::MAX, None)));
        assert_eq!(parse_byte_range("bytes=-9223372036854775807"), Some((-9223372036854775807, None)));
    }

    #[test]
    fn words() {
        assert!(contains_word("gzip, deflate, br", "br"));
        assert!(contains_word("gzip", "gzip"));
        assert!(!contains_word("xgzip", "gzip"));
        assert!(!contains_word("brotli", "br"));
    }

    #[test]
    fn dates_and_etag() {
        let n = parsedate("Sat, 07 Nov 2026 23:01:28 GMT").unwrap();
        assert_eq!(n.to_string(), "2026-11-07 23:01:28");
        assert_eq!(parsedate("07 Nov 2026 23:01 +0100").unwrap().to_string(), "2026-11-07 23:01:00");
        assert!(parsedate("garbage").is_none());
        // S-N1: non-ASCII (obs-text, decoded as Latin-1) → ignored instead of a panic
        assert!(parsedate("01 \u{e4}\u{e4}\u{e4} 2026 10:00:00").is_none());
        assert!(parsedate("Sat, 07 N\u{e4}v 2026 10:00:00 GMT").is_none());
        assert!(parsedate("07 \u{e4}N 2026 10:00").is_none());
        // reference: Last-Modified Sat, 07 Nov 2026 23:01:28 GMT, TZ Europe/Zurich → 6aef9fb8
        let tz: chrono_tz::Tz = "Europe/Zurich".parse().unwrap();
        assert_eq!(hex_i64(mktime_local(n, &tz)), "6aef9fb8");
        let (lm, _) = formatdate(1_794_092_488).unwrap();
        assert_eq!(lm, "Sat, 07 Nov 2026 23:01:28 GMT");
    }

    #[tokio::test]
    async fn serve_alternatives_ranges_and_304() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("frontend");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("main.js"), vec![b'a'; 1000]).unwrap();
        std::fs::write(sub.join("main.js.br"), vec![b'b'; 100]).unwrap();
        std::fs::write(sub.join("main.js.gz"), vec![b'g'; 200]).unwrap();
        std::fs::write(sub.join("x.png"), vec![1u8; 10]).unwrap();
        let tz: chrono_tz::Tz = "UTC".parse().unwrap();
        let sf = StaticFiles::scan(dir.path(), Some(60), &tz);
        assert_eq!(sf.len(), 2);
        assert!(sf.get("/static/frontend/main.js.br").is_none());
        let f = sf.get("/static/frontend/main.js").unwrap();
        let mut h = HeaderMap::new();
        h.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate, br, zstd"));
        let r = f.respond(&Method::GET, &h).await;
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(r.headers()[header::CONTENT_LENGTH], "100");
        assert_eq!(r.headers()[header::CONTENT_TYPE], "text/javascript; charset=\"utf-8\"");
        assert_eq!(r.headers()[header::CACHE_CONTROL], "max-age=60, public");
        assert_eq!(r.headers()[header::VARY], "Accept-Encoding");
        let etag = r.headers()[header::ETAG].to_str().unwrap().to_owned();
        let mut h2 = HeaderMap::new();
        h2.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        let r = f.respond(&Method::GET, &h2).await;
        assert_eq!(r.headers()[header::CONTENT_ENCODING], "gzip");
        let r = f.respond(&Method::GET, &HeaderMap::new()).await;
        assert!(r.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(r.headers()[header::CONTENT_LENGTH], "1000");
        let mut h3 = HeaderMap::new();
        h3.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&etag).unwrap());
        let r = f.respond(&Method::GET, &h3).await;
        assert_eq!(r.status(), 304);
        assert!(r.headers().get(header::LAST_MODIFIED).is_none());
        let mut h4 = HeaderMap::new();
        h4.insert(header::RANGE, HeaderValue::from_static("bytes=0-9"));
        let r = f.respond(&Method::GET, &h4).await;
        assert_eq!(r.status(), 206);
        assert_eq!(r.headers()[header::CONTENT_RANGE], "bytes 0-9/1000");
        assert_eq!(r.headers()[header::CONTENT_LENGTH], "10");
        h4.insert(header::RANGE, HeaderValue::from_static("bytes=5-5"));
        assert_eq!(f.respond(&Method::GET, &h4).await.status(), 416);
        let r = f.respond(&Method::POST, &HeaderMap::new()).await;
        assert_eq!(r.status(), 405);
        assert_eq!(r.headers()[header::ALLOW], "GET, HEAD");
        let png = sf.get("/static/frontend/x.png").unwrap();
        let r = png.respond(&Method::GET, &h).await;
        assert!(r.headers().get(header::VARY).is_none());
        assert_eq!(r.headers()[header::CONTENT_TYPE], "image/png");
    }

    async fn body(r: Response) -> Vec<u8> {
        axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    /// `PRD1` delta in the format of `frontend/tools/static-dedupe.mjs`.
    fn prd(ops: &[(u32, u32, &[u8])]) -> Vec<u8> {
        let mut d = b"PRD1".to_vec();
        for (off, len, ins) in ops {
            if ins.is_empty() {
                d.push(1);
                d.extend_from_slice(&off.to_le_bytes());
                d.extend_from_slice(&len.to_le_bytes());
            } else {
                d.push(2);
                d.extend_from_slice(&(ins.len() as u32).to_le_bytes());
                d.extend_from_slice(ins);
            }
        }
        d
    }

    #[test]
    fn fnv_and_delta_format() {
        // same value as static-dedupe.mjs fnv1a32("hello")
        assert_eq!(fnv1a32(b"hello"), 0x4f9f_2cab);
        let base = b"0123456789";
        assert_eq!(apply_delta(base, &prd(&[(0, 3, b""), (0, 0, b"XY"), (7, 3, b"")]), 100).unwrap(), b"012XY789");
        assert!(apply_delta(base, &prd(&[(8, 3, b"")]), 100).is_err()); // outside base
        assert!(apply_delta(base, b"PRD1\x02\xff\x00\x00\x00ab", 100).is_err()); // truncated insert
        assert!(apply_delta(base, b"PRD1\x01\x00", 100).is_err());
        assert!(apply_delta(base, b"XXXX", 100).is_err());
        assert!(apply_delta(base, &prd(&[(0, 10, b""), (0, 10, b"")]), 15).is_err()); // larger than manifest
        assert!(manifest_path(Path::new("r"), "a/../b").is_none());
        assert!(manifest_path(Path::new("r"), "/a").is_none());
        assert!(manifest_path(Path::new("r"), "C:/a").is_none());
        assert!(manifest_path(Path::new("r"), r"a\b").is_none());
        assert_eq!(manifest_path(Path::new("r"), "a/b").unwrap(), Path::new("r").join("a").join("b"));
    }

    #[tokio::test]
    async fn deduplicated_storage_serves_like_real_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let en = root.join("frontend").join("en-US");
        let de = root.join("frontend").join("de-DE");
        std::fs::create_dir_all(en.join("assets")).unwrap();
        std::fs::create_dir_all(&de).unwrap();
        let base: Vec<u8> = (0..4000u32).map(|i| b"abcdefghij"[(i % 10) as usize]).collect();
        let mut target = base[..1000].to_vec();
        target.extend_from_slice("Schließen".as_bytes());
        target.extend_from_slice(&base[1000..]);
        std::fs::write(en.join("main.js"), &base).unwrap();
        std::fs::write(en.join("assets").join("pdf.mjs"), b"PDFJS").unwrap();
        std::fs::write(en.join("assets").join("pdf.mjs.gz"), b"gz").unwrap();
        std::fs::write(de.join("main.js.br"), b"brotli").unwrap();
        let meta = root.join(".paperrust");
        std::fs::create_dir_all(meta.join("delta")).unwrap();
        let ins = "Schließen".as_bytes();
        std::fs::write(meta.join("delta").join("de.prd"), prd(&[(0, 1000, b""), (0, 0, ins), (1000, 3000, b"")])).unwrap();
        let manifest = serde_json::json!({
            "format": "paperrust-static/1",
            "alias": [{"dir": "frontend/de-DE/", "to": "frontend/en-US/", "files": ["assets/pdf.mjs", "assets/pdf.mjs.gz", "assets/missing.js", "../x"]}],
            "delta": {
                "frontend/de-DE/main.js": {"base": "frontend/en-US/main.js", "base_size": 4000, "file": ".paperrust/delta/de.prd",
                    "size": target.len(), "fnv1a32": format!("{:08x}", fnv1a32(&target)), "sha256": "-"},
                "frontend/fr-FR/main.js": {"base": "frontend/en-US/main.js", "base_size": 4000, "file": ".paperrust/delta/fr.prd",
                    "size": 1, "fnv1a32": "0", "sha256": "-"}
            }
        });
        std::fs::write(meta.join("static.json"), manifest.to_string()).unwrap();
        let tz: chrono_tz::Tz = "UTC".parse().unwrap();
        let sf = StaticFiles::scan(root, Some(60), &tz);
        // en main.js, en pdf.mjs, de pdf.mjs, de main.js; nothing below .paperrust/, no stray .gz/.br
        assert_eq!(sf.len(), 4, "{:?}", sf.files.keys().collect::<Vec<_>>());
        assert!(sf.get("/static/.paperrust/static.json").is_none());
        assert!(sf.get("/static/frontend/de-DE/main.js.br").is_none());
        assert!(sf.get("/static/frontend/fr-FR/main.js").is_none());

        // alias: same bytes, same headers (incl. ETag and Vary from the aliased .gz)
        let a = sf.get("/static/frontend/en-US/assets/pdf.mjs").unwrap();
        let b = sf.get("/static/frontend/de-DE/assets/pdf.mjs").unwrap();
        assert_eq!(a.headers, b.headers);
        let r = b.respond(&Method::GET, &HeaderMap::new()).await;
        assert_eq!(r.headers()[header::VARY], "Accept-Encoding");
        assert_eq!(body(r).await, b"PDFJS");
        let mut gz = HeaderMap::new();
        gz.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        let r = b.respond(&Method::GET, &gz).await;
        assert_eq!(r.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(body(r).await, b"gz");

        // delta: rebuilt identity body, precompressed alternative from disk, ranges, HEAD
        let m = sf.get("/static/frontend/de-DE/main.js").unwrap();
        let r = m.respond(&Method::GET, &HeaderMap::new()).await;
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()[header::CONTENT_LENGTH], target.len().to_string().as_str());
        assert_eq!(r.headers()[header::CONTENT_TYPE], "text/javascript; charset=\"utf-8\"");
        assert!(r.headers()[header::ETAG].to_str().unwrap().ends_with(&format!("-{:x}\"", target.len())));
        assert_eq!(body(r).await, target);
        let r = m.respond(&Method::GET, &HeaderMap::new()).await; // cached
        assert_eq!(body(r).await, target);
        let mut br = HeaderMap::new();
        br.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate, br"));
        let r = m.respond(&Method::GET, &br).await;
        assert_eq!(r.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(body(r).await, b"brotli");
        let mut rg = HeaderMap::new();
        rg.insert(header::RANGE, HeaderValue::from_static("bytes=998-1003"));
        let r = m.respond(&Method::GET, &rg).await;
        assert_eq!(r.status(), 206);
        assert_eq!(body(r).await, &target[998..=1003]);
        let r = m.respond(&Method::HEAD, &HeaderMap::new()).await;
        assert_eq!(r.headers()[header::CONTENT_LENGTH], target.len().to_string().as_str());

        // broken storage → 500 (like a vanished file), never wrong bytes
        let sf2 = StaticFiles::scan(root, Some(60), &tz);
        std::fs::write(en.join("main.js"), &base[..3999]).unwrap();
        let r = sf2.get("/static/frontend/de-DE/main.js").unwrap().respond(&Method::GET, &HeaderMap::new()).await;
        assert_eq!(r.status(), 500);
    }
}
