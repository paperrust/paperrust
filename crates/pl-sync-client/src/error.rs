//! Errors of the client core. Texts without secrets (never token/password in the error or log).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// Local database.
    #[error("local database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    /// Server not reachable (connection, timeout) → offline.
    #[error("server unreachable: {0}")]
    Offline(String),
    /// TLS: certificate does not match the confirmed SPKI pin (hard error, docs/SYNC.md §8).
    #[error("certificate does not match the pinned fingerprint")]
    PinMismatch,
    /// TLS error otherwise (unknown CA, name, expired …).
    #[error("tls: {0}")]
    Tls(String),
    /// 401: no/invalid/revoked token → log in again.
    #[error("authentication required")]
    Unauthenticated,
    /// Error response of the server in v2 format.
    #[error("server error {status} {code}: {message}")]
    Api { status: u16, code: String, message: String },
    /// 410 `cursor_expired` (or foreign server_uuid) → full sync.
    #[error("cursor expired")]
    CursorExpired,
    /// Response does not match the contract.
    #[error("protocol: {0}")]
    Protocol(String),
    /// Selection exceeds the limits (docs/SYNC.md §4.4).
    #[error("selection exceeds the limits: {0}")]
    LimitExceeded(String),
    /// File cache full (only unselected files can be evicted).
    #[error("file cache full")]
    CacheFull,
    /// File does not match the checksum.
    #[error("checksum mismatch for {0}")]
    Checksum(String),
    /// Keychain.
    #[error("keychain: {0}")]
    Secret(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("not found")]
    NotFound,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Network/reachability error (→ switch offline, retry later).
    pub fn is_offline(&self) -> bool {
        matches!(self, Error::Offline(_))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Protocol(e.to_string())
    }
}
