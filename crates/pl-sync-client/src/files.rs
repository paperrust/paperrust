//! File cache (docs/SYNC.md §4.3): `files/<uuid[0..2]>/<uuid>.<variant>`, variants
//! `original|archive|thumb`. Writing into `*.part` + rename, check against `checksum`
//! (SHA-256) for original/archive, resume via `Range`/`If-Range` (state `partial` with
//! ETag in the table `file`), thumbnails via `If-None-Match`.
//!
//! Size limit: files of the selection (`pinned`) are never evicted; others (e.g. PDFs opened online)
//! by last use (LRU). If a file still does not fit → [`Error::CacheFull`].

use std::path::{Path, PathBuf};
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
    /// fortgesetzt ab Byte (gesamt)
    Resumed(u64, u64),
}

pub struct FileCache {
    root: PathBuf,
    max_bytes: AtomicU64,
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
        FileCache { root: root.into(), max_bytes: AtomicU64::new(max_bytes) }
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

    /// Make room: delete unselected files by LRU.
    pub async fn ensure_space(&self, replica: &Replica, need: u64) -> Result<()> {
        let max = self.max_bytes();
        let mut used = replica.cache_bytes().await?.max(0) as u64;
        if used + need <= max {
            return Ok(());
        }
        for f in replica.evictable().await? {
            if used + need <= max {
                break;
            }
            let _ = tokio::fs::remove_file(&f.path).await;
            let _ = tokio::fs::remove_file(Self::part_path(Path::new(&f.path))).await;
            replica.delete_file(&f.doc, &f.variant).await?;
            used = used.saturating_sub(f.size.max(0) as u64);
        }
        if used + need <= max { Ok(()) } else { Err(Error::CacheFull) }
    }

    /// Delete all files of a document.
    pub async fn remove_doc(&self, replica: &Replica, doc: &str) -> Result<()> {
        for f in replica.files_of(doc).await? {
            let _ = tokio::fs::remove_file(&f.path).await;
            let _ = tokio::fs::remove_file(Self::part_path(Path::new(&f.path))).await;
            replica.delete_file(&f.doc, &f.variant).await?;
        }
        Ok(())
    }

    /// Delete paths (after `Replica::delete_document`).
    pub async fn remove_paths(paths: &[String]) {
        for p in paths {
            let _ = tokio::fs::remove_file(p).await;
            let _ = tokio::fs::remove_file(Self::part_path(Path::new(p))).await;
        }
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
                (None, _) => return Ok(Fetched::Unchanged),
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
        let len = resp.content_length().unwrap_or(0);
        let resumed_from = if status == 206 { resume.as_ref().map(|(n, _)| *n) } else { None };
        // old state of the entry does not count twice
        let old_size = row.as_ref().map(|r| r.size.max(0) as u64).unwrap_or(0);
        self.ensure_space(replica, (len + resumed_from.unwrap_or(0)).saturating_sub(old_size)).await?;

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
            size: (len + resumed_from.unwrap_or(0)) as i64,
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
        let mut written = resumed_from.unwrap_or(0);
        loop {
            let chunk = resp.chunk().await.map_err(crate::remote::paperrust::map_reqwest)?;
            let Some(chunk) = chunk else { break };
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
            let _ = tokio::fs::remove_file(&part).await;
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
}
