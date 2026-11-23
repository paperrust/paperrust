-- PaperRust: checksum of the file as delivered, for documents whose stored file differs from it
-- (PDF shrinking on ingest, PAPERRUST_SHRINK_ON_INGEST_MB). documents_document.checksum stays the
-- checksum of the stored file (the sanity checker compares them); the duplicate check of the consumer
-- (pre_check_duplicate / claim_unique) also matches this column, so uploading the same original again
-- is detected (contract docs/contract/pl-consumer.md "Duplicate check of shrunk files").
--
-- Expand-only and idempotent. No foreign key: documents_document is a view from data model stage 1
-- on; readers join documents_document, so rows of hard-deleted documents are ignored (never reused:
-- document ids come from a sequence). paperless-ngx does not know the table (way back: no effect).
CREATE TABLE IF NOT EXISTS paperrust_source_checksum (
    document_id integer PRIMARY KEY,
    checksum varchar(64) NOT NULL
);
CREATE INDEX IF NOT EXISTS paperrust_source_checksum_checksum_idx ON paperrust_source_checksum (checksum);
