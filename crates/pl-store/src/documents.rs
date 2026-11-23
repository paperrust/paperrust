//! Documents: query (`DocQuery` → transitionally `pl_docfilter`), single retrieval, batch, notes,
//! building the v2 document.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, NaiveDate, Utc};
use pl_authz::User;
use pl_authz::models::DOCUMENT;
use pl_authz::objects::{FilterOpts, permitted_objects_filter};
use pl_core::sqlbind::SqlxBinder as _;
use pl_docfilter::{DOC, FilterContext};
use pl_domain::cursor::KeysetCursor;
use pl_domain::model::*;
use pl_domain::query::{AsnCond, Cond, DateRange, DocQuery, RefSet, TextField, parse_sort};
use pl_domain::{DomainError, DomainResult, Kind, Principal};
use sea_query::{Expr, Order, PostgresQueryBuilder, Query, SelectStatement};
use serde_json::{Map, Value, json};
use sqlx::Row as _;
use uuid::Uuid;

use crate::ident::{self, Ident};
use crate::objects::{DEFAULT_LIMIT, MAX_LIMIT};
use crate::{Store, a, auth, db_err, perm};

const VIEW: &str = "documents.view_document";

/// Base set like the v1 list: root documents outside the trash, visible to `user`.
pub(crate) fn base_query() -> SelectStatement {
    let mut q = Query::select();
    q.column((a(DOC), a("id")))
        .from(a(DOC))
        .and_where(Expr::col((a(DOC), a("root_document_id"))).is_null())
        .and_where(Expr::col((a(DOC), a("deleted_at"))).is_null());
    q
}

pub(crate) fn restrict(q: &mut SelectStatement, user: &User) {
    q.cond_where(permitted_objects_filter(Some(user), &DOCUMENT, DOC, &FilterOpts::default()));
}

pub(crate) fn filter_err(e: pl_docfilter::FilterError) -> DomainError {
    match e {
        pl_docfilter::FilterError::Validation(v) => {
            let fields = match v {
                Value::Object(m) => m,
                other => {
                    let mut m = Map::new();
                    m.insert("filter".into(), other);
                    m
                }
            };
            DomainError::Invalid { message: "Invalid filter.".into(), fields }
        }
        other => DomainError::Internal(format!("{other:?}")),
    }
}

/// Filtered, visible root documents (without sorting) from v1 parameters (L1: AST + one query).
pub(crate) async fn filtered_query(store: &Store, user: &User, params: &[(String, String)]) -> DomainResult<SelectStatement> {
    let qp = pl_core::QueryParams::from_pairs(params.iter().cloned());
    let fctx = FilterContext::new(store.cfg.tz);
    let p = pl_docfilter::ast::parse_filters(&qp, &fctx, &store.db, false).await.map_err(filter_err)?;
    Ok(build_unordered(store, user, &p).await)
}

async fn build_unordered(store: &Store, user: &User, p: &pl_domain::filter::Pred) -> SelectStatement {
    let fold = crate::docsql::fold_for(&store.db, p).await;
    let trgm = crate::docsql::trigram_for(&store.db, p).await;
    let vis = crate::docsql::Visibility::for_user(user);
    crate::docsql::Builder::new(&vis, fold.as_deref()).with_trigram(trgm).select_unordered(p)
}

/// v2 condition tree → AST (L1): leaves via the v1 parameters (same check as v1),
/// `all` → AND, `any` → OR, `not` → set complement (also documents for which the condition
/// is undetermined for lack of a value, e.g. `not asn.gt` matches documents without an ASN).
fn cond_pred<'a>(
    store: &'a Store,
    user: &'a User,
    c: &'a Cond,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = DomainResult<pl_domain::filter::Pred>> + Send + 'a>> {
    use pl_domain::filter::Pred;
    Box::pin(async move {
        Ok(match c {
            Cond::All(v) => {
                let mut acc = Pred::True;
                for x in v {
                    acc = acc.and(cond_pred(store, user, x).await?);
                }
                acc
            }
            Cond::Any(v) => {
                let mut acc = Pred::False;
                for x in v {
                    acc = acc.or(cond_pred(store, user, x).await?);
                }
                acc
            }
            Cond::Not(x) => cond_pred(store, user, x).await?.complement(),
            leaf => {
                let params = to_params(store, user, &[leaf]).await?;
                if params.is_empty() {
                    return Ok(Pred::True);
                }
                let qp = pl_core::QueryParams::from_pairs(params);
                let fctx = FilterContext::new(store.cfg.tz);
                pl_docfilter::ast::parse_filters(&qp, &fctx, &store.db, false).await.map_err(filter_err)?
            }
        })
    })
}

// ------------------------------------------------------------------------- DocQuery → v1

/// Translates the leaves of the AST into v1 parameters (ANDed). Unknown UUIDs become
/// ID 0 (matches nothing) so that `any`/`all` do not end up empty.
pub(crate) async fn to_params(store: &Store, user: &User, leaves: &[&Cond]) -> DomainResult<Vec<(String, String)>> {
    let mut params: Vec<(String, String)> = Vec::new();
    let mut put = |k: &str, v: String, params: &mut Vec<(String, String)>| -> DomainResult<()> {
        if let Some(e) = params.iter_mut().find(|(key, _)| key == k) {
            // same list ANDed twice: for all/none equivalent to the union
            if k.ends_with("__all") || k.ends_with("__none") {
                e.1.push(',');
                e.1.push_str(&v);
                return Ok(());
            }
            return Err(DomainError::NotImplemented(format!("condition \"{k}\" given twice; combine the values into one condition")));
        }
        params.push((k.to_string(), v));
        Ok(())
    };
    for leaf in leaves {
        match leaf {
            Cond::Text(t) => {
                let key = match t.field {
                    TextField::TitleContent => "title_content",
                    TextField::Title => "title__icontains",
                    TextField::Content => "content__icontains",
                    TextField::OriginalFilename => "original_filename__icontains",
                };
                put(key, t.query.clone(), &mut params)?;
            }
            Cond::Tags(r) => refs(store, Kind::Tag, "tags__id", r, true, &mut params, &mut put).await?,
            Cond::Correspondent(r) => refs(store, Kind::Correspondent, "correspondent__id", r, false, &mut params, &mut put).await?,
            Cond::DocumentType(r) => refs(store, Kind::DocumentType, "document_type__id", r, false, &mut params, &mut put).await?,
            Cond::StoragePath(r) => refs(store, Kind::StoragePath, "storage_path__id", r, false, &mut params, &mut put).await?,
            Cond::Owner(r) => refs(store, Kind::User, "owner__id", r, false, &mut params, &mut put).await?,
            Cond::CustomFields(r) => refs(store, Kind::CustomField, "custom_fields__id", r, true, &mut params, &mut put).await?,
            Cond::CustomFieldQuery(v) => {
                let v = cf_query_ids(store, v).await?;
                put("custom_field_query", v.to_string(), &mut params)?;
            }
            Cond::Created(d) => dates("created__date", d, &mut params, &mut put)?,
            Cond::Added(d) => dates("added__date", d, &mut params, &mut put)?,
            Cond::Modified(d) => dates("modified__date", d, &mut params, &mut put)?,
            Cond::Asn(c) => asn(c, &mut params, &mut put)?,
            Cond::Uuids(u) => {
                let map = ident::resolve(&store.db, Kind::Document, u).await?;
                let mut ids: Vec<String> = map.values().map(|(id, _)| id.to_string()).collect();
                if ids.is_empty() {
                    ids.push("0".into());
                }
                put("id__in", ids.join(","), &mut params)?;
            }
            Cond::MimeType(m) => put("mime_type", m.clone(), &mut params)?,
            Cond::SharedByMe(true) => put("shared_by__id", user.id.to_string(), &mut params)?,
            Cond::SharedByMe(false) => {}
            Cond::Folder(f) => {
                if !f.any.is_empty() {
                    let ids = folder_ids(store, &f.any).await?;
                    put(if f.subfolders { "folder__id__tree" } else { "folder__id__in" }, ids, &mut params)?;
                }
                if let Some(n) = f.is_null {
                    put("folder__isnull", if n { "1" } else { "0" }.into(), &mut params)?;
                }
            }
            Cond::All(_) | Cond::Any(_) | Cond::Not(_) => {
                return Err(DomainError::Internal("non-leaf condition after flattening".into()));
            }
        }
    }
    Ok(params)
}

/// Folder UUIDs → ids (comma-separated, unknown → 0 = matches nothing).
async fn folder_ids(store: &Store, u: &[Uuid]) -> DomainResult<String> {
    let rows: Vec<(Uuid, i64)> =
        sqlx::query_as("SELECT uuid, id FROM paperrust_folder WHERE uuid = ANY($1::uuid[])").bind(u).fetch_all(&store.db).await.map_err(
            |e| match e.as_database_error().and_then(|d| d.code()).as_deref() {
                Some("42P01") => DomainError::NotInstalled("folder view (paperrust_folder, `paperrust manage migrate`)"),
                _ => DomainError::Internal(e.to_string()),
            },
        )?;
    let map: HashMap<Uuid, i64> = rows.into_iter().collect();
    Ok(u.iter().map(|x| map.get(x).map(|i| i.to_string()).unwrap_or_else(|| "0".into())).collect::<Vec<_>>().join(","))
}

type Put<'a> = dyn FnMut(&str, String, &mut Vec<(String, String)>) -> DomainResult<()> + Send + 'a;

async fn uuids_to_ids(store: &Store, kind: Kind, u: &[Uuid]) -> DomainResult<String> {
    let map = ident::resolve(&store.db, kind, u).await?;
    let ids: Vec<String> = u.iter().map(|x| map.get(x).map(|(id, _)| id.to_string()).unwrap_or_else(|| "0".into())).collect();
    Ok(ids.join(","))
}

async fn refs(
    store: &Store,
    kind: Kind,
    prefix: &str,
    r: &RefSet,
    has_all: bool,
    params: &mut Vec<(String, String)>,
    put: &mut Put<'_>,
) -> DomainResult<()> {
    if !r.any.is_empty() {
        put(&format!("{prefix}__in"), uuids_to_ids(store, kind, &r.any).await?, params)?;
    }
    if !r.all.is_empty() {
        if !has_all {
            return Err(DomainError::field("filter", format!("\"all\" is not supported for {}.", kind.as_str())));
        }
        put(&format!("{prefix}__all"), uuids_to_ids(store, kind, &r.all).await?, params)?;
    }
    if !r.none.is_empty() {
        put(&format!("{prefix}__none"), uuids_to_ids(store, kind, &r.none).await?, params)?;
    }
    if let Some(n) = r.is_null {
        let flag = if n { "1" } else { "0" };
        match kind {
            Kind::Tag => put("is_tagged", if n { "0" } else { "1" }.into(), params)?,
            Kind::CustomField => put("has_custom_fields", if n { "0" } else { "1" }.into(), params)?,
            _ => put(&format!("{}__isnull", prefix.trim_end_matches("__id")), flag.into(), params)?,
        }
    }
    Ok(())
}

fn dates(prefix: &str, d: &DateRange, params: &mut Vec<(String, String)>, put: &mut Put<'_>) -> DomainResult<()> {
    if let Some(f) = d.from {
        put(&format!("{prefix}__gte"), f.to_string(), params)?;
    }
    if let Some(t) = d.to {
        put(&format!("{prefix}__lte"), t.to_string(), params)?;
    }
    Ok(())
}

fn asn(c: &AsnCond, params: &mut Vec<(String, String)>, put: &mut Put<'_>) -> DomainResult<()> {
    if let Some(v) = c.eq {
        put("archive_serial_number", v.to_string(), params)?;
    }
    if let Some(v) = c.gt {
        put("archive_serial_number__gt", v.to_string(), params)?;
    }
    if let Some(v) = c.lt {
        put("archive_serial_number__lt", v.to_string(), params)?;
    }
    if let Some(n) = c.is_null {
        put("archive_serial_number__isnull", if n { "1" } else { "0" }.into(), params)?;
    }
    Ok(())
}

/// Custom field query: field UUIDs → IDs (names and numbers stay).
async fn cf_query_ids(store: &Store, v: &Value) -> DomainResult<Value> {
    let mut uuids = Vec::new();
    collect_uuids(v, &mut uuids);
    let map = ident::resolve(&store.db, Kind::CustomField, &uuids).await?;
    Ok(replace_uuids(v, &map))
}

fn collect_uuids(v: &Value, out: &mut Vec<Uuid>) {
    match v {
        Value::String(s) => {
            if let Ok(u) = Uuid::parse_str(s) {
                out.push(u);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_uuids(x, out)),
        _ => {}
    }
}

fn replace_uuids(v: &Value, map: &HashMap<Uuid, (i64, i64)>) -> Value {
    match v {
        Value::String(s) => match Uuid::parse_str(s).ok().and_then(|u| map.get(&u)) {
            Some((id, _)) => json!(id),
            None => v.clone(),
        },
        Value::Array(a) => Value::Array(a.iter().map(|x| replace_uuids(x, map)).collect()),
        other => other.clone(),
    }
}

// ------------------------------------------------------------------------- Query

fn sort_col(field: &str) -> (&'static str, &'static str) {
    match field {
        "created" => ("created", "date"),
        "added" => ("added", "timestamptz"),
        "modified" => ("modified", "timestamptz"),
        "title" => ("title", "text"),
        _ => ("id", "int8"),
    }
}

pub(crate) async fn query(store: &Store, p: &Principal, dq: &DocQuery) -> DomainResult<Page<Document>> {
    let user = auth::user(p)?;
    perm::require(user, VIEW)?;
    let include = DocInclude::parse(dq.include.iter().map(String::as_str))?;
    let (field, desc) = parse_sort(dq.sort.as_deref())?;
    let sort_key = if desc { format!("-{field}") } else { field.to_string() };
    let limit = match dq.limit {
        None => DEFAULT_LIMIT,
        Some(l) if (1..=MAX_LIMIT).contains(&l) => l,
        Some(_) => return Err(DomainError::field("limit", format!("Must be between 1 and {MAX_LIMIT}."))),
    };
    // L1: AST path (one query, `any`/`not` over arbitrary conditions)
    let p = match &dq.filter {
        Some(c) => cond_pred(store, user, c).await?,
        None => pl_domain::filter::Pred::True,
    };
    let mut q = build_unordered(store, user, &p).await;

    let count = if dq.count {
        let mut outer = Query::select();
        outer.expr(Expr::cust("count(*)::int8")).from_subquery(q.clone(), a("sub"));
        let (sql, values) = outer.build_sqlx(PostgresQueryBuilder);
        Some(sqlx::query_scalar_with::<_, i64, _>(&sql, values).fetch_one(&store.db).await.map_err(db_err)?)
    } else {
        None
    };

    let (col, ty) = sort_col(field);
    let doc_col = format!("\"{DOC}\".\"{col}\"");
    let doc_id = format!("\"{DOC}\".\"id\"");
    if let Some(c) = dq.cursor.as_deref() {
        let c = KeysetCursor::decode_for(c, &sort_key)?;
        let op = if desc { "<" } else { ">" };
        if col == "id" {
            q.and_where(Expr::cust_with_values(format!("{doc_id} {op} $1"), [c.id]));
        } else {
            let v = c.v.as_str().ok_or_else(|| DomainError::field("cursor", "Invalid cursor."))?.to_string();
            q.and_where(Expr::cust_with_values(
                format!("({doc_col}, {doc_id}) {op} (CAST($1 AS {ty}), $2)"),
                [sea_query::Value::from(v), sea_query::Value::from(c.id)],
            ));
        }
    }
    let ord = if desc { Order::Desc } else { Order::Asc };
    if col != "id" {
        q.expr_as(Expr::cust(format!("{doc_col}::text")), a("sort_value"));
        q.order_by_expr(Expr::cust(doc_col.clone()), ord.clone());
    }
    q.order_by_expr(Expr::cust(doc_id), ord);
    q.limit((limit + 1) as u64);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let mut rows = sqlx::query_with(&sql, values).fetch_all(&store.db).await.map_err(db_err)?;
    let more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let ids: Vec<i64> = rows.iter().map(|r| r.try_get::<i32, _>("id").map(i64::from)).collect::<Result<_, _>>().map_err(db_err)?;
    let next_cursor = if more {
        rows.last().map(|r| {
            let id = r.try_get::<i32, _>("id").map(i64::from).unwrap_or_default();
            let v = if col == "id" {
                Value::Null
            } else {
                // timestamp as text in full precision (CAST back is lossless)
                Value::from(r.try_get::<String, _>("sort_value").unwrap_or_default())
            };
            KeysetCursor { s: sort_key.clone(), v, id }.encode()
        })
    } else {
        None
    };
    let items = load(store, user, &ids, include).await?;
    Ok(Page { items, next_cursor, count })
}

/// Visible root documents among `ids` (order preserved).
async fn visible_ids(store: &Store, user: &User, ids: &[i64]) -> DomainResult<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = base_query();
    q.and_where(Expr::col((a(DOC), a("id"))).is_in(ids.iter().copied()));
    restrict(&mut q, user);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let found: HashSet<i64> =
        sqlx::query_scalar_with::<_, i32, _>(&sql, values).fetch_all(&store.db).await.map_err(db_err)?.into_iter().map(i64::from).collect();
    Ok(ids.iter().copied().filter(|i| found.contains(i)).collect())
}

/// Visible document or 404 (403 without the model permission).
pub(crate) async fn visible_one(store: &Store, user: &User, uuid: Uuid) -> DomainResult<(i64, i64)> {
    perm::require(user, VIEW)?;
    let (id, rev) = ident::resolve_one(&store.db, Kind::Document, uuid).await?;
    if visible_ids(store, user, &[id]).await?.is_empty() {
        return Err(DomainError::NotFound);
    }
    Ok((id, rev))
}

pub(crate) async fn get(store: &Store, p: &Principal, uuid: Uuid, include: DocInclude) -> DomainResult<Document> {
    let user = auth::user(p)?;
    let (id, _) = visible_one(store, user, uuid).await?;
    load(store, user, &[id], include).await?.pop().ok_or(DomainError::NotFound)
}

pub(crate) async fn get_by_id(store: &Store, user: &User, id: i64) -> DomainResult<Document> {
    let ids = visible_ids(store, user, &[id]).await?;
    load(store, user, &ids, DocInclude::default()).await?.pop().ok_or(DomainError::NotFound)
}

pub(crate) async fn batch(store: &Store, p: &Principal, uuids: &[Uuid], include: DocInclude) -> DomainResult<(Vec<Object>, Vec<Uuid>)> {
    let user = auth::user(p)?;
    perm::require(user, VIEW)?;
    let map = ident::resolve(&store.db, Kind::Document, uuids).await?;
    let ids: Vec<i64> = uuids.iter().filter_map(|u| map.get(u).map(|(id, _)| *id)).collect();
    let vis = visible_ids(store, user, &ids).await?;
    let docs = load(store, user, &vis, include).await?;
    let by_uuid: HashMap<Uuid, Document> = docs.into_iter().map(|d| (d.meta.uuid, d)).collect();
    let mut items = Vec::new();
    let mut missing = Vec::new();
    let mut seen = HashSet::new();
    for u in uuids {
        if !seen.insert(*u) {
            continue;
        }
        match by_uuid.get(u) {
            Some(d) => items.push(Object::Document(Box::new(d.clone()))),
            None => missing.push(*u),
        }
    }
    Ok((items, missing))
}

pub(crate) async fn by_legacy_ids(store: &Store, p: &Principal, ids: &[i64], embed: bool) -> DomainResult<Vec<(i64, Option<Object>)>> {
    let user = auth::user(p)?;
    if !(user.is_active && user.has_perm(VIEW)) {
        return Ok(ids.iter().map(|i| (*i, None)).collect());
    }
    let vis = visible_ids(store, user, ids).await?;
    let mut docs: HashMap<i64, Document> = if embed {
        load(store, user, &vis, DocInclude::default()).await?.into_iter().map(|d| (d.meta.legacy_id, d)).collect()
    } else {
        HashMap::new()
    };
    let vis: HashSet<i64> = vis.into_iter().collect();
    Ok(ids
        .iter()
        .map(|i| {
            let o = if vis.contains(i) {
                // without `embed` a placeholder object is enough for visibility
                Some(match docs.remove(i) {
                    Some(d) => Object::Document(Box::new(d)),
                    None => Object::Group(Group { meta: Meta { uuid: Uuid::nil(), legacy_id: *i, rev: 0 }, name: String::new() }),
                })
            } else {
                None
            };
            (*i, o)
        })
        .collect())
}

// ------------------------------------------------------------------------- Building

/// Row of the file-relevant columns (newest version, like v1 `download`).
#[derive(Debug, Clone)]
pub(crate) struct FileRow {
    pub id: i64,
    pub mime_type: String,
    pub checksum: String,
    pub archive_checksum: Option<String>,
    pub filename: Option<String>,
    pub archive_filename: Option<String>,
}

/// Newest version per root document (or the document itself).
pub(crate) async fn latest_files(store: &Store, ids: &[i64]) -> DomainResult<HashMap<i64, FileRow>> {
    let rows = sqlx::query(
        "SELECT DISTINCT ON (root) root, id::int8 AS id, mime_type, checksum, archive_checksum, filename, archive_filename
           FROM (SELECT coalesce(root_document_id, id)::int8 AS root, id, mime_type, checksum, archive_checksum,
                        filename, archive_filename, version_index,
                        (root_document_id IS NULL) AS is_root
                   FROM documents_document
                  WHERE (id = ANY($1::int4[]) OR root_document_id = ANY($1::int4[])) AND deleted_at IS NULL) x
          ORDER BY root, is_root, version_index DESC NULLS LAST, id DESC",
    )
    .bind(ids.iter().map(|i| *i as i32).collect::<Vec<_>>())
    .fetch_all(&store.db)
    .await
    .map_err(db_err)?;
    let mut out = HashMap::new();
    for r in rows {
        let root: i64 = r.try_get("root").map_err(db_err)?;
        out.insert(
            root,
            FileRow {
                id: r.try_get("id").map_err(db_err)?,
                mime_type: r.try_get("mime_type").map_err(db_err)?,
                checksum: r.try_get("checksum").map_err(db_err)?,
                archive_checksum: r.try_get("archive_checksum").map_err(db_err)?,
                filename: r.try_get("filename").map_err(db_err)?,
                archive_filename: r.try_get("archive_filename").map_err(db_err)?,
            },
        );
    }
    Ok(out)
}

impl FileRow {
    pub(crate) fn doc_row(&self) -> pl_files::db::DocRow {
        pl_files::db::DocRow {
            id: self.id,
            title: String::new(),
            created: NaiveDate::default(),
            modified: DateTime::<Utc>::default(),
            checksum: self.checksum.clone(),
            archive_checksum: self.archive_checksum.clone(),
            mime_type: self.mime_type.clone(),
            filename: self.filename.clone(),
            archive_filename: self.archive_filename.clone(),
            owner_id: None,
            root_document_id: None,
            correspondent: None,
        }
    }
}

pub(crate) async fn paths(store: &Store, f: &FileRow) -> DomainResult<(Option<PathBuf>, Option<PathBuf>, PathBuf)> {
    let cx = pl_files::docfile::Ctx { db: &store.db, cfg: &store.cfg };
    let row = f.doc_row();
    let original = pl_files::docfile::source_path(cx, &row).await.map_err(db_err)?;
    let archive = pl_files::docfile::archive_path(cx, &row);
    let thumb = pl_files::docfile::thumbnail_path(cx, &row);
    Ok((original, archive, thumb))
}

async fn stat(p: Option<&PathBuf>) -> Option<std::fs::Metadata> {
    match p {
        Some(p) => tokio::fs::metadata(p).await.ok().filter(|m| m.is_file()),
        None => None,
    }
}

/// Weak ETag of the thumbnail.
pub(crate) fn weak_etag(m: &std::fs::Metadata) -> String {
    let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    format!("W/\"{mtime}-{}\"", m.len())
}

async fn files_info(store: &Store, f: Option<&FileRow>) -> DomainResult<Files> {
    let Some(f) = f else {
        return Ok(Files { original: FileInfo { checksum: None, size: None }, archive: None, thumbnail: ThumbInfo { etag: None } });
    };
    let (orig, arch, thumb) = paths(store, f).await?;
    let os = stat(orig.as_ref()).await.map(|m| m.len());
    let archive = match &f.archive_filename {
        Some(_) => Some(FileInfo { checksum: f.archive_checksum.clone(), size: stat(arch.as_ref()).await.map(|m| m.len()) }),
        None => None,
    };
    let te = stat(Some(&thumb)).await.map(|m| weak_etag(&m));
    Ok(Files { original: FileInfo { checksum: Some(f.checksum.clone()), size: os }, archive, thumbnail: ThumbInfo { etag: te } })
}

/// Build documents (already checked as visible) in the order of `ids`.
pub(crate) async fn load(store: &Store, user: &User, ids: &[i64], include: DocInclude) -> DomainResult<Vec<Document>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let db = &store.db;
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let content_col = if include.content { "content" } else { "NULL::text AS content" };
    let rows = sqlx::query(&format!(
        "SELECT id::int8 AS id, title, created, added, modified, correspondent_id::int8 AS correspondent_id,
                document_type_id::int8 AS document_type_id, storage_path_id::int8 AS storage_path_id,
                owner_id::int8 AS owner_id, archive_serial_number::int8 AS asn, page_count::int8 AS page_count,
                mime_type, original_filename, {content_col}
           FROM documents_document WHERE id = ANY($1::int4[])"
    ))
    .bind(&ids32)
    .fetch_all(db)
    .await
    .map_err(db_err)?;

    let tag_rows: Vec<(i32, i32)> =
        sqlx::query_as("SELECT document_id, tag_id FROM documents_document_tags WHERE document_id = ANY($1::int4[]) ORDER BY tag_id")
            .bind(&ids32)
            .fetch_all(db)
            .await
            .map_err(db_err)?;
    let cf_rows = sqlx::query(
        "SELECT i.document_id::int8 AS doc, i.field_id::int8 AS field, f.data_type, i.value_text, i.value_bool, i.value_url,
                i.value_date, i.value_int::int8 AS value_int, i.value_float, i.value_monetary, i.value_document_ids,
                i.value_select, i.value_long_text
           FROM documents_customfieldinstance i JOIN documents_customfield f ON f.id = i.field_id
          WHERE i.document_id = ANY($1::int4[]) AND i.deleted_at IS NULL ORDER BY i.id",
    )
    .bind(&ids32)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    let note_counts: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT document_id, count(*)::int8 FROM documents_note WHERE document_id = ANY($1::int4[]) AND deleted_at IS NULL GROUP BY 1",
    )
    .bind(&ids32)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    let note_counts: HashMap<i64, i64> = note_counts.into_iter().map(|(d, c)| (i64::from(d), c)).collect();
    let files = latest_files(store, ids).await?;

    // identities of all references
    let fk = |col: &str| -> Vec<i64> { rows.iter().filter_map(|r| r.try_get::<Option<i64>, _>(col).ok().flatten()).collect() };
    let corr = ident::idents(db, Kind::Correspondent, &fk("correspondent_id")).await?;
    let dtype = ident::idents(db, Kind::DocumentType, &fk("document_type_id")).await?;
    let spath = ident::idents(db, Kind::StoragePath, &fk("storage_path_id")).await?;
    let owners = fk("owner_id");
    let tags = ident::idents(db, Kind::Tag, &tag_rows.iter().map(|(_, t)| i64::from(*t)).collect::<Vec<_>>()).await?;
    let mut cf_fields = Vec::new();
    let mut linked = Vec::new();
    for r in &cf_rows {
        cf_fields.push(r.try_get::<i64, _>("field").map_err(db_err)?);
        if let Ok(Some(sqlx::types::Json(Value::Array(a)))) = r.try_get::<Option<sqlx::types::Json<Value>>, _>("value_document_ids") {
            linked.extend(a.iter().filter_map(Value::as_i64));
        }
    }
    let cfi = ident::idents(db, Kind::CustomField, &cf_fields).await?;
    let mut doc_idents_for = ids.to_vec();
    doc_idents_for.extend(linked);
    let docs_i = ident::idents(db, Kind::Document, &doc_idents_for).await?;

    let notes = if include.notes && user.has_perm("documents.view_note") { Some(load_notes(store, ids).await?) } else { None };
    let mut user_ids = owners.clone();
    if let Some(n) = &notes {
        user_ids.extend(n.values().flatten().filter_map(|(_, uid)| *uid));
    }
    let users = perm::user_refs(db, &user_ids).await?;
    let grants = perm::grants(db, user, &DOCUMENT, ids, &["change", "delete"]).await?;
    let sharing = if include.sharing {
        let objs: Vec<(i64, Option<i64>)> =
            rows.iter().map(|r| (r.try_get("id").unwrap_or_default(), r.try_get::<Option<i64>, _>("owner_id").ok().flatten())).collect();
        perm::sharing(db, user, &DOCUMENT, &objs).await?
    } else {
        HashMap::new()
    };

    let mut tag_map: HashMap<i64, Vec<Uuid>> = HashMap::new();
    for (d, t) in &tag_rows {
        if let Some(i) = tags.get(&i64::from(*t)) {
            tag_map.entry(i64::from(*d)).or_default().push(i.uuid);
        }
    }
    let mut cf_map: HashMap<i64, Vec<CustomFieldValue>> = HashMap::new();
    for r in &cf_rows {
        let doc: i64 = r.try_get("doc").map_err(db_err)?;
        let field: i64 = r.try_get("field").map_err(db_err)?;
        let Some(fi) = cfi.get(&field) else { continue };
        cf_map.entry(doc).or_default().push(CustomFieldValue { field: fi.uuid, value: cf_value(r, &docs_i)? });
    }

    let mut by_id: HashMap<i64, Document> = HashMap::new();
    for r in &rows {
        let id: i64 = r.try_get("id").map_err(db_err)?;
        let Some(idn) = docs_i.get(&id) else { continue };
        let owner_id: Option<i64> = r.try_get("owner_id").map_err(db_err)?;
        let uref = |m: &HashMap<i64, Ident>, col: &str| -> Option<Uuid> {
            r.try_get::<Option<i64>, _>(col).ok().flatten().and_then(|x| m.get(&x).map(|i| i.uuid))
        };
        let doc = Document {
            meta: Meta { uuid: idn.uuid, legacy_id: id, rev: idn.rev },
            title: r.try_get("title").map_err(db_err)?,
            created: r.try_get("created").map_err(db_err)?,
            added: r.try_get("added").map_err(db_err)?,
            modified: r.try_get("modified").map_err(db_err)?,
            correspondent: uref(&corr, "correspondent_id"),
            document_type: uref(&dtype, "document_type_id"),
            storage_path: uref(&spath, "storage_path_id"),
            tags: tag_map.remove(&id).unwrap_or_default(),
            custom_fields: cf_map.remove(&id).unwrap_or_default(),
            asn: r.try_get("asn").map_err(db_err)?,
            owner: owner_id.and_then(|o| users.get(&o).cloned()),
            page_count: r.try_get("page_count").map_err(db_err)?,
            mime_type: r.try_get("mime_type").map_err(db_err)?,
            original_filename: r.try_get("original_filename").map_err(db_err)?,
            notes_count: note_counts.get(&id).copied().unwrap_or(0),
            files: files_info(store, files.get(&id)).await?,
            access: perm::access(user, &DOCUMENT, id, owner_id, &grants),
            content: if include.content { r.try_get("content").map_err(db_err)? } else { None },
            notes: notes.as_ref().map(|n| {
                n.get(&id)
                    .map(|v| v.iter().map(|(note, uid)| Note { user: uid.and_then(|u| users.get(&u).cloned()), ..note.clone() }).collect())
                    .unwrap_or_default()
            }),
            sharing: sharing.get(&id).cloned(),
        };
        by_id.insert(id, doc);
    }
    Ok(ids.iter().filter_map(|i| by_id.remove(i)).collect())
}

fn cf_value(r: &sqlx::postgres::PgRow, docs: &HashMap<i64, Ident>) -> DomainResult<Value> {
    let dt: String = r.try_get("data_type").map_err(db_err)?;
    let g = |c: &str| -> DomainResult<Value> {
        Ok(match c {
            "value_bool" => json!(r.try_get::<Option<bool>, _>(c).map_err(db_err)?),
            "value_date" => json!(r.try_get::<Option<NaiveDate>, _>(c).map_err(db_err)?.map(|d| d.to_string())),
            "value_int" => json!(r.try_get::<Option<i64>, _>(c).map_err(db_err)?),
            "value_float" => json!(r.try_get::<Option<f64>, _>(c).map_err(db_err)?),
            _ => json!(r.try_get::<Option<String>, _>(c).map_err(db_err)?),
        })
    };
    match dt.as_str() {
        "string" => g("value_text"),
        "url" => g("value_url"),
        "date" => g("value_date"),
        "boolean" => g("value_bool"),
        "integer" => g("value_int"),
        "float" => g("value_float"),
        "monetary" => g("value_monetary"),
        "select" => g("value_select"),
        "longtext" => g("value_long_text"),
        "documentlink" => {
            let v: Option<sqlx::types::Json<Value>> = r.try_get("value_document_ids").map_err(db_err)?;
            Ok(match v.map(|j| j.0) {
                Some(Value::Array(a)) => {
                    Value::Array(a.iter().filter_map(Value::as_i64).filter_map(|i| docs.get(&i)).map(|i| json!(i.uuid)).collect())
                }
                _ => Value::Null,
            })
        }
        _ => Ok(Value::Null),
    }
}

/// Notes per document (newest first, like v1), user ID separate for the resolution.
async fn load_notes(store: &Store, ids: &[i64]) -> DomainResult<HashMap<i64, Vec<(Note, Option<i64>)>>> {
    type Row = (i32, i32, String, DateTime<Utc>, Option<i32>, String);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT document_id, id, note, created, user_id, format FROM documents_note
          WHERE document_id = ANY($1::int4[]) AND deleted_at IS NULL ORDER BY created DESC, id DESC",
    )
    .bind(ids.iter().map(|i| *i as i32).collect::<Vec<_>>())
    .fetch_all(&store.db)
    .await
    .map_err(db_err)?;
    let mut out: HashMap<i64, Vec<(Note, Option<i64>)>> = HashMap::new();
    for (doc, id, note, created, uid, format) in rows {
        out.entry(i64::from(doc))
            .or_default()
            .push((Note { legacy_id: i64::from(id), note, format, created, user: None }, uid.map(i64::from)));
    }
    Ok(out)
}

pub(crate) async fn notes(store: &Store, p: &Principal, doc: Uuid) -> DomainResult<Vec<Note>> {
    let user = auth::user(p)?;
    perm::require(user, "documents.view_note")?;
    let (id, _) = visible_one(store, user, doc).await?;
    notes_by_id(store, id).await
}

pub(crate) async fn notes_by_id(store: &Store, id: i64) -> DomainResult<Vec<Note>> {
    let mut n = load_notes(store, &[id]).await?.remove(&id).unwrap_or_default();
    let uids: Vec<i64> = n.iter().filter_map(|(_, u)| *u).collect();
    let users = perm::user_refs(&store.db, &uids).await?;
    Ok(n.drain(..).map(|(note, uid)| Note { user: uid.and_then(|u| users.get(&u).cloned()), ..note }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_replacement_in_cf_query() {
        let u = Uuid::from_u128(7);
        let v = json!(["AND", [[u.to_string(), "exact", "x"], ["name", "exists", true]]]);
        let mut found = Vec::new();
        collect_uuids(&v, &mut found);
        assert_eq!(found, vec![u]);
        let map = HashMap::from([(u, (12i64, 1i64))]);
        assert_eq!(replace_uuids(&v, &map), json!(["AND", [[12, "exact", "x"], ["name", "exists", true]]]));
    }
}
