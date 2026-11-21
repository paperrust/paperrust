//! `document_exporter`.
//!
//! upstream: src/documents/management/commands/document_exporter.py (options, `dump`,
//! `generate_base_name`, `generate_document_targets`, `copy_document_files`,
//! `_write_split_manifest`, `_encrypt_record_inline`), src/documents/export/sinks.py
//! (`DirectoryExportSink`, `ZipExportSink`, `StreamingManifestWriter`, `_dumps`),
//! src/documents/export/compression.py, src/documents/models.py (`get_public_filename`) – v3.4.2.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sqlx::{Connection, PgConnection};

use super::descriptor::{self, Model};
use super::fernet::{self, Fernet};
use super::medialock::MediaLock;
use super::pypath;
use super::value::column_json;
use super::zip::{Method, ZipWriter};
use super::*;
use crate::args::{self, flag, value};
use crate::ctx::Ctx;
use crate::pyjson;
use crate::{CliError, CliResult};

const THUMBNAIL_SUFFIX: &str = "-thumbnail.webp";
const ARCHIVE_SUFFIX: &str = "-archive.pdf";
/// `Document.MAX_PUBLIC_FILENAME_BYTES - max(len(THUMBNAIL_SUFFIX), len(ARCHIVE_SUFFIX))`
const MAX_BASE_NAME_BYTES: usize = 255 - 15;

const OPTS: &[args::Opt] = &[
    args::PROGRESS,
    flag(&["-c", "--compare-checksums"], "compare_checksums"),
    flag(&["-cj", "--compare-json"], "compare_json"),
    flag(&["-d", "--delete"], "delete"),
    flag(&["-f", "--use-filename-format"], "use_filename_format"),
    flag(&["-na", "--no-archive"], "no_archive"),
    flag(&["-nt", "--no-thumbnail"], "no_thumbnail"),
    flag(&["-p", "--use-folder-prefix"], "use_folder_prefix"),
    flag(&["-sm", "--split-manifest"], "split_manifest"),
    flag(&["-z", "--zip"], "zip"),
    value(&["-zn", "--zip-name"], "zip_name"),
    value(&["--zip-compression"], "zip_compression"),
    value(&["--zip-compression-level"], "zip_compression_level"),
    flag(&["--data-only"], "data_only"),
    value(&["--passphrase"], "passphrase"),
    value(&["--batch-size"], "batch_size"),
];

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub target: PathBuf,
    pub compare_checksums: bool,
    pub compare_json: bool,
    pub delete: bool,
    pub use_filename_format: bool,
    pub no_archive: bool,
    pub no_thumbnail: bool,
    pub use_folder_prefix: bool,
    pub split_manifest: bool,
    pub zip: bool,
    pub zip_name: String,
    pub zip_compression: Option<String>,
    pub zip_compression_level: Option<i64>,
    pub data_only: bool,
    pub passphrase: Option<String>,
    pub batch_size: i64,
}

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 1)?;
    let Some(target) = a.positionals.first() else {
        return Err(CliError::Usage("the following arguments are required: target".into()));
    };
    let c = Ctx::connect().await?;
    let choices = ["stored", "deflated", "bzip2", "lzma", "zstd"];
    if let Some(z) = a.value("zip_compression")
        && !choices.contains(&z)
    {
        return Err(CliError::Usage(format!(
            "argument --zip-compression: invalid choice: '{z}' (choose from 'stored', 'deflated', 'bzip2', 'lzma', 'zstd')"
        )));
    }
    let today = chrono::Utc::now().with_timezone(&c.cfg.tz).date_naive();
    let opts = ExportOptions {
        target: PathBuf::from(target),
        compare_checksums: a.flag("compare_checksums"),
        compare_json: a.flag("compare_json"),
        delete: a.flag("delete"),
        use_filename_format: a.flag("use_filename_format"),
        no_archive: a.flag("no_archive"),
        no_thumbnail: a.flag("no_thumbnail"),
        use_folder_prefix: a.flag("use_folder_prefix"),
        split_manifest: a.flag("split_manifest"),
        zip: a.flag("zip"),
        zip_name: a.value("zip_name").map(str::to_string).unwrap_or_else(|| format!("export-{}", today.format("%Y-%m-%d"))),
        zip_compression: a.value("zip_compression").map(str::to_string),
        zip_compression_level: a.int("zip_compression_level")?,
        data_only: a.flag("data_only"),
        passphrase: a.value("passphrase").map(str::to_string),
        batch_size: a.int("batch_size")?.unwrap_or(500),
    };
    // tests/chaos C3: report write errors (disk full, no write permission) with target and hint instead of
    // just "I/O error: Permission denied".
    export(&c, &opts).await.map_err(|e| match e {
        CliError::Other(m) if m.starts_with("I/O error: ") => CliError::Command(format!(
            "Writing the export to {} failed: {} (disk full or no write permission?)",
            opts.target.display(),
            &m["I/O error: ".len()..]
        )),
        e => e,
    })
}

// ------------------------------------------------------------------ Sinks

enum Sink {
    Dir(DirSink),
    Zip(ZipSink),
}

struct DirSink {
    target: PathBuf,
    compare_checksums: bool,
    compare_json: bool,
    delete: bool,
    snapshot: HashSet<PathBuf>,
}

/// Archive name → target path; `..`, `\`, drives rejected (review C-N4).
fn arc_path(root: &Path, arcname: &str) -> CliResult<PathBuf> {
    super::safepath::arc_target(root, arcname)
}

fn walk_files(dir: &Path, out: &mut HashSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk_files(&p, out),
            Ok(t) if t.is_file() => {
                out.insert(p);
            }
            _ => {}
        }
    }
}

fn sha256_file(p: &Path) -> std::io::Result<String> {
    use sha2::Digest;
    let mut f = std::fs::File::open(p)?;
    let mut h = sha2::Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(fernet::hex_encode(&h.finalize()))
}

impl DirSink {
    fn open(&mut self) {
        let mut s = HashSet::new();
        walk_files(&self.target, &mut s);
        self.snapshot = s;
    }

    fn add_file(&mut self, source: &Path, arcname: &str, checksum: Option<&str>) -> CliResult {
        let target = arc_path(&self.target, arcname)?;
        self.snapshot.remove(&target);
        let perform_copy = if target.exists() {
            let ss = std::fs::metadata(source)?;
            let ts = std::fs::metadata(&target)?;
            if self.compare_checksums
                && let Some(c) = checksum
            {
                sha256_file(&target)? != c
            } else {
                ss.modified().ok() != ts.modified().ok() || ss.len() != ts.len()
            }
        } else {
            true
        };
        if perform_copy {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p)?;
            }
            copy_file_with_basic_stats(source, &target)?;
        }
        Ok(())
    }

    fn add_json(&mut self, content: &str, arcname: &str) -> CliResult {
        let target = arc_path(&self.target, arcname)?;
        let mut perform_write = true;
        if self.snapshot.remove(&target) && self.compare_json && std::fs::read(&target).map(|b| b == content.as_bytes()).unwrap_or(false) {
            perform_write = false;
        }
        if perform_write {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(&target, content.as_bytes())?;
        }
        Ok(())
    }

    fn commit_stream(&mut self, target: &Path, tmp: &Path) -> CliResult {
        if self.snapshot.remove(target) && self.compare_json {
            let new = std::fs::read(tmp)?;
            if std::fs::read(target).map(|b| b == new).unwrap_or(false) {
                std::fs::remove_file(tmp)?;
                return Ok(());
            }
        }
        std::fs::rename(tmp, target)?;
        Ok(())
    }

    fn finalize(&mut self) -> CliResult {
        if self.delete {
            for f in self.snapshot.drain() {
                if !f.starts_with(&self.target) {
                    continue;
                }
                std::fs::remove_file(&f)?;
                if let Some(parent) = f.parent() {
                    pl_templating::delete_empty_directories(parent, &self.target);
                }
            }
        }
        Ok(())
    }
}

struct ZipSink {
    target: PathBuf,
    zip_path: PathBuf,
    tmp_path: PathBuf,
    delete: bool,
    zip: Option<ZipWriter>,
    dirs: HashSet<String>,
    pending_manifest: Option<(PathBuf, String)>,
}

impl ZipSink {
    fn ensure_dirs(&mut self, arcname: &str) -> CliResult {
        // same naming rules as for the directory export (review C-N4)
        arc_path(&self.target, arcname)?;
        let parts: Vec<&str> = arcname.split('/').collect();
        let mut dir_arc = String::new();
        for part in &parts[..parts.len().saturating_sub(1)] {
            dir_arc.push_str(part);
            dir_arc.push('/');
            if self.dirs.insert(dir_arc.clone())
                && let Some(z) = self.zip.as_mut()
            {
                z.mkdir(&dir_arc)?;
            }
        }
        Ok(())
    }

    fn add_file(&mut self, source: &Path, arcname: &str) -> CliResult {
        self.ensure_dirs(arcname)?;
        if let Some(z) = self.zip.as_mut() {
            z.write_file(source, arcname)?;
        }
        Ok(())
    }

    fn add_json(&mut self, content: &str, arcname: &str) -> CliResult {
        self.ensure_dirs(arcname)?;
        if let Some(z) = self.zip.as_mut() {
            z.write_bytes(arcname, content.as_bytes())?;
        }
        Ok(())
    }

    fn finalize(&mut self) -> CliResult {
        if let Some((tmp, arc)) = self.pending_manifest.take() {
            self.ensure_dirs(&arc)?;
            if let Some(z) = self.zip.as_mut() {
                z.write_file(&tmp, &arc)?;
            }
            let _ = std::fs::remove_file(&tmp);
        }
        if let Some(z) = self.zip.take() {
            z.finish()?;
        }
        if self.delete {
            for e in std::fs::read_dir(&self.target)?.flatten() {
                let p = e.path();
                if p == self.zip_path || p == self.tmp_path {
                    continue;
                }
                if p.is_dir() {
                    std::fs::remove_dir_all(&p)?;
                } else {
                    std::fs::remove_file(&p)?;
                }
            }
        }
        std::fs::rename(&self.tmp_path, &self.zip_path)?;
        Ok(())
    }

    fn abort(&mut self) {
        self.zip.take();
        let _ = std::fs::remove_file(&self.tmp_path);
        if let Some((tmp, _)) = self.pending_manifest.take() {
            let _ = std::fs::remove_file(tmp);
        }
    }
}

impl Sink {
    fn add_file(&mut self, source: &Path, arcname: &str, checksum: Option<&str>) -> CliResult {
        match self {
            Sink::Dir(d) => d.add_file(source, arcname, checksum),
            Sink::Zip(z) => z.add_file(source, arcname),
        }
    }

    fn add_json(&mut self, content: &str, arcname: &str) -> CliResult {
        match self {
            Sink::Dir(d) => d.add_json(content, arcname),
            Sink::Zip(z) => z.add_json(content, arcname),
        }
    }
}

/// `StreamingManifestWriter` onto a file.
struct ManifestWriter {
    out: std::io::BufWriter<std::fs::File>,
    first: bool,
}

impl ManifestWriter {
    fn new(path: &Path) -> CliResult<Self> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        out.write_all(b"[")?;
        Ok(ManifestWriter { out, first: true })
    }

    fn write_record(&mut self, record: &Value) -> CliResult {
        if !self.first {
            self.out.write_all(b",\n")?;
        }
        self.first = false;
        self.out.write_all(pyjson::dumps(record, Some(2)).as_bytes())?;
        Ok(())
    }

    fn close(mut self) -> CliResult {
        self.out.write_all(b"\n]")?;
        self.out.flush()?;
        Ok(())
    }
}

// ------------------------------------------------------------------ Serialization

/// SQL literal for cursor queries (parameters are only user names/integers).
fn literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => {
            if *b {
                "true".into()
            } else {
                "false".into()
            }
        }
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        other => format!("'{}'", other.to_string().replace('\'', "''")),
    }
}

pub fn inline_params(sql: &str, params: &[Value]) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        match after[..digits].parse::<usize>().ok().filter(|n| *n >= 1).and_then(|n| params.get(n - 1)) {
            Some(p) => out.push_str(&literal(p)),
            None => {
                out.push('$');
                out.push_str(&after[..digits]);
            }
        }
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

/// Row → `{"model", "pk", "fields"}` (serializer "python"), incl. m2m lists.
pub async fn build_record(conn: &mut PgConnection, model: &Model, row: &sqlx::postgres::PgRow) -> CliResult<Map<String, Value>> {
    let mut fields = Map::new();
    let mut pk = Value::Null;
    for (i, f) in model.fields.iter().enumerate() {
        if !f.serialize && !f.primary_key {
            continue;
        }
        let v = column_json(row, i)?;
        if f.primary_key {
            pk = v.clone();
        }
        if f.serialize {
            fields.insert(f.name.clone(), v);
        }
    }
    for m in &model.m2m {
        if !m.serialize || !m.auto_created {
            continue;
        }
        let q = sqlx::query(&m.related_sql);
        let q = match &pk {
            Value::Number(n) => q.bind(n.as_i64().unwrap_or_default()),
            other => q.bind(other.as_str().unwrap_or_default().to_string()),
        };
        let mut list = Vec::new();
        for r in q.fetch_all(&mut *conn).await? {
            list.push(column_json(&r, 0)?);
        }
        fields.insert(m.name.clone(), Value::Array(list));
    }
    let mut rec = Map::new();
    rec.insert("model".into(), Value::String(model.label.clone()));
    rec.insert("pk".into(), pk);
    rec.insert("fields".into(), Value::Object(fields));
    Ok(rec)
}

struct Crypt {
    fernet: Fernet,
    salt: String,
    iterations: u32,
    key_size: usize,
    algorithm: String,
}

fn encrypt_inline(crypt: Option<&Crypt>, rec: &mut Map<String, Value>) {
    let Some(c) = crypt else { return };
    let d = descriptor::get();
    let label = rec.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
    let Some(cf) = d.crypt_fields.iter().find(|c| c.model_name == label) else { return };
    let Some(fields) = rec.get_mut("fields").and_then(Value::as_object_mut) else { return };
    for f in &cf.fields {
        if let Some(Value::String(s)) = fields.get(f)
            && !s.is_empty()
        {
            let enc = c.fernet.encrypt_string(s);
            fields.insert(f.clone(), Value::String(enc));
        }
    }
}

/// All records of a query (server-side cursor, `batch_size` per FETCH).
async fn for_each_record<F>(conn: &mut PgConnection, model: &Model, sql: &str, params: &[Value], batch: i64, mut f: F) -> CliResult
where
    F: FnMut(Map<String, Value>) -> CliResult,
{
    let mut full = inline_params(sql, params);
    // guardian models have no Meta.ordering: upstream returns the physical order of the
    // table (for an unchanged table = id order). Since data model stage 4 they are views
    // over paperrust_object_grant whose plan order means nothing → explicitly by id.
    if let Some(t) = ["guardian_userobjectpermission", "guardian_groupobjectpermission"].iter().find(|t| model.table == **t)
        && !full.contains("ORDER BY")
    {
        full.push_str(&format!(" ORDER BY \"{t}\".\"id\""));
    }
    sqlx::raw_sql(&format!("DECLARE pl_cli_export NO SCROLL CURSOR FOR {full}")).execute(&mut *conn).await?;
    loop {
        // simple protocol (text format): FETCH over the extended protocol returns no
        // row description on Describe, sqlx would expect binary values
        let rows = sqlx::raw_sql(&format!("FETCH {batch} FROM pl_cli_export")).fetch_all(&mut *conn).await?;
        if rows.is_empty() {
            break;
        }
        let mut records = Vec::with_capacity(rows.len());
        for r in &rows {
            records.push(build_record(conn, model, r).await?);
        }
        for r in records {
            f(r)?;
        }
    }
    sqlx::raw_sql("CLOSE pl_cli_export").execute(&mut *conn).await?;
    Ok(())
}

// ------------------------------------------------------------------ File names

/// Fields of a document for names/files.
#[derive(Debug, Clone)]
struct DocInfo {
    row: pl_consumer::db::DocRow,
    root: Option<pl_consumer::db::DocRow>,
    root_correspondent: Option<String>,
}

fn sanitize240(s: &str) -> String {
    pl_files::naming::sanitize_filename(s, "-", MAX_BASE_NAME_BYTES)
}

/// `Document.get_public_filename(counter=…, max_bytes=MAX_BASE_NAME_BYTES)`.
fn public_filename(info: &DocInfo, file_type: &str, counter: u32) -> String {
    let ctxdoc = info.root.as_ref().unwrap_or(&info.row);
    let s = pl_files::naming::document_str(ctxdoc.created, info.root_correspondent.as_deref(), &ctxdoc.title);
    let tail = if counter != 0 { format!("_{counter:02}{file_type}") } else { file_type.to_string() };
    let head_bytes = MAX_BASE_NAME_BYTES.saturating_sub(tail.len());
    let head = pl_files::naming::truncate_utf8(&s, head_bytes);
    sanitize240(&format!("{head}{tail}"))
}

pub async fn export(c: &Ctx, o: &ExportOptions) -> CliResult {
    let d = descriptor::get();
    if o.zip && (o.compare_checksums || o.compare_json) {
        return Err(CliError::Command("--compare-checksums and --compare-json have no effect when used with --zip".into()));
    }
    let target = std::path::absolute(&o.target).unwrap_or_else(|_| o.target.clone());
    if !target.exists() {
        return Err(CliError::Command("That path doesn't exist".into()));
    }
    if !target.is_dir() {
        return Err(CliError::Command("That path isn't a directory".into()));
    }
    if std::fs::metadata(&target).map(|m| m.permissions().readonly()).unwrap_or(true) {
        return Err(CliError::Command("That path doesn't appear to be writable".into()));
    }
    if !o.zip && (o.zip_compression.is_some() || o.zip_compression_level.is_some()) {
        return Err(CliError::Command("--zip-compression and --zip-compression-level require --zip".into()));
    }
    let method_name = o.zip_compression.clone().unwrap_or_else(|| "deflated".into());
    // B6: all methods of `COMPRESSION_CHOICES` available (upstream: bzip2/lzma depending on
    // Python modules, zstd only from Python 3.14 – PaperRust can always do all).
    let method = match method_name.as_str() {
        "stored" => Method::Stored,
        "bzip2" => Method::Bzip2,
        "lzma" => Method::Lzma,
        "zstd" => Method::Zstd,
        _ => Method::Deflated,
    };
    if o.zip
        && let Some(level) = o.zip_compression_level
    {
        let bounds = match method_name.as_str() {
            "deflated" => Some((0, 9)),
            "bzip2" => Some((1, 9)),
            "zstd" => Some((-22, 22)),
            _ => None,
        };
        match bounds {
            None => return Err(CliError::Command(format!("--zip-compression-level has no effect for '{method_name}'"))),
            Some((lo, hi)) if !(lo..=hi).contains(&level) => {
                return Err(CliError::Command(format!("--zip-compression-level for '{method_name}' must be between {lo} and {hi}")));
            }
            _ => {}
        }
    }
    let mut sink = if o.zip {
        // (target / zip_name).with_suffix(".zip")
        // PaperRust: only a file name, no path (review C-N3)
        let zn = o.zip_name.as_str();
        if !zip_name_ok(zn) {
            return Err(CliError::Command(format!("--zip-name must be a plain file name without directories: {zn}")));
        }
        if o.delete {
            eprintln!("Warning: --delete with --zip removes every other file and folder in {}", target.display());
        }
        let zip_path = target.join(format!("{}.zip", pypath::without_suffix(zn)));
        let tmp_path = zip_path.with_file_name(format!("{}.tmp", zip_path.file_name().and_then(|s| s.to_str()).unwrap_or("export.zip")));
        std::fs::create_dir_all(&c.cfg.scratch_dir)?;
        // bounds checked above (LEVEL_BOUNDS)
        let level = o.zip_compression_level.and_then(|l| i32::try_from(l).ok());
        let zw = ZipWriter::create(&tmp_path, method, level, c.cfg.tz)?;
        Sink::Zip(ZipSink {
            target: target.clone(),
            zip_path,
            tmp_path,
            delete: o.delete,
            zip: Some(zw),
            dirs: HashSet::new(),
            pending_manifest: None,
        })
    } else {
        let mut ds = DirSink {
            target: target.clone(),
            compare_checksums: o.compare_checksums,
            compare_json: o.compare_json,
            delete: o.delete,
            snapshot: HashSet::new(),
        };
        ds.open();
        Sink::Dir(ds)
    };

    let _lock = MediaLock::acquire(&c.cfg.media_lock).await?;
    let result = dump(c, o, d, &mut sink, &target).await;
    match (&mut sink, result) {
        (Sink::Zip(z), Err(e)) => {
            z.abort();
            Err(e)
        }
        (_, Err(e)) => Err(e),
        (Sink::Zip(z), Ok(())) => z.finalize(),
        (Sink::Dir(ds), Ok(())) => ds.finalize(),
    }
}

async fn dump(c: &Ctx, o: &ExportOptions, d: &descriptor::Descriptor, sink: &mut Sink, target: &Path) -> CliResult {
    let mut conn = c.pool.acquire().await?;
    let crypt = match &o.passphrase {
        Some(p) => {
            let salt: [u8; 16] = rand::random();
            let salt = fernet::hex_encode(&salt[..d.crypt_defaults.salt_size.min(16)]);
            let f = fernet::derive(p, &salt, d.crypt_defaults.iterations, d.crypt_defaults.key_size).map_err(CliError::Other)?;
            Some(Crypt {
                fernet: f,
                salt,
                iterations: d.crypt_defaults.iterations,
                key_size: d.crypt_defaults.key_size,
                algorithm: d.crypt_defaults.algorithm.clone(),
            })
        }
        None => {
            let mails: i64 = sqlx::query_scalar("SELECT count(*) FROM paperless_mail_mailaccount").fetch_one(&mut *conn).await?;
            let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM socialaccount_socialtoken").fetch_one(&mut *conn).await?;
            if mails > 0 || tokens > 0 {
                println!("No passphrase was given, sensitive fields will be in plaintext");
            }
            None
        }
    };

    // Manifest stream: folder → manifest.json.tmp (then rename), ZIP → spool file in SCRATCH_DIR
    let (manifest_tmp, manifest_target) = match sink {
        Sink::Dir(_) => {
            let t = target.join("manifest.json");
            let tmp = target.join("manifest.json.tmp");
            if let Sink::Dir(ds) = sink {
                ds.snapshot.remove(&tmp);
            }
            (tmp, Some(t))
        }
        Sink::Zip(_) => {
            std::fs::create_dir_all(&c.cfg.scratch_dir)?;
            let tmp = c.cfg.scratch_dir.join(format!("export-manifest-{}.json", uuid::Uuid::new_v4().simple()));
            (tmp, None)
        }
    };
    let mut writer = ManifestWriter::new(&manifest_tmp)?;
    let res = dump_records(c, o, d, sink, &mut conn, &mut writer, crypt.as_ref()).await;
    if let Err(e) = res {
        drop(writer);
        let _ = std::fs::remove_file(&manifest_tmp);
        return Err(e);
    }
    writer.close()?;
    match (sink as &mut Sink, manifest_target) {
        (Sink::Dir(ds), Some(t)) => ds.commit_stream(&t, &manifest_tmp)?,
        (Sink::Zip(z), _) => z.pending_manifest = Some((manifest_tmp, "manifest.json".into())),
        _ => {}
    }
    let mut metadata = Map::new();
    metadata.insert("version".into(), Value::String(d.version.clone()));
    if let Some(cr) = &crypt {
        let mut m = Map::new();
        m.insert(EXPORTER_CRYPTO_ALGO_NAME.into(), Value::String(cr.algorithm.clone()));
        m.insert(EXPORTER_CRYPTO_KEY_ITERATIONS_NAME.into(), Value::from(cr.iterations));
        m.insert(EXPORTER_CRYPTO_KEY_SIZE_NAME.into(), Value::from(cr.key_size));
        m.insert(EXPORTER_CRYPTO_SALT_NAME.into(), Value::String(cr.salt.clone()));
        metadata.insert(EXPORTER_CRYPTO_SETTINGS_NAME.into(), Value::Object(m));
    }
    sink.add_json(&pyjson::dumps(&Value::Object(metadata), Some(2)), "metadata.json")?;
    // PaperRust extensions (else branches, AI conditions, history, settings …) in a separate
    // file that paperless-ngx ignores (tests/chaos C6, finding C6-1).
    let extras = super::extras::dump(&mut conn).await?;
    sink.add_json(&pyjson::dumps(&extras, Some(2)), super::extras::FILE_NAME)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn dump_records(
    c: &Ctx,
    o: &ExportOptions,
    d: &descriptor::Descriptor,
    sink: &mut Sink,
    conn: &mut PgConnection,
    writer: &mut ManifestWriter,
    crypt: Option<&Crypt>,
) -> CliResult {
    let batch = o.batch_size.max(1);
    let mut tx = conn.begin().await?;
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY; SET LOCAL TimeZone = 'UTC'; SET LOCAL DateStyle = 'ISO, MDY'; SET LOCAL extra_float_digits = 1")
        .execute(&mut *tx)
        .await?;
    let mut documents: Vec<Map<String, Value>> = Vec::new();
    let mut bundles: Vec<Map<String, Value>> = Vec::new();
    // Tags without stored treenode fields (pl_objects.0001_paperrust_tag_adjacency): compute values for
    // the v3.4.2 manifest from the adjacency list (export::tagtree).
    let tag_tree = if super::tagtree::has_columns(&mut tx).await? { None } else { Some(super::tagtree::computed(&mut tx).await?) };
    for key in &d.keys {
        if key.audit_only && !c.cfg.audit_log_enabled {
            continue;
        }
        let Some(model) = d.models.get(&key.model) else {
            return Err(CliError::Other(format!("descriptor: unknown model {}", key.model)));
        };
        match key.key.as_str() {
            "documents" => {
                for_each_record(&mut tx, model, &key.sql, &key.params, batch, |mut r| {
                    encrypt_inline(crypt, &mut r);
                    documents.push(r);
                    Ok(())
                })
                .await?
            }
            "share_link_bundles" => {
                for_each_record(&mut tx, model, &key.sql, &key.params, batch, |mut r| {
                    encrypt_inline(crypt, &mut r);
                    bundles.push(r);
                    Ok(())
                })
                .await?
            }
            "notes" | "custom_field_instances" | "document_barcodes" if o.split_manifest => {}
            _ if key.model == super::tagtree::TAG_MODEL && tag_tree.is_some() => {
                let tree = tag_tree.as_ref();
                let sql = super::tagtree::placeholder_sql(&key.sql);
                for_each_record(&mut tx, model, &sql, &key.params, batch, |mut r| {
                    if let Some(t) = tree {
                        super::tagtree::fill(&mut r, t);
                    }
                    encrypt_inline(crypt, &mut r);
                    writer.write_record(&Value::Object(r))
                })
                .await?
            }
            _ => {
                for_each_record(&mut tx, model, &key.sql, &key.params, batch, |mut r| {
                    encrypt_inline(crypt, &mut r);
                    writer.write_record(&Value::Object(r))
                })
                .await?
            }
        }
    }

    // Files per document
    let app_cfg = pl_files::db::app_config(&c.pool).await?;
    let remote_tiff = pl_files::db::remote_ocr_active(app_cfg.as_ref(), &c.cfg);
    let topts = c.templating_options();
    let mut exported: HashSet<String> = HashSet::new();
    let mut corr_cache: HashMap<i64, Option<String>> = HashMap::new();
    for mut rec in documents {
        let pk = rec.get("pk").and_then(Value::as_i64).unwrap_or_default();
        let Some(row) = pl_consumer::db::load_document(&mut tx, pk).await? else {
            return Err(CliError::Command(format!("Document export ordering mismatch: document pk={pk} vanished")));
        };
        let root = match row.root_document_id {
            Some(r) => pl_consumer::db::load_document(&mut tx, r).await?,
            None => None,
        };
        let ctxdoc = root.as_ref().unwrap_or(&row);
        let root_correspondent = match ctxdoc.correspondent_id {
            Some(cid) => {
                if let Some(n) = corr_cache.get(&cid) {
                    n.clone()
                } else {
                    let n = pl_consumer::db::object_name(&mut tx, "documents_correspondent", cid).await?;
                    corr_cache.insert(cid, n.clone());
                    n
                }
            }
            None => None,
        };
        let info = DocInfo { row: row.clone(), root, root_correspondent };
        let file_type = pl_files::naming::default_file_extension(&row.mime_type, remote_tiff && row.mime_type == "image/tiff");

        // generate_base_name
        let mut counter: u32 = 0;
        let base_name = loop {
            let base = if o.use_filename_format {
                let ctx = pl_consumer::templating::document_context(&mut tx, &row).await?;
                let gopt = pl_templating::GenerateOptions { counter, archive_filename: false, use_format: true };
                pl_templating::generate_filename(&ctx, gopt, &topts).map_err(|e| CliError::Other(format!("filename template: {e}")))?
            } else {
                public_filename(&info, file_type, counter)
            };
            let base = pypath::posix(&base);
            let unique = pypath::without_suffix(&base);
            if exported.insert(unique) {
                break base;
            }
            counter += 1;
        };

        // generate_document_targets
        let mut original_arc = base_name.clone();
        if o.use_folder_prefix {
            original_arc = format!("originals/{original_arc}");
        }
        rec.insert(EXPORTER_FILE_NAME.into(), Value::String(original_arc.clone()));
        let thumbnail_arc = if !o.no_thumbnail {
            let mut t = pypath::join(pypath::parent(&base_name), &format!("{}{THUMBNAIL_SUFFIX}", pypath::stem(&base_name)));
            if o.use_folder_prefix {
                t = format!("thumbnails/{t}");
            }
            rec.insert(EXPORTER_THUMBNAIL_NAME.into(), Value::String(t.clone()));
            Some(t)
        } else {
            None
        };
        let archive_arc = if !o.no_archive && row.archive_filename.is_some() {
            let mut t = pypath::join(pypath::parent(&base_name), &format!("{}{ARCHIVE_SUFFIX}", pypath::stem(&base_name)));
            if o.use_folder_prefix {
                t = format!("archive/{t}");
            }
            rec.insert(EXPORTER_ARCHIVE_NAME.into(), Value::String(t.clone()));
            Some(t)
        } else {
            None
        };

        if !o.data_only {
            let fname = match row.filename.as_deref() {
                Some(f) if !f.is_empty() => f.to_string(),
                _ => format!("{:07}{file_type}", row.id),
            };
            sink.add_file(&c.source_path(&fname), &original_arc, Some(&row.checksum))?;
            if let Some(t) = &thumbnail_arc {
                sink.add_file(&c.thumbnail_path(row.id), t, None)?;
            }
            if let (Some(t), Some(af)) = (&archive_arc, row.archive_filename.as_deref()) {
                sink.add_file(&c.archive_path(af), t, row.archive_checksum.as_deref())?;
            }
        }

        if o.split_manifest {
            let mut content = vec![Value::Object(rec)];
            for name in ["notes", "custom_field_instances", "document_barcodes"] {
                let Some(sq) = d.split.get(name) else { continue };
                let Some(model) = d.models.get(&sq.model) else { continue };
                // text format like with the cursor (NUMERIC of the computed column value_monetary_amount)
                let rows = sqlx::raw_sql(&inline_params(&sq.sql, &[Value::from(pk)])).fetch_all(&mut *tx).await?;
                for r in &rows {
                    content.push(Value::Object(build_record(&mut tx, model, r).await?));
                }
            }
            let mut manifest_name = pypath::join(pypath::parent(&base_name), &format!("{}-manifest.json", pypath::stem(&base_name)));
            if o.use_folder_prefix {
                manifest_name = format!("json/{manifest_name}");
            }
            sink.add_json(&pyjson::dumps(&Value::Array(content), Some(2)), &manifest_name)?;
        } else {
            writer.write_record(&Value::Object(rec))?;
        }
    }

    // Share link bundles
    for mut b in bundles {
        let file_path = b.get("fields").and_then(|f| f.get("file_path")).and_then(Value::as_str).map(str::to_string);
        let arc = match file_path.filter(|s| !s.is_empty()) {
            Some(fp) => {
                let stored = Path::new(&fp);
                let portable = if stored.is_absolute() { pypath::name(&fp.replace('\\', "/")).to_string() } else { pypath::posix(&fp) };
                if let Some(f) = b.get_mut("fields").and_then(Value::as_object_mut) {
                    f.insert("file_path".into(), Value::String(portable.clone()));
                }
                let arc = format!("share_link_bundles/{portable}");
                b.insert(EXPORTER_SHARE_LINK_BUNDLE_NAME.into(), Value::String(arc.clone()));
                Some((arc, fp))
            }
            None => None,
        };
        if !o.data_only
            && let Some((arc, fp)) = &arc
        {
            // `bundle.absolute_file_path` (relative_to after resolve, review M5)
            let Some(src) = super::importer::bundle_target(&c.cfg.share_link_bundle_dir, fp) else {
                return Err(CliError::Other(format!("Share link bundle {} has no file path", b.get("pk").cloned().unwrap_or(Value::Null))));
            };
            sink.add_file(&src, arc, None)?;
        }
        writer.write_record(&Value::Object(b))?;
    }
    tx.rollback().await?;
    Ok(())
}

/// `--zip-name`: plain file name (review C-N3).
fn zip_name_ok(zn: &str) -> bool {
    !(zn.is_empty() || zn.contains(['/', '\\', '\0']) || zn == "." || zn == ".." || (zn.len() >= 2 && zn.as_bytes()[1] == b':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_name_must_be_plain() {
        assert!(zip_name_ok("export-2026-11-08"));
        assert!(zip_name_ok("exp-test.v2"));
        for bad in ["../x", "sub/x", "..\\x", "C:x", "..", "", "/tmp/x"] {
            assert!(!zip_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn dir_sink_refuses_dotdot_arcname() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("out");
        std::fs::create_dir_all(&target).unwrap();
        let src = d.path().join("src.pdf");
        std::fs::write(&src, b"%PDF").unwrap();
        let mut ds =
            DirSink { target: target.clone(), compare_checksums: false, compare_json: false, delete: false, snapshot: HashSet::new() };
        assert!(ds.add_file(&src, "originals/../../escape.pdf", None).is_err());
        assert!(ds.add_json("[]", "../manifest.json").is_err());
        assert!(!d.path().join("escape.pdf").exists() && !d.path().join("manifest.json").exists());
        ds.add_file(&src, "originals/ok.pdf", None).unwrap();
        assert!(target.join("originals").join("ok.pdf").exists());
    }

    #[test]
    fn inline_params_quotes() {
        assert_eq!(inline_params("x IN ($1, $2) AND y = $10", &[Value::from("a'b"), Value::from(2)]), "x IN ('a''b', 2) AND y = $10");
    }
}
