//! pl-indexer – tantivy WRITER in the same index format as upstream (tantivy-py 0.26.2,
//! tantivy 0.26.x). Worker building block: add/update/delete a document, rebuild from
//! the DB, lock compatible with the Python writer. See docs/contract/pl-indexer.md.
//!
//! upstream: src/documents/search/_backend.py@v3.4.2 (WriteBatch, TantivyBackend._build_tantivy_doc,
//! add_or_update, remove, rebuild, _DocumentViewerStream), _schema.py (open_or_rebuild_index,
//! needs_rebuild, wipe_index, _write_sentinels, schema_fingerprint), documents/tasks.py
//! (index_optimize, index_document, remove_document_from_index),
//! documents/management/commands/document_index.py.

mod blake2b;
pub mod db;
pub mod dirty;
pub mod doc;
pub mod error;
pub mod lock;
pub mod record;
pub mod settings;
pub mod writer;

pub use doc::build_tantivy_doc;
pub use error::IndexerError;
pub use record::{BarcodeRecord, CustomFieldRecord, DocumentRecord, NoteRecord};
pub use settings::{needs_rebuild, schema_fingerprint};
pub use writer::{Indexer, IndexerConfig, RebuildOptions, RebuildStats, WriteBatch, WriteOutcome};

/// Own migrations (expand-only, via `pl_cli::migrate::paperrust_migrations`).
pub mod migrations {
    /// L3: consistency of the search index for permissions/trash (docs/contract/pl-search.md "L3").
    pub const SEARCH_DIRTY: (&str, &str) = (
        "pl_indexer.0001_paperrust_search_dirty",
        include_str!("../migrations/0001_paperrust_search_dirty.sql"),
    );
    pub const ALL: &[(&str, &str)] = &[SEARCH_DIRTY];
}
