//! File cache (docs/SYNC.md §4.3): `files/<uuid[0..2]>/<uuid>.<variant>`, variants
//! `original|archive|thumb`. Writing into `*.part` + rename, check against `checksum`
//! (SHA-256) for original/archive, resume via `Range`/`If-Range` (state `partial` with
//! ETag in the table `file`), thumbnails via `If-None-Match`.
//!
//! Size limit: files of the selection (`pinned`) are never evicted; others (e.g. PDFs opened online)
//! by last use (LRU). If a file still does not fit → [`Error::CacheFull`]. Parallel downloads
//! reserve their bytes in memory ([`Reservation`]) under one gate, so they cannot all pass the check
//! at once; a download that writes more than reserved (unknown or wrong `Content-Length`) grows its
//! reservation or is aborted (partial file removed).
//!
//! Deleting: files are deleted **before** their rows. A file that cannot be deleted (e.g. locked on
//! Windows) is recorded in `pending_delete` and retried by the housekeeping, which also sweeps
//! orphan files without a row ([`FileCache::retry_pending`], [`FileCache::sweep_orphans`]).

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{Error, Result};
use crate::remote::{NgxRemote, PaperRustRemote};

/// Where the files come from (PaperRust v2 or paperless-ngx v1).
#[derive(Clone, Copy)]
pub enum Source<'a> {
    PaperRust(&'a PaperRustRemote),
    Ngx(&'a NgxRemote),
}

impl Source<'_> {
    async fn file(
        &self,
        doc: &Value,
        key: &str,
        variant: &str,
        inm: Option<&str>,
        resume: Option<(u64, &str)>,
    ) -> Result<reqwest::Response> {
        match self {
            Source::PaperRust(r) => r.file(key, server_variant(variant), inm, resume).await,
            Source::Ngx(n) => {
                let id = doc["legacy_id"].as_i64().ok_or_else(|| Error::Protocol("document without legacy_id".into()))?;
                n.file(id, server_variant(variant), inm).await
            }
        }
    }
}
use crate::store::{FileRow, Replica};

/// Result of a fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetched {
    /// was already up to date (without request or 304)
    Unchanged,
    /// newly loaded (bytes)
    Downloaded(u64),
    /// resumed from byte (total)
    Resumed(u64, u64),
    /// the server has no such file any more (thumbnail `etag: null`); the local copy was removed
    Removed,
}

/// Minimum step when a download outgrows its reservation (fewer gate round trips).
const GROW_STEP: u64 = 256 * 1024;

pub struct FileCache {
    root: PathBuf,
    max_bytes: AtomicU64,
    /// Bytes promised to running downloads that are not yet in `file.size`.
    reserved: Arc<std::sync::Mutex<u64>>,
    /// Serializes check + reservation (+ eviction), so parallel downloads cannot all see free space.
    gate: tokio::sync::Mutex<()>,
}

/// Space reserved for one running download; released on drop.
#[derive(Debug)]
pub struct Reservation {
    bytes: u64,
    pool: Arc<std::sync::Mutex<u64>>,
}

impl Reservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut g = self.pool.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *g = g.saturating_sub(self.bytes);
    }
}

/// Delete one path; "not found" counts as deleted.
async fn delete_path(p: &Path) -> bool {
    match tokio::fs::remove_file(p).await {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            tracing::warn!(path = %p.display(), "could not delete cache file (kept as pending delete): {e}");
            false
        }
    }
}

/// Size on disk (0 if unknown).
async fn disk_size(p: &Path) -> i64 {
    tokio::fs::metadata(p).await.map(|m| m.len() as i64).unwrap_or(0)
}

/// Server name of the variant.
pub fn server_variant(v: &str) -> &str {
    if v == "thumb" { "thumbnail" } else { v }
}

/// Expected checksum according to the v2 document.
pub fn expected_checksum(doc: &Value, variant: &str) -> Option<String> {
    match variant {
        "original" => doc["files"]["original"]["checksum"].as_str().map(str::to_string),
        "archive" => doc["files"]["archive"]["checksum"].as_str().map(str::to_string),
        _ => None,
    }
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl FileCache {
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> Self {
        FileCache {
            root: root.into(),
            max_bytes: AtomicU64::new(max_bytes),
            reserved: Arc::new(std::sync::Mutex::new(0)),
            gate: tokio::sync::Mutex::new(()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn set_max_bytes(&self, n: u64) {
        self.max_bytes.store(n, Ordering::Relaxed);
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// Target path (UUIDs are hex + `-`; other characters are replaced, no path escape possible).
    pub fn path_for(&self, doc: &str, variant: &str) -> PathBuf {
        let safe: String = doc.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        let shard: String = safe.chars().take(2).collect();
        self.root.join(if shard.is_empty() { "__".into() } else { shard }).join(format!("{safe}.{variant}"))
    }

    fn part_path(p: &Path) -> PathBuf {
        let mut s = p.as_os_str().to_owned();
        s.push(".part");
        PathBuf::from(s)
    }

    /// Bytes currently reserved by running downloads.
    pub fn reserved_bytes(&self) -> u64 {
        *self.reserved.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Make room for `need` more bytes (caller holds `gate`): used = rows + pending deletes +
    /// reservations; delete unselected files by LRU.
    async fn make_room(&self, replica: &Replica, need: u64) -> Result<()> {
        let max = self.max_bytes();
        let mut used = (replica.cache_bytes().await?.max(0) as u64).saturating_add(self.reserved_bytes());
        if used.saturating_add(need) <= max {
            return Ok(());
        }
        for f in replica.evictable().await? {
            if used.saturating_add(need) <= max {
                break;
            }
            // a file that cannot be deleted stays on disk (pending delete) and keeps counting
            if self.drop_file(replica, &f).await? {
                used = used.saturating_sub(f.size.max(0) as u64);
            }
        }
        if used.saturating_add(need) <= max { Ok(()) } else { Err(Error::CacheFull) }
    }

    /// Make room: delete unselected files by LRU (without reserving).
    pub async fn ensure_space(&self, replica: &Replica, need: u64) -> Result<()> {
        let _g = self.gate.lock().await;
        self.make_room(replica, need).await
    }

    /// Make room and reserve `need` bytes for a download.
    pub async fn reserve(&self, replica: &Replica, need: u64) -> Result<Reservation> {
        let _g = self.gate.lock().await;
        self.make_room(replica, need).await?;
        let mut r = self.reserved.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *r = r.saturating_add(need);
        Ok(Reservation { bytes: need, pool: self.reserved.clone() })
    }

    /// Grow a reservation by `extra` bytes (download larger than announced).
    pub async fn grow(&self, replica: &Replica, res: &mut Reservation, extra: u64) -> Result<()> {
        let _g = self.gate.lock().await;
        self.make_room(replica, extra).await?;
        let mut r = self.reserved.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *r = r.saturating_add(extra);
        res.bytes = res.bytes.saturating_add(extra);
        Ok(())
    }

    /// Delete a path and its `.part`; returns the paths that could not be deleted with their size
    /// (`known` = size of the row, used when the size on disk cannot be read).
    async fn delete_with_part(path: &str, known: i64) -> Vec<(String, i64)> {
        let mut failed = Vec::new();
        let p = Path::new(path);
        let part = Self::part_path(p);
        for (q, fallback) in [(p, known), (part.as_path(), 0)] {
            if !delete_path(q).await {
                failed.push((q.to_string_lossy().into_owned(), disk_size(q).await.max(fallback)));
            }
        }
        failed
    }

    /// Delete one cached file, then its row. Returns `true` if the file is gone from disk; otherwise
    /// the row is replaced by a `pending_delete` record (retried by the housekeeping).
    pub async fn drop_file(&self, replica: &Replica, f: &FileRow) -> Result<bool> {
        let failed = Self::delete_with_part(&f.path, f.size).await;
        replica.delete_file_keep_pending(&f.doc, &f.variant, &failed).await?;
        Ok(failed.is_empty())
    }

    /// Delete all files of a document (metadata stays, e.g. deselected). `true` = all gone from disk.
    pub async fn remove_doc(&self, replica: &Replica, doc: &str) -> Result<bool> {
        let mut all = true;
        for f in replica.files_of(doc).await? {
            all &= self.drop_file(replica, &f).await?;
        }
        Ok(all)
    }

    /// Remove a document completely (deleted, permission revoked, retention over): files first, then
    /// the rows; files that cannot be deleted stay recorded as `pending_delete`. `true` = all files
    /// gone from disk (only then the caller counts the document as removed).
    pub async fn purge_document(&self, replica: &Replica, key: &str) -> Result<bool> {
        let mut failed = Vec::new();
        for f in replica.files_of(key).await? {
            failed.extend(Self::delete_with_part(&f.path, f.size).await);
        }
        replica.delete_document(key, &failed).await?;
        Ok(failed.is_empty())
    }

    /// Retry the pending deletions. Returns how many are still pending.
    pub async fn retry_pending(&self, replica: &Replica) -> Result<u64> {
        let mut left = 0;
        for p in replica.pending_deletes().await? {
            let path = Path::new(&p);
            if delete_path(path).await {
                replica.clear_pending_delete(&p).await?;
            } else {
                left += 1;
                replica.add_pending_deletes(None, &[(p.clone(), disk_size(path).await)]).await?;
            }
        }
        Ok(left)
    }

    /// Delete files in the cache folder (`<root>/<shard>/<file>`) that have no row and no pending
    /// record (e.g. left over after a crash or an older client version). Returns the number deleted.
    pub async fn sweep_orphans(&self, replica: &Replica) -> Result<u64> {
        // list first, then load the rows: a download inserts its row before it creates the file,
        // so every file seen here that belongs to a download is covered by the row snapshot below
        let mut found = Vec::new();
        let mut shards = match tokio::fs::read_dir(&self.root).await {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        while let Some(shard) = shards.next_entry().await? {
            if !shard.file_type().await?.is_dir() {
                continue;
            }
            let mut dir = tokio::fs::read_dir(shard.path()).await?;
            while let Some(f) = dir.next_entry().await? {
                if f.file_type().await?.is_file() {
                    found.push(f.path());
                }
            }
        }
        if found.is_empty() {
            return Ok(0);
        }
        let mut known: HashSet<OsString> = HashSet::new();
        for p in replica.file_paths().await? {
            if let Some(n) = Path::new(&p).file_name() {
                let mut part = n.to_owned();
                part.push(".part");
                known.insert(n.to_owned());
                known.insert(part);
            }
        }
        let pending: HashSet<PathBuf> = replica.pending_deletes().await?.into_iter().map(PathBuf::from).collect();
        let mut removed = 0;
        for f in found {
            let Some(name) = f.file_name() else { continue };
            if known.contains(name) || pending.contains(&f) {
                continue;
            }
            if delete_path(&f).await {
                removed += 1;
            } else {
                replica.add_pending_deletes(None, &[(f.to_string_lossy().into_owned(), disk_size(&f).await)]).await?;
            }
        }
        Ok(removed)
    }

    /// Local, complete file (for offline opening); sets `last_access`.
    pub async fn local(&self, replica: &Replica, doc: &str, variant: &str) -> Result<Option<FileRow>> {
        match replica.file(doc, variant).await? {
            Some(f) if f.state == "complete" && Path::new(&f.path).is_file() => {
                replica.touch_file(doc, variant).await?;
                Ok(Some(f))
            }
            _ => Ok(None),
        }
    }

    /// Load/update a file. `doc` = stored v2 document (for the checksum).
    pub async fn fetch(&self, replica: &Replica, remote: Source<'_>, doc: &Value, variant: &str, pinned: bool) -> Result<Fetched> {
        let key = doc["uuid"].as_str().ok_or_else(|| Error::Protocol("document without uuid".into()))?.to_string();
        let target = self.path_for(&key, variant);
        let part = Self::part_path(&target);
        let expected = expected_checksum(doc, variant);
        let row = replica.file(&key, variant).await?;

        // Thumbnail: server reports none (etag null) → no request; same ETag as in the
        // cache → up to date without request
        if variant == "thumb" {
            let server_etag = doc["files"]["thumbnail"]["etag"].as_str();
            match (server_etag, &row) {
                // thumbnail gone on the server → do not keep showing the old one
                (None, Some(r)) => {
                    self.drop_file(replica, r).await?;
                    return Ok(Fetched::Removed);
                }
                (None, None) => {
                    if target.is_file() {
                        if !delete_path(&target).await {
                            let size = disk_size(&target).await;
                            replica.add_pending_deletes(Some(&key), &[(target.to_string_lossy().into_owned(), size)]).await?;
                        }
                        return Ok(Fetched::Removed);
                    }
                    return Ok(Fetched::Unchanged);
                }
                (Some(e), Some(r))
                    if r.state == "complete" && (r.etag.as_deref() == Some(e) || r.checksum.as_deref() == Some(e)) && target.is_file() =>
                {
                    return Ok(Fetched::Unchanged);
                }
                _ => {}
            }
        }
        // present and up to date?
        let mut if_none_match = None;
        if let Some(r) = &row
            && r.state == "complete"
            && target.is_file()
        {
            match (&expected, &r.checksum) {
                (Some(e), Some(c)) if e == c => {
                    if r.pinned != pinned && pinned {
                        replica.set_files_pinned(&key, true).await?;
                    }
                    return Ok(Fetched::Unchanged);
                }
                (None, _) => if_none_match = r.etag.clone(),
                _ => {}
            }
        }
        // resume?
        let mut resume: Option<(u64, String)> = None;
        if let Some(r) = &row
            && r.state == "partial"
            && let Some(etag) = r.etag.clone().filter(|e| !e.starts_with("W/"))
            && let Ok(m) = tokio::fs::metadata(&part).await
            && m.len() > 0
        {
            resume = Some((m.len(), etag));
        }

        let resp = remote.file(doc, &key, variant, if_none_match.as_deref(), resume.as_ref().map(|(n, e)| (*n, e.as_str()))).await?;
        let status = resp.status().as_u16();
        if status == 304 {
            replica.touch_file(&key, variant).await?;
            return Ok(Fetched::Unchanged);
        }
        let etag = resp.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).map(str::to_string);
        let resumed_from = if status == 206 { resume.as_ref().map(|(n, _)| *n) } else { None };
        // Reserve the announced new bytes (unknown length → 0, grown while writing). The partial row
        // below only counts the bytes already on disk, so nothing is counted twice; the old row
        // (if any) is replaced by it.
        let mut reservation = self.reserve(replica, resp.content_length().unwrap_or(0)).await?;

        if let Some(dir) = target.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        // record partial state (resume after abort)
        let mut entry = FileRow {
            doc: key.clone(),
            variant: variant.to_string(),
            path: target.to_string_lossy().into_owned(),
            etag: etag.clone(),
            checksum: None,
            size: resumed_from.unwrap_or(0) as i64,
            state: "partial".into(),
            pinned,
        };
        replica.put_file(&entry).await?;

        let mut hasher = Sha256::new();
        let mut file = if let Some(start) = resumed_from {
            // existing bytes into the checksum
            let mut f = tokio::fs::File::open(&part).await?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = f.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            let f = tokio::fs::OpenOptions::new().append(true).open(&part).await?;
            debug_assert_eq!(f.metadata().await?.len(), start);
            f
        } else {
            tokio::fs::File::create(&part).await?
        };
        let mut resp = resp;
        let start = resumed_from.unwrap_or(0);
        let mut written = start;
        loop {
            let chunk = resp.chunk().await.map_err(crate::remote::paperrust::map_reqwest)?;
            let Some(chunk) = chunk else { break };
            let new_total = (written - start).saturating_add(chunk.len() as u64);
            if new_total > reservation.bytes() {
                let over = new_total - reservation.bytes();
                let grown = match self.grow(replica, &mut reservation, over.max(GROW_STEP)).await {
                    Err(Error::CacheFull) if over < GROW_STEP => self.grow(replica, &mut reservation, over).await,
                    other => other,
                };
                if let Err(e) = grown {
                    // does not fit: abort and remove the partial file (no resume of a file that
                    // cannot be stored anyway)
                    drop(file);
                    if !delete_path(&part).await {
                        replica.add_pending_deletes(Some(&key), &[(part.to_string_lossy().into_owned(), disk_size(&part).await)]).await?;
                    }
                    replica.delete_file(&key, variant).await?;
                    return Err(e);
                }
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        let sum: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if let Some(e) = expected.as_deref().filter(|e| is_sha256(e))
            && !e.eq_ignore_ascii_case(&sum)
        {
            if !delete_path(&part).await {
                replica.add_pending_deletes(Some(&key), &[(part.to_string_lossy().into_owned(), disk_size(&part).await)]).await?;
            }
            replica.delete_file(&key, variant).await?;
            return Err(Error::Checksum(format!("{key}.{variant}")));
        }
        // replace (Windows: remove the target first)
        let _ = tokio::fs::remove_file(&target).await;
        tokio::fs::rename(&part, &target).await?;
        entry.state = "complete".into();
        entry.size = written as i64;
        // Thumbnail: remember the document's identifier (saves the request on the next run)
        entry.checksum = if variant == "thumb" {
            doc["files"]["thumbnail"]["etag"].as_str().map(str::to_string).or(Some(sum))
        } else {
            expected.or(Some(sum))
        };
        replica.put_file(&entry).await?;
        // the row now counts the bytes → release the reservation
        drop(reservation);
        Ok(match resumed_from {
            Some(start) => Fetched::Resumed(start, written),
            None => Fetched::Downloaded(written),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_sharded_and_safe() {
        let c = FileCache::new("/cache", 10);
        let p = c.path_for("ab12-x", "original");
        assert!(p.ends_with("ab/ab12-x.original"));
        let p = c.path_for("../../etc", "thumb");
        assert!(!p.to_string_lossy().contains(".."), "{p:?}");
        assert_eq!(server_variant("thumb"), "thumbnail");
        assert_eq!(server_variant("archive"), "archive");
    }

    #[tokio::test]
    async fn eviction_spares_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let r = Replica::open_in_memory().await.unwrap();
        let c = FileCache::new(dir.path(), 100);
        for (doc, size, pinned, at) in [("a", 40, true, "1"), ("b", 30, false, "2"), ("c", 20, false, "3")] {
            let p = c.path_for(doc, "original");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![0u8; size]).unwrap();
            let row = FileRow {
                doc: doc.into(),
                variant: "original".into(),
                path: p.to_string_lossy().into_owned(),
                etag: None,
                checksum: None,
                size: size as i64,
                state: "complete".into(),
                pinned,
            };
            r.put_file(&row).await.unwrap();
            sqlx::query("UPDATE file SET last_access = ? WHERE doc = ?")
                .bind(at.to_string())
                .bind(doc.to_string())
                .execute(r.pool())
                .await
                .unwrap();
        }
        // 90 used, 30 needed → b (oldest, not selected) is evicted
        c.ensure_space(&r, 30).await.unwrap();
        assert!(r.file("b", "original").await.unwrap().is_none());
        assert!(!c.path_for("b", "original").exists());
        assert!(r.file("c", "original").await.unwrap().is_some());
        // 60 used; 70 needed → c is evicted, a (selected) stays → does not fit
        assert!(matches!(c.ensure_space(&r, 70).await, Err(Error::CacheFull)));
        assert!(r.file("a", "original").await.unwrap().is_some());
        assert!(c.local(&r, "a", "original").await.unwrap().is_some());
        c.remove_doc(&r, "a").await.unwrap();
        assert!(c.local(&r, "a", "original").await.unwrap().is_none());
    }

    /// Row + file (`undeletable`: a non-empty directory at the path – `remove_file` fails on every
    /// OS, like a file locked by a viewer on Windows).
    async fn put(r: &Replica, c: &FileCache, doc: &str, variant: &str, size: usize, undeletable: bool) -> PathBuf {
        let p = c.path_for(doc, variant);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        if undeletable {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("x"), vec![1u8; size]).unwrap();
        } else {
            std::fs::write(&p, vec![1u8; size]).unwrap();
        }
        let row = FileRow {
            doc: doc.into(),
            variant: variant.into(),
            path: p.to_string_lossy().into_owned(),
            etag: Some("W/\"1\"".into()),
            checksum: Some("W/\"1\"".into()),
            size: size as i64,
            state: "complete".into(),
            pinned: true,
        };
        r.put_file(&row).await.unwrap();
        p
    }

    /// Regression: the rows were deleted first and delete errors ignored → a locked file stayed on
    /// disk without any record. Now: files first, failures kept as `pending_delete`, retried.
    #[tokio::test]
    async fn purge_keeps_pending_record_for_undeletable_file() {
        let dir = tempfile::tempdir().unwrap();
        let r = Replica::open_in_memory().await.unwrap();
        let c = FileCache::new(dir.path(), 1_000_000);
        r.upsert_documents(&[serde_json::json!({"uuid": "d1", "title": "x", "tags": []})], false, true).await.unwrap();
        let locked = put(&r, &c, "d1", "original", 50, true).await;
        let thumb = put(&r, &c, "d1", "thumb", 10, false).await;
        assert!(!c.purge_document(&r, "d1").await.unwrap(), "not everything could be deleted");
        assert!(r.document_state("d1").await.unwrap().is_none(), "metadata removed");
        assert!(r.files_of("d1").await.unwrap().is_empty());
        assert!(!thumb.exists());
        assert_eq!(r.pending_deletes().await.unwrap(), vec![locked.to_string_lossy().into_owned()]);
        assert!(r.cache_bytes().await.unwrap() > 0, "pending file still counts toward the cache");
        // still locked → stays pending
        assert_eq!(c.retry_pending(&r).await.unwrap(), 1);
        // unlocked → housekeeping deletes it and drops the record
        std::fs::remove_dir_all(&locked).unwrap();
        std::fs::write(&locked, b"secret").unwrap();
        assert_eq!(c.retry_pending(&r).await.unwrap(), 0);
        assert!(!locked.exists());
        assert!(r.pending_deletes().await.unwrap().is_empty());
        assert_eq!(r.cache_bytes().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn remove_doc_and_eviction_keep_pending_record() {
        let dir = tempfile::tempdir().unwrap();
        let r = Replica::open_in_memory().await.unwrap();
        let c = FileCache::new(dir.path(), 100);
        let locked = put(&r, &c, "d1", "archive", 60, true).await;
        assert!(!c.remove_doc(&r, "d1").await.unwrap());
        assert!(r.files_of("d1").await.unwrap().is_empty());
        assert_eq!(r.pending_deletes().await.unwrap(), vec![locked.to_string_lossy().into_owned()]);
        // the undeletable file still occupies 60 of 100 bytes
        assert!(matches!(c.ensure_space(&r, 50).await, Err(Error::CacheFull)));
    }

    #[tokio::test]
    async fn orphan_sweep_deletes_files_without_row() {
        let dir = tempfile::tempdir().unwrap();
        let r = Replica::open_in_memory().await.unwrap();
        let c = FileCache::new(dir.path(), 1_000_000);
        let kept = put(&r, &c, "ab01", "original", 5, false).await;
        let mut part = kept.clone().into_os_string();
        part.push(".part");
        std::fs::write(&part, b"in progress").unwrap();
        let orphan = c.path_for("ab02", "original");
        std::fs::write(&orphan, b"left behind").unwrap();
        let orphan2 = c.path_for("cd03", "thumb");
        std::fs::create_dir_all(orphan2.parent().unwrap()).unwrap();
        std::fs::write(&orphan2, b"left behind").unwrap();
        assert_eq!(c.sweep_orphans(&r).await.unwrap(), 2);
        assert!(kept.exists());
        assert!(Path::new(&part).exists(), "part of a known row stays");
        assert!(!orphan.exists());
        assert!(!orphan2.exists());
        // empty / missing cache folder
        let c2 = FileCache::new(dir.path().join("missing"), 10);
        assert_eq!(c2.sweep_orphans(&r).await.unwrap(), 0);
    }

    /// Regression: parallel downloads each checked the free space once and all passed.
    #[tokio::test]
    async fn reservations_are_shared() {
        let dir = tempfile::tempdir().unwrap();
        let r = Replica::open_in_memory().await.unwrap();
        let c = FileCache::new(dir.path(), 100);
        let a = c.reserve(&r, 60).await.unwrap();
        assert!(matches!(c.reserve(&r, 60).await, Err(Error::CacheFull)), "second download must not fit");
        assert!(matches!(c.ensure_space(&r, 41).await, Err(Error::CacheFull)));
        let mut b = c.reserve(&r, 30).await.unwrap();
        assert!(matches!(c.grow(&r, &mut b, 20).await, Err(Error::CacheFull)));
        c.grow(&r, &mut b, 10).await.unwrap();
        assert_eq!(c.reserved_bytes(), 100);
        drop(a);
        assert_eq!(c.reserved_bytes(), 40);
        drop(b);
        assert_eq!(c.reserved_bytes(), 0);
        c.reserve(&r, 100).await.unwrap();
    }

    /// Regression: `files.thumbnail.etag = null` kept the old preview.
    #[tokio::test]
    async fn null_thumbnail_etag_removes_local_thumb() {
        let dir = tempfile::tempdir().unwrap();
        let r = Replica::open_in_memory().await.unwrap();
        let c = FileCache::new(dir.path(), 1_000_000);
        let p = put(&r, &c, "d1", "thumb", 10, false).await;
        // no request is made for etag null (port 9: nothing listens)
        let remote = PaperRustRemote::new("http://127.0.0.1:9/", &crate::tls::TlsOptions::default()).unwrap();
        let doc = serde_json::json!({"uuid": "d1", "files": {"original": null, "archive": null, "thumbnail": {"etag": null}}});
        let got = c.fetch(&r, Source::PaperRust(&remote), &doc, "thumb", true).await.unwrap();
        assert_eq!(got, Fetched::Removed);
        assert!(r.file("d1", "thumb").await.unwrap().is_none());
        assert!(!p.exists());
        // nothing local → unchanged
        assert_eq!(c.fetch(&r, Source::PaperRust(&remote), &doc, "thumb", true).await.unwrap(), Fetched::Unchanged);
    }
}
