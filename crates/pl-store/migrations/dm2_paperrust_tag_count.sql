-- PaperRust own data model, stage 2 "tags + counters" (docs/DATAMODEL-V2.md §1.4, §1.5, §6).
-- Name in paperrust_migrations: pl_store.dm2_paperrust_tag_count. Rollback: dm2_paperrust_tag_count.down.sql.
-- Contract: docs/contract/pl-store.md "Stage 2". Presupposes stage 1.
--
-- 1. Tag mapping paperrust_document_tag: the Django table documents_document_tags is renamed in
--    place (no copying of millions of rows), primary key (document_id, tag_id) instead of
--    id, plus (tag_id, document_id) – both directions run index-only, two Django indexes are dropped.
--    The column id stays (without an index, default from the identity sequence): this way the view
--    documents_document_tags is a simple, automatically writable view (INSERT … ON CONFLICT,
--    UPDATE, DELETE keep working, triggers of the table fire). Deviation from the plan (there without id).
-- 2. Counters paperrust_label_count (kind, label_id, owner_bucket, n): non-deleted documents per
--    label (2 tag, 3 correspondent, 4 document type, 5 storage path) and "visibility owner"
--    (owner_id of the document, for versions that of the root; 0 = without owner). Same set as
--    upstream document_count: versions count too, visibility of a version follows the root.
--    Maintained by statement triggers (transition tables) on paperrust_document and
--    paperrust_document_tag; they write only differences into paperrust_label_count_delta (INSERT only,
--    no hot rows with parallel writers); paperrust_label_count_fold() folds them together.
--    Reading: paperrust_label_count_live (sum of both). Check/rebuild:
--    `paperrust manage datamodel counts check|rebuild|fold`.
-- 3. note_count/tag_count and paperrust_doc_totals (plan §1.1/§1.5) are NOT part of this stage:
--    see DATAMODEL-V2.md §6 stage 2 (rationale).

-- Counter tables and functions -----------------------------------------------------------------
CREATE TABLE IF NOT EXISTS paperrust_label_count (
    kind         smallint NOT NULL,
    label_id     integer  NOT NULL,
    owner_bucket integer  NOT NULL,
    n            integer  NOT NULL,
    PRIMARY KEY (kind, label_id, owner_bucket)
);
-- restricted users read the buckets {uid, 0} per kind (index-only)
CREATE INDEX IF NOT EXISTS paperrust_label_count_bucket ON paperrust_label_count (kind, owner_bucket, label_id) INCLUDE (n);
CREATE TABLE IF NOT EXISTS paperrust_label_count_delta (
    kind         smallint NOT NULL,
    label_id     integer  NOT NULL,
    owner_bucket integer  NOT NULL,
    d            integer  NOT NULL
);
CREATE OR REPLACE VIEW paperrust_label_count_live AS
    SELECT kind, label_id, owner_bucket, sum(n)::bigint AS n
      FROM (SELECT kind, label_id, owner_bucket, n FROM paperrust_label_count
            UNION ALL SELECT kind, label_id, owner_bucket, d FROM paperrust_label_count_delta) x
     GROUP BY kind, label_id, owner_bucket;

-- Counter functions: dm_label_count_fns.sql (included before, pl_store::datamodel)

-- Tag mapping for the capture triggers (same version as pl_sync 0001 and stage 1)
CREATE OR REPLACE FUNCTION paperrust_native_table(p text) RETURNS text
LANGUAGE sql STABLE AS $fn$
    SELECT coalesce((SELECT m.native FROM (VALUES
        ('documents_document',            'paperrust_document'),
        ('documents_document_tags',       'paperrust_document_tag'),
        ('documents_note',                'paperrust_note'),
        ('documents_customfieldinstance', 'paperrust_field_value'),
        ('documents_tag',                 'paperrust_tag'),
        ('documents_correspondent',       'paperrust_correspondent'),
        ('documents_documenttype',        'paperrust_document_type'),
        ('documents_storagepath',         'paperrust_storage_path'),
        ('documents_customfield',         'paperrust_custom_field')
    ) m(django, native)
    WHERE m.django = p
      AND EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                   WHERE n.nspname = 'public' AND c.relname = m.native AND c.relkind IN ('r', 'p'))), p)
$fn$;

DO $mig$
DECLARE
    r       record;
    sync_on boolean;
BEGIN
    IF to_regclass('public.paperrust_document_tag') IS NOT NULL THEN
        RETURN;  -- already converted
    END IF;
    IF to_regclass('public.paperrust_document') IS NULL THEN
        RAISE EXCEPTION 'stage 2 needs stage 1 (pl_store.dm1_paperrust_document)';
    END IF;
    sync_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.documents_document_tags'::regclass
                                                  AND tgname = 'paperrust_sync_upd');
    IF sync_on THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;

    -- 1. rename the mapping, keys and indexes
    ALTER TABLE documents_document_tags RENAME TO paperrust_document_tag;
    ALTER SEQUENCE IF EXISTS documents_document_tags_id_seq RENAME TO paperrust_document_tag_id_seq;
    FOR r IN SELECT conname FROM pg_constraint WHERE conrelid = 'paperrust_document_tag'::regclass AND contype = 'p' LOOP
        EXECUTE format('ALTER TABLE paperrust_document_tag DROP CONSTRAINT %I', r.conname);
    END LOOP;
    -- UNIQUE (document_id, tag_id) becomes the primary key (index stays, is only repurposed)
    FOR r IN SELECT c.conname, c.conindid::regclass AS idx FROM pg_constraint c
              WHERE c.conrelid = 'paperrust_document_tag'::regclass AND c.contype = 'u' LOOP
        EXECUTE format('ALTER TABLE paperrust_document_tag DROP CONSTRAINT %I', r.conname);
    END LOOP;
    ALTER TABLE paperrust_document_tag ADD CONSTRAINT paperrust_document_tag_pkey PRIMARY KEY (document_id, tag_id);
    -- replace the remaining single-column indexes (Django document_id, tag_id)
    FOR r IN SELECT i.indexrelid::regclass AS idx FROM pg_index i
              WHERE i.indrelid = 'paperrust_document_tag'::regclass AND NOT i.indisprimary LOOP
        EXECUTE format('DROP INDEX %s', r.idx);
    END LOOP;
    CREATE INDEX paperrust_document_tag_tag ON paperrust_document_tag (tag_id, document_id);

    -- 2. compatibility view (simple, automatically writable)
    CREATE VIEW documents_document_tags AS SELECT id, document_id, tag_id FROM paperrust_document_tag;

    -- 3. build counters and create triggers
    PERFORM paperrust_label_count_rebuild();
    PERFORM paperrust_label_count_install();

    -- 4. sync capture on the renamed table
    IF sync_on THEN
        PERFORM paperrust_sync_install_triggers();
    END IF;
END
$mig$;

ANALYZE paperrust_document_tag;
ANALYZE paperrust_label_count;
