-- PaperRust: watch list of redacted versions (docs/contract/pl-pdftools.md §4.1). Expand-only, idempotent.
-- One row per successful redaction: checksum (SHA-256, like documents_document.checksum) of the
-- redacted file that is ingested as a new version. When sharing, the UI uses it to recognize
-- which version is redacted – independent of the editable version name. Deliberately without a foreign key:
-- after the restructuring stages (docs/DATAMODEL-V2.md) documents_document is a view, and deletions in
-- paperless-ngx (after a way back) must never fail on this table. Orphaned rows do no harm.
CREATE TABLE IF NOT EXISTS paperrust_redaction (
    id bigserial PRIMARY KEY,
    root_document_id integer NOT NULL,
    source_document_id integer NULL,
    checksum varchar(64) NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    actor_id integer NULL
);
CREATE INDEX IF NOT EXISTS paperrust_redaction_root_idx ON paperrust_redaction (root_document_id);
CREATE INDEX IF NOT EXISTS paperrust_redaction_checksum_idx ON paperrust_redaction (checksum);
