-- Rollback of stage 2 "tags + counters" (dm2_paperrust_tag_count.sql): paperrust_document_tag becomes
-- documents_document_tags again with the v3.4.2 keys and indexes, counters and their triggers are dropped.
-- Call: `paperrust manage datamodel rollback --to 1`. Idempotent.

DO $mig$
DECLARE
    r       record;
    sync_on boolean;
BEGIN
    IF to_regclass('public.paperrust_document_tag') IS NULL THEN
        RETURN;
    END IF;
    sync_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.paperrust_document_tag'::regclass
                                                  AND tgname = 'paperrust_sync_upd');
    IF sync_on THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;
    -- counter triggers gone
    FOR r IN SELECT t.tgname, t.tgrelid::regclass AS tbl FROM pg_trigger t
              WHERE NOT t.tgisinternal AND t.tgname LIKE 'paperrust\_count\_%'
                AND t.tgrelid IN ('public.paperrust_document'::regclass, 'public.paperrust_document_tag'::regclass) LOOP
        EXECUTE format('DROP TRIGGER %I ON %s', r.tgname, r.tbl);
    END LOOP;

    DROP VIEW documents_document_tags;
    ALTER TABLE paperrust_document_tag RENAME TO documents_document_tags;
    ALTER SEQUENCE IF EXISTS paperrust_document_tag_id_seq RENAME TO documents_document_tags_id_seq;
    ALTER TABLE documents_document_tags DROP CONSTRAINT paperrust_document_tag_pkey;
    DROP INDEX IF EXISTS paperrust_document_tag_tag;
    -- v3.4.2 (pl-dbmigrate/schema/v3.4.2-postgres-post.sql)
    ALTER TABLE documents_document_tags ADD CONSTRAINT documents_document_tags_pkey PRIMARY KEY (id);
    ALTER TABLE documents_document_tags
        ADD CONSTRAINT documents_document_tags_document_id_tag_id_07e90502_uniq UNIQUE (document_id, tag_id);
    CREATE INDEX documents_document_tags_document_id_006f35a8 ON documents_document_tags (document_id);
    CREATE INDEX documents_document_tags_tag_id_4f4a71e9 ON documents_document_tags (tag_id);

    IF sync_on THEN
        PERFORM paperrust_sync_install_triggers();
    END IF;
END
$mig$;

DO $fns$
DECLARE
    r record;
BEGIN
    FOR r IN SELECT p.oid::regprocedure AS f FROM pg_proc p JOIN pg_namespace ns ON ns.oid = p.pronamespace
              WHERE ns.nspname = current_schema() AND p.proname LIKE 'paperrust\_count\_cap\_%' LOOP
        EXECUTE format('DROP FUNCTION %s', r.f);
    END LOOP;
END
$fns$;
DROP VIEW IF EXISTS paperrust_label_count_live;
DROP TABLE IF EXISTS paperrust_label_count, paperrust_label_count_delta;
DROP FUNCTION IF EXISTS paperrust_label_count_install(), paperrust_label_count_trunc(), paperrust_label_count_fold(),
    paperrust_label_count_rebuild(), paperrust_label_count_doc_body(text, text, boolean), paperrust_label_count_tag_body(text, text),
    paperrust_label_count_field_body(text, text),
    paperrust_doc_bucket(integer, integer);
