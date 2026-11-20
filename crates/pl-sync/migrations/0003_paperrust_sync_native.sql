-- PaperRust: sync capture for own data model stage 5 (docs/DATAMODEL-V2.md §1.9/§1.10,
-- §6 "Stufe 5"). Crate: pl-sync. The same file runs as part of pl_store.dm5_paperrust_label_sync.
--
-- All functions here detect the state themselves:
--   * before stage 5 (no table paperrust_sync_ident): as pl_sync 0001/0002 – identity (uuid, rev)
--     in the side table paperrust_sync_object;
--   * from stage 5: uuid/rev are in the row of the native table (paperrust_document,
--     paperrust_tag, paperrust_correspondent, paperrust_document_type, paperrust_storage_path,
--     paperrust_custom_field); views/users/groups (kinds 7–9, tables stay Django) in the
--     small table paperrust_sync_ident; tombstones of deleted objects in
--     paperrust_sync_tombstone. paperrust_sync_object is then a read view with the old columns
--     (kind, local_id, uuid, rev, deleted_at; local_id as integer so the primary keys apply).
--     The sequencer checks each object by primary key on the native table (no UNION view).
--
-- * idempotent: only CREATE OR REPLACE FUNCTION; no table, no data.
-- * Rollback: not needed (before stage 5 all functions behave like 0001/0002).

-- Create missing identities (before stage 5: side table; afterwards only kinds 7–9, all others
-- carry uuid/rev in the row).
CREATE OR REPLACE FUNCTION paperrust_sync_ensure(p_kind smallint, p_ids bigint[]) RETURNS void
LANGUAGE plpgsql AS $fn$
BEGIN
    IF to_regclass('public.paperrust_sync_ident') IS NULL THEN
        INSERT INTO paperrust_sync_object (kind, local_id)
        SELECT p_kind, x FROM unnest(p_ids) AS x ORDER BY x ON CONFLICT DO NOTHING;
    ELSIF p_kind >= 7 THEN
        INSERT INTO paperrust_sync_ident (kind, local_id)
        SELECT p_kind, x FROM unnest(p_ids) AS x ORDER BY x ON CONFLICT DO NOTHING;
    END IF;
END
$fn$;

-- Does paperrust_sync_ensure have to do anything at all for this kind? (as a gate condition before expensive
-- subqueries: false → the subquery does not run at all)
CREATE OR REPLACE FUNCTION paperrust_sync_ensure_needed(p_kind smallint) RETURNS boolean
LANGUAGE sql STABLE AS $fn$
    SELECT to_regclass('public.paperrust_sync_ident') IS NULL OR p_kind >= 7
$fn$;

-- Stage 5: rev + 1 per changed row of the native table (BEFORE UPDATE, per row). If the
-- statement sets rev itself (paperrust_sync_bump), it stays that way.
CREATE OR REPLACE FUNCTION paperrust_sync_rev_bump() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    IF NEW.rev IS NOT DISTINCT FROM OLD.rev THEN
        NEW.rev := OLD.rev + 1;
    END IF;
    RETURN NEW;
END
$fn$;

-- Stage 5: rev + 1 for objects whose row the statement did not change itself (tag assignment,
-- note, field value, share, version → root document). Kinds 7–9: create identity or rev + 1.
-- Sorted (locks always in the same order).
CREATE OR REPLACE FUNCTION paperrust_sync_bump(p_kind smallint, p_ids bigint[]) RETURNS void
LANGUAGE plpgsql AS $fn$
BEGIN
    IF p_ids IS NULL OR cardinality(p_ids) = 0 THEN
        RETURN;
    END IF;
    CASE p_kind
        WHEN 1 THEN UPDATE paperrust_document x SET rev = x.rev + 1
                      FROM (SELECT y.id FROM paperrust_document y WHERE y.id = ANY(p_ids) ORDER BY y.id FOR NO KEY UPDATE) s
                     WHERE x.id = s.id;
        WHEN 2 THEN UPDATE paperrust_tag x SET rev = x.rev + 1 WHERE x.id = ANY(p_ids);
        WHEN 3 THEN UPDATE paperrust_correspondent x SET rev = x.rev + 1 WHERE x.id = ANY(p_ids);
        WHEN 4 THEN UPDATE paperrust_document_type x SET rev = x.rev + 1 WHERE x.id = ANY(p_ids);
        WHEN 5 THEN UPDATE paperrust_storage_path x SET rev = x.rev + 1 WHERE x.id = ANY(p_ids);
        WHEN 6 THEN UPDATE paperrust_custom_field x SET rev = x.rev + 1 WHERE x.id = ANY(p_ids);
        ELSE
            INSERT INTO paperrust_sync_ident AS i (kind, local_id)
            SELECT p_kind, x FROM unnest(p_ids) AS x ORDER BY x
            ON CONFLICT (kind, local_id) DO UPDATE SET rev = i.rev + 1;
    END CASE;
END
$fn$;

-- Stage 5: body of a capture function. p_src returns (kind, local_id, perm) per affected
-- row (pending list), p_post additional steps (rev, tombstones).
-- Large statements (> 50 rows in the transition table p_tt) run via EXECUTE: plpgsql otherwise keeps
-- the plan of the many single-row calls of the same connection, and the owner comparison
-- (old against new rows) then runs as a nested loop (10,000 documents ≈ 13 s).
DROP FUNCTION IF EXISTS paperrust_sync_capture_body_native(text, text);
CREATE OR REPLACE FUNCTION paperrust_sync_capture_body_native(p_src text, p_post text, p_tt text) RETURNS text
LANGUAGE sql IMMUTABLE AS $fn$
    SELECT format($b$
DECLARE
    n bigint;
BEGIN
    IF (SELECT count(*) FROM (SELECT 1 FROM %3$s LIMIT 51) q) > 50 THEN
        EXECUTE %4$L;
    ELSE
        %4$s;
    END IF;
    GET DIAGNOSTICS n = ROW_COUNT;
    %2$s
    IF n > 0 THEN
        PERFORM pg_notify('paperrust_sync_pending', '');
    END IF;
    RETURN NULL;
END
$b$, p_src, p_post, p_tt, format(
    'INSERT INTO paperrust_sync_pending (kind, local_id, perm_change)
     SELECT kind, local_id, bool_or(perm) FROM (%s) s
      WHERE local_id IS NOT NULL GROUP BY kind, local_id ORDER BY kind, local_id', p_src))
$fn$;

CREATE OR REPLACE FUNCTION paperrust_sync_install_triggers() RETURNS integer
LANGUAGE plpgsql AS $fn$
DECLARE
    r      record;
    op     text;
    ref    text;
    src    text;
    post   text;
    fname  text;
    own    boolean;
    native boolean := to_regclass('public.paperrust_sync_ident') IS NOT NULL;
    n      integer := 0;
BEGIN
    PERFORM paperrust_sync_drop_triggers();
    -- (Tabelle, Funktionsart): 'obj' = Objekttabelle, 'guardian', 'epoch'
    FOR r IN
        SELECT t.tbl, t.kind, t.idexpr, t.flag, 'obj' AS mode FROM paperrust_sync_tables() t
        -- guardian only as a table (before data model stage 4); afterwards 'grant' captures the native
        -- share table paperrust_object_grant (kind = object_kind, same numbers)
        UNION ALL SELECT x, NULL, NULL, NULL, 'guardian'
          FROM unnest(ARRAY['guardian_userobjectpermission', 'guardian_groupobjectpermission']) x
         WHERE (SELECT c.relkind FROM pg_class c WHERE c.oid = to_regclass(x)) = 'r'
        UNION ALL SELECT 'paperrust_object_grant', NULL, NULL, NULL, 'grant'
         WHERE to_regclass('public.paperrust_object_grant') IS NOT NULL
        UNION ALL SELECT x, NULL, NULL, NULL, 'epoch'
          FROM unnest(ARRAY['auth_user_groups', 'auth_group_permissions', 'auth_user_user_permissions']) x
    LOOP
        CONTINUE WHEN to_regclass(r.tbl) IS NULL;
        -- Object table of the kind itself (id or root id) or dependent table (assignment, note …)
        own := r.mode = 'obj' AND r.idexpr IN ('id', 'coalesce(root_document_id, id)');
        IF native AND own AND r.kind BETWEEN 1 AND 6 THEN
            EXECUTE format('CREATE TRIGGER paperrust_sync_rev BEFORE UPDATE ON %I
                            FOR EACH ROW EXECUTE FUNCTION paperrust_sync_rev_bump()', r.tbl);
            n := n + 1;
        END IF;
        FOREACH op IN ARRAY ARRAY['INSERT', 'UPDATE', 'DELETE'] LOOP
            ref := CASE op WHEN 'INSERT' THEN 'NEW TABLE AS paperrust_n'
                           WHEN 'DELETE' THEN 'OLD TABLE AS paperrust_o'
                           ELSE 'NEW TABLE AS paperrust_n OLD TABLE AS paperrust_o' END;
            post := '';
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
                IF native THEN
                    IF NOT own THEN
                        -- dependent table: rev of the object + 1
                        post := format('PERFORM paperrust_sync_bump(b.kind, array_agg(DISTINCT b.local_id))
                                          FROM (%s) b WHERE b.local_id IS NOT NULL GROUP BY b.kind;', src);
                    ELSIF r.kind = 1 THEN
                        -- Document: rev of the own row is set by paperrust_sync_rev; versions count
                        -- as a change of the root document; deleted root documents → tombstone
                        post := format('PERFORM paperrust_sync_bump(1::smallint, array_agg(DISTINCT v.r))
                                          FROM (%s) v HAVING count(*) > 0;',
                                       CASE op WHEN 'INSERT' THEN 'SELECT root_document_id::bigint AS r FROM paperrust_n WHERE root_document_id IS NOT NULL'
                                               WHEN 'DELETE' THEN 'SELECT root_document_id::bigint AS r FROM paperrust_o WHERE root_document_id IS NOT NULL'
                                               ELSE 'SELECT root_document_id::bigint AS r FROM paperrust_n WHERE root_document_id IS NOT NULL
                                                     UNION ALL SELECT root_document_id FROM paperrust_o WHERE root_document_id IS NOT NULL' END);
                    ELSIF r.kind >= 7 THEN
                        -- View/user/group: identity in paperrust_sync_ident
                        IF op <> 'DELETE' THEN
                            post := format('PERFORM paperrust_sync_bump(%s::smallint, array_agg(DISTINCT x.id::bigint))
                                              FROM paperrust_n x HAVING count(*) > 0;', r.kind);
                        END IF;
                    END IF;
                    IF own AND op = 'DELETE' THEN
                        post := post || CASE WHEN r.kind >= 7 THEN format(
                            'WITH g AS (DELETE FROM paperrust_sync_ident i USING paperrust_o x
                                         WHERE i.kind = %1$s AND i.local_id = x.id
                                     RETURNING i.kind, i.local_id, i.uuid, i.rev)
                             INSERT INTO paperrust_sync_tombstone AS t (kind, local_id, uuid, rev)
                             SELECT kind, local_id, uuid, rev + 1 FROM g ORDER BY local_id
                             ON CONFLICT (kind, local_id) DO UPDATE
                                SET uuid = EXCLUDED.uuid, rev = EXCLUDED.rev, deleted_at = now();', r.kind)
                        ELSE format(
                            'INSERT INTO paperrust_sync_tombstone AS t (kind, local_id, uuid, rev)
                             SELECT %s::smallint, x.id, x.uuid, x.rev + 1 FROM paperrust_o x %s ORDER BY x.id
                             ON CONFLICT (kind, local_id) DO UPDATE
                                SET uuid = EXCLUDED.uuid, rev = EXCLUDED.rev, deleted_at = now();',
                            r.kind, CASE WHEN r.kind = 1 THEN 'WHERE x.root_document_id IS NULL' ELSE '' END) END;
                    ELSIF own AND op = 'INSERT' THEN
                        -- created again (same id, e.g. import): the old tombstone no longer applies
                        post := post || format(
                            'DELETE FROM paperrust_sync_tombstone t USING paperrust_n x
                              WHERE t.kind = %s AND t.local_id = x.id;', r.kind);
                    END IF;
                    src := paperrust_sync_capture_body_native(src, post, CASE WHEN op = 'DELETE' THEN 'paperrust_o' ELSE 'paperrust_n' END);
                ELSE
                    src := paperrust_sync_capture_body(src);
                END IF;
            ELSIF r.mode = 'guardian' THEN
                -- kind from content_type_id (only documents.*), ID = object_pk (numeric only), perm
                src := format(
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
                                  UNION ALL SELECT content_type_id, object_pk FROM paperrust_o' END);
                src := CASE WHEN native THEN paperrust_sync_capture_body_native(src, format(
                           'PERFORM paperrust_sync_bump(b.kind, array_agg(DISTINCT b.local_id))
                              FROM (%s) b WHERE b.local_id IS NOT NULL AND b.kind IS NOT NULL GROUP BY b.kind;', src), CASE WHEN op = 'DELETE' THEN 'paperrust_o' ELSE 'paperrust_n' END)
                            ELSE paperrust_sync_capture_body(src) END;
            ELSIF r.mode = 'grant' THEN
                src := format(
                    'SELECT g.object_kind::smallint AS kind, g.object_id::bigint AS local_id, true AS perm
                       FROM (%s) g WHERE g.object_kind BETWEEN 1 AND 7',
                    CASE op WHEN 'INSERT' THEN 'SELECT object_kind, object_id FROM paperrust_n'
                            WHEN 'DELETE' THEN 'SELECT object_kind, object_id FROM paperrust_o'
                            ELSE 'SELECT object_kind, object_id FROM paperrust_n
                                  UNION ALL SELECT object_kind, object_id FROM paperrust_o' END);
                src := CASE WHEN native THEN paperrust_sync_capture_body_native(src, format(
                           'PERFORM paperrust_sync_bump(b.kind, array_agg(DISTINCT b.local_id))
                              FROM (%s) b GROUP BY b.kind;', src), CASE WHEN op = 'DELETE' THEN 'paperrust_o' ELSE 'paperrust_n' END)
                            ELSE paperrust_sync_capture_body(src) END;
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

-- Sequencer (result like pl_sync 0002). From stage 5: identity and existence per object by
-- primary key from the native table or paperrust_sync_ident; if the row is missing, the
-- tombstone supplies uuid/rev (op 'd'). No more maintaining of a side table.
CREATE OR REPLACE FUNCTION paperrust_sync_sequence(p_limit integer DEFAULT 5000) RETURNS integer
LANGUAGE plpgsql
SET lock_timeout = '5s'
AS $fn$
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

    IF to_regclass('public.paperrust_sync_ident') IS NULL THEN
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
                           CASE WHEN CASE a.kind
                                    WHEN 1 THEN EXISTS (SELECT 1 FROM documents_document x
                                                        WHERE x.id = a.local_id AND x.deleted_at IS NULL
                                                          AND x.root_document_id IS NULL)
                                    WHEN 2 THEN EXISTS (SELECT 1 FROM documents_tag x WHERE x.id = a.local_id)
                                    WHEN 3 THEN EXISTS (SELECT 1 FROM documents_correspondent x WHERE x.id = a.local_id)
                                    WHEN 4 THEN EXISTS (SELECT 1 FROM documents_documenttype x WHERE x.id = a.local_id)
                                    WHEN 5 THEN EXISTS (SELECT 1 FROM documents_storagepath x WHERE x.id = a.local_id)
                                    WHEN 6 THEN EXISTS (SELECT 1 FROM documents_customfield x WHERE x.id = a.local_id)
                                    WHEN 7 THEN EXISTS (SELECT 1 FROM documents_savedview x WHERE x.id = a.local_id)
                                    WHEN 8 THEN EXISTS (SELECT 1 FROM auth_user x WHERE x.id = a.local_id)
                                    WHEN 9 THEN EXISTS (SELECT 1 FROM auth_group x WHERE x.id = a.local_id)
                                    ELSE false END
                                THEN 'u' ELSE 'd' END AS op
                    FROM agg a
                    JOIN paperrust_sync_object o ON o.kind = a.kind AND o.local_id = a.local_id
                    ORDER BY a.kind, a.local_id) s) q
        ON CONFLICT (kind, local_id) DO UPDATE
            SET seq = EXCLUDED.seq, uuid = EXCLUDED.uuid, op = EXCLUDED.op, rev = EXCLUDED.rev,
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
                   FOR UPDATE OF o2 SKIP LOCKED) x
         WHERE o.kind = x.kind AND o.local_id = x.local_id
    )
    SELECT count(*) INTO n_ready FROM ready;
    ELSE
    WITH ready AS (
        DELETE FROM paperrust_sync_pending
        WHERE id IN (SELECT id FROM paperrust_sync_pending WHERE xid < v_xmin
                     ORDER BY id LIMIT p_limit)
        RETURNING kind, local_id, perm_change
    ), agg AS (
        SELECT kind, local_id, bool_or(perm_change) AS perm FROM ready GROUP BY kind, local_id
    ), cand AS (
        -- one access per kind by primary key (local_id bigint, id int4: the int48 operator
        -- uses the PK index); pri 0 = live row before pri 1 = tombstone
        SELECT a.kind, a.local_id, a.perm, x.uuid, x.rev, (x.deleted_at IS NULL AND x.root_document_id IS NULL) AS live, 0 AS pri
          FROM agg a JOIN paperrust_document x ON a.kind = 1 AND x.id = a.local_id
        UNION ALL SELECT a.kind, a.local_id, a.perm, x.uuid, x.rev, true, 0
          FROM agg a JOIN paperrust_tag x ON a.kind = 2 AND x.id = a.local_id
        UNION ALL SELECT a.kind, a.local_id, a.perm, x.uuid, x.rev, true, 0
          FROM agg a JOIN paperrust_correspondent x ON a.kind = 3 AND x.id = a.local_id
        UNION ALL SELECT a.kind, a.local_id, a.perm, x.uuid, x.rev, true, 0
          FROM agg a JOIN paperrust_document_type x ON a.kind = 4 AND x.id = a.local_id
        UNION ALL SELECT a.kind, a.local_id, a.perm, x.uuid, x.rev, true, 0
          FROM agg a JOIN paperrust_storage_path x ON a.kind = 5 AND x.id = a.local_id
        UNION ALL SELECT a.kind, a.local_id, a.perm, x.uuid, x.rev, true, 0
          FROM agg a JOIN paperrust_custom_field x ON a.kind = 6 AND x.id = a.local_id
        UNION ALL SELECT a.kind, a.local_id, a.perm, i.uuid, i.rev,
                         CASE a.kind
                             WHEN 7 THEN EXISTS (SELECT 1 FROM documents_savedview x WHERE x.id = a.local_id)
                             WHEN 8 THEN EXISTS (SELECT 1 FROM auth_user x WHERE x.id = a.local_id)
                             WHEN 9 THEN EXISTS (SELECT 1 FROM auth_group x WHERE x.id = a.local_id)
                             ELSE false END, 0
          FROM agg a JOIN paperrust_sync_ident i ON i.kind = a.kind AND i.local_id = a.local_id
        UNION ALL SELECT a.kind, a.local_id, a.perm, t.uuid, t.rev, false, 1
          FROM agg a JOIN paperrust_sync_tombstone t ON t.kind = a.kind AND t.local_id = a.local_id
    ), pick AS (
        SELECT DISTINCT ON (kind, local_id) kind, local_id, perm, uuid, rev, live
          FROM cand ORDER BY kind, local_id, pri
    ), ins AS (
        INSERT INTO paperrust_sync_log AS l (seq, kind, local_id, uuid, op, rev, perm_seq, at)
        SELECT q.seq, q.kind, q.local_id, q.uuid, q.op, q.rev, CASE WHEN q.perm THEN q.seq END, now()
        FROM (SELECT nextval('paperrust_sync_seq') AS seq, s.*
              FROM (SELECT p.kind, p.local_id, p.uuid, p.rev, p.perm,
                           CASE WHEN p.live THEN 'u' ELSE 'd' END AS op
                      FROM pick p ORDER BY p.kind, p.local_id) s) q
        ON CONFLICT (kind, local_id) DO UPDATE
            SET seq = EXCLUDED.seq, uuid = EXCLUDED.uuid, op = EXCLUDED.op, rev = EXCLUDED.rev,
                -- an earlier permission change is kept (client with an older cursor)
                perm_seq = coalesce(EXCLUDED.perm_seq, l.perm_seq), at = EXCLUDED.at
        RETURNING 1
    )
    SELECT count(*) INTO n_ready FROM ready;
    END IF;
    IF n_ready > 0 THEN
        PERFORM pg_notify('paperrust_sync', '');
    END IF;
    RETURN n_ready;
END
$fn$;

-- Capture the existing data completely (as pl_sync 0001). From stage 5: create identities of kinds 7–9,
-- turn orphaned identities (object deleted without capture) into tombstones, then give every object
-- a new log entry (tombstones that are already in the feed as 'd' stay).
CREATE OR REPLACE FUNCTION paperrust_sync_rebuild() RETURNS bigint
LANGUAGE plpgsql AS $fn$
DECLARE
    v_xmin xid8;
    n      bigint;
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('paperrust_sync_sequencer'));
    v_xmin := pg_snapshot_xmin(pg_current_snapshot());
    IF to_regclass('public.paperrust_sync_ident') IS NULL THEN
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
    ELSE
        INSERT INTO paperrust_sync_ident (kind, local_id)
        SELECT k, id FROM (SELECT 7::smallint AS k, id FROM documents_savedview
                           UNION ALL SELECT 8::smallint, id FROM auth_user
                           UNION ALL SELECT 9::smallint, id FROM auth_group) x
         ORDER BY 1, 2
        ON CONFLICT DO NOTHING;
        WITH gone AS (
            DELETE FROM paperrust_sync_ident i
             WHERE NOT CASE i.kind
                       WHEN 7 THEN EXISTS (SELECT 1 FROM documents_savedview x WHERE x.id = i.local_id)
                       WHEN 8 THEN EXISTS (SELECT 1 FROM auth_user x WHERE x.id = i.local_id)
                       WHEN 9 THEN EXISTS (SELECT 1 FROM auth_group x WHERE x.id = i.local_id)
                       ELSE false END
            RETURNING i.kind, i.local_id, i.uuid, i.rev
        )
        INSERT INTO paperrust_sync_tombstone (kind, local_id, uuid, rev)
        SELECT kind, local_id, uuid, rev FROM gone ORDER BY 1, 2 ON CONFLICT DO NOTHING;

        DELETE FROM paperrust_sync_pending WHERE xid < v_xmin;

        WITH src AS (
            SELECT DISTINCT ON (o.kind, o.local_id) o.kind, o.local_id, o.uuid, o.rev,
                   CASE WHEN o.deleted_at IS NULL THEN 'u' ELSE 'd' END AS op
              FROM paperrust_sync_object o
             ORDER BY o.kind, o.local_id, o.deleted_at NULLS FIRST
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
            RETURNING 1
        )
        SELECT count(*) INTO n FROM ins;
    END IF;

    UPDATE paperrust_sync_meta
       SET value = to_jsonb(coalesce((value #>> '{}')::bigint, 0) + 1)
     WHERE key = 'perm_epoch';
    PERFORM pg_notify('paperrust_sync', '');
    RETURN n;
END
$fn$;

-- Delete tombstones older than p_days days (as pl_sync 0001); from stage 5 also the tombstone in
-- paperrust_sync_tombstone (trash documents keep their row with uuid).
CREATE OR REPLACE FUNCTION paperrust_sync_retention(p_days integer) RETURNS integer
LANGUAGE plpgsql AS $fn$
DECLARE
    n  integer;
    mx bigint;
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('paperrust_sync_sequencer'));
    IF to_regclass('public.paperrust_sync_ident') IS NULL THEN
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
    ELSE
        WITH del AS (
            DELETE FROM paperrust_sync_log
            WHERE op = 'd' AND at < now() - make_interval(days => p_days)
            RETURNING kind, local_id, seq
        ), obj AS (
            DELETE FROM paperrust_sync_tombstone t USING del d
             WHERE t.kind = d.kind AND t.local_id = d.local_id
        )
        SELECT count(*), max(seq) INTO n, mx FROM del;
    END IF;
    IF mx IS NOT NULL THEN
        UPDATE paperrust_sync_meta
           SET value = to_jsonb(greatest(coalesce((value #>> '{}')::bigint, 0), mx))
         WHERE key = 'min_cursor';
    END IF;
    RETURN n;
END
$fn$;
