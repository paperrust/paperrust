-- PaperRust own data model, stage 5 "labels + sync" (docs/DATAMODEL-V2.md §1.9, §1.10, §6).
-- Name in paperrust_migrations: pl_store.dm5_paperrust_label_sync. Rollback: dm5_paperrust_label_sync.down.sql.
-- Contract: docs/contract/pl-store.md "Stage 5". Presupposes stages 1–4. Before it runs (same
-- stage text) crates/pl-sync/migrations/0003_paperrust_sync_native.sql: the capture, sequencer,
-- rebuild and retention functions that know both states.
--
--   * documents_tag / _correspondent / _documenttype / _storagepath / _customfield are renamed to
--     paperrust_tag / _correspondent / _document_type / _storage_path / _custom_field (no
--     copying; foreign keys, indexes and triggers follow) and get uuid (UNIQUE) and rev. Under
--     the old name a simple view remains with exactly the previous columns (automatically
--     writable, also INSERT … ON CONFLICT and FOR UPDATE).
--   * paperrust_document gets uuid and rev in the row (table rewrite in heap order
--     via ALTER … USING, without bloat; fillfactor 85 so that rev changes stay HOT).
--   * paperrust_sync_object (side table, at 1 million documents ≈ 300 MB) is dropped:
--       kinds 1–6  uuid/rev in the row,
--       kinds 7–9  (views, users, groups – tables stay Django) in paperrust_sync_ident,
--       tombstones of deleted objects in paperrust_sync_tombstone.
--     paperrust_sync_object remains as a read view (same columns except seq); paperrust_sync_live
--     (UNION view) is dropped, the sequencer checks via the primary key.
-- * no entry in django_migrations; idempotent (nothing runs if paperrust_sync_ident exists).

SET LOCAL maintenance_work_mem = '256MB';

DO $mig$
DECLARE
    r        record;
    cols     text;
    has_sync boolean;
BEGIN
    IF to_regclass('public.paperrust_sync_ident') IS NOT NULL THEN
        RETURN;  -- already converted
    END IF;
    IF to_regclass('public.paperrust_object_grant') IS NULL THEN
        RAISE EXCEPTION 'stage 5 needs stage 4 (pl_store.dm4_paperrust_object_grant)';
    END IF;
    has_sync := (SELECT c.relkind FROM pg_class c WHERE c.oid = to_regclass('public.paperrust_sync_object')) = 'r';
    IF to_regprocedure('paperrust_sync_drop_triggers()') IS NOT NULL THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;

    -- helper functions for taking over the previous identity (same uuid, same rev)
    IF has_sync THEN
        CREATE OR REPLACE FUNCTION pg_temp.dm5_uuid(k smallint, i bigint) RETURNS uuid LANGUAGE sql VOLATILE AS
            'SELECT coalesce((SELECT o.uuid FROM paperrust_sync_object o WHERE o.kind = k AND o.local_id = i), gen_random_uuid())';
        CREATE OR REPLACE FUNCTION pg_temp.dm5_rev(k smallint, i bigint) RETURNS bigint LANGUAGE sql STABLE AS
            'SELECT coalesce((SELECT o.rev FROM paperrust_sync_object o WHERE o.kind = k AND o.local_id = i), 1)';
    ELSE
        CREATE OR REPLACE FUNCTION pg_temp.dm5_uuid(k smallint, i bigint) RETURNS uuid LANGUAGE sql VOLATILE AS 'SELECT gen_random_uuid()';
        CREATE OR REPLACE FUNCTION pg_temp.dm5_rev(k smallint, i bigint) RETURNS bigint LANGUAGE sql STABLE AS 'SELECT 1::bigint';
    END IF;

    -- 1. labels: rename, uuid/rev, compatibility view
    FOR r IN SELECT * FROM (VALUES
            ('documents_tag',           'paperrust_tag',           2),
            ('documents_correspondent', 'paperrust_correspondent', 3),
            ('documents_documenttype',  'paperrust_document_type', 4),
            ('documents_storagepath',   'paperrust_storage_path',  5),
            ('documents_customfield',   'paperrust_custom_field',  6)) v(django, native, kind)
    LOOP
        EXECUTE format('ALTER TABLE %I RENAME TO %I', r.django, r.native);
        EXECUTE format('ALTER SEQUENCE IF EXISTS %I RENAME TO %I', r.django || '_id_seq', r.native || '_id_seq');
        SELECT string_agg(quote_ident(a.attname), ', ' ORDER BY a.attnum) INTO cols
          FROM pg_attribute a WHERE a.attrelid = r.native::regclass AND a.attnum > 0 AND NOT a.attisdropped;
        EXECUTE format('ALTER TABLE %I ADD COLUMN uuid uuid, ADD COLUMN rev bigint NOT NULL DEFAULT 1', r.native);
        -- rewrite without triggers (UPDATE fired the label triggers)
        EXECUTE format('ALTER TABLE %I ALTER COLUMN uuid TYPE uuid USING pg_temp.dm5_uuid(%s::smallint, id),
                                       ALTER COLUMN rev TYPE bigint USING pg_temp.dm5_rev(%s::smallint, id)',
                       r.native, r.kind, r.kind);
        EXECUTE format('ALTER TABLE %I ALTER COLUMN uuid SET DEFAULT gen_random_uuid(), ALTER COLUMN uuid SET NOT NULL,
                                       ADD CONSTRAINT %I UNIQUE (uuid)', r.native, r.native || '_uuid_key');
        EXECUTE format('CREATE VIEW %I AS SELECT %s FROM %I', r.django, cols, r.native);
        -- import (setval via the column default of the view, pl_cli::export::importer)
        EXECUTE format('ALTER VIEW %I ALTER COLUMN id SET DEFAULT nextval(%L::regclass)', r.django, r.native || '_id_seq');
    END LOOP;

    -- 2. document: uuid/rev into the row (versions get their own uuid, the root document
    --    is reported)
    ALTER TABLE paperrust_document SET (fillfactor = 85);
    ALTER TABLE paperrust_document ADD COLUMN uuid uuid, ADD COLUMN rev bigint NOT NULL DEFAULT 1;
    ALTER TABLE paperrust_document
        ALTER COLUMN uuid TYPE uuid USING pg_temp.dm5_uuid(1::smallint, id),
        ALTER COLUMN rev TYPE bigint USING pg_temp.dm5_rev(1::smallint, id);
    ALTER TABLE paperrust_document ALTER COLUMN uuid SET DEFAULT gen_random_uuid(), ALTER COLUMN uuid SET NOT NULL,
        ADD CONSTRAINT paperrust_document_uuid_key UNIQUE (uuid);

    -- 3. identity of the remaining kinds and tombstones
    CREATE TABLE paperrust_sync_ident (
        kind     smallint NOT NULL,          -- 7 saved_view, 8 user, 9 group
        local_id integer  NOT NULL,
        uuid     uuid     NOT NULL DEFAULT gen_random_uuid() CONSTRAINT paperrust_sync_ident_uuid_key UNIQUE,
        rev      bigint   NOT NULL DEFAULT 1,
        CONSTRAINT paperrust_sync_ident_pkey PRIMARY KEY (kind, local_id)
    ) WITH (fillfactor = 70);
    CREATE TABLE paperrust_sync_tombstone (
        kind       smallint    NOT NULL,
        local_id   integer     NOT NULL,
        uuid       uuid        NOT NULL,
        rev        bigint      NOT NULL,
        deleted_at timestamptz NOT NULL DEFAULT now(),
        CONSTRAINT paperrust_sync_tombstone_pkey PRIMARY KEY (kind, local_id)
    );
    CREATE INDEX paperrust_sync_tombstone_uuid ON paperrust_sync_tombstone (uuid);
    IF has_sync THEN
        INSERT INTO paperrust_sync_ident (kind, local_id, uuid, rev)
        SELECT o.kind, o.local_id, o.uuid, o.rev FROM paperrust_sync_object o
         WHERE o.kind BETWEEN 7 AND 9
           AND CASE o.kind WHEN 7 THEN EXISTS (SELECT 1 FROM documents_savedview x WHERE x.id = o.local_id)
                           WHEN 8 THEN EXISTS (SELECT 1 FROM auth_user x WHERE x.id = o.local_id)
                           ELSE EXISTS (SELECT 1 FROM auth_group x WHERE x.id = o.local_id) END
         ORDER BY 1, 2;
        INSERT INTO paperrust_sync_tombstone (kind, local_id, uuid, rev, deleted_at)
        SELECT o.kind, o.local_id, o.uuid, o.rev, coalesce(o.deleted_at, now()) FROM paperrust_sync_object o
         WHERE o.local_id BETWEEN 1 AND 2147483647
           AND NOT CASE o.kind
                   WHEN 1 THEN EXISTS (SELECT 1 FROM paperrust_document x WHERE x.id = o.local_id)
                   WHEN 2 THEN EXISTS (SELECT 1 FROM paperrust_tag x WHERE x.id = o.local_id)
                   WHEN 3 THEN EXISTS (SELECT 1 FROM paperrust_correspondent x WHERE x.id = o.local_id)
                   WHEN 4 THEN EXISTS (SELECT 1 FROM paperrust_document_type x WHERE x.id = o.local_id)
                   WHEN 5 THEN EXISTS (SELECT 1 FROM paperrust_storage_path x WHERE x.id = o.local_id)
                   WHEN 6 THEN EXISTS (SELECT 1 FROM paperrust_custom_field x WHERE x.id = o.local_id)
                   WHEN 7 THEN EXISTS (SELECT 1 FROM documents_savedview x WHERE x.id = o.local_id)
                   WHEN 8 THEN EXISTS (SELECT 1 FROM auth_user x WHERE x.id = o.local_id)
                   WHEN 9 THEN EXISTS (SELECT 1 FROM auth_group x WHERE x.id = o.local_id)
                   ELSE true END
         ORDER BY 1, 2;
        DROP VIEW IF EXISTS paperrust_sync_live;
        DROP TABLE paperrust_sync_object;
    END IF;

    -- 4. read view under the old name (sync/ids, identity in pl-store, pl-timeline). local_id is
    --    integer: comparisons with int8 parameters thus use the primary keys (readers cast ::int8).
    CREATE VIEW paperrust_sync_object AS
        SELECT 1::smallint AS kind, d.id AS local_id, d.uuid, d.rev, d.deleted_at
          FROM paperrust_document d WHERE d.root_document_id IS NULL
        UNION ALL SELECT 2::smallint, x.id, x.uuid, x.rev, NULL::timestamptz FROM paperrust_tag x
        UNION ALL SELECT 3::smallint, x.id, x.uuid, x.rev, NULL FROM paperrust_correspondent x
        UNION ALL SELECT 4::smallint, x.id, x.uuid, x.rev, NULL FROM paperrust_document_type x
        UNION ALL SELECT 5::smallint, x.id, x.uuid, x.rev, NULL FROM paperrust_storage_path x
        UNION ALL SELECT 6::smallint, x.id, x.uuid, x.rev, NULL FROM paperrust_custom_field x
        UNION ALL SELECT i.kind, i.local_id, i.uuid, i.rev, NULL FROM paperrust_sync_ident i
        UNION ALL SELECT t.kind, t.local_id, t.uuid, t.rev, t.deleted_at FROM paperrust_sync_tombstone t;
END
$mig$;

-- 5. recreate capture on the renamed tables (functions from pl_sync 0003, above);
--    counter triggers in the version that looks only at counted changes (dm_label_count_fns.sql, above)
DO $mig$
BEGIN
    PERFORM paperrust_label_count_install();
    IF to_regclass('public.paperrust_sync_meta') IS NOT NULL
       AND coalesce((SELECT value FROM paperrust_sync_meta WHERE key = 'enabled'), 'true'::jsonb) = 'true'::jsonb
       AND NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.paperrust_document'::regclass
                                                  AND tgname = 'paperrust_sync_rev') THEN
        PERFORM paperrust_sync_install_triggers();
    END IF;
END
$mig$;

ANALYZE paperrust_document;
ANALYZE paperrust_sync_ident;
ANALYZE paperrust_sync_tombstone;
