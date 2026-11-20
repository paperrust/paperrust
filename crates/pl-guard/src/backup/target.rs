//! Backup target: local folder (also a mounted network share) or S3-compatible storage.
//!
//! Layout (the same for both):
//! ```text
//! config.json                    plaintext: format, public key (age1…), creation time
//! keys/repo.key.age              secret key, encrypted with the passphrase (age-scrypt)
//! objects/ab/abcdef…             one object per file content, name = SHA-256 of the plaintext, age-encrypted
//! snapshots/20261114T021500Z.age directory of a run (gzip + age)
//! ```

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::backup::s3;
use crate::config::Target;

/// Open target.
#[derive(Clone)]
pub enum Store {
    Local(PathBuf),
    S3(s3::Client),
}

fn io(p: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", p.display())
}

impl Store {
    pub fn open(t: &Target) -> Result<Self, String> {
        Ok(match t {
            Target::Local(p) => Store::Local(p.clone()),
            Target::S3(c) => Store::S3(s3::Client::new(c.clone())?),
        })
    }

    fn local(root: &Path, key: &str) -> PathBuf {
        let mut p = root.to_path_buf();
        for part in key.split('/') {
            p.push(part);
        }
        p
    }

    /// Retention period for new objects (S3 with Object Lock only).
    pub fn retain_until(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Store::Local(_) => None,
            Store::S3(c) => c.retain_until(now),
        }
    }

    /// Retention period in days (0 = none).
    pub fn lock_days(&self) -> i64 {
        match self {
            Store::S3(c) if c.config().lock != crate::config::LockMode::Off => c.config().lock_days,
            _ => 0,
        }
    }

    pub async fn put(&self, key: &str, data: Vec<u8>, until: Option<DateTime<Utc>>) -> Result<(), String> {
        match self {
            Store::Local(root) => {
                let p = Self::local(root, key);
                let root = root.clone();
                tokio::task::spawn_blocking(move || write_atomic(&root, &p, &data)).await.map_err(|e| e.to_string())?
            }
            Store::S3(c) => c.put(key, data, until).await.map_err(|e| e.to_string()),
        }
    }

    /// Upload a file (for large objects locally without a detour via memory).
    pub async fn put_file(&self, key: &str, src: &Path, until: Option<DateTime<Utc>>) -> Result<(), String> {
        match self {
            Store::Local(root) => {
                let p = Self::local(root, key);
                let (root, src) = (root.clone(), src.to_path_buf());
                tokio::task::spawn_blocking(move || -> Result<(), String> {
                    if let Some(parent) = p.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
                    }
                    let tmp = tmp_name(&root, &p);
                    std::fs::copy(&src, &tmp).map_err(|e| io(&tmp, e))?;
                    finish_atomic(&tmp, &p)
                })
                .await
                .map_err(|e| e.to_string())?
            }
            Store::S3(c) => {
                // S3 needs Content-MD5 + SHA-256 beforehand; objects are single files
                let data = tokio::fs::read(src).await.map_err(|e| io(src, e))?;
                c.put(key, data, until).await.map_err(|e| e.to_string())
            }
        }
    }

    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match self {
            Store::Local(root) => match tokio::fs::read(Self::local(root, key)).await {
                Ok(b) => Ok(Some(b)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(io(&Self::local(root, key), e)),
            },
            Store::S3(c) => c.get(key).await.map_err(|e| e.to_string()),
        }
    }

    pub async fn exists(&self, key: &str) -> Result<bool, String> {
        match self {
            Store::Local(root) => Ok(tokio::fs::metadata(Self::local(root, key)).await.is_ok()),
            Store::S3(c) => c.exists(key).await.map_err(|e| e.to_string()),
        }
    }

    /// Delete; `Ok(false)` if locked (S3 Object Lock).
    pub async fn delete(&self, key: &str) -> Result<bool, String> {
        match self {
            Store::Local(root) => {
                let p = Self::local(root, key);
                // objects are stored write-protected
                if let Ok(m) = std::fs::metadata(&p) {
                    let mut perm = m.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(false);
                    let _ = std::fs::set_permissions(&p, perm);
                }
                match tokio::fs::remove_file(&p).await {
                    Ok(()) => Ok(true),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
                    Err(e) => Err(io(&p, e)),
                }
            }
            Store::S3(c) => c.delete(key).await.map_err(|e| e.to_string()),
        }
    }

    pub async fn extend_retention(&self, key: &str, until: DateTime<Utc>) -> Result<(), String> {
        match self {
            Store::Local(_) => Ok(()),
            Store::S3(c) => c.extend_retention(key, until).await.map_err(|e| e.to_string()),
        }
    }

    /// Keys under `prefix` (e.g. `snapshots/`), relative to the root of the target.
    pub async fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        match self {
            Store::Local(root) => {
                let root = root.clone();
                let prefix = prefix.to_string();
                tokio::task::spawn_blocking(move || {
                    let dir = Self::local(&root, prefix.trim_end_matches('/'));
                    let mut out = Vec::new();
                    list_rec(&root, &dir, &mut out);
                    out.sort();
                    out
                })
                .await
                .map_err(|e| e.to_string())
            }
            Store::S3(c) => c.list(prefix).await.map_err(|e| e.to_string()),
        }
    }
}

fn list_rec(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            list_rec(root, &p, out);
        } else if !e.file_name().to_string_lossy().ends_with(".paperrust-tmp")
            && let Some(rel) = crate::guard::rel_path(root, &p)
        {
            out.push(rel);
        }
    }
}

fn tmp_name(_root: &Path, p: &Path) -> PathBuf {
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    p.with_file_name(format!(".{name}.{}.paperrust-tmp", rand::random::<u32>()))
}

fn finish_atomic(tmp: &Path, p: &Path) -> Result<(), String> {
    if let Ok(f) = std::fs::File::open(tmp) {
        let _ = f.sync_all();
    }
    std::fs::rename(tmp, p).map_err(|e| io(p, e))?;
    // small hurdle against accidental overwriting (no protection against ransomware, see docs)
    if let Ok(m) = std::fs::metadata(p) {
        let mut perm = m.permissions();
        perm.set_readonly(true);
        let _ = std::fs::set_permissions(p, perm);
    }
    Ok(())
}

fn write_atomic(root: &Path, p: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    // replace existing, write-protected file (e.g. config.json on a repeated init)
    if let Ok(m) = std::fs::metadata(p) {
        let mut perm = m.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        let _ = std::fs::set_permissions(p, perm);
    }
    let tmp = tmp_name(root, p);
    std::fs::write(&tmp, data).map_err(|e| io(&tmp, e))?;
    finish_atomic(&tmp, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_store() {
        let t = tempfile::tempdir().unwrap();
        let s = Store::Local(t.path().to_path_buf());
        s.put("objects/ab/abc", b"x".to_vec(), None).await.unwrap();
        s.put("snapshots/1.age", b"y".to_vec(), None).await.unwrap();
        assert!(s.exists("objects/ab/abc").await.unwrap());
        assert_eq!(s.get("objects/ab/abc").await.unwrap().as_deref(), Some(&b"x"[..]));
        assert_eq!(s.get("nope").await.unwrap(), None);
        assert_eq!(s.list("objects/").await.unwrap(), vec!["objects/ab/abc"]);
        assert!(s.delete("objects/ab/abc").await.unwrap());
        assert!(!s.exists("objects/ab/abc").await.unwrap());
    }
}
