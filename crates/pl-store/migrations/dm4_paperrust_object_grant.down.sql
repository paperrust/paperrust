-- Rollback of stage 4 "permissions" (dm4_paperrust_object_grant.sql): the guardian tables and the L5 table
-- paperrust_grant become tables again (filled from the views, same ids and sequence states),
-- L5 maintenance triggers and search triggers as before the stage. Call: `paperrust manage datamodel rollback --to 3`.

SET LOCAL synchronize_seqscans = off;

DO $mig$
DECLARE
    sync_on   boolean;
    search_on boolean;
BEGIN
    IF to_regclass('public.paperrust_object_grant') IS NULL THEN
        RETURN;
    END IF;
    sync_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.paperrust_object_grant'::regclass AND tgname = 'paperrust_sync_upd');
    search_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.paperrust_object_grant'::regclass AND tgname = 'paperrust_search_grant');
    IF sync_on THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;

    TRUNCATE ngx_legacy.guardian_userobjectpermission, ngx_legacy.guardian_groupobjectpermission;
    INSERT INTO ngx_legacy.guardian_userobjectpermission (id, object_pk, content_type_id, permission_id, user_id)
    SELECT id, object_pk, content_type_id, permission_id, user_id FROM public.guardian_userobjectpermission ORDER BY id;
    INSERT INTO ngx_legacy.guardian_groupobjectpermission (id, object_pk, content_type_id, group_id, permission_id)
    SELECT id, object_pk, content_type_id, group_id, permission_id FROM public.guardian_groupobjectpermission ORDER BY id;
    PERFORM setval('ngx_legacy.guardian_userobjectpermission_id_seq', s.last_value, s.is_called) FROM paperrust_grant_user_id_seq s;
    PERFORM setval('ngx_legacy.guardian_groupobjectpermission_id_seq', s.last_value, s.is_called) FROM paperrust_grant_group_id_seq s;

    DROP VIEW public.guardian_userobjectpermission;
    DROP VIEW public.guardian_groupobjectpermission;
    DROP VIEW public.paperrust_grant;
    ALTER TABLE ngx_legacy.guardian_userobjectpermission SET SCHEMA public;
    ALTER TABLE ngx_legacy.guardian_groupobjectpermission SET SCHEMA public;
    IF to_regclass('ngx_legacy.paperrust_grant') IS NOT NULL THEN
        ALTER TABLE ngx_legacy.paperrust_grant SET SCHEMA public;
        CREATE INDEX IF NOT EXISTS paperrust_grant_object ON paperrust_grant (object_id, ct) INCLUDE (perm_id, subject);
    END IF;
    DROP TABLE paperrust_object_grant, paperrust_grant_raw;
    DROP SEQUENCE paperrust_grant_user_id_seq, paperrust_grant_group_id_seq;

    -- L5 as before the stage (index anew from guardian, maintenance triggers if switched on)
    IF to_regclass('public.paperrust_grant_meta') IS NOT NULL AND to_regproc('paperrust_grant_rebuild') IS NOT NULL THEN
        IF (SELECT value FROM paperrust_grant_meta WHERE key = 'enabled') = 'true'::jsonb THEN
            PERFORM paperrust_grant_install();
        END IF;
        PERFORM paperrust_grant_rebuild();
    END IF;
    IF search_on THEN
        CREATE TRIGGER paperrust_search_uop AFTER UPDATE OR DELETE ON guardian_userobjectpermission
            FOR EACH ROW EXECUTE FUNCTION paperrust_search_guardian_trg();
        CREATE TRIGGER paperrust_search_gop AFTER UPDATE OR DELETE ON guardian_groupobjectpermission
            FOR EACH ROW EXECUTE FUNCTION paperrust_search_guardian_trg();
        CREATE TRIGGER paperrust_search_uop_trunc BEFORE TRUNCATE ON guardian_userobjectpermission
            FOR EACH STATEMENT EXECUTE FUNCTION paperrust_search_guardian_trunc_trg();
        CREATE TRIGGER paperrust_search_gop_trunc BEFORE TRUNCATE ON guardian_groupobjectpermission
            FOR EACH STATEMENT EXECUTE FUNCTION paperrust_search_guardian_trunc_trg();
    END IF;
    IF sync_on THEN
        PERFORM paperrust_sync_install_triggers();
    END IF;
END
$mig$;

DROP TRIGGER IF EXISTS paperrust_grant_perm_refresh ON auth_permission;
DROP TRIGGER IF EXISTS paperrust_grant_perm_refresh ON django_content_type;
DROP TABLE IF EXISTS paperrust_grant_perm;
DROP FUNCTION IF EXISTS paperrust_grant_perm_refresh(), paperrust_grant_perm_refresh_now();
DROP FUNCTION IF EXISTS paperrust_guardian_view_trg(), 
    paperrust_grant_remove(bigint, integer, text, integer, integer), paperrust_grant_add(bigint, integer, text, integer, integer),
    paperrust_search_grant_trg(), paperrust_search_grant_trunc_trg();
ANALYZE guardian_userobjectpermission;
ANALYZE guardian_groupobjectpermission;
