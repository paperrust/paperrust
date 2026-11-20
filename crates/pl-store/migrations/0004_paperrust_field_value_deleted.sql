-- PaperRust (docs/PERF-SCALE.md §13): partial index of the deleted custom field values.
--
-- `selection_data` with "select all" without a filter counts from the label counters of the own
-- data model (pl_documents::selection_data::for_all_unfiltered). The counters keep only non-deleted
-- field values; the POST path counts, like upstream (join without manager), also deleted values
-- of live documents. It fetches those directly – with this index from a few rows instead of from all
-- field values (at 1 million documents 3 million rows, ≈ 700 MB).
--
-- * Runs after the conversion stages (pl_cli::migrate::paperrust_migrations): from stage 3 the
--   values are in paperrust_field_value; before that (database without stage 3) on the Django table – there
--   nobody uses it, but it does no harm (expand-only).
-- * idempotent (IF NOT EXISTS). Rollback: DROP INDEX IF EXISTS paperrust_field_value_deleted;
DO $mig$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                WHERE n.nspname = current_schema() AND c.relname = 'paperrust_field_value' AND c.relkind = 'r') THEN
        CREATE INDEX IF NOT EXISTS paperrust_field_value_deleted
            ON paperrust_field_value (document_id) INCLUDE (field_id) WHERE deleted_at IS NOT NULL;
    ELSIF EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                   WHERE n.nspname = current_schema() AND c.relname = 'documents_customfieldinstance' AND c.relkind = 'r') THEN
        CREATE INDEX IF NOT EXISTS paperrust_field_value_deleted
            ON documents_customfieldinstance (document_id) INCLUDE (field_id) WHERE deleted_at IS NOT NULL;
    END IF;
END
$mig$;
