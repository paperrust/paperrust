-- PaperRust own data model, stage 4 "permissions" (docs/DATAMODEL-V2.md §1.8, §6).
-- Name in paperrust_migrations: pl_store.dm4_paperrust_object_grant. Rollback: dm4_paperrust_object_grant.down.sql.
-- Contract: docs/contract/pl-store.md "Stage 4". Presupposes stages 1–3.
--
-- Grants become the truth in an own, lossless form:
--   paperrust_object_grant (object_kind, object_id, subject, perms, ids)
--     object_kind  kinds as in SYNC.md (1 document … 7 saved_view)
--     subject      > 0 user, < 0 group (as L5)
--     perms        bitmask 1 view, 2 change, 4 delete, 8 add (= guardian codenames of the model)
--     ids          guardian row ids per bit (position 1 view, 2 change, 3 delete, 4 add) – export,
--                  audit and rollback keep the ids
--   paperrust_grant_raw: guardian rows that do not fit this grid (foreign model, permission
--     of another model, non-canonical object_pk) – unchanged, only for export/rollback.
-- guardian_userobjectpermission / guardian_groupobjectpermission become views with exactly the
-- v3.4.2 columns (rows per set bit + raw rows, ordered by id) and INSTEAD-OF triggers;
-- paperrust_grant (L5 derivation) becomes a view of the same shape (perm_id, subject, object_id, ct) –
-- the read paths of pl-authz stay, the L5 maintenance triggers are dropped. The old tables remain as
-- empty templates in ngx_legacy. Sync and search index capture attach to paperrust_object_grant.

SET LOCAL synchronize_seqscans = off;

-- permission → (kind, bit, position). Small table instead of a view: this way the planner knows its size
-- (≈ 28 rows) and runs grant queries via the indexes instead of over all grants; maintained
-- by triggers on auth_permission and django_content_type (new permissions apply immediately).
CREATE TABLE IF NOT EXISTS paperrust_grant_perm (
    perm_id integer  PRIMARY KEY,
    ct      integer  NOT NULL,
    kind    smallint NOT NULL,
    bit     smallint NOT NULL,
    pos     integer  NOT NULL,
    UNIQUE (kind, bit)
);
CREATE OR REPLACE FUNCTION paperrust_grant_perm_refresh_now() RETURNS void
LANGUAGE sql AS $fn$
    DELETE FROM paperrust_grant_perm;
    INSERT INTO paperrust_grant_perm (perm_id, ct, kind, bit, pos)
    SELECT p.id, p.content_type_id, k.kind, b.bit, b.pos
      FROM auth_permission p
      JOIN django_content_type c ON c.id = p.content_type_id AND c.app_label = 'documents'
      JOIN (VALUES ('document', 1::smallint), ('tag', 2::smallint), ('correspondent', 3::smallint),
                   ('documenttype', 4::smallint), ('storagepath', 5::smallint), ('customfield', 6::smallint),
                   ('savedview', 7::smallint)) k(model, kind) ON k.model = c.model
      JOIN (VALUES ('view', 1::smallint, 1), ('change', 2::smallint, 2), ('delete', 4::smallint, 3), ('add', 8::smallint, 4))
           b(verb, bit, pos) ON p.codename = b.verb || '_' || c.model;
$fn$;
CREATE OR REPLACE FUNCTION paperrust_grant_perm_refresh() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    PERFORM paperrust_grant_perm_refresh_now();
    RETURN NULL;
END
$fn$;



-- add a guardian row (subject with sign). If the same grant already exists
-- (guardian UNIQUE (subject, permission, object_pk)), nothing is inserted and false is returned –
-- like `bulk_create(ignore_conflicts=True)` / `INSERT … ON CONFLICT DO NOTHING`, the only
-- ways of writing used by the PaperRust writers (ON CONFLICT does not act on a view by itself).
DROP FUNCTION IF EXISTS paperrust_grant_add(integer, integer, text, integer, integer);
DROP FUNCTION IF EXISTS paperrust_grant_add(bigint, integer, text, integer, integer);
CREATE OR REPLACE FUNCTION paperrust_grant_add(p_subject bigint, p_id integer, p_object_pk text, p_ct integer, p_perm integer)
RETURNS boolean LANGUAGE plpgsql AS $fn$
DECLARE
    m   record;
    key bigint := paperrust_grant_key(p_object_pk);
    ids integer[];
BEGIN
    SELECT kind, bit, pos INTO m FROM paperrust_grant_perm WHERE perm_id = p_perm AND ct = p_ct;
    IF FOUND AND key IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM paperrust_object_grant g WHERE g.object_kind = m.kind AND g.object_id = key
                      AND g.subject = p_subject AND g.perms & m.bit <> 0) THEN
            RETURN false;
        END IF;
        ids := array_fill(NULL::integer, ARRAY[4]);
        ids[m.pos] := p_id;
        INSERT INTO paperrust_object_grant AS g (object_kind, object_id, subject, perms, ids)
        VALUES (m.kind, key, p_subject, m.bit, ids)
        ON CONFLICT (object_kind, object_id, subject) DO UPDATE
            SET perms = g.perms | EXCLUDED.perms, ids[m.pos] = p_id;
    ELSE
        INSERT INTO paperrust_grant_raw (id, subject, object_pk, content_type_id, permission_id)
        VALUES (p_id, p_subject, p_object_pk, p_ct, p_perm)
        ON CONFLICT (subject, permission_id, object_pk) DO NOTHING;
        RETURN FOUND;
    END IF;
    RETURN true;
END
$fn$;

-- remove a guardian row
CREATE OR REPLACE FUNCTION paperrust_grant_remove(p_subject bigint, p_id integer, p_object_pk text, p_ct integer, p_perm integer)
RETURNS void LANGUAGE plpgsql AS $fn$
DECLARE
    m   record;
    key bigint := paperrust_grant_key(p_object_pk);
BEGIN
    SELECT kind, bit, pos INTO m FROM paperrust_grant_perm WHERE perm_id = p_perm AND ct = p_ct;
    IF FOUND AND key IS NOT NULL THEN
        -- last bit: delete the row, otherwise clear the bit
        DELETE FROM paperrust_object_grant
         WHERE object_kind = m.kind AND object_id = key AND subject = p_subject AND ids[m.pos] = p_id AND perms = m.bit;
        IF NOT FOUND THEN
            UPDATE paperrust_object_grant SET perms = perms & ~m.bit, ids[m.pos] = NULL
             WHERE object_kind = m.kind AND object_id = key AND subject = p_subject AND ids[m.pos] = p_id;
        END IF;
    ELSE
        DELETE FROM paperrust_grant_raw WHERE id = p_id AND (subject < 0) = (p_subject < 0);
    END IF;
END
$fn$;

CREATE OR REPLACE FUNCTION paperrust_guardian_view_trg() RETURNS trigger
LANGUAGE plpgsql AS $fn$
DECLARE
    grp     boolean := TG_ARGV[0] = 'group';
    o_subj  bigint;
    n_subj  bigint;
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        o_subj := CASE WHEN grp THEN -(to_jsonb(OLD) ->> 'group_id')::integer ELSE (to_jsonb(OLD) ->> 'user_id')::integer END;
        PERFORM paperrust_grant_remove(o_subj, OLD.id, OLD.object_pk, OLD.content_type_id, OLD.permission_id);
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    IF NEW.id IS NULL THEN
        NEW.id := nextval(CASE WHEN grp THEN 'paperrust_grant_group_id_seq' ELSE 'paperrust_grant_user_id_seq' END);
    END IF;
    n_subj := CASE WHEN grp THEN -(to_jsonb(NEW) ->> 'group_id')::integer ELSE (to_jsonb(NEW) ->> 'user_id')::integer END;
    IF NOT paperrust_grant_add(n_subj, NEW.id, NEW.object_pk, NEW.content_type_id, NEW.permission_id) THEN
        RETURN NULL;  -- already present: not inserted
    END IF;
    RETURN NEW;
END
$fn$;

-- Search index: grant of a document changed/revoked → re-index the document
CREATE OR REPLACE FUNCTION paperrust_search_grant_trg() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    IF OLD.object_kind = 1 THEN
        PERFORM paperrust_search_mark(OLD.object_id);
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.object_kind = 1 THEN
        PERFORM paperrust_search_mark(NEW.object_id);
    END IF;
    RETURN NULL;
END
$fn$;

CREATE OR REPLACE FUNCTION paperrust_search_grant_trunc_trg() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    INSERT INTO paperrust_search_dirty (doc_id, seq)
    SELECT k, nextval('paperrust_search_dirty_seq') FROM (SELECT DISTINCT g.object_id AS k FROM paperrust_object_grant g WHERE g.object_kind = 1) s
    ON CONFLICT (doc_id) DO UPDATE SET seq = EXCLUDED.seq, marked = now();
    RETURN NULL;
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

DO $mig$
DECLARE
    sync_on   boolean;
    search_on boolean;
BEGIN
    IF to_regclass('public.paperrust_object_grant') IS NOT NULL THEN
        RETURN;  -- already converted
    END IF;
    IF to_regclass('public.paperrust_field_value') IS NULL THEN
        RAISE EXCEPTION 'stage 4 needs stages 1-3';
    END IF;
    sync_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.guardian_userobjectpermission'::regclass
                                                  AND tgname = 'paperrust_sync_upd');
    search_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.guardian_userobjectpermission'::regclass
                                                    AND tgname = 'paperrust_search_uop');
    IF sync_on THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;
    -- L5 maintenance triggers and search triggers of the guardian tables are dropped
    IF to_regproc('paperrust_grant_drop_triggers') IS NOT NULL THEN
        PERFORM paperrust_grant_drop_triggers();
    END IF;
    DROP TRIGGER IF EXISTS paperrust_search_uop ON guardian_userobjectpermission;
    DROP TRIGGER IF EXISTS paperrust_search_gop ON guardian_groupobjectpermission;
    DROP TRIGGER IF EXISTS paperrust_search_uop_trunc ON guardian_userobjectpermission;
    DROP TRIGGER IF EXISTS paperrust_search_gop_trunc ON guardian_groupobjectpermission;

    PERFORM paperrust_grant_perm_refresh_now();
    CREATE TRIGGER paperrust_grant_perm_refresh AFTER INSERT OR UPDATE OR DELETE ON auth_permission
        FOR EACH STATEMENT EXECUTE FUNCTION paperrust_grant_perm_refresh();
    CREATE TRIGGER paperrust_grant_perm_refresh AFTER INSERT OR UPDATE OR DELETE ON django_content_type
        FOR EACH STATEMENT EXECUTE FUNCTION paperrust_grant_perm_refresh();
    ANALYZE paperrust_grant_perm;

    CREATE TABLE paperrust_object_grant (
        object_kind smallint  NOT NULL,
        object_id   bigint    NOT NULL,
        subject     bigint    NOT NULL,
        perms       smallint  NOT NULL CHECK (perms > 0),
        ids         integer[] NOT NULL,
        PRIMARY KEY (object_kind, object_id, subject)
    );
    CREATE TABLE paperrust_grant_raw (
        id              integer      NOT NULL,
        subject         bigint       NOT NULL,
        object_pk       varchar(255) NOT NULL,
        content_type_id integer      NOT NULL,
        permission_id   integer      NOT NULL
    );
    CREATE SEQUENCE paperrust_grant_user_id_seq AS integer;
    CREATE SEQUENCE paperrust_grant_group_id_seq AS integer;
    PERFORM setval('paperrust_grant_user_id_seq', s.last_value, s.is_called) FROM guardian_userobjectpermission_id_seq s;
    PERFORM setval('paperrust_grant_group_id_seq', s.last_value, s.is_called) FROM guardian_groupobjectpermission_id_seq s;

    WITH g AS (
        SELECT u.id, u.user_id AS subject, u.object_pk, u.content_type_id, u.permission_id FROM guardian_userobjectpermission u
        UNION ALL
        SELECT x.id, -x.group_id, x.object_pk, x.content_type_id, x.permission_id FROM guardian_groupobjectpermission x
    ), mapped AS (
        SELECT g.*, m.kind, m.bit, m.pos, paperrust_grant_key(g.object_pk) AS key
          FROM g LEFT JOIN paperrust_grant_perm m ON m.perm_id = g.permission_id AND m.ct = g.content_type_id
    ), fit AS (
        INSERT INTO paperrust_object_grant (object_kind, object_id, subject, perms, ids)
        SELECT kind, key, subject, bit_or(bit)::smallint,
               ARRAY[max(id) FILTER (WHERE pos = 1), max(id) FILTER (WHERE pos = 2),
                     max(id) FILTER (WHERE pos = 3), max(id) FILTER (WHERE pos = 4)]
          FROM mapped WHERE kind IS NOT NULL AND key IS NOT NULL
         GROUP BY kind, key, subject
        RETURNING 1
    )
    INSERT INTO paperrust_grant_raw (id, subject, object_pk, content_type_id, permission_id)
    SELECT id, subject, object_pk, content_type_id, permission_id FROM mapped WHERE kind IS NULL OR key IS NULL;

    CREATE INDEX paperrust_object_grant_subject ON paperrust_object_grant (subject, object_kind, object_id) INCLUDE (perms);
    -- guardian readers filter via object_pk (text): expression index over the same conversion as the view
    CREATE INDEX paperrust_object_grant_pk_text ON paperrust_object_grant (((object_id::text)::varchar(255)), object_kind);
    CREATE UNIQUE INDEX paperrust_grant_raw_id ON paperrust_grant_raw (id, (subject < 0));
    CREATE UNIQUE INDEX paperrust_grant_raw_unique ON paperrust_grant_raw (subject, permission_id, object_pk);
    CREATE INDEX paperrust_grant_raw_key ON paperrust_grant_raw ((paperrust_grant_key(object_pk)));

    -- old tables as empty templates
    CREATE SCHEMA IF NOT EXISTS ngx_legacy;
    ALTER TABLE guardian_userobjectpermission SET SCHEMA ngx_legacy;
    ALTER TABLE guardian_groupobjectpermission SET SCHEMA ngx_legacy;
    TRUNCATE ngx_legacy.guardian_userobjectpermission, ngx_legacy.guardian_groupobjectpermission;
    IF to_regclass('public.paperrust_grant') IS NOT NULL THEN
        ALTER TABLE paperrust_grant SET SCHEMA ngx_legacy;
        TRUNCATE ngx_legacy.paperrust_grant;
        DROP INDEX IF EXISTS ngx_legacy.paperrust_grant_object;
    END IF;

    -- compatibility views (v3.4.2 columns; without a fixed order like the Django tables, the
    -- export sorts the guardian records by id itself)
    CREATE VIEW guardian_userobjectpermission AS
    SELECT x.id, x.object_pk, x.content_type_id, x.permission_id, x.user_id FROM (
        SELECT g.ids[m.pos] AS id, (g.object_id::text)::varchar(255) AS object_pk, m.ct AS content_type_id,
               m.perm_id AS permission_id, g.subject::integer AS user_id
          FROM paperrust_object_grant g JOIN paperrust_grant_perm m ON m.kind = g.object_kind AND g.perms & m.bit <> 0
         WHERE g.subject > 0
        UNION ALL
        SELECT r.id, r.object_pk, r.content_type_id, r.permission_id, r.subject::integer FROM paperrust_grant_raw r WHERE r.subject > 0
    ) x;
    CREATE VIEW guardian_groupobjectpermission AS
    SELECT x.id, x.object_pk, x.content_type_id, x.group_id, x.permission_id FROM (
        SELECT g.ids[m.pos] AS id, (g.object_id::text)::varchar(255) AS object_pk, m.ct AS content_type_id,
               (-g.subject)::integer AS group_id, m.perm_id AS permission_id
          FROM paperrust_object_grant g JOIN paperrust_grant_perm m ON m.kind = g.object_kind AND g.perms & m.bit <> 0
         WHERE g.subject < 0
        UNION ALL
        SELECT r.id, r.object_pk, r.content_type_id, (-r.subject)::integer, r.permission_id FROM paperrust_grant_raw r WHERE r.subject < 0
    ) x;
    ALTER VIEW guardian_userobjectpermission ALTER COLUMN id SET DEFAULT nextval('paperrust_grant_user_id_seq'::regclass);
    ALTER VIEW guardian_groupobjectpermission ALTER COLUMN id SET DEFAULT nextval('paperrust_grant_group_id_seq'::regclass);
    CREATE TRIGGER paperrust_guardian_view INSTEAD OF INSERT OR UPDATE OR DELETE ON guardian_userobjectpermission
        FOR EACH ROW EXECUTE FUNCTION paperrust_guardian_view_trg('user');
    CREATE TRIGGER paperrust_guardian_view INSTEAD OF INSERT OR UPDATE OR DELETE ON guardian_groupobjectpermission
        FOR EACH ROW EXECUTE FUNCTION paperrust_guardian_view_trg('group');

    -- L5 form for the read paths of pl-authz (same columns as the former table)
    CREATE VIEW paperrust_grant AS
        SELECT m.perm_id, g.subject, g.object_id, m.ct
          FROM paperrust_object_grant g JOIN paperrust_grant_perm m ON m.kind = g.object_kind AND g.perms & m.bit <> 0
        UNION ALL
        SELECT r.permission_id, r.subject, paperrust_grant_key(r.object_pk), r.content_type_id
          FROM paperrust_grant_raw r WHERE paperrust_grant_key(r.object_pk) IS NOT NULL;
    IF to_regclass('public.paperrust_grant_meta') IS NOT NULL THEN
        UPDATE paperrust_grant_meta SET value = 'true'::jsonb WHERE key IN ('enabled', 'built');
    END IF;

    -- capture: search index and sync on the native table
    IF search_on THEN
        CREATE TRIGGER paperrust_search_grant AFTER UPDATE OR DELETE ON paperrust_object_grant
            FOR EACH ROW EXECUTE FUNCTION paperrust_search_grant_trg();
        CREATE TRIGGER paperrust_search_grant_trunc BEFORE TRUNCATE ON paperrust_object_grant
            FOR EACH STATEMENT EXECUTE FUNCTION paperrust_search_grant_trunc_trg();
    END IF;
    IF sync_on THEN
        PERFORM paperrust_sync_install_triggers();
    END IF;
END
$mig$;

ANALYZE paperrust_object_grant;
ANALYZE paperrust_grant_raw;
