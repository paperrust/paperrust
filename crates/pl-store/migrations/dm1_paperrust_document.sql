-- PaperRust own data model, stage 1 "slim document" (docs/DATAMODEL-V2.md §1.1–1.3, §2).
-- Name in paperrust_migrations: pl_store.dm1_paperrust_document. Rollback: dm1_paperrust_document.down.sql
-- (`paperrust manage datamodel rollback --to 0`). Contract: docs/contract/pl-store.md "Data model".
--
-- documents_document (v3.4.2, 25 columns, heap ≈ 1 GB at 1 million documents, of which ≈ 90 % text) is
-- split into
--   paperrust_document          narrow: everything except text and file details (same ids)
--   paperrust_document_content  text (1:1, own heap, trigram index moves along)
--   paperrust_document_file     file names and checksums (1:1, duplicate/file indexes move along)
-- Under the old name the view documents_document remains with exactly the v3.4.2 columns (name, type,
-- order; checked by `paperrust manage datamodel check`). Reading via the view costs nothing
-- (LEFT JOIN on unique keys, unused joins are removed by the planner), writing is distributed by
-- INSTEAD-OF triggers per row to the three tables. Sync, search and foreign keys attach to
-- paperrust_document.
--
-- * The Django table moves as an EMPTY structure template to ngx_legacy (rollback fills it from the
--   view). The backup of the data is the pg_dump before the stage (pl_cli::datamodel::backup_before),
--   not a second copy in the database (at 1 million documents 2.8 GB) – deviation from plan §2.1.
-- * Column types stay as in Django (varchar(n)) so that the view is type-identical; content_length is
--   an ordinary column (upstream: GENERATED from content), maintained by the view triggers and the
--   trigger on paperrust_document_content.
-- * uuid/rev move into the row only with stage 5 (common to all kinds; until then unchanged
--   paperrust_sync_object) – deviation from plan §1.1, see DATAMODEL-V2.md.
-- * no entry in django_migrations; idempotent (nothing runs if paperrust_document exists).

SET LOCAL maintenance_work_mem = '256MB';
-- copy in heap order from the block start (see step 3)
SET LOCAL synchronize_seqscans = off;

-- Table mapping Django → PaperRust (also in pl_sync 0001, here for existing installations):
-- capture triggers attach to the native table as soon as it exists.
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

-- Writing via the view ----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION paperrust_document_view_ins() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    IF NEW.id IS NULL THEN
        NEW.id := nextval('paperrust_document_id_seq');
    END IF;
    NEW.content_length := length(NEW.content);
    INSERT INTO paperrust_document (id, deleted_at, restored_at, transaction_id, title, mime_type, page_count,
        created, modified, added, archive_serial_number, correspondent_id, document_type_id, storage_path_id,
        owner_id, content_length, root_document_id, version_label, version_index)
    VALUES (NEW.id, NEW.deleted_at, NEW.restored_at, NEW.transaction_id, NEW.title, NEW.mime_type, NEW.page_count,
        NEW.created, NEW.modified, NEW.added, NEW.archive_serial_number, NEW.correspondent_id, NEW.document_type_id,
        NEW.storage_path_id, NEW.owner_id, coalesce(NEW.content_length, 0), NEW.root_document_id, NEW.version_label,
        NEW.version_index);
    INSERT INTO paperrust_document_content (document_id, content) VALUES (NEW.id, NEW.content);
    INSERT INTO paperrust_document_file (document_id, checksum, archive_checksum, filename, archive_filename, original_filename)
    VALUES (NEW.id, NEW.checksum, NEW.archive_checksum, NEW.filename, NEW.archive_filename, NEW.original_filename);
    RETURN NEW;
END
$fn$;

-- The row in paperrust_document is always written (even if only text/file change): it
-- is the anchor of the sync capture (rev +1 per statement as before).
CREATE OR REPLACE FUNCTION paperrust_document_view_upd() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id THEN
        RAISE EXCEPTION 'changing documents_document.id is not supported' USING ERRCODE = 'feature_not_supported';
    END IF;
    NEW.content_length := length(NEW.content);
    UPDATE paperrust_document SET
        deleted_at = NEW.deleted_at, restored_at = NEW.restored_at, transaction_id = NEW.transaction_id,
        title = NEW.title, mime_type = NEW.mime_type, page_count = NEW.page_count, created = NEW.created,
        modified = NEW.modified, added = NEW.added, archive_serial_number = NEW.archive_serial_number,
        correspondent_id = NEW.correspondent_id, document_type_id = NEW.document_type_id,
        storage_path_id = NEW.storage_path_id, owner_id = NEW.owner_id,
        content_length = coalesce(NEW.content_length, 0), root_document_id = NEW.root_document_id,
        version_label = NEW.version_label, version_index = NEW.version_index
    WHERE id = OLD.id;
    IF NEW.content IS DISTINCT FROM OLD.content THEN
        UPDATE paperrust_document_content SET content = NEW.content WHERE document_id = OLD.id;
    END IF;
    IF (NEW.checksum, NEW.archive_checksum, NEW.filename, NEW.archive_filename, NEW.original_filename)
       IS DISTINCT FROM (OLD.checksum, OLD.archive_checksum, OLD.filename, OLD.archive_filename, OLD.original_filename) THEN
        UPDATE paperrust_document_file SET checksum = NEW.checksum, archive_checksum = NEW.archive_checksum,
            filename = NEW.filename, archive_filename = NEW.archive_filename, original_filename = NEW.original_filename
        WHERE document_id = OLD.id;
    END IF;
    RETURN NEW;
END
$fn$;

CREATE OR REPLACE FUNCTION paperrust_document_view_del() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    -- text and file follow via ON DELETE CASCADE
    DELETE FROM paperrust_document WHERE id = OLD.id;
    RETURN OLD;
END
$fn$;

-- Native writers on paperrust_document_content keep content_length current.
CREATE OR REPLACE FUNCTION paperrust_document_content_len() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    IF EXISTS (SELECT 1 FROM paperrust_document d WHERE d.id = NEW.document_id
               AND d.content_length IS DISTINCT FROM length(NEW.content)) THEN
        UPDATE paperrust_document SET content_length = length(NEW.content) WHERE id = NEW.document_id;
    END IF;
    RETURN NULL;
END
$fn$;

DO $mig$
DECLARE
    r         record;
    had_trgm  boolean;
    had_live_fk boolean;
    sync_on   boolean;
    search_on boolean;
    live_view boolean;
BEGIN
    IF to_regclass('public.paperrust_document') IS NOT NULL THEN
        RETURN;  -- already converted
    END IF;
    IF to_regclass('public.documents_document') IS NULL THEN
        RAISE EXCEPTION 'documents_document is missing – stage 1 needs the paperless-ngx v3.4.2 schema';
    END IF;
    had_trgm := to_regclass('public.paperrust_doc_content_trgm') IS NOT NULL;
    had_live_fk := to_regclass('public.paperrust_doc_live_fk') IS NOT NULL;
    sync_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.documents_document'::regclass AND tgname = 'paperrust_sync_upd');
    search_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.documents_document'::regclass AND tgname = 'paperrust_search_doc_upd');
    live_view := to_regclass('public.paperrust_sync_live') IS NOT NULL;

    -- 1. detach the triggers of the Django table (they are recreated on paperrust_document)
    IF sync_on THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;
    DROP TRIGGER IF EXISTS paperrust_search_doc_upd ON documents_document;
    DROP TRIGGER IF EXISTS paperrust_search_doc_del ON documents_document;

    -- 2. target tables (column order as Django without the moved-out columns)
    CREATE TABLE paperrust_document (
        id                    integer      GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
        deleted_at            timestamptz,
        restored_at           timestamptz,
        transaction_id        uuid,
        title                 varchar(128) NOT NULL,
        mime_type             varchar(256) NOT NULL,
        page_count            integer      CONSTRAINT paperrust_document_page_count_check CHECK (page_count >= 0),
        created               date         NOT NULL,
        modified              timestamptz  NOT NULL,
        added                 timestamptz  NOT NULL,
        archive_serial_number integer      CONSTRAINT paperrust_document_asn_check CHECK (archive_serial_number >= 0)
                                           CONSTRAINT paperrust_document_asn_key UNIQUE,
        correspondent_id      integer,
        document_type_id      integer,
        storage_path_id       integer,
        owner_id              integer,
        content_length        integer      NOT NULL DEFAULT 0,
        root_document_id      integer,
        version_label         varchar(64),
        version_index         integer      CONSTRAINT paperrust_document_version_index_check CHECK (version_index >= 0)
    );
    -- foreign keys only after copy and indexes (step 7): deferred checks from the INSERTs
    -- would block CREATE INDEX in the same transaction ("pending trigger events")
    CREATE TABLE paperrust_document_content (
        document_id integer PRIMARY KEY,
        content     text    NOT NULL
    );
    CREATE TABLE paperrust_document_file (
        document_id       integer       PRIMARY KEY,
        checksum          varchar(64)   NOT NULL,
        archive_checksum  varchar(64),
        filename          varchar(1024) CONSTRAINT paperrust_document_file_filename_key UNIQUE,
        archive_filename  varchar(1024) CONSTRAINT paperrust_document_file_archive_filename_key UNIQUE,
        original_filename varchar(1024)
    );

    -- 3. copy data (same ids, physical row order as before: without ORDER BY the seq
    --    scan reads in heap order – sorts with ties whose order neither upstream nor
    --    PaperRust defines thus stay as before the conversion), take over the sequence state
    INSERT INTO paperrust_document (id, deleted_at, restored_at, transaction_id, title, mime_type, page_count,
        created, modified, added, archive_serial_number, correspondent_id, document_type_id, storage_path_id,
        owner_id, content_length, root_document_id, version_label, version_index)
    SELECT id, deleted_at, restored_at, transaction_id, title, mime_type, page_count, created, modified, added,
           archive_serial_number, correspondent_id, document_type_id, storage_path_id, owner_id,
           coalesce(content_length, length(content)), root_document_id, version_label, version_index
      FROM documents_document;
    INSERT INTO paperrust_document_content (document_id, content)
    SELECT id, content FROM documents_document;
    INSERT INTO paperrust_document_file (document_id, checksum, archive_checksum, filename, archive_filename, original_filename)
    SELECT id, checksum, archive_checksum, filename, archive_filename, original_filename FROM documents_document;
    PERFORM setval('paperrust_document_id_seq', s.last_value, s.is_called) FROM documents_document_id_seq s;

    -- 5. Django table as an empty structure template to ngx_legacy (without foreign keys and without the
    --    own paperrust_* indexes; their names now belong to the new tables)
    CREATE SCHEMA IF NOT EXISTS ngx_legacy;
    FOR r IN SELECT conname FROM pg_constraint WHERE conrelid = 'public.documents_document'::regclass AND contype = 'f' LOOP
        EXECUTE format('ALTER TABLE public.documents_document DROP CONSTRAINT %I', r.conname);
    END LOOP;
    DROP INDEX IF EXISTS paperrust_doc_live_created, paperrust_doc_title_trgm, paperrust_doc_content_trgm, paperrust_doc_live_fk;
    IF live_view THEN
        DROP VIEW paperrust_sync_live;
    END IF;
    ALTER TABLE public.documents_document SET SCHEMA ngx_legacy;

    -- 6. indexes (small because the row is narrow; without the Django artefacts *_like)
    CREATE INDEX paperrust_doc_live_created ON paperrust_document (created DESC, id DESC) INCLUDE (owner_id)
        WHERE root_document_id IS NULL AND deleted_at IS NULL;
    CREATE INDEX paperrust_doc_live_added ON paperrust_document (added DESC, id DESC)
        WHERE root_document_id IS NULL AND deleted_at IS NULL;
    CREATE INDEX paperrust_doc_live_modified ON paperrust_document (modified DESC, id DESC)
        WHERE root_document_id IS NULL AND deleted_at IS NULL;
    IF had_live_fk THEN  -- pl_store.0003 (selection_data as an index-only scan)
        CREATE INDEX paperrust_doc_live_fk ON paperrust_document (id)
            INCLUDE (owner_id, correspondent_id, document_type_id, storage_path_id)
            WHERE root_document_id IS NULL AND deleted_at IS NULL;
    END IF;
    CREATE INDEX paperrust_doc_owner_created ON paperrust_document (owner_id, created);
    CREATE INDEX paperrust_doc_created ON paperrust_document (created);
    CREATE INDEX paperrust_doc_added ON paperrust_document (added);
    CREATE INDEX paperrust_doc_modified ON paperrust_document (modified);
    CREATE INDEX paperrust_doc_title ON paperrust_document (title, id);
    CREATE INDEX paperrust_doc_correspondent ON paperrust_document (correspondent_id) WHERE correspondent_id IS NOT NULL;
    CREATE INDEX paperrust_doc_document_type ON paperrust_document (document_type_id) WHERE document_type_id IS NOT NULL;
    CREATE INDEX paperrust_doc_storage_path ON paperrust_document (storage_path_id) WHERE storage_path_id IS NOT NULL;
    CREATE INDEX paperrust_doc_owner ON paperrust_document (owner_id) WHERE owner_id IS NOT NULL;
    CREATE INDEX paperrust_doc_root ON paperrust_document (root_document_id) WHERE root_document_id IS NOT NULL;
    CREATE UNIQUE INDEX paperrust_doc_root_version_uniq ON paperrust_document (root_document_id, version_index)
        WHERE root_document_id IS NOT NULL AND version_index IS NOT NULL;
    CREATE INDEX paperrust_doc_file_checksum ON paperrust_document_file (checksum);
    CREATE INDEX paperrust_doc_file_archive_checksum ON paperrust_document_file (archive_checksum)
        WHERE archive_checksum IS NOT NULL;
    IF had_trgm THEN
        CREATE INDEX paperrust_doc_title_trgm ON paperrust_document USING gin ((upper((title)::text)) gin_trgm_ops);
        CREATE INDEX paperrust_doc_content_trgm ON paperrust_document_content USING gin ((upper(content)) gin_trgm_ops);
    END IF;

    -- 6b. foreign keys: text/file → document, outgoing recreate, incoming re-point (same name,
    --     same options)
    ALTER TABLE paperrust_document_content ADD CONSTRAINT paperrust_document_content_document_fk
        FOREIGN KEY (document_id) REFERENCES paperrust_document (id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
    ALTER TABLE paperrust_document_file ADD CONSTRAINT paperrust_document_file_document_fk
        FOREIGN KEY (document_id) REFERENCES paperrust_document (id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
    ALTER TABLE paperrust_document
        ADD CONSTRAINT paperrust_document_correspondent_fk FOREIGN KEY (correspondent_id)
            REFERENCES documents_correspondent (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT paperrust_document_document_type_fk FOREIGN KEY (document_type_id)
            REFERENCES documents_documenttype (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT paperrust_document_storage_path_fk FOREIGN KEY (storage_path_id)
            REFERENCES documents_storagepath (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT paperrust_document_owner_fk FOREIGN KEY (owner_id)
            REFERENCES auth_user (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT paperrust_document_root_document_fk FOREIGN KEY (root_document_id)
            REFERENCES paperrust_document (id) DEFERRABLE INITIALLY DEFERRED;
    FOR r IN
        SELECT c.conname, c.conrelid::regclass AS tbl, pg_get_constraintdef(c.oid) AS def
          FROM pg_constraint c
         WHERE c.contype = 'f' AND c.confrelid = 'ngx_legacy.documents_document'::regclass AND c.conrelid <> c.confrelid
         ORDER BY 2, 1
    LOOP
        EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', r.tbl, r.conname);
        EXECUTE format('ALTER TABLE %s ADD CONSTRAINT %I %s', r.tbl, r.conname,
                       regexp_replace(r.def, 'REFERENCES (public\.|ngx_legacy\.)?documents_document\(', 'REFERENCES paperrust_document('));
    END LOOP;


    -- now nothing hangs on the template any more: empty it
    TRUNCATE ngx_legacy.documents_document;

    -- 7. compatibility view: exactly the v3.4.2 columns in v3.4.2 order
    CREATE VIEW documents_document AS
    SELECT d.id, d.deleted_at, d.restored_at, d.transaction_id, d.title, c.content, d.mime_type, f.checksum,
           f.archive_checksum, d.page_count, d.created, d.modified, d.added, f.filename, f.archive_filename,
           f.original_filename, d.archive_serial_number, d.correspondent_id, d.document_type_id, d.storage_path_id,
           d.owner_id, d.content_length, d.root_document_id, d.version_label, d.version_index
      FROM paperrust_document d
      LEFT JOIN paperrust_document_content c ON c.document_id = d.id
      LEFT JOIN paperrust_document_file f ON f.document_id = d.id;
    ALTER VIEW documents_document ALTER COLUMN id SET DEFAULT nextval('paperrust_document_id_seq'::regclass);
    CREATE TRIGGER paperrust_document_view_ins INSTEAD OF INSERT ON documents_document
        FOR EACH ROW EXECUTE FUNCTION paperrust_document_view_ins();
    CREATE TRIGGER paperrust_document_view_upd INSTEAD OF UPDATE ON documents_document
        FOR EACH ROW EXECUTE FUNCTION paperrust_document_view_upd();
    CREATE TRIGGER paperrust_document_view_del INSTEAD OF DELETE ON documents_document
        FOR EACH ROW EXECUTE FUNCTION paperrust_document_view_del();
    CREATE TRIGGER paperrust_document_content_len AFTER INSERT OR UPDATE OF content ON paperrust_document_content
        FOR EACH ROW EXECUTE FUNCTION paperrust_document_content_len();

    -- 8. sync and search triggers on the native table, rebind the view of live objects
    IF live_view THEN
        CREATE VIEW paperrust_sync_live AS
            SELECT 1::smallint AS kind, id AS local_id FROM paperrust_document
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
            AFTER UPDATE OF owner_id, deleted_at, root_document_id ON paperrust_document
            FOR EACH ROW
            WHEN (OLD.owner_id IS DISTINCT FROM NEW.owner_id
                  OR OLD.deleted_at IS DISTINCT FROM NEW.deleted_at
                  OR OLD.root_document_id IS DISTINCT FROM NEW.root_document_id)
            EXECUTE FUNCTION paperrust_search_doc_trg();
        CREATE TRIGGER paperrust_search_doc_del
            AFTER DELETE ON paperrust_document
            FOR EACH ROW EXECUTE FUNCTION paperrust_search_doc_trg();
    END IF;
    IF to_regproc('paperrust_sync_tables') IS NOT NULL THEN
        -- capture on the native table (pl_sync 0001 reads the mapping likewise)
        CREATE OR REPLACE FUNCTION paperrust_sync_tables()
        RETURNS TABLE (tbl text, kind smallint, idexpr text, flag text)
        LANGUAGE sql STABLE AS $fn$
            SELECT paperrust_native_table(v.tbl), v.kind, v.idexpr, v.flag FROM (VALUES
                ('documents_document',            1::smallint, 'coalesce(root_document_id, id)', 'owner'),
                ('documents_document_tags',       1::smallint, 'document_id',   ''),
                ('documents_note',                1::smallint, 'document_id',   ''),
                ('documents_customfieldinstance', 1::smallint, 'document_id',   ''),
                ('documents_tag',                 2::smallint, 'id',            'owner'),
                ('documents_correspondent',       3::smallint, 'id',            'owner'),
                ('documents_documenttype',        4::smallint, 'id',            'owner'),
                ('documents_storagepath',         5::smallint, 'id',            'owner'),
                ('documents_customfield',         6::smallint, 'id',            ''),
                ('documents_savedview',           7::smallint, 'id',            'owner'),
                ('documents_savedviewfilterrule', 7::smallint, 'saved_view_id', ''),
                ('auth_user',                     8::smallint, 'id',            ''),
                ('auth_group',                    9::smallint, 'id',            '')
            ) v(tbl, kind, idexpr, flag)
        $fn$;
    END IF;
    IF sync_on THEN
        PERFORM paperrust_sync_install_triggers();
    END IF;

    -- 9. nothing may hang on the old table any more
    IF EXISTS (SELECT 1 FROM pg_depend d JOIN pg_rewrite rw ON rw.oid = d.objid
                WHERE d.refobjid = 'ngx_legacy.documents_document'::regclass AND rw.ev_class <> d.refobjid)
       OR EXISTS (SELECT 1 FROM pg_constraint WHERE confrelid = 'ngx_legacy.documents_document'::regclass) THEN
        RAISE EXCEPTION 'stage 1: objects still depend on the old documents_document table';
    END IF;
END
$mig$;

ANALYZE paperrust_document;
ANALYZE paperrust_document_content;
ANALYZE paperrust_document_file;
