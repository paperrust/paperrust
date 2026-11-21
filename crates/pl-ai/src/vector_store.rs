//! Vector index in the **upstream format** – `paperless_ai/vector_store.py` +
//! `paperless_ai/tables.py` (v3.4.2): one SQLite file `LLM_INDEX_DIR/llmindex.db` with the
//! sqlite-vec table `documents` (vec0), `document_chunks`, `document_meta`, `index_meta`,
//! schema version 2. SQLite and sqlite-vec 0.1.9 (like the Python wheel version of the reference)
//! are statically linked; the same file can be read and
//! written alternately by Django and PaperRust.
//!
//! Locks (writers mutually, compaction against readers) see [`crate::locks`].

use std::path::{Path, PathBuf};
use std::sync::Once;

use rusqlite::{Connection, OptionalExtension, params};

use crate::node::TextNode;

/// `DB_FILENAME`.
pub const DB_FILENAME: &str = "llmindex.db";
/// `DEFAULT_TABLE_NAME`.
pub const TABLE: &str = "documents";
/// `SCHEMA_VERSION`.
pub const SCHEMA_VERSION: i64 = 2;
/// `COMPACT_BLOAT_RATIO`.
pub const COMPACT_BLOAT_RATIO: f64 = 2.0;
/// `VEC0_CHUNK_SIZE` (upstream fix d14cfc93f / #14678, after v3.4.2): rows per vec0 vector chunk
/// (vec0's default 1024 makes multi-megabyte chunk blobs). Only for tables created from now on;
/// an existing index picks it up on the next rebuild/compaction. sqlite-vec 0.1.9 (Django
/// v3.4.2 and PaperRust) reads such tables, the file stays shared.
pub const VEC0_CHUNK_SIZE: usize = 32;
/// `DB_PAGE_SIZE` (upstream fix d14cfc93f): SQLite page size; only takes effect on a new file
/// (set before WAL mode), so an existing index gets it when compacted.
pub const DB_PAGE_SIZE: u32 = 65536;
/// `BATCH_SIZE`.
pub const BATCH_SIZE: usize = 500;
/// `_MAX_INLINE_IN_VALUES` (upstream fix 62d729e30 / #14754, after v3.4.2; before: `_MAX_IN_VALUES`):
/// the longest IN / NOT IN list bound as one SQLite parameter per value (headroom under
/// `SQLITE_MAX_VARIABLE_NUMBER` = 32766). Longer lists are bound as **one** JSON array and read
/// with `json_each(?)` – v3.4.2 refused them and returned no rows, so a chat over more than
/// 32700 permitted documents found nothing.
pub const MAX_IN_VALUES: usize = 32700;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type SResult<T> = Result<T, StoreError>;

static VEC_INIT: Once = Once::new();

fn register_vec() {
    VEC_INIT.call_once(|| {
        // SAFETY: sqlite3_vec_init has the signature of a SQLite extension entry function
        // (`int(sqlite3*, char**, const sqlite3_api_routines*)`); sqlite-vec is built with SQLITE_CORE
        // and uses the statically linked SQLite. `sqlite3_auto_extension` registers it
        // for all future connections of this process (as in the sqlite-vec documentation).
        unsafe {
            #[allow(clippy::missing_transmute_annotations)]
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ())));
        }
    });
}

/// Filter on `document_id` (upstream `_build_where`, only IN/NIN/NE/EQ).
#[derive(Debug, Clone, PartialEq)]
pub enum DocFilter {
    In(Vec<i64>),
    NotIn(Vec<i64>),
    Ne(i64),
    Eq(i64),
}

/// A bound parameter of [`build_where`].
#[derive(Debug, Clone, PartialEq)]
pub enum WhereParam {
    Int(i64),
    /// JSON array of the ids for `json_each(?)`.
    Json(String),
}

impl WhereParam {
    fn to_sql(&self) -> rusqlite::types::Value {
        match self {
            WhereParam::Int(i) => rusqlite::types::Value::Integer(*i),
            WhereParam::Json(s) => rusqlite::types::Value::Text(s.clone()),
        }
    }
}

/// `_build_where(filters)` with condition AND.
pub fn build_where(filters: &[DocFilter]) -> (String, Vec<WhereParam>) {
    if filters.is_empty() {
        return (String::new(), Vec::new());
    }
    let mut clauses = Vec::new();
    let mut p = Vec::new();
    for f in filters {
        match f {
            DocFilter::In(v) | DocFilter::NotIn(v) => {
                let is_in = matches!(f, DocFilter::In(_));
                if v.is_empty() {
                    clauses.push(if is_in { "1 = 0".to_string() } else { "1 = 1".to_string() });
                    continue;
                }
                let op = if is_in { "IN" } else { "NOT IN" };
                if v.len() > MAX_IN_VALUES {
                    // upstream fix 62d729e30: one JSON parameter instead of refusing
                    clauses.push(format!("document_id {op} (SELECT value FROM json_each(?))"));
                    p.push(WhereParam::Json(serde_json::to_string(v).unwrap_or_else(|_| "[]".into())));
                    continue;
                }
                let ph = vec!["?"; v.len()].join(",");
                clauses.push(format!("document_id {op} ({ph})"));
                p.extend(v.iter().copied().map(WhereParam::Int));
            }
            DocFilter::Eq(x) => {
                clauses.push("document_id = ?".into());
                p.push(WhereParam::Int(*x));
            }
            DocFilter::Ne(x) => {
                clauses.push("document_id != ?".into());
                p.push(WhereParam::Int(*x));
            }
        }
    }
    (format!("({})", clauses.join(" AND ")), p)
}

/// `_pack` (float32 little-endian).
pub fn pack(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// `_unpack`.
pub fn unpack(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

/// A hit of the similarity search (`NodeWithScore`).
#[derive(Debug, Clone)]
pub struct Hit {
    pub node: TextNode,
    pub score: f64,
}

/// A node to be written (node + embedding + `modified`).
#[derive(Debug, Clone)]
pub struct Row {
    pub node: TextNode,
    pub embedding: Vec<f32>,
}

impl Row {
    fn document_id(&self) -> SResult<i64> {
        // upstream: int(node.ref_doc_id or node.metadata["document_id"])
        self.node
            .ref_doc_id
            .as_deref()
            .and_then(|s| s.parse().ok())
            .or_else(|| self.node.document_id())
            .ok_or_else(|| StoreError::Other("node without document_id".into()))
    }
    fn modified(&self) -> String {
        match self.node.metadata.get("modified") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Null) | None => String::new(),
            Some(v) => crate::node::py_str(v),
        }
    }
}

/// `PaperlessSqliteVecVectorStore`.
pub struct VecStore {
    dir: PathBuf,
    embed_model_name: Option<String>,
    conn: Connection,
}

fn open_connection(path: &Path) -> SResult<Connection> {
    register_vec();
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    conn.execute_batch(&format!("PRAGMA page_size={DB_PAGE_SIZE}"))?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    conn.execute_batch("PRAGMA synchronous=NORMAL")?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS index_meta (key TEXT PRIMARY KEY, value TEXT)")?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS document_chunks (chunk_id TEXT PRIMARY KEY, document_id INTEGER NOT NULL)")?;
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_document_chunks_document_id ON document_chunks (document_id)")?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS document_meta (document_id INTEGER PRIMARY KEY, modified TEXT NOT NULL)")?;
    Ok(conn)
}

fn meta_get(conn: &Connection, key: &str) -> SResult<Option<String>> {
    Ok(conn.query_row("SELECT value FROM index_meta WHERE key = ?", [key], |r| r.get::<_, Option<String>>(0)).optional()?.flatten())
}

fn meta_set(conn: &Connection, key: &str, value: &str) -> SResult<()> {
    conn.execute(
        "INSERT INTO index_meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn meta_int(conn: &Connection, key: &str) -> SResult<Option<i64>> {
    Ok(meta_get(conn, key)?.and_then(|v| v.trim().parse().ok()))
}

fn create_vec_table(conn: &Connection, dim: usize) -> SResult<()> {
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {TABLE} USING vec0(id TEXT PRIMARY KEY, document_id INTEGER, +node_content TEXT, embedding float[{dim}] distance_metric=cosine, chunk_size={VEC0_CHUNK_SIZE})"
    ))?;
    Ok(())
}

const INSERT: &str = "INSERT INTO documents (id, document_id, node_content, embedding) VALUES (?, ?, ?, ?)";

impl VecStore {
    /// Opens (or creates) `dir/llmindex.db`. `embed_model_name` is stored when the table is created
    /// (upstream `write_store(embed_model_name=…)`).
    pub fn open(dir: &Path, embed_model_name: Option<&str>) -> SResult<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = open_connection(&dir.join(DB_FILENAME))?;
        Ok(Self { dir: dir.to_path_buf(), embed_model_name: embed_model_name.map(str::to_string), conn })
    }

    pub fn db_path(&self) -> PathBuf {
        self.dir.join(DB_FILENAME)
    }

    pub fn table_exists(&self) -> SResult<bool> {
        Ok(self.conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?", [TABLE], |_| Ok(())).optional()?.is_some())
    }

    pub fn vector_dim(&self) -> SResult<Option<i64>> {
        if !self.table_exists()? {
            return Ok(None);
        }
        meta_int(&self.conn, "dim")
    }

    pub fn drop_table(&self) -> SResult<()> {
        self.conn.execute_batch(&format!("DROP TABLE IF EXISTS {TABLE}"))?;
        self.conn.execute_batch("DELETE FROM index_meta")?;
        self.conn.execute_batch("DELETE FROM document_chunks")?;
        self.conn.execute_batch("DELETE FROM document_meta")?;
        Ok(())
    }

    pub fn stored_model_name(&self) -> SResult<Option<String>> {
        if !self.table_exists()? {
            return Ok(None);
        }
        meta_get(&self.conn, "embed_model")
    }

    pub fn config_mismatch(&self, model_name: &str) -> SResult<bool> {
        Ok(match self.stored_model_name()? {
            None => false,
            Some(s) => s != model_name,
        })
    }

    fn create_table(&self, dim: usize) -> SResult<()> {
        create_vec_table(&self.conn, dim)?;
        meta_set(&self.conn, "dim", &dim.to_string())?;
        meta_set(&self.conn, "schema_version", &SCHEMA_VERSION.to_string())?;
        if let Some(m) = &self.embed_model_name {
            meta_set(&self.conn, "embed_model", m)?;
        }
        Ok(())
    }

    fn insert_rows(&self, rows: &[Row]) -> SResult<()> {
        let mut st = self.conn.prepare_cached(INSERT)?;
        let mut ch = self.conn.prepare_cached("INSERT INTO document_chunks (chunk_id, document_id) VALUES (?, ?)")?;
        let mut by_doc: Vec<(i64, String)> = Vec::new();
        for r in rows {
            let d = r.document_id()?;
            st.execute(params![r.node.id, d, r.node.node_content(), pack(&r.embedding)])?;
            ch.execute(params![r.node.id, d])?;
            // {r.document_id: r.modified for r in rows} – last value wins, order of first occurrence
            if let Some(e) = by_doc.iter_mut().find(|(x, _)| *x == d) {
                e.1 = r.modified();
            } else {
                by_doc.push((d, r.modified()));
            }
        }
        let mut up = self.conn.prepare_cached(
            "INSERT INTO document_meta (document_id, modified) VALUES (?, ?) ON CONFLICT(document_id) DO UPDATE SET modified = excluded.modified",
        )?;
        for (d, m) in by_doc {
            up.execute(params![d, m])?;
        }
        self.conn.execute(
            "INSERT INTO index_meta (key, value) VALUES ('total_inserts', ?) ON CONFLICT(key) DO UPDATE SET value = CAST(index_meta.value AS INTEGER) + CAST(excluded.value AS INTEGER)",
            [rows.len().to_string()],
        )?;
        Ok(())
    }

    fn delete_chunks(&self, document_id: i64) -> SResult<()> {
        let ids: Vec<String> = {
            let mut st = self.conn.prepare_cached("SELECT chunk_id FROM document_chunks WHERE document_id = ?")?;
            st.query_map([document_id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?
        };
        {
            let mut del = self.conn.prepare_cached(&format!("DELETE FROM {TABLE} WHERE id = ?"))?;
            for id in ids {
                del.execute([id])?;
            }
        }
        self.conn.execute("DELETE FROM document_chunks WHERE document_id = ?", [document_id])?;
        self.conn.execute("DELETE FROM document_meta WHERE document_id = ?", [document_id])?;
        Ok(())
    }

    fn tx<T>(&self, f: impl FnOnce() -> SResult<T>) -> SResult<T> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match f() {
            Ok(v) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// `add(nodes)`.
    pub fn add(&self, rows: &[Row]) -> SResult<()> {
        if rows.is_empty() {
            return Ok(());
        }
        self.tx(|| {
            if !self.table_exists()? {
                self.create_table(rows[0].embedding.len())?;
            }
            self.insert_rows(rows)
        })
    }

    /// `upsert_document(document_id, nodes)`.
    pub fn upsert_document(&self, document_id: i64, rows: &[Row]) -> SResult<()> {
        self.tx(|| {
            let mut exists = self.table_exists()?;
            if !rows.is_empty() && !exists {
                self.create_table(rows[0].embedding.len())?;
                exists = true;
            }
            if exists {
                self.delete_chunks(document_id)?;
            }
            if !rows.is_empty() {
                self.insert_rows(rows)?;
            }
            Ok(())
        })
    }

    /// `delete(ref_doc_id)`.
    pub fn delete(&self, document_id: i64) -> SResult<()> {
        if self.table_exists()? {
            self.tx(|| self.delete_chunks(document_id))?;
        }
        Ok(())
    }

    /// `query(VectorStoreQuery(query_embedding, similarity_top_k, filters))`.
    pub fn query(&self, embedding: &[f32], top_k: usize, filters: &[DocFilter]) -> SResult<Vec<Hit>> {
        if !self.table_exists()? {
            return Ok(Vec::new());
        }
        let (w, p) = build_where(filters);
        let mut sql = format!("SELECT id, node_content, embedding, distance FROM {TABLE} WHERE embedding MATCH ? AND k = ?");
        if !w.is_empty() {
            sql.push_str(" AND ");
            sql.push_str(&w);
        }
        let mut st = self.conn.prepare(&sql)?;
        let mut binds: Vec<rusqlite::types::Value> =
            vec![rusqlite::types::Value::Blob(pack(embedding)), rusqlite::types::Value::Integer(top_k as i64)];
        binds.extend(p.iter().map(WhereParam::to_sql));
        let rows = st.query_map(rusqlite::params_from_iter(binds.iter()), |r| Ok((r.get::<_, String>(1)?, r.get::<_, Option<f64>>(3)?)))?;
        let mut out = Vec::new();
        for r in rows.take(top_k) {
            let (nc, dist) = r?;
            let node = TextNode::from_node_content(&nc).map_err(StoreError::Other)?;
            out.push(Hit { node, score: 1.0 - dist.unwrap_or(1.0) });
        }
        Ok(out)
    }

    /// `indexed_document_ids()` (upstream fix 62d729e30 / #14754): all indexed document ids
    /// from `document_meta` (one row per document, cheaper than reading the vec0 table).
    pub fn indexed_document_ids(&self) -> SResult<std::collections::HashSet<i64>> {
        if !self.table_exists()? {
            return Ok(Default::default());
        }
        let mut st = self.conn.prepare("SELECT document_id FROM document_meta")?;
        let r = st.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<_, _>>()?;
        Ok(r)
    }

    /// `get_modified_times()`.
    pub fn get_modified_times(&self) -> SResult<std::collections::HashMap<String, String>> {
        if !self.table_exists()? {
            return Ok(Default::default());
        }
        let mut st = self.conn.prepare("SELECT document_id, modified FROM document_meta")?;
        let r = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, Option<String>>(1)?.unwrap_or_default())))?
            .collect::<Result<_, _>>()?;
        Ok(r)
    }

    fn stored_schema_version(&self) -> SResult<Option<i64>> {
        if !self.table_exists()? {
            return Ok(None);
        }
        Ok(Some(meta_int(&self.conn, "schema_version")?.unwrap_or(SCHEMA_VERSION)))
    }

    /// `has_pending_migration()`. PaperRust only knows schema 2; a v1 index (before upstream 3.x)
    /// is, as with upstream `re-embed`… here: rebuild required (see contract).
    pub fn has_pending_migration(&self) -> SResult<bool> {
        Ok(self.stored_schema_version()?.is_some_and(|v| v < SCHEMA_VERSION))
    }

    /// Number of live nodes.
    pub fn live_rows(&self) -> SResult<i64> {
        Ok(self.conn.query_row("SELECT count(*) FROM document_chunks", [], |r| r.get(0))?)
    }

    /// `compact(force)`: copy live rows into `llmindex.db.compact` and take over via file swap.
    /// Must run under exclusive access (no readers) – see [`crate::locks`].
    pub fn compact(&mut self, force: bool) -> SResult<bool> {
        if !self.table_exists()? || self.has_pending_migration()? {
            return Ok(false);
        }
        let live = self.live_rows()?;
        let total = meta_int(&self.conn, "total_inserts")?.filter(|v| *v != 0).unwrap_or(live);
        if !force && (total as f64) <= (live.max(1) as f64) * COMPACT_BLOAT_RATIO {
            return Ok(false);
        }
        let Some(dim) = self.vector_dim()? else { return Ok(false) };
        let db = self.db_path();
        let compact = PathBuf::from(format!("{}.compact", db.display()));
        for suf in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suf}", compact.display()));
        }
        let res = (|| -> SResult<()> {
            let dst = open_connection(&compact)?;
            create_vec_table(&dst, dim as usize)?;
            for k in ["dim", "embed_model", "schema_version"] {
                if let Some(v) = meta_get(&self.conn, k)? {
                    meta_set(&dst, k, &v)?;
                }
            }
            dst.execute_batch("BEGIN IMMEDIATE")?;
            let mut copied = 0usize;
            {
                let mut src = self.conn.prepare(&format!("SELECT id, document_id, node_content, embedding FROM {TABLE}"))?;
                let mut rows = src.query([])?;
                let mut ins = dst.prepare(INSERT)?;
                let mut ch = dst.prepare("INSERT INTO document_chunks (chunk_id, document_id) VALUES (?, ?)")?;
                while let Some(r) = rows.next()? {
                    let id: String = r.get(0)?;
                    let d: i64 = r.get(1)?;
                    let nc: String = r.get(2)?;
                    let e: Vec<u8> = r.get(3)?;
                    ins.execute(params![id, d, nc, e])?;
                    ch.execute(params![id, d])?;
                    copied += 1;
                }
                let mut m = self.conn.prepare("SELECT document_id, modified FROM document_meta")?;
                let mut mr = m.query([])?;
                let mut up = dst.prepare("INSERT INTO document_meta (document_id, modified) VALUES (?, ?) ON CONFLICT(document_id) DO UPDATE SET modified = excluded.modified")?;
                while let Some(r) = mr.next()? {
                    up.execute(params![r.get::<_, i64>(0)?, r.get::<_, String>(1)?])?;
                }
            }
            meta_set(&dst, "total_inserts", &copied.to_string())?;
            dst.execute_batch("COMMIT")?;
            drop(dst);
            Ok(())
        })();
        if let Err(e) = res {
            for suf in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suf}", compact.display()));
            }
            return Err(e);
        }
        // _swap_in_compact
        let placeholder = Connection::open_in_memory()?;
        let old = std::mem::replace(&mut self.conn, placeholder);
        old.close().map_err(|(_, e)| StoreError::Sql(e))?;
        for suf in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suf}", compact.display()));
        }
        std::fs::rename(&compact, &db)?;
        self.conn = open_connection(&db)?;
        Ok(true)
    }

    /// Raw access (tests).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_clause() {
        assert_eq!(build_where(&[]), (String::new(), vec![]));
        assert_eq!(build_where(&[DocFilter::In(vec![])]).0, "(1 = 0)");
        assert_eq!(build_where(&[DocFilter::NotIn(vec![])]).0, "(1 = 1)");
        let (w, p) = build_where(&[DocFilter::In(vec![1, 2]), DocFilter::Ne(5)]);
        assert_eq!(w, "(document_id IN (?,?) AND document_id != ?)");
        assert_eq!(p, vec![WhereParam::Int(1), WhereParam::Int(2), WhereParam::Int(5)]);
    }

    /// upstream fix 62d729e30 (#14754): lists over the inline limit become one JSON parameter
    /// (`test_build_where_large_in_uses_json_each` / `..._large_nin_...`) and really filter.
    #[test]
    fn large_in_and_not_in_use_json_each() {
        let big: Vec<i64> = (1..=MAX_IN_VALUES as i64 + 1).collect();
        let (w, p) = build_where(&[DocFilter::In(big.clone())]);
        assert_eq!(w, "(document_id IN (SELECT value FROM json_each(?)))");
        assert_eq!(p.len(), 1);
        let (w, p) = build_where(&[DocFilter::NotIn(big.clone())]);
        assert_eq!(w, "(document_id NOT IN (SELECT value FROM json_each(?)))");
        let WhereParam::Json(j) = &p[0] else { panic!("json param") };
        assert!(j.starts_with("[1,2,3,") && j.ends_with(&format!(",{}]", MAX_IN_VALUES + 1)));
        // exactly at the limit: still inline
        let (w, p) = build_where(&[DocFilter::In(big[..MAX_IN_VALUES].to_vec())]);
        assert!(w.starts_with("(document_id IN (?,"));
        assert_eq!(p.len(), MAX_IN_VALUES);

        let dir = TmpDir::new("bigin");
        let st = VecStore::open(dir.path(), Some("m")).unwrap();
        st.upsert_document(1, &[row("1-a", 1)]).unwrap();
        st.upsert_document(40_000, &[row("40000-a", 40_000)]).unwrap();
        st.upsert_document(50_000, &[row("50000-a", 50_000)]).unwrap();
        assert_eq!(st.indexed_document_ids().unwrap(), [1, 40_000, 50_000].into_iter().collect());
        let docs = |f: &[DocFilter]| {
            let mut d: Vec<i64> =
                st.query(&[0.1, 0.2, 0.3, 0.4], 10, f).unwrap().iter().filter_map(|h| h.node.ref_doc_id.as_deref()?.parse().ok()).collect();
            d.sort_unstable();
            d
        };
        let permitted: Vec<i64> = (1..=45_000).collect();
        assert_eq!(docs(&[DocFilter::In(permitted.clone())]), vec![1, 40_000]);
        assert_eq!(docs(&[DocFilter::NotIn(permitted)]), vec![50_000]);
    }

    // upstream fix d14cfc93f (#14678): TestVecChunkSize / TestIndexPageSize
    fn row(id: &str, doc: i64) -> Row {
        let mut rel = serde_json::Map::new();
        rel.insert(
            "1".into(),
            serde_json::json!({"node_id": doc.to_string(), "node_type": "4", "metadata": {}, "hash": "", "class_name": "RelatedNodeInfo"}),
        );
        Row {
            node: TextNode {
                id: id.into(),
                text: format!("text {id}"),
                metadata: serde_json::Map::new(),
                excluded_embed_metadata_keys: vec![],
                excluded_llm_metadata_keys: vec![],
                relationships: rel,
                start_char_idx: None,
                end_char_idx: None,
                metadata_template: "{key}: {value}".into(),
                metadata_separator: "
"
                .into(),
                text_template: "{metadata_str}

{content}"
                    .into(),
                ref_doc_id: Some(doc.to_string()),
            },
            embedding: vec![0.1, 0.2, 0.3, 0.4],
        }
    }

    /// Temp directory without an extra dev-dependency; removed on drop.
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("pl-ai-vs-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn vec_table_sql(st: &VecStore) -> String {
        st.conn().query_row("SELECT sql FROM sqlite_master WHERE name = ?", [TABLE], |r| r.get(0)).unwrap()
    }

    fn page_size(st: &VecStore) -> u32 {
        st.conn().query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap()
    }

    fn chunk_count(st: &VecStore) -> i64 {
        st.conn().query_row("SELECT count(*) FROM document_chunks", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn new_index_uses_chunk_size_and_page_size() {
        let dir = TmpDir::new("new");
        let st = VecStore::open(dir.path(), Some("m")).unwrap();
        st.upsert_document(1, &[row("1-a", 1)]).unwrap();
        assert!(vec_table_sql(&st).contains(&format!("chunk_size={VEC0_CHUNK_SIZE}")));
        assert_eq!(page_size(&st), DB_PAGE_SIZE);
        let hits = st.query(&[0.1, 0.2, 0.3, 0.4], 5, &[]).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn compact_moves_legacy_table_and_page_size() {
        let dir = TmpDir::new("legacy");
        // legacy file: 4096-byte pages, vec0 table without chunk_size (v3.4.2 layout)
        {
            register_vec();
            let c = Connection::open(dir.path().join(DB_FILENAME)).unwrap();
            c.execute_batch("PRAGMA page_size=4096; CREATE TABLE _seed(x); DROP TABLE _seed;").unwrap();
            c.execute_batch(&format!(
                "CREATE VIRTUAL TABLE {TABLE} USING vec0(id TEXT PRIMARY KEY, document_id INTEGER, +node_content TEXT, embedding float[4] distance_metric=cosine)"
            ))
            .unwrap();
            c.execute_batch("CREATE TABLE index_meta (key TEXT PRIMARY KEY, value TEXT); INSERT INTO index_meta VALUES ('dim', '4'), ('schema_version', '2');")
                .unwrap();
        }
        let mut st = VecStore::open(dir.path(), Some("m")).unwrap();
        assert_eq!(page_size(&st), 4096);
        st.upsert_document(1, &[row("1-a", 1), row("1-b", 1)]).unwrap();
        st.upsert_document(2, &[row("2-a", 2)]).unwrap();
        assert!(!vec_table_sql(&st).contains("chunk_size"));

        assert!(st.compact(true).unwrap());

        assert!(vec_table_sql(&st).contains(&format!("chunk_size={VEC0_CHUNK_SIZE}")));
        assert_eq!(page_size(&st), DB_PAGE_SIZE);
        assert_eq!(chunk_count(&st), 3);
    }
}
