//! Local replica (SQLite, sqlx-sqlite with bundled libsqlite3 incl. FTS5), docs/SYNC.md §4.2.
//!
//! One file per server profile (`replica.sqlite`). Migrations via `PRAGMA user_version`
//! (one SQL file per stage under `migrations/`, in one transaction).

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use serde_json::{Map, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::error::{Error, Result};

/// Migrations in order; index + 1 = `user_version` after applying.
pub const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_replica", include_str!("../migrations/0001_replica.sql")),
    ("0002_http_cache", include_str!("../migrations/0002_http_cache.sql")),
];

/// Metadata kinds that are always mirrored completely (docs/SYNC.md §4.2).
pub const META_KINDS: [&str; 8] = ["tag", "correspondent", "document_type", "storage_path", "custom_field", "saved_view", "user", "group"];

/// Now as RFC 3339 UTC.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Header row of the replica (table `remote`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteRow {
    pub url: String,
    pub flavor: String,
    pub server_uuid: Option<String>,
    pub user_name: Option<String>,
    pub device_uuid: Option<String>,
    pub cursor: Option<String>,
    pub perm_epoch: Option<i64>,
    pub last_full_sync: Option<String>,
    pub last_contact: Option<String>,
}

/// Ein Auswahleintrag (docs/SYNC.md §4.4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SelectionItem {
    /// `document` | `tag` | `saved_view`
    #[serde(rename = "type")]
    pub kind: String,
    pub key: String,
    /// `preferred` | `original` | `both` | `none`
    #[serde(default = "default_files")]
    pub files: String,
}

fn default_files() -> String {
    "preferred".into()
}

/// Entry of the file cache.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    pub doc: String,
    pub variant: String,
    pub path: String,
    pub etag: Option<String>,
    pub checksum: Option<String>,
    pub size: i64,
    pub state: String,
    pub pinned: bool,
}

pub(crate) fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}

/// Anzeigename eines Metadaten-Objekts.
pub fn object_name(v: &Value) -> Option<String> {
    s(v, "name").or_else(|| s(v, "username"))
}

/// Expression for the FTS column `names` of a document (correspondent, type, storage path, tags).
const NAMES_EXPR: &str = "trim(\
    coalesce((SELECT name FROM object WHERE kind = 'correspondent' AND key = document.correspondent), '') || ' ' || \
    coalesce((SELECT name FROM object WHERE kind = 'document_type' AND key = document.document_type), '') || ' ' || \
    coalesce((SELECT name FROM object WHERE kind = 'storage_path' AND key = document.storage_path), '') || ' ' || \
    coalesce((SELECT group_concat(o.name, ' ') FROM document_tag dt JOIN object o ON o.kind = 'tag' AND o.key = dt.tag \
              WHERE dt.doc = document.key), ''))";

/// Das Replikat.
#[derive(Clone, Debug)]
pub struct Replica {
    pool: SqlitePool,
}

impl Replica {
    /// Open/create and migrate.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(false)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await?;
        let r = Replica { pool };
        r.migrate().await?;
        Ok(r)
    }

    /// Tests only: purely in memory (one connection).
    pub async fn open_in_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?;
        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await?;
        let r = Replica { pool };
        r.migrate().await?;
        Ok(r)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Apply pending migrations; returns the schema version.
    pub async fn migrate(&self) -> Result<i64> {
        // only pool + `sqlx::query` (multiple statements per file work with sqlx-sqlite): `raw_sql`
        // or `Connection::begin(&mut conn)` make the future not `Send` for
        // `tokio::spawn` (rust-lang/rust#102211, "Executor is not general enough")
        let current: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(&self.pool).await?;
        for (idx, (_name, sql)) in MIGRATIONS.iter().enumerate() {
            let version = idx as i64 + 1;
            if version <= current {
                continue;
            }
            let mut tx = self.pool.begin().await?;
            sqlx::query(sql).execute(&mut *tx).await?;
            sqlx::query(&format!("PRAGMA user_version = {version}")).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        Ok(MIGRATIONS.len() as i64)
    }

    // ------------------------------------------------------------------ remote / settings

    /// Kopfzeile anlegen bzw. URL/Art setzen.
    pub async fn ensure_remote(&self, url: &str, flavor: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO remote (id, url, flavor) VALUES (1, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET url = excluded.url, flavor = excluded.flavor",
        )
        .bind(url.to_string())
        .bind(flavor.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remote(&self) -> Result<Option<RemoteRow>> {
        let r = sqlx::query(
            "SELECT url, flavor, server_uuid, user_name, device_uuid, cursor, perm_epoch, last_full_sync, last_contact \
             FROM remote WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(|r| RemoteRow {
            url: r.get(0),
            flavor: r.get(1),
            server_uuid: r.get(2),
            user_name: r.get(3),
            device_uuid: r.get(4),
            cursor: r.get(5),
            perm_epoch: r.get(6),
            last_full_sync: r.get(7),
            last_contact: r.get(8),
        }))
    }

    /// Set a field of the header row (known columns only). No `async fn`: the future holds
    /// no borrowed arguments (otherwise rust-lang/rust#102211 at the `tokio::spawn` of the loop).
    pub fn set_remote_field(&self, field: &str, value: Option<&str>) -> impl std::future::Future<Output = Result<()>> + Send + 'static {
        const OK: [&str; 9] = [
            "server_uuid",
            "user_name",
            "device_uuid",
            "cursor",
            "ngx_watermark",
            "last_full_sync",
            "last_contact",
            "me_json",
            "ui_settings",
        ];
        let known = OK.contains(&field);
        let sql = format!("UPDATE remote SET {field} = ? WHERE id = 1");
        let field = field.to_string();
        let value: Option<String> = value.map(str::to_string);
        let pool = self.pool.clone();
        async move {
            if !known {
                return Err(Error::Invalid(format!("remote field {field}")));
            }
            sqlx::query(&sql).bind(value).execute(&pool).await?;
            Ok(())
        }
    }

    pub async fn remote_text(&self, field: &str) -> Result<Option<String>> {
        if !["me_json", "ui_settings", "cursor", "server_uuid"].contains(&field) {
            return Err(Error::Invalid(format!("remote field {field}")));
        }
        Ok(sqlx::query_scalar::<_, Option<String>>(&format!("SELECT {field} FROM remote WHERE id = 1"))
            .fetch_optional(&self.pool)
            .await?
            .flatten())
    }

    pub async fn set_perm_epoch(&self, epoch: Option<i64>) -> Result<()> {
        sqlx::query("UPDATE remote SET perm_epoch = ? WHERE id = 1").bind(epoch).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT value FROM setting WHERE key = ?").bind(key.to_string()).fetch_optional(&self.pool).await?)
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query("INSERT INTO setting (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value")
            .bind(key.to_string())
            .bind(value)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Discard everything server-related (foreign server_uuid, full sync from scratch). Selection and
    /// settings stay; file rows are deleted (the caller deletes the files).
    pub async fn reset_data(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for t in ["object", "document_tag", "document", "file", "http_cache"] {
            sqlx::query(&format!("DELETE FROM {t}")).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO fts (fts) VALUES ('rebuild')").execute(&mut *tx).await?;
        sqlx::query("UPDATE remote SET cursor = NULL, perm_epoch = NULL, last_full_sync = NULL WHERE id = 1").execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    // ------------------------------------------------------------------ Metadaten

    /// Insert/replace an object. Returns: name has changed (→ `names` of the documents anew).
    pub async fn upsert_object(&self, kind: &str, obj: &Value) -> Result<bool> {
        let mut conn = self.pool.acquire().await?;
        upsert_object_on(&mut conn, kind, obj).await
    }

    /// Several objects in one transaction; name changes refresh `names`.
    pub async fn upsert_objects(&self, kind: &str, objs: &[Value]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let mut renamed = Vec::new();
        for o in objs {
            if upsert_object_on(&mut tx, kind, o).await?
                && let Some(k) = s(o, "uuid")
            {
                renamed.push(k);
            }
        }
        for k in renamed {
            refresh_names_for_ref(&mut tx, kind, &k).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_object(&self, kind: &str, key: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let n = sqlx::query("DELETE FROM object WHERE kind = ? AND key = ?")
            .bind(kind.to_string())
            .bind(key.to_string())
            .execute(&mut *tx)
            .await?;
        if kind == "tag" {
            let docs: Vec<String> =
                sqlx::query_scalar("SELECT doc FROM document_tag WHERE tag = ?").bind(key.to_string()).fetch_all(&mut *tx).await?;
            sqlx::query("DELETE FROM document_tag WHERE tag = ?").bind(key.to_string()).execute(&mut *tx).await?;
            for d in docs {
                refresh_names_where(&mut tx, "key = ?", &d).await?;
            }
        } else {
            refresh_names_for_ref(&mut tx, kind, key).await?;
        }
        tx.commit().await?;
        Ok(n.rows_affected() > 0)
    }

    pub async fn object(&self, kind: &str, key: &str) -> Result<Option<Value>> {
        let j: Option<String> = sqlx::query_scalar("SELECT json FROM object WHERE kind = ? AND key = ?")
            .bind(kind.to_string())
            .bind(key.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(match j {
            Some(j) => Some(serde_json::from_str(&j)?),
            None => None,
        })
    }

    /// `(key, rev)` of all objects of a kind.
    pub async fn object_revs(&self, kind: &str) -> Result<Vec<(String, i64)>> {
        let rows =
            sqlx::query("SELECT key, coalesce(rev, 0) FROM object WHERE kind = ?").bind(kind.to_string()).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    pub async fn count_objects(&self, kind: &str) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM object WHERE kind = ?").bind(kind.to_string()).fetch_one(&self.pool).await?)
    }

    // ------------------------------------------------------------------ Dokumente

    /// Insert/replace documents (one transaction). `detail`: the object comes from
    /// `objects/batch` with `include=content,notes` (content + notes are adopted); otherwise
    /// existing contents stay, and on a changed `rev` `has_detail` is reset.
    pub async fn upsert_documents(&self, docs: &[Value], detail: bool, in_selection: bool) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for d in docs {
            upsert_document_on(&mut tx, d, detail, in_selection).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Delete a document (row, tags, file rows). Returns: paths of the cache files.
    pub async fn delete_document(&self, key: &str) -> Result<Vec<String>> {
        let mut tx = self.pool.begin().await?;
        let paths: Vec<String> =
            sqlx::query_scalar("SELECT path FROM file WHERE doc = ?").bind(key.to_string()).fetch_all(&mut *tx).await?;
        sqlx::query("DELETE FROM file WHERE doc = ?").bind(key.to_string()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM document_tag WHERE doc = ?").bind(key.to_string()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM document WHERE key = ?").bind(key.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(paths)
    }

    /// Is the document present locally? → (rev, in_selection, has_detail)
    pub async fn document_state(&self, key: &str) -> Result<Option<(i64, bool, bool)>> {
        let r = sqlx::query("SELECT coalesce(rev, 0), in_selection, has_detail FROM document WHERE key = ?")
            .bind(key.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.map(|r| (r.get(0), r.get::<i64, _>(1) != 0, r.get::<i64, _>(2) != 0)))
    }

    /// Alle lokalen Dokumente: key → (rev, in_selection, has_detail).
    pub async fn document_states(&self) -> Result<Vec<(String, i64, bool, bool)>> {
        let rows = sqlx::query("SELECT key, coalesce(rev, 0), in_selection, has_detail FROM document").fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1), r.get::<i64, _>(2) != 0, r.get::<i64, _>(3) != 0)).collect())
    }

    /// Document leaves the selection: metadata stays (7 days), files gone (caller).
    pub async fn mark_unselected(&self, key: &str, at: &str) -> Result<()> {
        sqlx::query("UPDATE document SET in_selection = 0, left_selection = coalesce(left_selection, ?) WHERE key = ?")
            .bind(at.to_string())
            .bind(key.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remove documents no longer selected older than `before` (RFC 3339).
    pub async fn purge_unselected(&self, before: &str) -> Result<Vec<String>> {
        let keys: Vec<String> = sqlx::query_scalar("SELECT key FROM document WHERE in_selection = 0 AND left_selection < ?")
            .bind(before.to_string())
            .fetch_all(&self.pool)
            .await?;
        for k in &keys {
            self.delete_document(k).await?;
        }
        Ok(keys)
    }

    pub async fn document_json(&self, key: &str) -> Result<Option<Value>> {
        let j: Option<String> =
            sqlx::query_scalar("SELECT json FROM document WHERE key = ?").bind(key.to_string()).fetch_optional(&self.pool).await?;
        Ok(match j {
            Some(j) => Some(serde_json::from_str(&j)?),
            None => None,
        })
    }

    /// Stored v2 document, optionally with `content`/`notes` (like `include=`).
    pub async fn document_full(&self, key: &str, content: bool, notes: bool) -> Result<Option<Value>> {
        let r = sqlx::query("SELECT json, content, notes_json, has_detail FROM document WHERE key = ?")
            .bind(key.to_string())
            .fetch_optional(&self.pool)
            .await?;
        let Some(r) = r else { return Ok(None) };
        let mut v: Value = serde_json::from_str(r.get::<&str, _>(0))?;
        if content {
            v["content"] = r.get::<Option<String>, _>(1).map(Value::String).unwrap_or(Value::Null);
        }
        if notes {
            let n: Option<String> = r.get(2);
            v["notes"] = match n {
                Some(n) => serde_json::from_str(&n)?,
                None => Value::Array(vec![]),
            };
        }
        Ok(Some(v))
    }

    pub async fn count_documents(&self, only_selected: bool) -> Result<i64> {
        let sql = if only_selected { "SELECT count(*) FROM document WHERE in_selection = 1" } else { "SELECT count(*) FROM document" };
        Ok(sqlx::query_scalar(sql).fetch_one(&self.pool).await?)
    }

    // ------------------------------------------------------------------ Auswahl

    pub async fn selection(&self) -> Result<Vec<SelectionItem>> {
        let rows = sqlx::query("SELECT type, key, files FROM selection ORDER BY type, key").fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| SelectionItem { kind: r.get(0), key: r.get(1), files: r.get(2) }).collect())
    }

    pub async fn set_selection(&self, items: &[SelectionItem]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM selection").execute(&mut *tx).await?;
        for it in items {
            sqlx::query("INSERT OR REPLACE INTO selection (type, key, files) VALUES (?, ?, ?)")
                .bind(it.kind.clone())
                .bind(it.key.clone())
                .bind(it.files.clone())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Fingerabdruck (paperless-ngx, SYNC.md §7) eines Dokuments setzen.
    pub async fn set_fingerprint(&self, key: &str, fp: &str) -> Result<()> {
        sqlx::query("UPDATE document SET fingerprint = ? WHERE key = ?")
            .bind(fp.to_string())
            .bind(key.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ------------------------------------------------------------------ Antwort-Cache (v1)

    pub async fn cache_get(&self, key: &str) -> Result<Option<(u16, Option<String>, Vec<u8>)>> {
        let r = sqlx::query("SELECT status, content_type, body FROM http_cache WHERE key = ?")
            .bind(key.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.map(|r| (r.get::<i64, _>(0) as u16, r.get(1), r.get(2))))
    }

    pub async fn cache_put(&self, key: &str, status: u16, content_type: Option<&str>, body: &[u8]) -> Result<()> {
        sqlx::query(
            "INSERT INTO http_cache (key, status, content_type, body, stored) VALUES (?, ?, ?, ?, ?)              ON CONFLICT (key) DO UPDATE SET status = excluded.status, content_type = excluded.content_type,              body = excluded.body, stored = excluded.stored",
        )
        .bind(key.to_string())
        .bind(status as i64)
        .bind(content_type.map(str::to_string))
        .bind(body.to_vec())
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// (kind, key, legacy_id) aller Objekte + Dokumente (`kind = "document"`).
    pub async fn legacy_ids(&self) -> Result<Vec<(String, String, i64)>> {
        let rows = sqlx::query(
            "SELECT kind, key, legacy_id FROM object WHERE legacy_id IS NOT NULL              UNION ALL SELECT 'document', key, legacy_id FROM document WHERE legacy_id IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1), r.get(2))).collect())
    }

    // ------------------------------------------------------------------ Dateien

    pub async fn file(&self, doc: &str, variant: &str) -> Result<Option<FileRow>> {
        let r = sqlx::query("SELECT doc, variant, path, etag, checksum, size, state, pinned FROM file WHERE doc = ? AND variant = ?")
            .bind(doc.to_string())
            .bind(variant.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.map(file_row))
    }

    pub async fn files_of(&self, doc: &str) -> Result<Vec<FileRow>> {
        let rows = sqlx::query("SELECT doc, variant, path, etag, checksum, size, state, pinned FROM file WHERE doc = ?")
            .bind(doc.to_string())
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(file_row).collect())
    }

    pub async fn put_file(&self, f: &FileRow) -> Result<()> {
        let now = now();
        sqlx::query(
            "INSERT INTO file (doc, variant, path, etag, checksum, size, state, pinned, fetched, last_access) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (doc, variant) DO UPDATE SET path = excluded.path, \
             etag = excluded.etag, checksum = excluded.checksum, size = excluded.size, state = excluded.state, \
             pinned = excluded.pinned, fetched = excluded.fetched, last_access = excluded.last_access",
        )
        .bind(f.doc.clone())
        .bind(f.variant.clone())
        .bind(f.path.clone())
        .bind(f.etag.clone())
        .bind(f.checksum.clone())
        .bind(f.size)
        .bind(f.state.clone())
        .bind(f.pinned as i64)
        .bind(now.clone())
        .bind(now.clone())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_file(&self, doc: &str, variant: &str) -> Result<()> {
        sqlx::query("DELETE FROM file WHERE doc = ? AND variant = ?")
            .bind(doc.to_string())
            .bind(variant.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn touch_file(&self, doc: &str, variant: &str) -> Result<()> {
        sqlx::query("UPDATE file SET last_access = ? WHERE doc = ? AND variant = ?")
            .bind(now())
            .bind(doc.to_string())
            .bind(variant.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Mark the files of a document as (not) belonging to the selection.
    pub async fn set_files_pinned(&self, doc: &str, pinned: bool) -> Result<()> {
        sqlx::query("UPDATE file SET pinned = ? WHERE doc = ?").bind(pinned as i64).bind(doc.to_string()).execute(&self.pool).await?;
        Ok(())
    }

    /// Used bytes (all states).
    pub async fn cache_bytes(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT coalesce(sum(size), 0) FROM file").fetch_one(&self.pool).await?)
    }

    /// Evictable files (not selected), oldest use first.
    pub async fn evictable(&self) -> Result<Vec<FileRow>> {
        let rows = sqlx::query(
            "SELECT doc, variant, path, etag, checksum, size, state, pinned FROM file WHERE pinned = 0 ORDER BY last_access, doc",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(file_row).collect())
    }
}

fn file_row(r: sqlx::sqlite::SqliteRow) -> FileRow {
    FileRow {
        doc: r.get(0),
        variant: r.get(1),
        path: r.get(2),
        etag: r.get(3),
        checksum: r.get(4),
        size: r.get(5),
        state: r.get(6),
        pinned: r.get::<i64, _>(7) != 0,
    }
}

async fn upsert_object_on(conn: &mut SqliteConnection, kind: &str, obj: &Value) -> Result<bool> {
    let key = s(obj, "uuid").ok_or_else(|| Error::Protocol(format!("{kind} without uuid")))?;
    let name = object_name(obj);
    let old: Option<Option<String>> = sqlx::query_scalar("SELECT name FROM object WHERE kind = ? AND key = ?")
        .bind(kind.to_string())
        .bind(key.clone())
        .fetch_optional(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO object (kind, key, legacy_id, rev, json, name) VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT (kind, key) DO UPDATE SET legacy_id = excluded.legacy_id, rev = excluded.rev, \
         json = excluded.json, name = excluded.name",
    )
    .bind(kind.to_string())
    .bind(key.clone())
    .bind(i(obj, "legacy_id"))
    .bind(i(obj, "rev"))
    .bind(serde_json::to_string(obj)?)
    .bind(name.clone())
    .execute(&mut *conn)
    .await?;
    // newly created or renamed → `names` of affected documents anew
    Ok(old.is_none_or(|o| o != name))
}

async fn refresh_names_where(conn: &mut SqliteConnection, cond: &str, arg: &str) -> Result<()> {
    sqlx::query(&format!("UPDATE document SET names = {NAMES_EXPR} WHERE {cond}")).bind(arg.to_string()).execute(&mut *conn).await?;
    Ok(())
}

async fn refresh_names_for_ref(conn: &mut SqliteConnection, kind: &str, key: &str) -> Result<()> {
    let cond = match kind {
        "correspondent" => "correspondent = ?",
        "document_type" => "document_type = ?",
        "storage_path" => "storage_path = ?",
        "tag" => "key IN (SELECT doc FROM document_tag WHERE tag = ?)",
        _ => return Ok(()),
    };
    refresh_names_where(conn, cond, key).await
}

/// Note texts (for FTS) from the v2 note list.
fn notes_text(notes: &Value) -> String {
    notes
        .as_array()
        .map(|a| a.iter().filter_map(|n| n.get("note").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

async fn upsert_document_on(conn: &mut SqliteConnection, d: &Value, detail: bool, in_selection: bool) -> Result<()> {
    let key = s(d, "uuid").ok_or_else(|| Error::Protocol("document without uuid".into()))?;
    // stored JSON without content/notes (those are in their own columns)
    let mut meta: Map<String, Value> = d.as_object().cloned().unwrap_or_default();
    let content = meta.remove("content");
    let notes = meta.remove("notes");
    meta.remove("sharing");
    let json = serde_json::to_string(&Value::Object(meta))?;
    let (content, notes_txt, notes_json) = if detail {
        (
            content.and_then(|c| c.as_str().map(str::to_string)).or(Some(String::new())),
            Some(notes.as_ref().map(notes_text).unwrap_or_default()),
            Some(serde_json::to_string(&notes.unwrap_or(Value::Array(vec![])))?),
        )
    } else {
        (None, None, None)
    };
    let owner = d.get("owner").and_then(|o| o.get("uuid")).and_then(Value::as_str).map(str::to_string);
    sqlx::query(
        "INSERT INTO document (key, legacy_id, rev, title, created, added, modified, correspondent, document_type, \
         storage_path, owner, asn, mime_type, original_filename, json, content, notes, notes_json, has_detail, \
         in_selection, left_selection) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, \
                 CASE WHEN ?20 = 1 THEN NULL ELSE ?21 END) \
         ON CONFLICT (key) DO UPDATE SET legacy_id = excluded.legacy_id, title = excluded.title, \
           created = excluded.created, added = excluded.added, modified = excluded.modified, \
           correspondent = excluded.correspondent, document_type = excluded.document_type, \
           storage_path = excluded.storage_path, owner = excluded.owner, asn = excluded.asn, \
           mime_type = excluded.mime_type, original_filename = excluded.original_filename, json = excluded.json, \
           content = CASE WHEN ?19 = 1 THEN excluded.content ELSE document.content END, \
           notes = CASE WHEN ?19 = 1 THEN excluded.notes ELSE document.notes END, \
           notes_json = CASE WHEN ?19 = 1 THEN excluded.notes_json ELSE document.notes_json END, \
           has_detail = CASE WHEN ?19 = 1 THEN 1 WHEN document.rev = excluded.rev THEN document.has_detail ELSE 0 END, \
           rev = excluded.rev, \
           in_selection = CASE WHEN ?20 = 1 THEN 1 ELSE document.in_selection END, \
           left_selection = CASE WHEN ?20 = 1 THEN NULL ELSE document.left_selection END",
    )
    .bind(key.clone())
    .bind(i(d, "legacy_id"))
    .bind(i(d, "rev"))
    .bind(s(d, "title").unwrap_or_default())
    .bind(s(d, "created"))
    .bind(s(d, "added"))
    .bind(s(d, "modified"))
    .bind(s(d, "correspondent"))
    .bind(s(d, "document_type"))
    .bind(s(d, "storage_path"))
    .bind(owner)
    .bind(i(d, "asn"))
    .bind(s(d, "mime_type"))
    .bind(s(d, "original_filename"))
    .bind(json)
    .bind(content)
    .bind(notes_txt)
    .bind(notes_json)
    .bind(detail as i64)
    .bind(in_selection as i64)
    .bind(now())
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM document_tag WHERE doc = ?").bind(key.clone()).execute(&mut *conn).await?;
    if let Some(tags) = d.get("tags").and_then(Value::as_array) {
        for t in tags.iter().filter_map(Value::as_str) {
            sqlx::query("INSERT OR IGNORE INTO document_tag (doc, tag) VALUES (?, ?)")
                .bind(key.clone())
                .bind(t.to_string())
                .execute(&mut *conn)
                .await?;
        }
    }
    refresh_names_where(conn, "key = ?", &key).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn doc(uuid: &str, title: &str, tags: &[&str], corr: Option<&str>) -> Value {
        json!({"uuid": uuid, "legacy_id": 1, "rev": 1, "title": title, "created": "2024-01-31",
               "added": "2024-02-01T10:00:00Z", "modified": "2024-02-01T10:00:00Z",
               "correspondent": corr, "document_type": null, "storage_path": null, "tags": tags,
               "custom_fields": [], "asn": null, "owner": null, "page_count": 1, "mime_type": "application/pdf",
               "original_filename": "a.pdf", "notes_count": 0,
               "files": {"original": {"checksum": "x", "size": 10}, "archive": null, "thumbnail": {"etag": null}},
               "access": {"can_view": true, "can_change": true, "can_delete": true}})
    }

    /// Spike proof (ROADMAP P5): FTS5 is active in the bundled libsqlite3 of sqlx-sqlite.
    #[tokio::test]
    async fn fts5_available_in_bundled_sqlite() {
        let r = Replica::open_in_memory().await.unwrap();
        let version: String = sqlx::query_scalar("SELECT sqlite_version()").fetch_one(r.pool()).await.unwrap();
        let opts: Vec<String> = sqlx::query_scalar("PRAGMA compile_options").fetch_all(r.pool()).await.unwrap();
        assert!(opts.iter().any(|o| o == "ENABLE_FTS5"), "compile options: {opts:?}");
        assert!(version.as_str() >= "3.43", "sqlite {version}");
        let ok: i64 = sqlx::query_scalar("SELECT sqlite_compileoption_used('ENABLE_FTS5')").fetch_one(r.pool()).await.unwrap();
        assert_eq!(ok, 1);
    }

    #[tokio::test]
    async fn migrate_idempotent() {
        let r = Replica::open_in_memory().await.unwrap();
        assert_eq!(r.migrate().await.unwrap(), MIGRATIONS.len() as i64);
        let v: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(r.pool()).await.unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
    }

    #[tokio::test]
    async fn document_upsert_names_and_fts() {
        let r = Replica::open_in_memory().await.unwrap();
        r.upsert_object("tag", &json!({"uuid": "t1", "legacy_id": 1, "rev": 1, "name": "Rechnungen"})).await.unwrap();
        r.upsert_object("correspondent", &json!({"uuid": "c1", "legacy_id": 1, "rev": 1, "name": "Müller AG"})).await.unwrap();
        let mut d = doc("d1", "Stromrechnung Januar", &["t1"], Some("c1"));
        d["content"] = json!("Betrag fällig");
        d["notes"] = json!([{"legacy_id": 1, "note": "bezahlt per Überweisung", "created": "2024-02-01T10:00:00Z", "user": null}]);
        r.upsert_documents(&[d], true, true).await.unwrap();
        let hit = |q: &'static str| {
            let pool = r.pool().clone();
            async move { sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fts WHERE fts MATCH ?").bind(q).fetch_one(&pool).await.unwrap() }
        };
        assert_eq!(hit("muller").await, 1, "diacritics removed in names");
        assert_eq!(hit("faellig OR fallig").await, 1);
        assert_eq!(hit("uberweisung").await, 1, "notes indexed");
        assert_eq!(hit("rechnungen").await, 1, "tag names indexed");
        assert_eq!(hit("strom*").await, 1);
        // rename the tag → names anew
        r.upsert_objects("tag", &[json!({"uuid": "t1", "legacy_id": 1, "rev": 2, "name": "Belege"})]).await.unwrap();
        assert_eq!(hit("rechnungen").await, 0);
        assert_eq!(hit("belege").await, 1);
        // metadata update without detail keeps content, resets has_detail on a new rev
        let mut d2 = doc("d1", "Stromrechnung Februar", &[], Some("c1"));
        d2["rev"] = json!(2);
        r.upsert_documents(&[d2], false, true).await.unwrap();
        assert_eq!(hit("fallig").await, 1);
        assert_eq!(hit("februar").await, 1);
        assert_eq!(hit("januar").await, 0);
        assert_eq!(hit("belege").await, 0, "tag removed");
        assert_eq!(r.document_state("d1").await.unwrap(), Some((2, true, false)));
        let full = r.document_full("d1", true, true).await.unwrap().unwrap();
        assert_eq!(full["content"], "Betrag fällig");
        assert_eq!(full["notes"][0]["note"], "bezahlt per Überweisung");
        // deleting cleans FTS
        r.delete_document("d1").await.unwrap();
        assert_eq!(hit("februar").await, 0);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM document_tag").fetch_one(r.pool()).await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn unselected_retention() {
        let r = Replica::open_in_memory().await.unwrap();
        r.upsert_documents(&[doc("a", "A", &[], None), doc("b", "B", &[], None)], false, true).await.unwrap();
        r.mark_unselected("a", "2020-01-01T00:00:00Z").await.unwrap();
        assert_eq!(r.count_documents(true).await.unwrap(), 1);
        // selected again → left_selection gone
        r.upsert_documents(&[doc("a", "A", &[], None)], false, true).await.unwrap();
        assert_eq!(r.count_documents(true).await.unwrap(), 2);
        r.mark_unselected("a", "2020-01-01T00:00:00Z").await.unwrap();
        assert_eq!(r.purge_unselected("2020-01-02T00:00:00Z").await.unwrap(), vec!["a".to_string()]);
        assert_eq!(r.count_documents(false).await.unwrap(), 1);
    }
}
