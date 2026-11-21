//! Document serializer, bundled without N+1.
//!
//! upstream: src/documents/serialisers.py@v3.4.2 (DocumentSerializer, OwnedObjectSerializer,
//! DynamicFieldsModelSerializer, NotesSerializer, CustomFieldInstanceSerializer,
//! `_get_viewable_duplicates`), src/documents/models.py (Document.get_public_filename,
//! get_effective_content, CustomFieldInstance.value), src/documents/versioning.py.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use sea_query::{Expr, Order, Query};
use serde_json::{Map, Value, json};

use pl_authz::User;
use pl_authz::models::DOCUMENT;
use pl_authz::owned::{OwnedFields, OwnedInfo};
use pl_core::QueryParams;
use pl_core::datetime::{format_date, format_datetime};

use crate::db::{ApiResult, a, fetch_all, get, i64_col, opt_i64};

/// Action of the ViewSet – controls `duplicate_documents` (`Retrieve` only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SerializeAction {
    #[default]
    List,
    Retrieve,
    Other,
}

/// Options like the kwargs of `DocumentSerializer(...)`.
#[derive(Clone, Debug, Default)]
pub struct SerializeOpts {
    /// `fields` (sparse fieldset); `None` = all.
    pub fields: Option<Vec<String>>,
    pub truncate_content: bool,
    /// `full_perms` (on PUT/PATCH upstream always sets it to `true`).
    pub full_perms: bool,
    pub action: SerializeAction,
    /// `true` if the documents are loaded as in `DocumentViewSet.get_queryset()`
    /// (prefetch of `versions` + content of the latest version). Then the effective content
    /// and `page_count` of the latest version apply. `false` for TrashView, GlobalSearch and
    /// `SearchResultSerializer.fetch_documents`, which build their own querysets (there: own
    /// content, own page count, versions via a separate query).
    pub viewset_queryset: bool,
}

impl SerializeOpts {
    /// Like `DocumentViewSet.get_serializer` from the query parameters.
    pub fn from_query(params: &QueryParams, action: SerializeAction) -> Self {
        let fields = params.get("fields").filter(|f| !f.is_empty()).map(|f| f.split(',').map(str::to_string).collect());
        let truncate_content = matches!(params.get("truncate_content").unwrap_or("False").to_lowercase().as_str(), "true" | "1");
        let full_perms = pl_authz::owned::parse_full_perms(params.get("full_perms"));
        SerializeOpts { fields, truncate_content, full_perms, action, viewset_queryset: true }
    }
}

/// `DocumentSerializer.Meta.fields` without write-only fields, in order.
pub const DOCUMENT_FIELDS: [&str; 26] = [
    "id",
    "correspondent",
    "document_type",
    "storage_path",
    "title",
    "content",
    "tags",
    "created",
    "created_date",
    "modified",
    "added",
    "deleted_at",
    "archive_serial_number",
    "original_file_name",
    "archived_file_name",
    "duplicate_documents",
    "owner",
    "permissions",
    "user_can_change",
    "is_shared_by_requester",
    "notes",
    "custom_fields",
    "page_count",
    "mime_type",
    "root_document",
    "versions",
];

/// Active fields according to `DynamicFieldsModelSerializer` and `OwnedObjectSerializer.__init__`.
pub fn active_fields(opts: &SerializeOpts) -> Vec<&'static str> {
    let mut f: Vec<&'static str> = match &opts.fields {
        Some(list) => DOCUMENT_FIELDS.iter().copied().filter(|n| list.iter().any(|x| x == n)).collect(),
        None => DOCUMENT_FIELDS.to_vec(),
    };
    if opts.full_perms {
        // pop("user_can_change") raises KeyError → is_shared_by_requester remains
        if let Some(pos) = f.iter().position(|n| *n == "user_can_change") {
            f.remove(pos);
            f.retain(|n| *n != "is_shared_by_requester");
        }
    } else {
        f.retain(|n| *n != "permissions");
    }
    f
}

struct DocRow {
    id: i64,
    correspondent: Option<i64>,
    correspondent_name: Option<String>,
    document_type: Option<i64>,
    storage_path: Option<i64>,
    title: String,
    content: Option<String>,
    created: NaiveDate,
    modified: DateTime<Utc>,
    added: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
    asn: Option<i64>,
    original_filename: Option<String>,
    archive_filename: Option<String>,
    owner: Option<i64>,
    mime_type: String,
    root_document: Option<i64>,
    page_count: Option<i64>,
    checksum: String,
    archive_checksum: Option<String>,
    version_index: Option<i64>,
    version_label: Option<String>,
    /// bundled (only if requested, otherwise empty/None): tag ids in `name` order,
    /// notes as `json_agg` (see [`Bundle`]).
    tags: Vec<i64>,
    notes: Option<Value>,
    /// at least one non-deleted version (only with `Bundle::version_flag`)
    has_versions: bool,
}

/// What [`load_docs`] reads in the **same** query (per document a correlated
/// subquery over the index instead of a separate round trip per relation).
#[derive(Clone, Copy, Default)]
struct Bundle {
    content: bool,
    /// `content[:550]` in SQL (`LEFT(content, 550)`, counts characters like Python)
    truncate: bool,
    tags: bool,
    notes: bool,
    version_flag: bool,
}

/// `DocumentSerializer.to_representation`: `content[0:550]` with `truncate_content`.
const TRUNCATE_CHARS: i32 = 550;

/// Tags per document in `Tag.Meta.ordering` (`name`).
const TAGS_SQL: &str = "ARRAY(SELECT bt.tag_id::bigint FROM documents_document_tags bt     JOIN documents_tag btt ON btt.id = bt.tag_id WHERE bt.document_id = d.id ORDER BY btt.name)";
/// Non-deleted notes (`ORDER BY created`) with user.
const NOTES_SQL: &str = "(SELECT json_agg(json_build_array(bn.id, bn.note, (EXTRACT(EPOCH FROM bn.created) * 1000000)::bigint, bu.id,     bu.username, bu.first_name, bu.last_name) ORDER BY bn.created) FROM documents_note bn     LEFT JOIN auth_user bu ON bu.id = bn.user_id WHERE bn.document_id = d.id AND bn.deleted_at IS NULL)";
const HAS_VERSIONS_SQL: &str = "EXISTS(SELECT 1 FROM documents_document bv     WHERE bv.root_document_id = d.id AND bv.deleted_at IS NULL)";

#[derive(Clone)]
struct VersionRow {
    id: i64,
    added: DateTime<Utc>,
    checksum: String,
    version_label: Option<String>,
    version_index: Option<i64>,
    page_count: Option<i64>,
}

/// `sort_versions_newest_first`: key `(version_index or 0, id)` descending.
fn sort_newest_first(v: &mut [VersionRow]) {
    v.sort_by(|x, y| (y.version_index.unwrap_or(0), y.id).cmp(&(x.version_index.unwrap_or(0), x.id)));
}

fn doc_columns(with_content: bool) -> Vec<&'static str> {
    let mut c = vec![
        "id",
        "correspondent_id",
        "document_type_id",
        "storage_path_id",
        "title",
        "created",
        "modified",
        "added",
        "deleted_at",
        "archive_serial_number",
        "original_filename",
        "archive_filename",
        "owner_id",
        "mime_type",
        "root_document_id",
        "page_count",
        "checksum",
        "archive_checksum",
        "version_index",
        "version_label",
    ];
    if with_content {
        c.push("content");
    }
    c
}

async fn load_docs(db: &sqlx::PgPool, ids: &[i64], b: Bundle) -> ApiResult<Vec<DocRow>> {
    let mut q = Query::select();
    for c in doc_columns(b.content && !b.truncate) {
        q.column((a("d"), a(c)));
    }
    if b.content && b.truncate {
        q.expr_as(
            Expr::cust_with_exprs("LEFT($1, $2)", [Expr::col((a("d"), a("content"))).into(), Expr::val(TRUNCATE_CHARS).into()]),
            a("content"),
        );
    }
    if b.tags {
        q.expr_as(Expr::cust(TAGS_SQL), a("b_tags"));
    }
    if b.notes {
        q.expr_as(Expr::cust(NOTES_SQL), a("b_notes"));
    }
    if b.version_flag {
        q.expr_as(Expr::cust(HAS_VERSIONS_SQL), a("b_has_versions"));
    }
    q.expr_as(Expr::col((a("c"), a("name"))), a("correspondent_name"))
        .from_as(a("documents_document"), a("d"))
        .join_as(
            sea_query::JoinType::LeftJoin,
            a("documents_correspondent"),
            a("c"),
            Expr::col((a("c"), a("id"))).equals((a("d"), a("correspondent_id"))),
        )
        .and_where(crate::db::id_in_list(Expr::col((a("d"), a("id"))).into(), ids));
    let rows = fetch_all(db, &q).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(DocRow {
            id: i64_col(&r, "id")?,
            correspondent: opt_i64(&r, "correspondent_id")?,
            correspondent_name: get(&r, "correspondent_name")?,
            document_type: opt_i64(&r, "document_type_id")?,
            storage_path: opt_i64(&r, "storage_path_id")?,
            title: get(&r, "title")?,
            content: if b.content { Some(get(&r, "content")?) } else { None },
            created: get(&r, "created")?,
            modified: get(&r, "modified")?,
            added: get(&r, "added")?,
            deleted_at: get(&r, "deleted_at")?,
            asn: opt_i64(&r, "archive_serial_number")?,
            original_filename: get(&r, "original_filename")?,
            archive_filename: get(&r, "archive_filename")?,
            owner: opt_i64(&r, "owner_id")?,
            mime_type: get(&r, "mime_type")?,
            root_document: opt_i64(&r, "root_document_id")?,
            page_count: opt_i64(&r, "page_count")?,
            checksum: get(&r, "checksum")?,
            archive_checksum: get(&r, "archive_checksum")?,
            version_index: opt_i64(&r, "version_index")?,
            version_label: get(&r, "version_label")?,
            tags: if b.tags { get(&r, "b_tags")? } else { Vec::new() },
            notes: if b.notes { get(&r, "b_notes")? } else { None },
            has_versions: b.version_flag && get::<bool>(&r, "b_has_versions")?,
        });
    }
    Ok(out)
}

/// Non-deleted versions of the roots `ids`, grouped by root.
async fn load_versions(db: &sqlx::PgPool, ids: &[i64]) -> ApiResult<HashMap<i64, Vec<VersionRow>>> {
    let q = Query::select()
        .columns(["id", "added", "checksum", "version_label", "root_document_id", "version_index", "page_count"].map(a))
        .from(a("documents_document"))
        .and_where(crate::db::id_in_list(Expr::col(a("root_document_id")).into(), ids))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .to_owned();
    let mut out: HashMap<i64, Vec<VersionRow>> = HashMap::new();
    for r in fetch_all(db, &q).await? {
        let root = i64_col(&r, "root_document_id")?;
        out.entry(root).or_default().push(VersionRow {
            id: i64_col(&r, "id")?,
            added: get(&r, "added")?,
            checksum: get(&r, "checksum")?,
            version_label: get(&r, "version_label")?,
            version_index: opt_i64(&r, "version_index")?,
            page_count: opt_i64(&r, "page_count")?,
        });
    }
    Ok(out)
}

/// `latest_version_content_prefetch()`: content of the latest non-deleted version per root.
async fn load_latest_content(db: &sqlx::PgPool, ids: &[i64], truncate: bool) -> ApiResult<HashMap<i64, String>> {
    let content: sea_query::SimpleExpr = if truncate {
        Expr::cust_with_exprs("LEFT($1, $2)", [Expr::col(a("content")).into(), Expr::val(TRUNCATE_CHARS).into()])
    } else {
        Expr::col(a("content")).into()
    };
    let q = Query::select()
        .distinct_on([a("root_document_id")])
        .column(a("root_document_id"))
        .expr_as(content, a("content"))
        .from(a("documents_document"))
        .and_where(crate::db::id_in_list(Expr::col(a("root_document_id")).into(), ids))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .order_by(a("root_document_id"), Order::Asc)
        .order_by_with_nulls(a("version_index"), Order::Desc, sea_query::NullOrdering::Last)
        .order_by(a("id"), Order::Desc)
        .to_owned();
    let mut out = HashMap::new();
    for r in fetch_all(db, &q).await? {
        out.insert(i64_col(&r, "root_document_id")?, get::<String>(&r, "content")?);
    }
    Ok(out)
}

/// Notes (non-deleted, `ORDER BY created`) in the format of `NotesSerializer`.
pub(crate) async fn load_notes(db: &sqlx::PgPool, ids: &[i64], tz: Tz) -> ApiResult<HashMap<i64, Vec<Value>>> {
    let q = Query::select()
        .column((a("n"), a("id")))
        .column((a("n"), a("note")))
        .column((a("n"), a("created")))
        .column((a("n"), a("document_id")))
        .expr_as(Expr::col((a("u"), a("id"))), a("u_id"))
        .expr_as(Expr::col((a("u"), a("username"))), a("u_username"))
        .expr_as(Expr::col((a("u"), a("first_name"))), a("u_first_name"))
        .expr_as(Expr::col((a("u"), a("last_name"))), a("u_last_name"))
        .from_as(a("documents_note"), a("n"))
        .join_as(sea_query::JoinType::LeftJoin, a("auth_user"), a("u"), Expr::col((a("u"), a("id"))).equals((a("n"), a("user_id"))))
        .and_where(crate::db::id_in_list(Expr::col((a("n"), a("document_id"))).into(), ids))
        .and_where(Expr::col((a("n"), a("deleted_at"))).is_null())
        .order_by((a("n"), a("created")), Order::Asc)
        .to_owned();
    let mut out: HashMap<i64, Vec<Value>> = HashMap::new();
    for r in fetch_all(db, &q).await? {
        let user = match opt_i64(&r, "u_id")? {
            Some(uid) => json!({
                "id": uid,
                "username": get::<String>(&r, "u_username")?,
                "first_name": get::<String>(&r, "u_first_name")?,
                "last_name": get::<String>(&r, "u_last_name")?,
            }),
            None => Value::Null,
        };
        let created: DateTime<Utc> = get(&r, "created")?;
        let note = json!({
            "id": i64_col(&r, "id")?,
            "note": get::<String>(&r, "note")?,
            "created": format_datetime(&created, &tz),
            "user": user,
        });
        out.entry(i64_col(&r, "document_id")?).or_default().push(note);
    }
    Ok(out)
}

/// `CustomFieldInstance.value` per `data_type`.
///
/// Own query (not bundled): `ORDER BY created` has many ties per document in the data;
/// only the same query shape as Django's prefetch yields the same order.
fn custom_field_value(r: &sqlx::postgres::PgRow, data_type: &str) -> ApiResult<Value> {
    Ok(match data_type {
        "string" => get::<Option<String>>(r, "value_text")?.map(Value::String).unwrap_or(Value::Null),
        "url" => get::<Option<String>>(r, "value_url")?.map(Value::String).unwrap_or(Value::Null),
        "date" => get::<Option<NaiveDate>>(r, "value_date")?.map(|d| Value::String(format_date(&d))).unwrap_or(Value::Null),
        "boolean" => get::<Option<bool>>(r, "value_bool")?.map(Value::Bool).unwrap_or(Value::Null),
        "integer" => opt_i64(r, "value_int")?.map(Value::from).unwrap_or(Value::Null),
        "float" => get::<Option<f64>>(r, "value_float")?
            .map(|f| serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null))
            .unwrap_or(Value::Null),
        "monetary" => get::<Option<String>>(r, "value_monetary")?.map(Value::String).unwrap_or(Value::Null),
        "documentlink" => get::<Option<Value>>(r, "value_document_ids")?.unwrap_or(Value::Null),
        "select" => get::<Option<String>>(r, "value_select")?.map(Value::String).unwrap_or(Value::Null),
        "longtext" => get::<Option<String>>(r, "value_long_text")?.map(Value::String).unwrap_or(Value::Null),
        _ => Value::Null,
    })
}

async fn load_custom_fields(db: &sqlx::PgPool, ids: &[i64]) -> ApiResult<HashMap<i64, Vec<Value>>> {
    let mut q = Query::select();
    for c in [
        "document_id",
        "field_id",
        "value_text",
        "value_bool",
        "value_url",
        "value_date",
        "value_int",
        "value_float",
        "value_monetary",
        "value_document_ids",
        "value_select",
        "value_long_text",
    ] {
        q.column((a("ci"), a(c)));
    }
    q.column((a("f"), a("data_type")))
        .from_as(a("documents_customfieldinstance"), a("ci"))
        .join_as(
            sea_query::JoinType::InnerJoin,
            a("documents_customfield"),
            a("f"),
            Expr::col((a("f"), a("id"))).equals((a("ci"), a("field_id"))),
        )
        .and_where(crate::db::id_in_list(Expr::col((a("ci"), a("document_id"))).into(), ids))
        .and_where(Expr::col((a("ci"), a("deleted_at"))).is_null())
        .order_by((a("ci"), a("created")), Order::Asc);
    let mut out: HashMap<i64, Vec<Value>> = HashMap::new();
    for r in fetch_all(db, &q).await? {
        let dt: String = get(&r, "data_type")?;
        let v = json!({ "value": custom_field_value(&r, &dt)?, "field": i64_col(&r, "field_id")? });
        out.entry(i64_col(&r, "document_id")?).or_default().push(v);
    }
    Ok(out)
}

/// `_get_viewable_duplicates(document, user)`.
async fn load_duplicates(db: &sqlx::PgPool, v: &User, d: &DocRow) -> ApiResult<Vec<Value>> {
    let mut checksums = vec![d.checksum.clone()];
    if let Some(ac) = d.archive_checksum.as_ref().filter(|s| !s.is_empty())
        && !checksums.contains(ac)
    {
        checksums.push(ac.clone());
    }
    let mut q = Query::select();
    q.columns(["id", "title", "deleted_at"].map(a))
        .from(a("documents_document"))
        .cond_where(
            sea_query::Cond::any()
                .add(Expr::col(a("checksum")).is_in(checksums.iter().map(String::as_str)))
                .add(Expr::col(a("archive_checksum")).is_in(checksums.iter().map(String::as_str))),
        )
        .and_where(Expr::col(a("id")).ne(d.id))
        .and_where(Expr::col(a("root_document_id")).is_null())
        .order_by(a("created"), Order::Desc);
    // .filter(id__in=permitted_document_ids(user, include_deleted=True))
    if !pl_authz::objects::user_is_active_superuser(Some(v)) {
        q.and_where(Expr::col(a("id")).in_subquery(pl_authz::objects::permitted_document_ids(Some(v), "view_document", true)));
    }
    let mut out = Vec::new();
    for r in fetch_all(db, &q).await? {
        let deleted: Option<DateTime<Utc>> = get(&r, "deleted_at")?;
        out.push(json!({
            "id": i64_col(&r, "id")?,
            "title": get::<String>(&r, "title")?,
            // Raw value via DRF JSONEncoder → UTC
            "deleted_at": deleted.map(|x| Value::String(format_datetime(&x, &Tz::UTC))).unwrap_or(Value::Null),
        }));
    }
    Ok(out)
}

/// `str(document)`: `created.isoformat()` [+ " " + correspondent] [+ " " + title].
fn document_str(created: &NaiveDate, correspondent: Option<&str>, title: &str) -> String {
    let mut s = format_date(created);
    if let Some(c) = correspondent {
        s.push(' ');
        s.push_str(c);
    }
    if !title.is_empty() {
        s.push(' ');
        s.push_str(title);
    }
    s
}

/// `pathvalidate.sanitize_filename(name, replacement_text="-", max_len=255)` (platform universal).
pub fn sanitize_filename(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if (c as u32) < 0x20 || c as u32 == 0x7f => '-',
            c => c,
        })
        .collect();
    while s.len() > 255 {
        s.pop();
    }
    s
}

/// `Document.get_public_filename(archive=True)` for a root document.
pub fn public_archive_filename(created: &NaiveDate, correspondent: Option<&str>, title: &str) -> String {
    let tail = ".pdf";
    let head_bytes = 255 - tail.len();
    let full = document_str(created, correspondent, title);
    let bytes = full.as_bytes();
    let cut = &bytes[..bytes.len().min(head_bytes)];
    // decode(errors="ignore"): discard an incomplete character at the end
    let head = match std::str::from_utf8(cut) {
        Ok(s) => s.to_string(),
        Err(e) => String::from_utf8_lossy(&cut[..e.valid_up_to()]).into_owned(),
    };
    sanitize_filename(&format!("{head}{tail}"))
}

/// PaperRust (docs/contract/pl-pdftools.md §4.1): checksums of the redacted versions from
/// `paperrust_redaction`. Only called if there are versions; if the table is missing (never
/// redacted), the set is empty.
async fn load_redacted(db: &sqlx::PgPool, checksums: Vec<String>) -> ApiResult<HashSet<String>> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static READY: AtomicBool = AtomicBool::new(false);
    if checksums.is_empty() {
        return Ok(HashSet::new());
    }
    let map = |e: sqlx::Error| pl_core::ApiError::internal(format!("redaction marks: {e}"));
    if !READY.load(Ordering::Relaxed) {
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('public.paperrust_redaction') IS NOT NULL").fetch_one(db).await.map_err(map)?;
        if !exists {
            return Ok(HashSet::new());
        }
        READY.store(true, Ordering::Relaxed);
    }
    let rows: Vec<String> = sqlx::query_scalar("SELECT DISTINCT checksum::text FROM paperrust_redaction WHERE checksum = ANY($1)")
        .bind(checksums)
        .fetch_all(db)
        .await
        .map_err(map)?;
    Ok(rows.into_iter().collect())
}

fn version_info(v: &VersionRow, root_id: i64, redacted: &HashSet<String>) -> Value {
    json!({
        "id": v.id,
        // SerializerMethodField returns datetime raw → DRF JSONEncoder (UTC)
        "added": format_datetime(&v.added, &Tz::UTC),
        "version_label": v.version_label,
        "checksum": v.checksum,
        "is_root": v.id == root_id,
        // PaperRust: version is the result of a redaction (sharing hint)
        "redacted": redacted.contains(&v.checksum),
    })
}

impl DocRow {
    fn as_version(&self) -> VersionRow {
        VersionRow {
            id: self.id,
            added: self.added,
            checksum: self.checksum.clone(),
            version_label: self.version_label.clone(),
            version_index: self.version_index,
            page_count: self.page_count,
        }
    }
}

/// Serializes the documents `ids` in this order (missing ids are omitted).
///
/// `user` is `request.user` (for `user_can_change`, `is_shared_by_requester` and the
/// visible `duplicate_documents`).
pub async fn serialize_documents(state: &pl_core::AppState, user: &User, ids: &[i64], opts: &SerializeOpts) -> ApiResult<Vec<Value>> {
    serialize_documents_db(state.db(), *state.tz(), user, ids, opts).await
}

/// Like [`serialize_documents`] with an explicit pool and `PAPERLESS_TIME_ZONE`.
pub async fn serialize_documents_db(db: &sqlx::PgPool, tz: Tz, user: &User, ids: &[i64], opts: &SerializeOpts) -> ApiResult<Vec<Value>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let fields = active_fields(opts);
    let has = |n: &str| fields.contains(&n);

    let need_versions = has("versions") || has("page_count");
    let want_latest = has("content") && opts.viewset_queryset;
    let bundle = Bundle {
        content: has("content"),
        truncate: opts.truncate_content,
        tags: has("tags"),
        notes: has("notes"),
        // Prefetch case: load versions / latest version only if there are any
        version_flag: opts.viewset_queryset && (need_versions || want_latest),
    };
    let mut docs = load_docs(db, ids, bundle).await?;
    let doc_ids: Vec<i64> = docs.iter().map(|d| d.id).collect();
    // ids requested twice → copy values instead of moving them
    let unique = ids.iter().collect::<HashSet<_>>().len() == ids.len();
    let mut contents: HashMap<i64, String> = HashMap::new();
    let mut tags: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut notes: HashMap<i64, Option<Value>> = HashMap::new();
    for d in docs.iter_mut() {
        if let Some(c) = d.content.take() {
            contents.insert(d.id, c);
        }
        tags.insert(d.id, std::mem::take(&mut d.tags));
        notes.insert(d.id, d.notes.take());
    }

    // Root documents of the versions (for versions/archived_file_name)
    let missing_roots: Vec<i64> =
        docs.iter().filter_map(|d| d.root_document).filter(|r| !doc_ids.contains(r)).collect::<HashSet<_>>().into_iter().collect();
    let extra_roots = if missing_roots.is_empty() || !(has("versions") || has("archived_file_name")) {
        Vec::new()
    } else {
        load_docs(db, &missing_roots, Bundle::default()).await?
    };
    let by_id: HashMap<i64, &DocRow> = docs.iter().chain(extra_roots.iter()).map(|d| (d.id, d)).collect();
    let requested: HashMap<i64, &DocRow> = docs.iter().map(|d| (d.id, d)).collect();

    // versions of the documents themselves (prefetch: only those with versions) or of the roots (without prefetch)
    let version_keys: Vec<i64> = if opts.viewset_queryset {
        docs.iter().filter(|d| d.has_versions).map(|d| d.id).collect()
    } else {
        docs.iter().map(|d| d.root_document.unwrap_or(d.id)).collect()
    };
    let versions = if need_versions && !version_keys.is_empty() { load_versions(db, &version_keys).await? } else { HashMap::new() };
    let redacted = if has("versions") && !versions.is_empty() {
        load_redacted(db, versions.values().flatten().map(|v| v.checksum.clone()).collect()).await?
    } else {
        HashSet::new()
    };
    let mut latest_content = if want_latest && !version_keys.is_empty() {
        load_latest_content(db, &version_keys, opts.truncate_content).await?
    } else {
        HashMap::new()
    };
    let mut cfs = if has("custom_fields") { load_custom_fields(db, &doc_ids).await? } else { HashMap::new() };
    // BulkPermissionMixin / get_shared_object_pks: one batch for all documents.
    // The field selection follows `active_fields` (quirk with full_perms + fields).
    let owned_fields = OwnedFields {
        permissions: has("permissions"),
        user_can_change: has("user_can_change"),
        is_shared_by_requester: has("is_shared_by_requester"),
    };
    let owned = OwnedInfo::load(db, &DOCUMENT, &doc_ids, owned_fields, true).await?;

    let mut out = Vec::with_capacity(docs.len());
    for id in ids {
        let Some(d) = requested.get(id).copied() else {
            continue;
        };
        let mut m = Map::new();
        for f in &fields {
            let v = match *f {
                "id" => Value::from(d.id),
                "correspondent" => json!(d.correspondent),
                "document_type" => json!(d.document_type),
                "storage_path" => json!(d.storage_path),
                "title" => Value::String(d.title.clone()),
                "content" => {
                    // `truncate_content` already in SQL (LEFT(content, 550))
                    let latest = if opts.viewset_queryset && d.root_document.is_none() {
                        take_or_clone(&mut latest_content, d.id, unique)
                    } else {
                        None
                    };
                    Value::String(match latest {
                        Some(c) => c,
                        None => take_or_clone(&mut contents, d.id, unique).unwrap_or_default(),
                    })
                }
                "tags" => Value::Array(take_or_clone(&mut tags, d.id, unique).unwrap_or_default().into_iter().map(Value::from).collect()),
                "created" | "created_date" => Value::String(format_date(&d.created)),
                "modified" => Value::String(format_datetime(&d.modified, &tz)),
                "added" => Value::String(format_datetime(&d.added, &tz)),
                "deleted_at" => d.deleted_at.map(|x| Value::String(format_datetime(&x, &tz))).unwrap_or(Value::Null),
                "archive_serial_number" => json!(d.asn),
                "original_file_name" => json!(d.original_filename),
                "archived_file_name" => match &d.archive_filename {
                    Some(_) => {
                        let ctx = d.root_document.and_then(|r| by_id.get(&r).copied()).unwrap_or(d);
                        Value::String(public_archive_filename(&ctx.created, ctx.correspondent_name.as_deref(), &ctx.title))
                    }
                    None => Value::Null,
                },
                "duplicate_documents" => {
                    if opts.action == SerializeAction::Retrieve {
                        Value::Array(load_duplicates(db, user, d).await?)
                    } else {
                        Value::Array(Vec::new())
                    }
                }
                "owner" => json!(d.owner),
                "permissions" => owned.permissions_json(d.id),
                "user_can_change" => json!(owned.user_can_change(Some(user), d.id, d.owner)),
                "is_shared_by_requester" => {
                    json!(owned.is_shared_by_requester(Some(user), d.id, d.owner))
                }
                "notes" => notes_json(take_or_clone(&mut notes, d.id, unique).flatten(), tz)?,
                "custom_fields" => Value::Array(take_or_clone(&mut cfs, d.id, unique).unwrap_or_default()),
                "page_count" => {
                    let pc = if opts.viewset_queryset && d.root_document.is_none() {
                        match versions.get(&d.id) {
                            Some(vs) if !vs.is_empty() => {
                                let mut vs = vs.clone();
                                sort_newest_first(&mut vs);
                                vs[0].page_count
                            }
                            _ => d.page_count,
                        }
                    } else {
                        d.page_count
                    };
                    json!(pc)
                }
                "mime_type" => Value::String(d.mime_type.clone()),
                "root_document" => json!(d.root_document),
                "versions" => {
                    let root_id = d.root_document.unwrap_or(d.id);
                    match by_id.get(&root_id) {
                        None => Value::Array(Vec::new()),
                        Some(root) => {
                            let mut list: Vec<VersionRow> = if opts.viewset_queryset {
                                versions.get(&d.id).cloned().unwrap_or_default()
                            } else {
                                versions.get(&root_id).cloned().unwrap_or_default()
                            };
                            list.push(root.as_version());
                            sort_newest_first(&mut list);
                            Value::Array(list.iter().map(|x| version_info(x, root_id, &redacted)).collect())
                        }
                    }
                }
                _ => continue,
            };
            m.insert((*f).to_string(), v);
        }
        out.push(Value::Object(m));
    }
    Ok(out)
}

/// Take the value for `id` from the map (copy for ids requested twice).
fn take_or_clone<T: Clone>(m: &mut HashMap<i64, T>, id: i64, unique: bool) -> Option<T> {
    if unique { m.remove(&id) } else { m.get(&id).cloned() }
}

fn bundle_err(what: &str) -> pl_core::ApiError {
    pl_core::ApiError::internal(format!("bundle: {what}"))
}

fn json_i64(v: &Value) -> ApiResult<i64> {
    v.as_i64().ok_or_else(|| bundle_err("expected integer"))
}

fn json_str(v: &mut Value) -> Value {
    match v {
        Value::String(s) => Value::String(std::mem::take(s)),
        _ => Value::Null,
    }
}

/// `json_agg` of the notes (`NOTES_SQL`) → `NotesSerializer` list like [`load_notes`].
fn notes_json(agg: Option<Value>, tz: Tz) -> ApiResult<Value> {
    let Some(Value::Array(rows)) = agg else {
        return Ok(Value::Array(Vec::new()));
    };
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let Value::Array(mut c) = r else {
            return Err(bundle_err("note row"));
        };
        if c.len() != 7 {
            return Err(bundle_err("note row"));
        }
        let created = DateTime::from_timestamp_micros(json_i64(&c[2])?).ok_or_else(|| bundle_err("note created"))?;
        let user = if c[3].is_null() {
            Value::Null
        } else {
            json!({
                "id": json_i64(&c[3])?,
                "username": json_str(&mut c[4]),
                "first_name": json_str(&mut c[5]),
                "last_name": json_str(&mut c[6]),
            })
        };
        out.push(json!({
            "id": json_i64(&c[0])?,
            "note": json_str(&mut c[1]),
            "created": format_datetime(&created, &tz),
            "user": user,
        }));
    }
    Ok(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(fields: Option<&str>, full: bool) -> SerializeOpts {
        SerializeOpts { fields: fields.map(|f| f.split(',').map(str::to_string).collect()), full_perms: full, ..Default::default() }
    }

    #[test]
    fn fields_default_order() {
        let f = active_fields(&opts(None, false));
        assert_eq!(f[0], "id");
        assert!(!f.contains(&"permissions"));
        assert!(f.contains(&"user_can_change"));
        let f = active_fields(&opts(None, true));
        assert!(f.contains(&"permissions"));
        assert!(!f.contains(&"user_can_change"));
        assert!(!f.contains(&"is_shared_by_requester"));
    }

    #[test]
    fn fields_quirk_keyerror() {
        let f = active_fields(&opts(Some("id,is_shared_by_requester"), true));
        assert_eq!(f, vec!["id", "is_shared_by_requester"]);
        let f = active_fields(&opts(Some("dgfhs"), false));
        assert!(f.is_empty());
        let f = active_fields(&opts(Some("id,conteasdnt"), false));
        assert_eq!(f, vec!["id"]);
    }

    #[test]
    fn archive_name() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        assert_eq!(public_archive_filename(&d, Some("ACME"), "a/b"), "2024-01-15 ACME a-b.pdf");
        assert_eq!(public_archive_filename(&d, None, ""), "2024-01-15.pdf");
        let long = "ä".repeat(300);
        let n = public_archive_filename(&d, None, &long);
        assert!(n.len() <= 255 && n.ends_with(".pdf"));
    }

    #[test]
    fn versions_sorting() {
        let t = Utc::now();
        let mk = |id, vi| VersionRow { id, added: t, checksum: String::new(), version_label: None, version_index: vi, page_count: None };
        let mut v = vec![mk(1, None), mk(5, Some(1)), mk(3, Some(2))];
        sort_newest_first(&mut v);
        assert_eq!(v.iter().map(|x| x.id).collect::<Vec<_>>(), vec![3, 5, 1]);
    }
}
