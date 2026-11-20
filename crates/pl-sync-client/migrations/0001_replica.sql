-- pl-sync-client 0001: local replica of a server profile (docs/SYNC.md §4.2).
-- Deviations from the draft (docs/contract/pl-sync-client.md "Known differences"):
--  * document has an INTEGER rowid (`id`), `key` is UNIQUE: FTS5 needs stable rowids.
--  * fts with external content (content='document') instead of contentless → snippet()/highlight() work;
--    the columns notes/names are stored as text in document for that.
--  * additional columns for the offline list (added, storage_path, owner, mime_type, …).

CREATE TABLE remote (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    url            TEXT NOT NULL,
    flavor         TEXT NOT NULL CHECK (flavor IN ('paperrust', 'ngx')),
    server_uuid    TEXT,
    user_name      TEXT,
    device_uuid    TEXT,
    cursor         TEXT,
    perm_epoch     INTEGER,
    ngx_watermark  TEXT,
    last_full_sync TEXT,
    last_contact   TEXT,
    me_json        TEXT,
    ui_settings    TEXT
);

-- Profile settings (limits, file mode …), key/value
CREATE TABLE setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Metadata kinds (always mirrored completely)
CREATE TABLE object (
    kind      TEXT NOT NULL,
    key       TEXT NOT NULL,
    legacy_id INTEGER,
    rev       INTEGER,
    json      TEXT NOT NULL,
    name      TEXT,
    PRIMARY KEY (kind, key)
) WITHOUT ROWID;
CREATE INDEX object_kind_name ON object (kind, name COLLATE NOCASE);

CREATE TABLE document (
    id                INTEGER PRIMARY KEY,
    key               TEXT NOT NULL UNIQUE,
    legacy_id         INTEGER,
    rev               INTEGER,
    title             TEXT NOT NULL DEFAULT '',
    created           TEXT,
    added             TEXT,
    modified          TEXT,
    correspondent     TEXT,
    document_type     TEXT,
    storage_path      TEXT,
    owner             TEXT,
    asn               INTEGER,
    mime_type         TEXT,
    original_filename TEXT,
    json              TEXT NOT NULL,
    content           TEXT,
    notes             TEXT,           -- note texts (for FTS), newline-separated
    notes_json        TEXT,           -- notes as in v2 (`include=notes`)
    names             TEXT,           -- correspondent, type, storage path, tags as text (FTS)
    has_detail        INTEGER NOT NULL DEFAULT 0,   -- content/notes loaded (objects/batch)
    in_selection      INTEGER NOT NULL DEFAULT 1,
    left_selection    TEXT,           -- time since which no longer selected (7 days)
    fingerprint       TEXT            -- §7 (paperless-ngx, P6)
);
CREATE INDEX document_created  ON document (created, legacy_id);
CREATE INDEX document_added    ON document (added, legacy_id);
CREATE INDEX document_modified ON document (modified, legacy_id);
CREATE INDEX document_title    ON document (title COLLATE NOCASE, legacy_id);
CREATE INDEX document_corr     ON document (correspondent);
CREATE INDEX document_type     ON document (document_type);
CREATE INDEX document_asn      ON document (asn);

CREATE TABLE document_tag (
    doc TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (doc, tag)
) WITHOUT ROWID;
CREATE INDEX document_tag_tag ON document_tag (tag, doc);

CREATE TABLE selection (
    type  TEXT NOT NULL CHECK (type IN ('document', 'tag', 'saved_view')),
    key   TEXT NOT NULL,
    files TEXT NOT NULL DEFAULT 'preferred' CHECK (files IN ('preferred', 'original', 'both', 'none')),
    PRIMARY KEY (type, key)
) WITHOUT ROWID;

-- File cache; `state` = complete | partial (download interrupted, `.part` present, range resume)
CREATE TABLE file (
    doc         TEXT NOT NULL,
    variant     TEXT NOT NULL CHECK (variant IN ('original', 'archive', 'thumb')),
    path        TEXT NOT NULL,
    etag        TEXT,
    checksum    TEXT,
    size        INTEGER NOT NULL DEFAULT 0,
    state       TEXT NOT NULL DEFAULT 'complete' CHECK (state IN ('complete', 'partial')),
    pinned      INTEGER NOT NULL DEFAULT 1,   -- 1 = belongs to the selection, 0 = cache (evictable)
    fetched     TEXT,
    last_access TEXT,
    PRIMARY KEY (doc, variant)
) WITHOUT ROWID;
CREATE INDEX file_lru ON file (pinned, last_access);

CREATE VIRTUAL TABLE fts USING fts5(
    title, content, notes, names,
    content = 'document', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER document_fts_ai AFTER INSERT ON document BEGIN
    INSERT INTO fts (rowid, title, content, notes, names)
    VALUES (new.id, new.title, new.content, new.notes, new.names);
END;
CREATE TRIGGER document_fts_ad AFTER DELETE ON document BEGIN
    INSERT INTO fts (fts, rowid, title, content, notes, names)
    VALUES ('delete', old.id, old.title, old.content, old.notes, old.names);
END;
CREATE TRIGGER document_fts_au AFTER UPDATE OF title, content, notes, names ON document BEGIN
    INSERT INTO fts (fts, rowid, title, content, notes, names)
    VALUES ('delete', old.id, old.title, old.content, old.notes, old.names);
    INSERT INTO fts (rowid, title, content, notes, names)
    VALUES (new.id, new.title, new.content, new.notes, new.names);
END;

-- 3b (P7): outbox, only created here so that later migrations need not rebuild anything.
CREATE TABLE outbox (
    id              INTEGER PRIMARY KEY,
    created         TEXT,
    op              TEXT CHECK (op IN ('patch_document', 'add_note', 'upload', 'delete_document', 'bulk_tags')),
    target          TEXT,
    base_rev        INTEGER,
    payload         TEXT NOT NULL,
    idempotency_key TEXT UNIQUE,
    state           TEXT DEFAULT 'queued' CHECK (state IN ('queued', 'sending', 'done', 'conflict', 'failed')),
    error           TEXT,
    attempts        INTEGER DEFAULT 0
);
