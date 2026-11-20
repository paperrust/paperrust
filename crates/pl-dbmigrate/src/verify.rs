//! Verification report of the switchover (package Z1): object counts per kind source ↔ target, checksums of every
//! original/archive file against the database, missing/surplus files, comparison of every
//! document row (title, content hash, tags, checksums), sample via API v1 and a verdict
//! OK / WARN / FAIL. Everything read-only.
//!
//! upstream: src/documents/models.py (`Document.source_path` = ORIGINALS_DIR/filename,
//! `archive_path` = ARCHIVE_DIR/archive_filename, `thumbnail_path` = THUMBNAIL_DIR/{pk:07}.webp;
//! `checksum` max_length 64 → SHA-256, older collections MD5 with 32 characters),
//! src/documents/sanity_checker.py (same checks: missing, checksum, orphaned).

use crate::export::{DocRow, ExportSource, ManifestSummary};
use crate::source::Source;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Ok => "OK",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        }
    }
}

pub struct ObjectType {
    pub key: &'static str,
    pub label: &'static str,
    pub table: &'static str,
    pub model: &'static str,
    pub cond: Option<&'static str>,
}

const fn o(
    key: &'static str,
    label: &'static str,
    table: &'static str,
    model: &'static str,
) -> ObjectType {
    ObjectType {
        key,
        label,
        table,
        model,
        cond: None,
    }
}

/// Counted object kinds. Users like the upstream exporter without `consumer`/`AnonymousUser`.
pub const OBJECTS: &[ObjectType] = &[
    o(
        "documents",
        "Dokumente",
        "documents_document",
        "documents.document",
    ),
    o(
        "document_tag_links",
        "Tag-Zuordnungen",
        "documents_document_tags",
        "documents.document_tags",
    ),
    o("tags", "Tags", "documents_tag", "documents.tag"),
    o(
        "correspondents",
        "Korrespondenten",
        "documents_correspondent",
        "documents.correspondent",
    ),
    o(
        "document_types",
        "Dokumenttypen",
        "documents_documenttype",
        "documents.documenttype",
    ),
    o(
        "storage_paths",
        "Speicherpfade",
        "documents_storagepath",
        "documents.storagepath",
    ),
    o(
        "custom_fields",
        "Benutzerdefinierte Felder",
        "documents_customfield",
        "documents.customfield",
    ),
    o(
        "custom_field_instances",
        "Feldwerte",
        "documents_customfieldinstance",
        "documents.customfieldinstance",
    ),
    o("notes", "Notizen", "documents_note", "documents.note"),
    o(
        "saved_views",
        "Gespeicherte Ansichten",
        "documents_savedview",
        "documents.savedview",
    ),
    o(
        "workflows",
        "Workflows",
        "documents_workflow",
        "documents.workflow",
    ),
    o(
        "workflow_triggers",
        "Workflow-Auslöser",
        "documents_workflowtrigger",
        "documents.workflowtrigger",
    ),
    o(
        "workflow_actions",
        "Workflow-Aktionen",
        "documents_workflowaction",
        "documents.workflowaction",
    ),
    ObjectType {
        key: "users",
        label: "Benutzer",
        table: "auth_user",
        model: "auth.user",
        cond: Some("username NOT IN ('consumer', 'AnonymousUser')"),
    },
    o("groups", "Gruppen", "auth_group", "auth.group"),
    o(
        "mail_accounts",
        "E-Mail-Konten",
        "paperless_mail_mailaccount",
        "paperless_mail.mailaccount",
    ),
    o(
        "mail_rules",
        "E-Mail-Regeln",
        "paperless_mail_mailrule",
        "paperless_mail.mailrule",
    ),
    o(
        "share_links",
        "Freigabelinks",
        "documents_sharelink",
        "documents.sharelink",
    ),
    o(
        "share_link_bundles",
        "Freigabe-Bündel",
        "documents_sharelinkbundle",
        "documents.sharelinkbundle",
    ),
];

pub type Counts = BTreeMap<&'static str, i64>;

/// Checksums equal? MD5 (32 characters, paperless-ngx 2.x) against SHA-256 (64, after the conversion like
/// upstream documents.0016) is not a contradiction: whether the SHA-256 matches the file is checked by the
/// file check of the target.
pub fn same_checksum(source: &str, target: &str) -> bool {
    source.eq_ignore_ascii_case(target) || (source.len() == 32 && target.len() == 64)
}

/// Counts per object kind from a database (tables that do not exist there are missing).
pub async fn db_counts(src: &mut Source) -> Result<Counts> {
    let tables = src.tables().await?;
    let mut out = Counts::new();
    for o in OBJECTS {
        if tables.contains(o.table) {
            out.insert(o.key, src.count_where(o.table, o.cond).await?);
        }
    }
    Ok(out)
}

pub fn export_counts(sum: &ManifestSummary) -> Counts {
    OBJECTS
        .iter()
        .map(|o| (o.key, sum.counts.get(o.model).copied().unwrap_or(0)))
        .collect()
}

fn truthy(s: Option<&str>) -> bool {
    matches!(s, Some("1" | "t" | "true"))
}

/// All document rows of a database (content only as SHA-256), streamed.
pub async fn db_docs(src: &mut Source) -> Result<Vec<DocRow>> {
    let t = |s: &Source, e: &str| s.text(e);
    // paperless-ngx 2.x does not know versions yet (documents.0012/0015 come with 3.0)
    let cols: Vec<String> = src.columns("documents_document").await?.into_iter().map(|(n, _)| n).collect();
    let opt = |s: &Source, c: &str| if cols.iter().any(|x| x == c) { t(s, c) } else { "NULL".to_string() };
    let sql = format!(
        "SELECT {}, title, content, checksum, archive_checksum, filename, archive_filename, {}, {}, {} FROM documents_document",
        t(src, "id"),
        t(src, "CASE WHEN deleted_at IS NULL THEN 0 ELSE 1 END"),
        opt(src, "root_document_id"),
        opt(src, "version_index")
    );
    let mut docs = Vec::new();
    src.for_each_text_row(&sql, |r| {
        let mut r = r.into_iter();
        let mut next = || r.next().flatten();
        docs.push(DocRow {
            id: next().and_then(|s| s.parse().ok()).unwrap_or(-1),
            title: next().unwrap_or_default(),
            content_sha256: crate::export::sha256_hex(next().unwrap_or_default().as_bytes()),
            checksum: next().unwrap_or_default(),
            archive_checksum: next().filter(|s| !s.is_empty()),
            filename: next().filter(|s| !s.is_empty()),
            archive_filename: next().filter(|s| !s.is_empty()),
            thumbnail: None,
            deleted: truthy(next().as_deref()),
            tags: vec![],
            root: next().and_then(|s| s.parse().ok()),
            version_index: next().and_then(|s| s.parse().ok()),
        });
    })
    .await?;
    let mut tags: HashMap<i64, Vec<i64>> = HashMap::new();
    let sql = format!(
        "SELECT {}, {} FROM documents_document_tags",
        t(src, "document_id"),
        t(src, "tag_id")
    );
    src.for_each_text_row(&sql, |r| {
        if let [Some(d), Some(tg)] = r.as_slice()
            && let (Ok(d), Ok(tg)) = (d.parse(), tg.parse())
        {
            tags.entry(d).or_default().push(tg);
        }
    })
    .await?;
    for d in &mut docs {
        if let Some(mut t) = tags.remove(&d.id) {
            t.sort_unstable();
            d.tags = t;
        }
    }
    docs.sort_by_key(|d| d.id);
    Ok(docs)
}

// ---------------------------------------------------------------------------------------------
// Files

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    Original,
    Archive,
    Thumbnail,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Original => "original",
            FileKind::Archive => "archive",
            FileKind::Thumbnail => "thumbnail",
        }
    }
    fn label(self) -> &'static str {
        match self {
            FileKind::Original => "Original",
            FileKind::Archive => "Archivdatei",
            FileKind::Thumbnail => "Vorschaubild",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileIssue {
    Missing,
    Mismatch { expected: String, actual: String },
    NoFilename,
    Unreadable(String),
}

impl FileIssue {
    fn code(&self) -> &'static str {
        match self {
            FileIssue::Missing => "missing",
            FileIssue::Mismatch { .. } => "checksum-mismatch",
            FileIssue::NoFilename => "no-filename",
            FileIssue::Unreadable(_) => "unreadable",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileProblem {
    pub doc: i64,
    pub kind: FileKind,
    pub path: String,
    pub issue: FileIssue,
}

impl FileProblem {
    pub fn describe(&self) -> String {
        let what = format!("Dokument {}: {}", self.doc, self.kind.label());
        match &self.issue {
            FileIssue::Missing => format!("{what} fehlt ({})", self.path),
            FileIssue::Mismatch { expected, actual } => {
                format!(
                    "{what} hat falsche Prüfsumme ({}: erwartet {expected}, Datei {actual})",
                    self.path
                )
            }
            FileIssue::NoFilename => format!("{what}: kein Dateiname in Datenbank/Manifest"),
            FileIssue::Unreadable(e) => format!("{what} nicht lesbar ({}): {e}", self.path),
        }
    }
    fn to_json(&self) -> Value {
        let mut v = json!({ "document": self.doc, "kind": self.kind.as_str(), "path": self.path, "issue": self.issue.code() });
        match &self.issue {
            FileIssue::Mismatch { expected, actual } => {
                v["expected"] = json!(expected);
                v["actual"] = json!(actual);
            }
            FileIssue::Unreadable(e) => v["error"] = json!(e),
            _ => {}
        }
        v
    }
}

#[derive(Debug, Clone, Default)]
pub struct FileSummary {
    pub location: String,
    pub originals: u64,
    pub archives: u64,
    pub thumbnails: u64,
    pub ok: u64,
    pub bytes: u64,
    pub md5: u64,
    pub sha256: u64,
    pub problems: Vec<FileProblem>,
}

impl FileSummary {
    fn to_json(&self) -> Value {
        json!({
            "location": self.location,
            "originals_checked": self.originals,
            "archives_checked": self.archives,
            "thumbnails_checked": self.thumbnails,
            "ok": self.ok,
            "bytes": self.bytes,
            "md5_checksums": self.md5,
            "sha256_checksums": self.sha256,
            "problems": self.problems.len(),
            "problem_list": self.problems.iter().take(MAX_LIST).map(FileProblem::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Where the files are.
pub enum Files<'a> {
    /// `MEDIA_ROOT` (documents/originals, documents/archive, documents/thumbnails)
    Media(&'a Path),
    /// Export: paths from the `__exported_*` keys of the manifest
    Export(&'a ExportSource),
}

enum Hasher {
    Md5(md5::Md5),
    Sha256(sha2::Sha256),
}

/// Checksum of the file using the method that matches the stored checksum (32 characters = MD5,
/// otherwise SHA-256).
fn hash_reader(mut r: impl Read, expected: &str) -> std::io::Result<(String, u64, bool)> {
    use sha2::Digest;
    let md5 = expected.len() == 32;
    let mut h = if md5 {
        Hasher::Md5(md5::Md5::new())
    } else {
        Hasher::Sha256(sha2::Sha256::new())
    };
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let k = r.read(&mut buf)?;
        if k == 0 {
            break;
        }
        n += k as u64;
        match &mut h {
            Hasher::Md5(h) => h.update(&buf[..k]),
            Hasher::Sha256(h) => h.update(&buf[..k]),
        }
    }
    let hex = match h {
        Hasher::Md5(h) => crate::value::hex(&h.finalize()),
        Hasher::Sha256(h) => crate::value::hex(&h.finalize()),
    };
    Ok((hex, n, md5))
}

struct Task {
    doc: i64,
    kind: FileKind,
    rel: String,
    /// None = only check existence
    expected: Option<String>,
}

fn media_path(media: &Path, kind: FileKind, rel: &str) -> std::path::PathBuf {
    let sub = match kind {
        FileKind::Original => "originals",
        FileKind::Archive => "archive",
        FileKind::Thumbnail => "thumbnails",
    };
    media.join("documents").join(sub).join(rel)
}

/// Check all files of the documents (in parallel with `threads` readers).
pub fn check_files(files: &Files, docs: &[DocRow], threads: usize) -> FileSummary {
    let mut sum = FileSummary {
        location: match files {
            Files::Media(m) => m.display().to_string(),
            Files::Export(_) => "Export".into(),
        },
        ..Default::default()
    };
    let mut tasks = Vec::new();
    for d in docs {
        let (orig, arch, thumb) = match files {
            Files::Media(_) => (d.filename.clone(), d.archive_filename.clone(), Some(None)),
            Files::Export(_) => (
                d.filename.clone(),
                d.archive_filename.clone(),
                d.thumbnail.clone().map(Some),
            ),
        };
        match orig {
            Some(rel) => tasks.push(Task {
                doc: d.id,
                kind: FileKind::Original,
                rel,
                expected: Some(d.checksum.clone()),
            }),
            None => sum.problems.push(FileProblem {
                doc: d.id,
                kind: FileKind::Original,
                path: String::new(),
                issue: FileIssue::NoFilename,
            }),
        }
        if d.archive_checksum.is_some() || arch.is_some() {
            match arch {
                Some(rel) => tasks.push(Task {
                    doc: d.id,
                    kind: FileKind::Archive,
                    rel,
                    expected: Some(d.archive_checksum.clone().unwrap_or_default()),
                }),
                None => sum.problems.push(FileProblem {
                    doc: d.id,
                    kind: FileKind::Archive,
                    path: String::new(),
                    issue: FileIssue::NoFilename,
                }),
            }
        }
        // Thumbnail: media folder always ({pk:07}.webp, older .png); export only if exported
        match thumb {
            Some(Some(rel)) => tasks.push(Task {
                doc: d.id,
                kind: FileKind::Thumbnail,
                rel,
                expected: None,
            }),
            Some(None) => tasks.push(Task {
                doc: d.id,
                kind: FileKind::Thumbnail,
                rel: format!("{:07}.webp", d.id),
                expected: None,
            }),
            None => {}
        }
    }
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::<(usize, Result<Option<(String, u64, bool)>, FileIssue>)>::new());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(t) = tasks.get(i) else { break };
                    let r = check_one(files, t);
                    out.lock().unwrap_or_else(|e| e.into_inner()).push((i, r));
                }
            });
        }
    });
    let mut results = out.into_inner().unwrap_or_else(|e| e.into_inner());
    results.sort_by_key(|(i, _)| *i);
    for (i, r) in results {
        let t = &tasks[i];
        match t.kind {
            FileKind::Original => sum.originals += 1,
            FileKind::Archive => sum.archives += 1,
            FileKind::Thumbnail => sum.thumbnails += 1,
        }
        let path = match files {
            Files::Media(_) => format!(
                "documents/{}/{}",
                match t.kind {
                    FileKind::Original => "originals",
                    FileKind::Archive => "archive",
                    FileKind::Thumbnail => "thumbnails",
                },
                t.rel
            ),
            Files::Export(_) => t.rel.clone(),
        };
        match r {
            Ok(Some((actual, n, md5))) => {
                sum.bytes += n;
                if md5 {
                    sum.md5 += 1;
                } else {
                    sum.sha256 += 1;
                }
                let expected = t.expected.clone().unwrap_or_default();
                if actual.eq_ignore_ascii_case(&expected) {
                    sum.ok += 1;
                } else {
                    sum.problems.push(FileProblem {
                        doc: t.doc,
                        kind: t.kind,
                        path,
                        issue: FileIssue::Mismatch { expected, actual },
                    });
                }
            }
            Ok(None) => sum.ok += 1,
            Err(issue) => sum.problems.push(FileProblem {
                doc: t.doc,
                kind: t.kind,
                path,
                issue,
            }),
        }
    }
    sum.problems
        .sort_by(|a, b| (a.doc, a.kind.as_str()).cmp(&(b.doc, b.kind.as_str())));
    sum
}

fn check_one(files: &Files, t: &Task) -> Result<Option<(String, u64, bool)>, FileIssue> {
    match files {
        Files::Media(m) => {
            let mut p = media_path(m, t.kind, &t.rel);
            if t.kind == FileKind::Thumbnail && !p.is_file() {
                p.set_extension("png");
            }
            if !p.is_file() {
                return Err(FileIssue::Missing);
            }
            let Some(exp) = &t.expected else {
                return Ok(None);
            };
            let f = std::fs::File::open(&p).map_err(|e| FileIssue::Unreadable(e.to_string()))?;
            hash_reader(f, exp)
                .map(Some)
                .map_err(|e| FileIssue::Unreadable(e.to_string()))
        }
        Files::Export(e) => {
            if !e.exists(&t.rel) {
                return Err(FileIssue::Missing);
            }
            let Some(exp) = &t.expected else {
                return Ok(None);
            };
            let r = e
                .open_file(&t.rel)
                .map_err(|e| FileIssue::Unreadable(format!("{e:#}")))?;
            hash_reader(r, exp)
                .map(Some)
                .map_err(|e| FileIssue::Unreadable(e.to_string()))
        }
    }
}

/// Files under documents/originals and documents/archive that no document points to.
pub fn extra_files(media: &Path, docs: &[DocRow]) -> Vec<String> {
    let mut want: HashSet<String> = HashSet::new();
    for d in docs {
        if let Some(f) = &d.filename {
            want.insert(format!("originals/{}", f.replace('\\', "/")));
        }
        if let Some(f) = &d.archive_filename {
            want.insert(format!("archive/{}", f.replace('\\', "/")));
        }
    }
    let mut extra = vec![];
    for sub in ["originals", "archive"] {
        let base = media.join("documents");
        let mut stack = vec![base.join(sub)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if e.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if let Ok(rel) = p.strip_prefix(&base) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if !want.contains(&rel) {
                        extra.push(rel);
                    }
                }
            }
        }
    }
    extra.sort();
    extra
}

// ---------------------------------------------------------------------------------------------
// Bericht

pub const MAX_LIST: usize = 500;

#[derive(Debug, Clone)]
pub struct Step {
    pub level: Level,
    pub name: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub level: Level,
    pub area: &'static str,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct CountRow {
    pub key: &'static str,
    pub label: &'static str,
    pub source: Option<i64>,
    pub target: Option<i64>,
}

impl CountRow {
    pub fn level(&self) -> Level {
        match (self.source, self.target) {
            (a, b) if a == b => Level::Ok,
            (None, Some(0)) | (Some(0), None) => Level::Ok,
            (_, None) => Level::Info,
            _ => Level::Fail,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SpotRow {
    pub id: i64,
    pub title: String,
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Spot {
    /// skipped | done | error
    pub status: String,
    pub api_url: Option<String>,
    pub message: Option<String>,
    pub rows: Vec<SpotRow>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub tool_version: String,
    /// switch | verify | dry-run
    pub mode: String,
    pub started: String,
    pub finished: String,
    pub duration_s: f64,
    pub source_kind: String,
    pub source_label: String,
    pub source: String,
    pub source_version: Option<String>,
    pub source_media: Option<String>,
    pub target: Option<String>,
    pub target_media: Option<String>,
    pub steps: Vec<Step>,
    pub counts: Vec<CountRow>,
    pub source_files: Option<FileSummary>,
    pub target_files: Option<FileSummary>,
    pub extra_files: Vec<String>,
    pub docs_compared: usize,
    pub spot: Spot,
    pub findings: Vec<Finding>,
    pub findings_total: BTreeMap<&'static str, usize>,
    pub verdict: Option<Level>,
}

impl Report {
    pub fn step(&mut self, level: Level, name: &str, message: impl Into<String>) {
        self.steps.push(Step {
            level,
            name: name.into(),
            message: message.into(),
        });
    }

    pub fn finding(&mut self, level: Level, area: &'static str, message: impl Into<String>) {
        *self.findings_total.entry(level.as_str()).or_default() += 1;
        if self.findings.len() < MAX_LIST * 4 {
            self.findings.push(Finding {
                level,
                area,
                message: message.into(),
            });
        }
    }

    pub fn set_counts(&mut self, source: &Counts, target: Option<&Counts>) {
        self.counts = OBJECTS
            .iter()
            .map(|o| CountRow {
                key: o.key,
                label: o.label,
                source: source.get(o.key).copied(),
                target: target.and_then(|t| t.get(o.key).copied()),
            })
            .collect();
        if target.is_some() {
            let bad: Vec<String> = self
                .counts
                .iter()
                .filter(|c| c.level() == Level::Fail)
                .map(|c| {
                    format!(
                        "{}: Quelle {}, Ziel {}",
                        c.label,
                        fmt_opt(c.source),
                        fmt_opt(c.target)
                    )
                })
                .collect();
            for b in bad {
                self.finding(Level::Fail, "Anzahl", b);
            }
        }
    }

    /// Every document row source ↔ target: present, checksums, title, content (hash), tags.
    pub fn compare_docs(&mut self, source: &[DocRow], target: &[DocRow]) {
        let t: HashMap<i64, &DocRow> = target.iter().map(|d| (d.id, d)).collect();
        let s_ids: HashSet<i64> = source.iter().map(|d| d.id).collect();
        self.docs_compared = source.len();
        for s in source {
            let Some(d) = t.get(&s.id) else {
                self.finding(
                    Level::Fail,
                    "Dokument",
                    format!("Dokument {} ({}) fehlt im Ziel", s.id, s.title),
                );
                continue;
            };
            let mut diff = vec![];
            if !same_checksum(&s.checksum, &d.checksum) {
                diff.push("Prüfsumme");
            }
            let arch_same = match (s.archive_checksum.as_deref(), d.archive_checksum.as_deref()) {
                (Some(a), Some(b)) => same_checksum(a, b),
                (None, None) => true,
                _ => false,
            };
            if !arch_same {
                diff.push("Archiv-Prüfsumme");
            }
            if s.title != d.title {
                diff.push("Titel");
            }
            if s.content_sha256 != d.content_sha256 {
                diff.push("Inhalt");
            }
            if s.tags != d.tags {
                diff.push("Tags");
            }
            if s.deleted != d.deleted {
                diff.push("Papierkorb-Status");
            }
            if !diff.is_empty() {
                self.finding(
                    Level::Fail,
                    "Dokument",
                    format!("Dokument {}: abweichend im Ziel: {}", s.id, diff.join(", ")),
                );
            }
        }
        for d in target.iter().filter(|d| !s_ids.contains(&d.id)) {
            self.finding(
                Level::Warn,
                "Dokument",
                format!(
                    "Dokument {} ({}) ist im Ziel, aber nicht in der Quelle",
                    d.id, d.title
                ),
            );
        }
    }

    /// Findings from the file check. `dry_run`: only source checked → everything at most WARN.
    pub fn file_findings(&mut self) {
        let src_keys: HashMap<(i64, FileKind), &FileIssue> = self
            .source_files
            .iter()
            .flat_map(|s| s.problems.iter())
            .map(|p| ((p.doc, p.kind), &p.issue))
            .collect();
        let mut add = vec![];
        if let Some(t) = &self.target_files {
            for p in &t.problems {
                let pre = src_keys.contains_key(&(p.doc, p.kind));
                let level = if p.kind == FileKind::Thumbnail || pre {
                    Level::Warn
                } else {
                    Level::Fail
                };
                let mut m = format!("Ziel: {}", p.describe());
                if pre {
                    m.push_str(
                        " – bereits in der Quelle so, der Umstieg hat nichts verschlechtert",
                    );
                }
                add.push((level, m));
            }
            // Source defective, target intact (e.g. thumbnail regenerated): note only
            let t_keys: HashSet<(i64, FileKind)> =
                t.problems.iter().map(|p| (p.doc, p.kind)).collect();
            for p in self.source_files.iter().flat_map(|s| s.problems.iter()) {
                if !t_keys.contains(&(p.doc, p.kind)) {
                    add.push((
                        Level::Info,
                        format!("Quelle: {} – im Ziel in Ordnung", p.describe()),
                    ));
                }
            }
        } else if let Some(s) = &self.source_files {
            for p in &s.problems {
                add.push((Level::Warn, format!("Quelle: {}", p.describe())));
            }
        }
        for (l, m) in add {
            self.finding(l, "Datei", m);
        }
        if !self.extra_files.is_empty() {
            let n = self.extra_files.len();
            let first: Vec<&str> = self
                .extra_files
                .iter()
                .take(5)
                .map(String::as_str)
                .collect();
            self.finding(
                Level::Warn,
                "Datei",
                format!(
                    "{n} Datei(en) im Ziel-Medienordner ohne Dokument (verwaist), z. B. {}",
                    first.join(", ")
                ),
            );
        }
    }

    pub fn spot_findings(&mut self) {
        match self.spot.status.as_str() {
            "done" => {
                let bad: Vec<String> = self
                    .spot
                    .rows
                    .iter()
                    .filter(|r| !r.problems.is_empty())
                    .map(|r| format!("Stichprobe Dokument {}: {}", r.id, r.problems.join("; ")))
                    .collect();
                for b in bad {
                    self.finding(Level::Fail, "API", b);
                }
            }
            "error" => {
                let m = format!(
                    "Stichprobe über API v1 nicht möglich: {}",
                    self.spot.message.clone().unwrap_or_default()
                );
                self.finding(Level::Warn, "API", m);
            }
            _ => {}
        }
    }

    pub fn finish(&mut self) -> Level {
        let worst = self
            .steps
            .iter()
            .map(|s| s.level)
            .chain(self.findings.iter().map(|f| f.level))
            .chain(
                self.counts
                    .iter()
                    .map(CountRow::level)
                    .filter(|_| self.target.is_some()),
            )
            .max()
            .unwrap_or(Level::Ok);
        let v = if worst == Level::Info {
            Level::Ok
        } else {
            worst
        };
        self.verdict = Some(v);
        v
    }

    pub fn verdict(&self) -> Level {
        self.verdict.unwrap_or(Level::Fail)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "tool": "paperrust-migrate-db",
            "report": "switch-verification",
            "tool_version": self.tool_version,
            "mode": self.mode,
            "verdict": self.verdict().as_str(),
            // Short version as lines (for the desktop first-time setup, apps/desktop SwitchCheck)
            "summary": self.summary().lines().skip(2).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect::<Vec<_>>(),
            "started": self.started,
            "finished": self.finished,
            "duration_s": self.duration_s,
            "source": {
                "kind": self.source_kind,
                "label": self.source_label,
                "location": self.source,
                "paperless_version": self.source_version,
                "media": self.source_media,
            },
            "target": { "database": self.target, "media": self.target_media },
            "steps": self.steps.iter().map(|s| json!({"level": s.level.as_str(), "step": s.name, "message": s.message})).collect::<Vec<_>>(),
            "counts": self.counts.iter().map(|c| json!({
                "object": c.key, "label": c.label, "source": c.source, "target": c.target, "level": c.level().as_str()
            })).collect::<Vec<_>>(),
            "documents_compared": self.docs_compared,
            "files": {
                "source": self.source_files.as_ref().map(FileSummary::to_json),
                "target": self.target_files.as_ref().map(FileSummary::to_json),
                "extra_in_target": self.extra_files.len(),
                "extra_in_target_list": self.extra_files.iter().take(MAX_LIST).collect::<Vec<_>>(),
            },
            "spot_check": {
                "status": self.spot.status,
                "api_url": self.spot.api_url,
                "message": self.spot.message,
                "documents": self.spot.rows.iter().map(|r| json!({
                    "id": r.id, "title": r.title, "ok": r.problems.is_empty(), "problems": r.problems
                })).collect::<Vec<_>>(),
            },
            "findings_total": self.findings_total,
            "findings": self.findings.iter().map(|f| json!({"level": f.level.as_str(), "area": f.area, "message": f.message})).collect::<Vec<_>>(),
        })
    }

    fn files_line(label: &str, f: &FileSummary) -> String {
        format!(
            "{label}: {} Originale, {} Archivdateien, {} Vorschaubilder geprüft ({} SHA-256, {} MD5, {:.1} MiB) – {} in Ordnung, {} Befund(e) [{}]\n",
            f.originals,
            f.archives,
            f.thumbnails,
            f.sha256,
            f.md5,
            f.bytes as f64 / 1_048_576.0,
            f.ok,
            f.problems.len(),
            f.location
        )
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str("PaperRust – Umstieg von paperless-ngx: Prüfbericht\n");
        s.push_str(&format!(
            "Modus: {}   Werkzeug: paperrust-migrate-db {}\n",
            self.mode, self.tool_version
        ));
        s.push_str(&format!(
            "Quelle: {} – {}{}\n",
            self.source_label,
            self.source,
            self.source_version
                .as_ref()
                .map(|v| format!(" (paperless-ngx {v})"))
                .unwrap_or_default()
        ));
        if let Some(m) = &self.source_media {
            s.push_str(&format!("        Medien: {m}\n"));
        }
        if let Some(t) = &self.target {
            s.push_str(&format!("Ziel:   {t}\n"));
        }
        if let Some(m) = &self.target_media {
            s.push_str(&format!("        Medien: {m}\n"));
        }
        s.push_str(&format!(
            "Zeit:   {} ({:.1} s)\n\nSchritte:\n",
            self.started, self.duration_s
        ));
        for st in &self.steps {
            s.push_str(&format!(
                "  [{:<4}] {}: {}\n",
                st.level.as_str(),
                st.name,
                st.message
            ));
        }
        if !self.counts.is_empty() {
            s.push_str(&format!(
                "\n{:<28} {:>10} {:>10}  Prüfung\n",
                "Objektart", "Quelle", "Ziel"
            ));
            for c in &self.counts {
                s.push_str(&format!(
                    "{:<28} {:>10} {:>10}  {}\n",
                    c.label,
                    fmt_opt(c.source),
                    fmt_opt(c.target),
                    if self.target.is_some() {
                        c.level().as_str()
                    } else {
                        "-"
                    }
                ));
            }
        }
        s.push('\n');
        if self.docs_compared > 0 && self.target.is_some() {
            s.push_str(&format!(
                "Dokumentzeilen verglichen (Titel, Inhalt, Tags, Prüfsummen): {}\n",
                self.docs_compared
            ));
        }
        if let Some(f) = &self.source_files {
            s.push_str(&Self::files_line("Dateien Quelle", f));
        }
        if let Some(f) = &self.target_files {
            s.push_str(&Self::files_line("Dateien Ziel", f));
            s.push_str(&format!(
                "Verwaiste Dateien im Ziel: {}\n",
                self.extra_files.len()
            ));
        }
        match self.spot.status.as_str() {
            "done" => {
                let ok = self
                    .spot
                    .rows
                    .iter()
                    .filter(|r| r.problems.is_empty())
                    .count();
                s.push_str(&format!(
                    "Stichprobe API v1 ({}): {} Dokumente, {ok} in Ordnung\n",
                    self.spot.api_url.as_deref().unwrap_or("-"),
                    self.spot.rows.len()
                ));
            }
            "error" => s.push_str(&format!(
                "Stichprobe API v1: FEHLER {}\n",
                self.spot.message.as_deref().unwrap_or("")
            )),
            _ => s.push_str(&format!(
                "Stichprobe API v1: nicht durchgeführt{}\n",
                self.spot
                    .message
                    .as_ref()
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default()
            )),
        }
        if !self.findings.is_empty() {
            s.push_str("\nBefunde:\n");
            for f in self
                .findings
                .iter()
                .filter(|f| f.level >= Level::Warn)
                .chain(self.findings.iter().filter(|f| f.level < Level::Warn))
            {
                s.push_str(&format!(
                    "  {:<4} {:<9} {}\n",
                    f.level.as_str(),
                    f.area,
                    f.message
                ));
            }
            let shown = self.findings.len();
            let total: usize = self.findings_total.values().sum();
            if total > shown {
                s.push_str(&format!(
                    "  … {} weitere Befunde (nur gezählt)\n",
                    total - shown
                ));
            }
        }
        s.push_str(&format!(
            "\nErgebnis: {} – {}\n",
            self.verdict().as_str(),
            match (self.verdict(), self.mode.as_str()) {
                (Level::Fail, "dry-run") => "Umstieg so NICHT möglich, siehe Befunde.",
                (Level::Fail, _) =>
                    "Umstieg NICHT in Ordnung – Befunde prüfen, Quelle ist unverändert.",
                (Level::Warn, _) => "Umstieg möglich/erfolgt, aber Hinweise beachten.",
                (_, "dry-run") => "Probelauf ohne Befund – der Umstieg kann starten.",
                _ => "Alles übernommen und geprüft.",
            }
        ));
        s.push_str(&self.summary());
        s
    }

    /// Short version in plain words (comparison: documents, tags, users, files).
    pub fn summary(&self) -> String {
        let mut s = String::from("\nKurzfassung:\n");
        let count = |key: &str| self.counts.iter().find(|c| c.key == key);
        for (key, label) in [("documents", "Dokumente"), ("tags", "Tags"), ("users", "Benutzer")] {
            let Some(c) = count(key) else { continue };
            let line = match (c.source, c.target, self.target.is_some() && self.mode != "dry-run") {
                (Some(a), Some(b), true) if a == b => format!("{label}: alle {b} übernommen"),
                (Some(a), Some(b), true) => format!("{label}: {b} von {a} übernommen – ABWEICHUNG"),
                (Some(a), _, _) => format!("{label}: {a} gefunden"),
                _ => continue,
            };
            s.push_str(&format!("  {line}\n"));
        }
        let f = self.target_files.as_ref().or(self.source_files.as_ref());
        if let Some(f) = f {
            let n = |pred: fn(&FileIssue) -> bool| f.problems.iter().filter(|p| pred(&p.issue)).count();
            let missing = n(|i| matches!(i, FileIssue::Missing | FileIssue::NoFilename));
            let broken = n(|i| matches!(i, FileIssue::Mismatch { .. } | FileIssue::Unreadable(_)));
            let place = if self.target_files.is_some() { "im Ziel" } else { "in der Quelle" };
            s.push_str(&format!(
                "  Dateien {place}: {} gefunden und in Ordnung, {missing} fehlen, {broken} beschädigt\n",
                f.ok
            ));
        }
        s
    }

    pub fn to_html(&self) -> String {
        let e = html_escape;
        let badge = |l: Level| {
            format!(
                "<span class=\"b {}\">{}</span>",
                l.as_str().to_lowercase(),
                l.as_str()
            )
        };
        let mut h = String::from(
            "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <title>Umstieg – Prüfbericht</title><style>\
             :root{color-scheme:light dark;--bg:#fff;--fg:#1d1d1f;--mut:#666;--line:#ddd}\
             @media (prefers-color-scheme:dark){:root{--bg:#17181a;--fg:#e8e8e8;--mut:#9a9a9a;--line:#333}}\
             body{background:var(--bg);color:var(--fg);font:15px/1.45 system-ui,sans-serif;margin:0 auto;max-width:960px;padding:16px}\
             h1{font-size:1.4em}h2{font-size:1.1em;margin-top:1.6em}table{border-collapse:collapse;width:100%}\
             td,th{border-bottom:1px solid var(--line);padding:4px 6px;text-align:left;vertical-align:top}td.n{text-align:right;font-variant-numeric:tabular-nums}\
             .b{border-radius:4px;padding:1px 6px;font-weight:600;font-size:.85em;color:#fff}.ok{background:#2e7d32}.info{background:#546e7a}.warn{background:#b26a00}.fail{background:#c62828}\
             .verdict{font-size:1.3em;margin:12px 0}.mut{color:var(--mut)}code{word-break:break-all}</style></head><body>",
        );
        h.push_str("<h1>Umstieg von paperless-ngx – Prüfbericht</h1>");
        h.push_str(&format!(
            "<p class=\"verdict\">Ergebnis: {}</p>",
            badge(self.verdict())
        ));
        h.push_str(&format!(
            "<p class=\"mut\">Modus {} · {} · {:.1} s · paperrust-migrate-db {}</p>",
            e(&self.mode),
            e(&self.started),
            self.duration_s,
            e(&self.tool_version)
        ));
        h.push_str("<table>");
        h.push_str(&format!(
            "<tr><th>Quelle</th><td>{} – <code>{}</code>{}{}</td></tr>",
            e(&self.source_label),
            e(&self.source),
            self.source_version
                .as_ref()
                .map(|v| format!(" (paperless-ngx {})", e(v)))
                .unwrap_or_default(),
            self.source_media
                .as_ref()
                .map(|m| format!("<br>Medien: <code>{}</code>", e(m)))
                .unwrap_or_default()
        ));
        if let Some(t) = &self.target {
            h.push_str(&format!(
                "<tr><th>Ziel</th><td><code>{}</code>{}</td></tr>",
                e(t),
                self.target_media
                    .as_ref()
                    .map(|m| format!("<br>Medien: <code>{}</code>", e(m)))
                    .unwrap_or_default()
            ));
        }
        h.push_str("</table><h2>Schritte</h2><table>");
        for st in &self.steps {
            h.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                badge(st.level),
                e(&st.name),
                e(&st.message)
            ));
        }
        h.push_str("</table>");
        if !self.counts.is_empty() {
            h.push_str("<h2>Objekte</h2><table><tr><th>Objektart</th><th>Quelle</th><th>Ziel</th><th></th></tr>");
            for c in &self.counts {
                h.push_str(&format!(
                    "<tr><td>{}</td><td class=n>{}</td><td class=n>{}</td><td>{}</td></tr>",
                    e(c.label),
                    fmt_opt(c.source),
                    fmt_opt(c.target),
                    if self.target.is_some() {
                        badge(c.level())
                    } else {
                        String::new()
                    }
                ));
            }
            h.push_str("</table>");
        }
        h.push_str("<h2>Dateien und Stichprobe</h2><ul>");
        if let Some(f) = &self.source_files {
            h.push_str(&format!(
                "<li>{}</li>",
                e(Self::files_line("Quelle", f).trim())
            ));
        }
        if let Some(f) = &self.target_files {
            h.push_str(&format!(
                "<li>{}</li>",
                e(Self::files_line("Ziel", f).trim())
            ));
            h.push_str(&format!(
                "<li>Verwaiste Dateien im Ziel: {}</li>",
                self.extra_files.len()
            ));
        }
        if self.docs_compared > 0 && self.target.is_some() {
            h.push_str(&format!(
                "<li>Dokumentzeilen verglichen: {}</li>",
                self.docs_compared
            ));
        }
        h.push_str(&format!(
            "<li>Stichprobe API v1: {}</li></ul>",
            match self.spot.status.as_str() {
                "done" => format!(
                    "{} Dokumente, {} in Ordnung",
                    self.spot.rows.len(),
                    self.spot
                        .rows
                        .iter()
                        .filter(|r| r.problems.is_empty())
                        .count()
                ),
                "error" => format!("Fehler: {}", e(self.spot.message.as_deref().unwrap_or(""))),
                _ => format!(
                    "nicht durchgeführt {}",
                    e(self.spot.message.as_deref().unwrap_or(""))
                ),
            }
        ));
        if !self.findings.is_empty() {
            h.push_str("<h2>Befunde</h2><table>");
            for f in self
                .findings
                .iter()
                .filter(|f| f.level >= Level::Warn)
                .chain(self.findings.iter().filter(|f| f.level < Level::Warn))
            {
                h.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                    badge(f.level),
                    f.area,
                    e(&f.message)
                ));
            }
            h.push_str("</table>");
        }
        h.push_str("</body></html>\n");
        h
    }

    /// `<dir>/umstieg-bericht.{json,txt,html}`
    pub fn write(&self, dir: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        std::fs::create_dir_all(dir)?;
        let files = [
            (
                dir.join("umstieg-bericht.json"),
                serde_json::to_string_pretty(&self.to_json()).unwrap_or_default(),
            ),
            (dir.join("umstieg-bericht.txt"), self.to_text()),
            (dir.join("umstieg-bericht.html"), self.to_html()),
        ];
        let mut out = vec![];
        for (p, c) in files {
            std::fs::write(&p, c)?;
            out.push(p);
        }
        Ok(out)
    }
}

fn fmt_opt(v: Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "-".into())
}

pub fn html_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::sha256_hex;
    use sha2::Digest;

    fn doc(id: i64, file: &str, content: &[u8]) -> DocRow {
        DocRow {
            id,
            title: format!("T{id}"),
            content_sha256: sha256_hex(b"x"),
            checksum: sha256_hex(content),
            filename: Some(file.into()),
            ..Default::default()
        }
    }

    fn media_with(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for sub in ["originals", "archive", "thumbnails"] {
            std::fs::create_dir_all(d.path().join("documents").join(sub)).unwrap();
        }
        for (rel, data) in files {
            let p = d.path().join("documents").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, data).unwrap();
        }
        d
    }

    #[test]
    fn file_check_detects_missing_mismatch_md5_and_extra() {
        let m = media_with(&[
            ("originals/a.pdf", b"AAAA"),
            ("originals/sub/b.pdf", b"BBBB-corrupt"),
            ("originals/orphan.pdf", b"?"),
            ("archive/a.pdf", b"arch"),
            ("thumbnails/0000001.webp", b"w"),
            ("thumbnails/0000002.png", b"p"),
            ("originals/.keep", b""),
        ]);
        let mut a = doc(1, "a.pdf", b"AAAA");
        a.archive_filename = Some("a.pdf".into());
        a.archive_checksum = Some(crate::value::hex(&md5::Md5::digest(b"arch")));
        let b = doc(2, "sub/b.pdf", b"BBBB");
        let c = doc(3, "c.pdf", b"CCCC");
        let mut d = doc(4, "x", b"");
        d.filename = None;
        let docs = vec![a, b, c, d];
        let s = check_files(&Files::Media(m.path()), &docs, 3);
        assert_eq!((s.originals, s.archives, s.thumbnails), (3, 1, 4));
        assert_eq!((s.md5, s.sha256), (1, 2));
        let kinds: Vec<(i64, FileKind, &str)> = s
            .problems
            .iter()
            .map(|p| (p.doc, p.kind, p.issue.code()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (2, FileKind::Original, "checksum-mismatch"),
                (3, FileKind::Original, "missing"),
                (3, FileKind::Thumbnail, "missing"),
                (4, FileKind::Original, "no-filename"),
                (4, FileKind::Thumbnail, "missing"),
            ]
        );
        assert_eq!(extra_files(m.path(), &docs), vec!["originals/orphan.pdf"]);
    }

    #[test]
    fn export_files_checked_from_zip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("e.zip");
        std::fs::write(
            &p,
            crate::zipread::testzip::build(&[
                ("manifest.json", b"[]", false),
                ("o/1.pdf", b"one", true),
            ]),
        )
        .unwrap();
        let e = ExportSource::open(&p).unwrap();
        let mut ok = doc(1, "o/1.pdf", b"one");
        ok.thumbnail = Some("t/1.webp".into());
        let s = check_files(&Files::Export(&e), &[ok], 2);
        assert_eq!(s.ok, 1);
        assert_eq!(s.problems.len(), 1);
        assert_eq!(s.problems[0].kind, FileKind::Thumbnail);
    }

    fn base_report() -> Report {
        Report {
            target: Some("postgres://x".into()),
            ..Default::default()
        }
    }

    #[test]
    fn verdicts() {
        let mut r = base_report();
        let mut src = Counts::new();
        src.insert("documents", 2);
        src.insert("tags", 1);
        r.set_counts(&src, Some(&src.clone()));
        r.step(Level::Ok, "Erkennung", "ok");
        assert_eq!(r.finish(), Level::Ok);

        let mut t = src.clone();
        t.insert("tags", 0);
        let mut r = base_report();
        r.set_counts(&src, Some(&t));
        assert_eq!(r.finish(), Level::Fail);
        assert!(r.to_text().contains("Tags: Quelle 1, Ziel 0"));

        // Source does not have the table (older version), target 0 → ok
        let mut r = base_report();
        let mut t2 = src.clone();
        t2.insert("share_link_bundles", 0);
        r.set_counts(&src, Some(&t2));
        assert_eq!(r.finish(), Level::Ok);
    }

    #[test]
    fn doc_and_file_comparison_levels() {
        let a = doc(1, "a.pdf", b"a");
        let b = doc(2, "b.pdf", b"b");
        let mut b2 = b.clone();
        b2.tags = vec![5];
        let mut r = base_report();
        r.compare_docs(&[a.clone(), b.clone()], &[a.clone(), b2]);
        assert_eq!(r.finish(), Level::Fail);
        assert!(r.findings[0].message.contains("Tags"));

        let mut r = base_report();
        r.compare_docs(std::slice::from_ref(&a), &[a.clone(), b.clone()]);
        assert_eq!(r.finish(), Level::Warn, "zusätzliches Dokument im Ziel");

        // Damage already in the source → WARN; only in the target → FAIL
        let prob = |doc, kind| FileProblem {
            doc,
            kind,
            path: "p".into(),
            issue: FileIssue::Mismatch {
                expected: "e".into(),
                actual: "a".into(),
            },
        };
        let mut r = base_report();
        r.source_files = Some(FileSummary {
            problems: vec![prob(1, FileKind::Original)],
            ..Default::default()
        });
        r.target_files = Some(FileSummary {
            problems: vec![prob(1, FileKind::Original)],
            ..Default::default()
        });
        r.file_findings();
        assert_eq!(r.finish(), Level::Warn);
        assert!(r.to_text().contains("bereits in der Quelle"));
        let mut r = base_report();
        r.source_files = Some(FileSummary::default());
        r.target_files = Some(FileSummary {
            problems: vec![prob(1, FileKind::Original)],
            ..Default::default()
        });
        r.file_findings();
        assert_eq!(r.finish(), Level::Fail);
        let mut r = base_report();
        r.target_files = Some(FileSummary {
            problems: vec![prob(1, FileKind::Thumbnail)],
            ..Default::default()
        });
        r.file_findings();
        assert_eq!(r.finish(), Level::Warn);
    }

    #[test]
    fn spot_and_rendering() {
        let mut r = base_report();
        r.mode = "switch".into();
        r.source = "C:/export <x>".into();
        r.spot = Spot {
            status: "done".into(),
            api_url: Some("http://127.0.0.1:1".into()),
            message: None,
            rows: vec![
                SpotRow {
                    id: 1,
                    title: "a".into(),
                    problems: vec![],
                },
                SpotRow {
                    id: 2,
                    title: "b".into(),
                    problems: vec!["Titel".into()],
                },
            ],
        };
        r.spot_findings();
        assert_eq!(r.finish(), Level::Fail);
        let j = r.to_json();
        assert_eq!(j["verdict"], "FAIL");
        assert_eq!(j["spot_check"]["documents"][1]["ok"], false);
        let h = r.to_html();
        assert!(h.contains("C:/export &lt;x&gt;") && h.contains("class=\"b fail\""));
        let mut r = base_report();
        r.spot = Spot {
            status: "error".into(),
            message: Some("Verbindung".into()),
            ..Default::default()
        };
        r.spot_findings();
        assert_eq!(r.finish(), Level::Warn);
        let d = tempfile::tempdir().unwrap();
        assert_eq!(r.write(d.path()).unwrap().len(), 3);
        assert!(d.path().join("umstieg-bericht.html").is_file());
    }

    #[test]
    fn md5_source_sha256_target_is_no_difference() {
        let md5 = "0123456789abcdef0123456789abcdef";
        let sha = "a".repeat(64);
        assert!(same_checksum(md5, &sha));
        assert!(same_checksum(md5, &md5.to_uppercase()));
        assert!(!same_checksum(&sha, md5), "Ziel MD5, Quelle SHA-256 wäre ein Rückschritt");
        assert!(!same_checksum(&sha, &"b".repeat(64)));
    }
}
