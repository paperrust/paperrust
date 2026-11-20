-- PaperRust L3 (ARCHITECTURE-V2 §5, docs/PERF-SCALE.md bottleneck #2): trigram indexes for
-- substring search over title and content (`title_content`, `title__icontains`, `content__icontains`,
-- `?search=`). Contract: docs/contract/pl-docfilter.md "L3: trigram indexes".
--
-- Django checks `UPPER(x::text) LIKE UPPER('%…%')` (icontains) – without an index a scan over the
-- whole text. These indexes over EXACTLY this expression deliver candidates; pg_trgm re-checks every
-- candidate row with the LIKE, the result stays identical (pl_store::docsql uses them only
-- if both are valid and the search text has a word part of ≥ 3 letters/digits).
--
-- * expand-only: extension pg_trgm (trusted from PostgreSQL 13, the database owner may
--   create it) + two indexes with prefix paperrust_. If pg_trgm or the privilege is missing, nothing is
--   created (note in the log) and the search stays with the scan.
-- * Build: on 1 million documents (2.2 GB content) see docs/PERF-SCALE.md §9; the table is
--   locked for writers meanwhile (CREATE INDEX without CONCURRENTLY, migration in one
--   transaction). maintenance_work_mem raised only for this transaction.
-- * Rollback: DROP INDEX IF EXISTS paperrust_doc_content_trgm, paperrust_doc_title_trgm;
--   (the extension pg_trgm can stay or be removed with DROP EXTENSION pg_trgm).

SET LOCAL maintenance_work_mem = '256MB';

DO $mig$
BEGIN
    BEGIN
        CREATE EXTENSION IF NOT EXISTS pg_trgm;
    EXCEPTION WHEN OTHERS THEN
        RAISE NOTICE 'pg_trgm not available (%), paperrust trigram indexes skipped', SQLERRM;
        RETURN;
    END;
    EXECUTE 'CREATE INDEX IF NOT EXISTS paperrust_doc_title_trgm
                 ON documents_document USING gin ((upper((title)::text)) gin_trgm_ops)';
    EXECUTE 'CREATE INDEX IF NOT EXISTS paperrust_doc_content_trgm
                 ON documents_document USING gin ((upper((content)::text)) gin_trgm_ops)';
END
$mig$;
