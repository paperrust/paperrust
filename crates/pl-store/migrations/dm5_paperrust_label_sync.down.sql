-- Rollback of stage 5 "labels + sync" (dm5_paperrust_label_sync.sql): label tables get their
-- Django names back, uuid/rev move back into the side table paperrust_sync_object (same
-- uuid, same rev, tombstones as rows with deleted_at), paperrust_sync_live as after stage 1.
-- Call: `paperrust manage datamodel rollback --to 4`.

DO $mig$
DECLARE
    r        record;
    has_sync boolean;
BEGIN
    IF to_regclass('public.paperrust_sync_ident') IS NULL THEN
        RETURN;
    END IF;
    has_sync := to_regclass('public.paperrust_sync_log') IS NOT NULL;
    IF to_regprocedure('paperrust_sync_drop_triggers()') IS NOT NULL THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;

    -- 1. side table from the read view (only if the sync capture is installed; after
    --    `sync disable --purge` it did not exist before the stage)
    IF has_sync THEN
        CREATE TABLE paperrust_sync_object_dm5 (
            kind        smallint    NOT NULL,
            local_id    bigint      NOT NULL,
            uuid        uuid        NOT NULL DEFAULT gen_random_uuid(),
            rev         bigint      NOT NULL DEFAULT 1,
            seq         bigint,
            deleted_at  timestamptz
        ) WITH (fillfactor = 70);
        INSERT INTO paperrust_sync_object_dm5 (kind, local_id, uuid, rev, seq, deleted_at)
        SELECT DISTINCT ON (o.kind, o.local_id) o.kind, o.local_id, o.uuid, o.rev, l.seq, o.deleted_at
          FROM paperrust_sync_object o
          LEFT JOIN paperrust_sync_log l ON l.kind = o.kind AND l.local_id = o.local_id
         ORDER BY o.kind, o.local_id, o.deleted_at NULLS FIRST;
    END IF;
    DROP VIEW paperrust_sync_object;
    IF has_sync THEN
        ALTER TABLE paperrust_sync_object_dm5 RENAME TO paperrust_sync_object;
        ALTER TABLE paperrust_sync_object ADD CONSTRAINT paperrust_sync_object_pkey PRIMARY KEY (kind, local_id),
                                          ADD CONSTRAINT paperrust_sync_object_uuid_key UNIQUE (uuid);
    END IF;
    DROP TABLE paperrust_sync_ident, paperrust_sync_tombstone;

    -- 2. labels: drop view, old name, drop uuid/rev
    FOR r IN SELECT * FROM (VALUES
            ('documents_tag',           'paperrust_tag'),
            ('documents_correspondent', 'paperrust_correspondent'),
            ('documents_documenttype',  'paperrust_document_type'),
            ('documents_storagepath',   'paperrust_storage_path'),
            ('documents_customfield',   'paperrust_custom_field')) v(django, native)
    LOOP
        EXECUTE format('DROP VIEW %I', r.django);
        EXECUTE format('ALTER TABLE %I DROP COLUMN uuid, DROP COLUMN rev', r.native);
        EXECUTE format('ALTER TABLE %I RENAME TO %I', r.native, r.django);
        EXECUTE format('ALTER SEQUENCE IF EXISTS %I RENAME TO %I', r.native || '_id_seq', r.django || '_id_seq');
    END LOOP;

    -- 3. Document
    ALTER TABLE paperrust_document DROP COLUMN uuid, DROP COLUMN rev;
    ALTER TABLE paperrust_document RESET (fillfactor);

    -- 4. view of live objects (stage 1) and capture as before the stage
    IF has_sync THEN
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
        IF coalesce((SELECT value FROM paperrust_sync_meta WHERE key = 'enabled'), 'true'::jsonb) = 'true'::jsonb THEN
            PERFORM paperrust_sync_install_triggers();
        END IF;
    END IF;
END
$mig$;
