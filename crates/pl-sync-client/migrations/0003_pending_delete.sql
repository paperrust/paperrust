-- pl-sync-client 0003: cache files that could not be deleted (e.g. locked on Windows) after a
-- permission revocation, deletion, deselection or eviction. The rows of the document/file are gone,
-- this record keeps the path until housekeeping succeeds (docs/contract/pl-sync-client.md).
CREATE TABLE pending_delete (
    path     TEXT PRIMARY KEY,
    doc      TEXT,
    size     INTEGER NOT NULL DEFAULT 0,   -- bytes on disk when the deletion failed (counts toward the cache)
    since    TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
