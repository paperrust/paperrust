// Errors of the index writer. Never contain document content.

#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("tantivy: {0}")]
    Tantivy(#[from] tantivy::TantivyError),
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// upstream `SearchIndexLockError` (".tantivy.lock" not acquired).
    #[error("Could not acquire index lock after {attempts} attempts (timeout={timeout_secs}s each)")]
    LockExhausted { attempts: u32, timeout_secs: f64 },
    /// A value tantivy-py would reject as well (e.g. a date out of range, a negative u64).
    #[error("invalid value: {0}")]
    InvalidValue(String),
    #[error("index directory does not exist: {0}")]
    MissingIndexDir(String),
}

pub type Result<T> = std::result::Result<T, IndexerError>;

impl IndexerError {
    /// Transient file/IO error while writing (BEFUNDE B8): on Windows e.g.
    /// "Access denied (os error 5)" while a reader, the virus scanner or Windows search
    /// holds an index file open or a deleted file is still "pending deletion", or
    /// `FileAlreadyExists(….del)` after a failed commit. A new attempt with a fresh
    /// writer usually succeeds. Lock held (`LockExhausted`) does not count here (separate rule).
    pub fn is_transient(&self) -> bool {
        match self {
            IndexerError::Io(_) => true,
            IndexerError::Tantivy(t) => matches!(
                t,
                tantivy::TantivyError::IoError(_)
                    | tantivy::TantivyError::OpenWriteError(_)
                    | tantivy::TantivyError::OpenReadError(_)
                    | tantivy::TantivyError::LockFailure(..)
                    | tantivy::TantivyError::ErrorInThread(_)
            ),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_errors() {
        assert!(IndexerError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied)).is_transient());
        let t = tantivy::TantivyError::IoError(std::sync::Arc::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        assert!(IndexerError::Tantivy(t).is_transient());
        assert!(!IndexerError::LockExhausted { attempts: 4, timeout_secs: 10.0 }.is_transient());
        assert!(!IndexerError::InvalidValue("x".into()).is_transient());
        assert!(!IndexerError::Tantivy(tantivy::TantivyError::SchemaError("x".into())).is_transient());
    }
}
