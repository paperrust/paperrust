//! pl-sync-client – client core for PaperRust Desktop mode 2b/3 (docs/SYNC.md §4/§5/§8,
//! docs/REMOTE-APP.md §5). Contract: docs/contract/pl-sync-client.md.
//!
//! * [`store`] – SQLite replica (sqlx-sqlite, bundled libsqlite3 with FTS5), migrations
//! * [`remote`] – PaperRust counterpart via API v2 (device token, feed, resolve, batch, ids, files)
//! * [`engine`] – reconciliation (initial sync, long poll, full sync, query mode without feed)
//! * [`selection`] – selection + limits, [`files`] – file cache with size limit/eviction
//! * [`query`] – offline list/search in v2 form, [`gateway`] – `prapi://` logic for the shell
//! * [`secret`] – token in the OS keychain, [`tls`] – SPKI pinning
//! * [`wsrelay`] – `/ws/status/` for mode 2b (loopback → server with device token)

pub mod app;
pub mod engine;
pub mod error;
pub mod files;
pub mod filter_rules;
pub mod gateway;
pub mod query;
pub mod remote;
pub mod secret;
pub mod selection;
pub mod store;
pub mod tls;
pub mod v1local;
pub mod wsrelay;

pub use engine::{ProfileConfig, Status, SyncClient, SyncReport};
pub use error::{Error, Result};
pub use gateway::{Gateway, GwRequest, GwResponse};
