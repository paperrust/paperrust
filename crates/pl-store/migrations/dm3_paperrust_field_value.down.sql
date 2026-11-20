-- Rollback of stage 3 "typed field values" (dm3_paperrust_field_value.sql): documents_customfieldinstance
-- becomes the Django table again (filled from the view, same ids and sequence state), counters without
-- kind 6. Included before: dm_label_count_fns.sql. Call: `paperrust manage datamodel rollback --to 2`.

SET LOCAL synchronize_seqscans = off;

DO $mig$
DECLARE
    r       record;
    sync_on boolean;
BEGIN
    IF to_regclass('public.paperrust_field_value') IS NULL THEN
        RETURN;
    END IF;
    sync_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.paperrust_field_value'::regclass
                                                  AND tgname = 'paperrust_sync_upd');
    IF sync_on THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;

    -- value_monetary_amount is GENERATED in the Django table
    TRUNCATE ngx_legacy.documents_customfieldinstance;
    INSERT INTO ngx_legacy.documents_customfieldinstance (id, deleted_at, restored_at, transaction_id, created, value_text,
        value_bool, value_url, value_date, value_int, value_float, value_monetary, value_document_ids, value_select,
        value_long_text, field_id, document_id)
    SELECT id, deleted_at, restored_at, transaction_id, created, value_text, value_bool, value_url, value_date, value_int,
           value_float, value_monetary, value_document_ids, value_select, value_long_text, field_id, document_id
      FROM public.documents_customfieldinstance;  -- heap order (= Django order, see conversion)
    PERFORM setval('ngx_legacy.documents_customfieldinstance_id_seq', s.last_value, s.is_called) FROM paperrust_field_value_id_seq s;

    DROP VIEW public.documents_customfieldinstance;
    ALTER TABLE ngx_legacy.documents_customfieldinstance SET SCHEMA public;
    -- foreign keys as v3.4.2 (the document has been paperrust_document since stage 1)
    ALTER TABLE documents_customfieldinstance
        ADD CONSTRAINT documents_customfiel_document_id_610a968e_fk_documents FOREIGN KEY (document_id)
            REFERENCES paperrust_document (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT documents_customfiel_field_id_6c59e32f_fk_documents FOREIGN KEY (field_id)
            REFERENCES documents_customfield (id) DEFERRABLE INITIALLY DEFERRED;
    FOR r IN SELECT c.conname, c.conrelid::regclass AS tbl, pg_get_constraintdef(c.oid) AS def FROM pg_constraint c
              WHERE c.contype = 'f' AND c.confrelid = 'public.paperrust_field_value'::regclass
                AND c.conrelid <> c.confrelid LOOP
        EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', r.tbl, r.conname);
        EXECUTE format('ALTER TABLE %s ADD CONSTRAINT %I %s', r.tbl, r.conname,
                       regexp_replace(r.def, 'REFERENCES (public\.)?paperrust_field_value\(', 'REFERENCES documents_customfieldinstance('));
    END LOOP;
    DROP TABLE paperrust_field_value;

    PERFORM paperrust_label_count_install();
    PERFORM paperrust_label_count_rebuild();
    IF sync_on THEN
        PERFORM paperrust_sync_install_triggers();
    END IF;
END
$mig$;

DROP FUNCTION IF EXISTS paperrust_field_value_view_ins(), paperrust_field_value_view_upd(), paperrust_field_value_view_del(),
    paperrust_cf_row_kind(text, text, text, text, text, boolean, date, integer, double precision, jsonb, integer),
    paperrust_cf_kind(text);
DO $fns$
DECLARE
    r record;
BEGIN
    FOR r IN SELECT p.oid::regprocedure AS f FROM pg_proc p JOIN pg_namespace ns ON ns.oid = p.pronamespace
              WHERE ns.nspname = current_schema() AND p.proname LIKE 'paperrust\_count\_cap\_paperrust\_field\_value\_%' LOOP
        EXECUTE format('DROP FUNCTION %s', r.f);
    END LOOP;
END
$fns$;
ANALYZE documents_customfieldinstance;
