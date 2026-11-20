// upstream: django/http/response.py (FileResponse: streamed body, Content-Length from the file)
//           src/documents/models.py@v3.4.2 (source_path / archive_path / thumbnail_path)
//
// Streaming file bodies with constant memory, plus the path-traversal guard.

use std::path::{Path, PathBuf};

use axum::body::Body;
use futures::stream;
use tokio::io::AsyncReadExt;

const CHUNK: usize = 64 * 1024;

/// Join a DB-provided relative path onto a base directory, rejecting absolute paths, drive
/// prefixes and any `..` that would leave the base (lexical check, so symlinked media directories
/// still work). `\` is a separator only on Windows, like `pathlib` – shared with pl-documents-write
/// and pl-consumer (security review final C-N1).
pub use pl_core::safepath::safe_join;

#[derive(Debug)]
pub enum OpenError {
    /// FileNotFoundError (also: path rejected by [`safe_join`]).
    NotFound,
    /// Any other OSError (e.g. IsADirectoryError) – a 500 upstream.
    Other(std::io::Error),
}

pub struct OpenedFile {
    pub file: tokio::fs::File,
    pub len: u64,
    pub modified: Option<std::time::SystemTime>,
}

pub async fn open(path: Option<&Path>) -> Result<OpenedFile, OpenError> {
    let Some(path) = path else {
        return Err(OpenError::NotFound);
    };
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(OpenError::NotFound),
        Err(e) => return Err(OpenError::Other(e)),
    };
    let meta = file.metadata().await.map_err(OpenError::Other)?;
    if meta.is_dir() {
        return Err(OpenError::Other(std::io::Error::other("is a directory")));
    }
    Ok(OpenedFile {
        file,
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// Removes a temporary file once the body that streams it is dropped.
pub struct TempGuard(pub Option<PathBuf>);

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Stream a file in fixed-size chunks.
pub fn file_body(file: tokio::fs::File, guard: Option<TempGuard>) -> Body {
    let s = stream::unfold(
        (file, guard, false),
        |(mut file, guard, done)| async move {
            if done {
                return None;
            }
            let mut buf = vec![0u8; CHUNK];
            match file.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), (file, guard, false)))
                }
                Err(e) => Some((Err(e), (file, guard, true))),
            }
        },
    );
    Body::from_stream(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_rejected() {
        let base = Path::new("/media/documents/originals");
        assert!(safe_join(base, "../../etc/passwd").is_none());
        assert!(safe_join(base, "/etc/passwd").is_none());
        assert!(safe_join(base, "a/../../x").is_none());
        assert_eq!(
            safe_join(base, "2020/./a/../b.pdf").unwrap(),
            base.join("2020").join("b.pdf")
        );
        assert_eq!(safe_join(base, "0000001.pdf").unwrap(), base.join("0000001.pdf"));
    }

    /// C-N1: download/bundle use the same function as the write path and the worker – `\` is exactly
    /// on Windows a separator (legacy names of the Windows reference stay findable), on Linux a
    /// normal character (no reinterpretation `a\..\b.pdf` → `b.pdf`).
    #[test]
    fn backslash_like_pathlib() {
        let base = Path::new("/media/documents/originals");
        if cfg!(windows) {
            assert_eq!(safe_join(base, "ACME Corp\\2019\\a.pdf").unwrap(), base.join("ACME Corp").join("2019").join("a.pdf"));
            assert!(safe_join(base, "..\\..\\x").is_none());
            assert!(safe_join(base, "C:\\x").is_none());
        } else {
            assert_eq!(safe_join(base, "a\\..\\b.pdf").unwrap(), base.join("a\\..\\b.pdf"));
        }
        assert_eq!(safe_join(base, "").unwrap(), base.to_path_buf());
    }
}
