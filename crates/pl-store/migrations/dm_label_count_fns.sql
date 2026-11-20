-- PaperRust own data model: functions of the label counters (docs/DATAMODEL-V2.md §1.5).
-- One version for all stages: pl_store::datamodel includes this file before dm2 (create), dm3
-- (kind 6 custom field is added) and their rollbacks (concat!). Everything CREATE OR REPLACE.
--
-- Kinds: 2 tag, 3 correspondent, 4 document type, 5 storage path, 6 custom field (only if
-- paperrust_field_value exists, i.e. from stage 3; a non-deleted field value counts).

-- Visibility owner of a document in the current state (0 = without owner or root missing).
CREATE OR REPLACE FUNCTION paperrust_doc_bucket(p_owner integer, p_root integer) RETURNS integer
LANGUAGE sql STABLE AS $fn$
    SELECT CASE WHEN p_root IS NULL THEN coalesce(p_owner, 0)
                ELSE coalesce((SELECT r.owner_id FROM paperrust_document r WHERE r.id = p_root), 0) END
$fn$;

-- Complete rebuild from the data (also after TRUNCATE).
CREATE OR REPLACE FUNCTION paperrust_label_count_rebuild() RETURNS bigint
LANGUAGE plpgsql AS $fn$
DECLARE
    n  bigint;
    n6 bigint := 0;
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('paperrust_label_count'));
    DELETE FROM paperrust_label_count_delta;
    DELETE FROM paperrust_label_count;
    WITH d AS (
        SELECT x.id, x.correspondent_id AS c, x.document_type_id AS t, x.storage_path_id AS s,
               CASE WHEN x.root_document_id IS NULL THEN coalesce(x.owner_id, 0) ELSE coalesce(r.owner_id, 0) END AS b
          FROM paperrust_document x LEFT JOIN paperrust_document r ON r.id = x.root_document_id
         WHERE x.deleted_at IS NULL
    )
    INSERT INTO paperrust_label_count (kind, label_id, owner_bucket, n)
    SELECT k.kind, k.label, d.b, count(*) FROM d
      CROSS JOIN LATERAL (VALUES (3::smallint, d.c), (4::smallint, d.t), (5::smallint, d.s)) k(kind, label)
     WHERE k.label IS NOT NULL GROUP BY 1, 2, 3
    UNION ALL
    SELECT 2::smallint, dt.tag_id, d.b, count(*) FROM d JOIN paperrust_document_tag dt ON dt.document_id = d.id
     GROUP BY 2, 3;
    GET DIAGNOSTICS n = ROW_COUNT;
    IF to_regclass('public.paperrust_field_value') IS NOT NULL THEN
        WITH d AS (
            SELECT x.id,
                   CASE WHEN x.root_document_id IS NULL THEN coalesce(x.owner_id, 0) ELSE coalesce(r.owner_id, 0) END AS b
              FROM paperrust_document x LEFT JOIN paperrust_document r ON r.id = x.root_document_id
             WHERE x.deleted_at IS NULL
        )
        INSERT INTO paperrust_label_count (kind, label_id, owner_bucket, n)
        SELECT 6::smallint, fv.field_id, d.b, count(*) FROM d JOIN paperrust_field_value fv ON fv.document_id = d.id
         WHERE fv.deleted_at IS NULL GROUP BY 2, 3;
        GET DIAGNOSTICS n6 = ROW_COUNT;
    END IF;
    RETURN n + n6;
END
$fn$;

-- Fold differences together (one instance at a time, never waits). Returns: folded delta rows,
-- -1 if another run is active.
CREATE OR REPLACE FUNCTION paperrust_label_count_fold() RETURNS integer
LANGUAGE plpgsql AS $fn$
DECLARE
    folded integer;
BEGIN
    IF NOT pg_try_advisory_xact_lock(hashtext('paperrust_label_count')) THEN
        RETURN -1;
    END IF;
    WITH del AS (
        DELETE FROM paperrust_label_count_delta RETURNING kind, label_id, owner_bucket, d
    ), agg AS (
        SELECT kind, label_id, owner_bucket, sum(d)::integer AS d, count(*) AS rows FROM del GROUP BY 1, 2, 3
    ), up AS (
        INSERT INTO paperrust_label_count AS c (kind, label_id, owner_bucket, n)
        SELECT kind, label_id, owner_bucket, d FROM agg WHERE d <> 0 ORDER BY 1, 2, 3
        ON CONFLICT (kind, label_id, owner_bucket) DO UPDATE SET n = c.n + EXCLUDED.n
        RETURNING 1
    )
    SELECT coalesce(sum(rows), 0)::integer INTO folded FROM agg;
    DELETE FROM paperrust_label_count WHERE n = 0;
    RETURN folded;
END
$fn$;

-- Trigger on paperrust_document: contribution of old and new states (correspondent, type, path, tags,
-- from stage 3 field values) per visibility owner. If the owner of a root changes (or
-- it appears/disappears), its versions move along. Generated from a template.
DROP FUNCTION IF EXISTS paperrust_label_count_doc_body(text, text);
CREATE OR REPLACE FUNCTION paperrust_label_count_doc_body(p_old text, p_new text, p_fields boolean) RETURNS text
LANGUAGE sql IMMUTABLE AS $fn$
    -- Large statements (> 50 rows) run via EXECUTE: otherwise plpgsql keeps the plan of the many
    -- single-row calls of the same connection (nested loops over the transition
    -- tables, 10,000 documents ≈ 50 s); small ones keep the retained plan (no planning time).
    SELECT format($w$
DECLARE
    n bigint;
BEGIN
    SELECT (SELECT count(*) FROM (SELECT 1 FROM (%1$s) q LIMIT 51) x)
         + (SELECT count(*) FROM (SELECT 1 FROM (%2$s) q LIMIT 51) y) INTO n;
    IF n = 0 THEN
        RETURN NULL;
    ELSIF n > 50 THEN
        EXECUTE %3$L;
    ELSE
        %3$s;
    END IF;
    RETURN NULL;
END
$w$, p_old, p_new, format($b$
    WITH o AS (%1$s), n AS (%2$s),
    os AS (
        -- Join instead of a subquery per row on o: the transition tables have no index, a
        -- subquery per row read them completely each time (10,000 documents ≈ 18 s). The document table
        -- itself stays a subquery over the primary key (executed for versions only).
        SELECT o.id, o.deleted_at IS NULL AS live,
               CASE WHEN o.root_document_id IS NULL THEN coalesce(o.owner_id, 0)
                    WHEN x.id IS NOT NULL THEN coalesce(x.owner_id, 0)
                    ELSE coalesce((SELECT coalesce(r.owner_id, 0) FROM paperrust_document r WHERE r.id = o.root_document_id), 0) END AS b,
               o.correspondent_id AS c, o.document_type_id AS t, o.storage_path_id AS s
          FROM o
          LEFT JOIN o x ON x.id = o.root_document_id
    ), ns AS (
        SELECT n.id, n.deleted_at IS NULL AS live, paperrust_doc_bucket(n.owner_id, n.root_document_id) AS b,
               n.correspondent_id AS c, n.document_type_id AS t, n.storage_path_id AS s
          FROM n
    ), roots AS (
        SELECT id FROM o WHERE root_document_id IS NULL UNION SELECT id FROM n WHERE root_document_id IS NULL
    ), rc AS (
        SELECT r.id, coalesce(o.owner_id, 0) AS ob,
               coalesce((SELECT coalesce(d.owner_id, 0) FROM paperrust_document d
                          WHERE d.id = r.id AND d.root_document_id IS NULL), 0) AS nb
          FROM roots r
          LEFT JOIN o ON o.id = r.id AND o.root_document_id IS NULL
    ), vs AS (
        SELECT v.id, v.deleted_at IS NULL AS live, rc.ob, rc.nb,
               v.correspondent_id AS c, v.document_type_id AS t, v.storage_path_id AS s
          FROM rc JOIN paperrust_document v ON v.root_document_id = rc.id
         WHERE rc.ob <> rc.nb
           AND NOT EXISTS (SELECT 1 FROM o WHERE o.id = v.id) AND NOT EXISTS (SELECT 1 FROM n WHERE n.id = v.id)
    ), st AS (
        SELECT id, live, b, c, t, s, -1 AS sign FROM os
        UNION ALL SELECT id, live, b, c, t, s, 1 FROM ns
        UNION ALL SELECT id, live, ob, c, t, s, -1 FROM vs
        UNION ALL SELECT id, live, nb, c, t, s, 1 FROM vs
    ), d AS (
        SELECT k.kind, k.label, st.b, st.sign FROM st
          CROSS JOIN LATERAL (VALUES (3::smallint, st.c), (4::smallint, st.t), (5::smallint, st.s)) k(kind, label)
         WHERE st.live AND k.label IS NOT NULL
        UNION ALL
        -- LATERAL … OFFSET 0: one index access per document. Without it the planner (which overestimates
        -- st) chooses a hash join and reads all tag mappings or field values (1 million documents ≈ 3 s)
        SELECT 2::smallint, dt.tag_id, st.b, st.sign
          FROM st CROSS JOIN LATERAL (SELECT x.tag_id FROM paperrust_document_tag x WHERE x.document_id = st.id OFFSET 0) dt
         WHERE st.live
        %3$s
    )
    INSERT INTO paperrust_label_count_delta (kind, label_id, owner_bucket, d)
    SELECT kind, label, b, sum(sign)::integer FROM d GROUP BY 1, 2, 3 HAVING sum(sign) <> 0
$b$, p_old, p_new,
    CASE WHEN p_fields THEN 'UNION ALL
        SELECT 6::smallint, fv.field_id, st.b, st.sign
          FROM st CROSS JOIN LATERAL (SELECT x.field_id FROM paperrust_field_value x
                                       WHERE x.document_id = st.id AND x.deleted_at IS NULL OFFSET 0) fv
         WHERE st.live' ELSE '' END))
$fn$;

-- Trigger on paperrust_document_tag (kind 2) or paperrust_field_value (kind 6, non-deleted
-- values only): row comes/goes in the current state of the document (if the document is missing – deferred
-- foreign key –, it does not count; the document trigger takes existing rows into account).
CREATE OR REPLACE FUNCTION paperrust_label_count_tag_body(p_old text, p_new text) RETURNS text
LANGUAGE sql IMMUTABLE AS $fn$
    SELECT format($b$
BEGIN
    WITH o AS (%1$s), n AS (%2$s), x AS (
        SELECT document_id, tag_id, -1 AS sign FROM o UNION ALL SELECT document_id, tag_id, 1 FROM n
    )
    INSERT INTO paperrust_label_count_delta (kind, label_id, owner_bucket, d)
    SELECT 2::smallint, x.tag_id, paperrust_doc_bucket(d.owner_id, d.root_document_id), sum(x.sign)::integer
      FROM x JOIN paperrust_document d ON d.id = x.document_id
     WHERE d.deleted_at IS NULL
     GROUP BY 2, 3 HAVING sum(x.sign) <> 0;
    RETURN NULL;
END
$b$, p_old, p_new)
$fn$;

CREATE OR REPLACE FUNCTION paperrust_label_count_field_body(p_old text, p_new text) RETURNS text
LANGUAGE sql IMMUTABLE AS $fn$
    SELECT format($b$
BEGIN
    WITH o AS (%1$s), n AS (%2$s), x AS (
        SELECT document_id, field_id, -1 AS sign FROM o WHERE deleted_at IS NULL
        UNION ALL SELECT document_id, field_id, 1 FROM n WHERE deleted_at IS NULL
    )
    INSERT INTO paperrust_label_count_delta (kind, label_id, owner_bucket, d)
    SELECT 6::smallint, x.field_id, paperrust_doc_bucket(d.owner_id, d.root_document_id), sum(x.sign)::integer
      FROM x JOIN paperrust_document d ON d.id = x.document_id
     WHERE d.deleted_at IS NULL
     GROUP BY 2, 3 HAVING sum(x.sign) <> 0;
    RETURN NULL;
END
$b$, p_old, p_new)
$fn$;

CREATE OR REPLACE FUNCTION paperrust_label_count_trunc() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    PERFORM paperrust_label_count_rebuild();
    RETURN NULL;
END
$fn$;

-- Create or recreate triggers (idempotent; call again after every stage that
-- creates or removes a counted table).
CREATE OR REPLACE FUNCTION paperrust_label_count_install() RETURNS integer
LANGUAGE plpgsql AS $fn$
DECLARE
    t      text;
    op     text;
    ref    text;
    src    text;
    o_src  text;
    n_src  text;
    n      integer := 0;
    emp    text;
    fields boolean := to_regclass('public.paperrust_field_value') IS NOT NULL;
    chg    text := '(a.deleted_at IS NULL, a.owner_id, a.root_document_id, a.correspondent_id, a.document_type_id, a.storage_path_id)
                    IS NOT DISTINCT FROM
                    (b.deleted_at IS NULL, b.owner_id, b.root_document_id, b.correspondent_id, b.document_type_id, b.storage_path_id)';
BEGIN
    FOREACH t IN ARRAY ARRAY['paperrust_document', 'paperrust_document_tag', 'paperrust_field_value'] LOOP
        CONTINUE WHEN to_regclass('public.' || t) IS NULL;
        emp := format('SELECT * FROM %I WHERE false', t);
        FOREACH op IN ARRAY ARRAY['INSERT', 'UPDATE', 'DELETE'] LOOP
            ref := CASE op WHEN 'INSERT' THEN 'NEW TABLE AS paperrust_n'
                           WHEN 'DELETE' THEN 'OLD TABLE AS paperrust_o'
                           ELSE 'NEW TABLE AS paperrust_n OLD TABLE AS paperrust_o' END;
            o_src := CASE WHEN op = 'INSERT' THEN emp ELSE 'SELECT * FROM paperrust_o' END;
            n_src := CASE WHEN op = 'DELETE' THEN emp ELSE 'SELECT * FROM paperrust_n' END;
            IF t = 'paperrust_document' AND op = 'UPDATE' THEN
                -- only rows where something counted has changed (title, rev, date …
                -- change no counter; unchanged versions of a changed root are captured by vs)
                -- (NOT EXISTS instead of join with filter: the planner then estimates at most the row count of the
                -- statement and sticks with index accesses on tag mapping and field values)
                o_src := 'SELECT a.* FROM paperrust_o a WHERE NOT EXISTS (SELECT 1 FROM paperrust_n b WHERE b.id = a.id AND ' || chg || ')';
                n_src := 'SELECT b.* FROM paperrust_n b WHERE NOT EXISTS (SELECT 1 FROM paperrust_o a WHERE a.id = b.id AND ' || chg || ')';
            END IF;
            src := CASE t WHEN 'paperrust_document' THEN paperrust_label_count_doc_body(o_src, n_src, fields)
                          WHEN 'paperrust_document_tag' THEN paperrust_label_count_tag_body(o_src, n_src)
                          ELSE paperrust_label_count_field_body(o_src, n_src) END;
            EXECUTE format('CREATE OR REPLACE FUNCTION %I() RETURNS trigger LANGUAGE plpgsql AS %L',
                           'paperrust_count_cap_' || t || '_' || lower(left(op, 3)), src);
            EXECUTE format('DROP TRIGGER IF EXISTS %I ON %I', 'paperrust_count_' || lower(left(op, 3)), t);
            EXECUTE format('CREATE TRIGGER %I AFTER %s ON %I REFERENCING %s FOR EACH STATEMENT EXECUTE FUNCTION %I()',
                           'paperrust_count_' || lower(left(op, 3)), op, t, ref,
                           'paperrust_count_cap_' || t || '_' || lower(left(op, 3)));
            n := n + 1;
        END LOOP;
        EXECUTE format('DROP TRIGGER IF EXISTS paperrust_count_trunc ON %I', t);
        EXECUTE format('CREATE TRIGGER paperrust_count_trunc AFTER TRUNCATE ON %I FOR EACH STATEMENT EXECUTE FUNCTION paperrust_label_count_trunc()', t);
    END LOOP;
    RETURN n;
END
$fn$;

