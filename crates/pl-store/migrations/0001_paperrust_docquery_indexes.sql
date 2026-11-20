-- PaperRust L1 (ARCHITECTURE-V2 §5): index for the document list from the filter AST
-- (pl_store::docsql). Contract/measurement: docs/contract/pl-docfilter.md, section "L1".
--
-- * expand-only: only one new index with prefix paperrust_ on documents_document; no column,
--   default or constraint changed. Results unchanged (plans only) – parity-neutral.
-- * Partial index over the base set of the list (root documents outside the trash) in the
--   default sort order `-created, -id`, `owner_id` as INCLUDE: page and COUNT run for
--   superusers as an index-only scan, for other users visibility
--   (`owner_id = uid OR owner_id IS NULL OR id ∈ grants`) is checked without heap access.
-- * idempotent (IF NOT EXISTS). Rollback: DROP INDEX IF EXISTS paperrust_doc_live_created;

CREATE INDEX IF NOT EXISTS paperrust_doc_live_created
    ON documents_document (created DESC, id DESC) INCLUDE (owner_id)
    WHERE root_document_id IS NULL AND deleted_at IS NULL;
