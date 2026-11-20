-- PaperRust Z2 (integrity check): last confirmed checksum per file for the
-- incremental mode of `documents.tasks.sanity_check` / `document_sanity_checker`.
-- Own table (prefix paperrust_), expand-only, idempotent; no foreign key to
-- documents_document (cleanup happens in the check run), so that deleting/emptying the trash
-- stays unchanged.
CREATE TABLE IF NOT EXISTS paperrust_integrity_state (
    document_id integer     NOT NULL,
    -- 'original' | 'archive'
    kind        varchar(16) NOT NULL,
    path        text        NOT NULL,
    size        bigint      NOT NULL,
    mtime_ns    bigint      NOT NULL,
    checksum    varchar(64) NOT NULL,
    verified_at timestamptz NOT NULL,
    PRIMARY KEY (document_id, kind)
);
