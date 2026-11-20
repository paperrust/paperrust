-- PaperRust: change capture for sync (docs/SYNC.md §2). Crate: pl-sync.
-- Contract: docs/contract/pl-sync.md.
--
-- * expand-only: only new tables, a sequence, functions and AFTER statement triggers with
--   prefix paperrust_. No column, no default, no constraint on Django tables; the triggers
--   change no Django row and write only to paperrust_sync_*.
-- * idempotent: may run multiple times (CREATE … IF NOT EXISTS, CREATE OR REPLACE, triggers are
--   recreated). The initial fill runs only as long as paperrust_sync_log is empty.
-- * respects `paperrust manage sync disable`: if paperrust_sync_meta.enabled = false, a repeated
--   run creates no triggers.
-- * Rollback:
--     paperrust manage sync disable          → triggers gone, tables stay (paperless-ngx keeps running
--                                              unchanged)
--     paperrust manage sync disable --purge  → additionally remove all paperrust_sync_* objects and the
--                                              row in paperrust_migrations
--   by hand: SELECT paperrust_sync_drop_triggers(); then DROP TABLE paperrust_sync_object,
--   paperrust_sync_pending, paperrust_sync_log, paperrust_sync_meta; DROP SEQUENCE
--   paperrust_sync_seq; DROP FUNCTION paperrust_sync_* (no upstream data affected).
--
-- Kinds (stable, never renumber): 1 document, 2 tag, 3 correspondent, 4 document_type,
-- 5 storage_path, 6 custom_field, 7 saved_view, 8 user, 9 group.

CREATE TABLE IF NOT EXISTS paperrust_sync_object (
    kind        smallint    NOT NULL,
    local_id    bigint      NOT NULL,             -- id of the Django row
    uuid        uuid        NOT NULL DEFAULT gen_random_uuid(),
    rev         bigint      NOT NULL DEFAULT 1,   -- +1 per captured statement (optimistic locking)
    seq         bigint,                           -- letzte Feed-Position (Sequenzer)
    deleted_at  timestamptz,                      -- Tombstone (Sequenzer)
    PRIMARY KEY (kind, local_id),
    UNIQUE (uuid)
) WITH (fillfactor = 70);
-- Room for HOT updates (rev/seq/deleted_at are not indexed) → cheaper triggers. From
-- data model stage 5 paperrust_sync_object is a read view (pl_sync 0003) – then do nothing.
DO $ff$
BEGIN
    IF (SELECT c.relkind FROM pg_class c WHERE c.oid = to_regclass('public.paperrust_sync_object')) = 'r' THEN
        ALTER TABLE paperrust_sync_object SET (fillfactor = 70);
    END IF;
END
$ff$;

-- Pending list: filled by triggers, emptied by the sequencer. Deliberately without a further index
-- (keep the write cost in the trigger small; the table is almost empty in normal operation).
CREATE TABLE IF NOT EXISTS paperrust_sync_pending (
    id          bigint   GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind        smallint NOT NULL,
    local_id    bigint   NOT NULL,
    perm_change boolean  NOT NULL DEFAULT false,
    xid         xid8     NOT NULL DEFAULT pg_current_xact_id()
);

-- Feed: exactly one entry per object (the newest).
CREATE TABLE IF NOT EXISTS paperrust_sync_log (
    seq         bigint      PRIMARY KEY,
    kind        smallint    NOT NULL,
    local_id    bigint      NOT NULL,
    uuid        uuid        NOT NULL,
    op          char(1)     NOT NULL CHECK (op IN ('u','d')),
    rev         bigint      NOT NULL,
    -- seq of the last permission change (owner/guardian) of this object, NULL = none.
    -- Feed: invisible and perm_seq > cursor → for the client "delete" (revocation).
    perm_seq    bigint,
    at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (kind, local_id)
);
CREATE INDEX IF NOT EXISTS paperrust_sync_log_tombstone_idx ON paperrust_sync_log (at) WHERE op = 'd';
CREATE SEQUENCE IF NOT EXISTS paperrust_sync_seq;

CREATE TABLE IF NOT EXISTS paperrust_sync_meta (
    key   text  PRIMARY KEY,
    value jsonb NOT NULL
);
INSERT INTO paperrust_sync_meta (key, value) VALUES
    ('server_uuid', to_jsonb(gen_random_uuid()::text)),
    ('perm_epoch', '0'::jsonb),
    ('min_cursor', '0'::jsonb),
    ('enabled', 'true'::jsonb)
ON CONFLICT (key) DO NOTHING;

-- ---------------------------------------------------------------------------------------------
-- Triggers: per (table, operation) one function generated from the same template
-- paperrust_sync_cap_<table>_<ins|upd|del> with static SQL (plpgsql keeps the plan per
-- connection; dynamic EXECUTE cost ~0.8 ms per statement). The template stays generic:
-- only kind and ID expression (column/FK) come from paperrust_sync_tables() (ARCHITECTURE-V2 §4.7).
-- Transition tables are called paperrust_n (NEW) and paperrust_o (OLD).
-- ---------------------------------------------------------------------------------------------

-- Own data model (docs/DATAMODEL-V2.md, pl_store::datamodel): Django name → native table,
-- as soon as the conversion stage has created it (otherwise the Django name). Same version in
-- crates/pl-store/migrations/dm1_paperrust_document.sql (test pl_store::datamodel::tests).
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

-- Captured tables (name, kind, ID expression, flag 'owner' = UPDATE of owner_id is a
-- permission change). Document versions (root_document_id) count as a change of the root document.
-- Name = native table if the own data model already has it (paperrust_native_table).
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

-- Body of a capture function. p_src returns (kind, local_id, perm) per affected row.
CREATE OR REPLACE FUNCTION paperrust_sync_capture_body(p_src text) RETURNS text
LANGUAGE sql IMMUTABLE AS $fn$
    SELECT format($b$
DECLARE
    n bigint;
BEGIN
    WITH agg AS (
        SELECT kind, local_id, bool_or(perm) AS perm FROM (%s) s
        WHERE local_id IS NOT NULL GROUP BY kind, local_id
    ), obj AS (
        -- sorted: row locks always in the same order (no deadlock between
        -- two writers that capture the same objects)
        INSERT INTO paperrust_sync_object (kind, local_id)
        SELECT kind, local_id FROM agg ORDER BY kind, local_id
        ON CONFLICT (kind, local_id) DO UPDATE
            SET rev = paperrust_sync_object.rev + 1, deleted_at = NULL
    )
    INSERT INTO paperrust_sync_pending (kind, local_id, perm_change)
    SELECT kind, local_id, perm FROM agg;
    GET DIAGNOSTICS n = ROW_COUNT;
    IF n > 0 THEN
        PERFORM pg_notify('paperrust_sync_pending', '');
    END IF;
    RETURN NULL;
END
$b$, p_src)
$fn$;

-- Body of the perm_epoch functions (auth_user_groups, auth_group_permissions,
-- auth_user_user_permissions; auth_user on is_superuser/is_active change). p_hit = condition.
CREATE OR REPLACE FUNCTION paperrust_sync_epoch_body(p_hit text) RETURNS text
LANGUAGE sql IMMUTABLE AS $fn$
    SELECT format($b$
BEGIN
    IF %s THEN
        UPDATE paperrust_sync_meta
           SET value = to_jsonb(coalesce((value #>> '{}')::bigint, 0) + 1)
         WHERE key = 'perm_epoch';
        PERFORM pg_notify('paperrust_sync_pending', '');
    END IF;
    RETURN NULL;
END
$b$, p_hit)
$fn$;

CREATE OR REPLACE FUNCTION paperrust_sync_drop_triggers() RETURNS integer
LANGUAGE plpgsql AS $fn$
DECLARE
    r record;
    n integer := 0;
BEGIN
    FOR r IN
        SELECT t.tgname, c.relname
        FROM pg_trigger t
        JOIN pg_class c ON c.oid = t.tgrelid
        JOIN pg_namespace ns ON ns.oid = c.relnamespace
        WHERE NOT t.tgisinternal AND t.tgname LIKE 'paperrust\_sync\_%'
          AND ns.nspname = current_schema()
        ORDER BY c.relname, t.tgname
    LOOP
        EXECUTE format('DROP TRIGGER IF EXISTS %I ON %I', r.tgname, r.relname);
        n := n + 1;
    END LOOP;
    -- erzeugte Erfassungsfunktionen
    FOR r IN
        SELECT p.oid::regprocedure AS f FROM pg_proc p
        JOIN pg_namespace ns ON ns.oid = p.pronamespace
        WHERE p.proname LIKE 'paperrust\_sync\_cap\_%' AND ns.nspname = current_schema()
    LOOP
        EXECUTE format('DROP FUNCTION IF EXISTS %s', r.f);
    END LOOP;
    RETURN n;
END
$fn$;

CREATE OR REPLACE FUNCTION paperrust_sync_install_triggers() RETURNS integer
LANGUAGE plpgsql AS $fn$
DECLARE
    r     record;
    op    text;
    ref   text;
    src   text;
    fname text;
    n     integer := 0;
BEGIN
    PERFORM paperrust_sync_drop_triggers();
    -- (table, function kind): 'obj' = object table, 'guardian', 'epoch'
    FOR r IN
        SELECT t.tbl, t.kind, t.idexpr, t.flag, 'obj' AS mode FROM paperrust_sync_tables() t
        -- guardian only as a table (before data model stage 4); afterwards 'grant' captures the native
        -- grant table paperrust_object_grant (kind = object_kind, same numbers)
        UNION ALL SELECT x, NULL, NULL, NULL, 'guardian'
          FROM unnest(ARRAY['guardian_userobjectpermission', 'guardian_groupobjectpermission']) x
         WHERE (SELECT c.relkind FROM pg_class c WHERE c.oid = to_regclass(x)) = 'r'
        UNION ALL SELECT 'paperrust_object_grant', NULL, NULL, NULL, 'grant'
         WHERE to_regclass('public.paperrust_object_grant') IS NOT NULL
        UNION ALL SELECT x, NULL, NULL, NULL, 'epoch'
          FROM unnest(ARRAY['auth_user_groups', 'auth_group_permissions', 'auth_user_user_permissions']) x
    LOOP
        CONTINUE WHEN to_regclass(r.tbl) IS NULL;
        FOREACH op IN ARRAY ARRAY['INSERT', 'UPDATE', 'DELETE'] LOOP
            ref := CASE op WHEN 'INSERT' THEN 'NEW TABLE AS paperrust_n'
                           WHEN 'DELETE' THEN 'OLD TABLE AS paperrust_o'
                           ELSE 'NEW TABLE AS paperrust_n OLD TABLE AS paperrust_o' END;
            IF r.mode = 'obj' THEN
                src := CASE op
                    WHEN 'INSERT' THEN format(
                        'SELECT %s::smallint AS kind, (%s)::bigint AS local_id, false AS perm FROM paperrust_n',
                        r.kind, r.idexpr)
                    WHEN 'DELETE' THEN format(
                        'SELECT %s::smallint AS kind, (%s)::bigint AS local_id, false AS perm FROM paperrust_o',
                        r.kind, r.idexpr)
                    -- new and old ID (e.g. note attached to another document)
                    ELSE format(
                        'SELECT %1$s::smallint AS kind, (%2$s)::bigint AS local_id, false AS perm FROM paperrust_n
                         UNION ALL SELECT %1$s::smallint, (%2$s)::bigint, false FROM paperrust_o', r.kind, r.idexpr)
                      || CASE WHEN r.flag = 'owner' THEN format(
                        ' UNION ALL SELECT %s::smallint, (%s)::bigint, true FROM paperrust_n x
                          WHERE EXISTS (SELECT 1 FROM paperrust_o y WHERE y.id = x.id
                                        AND y.owner_id IS DISTINCT FROM x.owner_id)', r.kind, r.idexpr)
                         ELSE '' END
                END;
                src := paperrust_sync_capture_body(src);
            ELSIF r.mode = 'guardian' THEN
                -- kind from content_type_id (documents.* only), ID = object_pk (numeric only), perm
                src := paperrust_sync_capture_body(format(
                    'SELECT (CASE ct.model WHEN ''document'' THEN 1 WHEN ''tag'' THEN 2
                                 WHEN ''correspondent'' THEN 3 WHEN ''documenttype'' THEN 4
                                 WHEN ''storagepath'' THEN 5 WHEN ''customfield'' THEN 6
                                 WHEN ''savedview'' THEN 7 END)::smallint AS kind,
                            (CASE WHEN g.object_pk ~ ''^[0-9]{1,18}$'' THEN g.object_pk END)::bigint AS local_id,
                            true AS perm
                     FROM (%s) g JOIN django_content_type ct ON ct.id = g.content_type_id
                     WHERE ct.app_label = ''documents''
                       AND ct.model IN (''document'', ''tag'', ''correspondent'', ''documenttype'',
                                        ''storagepath'', ''customfield'', ''savedview'')',
                    CASE op WHEN 'INSERT' THEN 'SELECT content_type_id, object_pk FROM paperrust_n'
                            WHEN 'DELETE' THEN 'SELECT content_type_id, object_pk FROM paperrust_o'
                            ELSE 'SELECT content_type_id, object_pk FROM paperrust_n
                                  UNION ALL SELECT content_type_id, object_pk FROM paperrust_o' END));
            ELSIF r.mode = 'grant' THEN
                src := paperrust_sync_capture_body(format(
                    'SELECT g.object_kind::smallint AS kind, g.object_id::bigint AS local_id, true AS perm
                       FROM (%s) g WHERE g.object_kind BETWEEN 1 AND 7',
                    CASE op WHEN 'INSERT' THEN 'SELECT object_kind, object_id FROM paperrust_n'
                            WHEN 'DELETE' THEN 'SELECT object_kind, object_id FROM paperrust_o'
                            ELSE 'SELECT object_kind, object_id FROM paperrust_n
                                  UNION ALL SELECT object_kind, object_id FROM paperrust_o' END));
            ELSE
                src := paperrust_sync_epoch_body(CASE op
                    WHEN 'DELETE' THEN 'EXISTS (SELECT 1 FROM paperrust_o)'
                    ELSE 'EXISTS (SELECT 1 FROM paperrust_n)' END);
            END IF;
            fname := format('paperrust_sync_cap_%s_%s', r.tbl, lower(left(op, 3)));
            EXECUTE format('CREATE FUNCTION %I() RETURNS trigger LANGUAGE plpgsql AS %L', fname, src);
            EXECUTE format('CREATE TRIGGER %I AFTER %s ON %I REFERENCING %s FOR EACH STATEMENT
                            EXECUTE FUNCTION %I()',
                           'paperrust_sync_' || lower(left(op, 3)), op, r.tbl, ref, fname);
            n := n + 1;
        END LOOP;
    END LOOP;
    IF to_regclass('auth_user') IS NOT NULL THEN
        EXECUTE format('CREATE FUNCTION paperrust_sync_cap_auth_user_perm() RETURNS trigger LANGUAGE plpgsql AS %L',
            paperrust_sync_epoch_body(
                'EXISTS (SELECT 1 FROM paperrust_n x JOIN paperrust_o y ON y.id = x.id
                         WHERE y.is_superuser IS DISTINCT FROM x.is_superuser
                            OR y.is_active IS DISTINCT FROM x.is_active)'));
        CREATE TRIGGER paperrust_sync_perm AFTER UPDATE ON auth_user
            REFERENCING NEW TABLE AS paperrust_n OLD TABLE AS paperrust_o
            FOR EACH STATEMENT EXECUTE FUNCTION paperrust_sync_cap_auth_user_perm();
        n := n + 1;
    END IF;
    RETURN n;
END
$fn$;

-- ---------------------------------------------------------------------------------------------
-- Sequenzer, Neuaufbau, Aufbewahrung
-- ---------------------------------------------------------------------------------------------

-- Live objects per kind (for clients "present"). Trash and document versions
-- (root_document_id) count as not present. Usable set-based (join instead of a function per
-- row); local_id stays int4 so the primary key indexes apply.
-- (no longer needed from data model stage 5: the sequencer checks by primary key)
DO $lv$
BEGIN
    IF to_regclass('public.paperrust_sync_ident') IS NULL THEN
        CREATE OR REPLACE VIEW paperrust_sync_live AS
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
END
$lv$;
DROP FUNCTION IF EXISTS paperrust_sync_exists(smallint, bigint);

-- One pass: numbers at most p_limit pending-list rows whose transaction is certainly
-- finished (xid < xmin of the own snapshot). Returns: number of processed
-- pending-list rows, -1 if another sequencer is running. One instance at a time via the
-- transaction advisory lock; it holds until commit, so seq values become visible in
-- commit order (every new run only assigns larger numbers).
CREATE OR REPLACE FUNCTION paperrust_sync_sequence(p_limit integer DEFAULT 50000) RETURNS integer
LANGUAGE plpgsql AS $fn$
DECLARE
    v_xmin  xid8;
    n_ready integer;
BEGIN
    IF NOT pg_try_advisory_xact_lock(hashtext('paperrust_sync_sequencer')) THEN
        RETURN -1;
    END IF;
    -- own statement: the following statement has a newer snapshot in which all
    -- xid < v_xmin are finished (and thus, if committed, visible).
    v_xmin := pg_snapshot_xmin(pg_current_snapshot());

    WITH ready AS (
        DELETE FROM paperrust_sync_pending
        WHERE id IN (SELECT id FROM paperrust_sync_pending WHERE xid < v_xmin
                     ORDER BY id LIMIT p_limit)
        RETURNING kind, local_id, perm_change
    ), agg AS (
        SELECT kind, local_id, bool_or(perm_change) AS perm FROM ready GROUP BY kind, local_id
    ), ins AS (
        INSERT INTO paperrust_sync_log AS l (seq, kind, local_id, uuid, op, rev, perm_seq, at)
        SELECT q.seq, q.kind, q.local_id, q.uuid, q.op, q.rev, CASE WHEN q.perm THEN q.seq END, now()
        FROM (SELECT nextval('paperrust_sync_seq') AS seq, s.*
              FROM (SELECT a.kind, a.local_id, o.uuid, o.rev, a.perm,
                           CASE WHEN lv.local_id IS NOT NULL THEN 'u' ELSE 'd' END AS op
                    FROM agg a
                    JOIN paperrust_sync_object o ON o.kind = a.kind AND o.local_id = a.local_id
                    LEFT JOIN paperrust_sync_live lv ON lv.kind = a.kind AND lv.local_id = a.local_id
                    ORDER BY a.kind, a.local_id) s) q
        ON CONFLICT (kind, local_id) DO UPDATE
            SET seq = EXCLUDED.seq, uuid = EXCLUDED.uuid, op = EXCLUDED.op, rev = EXCLUDED.rev,
                -- an earlier permission change is kept (client with an older cursor)
                perm_seq = coalesce(EXCLUDED.perm_seq, l.perm_seq), at = EXCLUDED.at
        RETURNING l.kind, l.local_id, l.seq, l.op
    ), upd AS (
        UPDATE paperrust_sync_object o
           SET seq = x.seq,
               deleted_at = CASE WHEN x.op = 'd' THEN coalesce(o.deleted_at, now()) END
          FROM (SELECT o2.kind, o2.local_id, i.seq, i.op
                  FROM paperrust_sync_object o2
                  JOIN ins i ON i.kind = o2.kind AND i.local_id = o2.local_id
                 ORDER BY o2.kind, o2.local_id
                 -- never wait for writers (no deadlock with Django/Rust transactions);
                 -- a locked row has a pending-list entry itself and comes up in the
                 -- next run
                   FOR UPDATE OF o2 SKIP LOCKED) x
         WHERE o.kind = x.kind AND o.local_id = x.local_id
    )
    SELECT count(*) INTO n_ready FROM ready;
    IF n_ready > 0 THEN
        PERFORM pg_notify('paperrust_sync', '');
    END IF;
    RETURN n_ready;
END
$fn$;

-- Capture the existing data completely: paperrust_sync_object for every existing row, discard the pending list
-- (finished transactions), give every object a new log entry
-- (tombstones whose object is still missing stay unchanged). Increases perm_epoch, because
-- permission changes from a time without triggers are not known individually. Returns: number of
-- newly written log entries.
CREATE OR REPLACE FUNCTION paperrust_sync_rebuild() RETURNS bigint
LANGUAGE plpgsql AS $fn$
DECLARE
    v_xmin xid8;
    n      bigint;
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('paperrust_sync_sequencer'));
    v_xmin := pg_snapshot_xmin(pg_current_snapshot());
    INSERT INTO paperrust_sync_object (kind, local_id)
    SELECT kind, local_id FROM paperrust_sync_live ORDER BY kind, local_id
    ON CONFLICT DO NOTHING;

    DELETE FROM paperrust_sync_pending WHERE xid < v_xmin;

    WITH src AS (
        SELECT o.kind, o.local_id, o.uuid, o.rev,
               CASE WHEN lv.local_id IS NOT NULL THEN 'u' ELSE 'd' END AS op
        FROM paperrust_sync_object o
        LEFT JOIN paperrust_sync_live lv ON lv.kind = o.kind AND lv.local_id = o.local_id
    ), ins AS (
        INSERT INTO paperrust_sync_log AS l (seq, kind, local_id, uuid, op, rev, perm_seq, at)
        SELECT q.seq, q.kind, q.local_id, q.uuid, q.op, q.rev, q.seq, now()
        FROM (SELECT nextval('paperrust_sync_seq') AS seq, t.*
              FROM (SELECT s.* FROM src s
                    LEFT JOIN paperrust_sync_log l2 ON l2.kind = s.kind AND l2.local_id = s.local_id
                    WHERE NOT (s.op = 'd' AND l2.op IS NOT DISTINCT FROM 'd')
                    ORDER BY s.kind, s.local_id) t) q
        ON CONFLICT (kind, local_id) DO UPDATE
            SET seq = EXCLUDED.seq, uuid = EXCLUDED.uuid, op = EXCLUDED.op, rev = EXCLUDED.rev,
                perm_seq = EXCLUDED.perm_seq, at = EXCLUDED.at
        RETURNING l.kind, l.local_id, l.seq, l.op
    ), upd AS (
        UPDATE paperrust_sync_object o
           SET seq = x.seq,
               deleted_at = CASE WHEN x.op = 'd' THEN coalesce(o.deleted_at, now()) END
          FROM (SELECT o2.kind, o2.local_id, i.seq, i.op
                  FROM paperrust_sync_object o2
                  JOIN ins i ON i.kind = o2.kind AND i.local_id = o2.local_id
                 ORDER BY o2.kind, o2.local_id
                   FOR UPDATE OF o2 SKIP LOCKED) x
         WHERE o.kind = x.kind AND o.local_id = x.local_id
    )
    SELECT count(*) INTO n FROM ins;

    UPDATE paperrust_sync_meta
       SET value = to_jsonb(coalesce((value #>> '{}')::bigint, 0) + 1)
     WHERE key = 'perm_epoch';
    PERFORM pg_notify('paperrust_sync', '');
    RETURN n;
END
$fn$;

-- Delete tombstones older than p_days days; min_cursor = largest deleted seq (cursor below it
-- → 410 cursor_expired). Returns: number of deleted tombstones.
CREATE OR REPLACE FUNCTION paperrust_sync_retention(p_days integer) RETURNS integer
LANGUAGE plpgsql AS $fn$
DECLARE
    n  integer;
    mx bigint;
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('paperrust_sync_sequencer'));
    WITH del AS (
        DELETE FROM paperrust_sync_log
        WHERE op = 'd' AND at < now() - make_interval(days => p_days)
        RETURNING kind, local_id, seq
    ), obj AS (
        DELETE FROM paperrust_sync_object o
         USING (SELECT o2.kind, o2.local_id
                  FROM paperrust_sync_object o2
                  JOIN del d ON d.kind = o2.kind AND d.local_id = o2.local_id
                 WHERE o2.deleted_at IS NOT NULL
                   FOR UPDATE OF o2 SKIP LOCKED) x
         WHERE o.kind = x.kind AND o.local_id = x.local_id
    )
    SELECT count(*), max(seq) INTO n, mx FROM del;
    IF mx IS NOT NULL THEN
        UPDATE paperrust_sync_meta
           SET value = to_jsonb(greatest(coalesce((value #>> '{}')::bigint, 0), mx))
         WHERE key = 'min_cursor';
    END IF;
    RETURN n;
END
$fn$;

-- ---------------------------------------------------------------------------------------------
-- Enable (default) and initial fill
-- ---------------------------------------------------------------------------------------------
DO $do$
BEGIN
    -- from data model stage 5, pl_sync 0003 (functions of both states) takes over the enabling
    IF coalesce((SELECT value FROM paperrust_sync_meta WHERE key = 'enabled'), 'true'::jsonb)
       = 'true'::jsonb AND to_regclass('public.paperrust_sync_ident') IS NULL THEN
        PERFORM paperrust_sync_install_triggers();
        IF NOT EXISTS (SELECT 1 FROM paperrust_sync_log) THEN
            PERFORM paperrust_sync_rebuild();
        END IF;
    END IF;
END
$do$;

-- Remove the draft (generic functions with EXECUTE) if present from test setups.
DROP FUNCTION IF EXISTS paperrust_sync_capture();
DROP FUNCTION IF EXISTS paperrust_sync_capture_guardian();
DROP FUNCTION IF EXISTS paperrust_sync_bump_perm_epoch();
