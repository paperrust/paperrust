// upstream: src/documents/management/commands/document_index.py@v3.4.2 (reindex queryset),
//           src/documents/search/_backend.py (WriteBatch.add_or_update[_ids],
//           _DocumentViewerStream, _bulk_get_viewer_permissions),
//           src/documents/permissions.py (get_permission_grantees),
//           src/documents/versioning.py (root_document_ids, annotate_effective_content,
//           latest_version), src/documents/models.py (Meta.ordering of Tag/Note/
//           CustomFieldInstance/DocumentBarcode, SoftDeleteModel managers).
//
// READ-ONLY access. Every chunk of up to CHUNK_SIZE root documents costs a fixed
// number of queries (documents, versions, tags, notes, custom fields, barcodes,
// user grants, group grants) – no N+1.
//
// Raw SQL with bound parameters (like pl-search/db.rs): the statements are static
// apart from the generated `$n` placeholder lists for IN (...); no value is ever
// concatenated. All SQL is plain ANSI apart from the `$n` placeholder style.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgPool, Row};

use crate::error::Result;
use crate::record::{
    BarcodeRecord, CustomFieldRaw, CustomFieldRecord, DocumentRecord, NoteRecord, VersionPick, VersionRow, newest_version,
};

/// `_DocumentViewerStream(chunk_size=1000)`.
pub const CHUNK_SIZE: usize = 1000;

/// Which upstream code path the data is loaded for (they resolve versions differently).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadMode {
    /// how `get_effective_content()` resolves the newest version
    pub content: VersionPick,
    /// how `get_effective_barcodes()` (latest_version) resolves it
    pub barcodes: VersionPick,
    /// whether a root that is in the trash is still loaded
    pub include_deleted_roots: bool,
}

impl LoadMode {
    /// `document_index reindex` → `rebuild()`: prefetched `versions` for both.
    pub const REBUILD: LoadMode =
        LoadMode { content: VersionPick::Prefetch, barcodes: VersionPick::Prefetch, include_deleted_roots: false };
    /// `WriteBatch.add_or_update_ids`: `annotate_effective_content` (SQL order) +
    /// prefetched `versions__barcodes` (Python order); `Document.objects` (no trash).
    pub const BATCH_IDS: LoadMode = LoadMode { content: VersionPick::Sql, barcodes: VersionPick::Prefetch, include_deleted_roots: false };
    /// `WriteBatch.add_or_update(document)` on an instance without prefetches: both
    /// resolved by query (SQL order); the root is reached through the FK (base
    /// manager, includes trashed roots).
    pub const SINGLE: LoadMode = LoadMode { content: VersionPick::Sql, barcodes: VersionPick::Sql, include_deleted_roots: true };
}

fn placeholders(n: usize) -> String {
    (1..=n).map(|i| format!("${i}")).collect::<Vec<_>>().join(", ")
}

fn ids_i32(ids: &[i64]) -> Vec<i32> {
    ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect()
}

/// Number of documents a full rebuild would index (same filter as [`rebuild_root_ids`]).
pub async fn count_root_docs(pool: &PgPool) -> Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM documents_document d WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// Root ids of the reindex queryset: `Document.objects.filter(root_document__isnull=True)`
/// in model order (`-created`).
pub async fn rebuild_root_ids(pool: &PgPool) -> Result<Vec<i64>> {
    let rows = sqlx::query(
        "SELECT d.id FROM documents_document d \
         WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL \
         ORDER BY d.created DESC",
    )
    .fetch_all(pool)
    .await?;
    rows.iter().map(|r| Ok(i64::from(r.try_get::<i32, _>(0)?))).collect()
}

/// `root_document_ids(ids)` (over `Document.objects`, i.e. not trashed), sorted.
pub async fn root_document_ids(pool: &PgPool, ids: &[i64]) -> Result<Vec<i64>> {
    let mut out = Vec::new();
    for chunk in ids_i32(ids).chunks(CHUNK_SIZE) {
        let sql = format!(
            "SELECT DISTINCT COALESCE(d.root_document_id, d.id) FROM documents_document d \
             WHERE d.deleted_at IS NULL AND d.id IN ({})",
            placeholders(chunk.len())
        );
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(*id);
        }
        for r in q.fetch_all(pool).await? {
            out.push(i64::from(r.try_get::<i32, _>(0)?));
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// For a single document id: the id of its root (`document.root_document`), any
/// deletion state. None when the document does not exist.
pub async fn root_of(pool: &PgPool, id: i64) -> Result<Option<i64>> {
    let Ok(id32) = i32::try_from(id) else {
        return Ok(None);
    };
    let row = sqlx::query("SELECT COALESCE(d.root_document_id, d.id) FROM documents_document d WHERE d.id = $1")
        .bind(id32)
        .fetch_optional(pool)
        .await?;
    Ok(match row {
        Some(r) => Some(i64::from(r.try_get::<i32, _>(0)?)),
        None => None,
    })
}

struct DocRow {
    id: i64,
    title: String,
    content: String,
    checksum: String,
    original_filename: Option<String>,
    created: NaiveDate,
    modified: DateTime<Utc>,
    added: DateTime<Utc>,
    asn: Option<i64>,
    page_count: Option<i64>,
    owner_id: Option<i64>,
    correspondent: Option<String>,
    document_type: Option<String>,
    storage_path: Option<String>,
}

/// Load the index records of the given root documents (order of `root_ids` kept,
/// missing/filtered ids skipped). Chunked by [`CHUNK_SIZE`].
pub async fn load_records(pool: &PgPool, root_ids: &[i64], mode: LoadMode) -> Result<Vec<DocumentRecord>> {
    let mut out = Vec::with_capacity(root_ids.len());
    for chunk in root_ids.chunks(CHUNK_SIZE) {
        out.extend(load_chunk(pool, chunk, mode).await?);
    }
    Ok(out)
}

async fn load_chunk(pool: &PgPool, root_ids: &[i64], mode: LoadMode) -> Result<Vec<DocumentRecord>> {
    let ids = ids_i32(root_ids);
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ph = placeholders(ids.len());

    // --- documents (select_related correspondent, document_type, storage_path)
    let deleted_filter = if mode.include_deleted_roots { "" } else { "d.deleted_at IS NULL AND " };
    let sql = format!(
        "SELECT d.id, d.title, d.content, d.checksum, d.original_filename, d.created, \
                d.modified, d.added, d.archive_serial_number, d.page_count, d.owner_id, \
                c.name, t.name, s.name \
         FROM documents_document d \
         LEFT JOIN documents_correspondent c ON c.id = d.correspondent_id \
         LEFT JOIN documents_documenttype t ON t.id = d.document_type_id \
         LEFT JOIN documents_storagepath s ON s.id = d.storage_path_id \
         WHERE {deleted_filter}d.root_document_id IS NULL AND d.id IN ({ph})"
    );
    let mut q = sqlx::query(&sql);
    for id in &ids {
        q = q.bind(*id);
    }
    let mut docs: HashMap<i64, DocRow> = HashMap::new();
    for r in q.fetch_all(pool).await? {
        let id = i64::from(r.try_get::<i32, _>(0)?);
        docs.insert(
            id,
            DocRow {
                id,
                title: r.try_get(1)?,
                content: r.try_get(2)?,
                checksum: r.try_get(3)?,
                original_filename: r.try_get(4)?,
                created: r.try_get(5)?,
                modified: r.try_get(6)?,
                added: r.try_get(7)?,
                asn: r.try_get::<Option<i32>, _>(8)?.map(i64::from),
                page_count: r.try_get::<Option<i32>, _>(9)?.map(i64::from),
                owner_id: r.try_get::<Option<i32>, _>(10)?.map(i64::from),
                correspondent: r.try_get(11)?,
                document_type: r.try_get(12)?,
                storage_path: r.try_get(13)?,
            },
        );
    }
    if docs.is_empty() {
        return Ok(Vec::new());
    }

    // --- versions (`versions` related manager = Document.objects: not trashed)
    let sql = format!(
        "SELECT v.id, v.root_document_id, v.version_index, v.content FROM documents_document v \
         WHERE v.deleted_at IS NULL AND v.root_document_id IN ({ph})"
    );
    let mut q = sqlx::query(&sql);
    for id in &ids {
        q = q.bind(*id);
    }
    let mut versions: HashMap<i64, Vec<VersionRow>> = HashMap::new();
    for r in q.fetch_all(pool).await? {
        let root = i64::from(r.try_get::<i32, _>(1)?);
        versions.entry(root).or_default().push(VersionRow {
            id: i64::from(r.try_get::<i32, _>(0)?),
            version_index: r.try_get::<Option<i32>, _>(2)?.map(i64::from),
            content: r.try_get(3)?,
        });
    }

    // --- tags (Tag.Meta.ordering = ("name",))
    let sql = format!(
        "SELECT dt.document_id, t.name FROM documents_document_tags dt \
         JOIN documents_tag t ON t.id = dt.tag_id \
         WHERE dt.document_id IN ({ph}) ORDER BY t.name ASC"
    );
    let mut q = sqlx::query(&sql);
    for id in &ids {
        q = q.bind(*id);
    }
    let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
    for r in q.fetch_all(pool).await? {
        tags.entry(i64::from(r.try_get::<i32, _>(0)?)).or_default().push(r.try_get(1)?);
    }

    // --- notes (Note.objects: not deleted; ordering ("created",); notes__user)
    let sql = format!(
        "SELECT n.document_id, n.note, u.username FROM documents_note n \
         LEFT JOIN auth_user u ON u.id = n.user_id \
         WHERE n.deleted_at IS NULL AND n.document_id IN ({ph}) ORDER BY n.created ASC"
    );
    let mut q = sqlx::query(&sql);
    for id in &ids {
        q = q.bind(*id);
    }
    let mut notes: HashMap<i64, Vec<NoteRecord>> = HashMap::new();
    for r in q.fetch_all(pool).await? {
        notes.entry(i64::from(r.try_get::<i32, _>(0)?)).or_default().push(NoteRecord { note: r.try_get(1)?, user: r.try_get(2)? });
    }

    // --- custom fields (CustomFieldInstance.objects: not deleted; ordering ("created",))
    let sql = format!(
        "SELECT ci.document_id, f.name, f.data_type, f.extra_data, ci.value_text, \
                ci.value_bool, ci.value_url, ci.value_date, ci.value_int, ci.value_float, \
                ci.value_monetary, ci.value_document_ids, ci.value_select, ci.value_long_text \
         FROM documents_customfieldinstance ci \
         JOIN documents_customfield f ON f.id = ci.field_id \
         WHERE ci.deleted_at IS NULL AND ci.document_id IN ({ph}) ORDER BY ci.created ASC"
    );
    let mut q = sqlx::query(&sql);
    for id in &ids {
        q = q.bind(*id);
    }
    let mut cfs: HashMap<i64, Vec<CustomFieldRecord>> = HashMap::new();
    for r in q.fetch_all(pool).await? {
        let raw = CustomFieldRaw {
            name: r.try_get(1)?,
            data_type: r.try_get(2)?,
            extra_data: r.try_get(3)?,
            value_text: r.try_get(4)?,
            value_bool: r.try_get(5)?,
            value_url: r.try_get(6)?,
            value_date: r.try_get(7)?,
            value_int: r.try_get::<Option<i32>, _>(8)?.map(i64::from),
            value_float: r.try_get(9)?,
            value_monetary: r.try_get(10)?,
            value_document_ids: r.try_get(11)?,
            value_select: r.try_get(12)?,
            value_long_text: r.try_get(13)?,
        };
        cfs.entry(i64::from(r.try_get::<i32, _>(0)?))
            .or_default()
            .push(CustomFieldRecord { search_value: raw.value_for_search(), name: raw.name });
    }

    // --- barcodes of the effective document (latest version or the root itself)
    let mut barcode_owner: HashMap<i64, i64> = HashMap::new(); // doc whose barcodes -> root
    for id in docs.keys() {
        let vs = versions.get(id).map(Vec::as_slice).unwrap_or(&[]);
        let owner = newest_version(vs, mode.barcodes).map(|v| v.id).unwrap_or(*id);
        barcode_owner.insert(owner, *id);
    }
    let owner_ids = ids_i32(&barcode_owner.keys().copied().collect::<Vec<_>>());
    let sql = format!(
        "SELECT b.document_id, b.value, b.format FROM documents_documentbarcode b \
         WHERE b.document_id IN ({}) ORDER BY b.page ASC, b.id ASC",
        placeholders(owner_ids.len())
    );
    let mut q = sqlx::query(&sql);
    for id in &owner_ids {
        q = q.bind(*id);
    }
    let mut barcodes: HashMap<i64, Vec<BarcodeRecord>> = HashMap::new();
    for r in q.fetch_all(pool).await? {
        let owner = i64::from(r.try_get::<i32, _>(0)?);
        if let Some(root) = barcode_owner.get(&owner) {
            barcodes.entry(*root).or_default().push(BarcodeRecord { value: r.try_get(1)?, format: r.try_get(2)? });
        }
    }

    // --- direct view grants (get_permission_grantees(Document, pks, ("view",)))
    let pks: Vec<String> = docs.keys().map(|i| i.to_string()).collect();
    let (users, groups) = view_grantees(pool, &pks).await?;

    let mut out = Vec::with_capacity(docs.len());
    for id in root_ids {
        let Some(d) = docs.remove(id) else { continue };
        let vs = versions.get(id).map(Vec::as_slice).unwrap_or(&[]);
        let effective_content = match newest_version(vs, mode.content) {
            Some(v) => v.content.clone(),
            None => d.content,
        };
        out.push(DocumentRecord {
            id: d.id,
            title: d.title,
            effective_content,
            checksum: d.checksum,
            original_filename: d.original_filename,
            correspondent: d.correspondent,
            document_type: d.document_type,
            storage_path: d.storage_path,
            tags: tags.remove(id).unwrap_or_default(),
            notes: notes.remove(id).unwrap_or_default(),
            custom_fields: cfs.remove(id).unwrap_or_default(),
            barcodes: barcodes.remove(id).unwrap_or_default(),
            created: d.created,
            modified: d.modified,
            added: d.added,
            archive_serial_number: d.asn,
            page_count: d.page_count,
            owner_id: d.owner_id,
            viewer_ids: users.get(id).cloned().unwrap_or_default(),
            viewer_group_ids: groups.get(id).cloned().unwrap_or_default(),
        });
    }
    Ok(out)
}

type Grants = HashMap<i64, Vec<i64>>;

/// `get_permission_grantees(Document, pks, actions=("view",))`: user and group ids
/// holding `view_document` directly, each list sorted (`order_by(identity_field)`).
pub async fn view_grantees(pool: &PgPool, pks: &[String]) -> Result<(Grants, Grants)> {
    let mut users: Grants = HashMap::new();
    let mut groups: Grants = HashMap::new();
    if pks.is_empty() {
        return Ok((users, groups));
    }
    for (table, col, target) in
        [("guardian_userobjectpermission", "user_id", &mut users), ("guardian_groupobjectpermission", "group_id", &mut groups)]
    {
        let sql = format!(
            "SELECT g.object_pk, g.{col} FROM {table} g \
             JOIN django_content_type ct ON ct.id = g.content_type_id \
             JOIN auth_permission p ON p.id = g.permission_id \
             WHERE ct.app_label = 'documents' AND ct.model = 'document' \
               AND p.content_type_id = g.content_type_id AND p.codename = 'view_document' \
               AND g.object_pk IN ({}) \
             ORDER BY g.{col} ASC",
            placeholders(pks.len())
        );
        let mut q = sqlx::query(&sql);
        for pk in pks {
            q = q.bind(pk.as_str());
        }
        for r in q.fetch_all(pool).await? {
            let object_pk: String = r.try_get(0)?;
            let ident = i64::from(r.try_get::<i32, _>(1)?);
            if let Ok(pk) = object_pk.trim().parse::<i64>() {
                target.entry(pk).or_default().push(ident);
            }
        }
    }
    Ok((users, groups))
}
