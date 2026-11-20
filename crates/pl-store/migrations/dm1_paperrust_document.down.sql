-- Rollback of stage 1 "slim document" (dm1_paperrust_document.sql): documents_document becomes the
-- Django table again, filled from the view (current state, same ids, same sequence state),
-- foreign keys/triggers/indexes as before the stage. Call: `paperrust manage datamodel rollback --to 0`
-- (also removes the row in paperrust_migrations). Idempotent.

SET LOCAL maintenance_work_mem = '256MB';
-- copy in heap order from the block start (see step 3)
SET LOCAL synchronize_seqscans = off;

DO $mig$
DECLARE
    r         record;
    had_trgm  boolean;
    had_live_fk boolean;
    sync_on   boolean;
    search_on boolean;
    live_view boolean;
BEGIN
    IF to_regclass('public.paperrust_document') IS NULL THEN
        RETURN;
    END IF;
    IF to_regclass('ngx_legacy.documents_document') IS NULL THEN
        RAISE EXCEPTION 'ngx_legacy.documents_document is missing – cannot roll back stage 1';
    END IF;
    had_trgm := to_regclass('public.paperrust_doc_content_trgm') IS NOT NULL;
    had_live_fk := to_regclass('public.paperrust_doc_live_fk') IS NOT NULL;
    sync_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.paperrust_document'::regclass AND tgname = 'paperrust_sync_upd');
    search_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.paperrust_document'::regclass AND tgname = 'paperrust_search_doc_upd');
    live_view := to_regclass('public.paperrust_sync_live') IS NOT NULL;
    IF sync_on THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;

    -- 1. data into the Django table (content_length is GENERATED there)
    TRUNCATE ngx_legacy.documents_document;
    INSERT INTO ngx_legacy.documents_document (id, deleted_at, restored_at, transaction_id, title, content, mime_type,
        checksum, archive_checksum, page_count, created, modified, added, filename, archive_filename, original_filename,
        archive_serial_number, correspondent_id, document_type_id, storage_path_id, owner_id, root_document_id,
        version_label, version_index)
    SELECT id, deleted_at, restored_at, transaction_id, title, content, mime_type, checksum, archive_checksum,
           page_count, created, modified, added, filename, archive_filename, original_filename, archive_serial_number,
           correspondent_id, document_type_id, storage_path_id, owner_id, root_document_id, version_label, version_index
      FROM public.documents_document;  -- heap order of paperrust_document
    PERFORM setval('ngx_legacy.documents_document_id_seq', s.last_value, s.is_called) FROM paperrust_document_id_seq s;

    -- 2. drop the view, table back
    IF live_view THEN
        DROP VIEW paperrust_sync_live;
    END IF;
    DROP VIEW public.documents_document;
    ALTER TABLE ngx_legacy.documents_document SET SCHEMA public;

    -- 3. foreign keys as v3.4.2 (names from pl-dbmigrate/schema/v3.4.2-postgres-post.sql)
    ALTER TABLE documents_document
        ADD CONSTRAINT documents_document_correspondent_id_6164eb0c_fk_documents FOREIGN KEY (correspondent_id)
            REFERENCES documents_correspondent (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT documents_document_document_type_id_1f88b50c_fk_documents FOREIGN KEY (document_type_id)
            REFERENCES documents_documenttype (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT documents_document_storage_path_id_07d27bdb_fk_documents FOREIGN KEY (storage_path_id)
            REFERENCES documents_storagepath (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT documents_document_owner_id_04d2b723_fk_auth_user_id FOREIGN KEY (owner_id)
            REFERENCES auth_user (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT documents_document_root_document_id_78094bf2_fk_documents FOREIGN KEY (root_document_id)
            REFERENCES documents_document (id) DEFERRABLE INITIALLY DEFERRED;
    FOR r IN
        SELECT c.conname, c.conrelid::regclass AS tbl, pg_get_constraintdef(c.oid) AS def
          FROM pg_constraint c
         WHERE c.contype = 'f' AND c.confrelid = 'public.paperrust_document'::regclass
           AND c.conrelid NOT IN ('public.paperrust_document'::regclass, 'public.paperrust_document_content'::regclass,
                                  'public.paperrust_document_file'::regclass)
         ORDER BY 2, 1
    LOOP
        EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', r.tbl, r.conname);
        EXECUTE format('ALTER TABLE %s ADD CONSTRAINT %I %s', r.tbl, r.conname,
                       regexp_replace(r.def, 'REFERENCES (public\.)?paperrust_document\(', 'REFERENCES documents_document('));
    END LOOP;

    -- 4. drop native tables
    DROP TABLE paperrust_document_file, paperrust_document_content, paperrust_document;

    -- 5. own indexes, view of live objects, triggers as before the stage
    CREATE INDEX paperrust_doc_live_created ON documents_document (created DESC, id DESC) INCLUDE (owner_id)
        WHERE root_document_id IS NULL AND deleted_at IS NULL;
    IF had_live_fk THEN
        CREATE INDEX paperrust_doc_live_fk ON documents_document (id)
            INCLUDE (owner_id, correspondent_id, document_type_id, storage_path_id)
            WHERE root_document_id IS NULL AND deleted_at IS NULL;
    END IF;
    IF had_trgm THEN
        CREATE INDEX paperrust_doc_title_trgm ON documents_document USING gin ((upper((title)::text)) gin_trgm_ops);
        CREATE INDEX paperrust_doc_content_trgm ON documents_document USING gin ((upper(content)) gin_trgm_ops);
    END IF;
    IF live_view THEN
        CREATE VIEW paperrust_sync_live AS
            SELECT 1::smallint AS kind, id AS local_id FROM documents_document
             WHERE deleted_at IS NULL AND root_document_id IS NULL
            UNION ALL SELECT 2::smallint, id FROM documents_tag
            UNION ALL SELECT 3::smallint, id FROM documents_correspondent
            UNION ALL SELECT 4::smallint, id FROM documents_documenttype
            UNION ALL SELECT 5::smallint, id FROM documents_storagepath
            UNION ALL SELECT 6::smallint, id FROM documents_customfield
            UNION ALL SELECT 7::smallint, id FROM documents_savedview
            UNION ALL SELECT 8::smallint, id FROM auth_user
            UNION ALL SELECT 9::smallint, id FROM auth_group;
    END IF;
    IF search_on THEN
        CREATE TRIGGER paperrust_search_doc_upd
            AFTER UPDATE OF owner_id, deleted_at, root_document_id ON documents_document
            FOR EACH ROW
            WHEN (OLD.owner_id IS DISTINCT FROM NEW.owner_id
                  OR OLD.deleted_at IS DISTINCT FROM NEW.deleted_at
                  OR OLD.root_document_id IS DISTINCT FROM NEW.root_document_id)
            EXECUTE FUNCTION paperrust_search_doc_trg();
        CREATE TRIGGER paperrust_search_doc_del
            AFTER DELETE ON documents_document
            FOR EACH ROW EXECUTE FUNCTION paperrust_search_doc_trg();
    END IF;
    IF sync_on THEN
        PERFORM paperrust_sync_install_triggers();
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'ngx_legacy') THEN
        DROP SCHEMA IF EXISTS ngx_legacy;
    END IF;
END
$mig$;

DROP FUNCTION IF EXISTS paperrust_document_view_ins(), paperrust_document_view_upd(), paperrust_document_view_del(),
    paperrust_document_content_len();
ANALYZE documents_document;
