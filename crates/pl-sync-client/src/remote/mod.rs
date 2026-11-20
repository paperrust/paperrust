//! Counterparts of the client core. Today PaperRust (API v2, [`paperrust`]); paperless-ngx (v1,
//! docs/SYNC.md §7) follows in P6 as `ngx.rs`.

pub mod ngx;
pub mod paperrust;

use serde::Deserialize;
use serde_json::Value;

pub use ngx::NgxRemote;
pub use paperrust::PaperRustRemote;

/// `GET /rs/api/v2/server`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ServerInfo {
    pub product: String,
    pub api: i64,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub server_uuid: Option<String>,
    #[serde(default)]
    pub min_cursor: Option<String>,
    #[serde(default)]
    pub perm_epoch: Option<i64>,
}

impl ServerInfo {
    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

/// Result of `POST /auth/devices` (token only in memory, immediately into the keychain).
pub struct DeviceGrant {
    pub token: zeroize::Zeroizing<String>,
    pub device_uuid: String,
    pub device_name: String,
}

impl std::fmt::Debug for DeviceGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceGrant").field("device_uuid", &self.device_uuid).field("token", &"<redacted>").finish()
    }
}

/// An entry of the change feed (docs/SYNC.md §3.2).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Change {
    pub seq: i64,
    pub kind: String,
    pub uuid: String,
    /// `upsert` | `delete`
    pub op: String,
    #[serde(default)]
    pub rev: Option<i64>,
    #[serde(default)]
    pub object: Option<Value>,
}

/// A page of the feed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ChangePage {
    pub changes: Vec<Change>,
    pub next_cursor: String,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub perm_epoch: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct ResolvedSize {
    #[serde(default)]
    pub original: Option<u64>,
    #[serde(default)]
    pub archive: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResolvedDoc {
    pub uuid: String,
    pub rev: i64,
    #[serde(default)]
    pub size: ResolvedSize,
}

/// `POST /sync/resolve`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Resolved {
    pub documents: Vec<ResolvedDoc>,
    #[serde(default)]
    pub as_of_seq: Option<i64>,
}

/// `POST /objects/batch`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Batch {
    pub items: Vec<Value>,
    #[serde(default)]
    pub missing: Vec<String>,
}

/// `GET /sync/ids`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Ids {
    pub items: Vec<(String, i64)>,
    #[serde(default)]
    pub as_of_seq: Option<i64>,
}

/// Raw response (passing through in the online mode of the gateway).
#[derive(Debug, Clone)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: bytes::Bytes,
}
