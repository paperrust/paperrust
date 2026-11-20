//! `document_importer`.
//!
//! upstream: src/documents/management/commands/document_importer.py (v3.4.2): `pre_check`,
//! `load_metadata`, `load_manifest_files`, `check_manifest_validity`, `decrypt_secret_fields`,
//! `load_data_to_database` (delete ContentType/Permission, `bulk_create(update_conflicts=True)`
//! per model in batches, m2m `.set()`, `check_constraints`, `sequence_reset_sql`),
//! `_import_files_from_manifest`, afterwards `document_index reindex`.
//! Django behavior: `Field.to_python`, `Field.get_default`, `pre_save` (auto_now/auto_now_add →
//! now, also on import), collector cascade (CASCADE/SET_NULL/PROTECT).

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sqlx::{Connection, PgConnection, Row};

use super::descriptor::{self, Field, Model};
use super::fernet::{self, Fernet};
use super::medialock::MediaLock;
use super::safepath;
use super::zip::{ZipReader, is_zipfile};
use super::*;
use crate::args::{self, flag, value};
use crate::ctx::Ctx;
use crate::pyjson::py_float_repr;
use crate::{CliError, CliResult};

const OPTS: &[args::Opt] = &[
    args::PROGRESS,
    flag(&["--data-only"], "data_only"),
    value(&["--passphrase"], "passphrase"),
    value(&["--batch-size"], "batch_size"),
];

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub source: PathBuf,
    pub data_only: bool,
    pub passphrase: Option<String>,
    pub batch_size: usize,
    /// Rebuild the search index after the import (upstream always; tests can switch it off).
    pub reindex: bool,
}

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 1)?;
    let Some(source) = a.positionals.first() else {
        return Err(CliError::Usage("the following arguments are required: source".into()));
    };
    let c = Ctx::connect().await?;
    let opts = ImportOptions {
        source: PathBuf::from(source),
        data_only: a.flag("data_only"),
        passphrase: a.value("passphrase").map(str::to_string),
        batch_size: a.int("batch_size")?.unwrap_or(500).max(1) as usize,
        reindex: true,
    };
    import(&c, &opts).await
}

// ------------------------------------------------------------------ Reading the manifest (stream)

/// Reads a JSON array element by element (like `ijson.items(f, "item", use_float=True)`).
pub struct ManifestIter {
    r: BufReader<std::fs::File>,
    path: PathBuf,
    started: bool,
    done: bool,
}

impl ManifestIter {
    pub fn open(path: &Path) -> CliResult<ManifestIter> {
        let f = std::fs::File::open(path)?;
        Ok(ManifestIter { r: BufReader::with_capacity(1 << 16, f), path: path.to_path_buf(), started: false, done: false })
    }

    fn err(&self, msg: &str) -> CliError {
        CliError::Command(format!("Failed to parse manifest file {}: {msg}", self.path.display()))
    }

    fn peek(&mut self) -> CliResult<Option<u8>> {
        loop {
            let buf = self.r.fill_buf()?;
            let Some(&b) = buf.first() else { return Ok(None) };
            if b.is_ascii_whitespace() {
                self.r.consume(1);
                continue;
            }
            return Ok(Some(b));
        }
    }

    fn byte(&mut self) -> CliResult<Option<u8>> {
        let mut one = [0u8; 1];
        match self.r.read(&mut one)? {
            0 => Ok(None),
            _ => Ok(Some(one[0])),
        }
    }

    fn next_record(&mut self) -> CliResult<Option<Value>> {
        if self.done {
            return Ok(None);
        }
        if !self.started {
            self.started = true;
            match self.peek()? {
                Some(b'[') => self.r.consume(1),
                Some(_) => return Err(self.err("expected a JSON array")),
                None => return Err(self.err("Incomplete JSON content")),
            }
            if self.peek()? == Some(b']') {
                self.r.consume(1);
                self.done = true;
                return Ok(None);
            }
        } else {
            match self.peek()? {
                Some(b',') => self.r.consume(1),
                Some(b']') => {
                    self.r.consume(1);
                    self.done = true;
                    return Ok(None);
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
        // collect one element up to the end of its bracketing
        let mut buf = Vec::new();
        let mut depth = 0i32;
        let mut in_str = false;
        let mut esc = false;
        let first = self.peek()?.ok_or_else(|| self.err("Incomplete JSON content"))?;
        if first != b'{' && first != b'[' {
            // scalar (does not occur in the manifest): read up to , or ]
            loop {
                match self.r.fill_buf()?.first().copied() {
                    Some(b',') | Some(b']') | None => break,
                    Some(b) => {
                        buf.push(b);
                        self.r.consume(1);
                    }
                }
            }
        } else {
            loop {
                let Some(b) = self.byte()? else { return Err(self.err("Incomplete JSON content")) };
                buf.push(b);
                if in_str {
                    if esc {
                        esc = false;
                    } else if b == b'\\' {
                        esc = true;
                    } else if b == b'"' {
                        in_str = false;
                    }
                    continue;
                }
                match b {
                    b'"' => in_str = true,
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        serde_json::from_slice(&buf).map(Some).map_err(|e| self.err(&e.to_string()))
    }
}

impl Iterator for ManifestIter {
    type Item = CliResult<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_record() {
            Ok(Some(v)) => Some(Ok(v)),
            Ok(None) => None,
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

// ------------------------------------------------------------------ Field values (to_python)

/// Python `str(value)` for JSON values (CharField.to_python).
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() => py_float_repr(f),
            _ => n.to_string(),
        },
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

fn deser(model: &str, pk: &Value, field: &str, v: &Value, why: &str) -> CliError {
    CliError::Command(format!(
        "Could not coerce {field}={} for {model}(pk={}): {why}",
        crate::pyjson::dumps(v, None),
        crate::pyjson::dumps(pk, None)
    ))
}

/// A bound parameter.
#[derive(Debug, Clone)]
pub enum Param {
    Text(Option<String>),
    Json(Option<Value>),
}

fn is_text_type(db_type: &str) -> bool {
    db_type.starts_with("varchar") || db_type == "text" || db_type.starts_with("character varying")
}

/// `Field.to_python(value)` → text form for `$n::text::<db_type>`.
fn to_python(model: &str, pk: &Value, f: &Field, v: &Value) -> CliResult<Param> {
    if f.db_type == "jsonb" {
        return Ok(Param::Json(if v.is_null() { None } else { Some(v.clone()) }));
    }
    if v.is_null() {
        return Ok(Param::Text(None));
    }
    let it = f.internal_type.as_str();
    let s = match it {
        "BooleanField" | "NullBooleanField" => match v {
            Value::Bool(b) => b.to_string(),
            Value::Number(n) if n.as_i64() == Some(1) => "true".into(),
            Value::Number(n) if n.as_i64() == Some(0) => "false".into(),
            Value::String(s) if matches!(s.as_str(), "t" | "True" | "1") => "true".into(),
            Value::String(s) if matches!(s.as_str(), "f" | "False" | "0") => "false".into(),
            _ => return Err(deser(model, pk, &f.name, v, "value must be either True or False.")),
        },
        "AutoField" | "BigAutoField" | "SmallAutoField" | "IntegerField" | "BigIntegerField" | "SmallIntegerField"
        | "PositiveIntegerField" | "PositiveSmallIntegerField" | "PositiveBigIntegerField" | "ForeignKey" | "OneToOneField" => match v {
            Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                (Some(i), _) => i.to_string(),
                (None, Some(f)) if f.is_finite() => (f.trunc() as i64).to_string(),
                _ => return Err(deser(model, pk, &f.name, v, "value must be an integer.")),
            },
            Value::String(s) if s.trim().parse::<i64>().is_ok() => s.trim().to_string(),
            Value::Bool(b) => (*b as i64).to_string(),
            _ => {
                // FK to models with a text primary key
                if f.kind == "fk" && f.db_type.starts_with("varchar") {
                    py_str(v)
                } else {
                    return Err(deser(model, pk, &f.name, v, "value must be an integer."));
                }
            }
        },
        "FloatField" => match v {
            Value::Number(n) => n.as_f64().map(py_float_repr).unwrap_or_else(|| n.to_string()),
            Value::String(s) if s.trim().parse::<f64>().is_ok() => s.trim().to_string(),
            _ => return Err(deser(model, pk, &f.name, v, "value must be a float.")),
        },
        "DateTimeField" | "DateField" | "TimeField" => match v {
            Value::String(s) => s.clone(),
            _ => return Err(deser(model, pk, &f.name, v, "value has an invalid format.")),
        },
        _ => py_str(v),
    };
    Ok(Param::Text(Some(s)))
}

/// `Field.get_default()` or `pre_save(add=True)` for missing fields.
fn default_param(f: &Field, now: &str, today: &str) -> Param {
    if f.db_type == "jsonb" {
        return match (&f.default.kind[..], f.default.name.as_deref()) {
            ("value", _) => Param::Json(f.default.value.clone().filter(|v| !v.is_null())),
            ("callable", Some("builtins.dict")) => Param::Json(Some(Value::Object(Map::new()))),
            ("callable", Some("builtins.list")) => Param::Json(Some(Value::Array(Vec::new()))),
            _ => Param::Json(None),
        };
    }
    match (&f.default.kind[..], f.default.name.as_deref()) {
        ("value", _) => match &f.default.value {
            None | Some(Value::Null) => Param::Text(None),
            Some(Value::Bool(b)) => Param::Text(Some(b.to_string())),
            Some(v) => Param::Text(Some(py_str(v))),
        },
        ("callable", Some(n)) if n.ends_with("timezone.now") => Param::Text(Some(now.to_string())),
        ("callable", Some(n)) if n.ends_with("date.today") => Param::Text(Some(today.to_string())),
        _ => {
            let empty_ok = matches!(
                f.internal_type.as_str(),
                "CharField" | "TextField" | "SlugField" | "FilePathField" | "FileField" | "EmailField" | "URLField"
                    | "GenericIPAddressField"
            );
            if empty_ok && !f.null { Param::Text(Some(String::new())) } else { Param::Text(None) }
        }
    }
}

// ------------------------------------------------------------------ DB loading

/// (column values, m2m lists per field index, primary key)
type PendingRow = (Vec<Param>, Vec<(usize, Vec<i64>)>, Param);

struct Pending {
    rows: Vec<PendingRow>,
}

struct Loader<'a> {
    d: &'a descriptor::Descriptor,
    batch: usize,
    pending: BTreeMap<String, Pending>,
    order: Vec<String>,
    loaded: Vec<String>,
    now: String,
    today: String,
    /// `documents_tag` without treenode columns (PaperRust migration) → skip these fields.
    skip_treenode: bool,
    /// Table → is a compatibility view of the own data model (docs/DATAMODEL-V2.md):
    /// `INSERT … ON CONFLICT` does not work there (INSTEAD-OF trigger), UPDATE + INSERT instead.
    views: BTreeMap<String, bool>,
}

impl Loader<'_> {
    fn insert_columns<'m>(&self, model: &'m Model) -> Vec<&'m Field> {
        model
            .fields
            .iter()
            .filter(|f| !f.generated && !(self.skip_treenode && super::tagtree::is_derived(&model.label, &f.name)))
            .collect()
    }

    fn add(&mut self, rec: &Value) -> CliResult<Option<String>> {
        let label = rec.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
        let Some(model) = self.d.models.get(&label) else {
            return Err(CliError::Command(format!("Invalid model identifier: {label}")));
        };
        let pk = rec.get("pk").cloned().unwrap_or(Value::Null);
        let empty = Map::new();
        let fields = rec.get("fields").and_then(Value::as_object).unwrap_or(&empty);
        for name in fields.keys() {
            if model.fields.iter().all(|f| &f.name != name) && model.m2m.iter().all(|m| &m.name != name) {
                let class = label.split('.').nth(1).unwrap_or(&label);
                return Err(CliError::Command(format!("{class} has no field named '{name}'")));
            }
        }
        let mut params = Vec::new();
        let cols = self.insert_columns(model);
        for f in &cols {
            let p = if f.primary_key {
                match to_python(&label, &pk, f, &pk)? {
                    Param::Text(None) => {
                        return Err(CliError::Command(format!("Could not coerce pk={} for {label}: missing", crate::pyjson::dumps(&pk, None))));
                    }
                    p => p,
                }
            } else if f.auto_now || f.auto_now_add {
                Param::Text(Some(self.now.clone()))
            } else {
                match fields.get(&f.name) {
                    Some(v) => to_python(&label, &pk, f, v)?,
                    None => default_param(f, &self.now, &self.today),
                }
            };
            params.push(p);
        }
        let mut m2m = Vec::new();
        for (i, m) in model.m2m.iter().enumerate() {
            if let Some(v) = fields.get(&m.name) {
                let mut ids = Vec::new();
                for x in v.as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    let id = match x {
                        Value::Number(n) => n.as_i64(),
                        Value::String(s) => s.trim().parse().ok(),
                        _ => None,
                    }
                    .ok_or_else(|| deser(&label, &pk, &m.name, x, "value must be an integer."))?;
                    ids.push(id);
                }
                m2m.push((i, ids));
            }
        }
        let pk_param = params.first().cloned().unwrap_or(Param::Text(None));
        let pk_param = cols.iter().position(|f| f.primary_key).and_then(|i| params.get(i).cloned()).unwrap_or(pk_param);
        let entry = self.pending.entry(label.clone()).or_insert_with(|| Pending { rows: Vec::new() });
        if !self.order.contains(&label) {
            self.order.push(label.clone());
        }
        entry.rows.push((params, m2m, pk_param));
        let full = entry.rows.len() >= self.batch;
        Ok(full.then_some(label))
    }

    async fn flush(&mut self, conn: &mut PgConnection, label: &str) -> CliResult {
        let Some(p) = self.pending.remove(label) else { return Ok(()) };
        if p.rows.is_empty() {
            return Ok(());
        }
        let Some(model) = self.d.models.get(label) else { return Ok(()) };
        let cols = self.insert_columns(model);
        let col_names: Vec<String> = cols.iter().map(|f| format!("\"{}\"", f.column)).collect();
        let mut vals = String::new();
        let mut n = 0;
        for (ri, _) in p.rows.iter().enumerate() {
            if ri > 0 {
                vals.push_str(", ");
            }
            vals.push('(');
            for (ci, f) in cols.iter().enumerate() {
                if ci > 0 {
                    vals.push_str(", ");
                }
                n += 1;
                if f.db_type == "jsonb" {
                    vals.push_str(&format!("${n}::jsonb"));
                } else if is_text_type(&f.db_type) {
                    vals.push_str(&format!("${n}::text"));
                } else {
                    vals.push_str(&format!("${n}::text::{}", f.db_type));
                }
            }
            vals.push(')');
        }
        let is_view = match self.views.get(&model.table) {
            Some(v) => *v,
            None => {
                let v: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace                       WHERE n.nspname = current_schema() AND c.relname = $1 AND c.relkind = 'v')",
                )
                .bind(&model.table)
                .fetch_one(&mut *conn)
                .await?;
                self.views.insert(model.table.clone(), v);
                v
            }
        };
        let updates: Vec<String> = cols.iter().filter(|f| !f.primary_key).map(|f| format!("\"{0}\" = EXCLUDED.\"{0}\"", f.column)).collect();
        let statements: Vec<String> = if is_view {
            // compatibility view: first update existing rows, then insert missing ones (like
            // Django's save(): UPDATE, otherwise INSERT)
            let t = &model.table;
            let pk = &model.pk_column;
            let sets: Vec<String> = cols.iter().filter(|f| !f.primary_key).map(|f| format!("\"{0}\" = v.\"{0}\"", f.column)).collect();
            let mut st = Vec::new();
            if !sets.is_empty() {
                st.push(format!(
                    "UPDATE \"{t}\" AS t SET {} FROM (VALUES {vals}) AS v ({}) WHERE t.\"{pk}\" = v.\"{pk}\"",
                    sets.join(", "),
                    col_names.join(", ")
                ));
            }
            st.push(format!(
                "INSERT INTO \"{t}\" ({cl}) SELECT * FROM (VALUES {vals}) AS v ({cl})                  WHERE NOT EXISTS (SELECT 1 FROM \"{t}\" x WHERE x.\"{pk}\" = v.\"{pk}\")",
                cl = col_names.join(", ")
            ));
            st
        } else {
            vec![format!(
                "INSERT INTO \"{}\" ({}) VALUES {vals} ON CONFLICT (\"{}\") DO UPDATE SET {}",
                model.table,
                col_names.join(", "),
                model.pk_column,
                updates.join(", ")
            )]
        };
        for sql in &statements {
            let mut q = sqlx::query(sql);
            for (params, _, _) in &p.rows {
                for prm in params {
                    q = match prm {
                        Param::Text(t) => q.bind(t.clone()),
                        Param::Json(j) => q.bind(j.clone().map(sqlx::types::Json)),
                    };
                }
            }
            q.execute(&mut *conn).await.map_err(import_failed)?;
        }
        if !self.loaded.iter().any(|l| l == label) {
            self.loaded.push(label.to_string());
        }
        // m2m .set()
        for (_, m2m, pkp) in &p.rows {
            let Param::Text(Some(pk)) = pkp else { continue };
            let Ok(pk) = pk.parse::<i64>() else { continue };
            for (mi, ids) in m2m {
                let m = &model.m2m[*mi];
                sqlx::query(&format!(
                    "DELETE FROM \"{t}\" WHERE \"{s}\" = $1 AND NOT (\"{d}\" = ANY($2::int8[]))",
                    t = m.through_table,
                    s = m.src_column,
                    d = m.dst_column
                ))
                .bind(pk)
                .bind(ids)
                .execute(&mut *conn)
                .await
                .map_err(import_failed)?;
                if !ids.is_empty() {
                    sqlx::query(&format!(
                        "INSERT INTO \"{t}\" (\"{s}\", \"{d}\") SELECT $1, x FROM unnest($2::int8[]) WITH ORDINALITY AS u(x, o)
                         WHERE NOT EXISTS (SELECT 1 FROM \"{t}\" WHERE \"{s}\" = $1 AND \"{d}\" = u.x) ORDER BY o",
                        t = m.through_table,
                        s = m.src_column,
                        d = m.dst_column
                    ))
                    .bind(pk)
                    .bind(crate::pyset::missing_targets_order(ids))
                    .execute(&mut *conn)
                    .await
                    .map_err(import_failed)?;
                }
            }
        }
        Ok(())
    }

    async fn flush_all(&mut self, conn: &mut PgConnection) -> CliResult {
        let labels: Vec<String> = self.order.iter().filter(|l| self.pending.contains_key(*l)).cloned().collect();
        for l in labels {
            self.flush(conn, &l).await?;
        }
        Ok(())
    }
}


fn import_failed(e: sqlx::Error) -> CliError {
    CliError::Command(format!("Database import failed: {e}"))
}

/// `Model.objects.all().delete()` with Django's cascade (fk_refs from the descriptor).
pub(crate) fn cascade_delete<'a>(
    conn: &'a mut PgConnection,
    d: &'a descriptor::Descriptor,
    label: &'a str,
    ids: Vec<i64>,
) -> futures_lite::BoxFuture<'a, CliResult> {
    Box::pin(async move {
        if ids.is_empty() {
            return Ok(());
        }
        for r in d.fk_refs.iter().filter(|r| r.target == label) {
            match r.on_delete.as_deref() {
                Some("CASCADE") => {
                    let rows = sqlx::query(&format!(
                        "DELETE FROM \"{}\" WHERE \"{}\" = ANY($1::int8[]) RETURNING \"{}\"::text",
                        r.table, r.column, r.pk_column
                    ))
                    .bind(&ids)
                    .fetch_all(&mut *conn)
                    .await?;
                    if let Some(m) = d.models.values().find(|m| m.table == r.table) {
                        let child: Vec<i64> = rows.iter().filter_map(|x| x.try_get::<String, _>(0).ok()?.parse().ok()).collect();
                        let lbl = m.label.clone();
                        cascade_delete(conn, d, &lbl, child).await?;
                    }
                }
                Some("SET_NULL") => {
                    sqlx::query(&format!("UPDATE \"{0}\" SET \"{1}\" = NULL WHERE \"{1}\" = ANY($1::int8[])", r.table, r.column))
                        .bind(&ids)
                        .execute(&mut *conn)
                        .await?;
                }
                Some("PROTECT") | Some("RESTRICT") => {
                    let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM \"{}\" WHERE \"{}\" = ANY($1::int8[])", r.table, r.column))
                        .bind(&ids)
                        .fetch_one(&mut *conn)
                        .await?;
                    if n > 0 {
                        return Err(CliError::Command(format!("Cannot delete some instances of model '{label}' because they are referenced through protected foreign keys: '{}'", r.table)));
                    }
                }
                _ => {}
            }
        }
        if let Some(m) = d.models.get(label) {
            sqlx::query(&format!("DELETE FROM \"{}\" WHERE \"{}\" = ANY($1::int8[])", m.table, m.pk_column)).bind(&ids).execute(&mut *conn).await?;
        }
        Ok(())
    })
}

mod futures_lite {
    pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
}

async fn delete_all(conn: &mut PgConnection, d: &descriptor::Descriptor, label: &str) -> CliResult {
    let Some(m) = d.models.get(label) else { return Ok(()) };
    let ids: Vec<i64> = sqlx::query_scalar::<_, i64>(&format!("SELECT \"{}\"::int8 FROM \"{}\"", m.pk_column, m.table)).fetch_all(&mut *conn).await?;
    cascade_delete(conn, d, label, ids).await
}

// ------------------------------------------------------------------ Flow

struct Source {
    dir: PathBuf,
    tmp: Option<PathBuf>,
}

impl Drop for Source {
    fn drop(&mut self) {
        if let Some(t) = &self.tmp {
            let _ = std::fs::remove_dir_all(t);
        }
    }
}

fn manifest_paths(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![dir.join("manifest.json")];
    let mut extra = Vec::new();
    // `Path.glob("**/…")` does not descend into directory symlinks; PaperRust additionally depth ≤ 64
    // (review C-N2)
    fn walk(p: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > MAX_WALK_DEPTH {
            return;
        }
        let Ok(rd) = std::fs::read_dir(p) else { return };
        // directory order of the file system like Python's `Path.glob` (os.scandir): first the
        // files of this folder, then the subfolders
        let entries: Vec<_> = rd.flatten().collect();
        let mut dirs = Vec::new();
        for e in entries {
            let path = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                dirs.push(path);
            } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("-manifest.json")) {
                out.push(path);
            }
        }
        for d in dirs {
            walk(&d, out, depth + 1);
        }
    }
    walk(dir, &mut extra, 0);
    out.extend(extra);
    out
}

pub async fn import(c: &Ctx, o: &ImportOptions) -> CliResult {
    let d = descriptor::get();
    let src = std::path::absolute(&o.source).unwrap_or_else(|_| o.source.clone());
    let source = if is_zipfile(&src) {
        let mut z = ZipReader::open(&src)?;
        let bad = z.unsupported_methods();
        if !bad.is_empty() {
            let mut names: Vec<String> = bad
                .iter()
                .map(|m| match m {
                    12 => "bzip2".to_string(),
                    14 => "lzma".to_string(),
                    93 => "zstd".to_string(),
                    o => format!("method {o}"),
                })
                .collect();
            names.sort();
            let mut msg = format!("This archive uses compression this Python version cannot read ({}).", names.join(", "));
            if names.iter().any(|n| n == "zstd") {
                msg.push_str(" zstd archives require Python 3.14+.");
            }
            return Err(CliError::Command(msg));
        }
        std::fs::create_dir_all(&c.cfg.scratch_dir)?;
        let tmp = c.cfg.scratch_dir.join(format!("import-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&tmp)?;
        let s = Source { dir: tmp.clone(), tmp: Some(tmp.clone()) };
        z.extract_all(&tmp)?;
        s
    } else {
        Source { dir: src.clone(), tmp: None }
    };
    let dir = source.dir.clone();

    // pre_check
    if !dir.exists() {
        return Err(CliError::Command("That path doesn't exist".into()));
    }
    if std::fs::read_dir(&dir).is_err() && !dir.is_file() {
        return Err(CliError::Command("That path doesn't appear to be readable".into()));
    }
    if !o.data_only {
        for docdir in [&c.cfg.originals_dir, &c.cfg.archive_dir] {
            if let Some(f) = first_file(docdir) {
                let rel = f.strip_prefix(docdir).unwrap_or(&f).to_string_lossy().replace('\\', "/");
                println!("Found file {rel}, this might indicate a non-empty installation");
            }
        }
    }
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_user WHERE NOT username = ANY($1)")
        .bind(&d.excluded_usernames)
        .fetch_one(&c.pool)
        .await?;
    if users != 0 {
        println!("Found existing user(s), this might indicate a non-empty installation");
    }
    let docs: i64 = sqlx::query_scalar("SELECT count(*) FROM documents_document").fetch_one(&c.pool).await?;
    if docs != 0 {
        println!("Found existing documents(s), this might indicate a non-empty installation");
    }
    if !dir.join("manifest.json").exists() {
        return Err(CliError::Command("That directory doesn't appear to contain a manifest.json file.".into()));
    }

    // load_metadata
    let mut crypt: Option<Fernet> = None;
    let version_path = dir.join("version.json");
    let metadata_path = dir.join("metadata.json");
    let mut version: Option<String> = None;
    if !version_path.exists() && !metadata_path.exists() {
        println!("No version.json or metadata.json file located");
    } else if metadata_path.exists() {
        let data: Value = serde_json::from_slice(&std::fs::read(&metadata_path)?).map_err(|e| CliError::Other(format!("metadata.json: {e}")))?;
        version = data.get("version").and_then(Value::as_str).map(str::to_string);
        if let Some(cs) = data.get(EXPORTER_CRYPTO_SETTINGS_NAME) {
            let Some(pass) = &o.passphrase else {
                return Err(CliError::Command("No passphrase was given, but this export contains encrypted fields".into()));
            };
            let algo = cs.get(EXPORTER_CRYPTO_ALGO_NAME).and_then(Value::as_str).unwrap_or_default();
            if algo != "pbkdf2_sha256" {
                return Err(CliError::Command(format!("{algo} is an unknown key derivation function")));
            }
            let (iters, size) = crypto_params(cs)?;
            let salt = cs.get(EXPORTER_CRYPTO_SALT_NAME).and_then(Value::as_str).unwrap_or_default();
            crypt = Some(fernet::derive(pass, salt, iters, size).map_err(CliError::Command)?);
        }
    } else {
        let data: Value = serde_json::from_slice(&std::fs::read(&version_path)?).map_err(|e| CliError::Other(format!("version.json: {e}")))?;
        version = data.get("version").and_then(Value::as_str).map(str::to_string);
    }
    if let Some(v) = &version
        && v != &d.version
    {
        println!("Version mismatch: Currently {}, importing {v}. Continuing, but import may fail.", d.version);
    }

    let paths = manifest_paths(&dir);

    // check_manifest_validity
    println!("Checking the manifest");
    check_manifest_validity(&dir, &paths, o.data_only)?;

    // load_data_to_database
    let mut conn = c.pool.acquire().await?;
    let mut tx = conn.begin().await?;
    sqlx::query(&format!("SET LOCAL TimeZone = '{}'", c.cfg.time_zone.replace('\'', ""))).execute(&mut *tx).await?;
    let now: String = sqlx::query_scalar("SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')").fetch_one(&mut *tx).await?;
    let today = chrono::Utc::now().with_timezone(&c.cfg.tz).date_naive().format("%Y-%m-%d").to_string();
    let r = async {
        delete_all(&mut tx, d, "contenttypes.contenttype").await?;
        delete_all(&mut tx, d, "auth.permission").await?;
        sqlx::raw_sql("SET CONSTRAINTS ALL DEFERRED").execute(&mut *tx).await?;
        let skip_treenode = !super::tagtree::has_columns(&mut tx).await?;
        let mut loader = Loader {
            d,
            batch: o.batch_size,
            pending: BTreeMap::new(),
            order: Vec::new(),
            loaded: Vec::new(),
            now,
            today,
            skip_treenode,
            views: BTreeMap::new(),
        };
        for p in &paths {
            for rec in ManifestIter::open(p)? {
                let mut rec = rec?;
                if let Some(f) = &crypt {
                    decrypt_record(f, &mut rec)?;
                }
                if let Some(full) = loader.add(&rec)? {
                    loader.flush(&mut tx, &full).await?;
                }
            }
        }
        loader.flush_all(&mut tx).await?;
        // PaperRust extensions (tests/chaos C6): after the upstream models, same transaction
        let extras_path = dir.join(super::extras::FILE_NAME);
        if extras_path.is_file() {
            let v: Value = serde_json::from_slice(&std::fs::read(&extras_path)?)
                .map_err(|e| CliError::Other(format!("{}: {e}", super::extras::FILE_NAME)))?;
            let n = super::extras::load(&mut tx, &v).await?;
            println!("Restored {n} PaperRust extension row(s)");
        }
        // _finalize_db_load: check_constraints + sequence_reset_sql
        sqlx::raw_sql("SET CONSTRAINTS ALL IMMEDIATE").execute(&mut *tx).await.map_err(import_failed)?;
        sqlx::raw_sql("SET CONSTRAINTS ALL DEFERRED").execute(&mut *tx).await?;
        for label in &loader.loaded {
            let Some(m) = d.models.get(label) else { continue };
            if !m.has_auto_pk() {
                continue;
            }
            sqlx::query(&format!(
                // compatibility view (docs/DATAMODEL-V2.md): no own sequence, the column has
                // nextval('<native>_id_seq') as default → this sequence
                "SELECT setval(coalesce(pg_get_serial_sequence('\"{t}\"','{c}'), \
                   (SELECT substring(pg_get_expr(ad.adbin, ad.adrelid) FROM 'nextval\\(''([^'']+)''') \
                      FROM pg_attrdef ad JOIN pg_attribute at ON at.attrelid = ad.adrelid AND at.attnum = ad.adnum \
                     WHERE ad.adrelid = '\"{t}\"'::regclass AND at.attname = '{c}')), \
                   coalesce(max(\"{c}\"), 1), max(\"{c}\") IS NOT null) FROM \"{t}\"",
                t = m.table,
                c = m.pk_column
            ))
            .execute(&mut *tx)
            .await?;
        }
        Ok::<(), CliError>(())
    }
    .await;
    if let Err(e) = r {
        println!("Database import failed");
        match &version {
            Some(v) if v != &d.version => println!("Version mismatch: Currently {}, importing {v}", d.version),
            _ => println!("No version information present"),
        }
        let _ = tx.rollback().await;
        return Err(e);
    }
    tx.commit().await?;

    if !o.data_only {
        import_files(c, &dir, &paths).await?;
    } else {
        println!("Data only import completed");
    }

    println!("Updating search index...");
    if o.reindex {
        crate::index::reindex(c, &crate::index::ReindexArgs::default()).await?;
    }
    drop(source);
    Ok(())
}

/// Maximum for `__key_iters__` (upstream writes 1 000 000; PaperRust rejects values above
/// 10 000 000 and 0, review C-N1 – otherwise hours of CPU load or silently truncated values).
pub(crate) const MAX_KEY_ITERATIONS: u64 = 10_000_000;

/// Check `__key_iters__`/`__key_size__` from `metadata.json` (Fernet needs exactly 32 bytes).
pub(crate) fn crypto_params(cs: &Value) -> CliResult<(u32, usize)> {
    let iters = match cs.get(EXPORTER_CRYPTO_KEY_ITERATIONS_NAME) {
        None => 1_000_000,
        Some(v) => v.as_u64().filter(|n| (1..=MAX_KEY_ITERATIONS).contains(n)).ok_or_else(|| {
            CliError::Command(format!("Invalid key iteration count {v} in metadata.json (allowed: 1 to {MAX_KEY_ITERATIONS})"))
        })?,
    };
    let size = match cs.get(EXPORTER_CRYPTO_KEY_SIZE_NAME) {
        None => 32,
        Some(v) => v.as_u64().filter(|n| *n == 32).ok_or_else(|| CliError::Command(format!("Invalid key size {v} in metadata.json (expected 32)")))?,
    };
    Ok((u32::try_from(iters).map_err(|_| CliError::Command("Invalid key iteration count".into()))?, size as usize))
}

/// Recursion depth for directory traversals (review C-N2).
pub(crate) const MAX_WALK_DEPTH: usize = 64;

fn first_file(dir: &Path) -> Option<PathBuf> {
    first_file_at(dir, 0)
}

fn first_file_at(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth > MAX_WALK_DEPTH {
        return None;
    }
    let rd = std::fs::read_dir(dir).ok()?;
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        // do not follow directory symlinks (like `glob("**/*")`), the entry itself counts
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if let Some(f) = first_file_at(&p, depth + 1) {
                return Some(f);
            }
        } else {
            return Some(p);
        }
    }
    None
}

/// `check_manifest_validity`; PaperRust additionally checks containment (review H2/M4).
fn check_manifest_validity(dir: &Path, paths: &[PathBuf], data_only: bool) -> CliResult {
    for p in paths {
        for rec in ManifestIter::open(p)? {
            let rec = rec?;
            // PaperRust (review H2): also check stored file names with --data-only
            if rec.get("model").and_then(Value::as_str) == Some("documents.document") {
                check_stored_names(&rec)?;
            }
            if data_only {
                continue;
            }
            match rec.get("model").and_then(Value::as_str) {
                Some("documents.document") => check_document(dir, &rec)?,
                Some("documents.sharelinkbundle") => {
                    if let Some(f) = rec.get(EXPORTER_SHARE_LINK_BUNDLE_NAME).and_then(Value::as_str)
                        && safepath::source_in(dir, f)?.is_none()
                    {
                        return Err(CliError::Command(format!("The manifest file refers to \"{f}\" which does not appear to be in the source directory.")));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// PaperRust (review H2): `fields.filename`/`archive_filename` of a document must be safe
/// relative paths (upstream takes them unchecked and later writes to
/// `ORIGINALS_DIR / filename`).
fn check_stored_names(rec: &Value) -> CliResult {
    let Some(fields) = rec.get("fields") else { return Ok(()) };
    for name in ["filename", "archive_filename"] {
        if let Some(v) = fields.get(name).and_then(Value::as_str)
            && !v.is_empty()
            && !safepath::is_safe_relative(v)
        {
            let pk = rec.get("pk").cloned().unwrap_or(Value::Null);
            return Err(CliError::Command(format!(
                "Document {pk} has an unsafe {name} \"{v}\" (must be relative, without \"..\" or a drive)."
            )));
        }
    }
    Ok(())
}

fn check_document(dir: &Path, rec: &Value) -> CliResult {
    let Some(doc_file) = rec.get(EXPORTER_FILE_NAME).and_then(Value::as_str) else {
        return Err(CliError::Command("The manifest file contains a record which does not refer to an actual document file.".into()));
    };
    let Some(p) = safepath::source_in(dir, doc_file)? else {
        return Err(CliError::Command(format!("The manifest file refers to \"{doc_file}\" which does not appear to be in the source directory.")));
    };
    std::fs::File::open(&p).map_err(|_| CliError::Command(format!("Failed to read from original file {}", p.display())))?;
    if let Some(t) = rec.get(EXPORTER_THUMBNAIL_NAME).and_then(Value::as_str).filter(|s| !s.is_empty()) {
        safepath::source_in(dir, t)?;
    }
    if let Some(a) = rec.get(EXPORTER_ARCHIVE_NAME).and_then(Value::as_str) {
        let Some(ap) = safepath::source_in(dir, a)? else {
            return Err(CliError::Command(format!("The manifest file refers to {a} which does not appear to be in the source directory.")));
        };
        std::fs::File::open(&ap).map_err(|_| CliError::Command(format!("Failed to read from archive file {}", ap.display())))?;
    }
    Ok(())
}

fn decrypt_record(f: &Fernet, rec: &mut Value) -> CliResult {
    let d = descriptor::get();
    let label = rec.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
    let Some(cf) = d.crypt_fields.iter().find(|c| c.model_name == label) else { return Ok(()) };
    let Some(fields) = rec.get_mut("fields").and_then(Value::as_object_mut) else { return Ok(()) };
    for name in &cf.fields {
        if let Some(Value::String(s)) = fields.get(name)
            && !s.is_empty()
        {
            let plain = f.decrypt_string(s).map_err(|_| CliError::Command("cryptography.fernet.InvalidToken (wrong passphrase?)".into()))?;
            fields.insert(name.clone(), Value::String(plain));
        }
    }
    Ok(())
}

async fn import_files(c: &Ctx, dir: &Path, paths: &[PathBuf]) -> CliResult {
    for d in [&c.cfg.originals_dir, &c.cfg.thumbnail_dir, &c.cfg.archive_dir, &c.cfg.share_link_bundle_dir] {
        std::fs::create_dir_all(d)?;
    }
    println!("Copy files into PaperRust...");
    let app_cfg = pl_files::db::app_config(&c.pool).await?;
    let remote_tiff = pl_files::db::remote_ocr_active(app_cfg.as_ref(), &c.cfg);
    let mut conn = c.pool.acquire().await?;
    let mut bundles: Vec<(i64, String)> = Vec::new();
    let mut by_pk: HashMap<i64, ()> = HashMap::new();
    for p in paths {
        for rec in ManifestIter::open(p)? {
            let rec = rec?;
            let model = rec.get("model").and_then(Value::as_str).unwrap_or_default();
            let pk = rec.get("pk").and_then(Value::as_i64).unwrap_or_default();
            if model == "documents.sharelinkbundle" {
                if let Some(b) = rec.get(EXPORTER_SHARE_LINK_BUNDLE_NAME).and_then(Value::as_str) {
                    bundles.push((pk, b.to_string()));
                }
                continue;
            }
            if model != "documents.document" {
                continue;
            }
            by_pk.insert(pk, ());
            let Some(doc) = pl_consumer::db::load_document(&mut conn, pk).await? else {
                return Err(CliError::Command(format!("Document matching query does not exist. (pk={pk})")));
            };
            let doc_file = rec.get(EXPORTER_FILE_NAME).and_then(Value::as_str).unwrap_or_default();
            let thumb = rec.get(EXPORTER_THUMBNAIL_NAME).and_then(Value::as_str).filter(|s| !s.is_empty());
            let archive = rec.get(EXPORTER_ARCHIVE_NAME).and_then(Value::as_str).filter(|s| !s.is_empty());
            let _lock = MediaLock::acquire(&c.cfg.media_lock).await?;
            let file_type = pl_files::naming::default_file_extension(&doc.mime_type, remote_tiff && doc.mime_type == "image/tiff");
            let fname = match doc.filename.as_deref() {
                Some(f) if !f.is_empty() => f.to_string(),
                _ => format!("{pk:07}{file_type}"),
            };
            let source_path = safepath::target_in(&c.cfg.originals_dir, &fname, "original")?;
            if source_path.is_file() {
                return Err(CliError::Command(format!("FileExistsError: {}", source_path.display())));
            }
            if let Some(parent) = source_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            copy_file_with_basic_stats(&safepath::existing_source_in(dir, doc_file)?, &source_path)?;
            if let Some(t) = thumb {
                copy_file_with_basic_stats(&safepath::existing_source_in(dir, t)?, &c.thumbnail_path(pk))?;
            }
            if let Some(a) = archive {
                let Some(af) = doc.archive_filename.as_deref() else {
                    return Err(CliError::Command(format!("Document {pk} has an archive file in the export but no archive_filename")));
                };
                let ap = safepath::target_in(&c.cfg.archive_dir, af, "archive file")?;
                if let Some(parent) = ap.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                copy_file_with_basic_stats(&safepath::existing_source_in(dir, a)?, &ap)?;
            }
        }
    }
    for (pk, arc) in bundles {
        let fp: Option<String> = sqlx::query_scalar("SELECT file_path FROM documents_sharelinkbundle WHERE id = $1")
            .bind(pk as i32)
            .fetch_optional(&mut *conn)
            .await?
            .flatten();
        // ShareLinkBundle.absolute_file_path: not absolute (on Windows also `\x`/`C:x`), after
        // resolve() inside SHARE_LINK_BUNDLE_DIR (review M5)
        let target = fp.and_then(|f| bundle_target(&c.cfg.share_link_bundle_dir, &f));
        let Some(target) = target else {
            return Err(CliError::Command(format!("Share link bundle {pk} does not have a valid file path.")));
        };
        let _lock = MediaLock::acquire(&c.cfg.media_lock).await?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        copy_file_with_basic_stats(&safepath::existing_source_in(dir, &arc)?, &target)?;
    }
    Ok(())
}

/// `ShareLinkBundle.absolute_file_path` (models.py:1225-1237).
pub(crate) fn bundle_target(bundle_dir: &Path, file_path: &str) -> Option<PathBuf> {
    let rel = Path::new(file_path);
    if file_path.is_empty() || file_path.contains('\0') || rel.is_absolute() || rel.has_root() {
        return None;
    }
    if rel.components().any(|c| matches!(c, std::path::Component::Prefix(_) | std::path::Component::RootDir)) {
        return None;
    }
    let target = bundle_dir.join(rel);
    pl_templating::validate_path_in_root(&target, bundle_dir).ok()?;
    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_iter_streams_records() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.json");
        std::fs::write(&p, "[{\n  \"a\": \"x]}\\\"\",\n  \"b\": [1, 2.0]\n},\n{\"c\": {}}\n]").unwrap();
        let v: Vec<Value> = ManifestIter::open(&p).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["a"], "x]}\"");
        assert!(v[0]["b"][1].is_f64());
        std::fs::write(&p, "[]").unwrap();
        assert_eq!(ManifestIter::open(&p).unwrap().count(), 0);
        std::fs::write(&p, "[{\"a\": 1}").unwrap();
        assert!(ManifestIter::open(&p).unwrap().any(|r| r.is_err()));
    }

    /// Export directory with one document record (fields/export names freely selectable).
    fn prepared_export(fields: Value, extra: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("export");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("0000001.pdf"), b"%PDF-1.4").unwrap();
        std::fs::write(d.path().join("secret.txt"), b"host secret").unwrap();
        let mut rec = serde_json::json!({"model": "documents.document", "pk": 1, "fields": fields, EXPORTER_FILE_NAME: "0000001.pdf"});
        for (k, v) in extra {
            rec[*k] = Value::String((*v).to_string());
        }
        std::fs::write(src.join("manifest.json"), serde_json::to_vec(&Value::Array(vec![rec])).unwrap()).unwrap();
        (d, src)
    }

    fn validity(src: &Path, data_only: bool) -> Result<(), String> {
        check_manifest_validity(src, &manifest_paths(src), data_only).map_err(|e| e.to_string())
    }

    /// H2: `filename`/`archive_filename` from the manifest must not leave ORIGINALS_DIR/ARCHIVE_DIR.
    #[test]
    fn h2_unsafe_stored_filenames_rejected() {
        let (_d, ok) = prepared_export(serde_json::json!({"filename": "2024/Firma/Rechnung.pdf", "archive_filename": "2024/Firma/Rechnung.pdf"}), &[]);
        validity(&ok, false).unwrap();
        // upstream on Windows stores `\` as the separator – valid
        let (_dw, okw) = prepared_export(serde_json::json!({"filename": "ACME Corp\\2019\\Letter (edited).pdf"}), &[]);
        validity(&okw, false).unwrap();
        for (field, bad) in [
            ("archive_filename", "/home/paperless/.ssh/authorized_keys"),
            ("filename", "../../escape.pdf"),
            ("filename", "a\\..\\..\\escape.pdf"),
            ("filename", "\\escape.pdf"),
            ("archive_filename", "C:\\Windows\\escape.pdf"),
            ("filename", "C:escape.pdf"),
        ] {
            let (_d, src) = prepared_export(Value::Object(Map::from_iter([(field.to_string(), Value::from(bad))])), &[]);
            let e = validity(&src, false).unwrap_err();
            assert!(e.contains(&format!("unsafe {field}")), "{bad}: {e}");
            // also with --data-only (no file copying, but the value ends up in the DB)
            assert!(validity(&src, true).is_err(), "{bad}");
        }
        // second line of defense when copying
        let root = _d.path().join("originals");
        std::fs::create_dir_all(&root).unwrap();
        assert!(safepath::target_in(&root, "/etc/passwd", "original").is_err());
    }

    /// M4: `__exported_*` names may only point to files in the import directory.
    #[test]
    fn m4_source_paths_outside_import_dir_rejected() {
        let (_d, src) = prepared_export(serde_json::json!({}), &[]);
        validity(&src, false).unwrap();
        // None = absolute path to the host file
        for (key, rel) in [
            (EXPORTER_FILE_NAME, Some("../secret.txt")),
            (EXPORTER_FILE_NAME, None),
            (EXPORTER_ARCHIVE_NAME, Some("../secret.txt")),
            (EXPORTER_ARCHIVE_NAME, None),
            (EXPORTER_THUMBNAIL_NAME, Some("../secret.txt")),
            (EXPORTER_THUMBNAIL_NAME, Some("..\\secret.txt")),
        ] {
            let (d2, src2) = prepared_export(serde_json::json!({}), &[]);
            let bad = rel.map(str::to_string).unwrap_or_else(|| d2.path().join("secret.txt").to_string_lossy().into_owned());
            let (_d3, src3) = prepared_export(serde_json::json!({}), &[(key, bad.as_str())]);
            std::fs::write(_d3.path().join("secret.txt"), b"host secret").unwrap();
            let e = validity(&src3, false).unwrap_err();
            assert!(e.contains("outside of the source directory"), "{key}={bad}: {e}");
            drop(src2);
        }
        // share link bundles likewise
        let m = serde_json::json!([{"model": "documents.sharelinkbundle", "pk": 1, "fields": {"file_path": "b.zip"}, EXPORTER_SHARE_LINK_BUNDLE_NAME: "../secret.txt"}]);
        std::fs::write(src.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(validity(&src, false).unwrap_err().contains("outside of the source directory"));
    }

    /// M5: target of the share link bundles like `absolute_file_path` (relative_to after resolve), also on Windows.
    #[test]
    fn m5_share_link_bundle_target_contained() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("share_link_bundles");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(bundle_target(&root, "ab/c.zip"), Some(root.join("ab/c.zip")));
        // same as upstream: lexical `..` that stays inside is allowed
        assert!(bundle_target(&root, "ab/../c.zip").is_some());
        for bad in ["", "/etc/x.zip", "\\x.zip", "C:\\x.zip", "C:x.zip", "../x.zip", "a/../../x.zip", "..\\x.zip"] {
            if !cfg!(windows) && (bad.contains('\\') || bad.starts_with("C:")) {
                // on Linux `\` and `C:` are ordinary characters of a file name (stays inside)
                continue;
            }
            assert_eq!(bundle_target(&root, bad), None, "{bad}");
        }
    }

    /// C-N1: PBKDF2 iterations and key length from metadata.json are bounded.
    #[test]
    fn crypto_params_bounded() {
        assert_eq!(crypto_params(&serde_json::json!({})).unwrap(), (1_000_000, 32));
        assert_eq!(crypto_params(&serde_json::json!({EXPORTER_CRYPTO_KEY_ITERATIONS_NAME: 1_000_000, EXPORTER_CRYPTO_KEY_SIZE_NAME: 32})).unwrap(), (1_000_000, 32));
        for bad in [serde_json::json!(0), serde_json::json!(4_000_000_000u64), serde_json::json!(u64::MAX), serde_json::json!("1000"), serde_json::json!(-1)] {
            assert!(crypto_params(&serde_json::json!({EXPORTER_CRYPTO_KEY_ITERATIONS_NAME: bad})).is_err(), "{bad}");
        }
        // previously `vec![0u8; key_size]` from input data
        assert!(crypto_params(&serde_json::json!({EXPORTER_CRYPTO_KEY_SIZE_NAME: 1u64 << 40})).is_err());
    }

    /// C-N2: manifest search without endless recursion (depth limit; symlinks are not followed).
    #[test]
    fn manifest_walk_depth_limited() {
        let d = tempfile::tempdir().unwrap();
        let mut deep = d.path().to_path_buf();
        for i in 0..(MAX_WALK_DEPTH + 5) {
            deep.push(format!("{}", i % 10));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("x-manifest.json"), b"[]").unwrap();
        std::fs::write(d.path().join("0").join("a-manifest.json"), b"[]").unwrap();
        let paths = manifest_paths(d.path());
        assert_eq!(paths.len(), 2, "{paths:?}");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path(), d.path().join("loop")).unwrap();
            assert_eq!(manifest_paths(d.path()).len(), 2);
            assert!(first_file(d.path()).is_some());
        }
    }

    #[test]
    fn to_python_conversions() {
        let d = descriptor::get();
        let doc = &d.models["documents.document"];
        let f = doc.field("archive_serial_number").unwrap();
        assert!(matches!(to_python("documents.document", &Value::from(1), f, &Value::from(5.0)).unwrap(), Param::Text(Some(ref s)) if s == "5"));
        let title = doc.field("title").unwrap();
        assert!(matches!(to_python("documents.document", &Value::from(1), title, &Value::from(1.5)).unwrap(), Param::Text(Some(ref s)) if s == "1.5"));
        let filename = doc.field("filename").unwrap();
        assert!(matches!(default_param(filename, "n", "t"), Param::Text(None)));
        let created = doc.field("created").unwrap();
        assert!(matches!(default_param(created, "n", "t"), Param::Text(Some(ref s)) if s == "t"));
    }
}
