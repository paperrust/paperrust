-- PaperRust L5 (ARCHITECTURE-V2 §5): grant index paperrust_grant. Crate: pl-authz.
-- Contract: docs/contract/pl-authz.md, section "L5".
--
-- paperrust_grant is a DERIVED index of the guardian object permissions: for each row of
-- guardian_userobjectpermission / guardian_groupobjectpermission with a canonical integer object_pk
-- exactly one row (perm_id, subject, object_id, ct). The Django tables remain the truth
-- (mixed operation, way back to paperless-ngx); AFTER statement triggers keep the index current in the same
-- transaction. Owner (owner_id), superuser/active and group membership are
-- NOT in it: they are read per request from the object row or auth_user/auth_user_groups
-- (rationale in the contract). The codename and model of a permission come at query time via perm_id from
-- auth_permission, so renaming permissions needs no maintenance.
--
-- * expand-only: new tables/functions/triggers with prefix paperrust_; no Django row is
--   changed, the triggers write only into paperrust_grant.
-- * idempotent (CREATE … IF NOT EXISTS, CREATE OR REPLACE, triggers re-created). Initial fill
--   as long as the index is not marked as built.
-- * Rollback: `paperrust manage grants disable --purge` or by hand
--     SELECT paperrust_grant_drop_triggers();
--     DROP TABLE paperrust_grant, paperrust_grant_meta;
--     DROP FUNCTION paperrust_grant_key(text), paperrust_grant_expected(), paperrust_grant_rebuild(),
--                   paperrust_grant_check(), paperrust_grant_install(), paperrust_grant_drop_triggers(),
--                   paperrust_grant_*_ins/upd/del/trunc();

CREATE TABLE IF NOT EXISTS paperrust_grant (
    perm_id   integer NOT NULL,  -- auth_permission.id (codename + model content type from there)
    subject   bigint  NOT NULL,  -- > 0: auth_user.id, < 0: -auth_group.id
    object_id bigint  NOT NULL,  -- guardian object_pk as a number (canonical spelling only)
    ct        integer NOT NULL,  -- guardian content_type_id of the row (not that of the permission)
    PRIMARY KEY (perm_id, subject, object_id)
);
-- Object side (check per object, grantees of a page, "shared"): object_id first.
CREATE INDEX IF NOT EXISTS paperrust_grant_object ON paperrust_grant (object_id, ct) INCLUDE (perm_id, subject);

CREATE TABLE IF NOT EXISTS paperrust_grant_meta (
    key   text  PRIMARY KEY,
    value jsonb NOT NULL
);
INSERT INTO paperrust_grant_meta (key, value) VALUES ('enabled', 'true'::jsonb), ('built', 'false'::jsonb)
ON CONFLICT (key) DO NOTHING;

-- guardian object_pk → bigint, NULL if not canonical. upstream compares
-- CAST(id AS varchar) = object_pk; only the canonical spelling of an integer can be equal.
CREATE OR REPLACE FUNCTION paperrust_grant_key(pk text) RETURNS bigint
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $fn$
    SELECT CASE WHEN pk ~ '^(0|-?[1-9][0-9]{0,17})$' THEN pk::bigint END
$fn$;

-- Full recomputation from guardian (rebuild, consistency check). Function body as a
-- string: no dependency on guardian columns (Django migrations stay free).
CREATE OR REPLACE FUNCTION paperrust_grant_expected()
RETURNS TABLE (perm_id integer, subject bigint, object_id bigint, ct integer)
LANGUAGE sql STABLE AS $fn$
    SELECT g.permission_id, g.user_id::bigint, paperrust_grant_key(g.object_pk), g.content_type_id
      FROM guardian_userobjectpermission g WHERE paperrust_grant_key(g.object_pk) IS NOT NULL
    UNION ALL
    SELECT g.permission_id, -g.group_id::bigint, paperrust_grant_key(g.object_pk), g.content_type_id
      FROM guardian_groupobjectpermission g WHERE paperrust_grant_key(g.object_pk) IS NOT NULL
$fn$;

-- Rebuild the index (locks writers on guardian until the end of the transaction). Returns: rows.
CREATE OR REPLACE FUNCTION paperrust_grant_rebuild() RETURNS bigint
LANGUAGE plpgsql AS $fn$
DECLARE
    n bigint;
BEGIN
    LOCK TABLE guardian_userobjectpermission, guardian_groupobjectpermission IN SHARE MODE;
    DELETE FROM paperrust_grant;
    INSERT INTO paperrust_grant (perm_id, subject, object_id, ct)
        SELECT e.perm_id, e.subject, e.object_id, e.ct FROM paperrust_grant_expected() e;
    GET DIAGNOSTICS n = ROW_COUNT;
    INSERT INTO paperrust_grant_meta (key, value) VALUES ('built', 'true'::jsonb)
        ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
    ANALYZE paperrust_grant;
    RETURN n;
END
$fn$;

-- Consistency: (missing, surplus, differing ct) compared with the recomputation.
CREATE OR REPLACE FUNCTION paperrust_grant_check(OUT missing bigint, OUT extra bigint)
LANGUAGE sql STABLE AS $fn$
    SELECT
        (SELECT count(*) FROM (SELECT * FROM paperrust_grant_expected()
                               EXCEPT ALL SELECT perm_id, subject, object_id, ct FROM paperrust_grant) a),
        (SELECT count(*) FROM (SELECT perm_id, subject, object_id, ct FROM paperrust_grant
                               EXCEPT ALL SELECT * FROM paperrust_grant_expected()) b)
$fn$;

-- ------------------------------------------------------------------------ Trigger functions
-- Static statement triggers with transition tables (like pl-sync P2): paperrust_n = NEW,
-- paperrust_o = OLD. User rows: subject = user_id, group rows: subject = -group_id.

CREATE OR REPLACE FUNCTION paperrust_grant_user_ins() RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    INSERT INTO paperrust_grant (perm_id, subject, object_id, ct)
        SELECT n.permission_id, n.user_id::bigint, paperrust_grant_key(n.object_pk), n.content_type_id
          FROM paperrust_n n WHERE paperrust_grant_key(n.object_pk) IS NOT NULL
    ON CONFLICT (perm_id, subject, object_id) DO UPDATE SET ct = EXCLUDED.ct;
    RETURN NULL;
END $fn$;

CREATE OR REPLACE FUNCTION paperrust_grant_user_del() RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    DELETE FROM paperrust_grant t USING paperrust_o o
     WHERE t.perm_id = o.permission_id AND t.subject = o.user_id::bigint
       AND t.object_id = paperrust_grant_key(o.object_pk);
    RETURN NULL;
END $fn$;

CREATE OR REPLACE FUNCTION paperrust_grant_user_upd() RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    DELETE FROM paperrust_grant t USING paperrust_o o
     WHERE t.perm_id = o.permission_id AND t.subject = o.user_id::bigint
       AND t.object_id = paperrust_grant_key(o.object_pk);
    INSERT INTO paperrust_grant (perm_id, subject, object_id, ct)
        SELECT n.permission_id, n.user_id::bigint, paperrust_grant_key(n.object_pk), n.content_type_id
          FROM paperrust_n n WHERE paperrust_grant_key(n.object_pk) IS NOT NULL
    ON CONFLICT (perm_id, subject, object_id) DO UPDATE SET ct = EXCLUDED.ct;
    RETURN NULL;
END $fn$;

CREATE OR REPLACE FUNCTION paperrust_grant_user_trunc() RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    DELETE FROM paperrust_grant WHERE subject > 0;
    RETURN NULL;
END $fn$;

CREATE OR REPLACE FUNCTION paperrust_grant_group_ins() RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    INSERT INTO paperrust_grant (perm_id, subject, object_id, ct)
        SELECT n.permission_id, -n.group_id::bigint, paperrust_grant_key(n.object_pk), n.content_type_id
          FROM paperrust_n n WHERE paperrust_grant_key(n.object_pk) IS NOT NULL
    ON CONFLICT (perm_id, subject, object_id) DO UPDATE SET ct = EXCLUDED.ct;
    RETURN NULL;
END $fn$;

CREATE OR REPLACE FUNCTION paperrust_grant_group_del() RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    DELETE FROM paperrust_grant t USING paperrust_o o
     WHERE t.perm_id = o.permission_id AND t.subject = -o.group_id::bigint
       AND t.object_id = paperrust_grant_key(o.object_pk);
    RETURN NULL;
END $fn$;

CREATE OR REPLACE FUNCTION paperrust_grant_group_upd() RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    DELETE FROM paperrust_grant t USING paperrust_o o
     WHERE t.perm_id = o.permission_id AND t.subject = -o.group_id::bigint
       AND t.object_id = paperrust_grant_key(o.object_pk);
    INSERT INTO paperrust_grant (perm_id, subject, object_id, ct)
        SELECT n.permission_id, -n.group_id::bigint, paperrust_grant_key(n.object_pk), n.content_type_id
          FROM paperrust_n n WHERE paperrust_grant_key(n.object_pk) IS NOT NULL
    ON CONFLICT (perm_id, subject, object_id) DO UPDATE SET ct = EXCLUDED.ct;
    RETURN NULL;
END $fn$;

CREATE OR REPLACE FUNCTION paperrust_grant_group_trunc() RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    DELETE FROM paperrust_grant WHERE subject < 0;
    RETURN NULL;
END $fn$;

CREATE OR REPLACE FUNCTION paperrust_grant_drop_triggers() RETURNS void LANGUAGE plpgsql AS $fn$
DECLARE
    t text;
    op text;
BEGIN
    FOREACH t IN ARRAY ARRAY['guardian_userobjectpermission', 'guardian_groupobjectpermission'] LOOP
        CONTINUE WHEN to_regclass(t) IS NULL;
        FOREACH op IN ARRAY ARRAY['ins', 'upd', 'del', 'trunc'] LOOP
            IF EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = to_regclass(t) AND tgname = 'paperrust_grant_' || op) THEN
                EXECUTE format('DROP TRIGGER %I ON %I', 'paperrust_grant_' || op, t);
            END IF;
        END LOOP;
    END LOOP;
END $fn$;

-- Create triggers (8 of them). Returns: count.
CREATE OR REPLACE FUNCTION paperrust_grant_install() RETURNS integer LANGUAGE plpgsql AS $fn$
DECLARE
    r record;
    n integer := 0;
BEGIN
    PERFORM paperrust_grant_drop_triggers();
    FOR r IN SELECT * FROM (VALUES ('guardian_userobjectpermission', 'user'),
                                   ('guardian_groupobjectpermission', 'group')) v(tbl, kind) LOOP
        CONTINUE WHEN to_regclass(r.tbl) IS NULL;
        EXECUTE format('CREATE TRIGGER paperrust_grant_ins AFTER INSERT ON %I
                        REFERENCING NEW TABLE AS paperrust_n FOR EACH STATEMENT
                        EXECUTE FUNCTION %I()', r.tbl, 'paperrust_grant_' || r.kind || '_ins');
        EXECUTE format('CREATE TRIGGER paperrust_grant_upd AFTER UPDATE ON %I
                        REFERENCING NEW TABLE AS paperrust_n OLD TABLE AS paperrust_o FOR EACH STATEMENT
                        EXECUTE FUNCTION %I()', r.tbl, 'paperrust_grant_' || r.kind || '_upd');
        EXECUTE format('CREATE TRIGGER paperrust_grant_del AFTER DELETE ON %I
                        REFERENCING OLD TABLE AS paperrust_o FOR EACH STATEMENT
                        EXECUTE FUNCTION %I()', r.tbl, 'paperrust_grant_' || r.kind || '_del');
        EXECUTE format('CREATE TRIGGER paperrust_grant_trunc AFTER TRUNCATE ON %I
                        FOR EACH STATEMENT EXECUTE FUNCTION %I()', r.tbl, 'paperrust_grant_' || r.kind || '_trunc');
        n := n + 4;
    END LOOP;
    RETURN n;
END $fn$;

-- Install: triggers (if enabled) and initial fill (if not yet built).
DO $do$
BEGIN
    IF to_regclass('guardian_userobjectpermission') IS NULL
       OR to_regclass('guardian_groupobjectpermission') IS NULL THEN
        RETURN;
    END IF;
    IF (SELECT value FROM paperrust_grant_meta WHERE key = 'enabled') = 'true'::jsonb THEN
        PERFORM paperrust_grant_install();
        IF (SELECT value FROM paperrust_grant_meta WHERE key = 'built') IS DISTINCT FROM 'true'::jsonb THEN
            PERFORM paperrust_grant_rebuild();
        END IF;
    END IF;
END
$do$;
