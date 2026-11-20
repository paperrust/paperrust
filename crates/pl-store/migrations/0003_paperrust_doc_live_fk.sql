-- PaperRust (docs/PERF-SCALE.md §12): narrow partial index for `selection_data` over large selections
-- ("select all", Angular document list with include_selection_data).
--
-- The selection (`WITH sel AS MATERIALIZED (SELECT id, correspondent_id, document_type_id,
-- storage_path_id …)`) read the whole heap table at 1 million documents (1 GB, content mostly lies
-- inline there), although only five integer columns are needed. With this index it runs
-- as an index-only scan (≈ 40 MB); `owner_id` is included so that the visibility of
-- restricted users is also checked without heap access. Key `id`: same order as
-- the junction tables (`document_id`), the planner can merge instead of hashing.
--
-- * expand-only: only one new index with prefix paperrust_ on documents_document; no column,
--   default or constraint changed. Results unchanged (plans only).
-- * Write cost: one more index entry per new row version (HOT updates are not possible anyway because of the
--   indexed `modified`).
-- * idempotent (IF NOT EXISTS). Rollback: DROP INDEX IF EXISTS paperrust_doc_live_fk;

-- * own data model (docs/DATAMODEL-V2.md §6): after stage 1 documents_document is a view,
--   the index then belongs on paperrust_document (stage 1 takes it along during the conversion).
DO $mig$
BEGIN
    IF to_regclass('public.paperrust_document') IS NOT NULL THEN
        CREATE INDEX IF NOT EXISTS paperrust_doc_live_fk
            ON paperrust_document (id) INCLUDE (owner_id, correspondent_id, document_type_id, storage_path_id)
            WHERE root_document_id IS NULL AND deleted_at IS NULL;
    ELSE
        CREATE INDEX IF NOT EXISTS paperrust_doc_live_fk
            ON documents_document (id) INCLUDE (owner_id, correspondent_id, document_type_id, storage_path_id)
            WHERE root_document_id IS NULL AND deleted_at IS NULL;
    END IF;
END
$mig$;
