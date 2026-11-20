-- PaperRust L3 (ARCHITECTURE-V2 §5): consistency of the search index for permissions and trash.
-- Crates: pl-indexer (catch-up), pl-search (fast path). Contract: docs/contract/pl-search.md "L3".
--
-- The fast search path (PAPERRUST_SEARCH_ENGINE=native) relies on every document in the
-- index that the index permission filter shows to a user also being a visible root document
-- outside the trash in the database. That can only be violated by changes that REMOVE visibility
-- or liveness before the index has caught up:
-- owner change, revoking a guardian grant, trash, appending a version, deletion.
-- These triggers note the affected documents in paperrust_search_dirty; as long as a row
-- is present there, pl-search searches exactly like upstream (intersection with SQL). The drainer in the
-- server (pl-indexer `dirty::drain_once`) re-indexes the documents and deletes the rows.
-- New grants/documents need no entry: there the index shows at most less than
-- the database – exactly like upstream, which applies the index permission filter as well.
--
-- * expand-only: new table/sequence/functions/triggers with prefix paperrust_; no Django row
--   is changed (the triggers write only into paperrust_search_dirty).
-- * idempotent (IF NOT EXISTS / OR REPLACE / triggers re-created).
-- * Rollback:
--     DROP TRIGGER IF EXISTS paperrust_search_doc_upd ON documents_document;
--     DROP TRIGGER IF EXISTS paperrust_search_doc_del ON documents_document;
--     DROP TRIGGER IF EXISTS paperrust_search_uop ON guardian_userobjectpermission;
--     DROP TRIGGER IF EXISTS paperrust_search_gop ON guardian_groupobjectpermission;
--     DROP TRIGGER IF EXISTS paperrust_search_uop_trunc ON guardian_userobjectpermission;
--     DROP TRIGGER IF EXISTS paperrust_search_gop_trunc ON guardian_groupobjectpermission;
--     DROP TABLE paperrust_search_dirty; DROP SEQUENCE paperrust_search_dirty_seq;
--     DROP FUNCTION paperrust_search_mark(bigint), paperrust_search_doc_trg(),
--                   paperrust_search_guardian_trg(), paperrust_search_guardian_trunc_trg();

CREATE SEQUENCE IF NOT EXISTS paperrust_search_dirty_seq;

CREATE TABLE IF NOT EXISTS paperrust_search_dirty (
    doc_id bigint      PRIMARY KEY,
    seq    bigint      NOT NULL,
    marked timestamptz NOT NULL DEFAULT now()
);

-- Note one document (new seq on repeated marking: the drainer deletes only what it has
-- seen).
CREATE OR REPLACE FUNCTION paperrust_search_mark(doc bigint) RETURNS void
LANGUAGE sql AS $fn$
    INSERT INTO paperrust_search_dirty (doc_id, seq)
    VALUES (doc, nextval('paperrust_search_dirty_seq'))
    ON CONFLICT (doc_id) DO UPDATE SET seq = EXCLUDED.seq, marked = now()
$fn$;

CREATE OR REPLACE FUNCTION paperrust_search_doc_trg() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM paperrust_search_mark(OLD.id);
        RETURN OLD;
    END IF;
    PERFORM paperrust_search_mark(NEW.id);
    RETURN NEW;
END
$fn$;

-- guardian object_pk → document id (canonical integers only, like CAST(id AS varchar) = object_pk).
-- Deliberately no check of content_type/permission: better to re-index one document too many (e.g. for
-- grants on tags with the same number) than to miss a grant with a differing row ct.
CREATE OR REPLACE FUNCTION paperrust_search_guardian_trg() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    IF OLD.object_pk ~ '^(0|-?[1-9][0-9]{0,17})$' THEN
        PERFORM paperrust_search_mark(OLD.object_pk::bigint);
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.object_pk ~ '^(0|-?[1-9][0-9]{0,17})$' THEN
        PERFORM paperrust_search_mark(NEW.object_pk::bigint);
    END IF;
    RETURN NULL;
END
$fn$;

-- TRUNCATE: first note all affected documents (afterwards the rows are gone).
CREATE OR REPLACE FUNCTION paperrust_search_guardian_trunc_trg() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    EXECUTE format(
        'INSERT INTO paperrust_search_dirty (doc_id, seq)
         SELECT k, nextval(''paperrust_search_dirty_seq'') FROM (
             SELECT DISTINCT g.object_pk::bigint AS k FROM %I g
              WHERE g.object_pk ~ ''^(0|-?[1-9][0-9]{0,17})$'') s
         ON CONFLICT (doc_id) DO UPDATE SET seq = EXCLUDED.seq, marked = now()',
        TG_TABLE_NAME);
    RETURN NULL;
END
$fn$;

DROP TRIGGER IF EXISTS paperrust_search_doc_upd ON documents_document;
CREATE TRIGGER paperrust_search_doc_upd
    AFTER UPDATE OF owner_id, deleted_at, root_document_id ON documents_document
    FOR EACH ROW
    WHEN (OLD.owner_id IS DISTINCT FROM NEW.owner_id
          OR OLD.deleted_at IS DISTINCT FROM NEW.deleted_at
          OR OLD.root_document_id IS DISTINCT FROM NEW.root_document_id)
    EXECUTE FUNCTION paperrust_search_doc_trg();

DROP TRIGGER IF EXISTS paperrust_search_doc_del ON documents_document;
CREATE TRIGGER paperrust_search_doc_del
    AFTER DELETE ON documents_document
    FOR EACH ROW EXECUTE FUNCTION paperrust_search_doc_trg();

DROP TRIGGER IF EXISTS paperrust_search_uop ON guardian_userobjectpermission;
CREATE TRIGGER paperrust_search_uop
    AFTER UPDATE OR DELETE ON guardian_userobjectpermission
    FOR EACH ROW EXECUTE FUNCTION paperrust_search_guardian_trg();

DROP TRIGGER IF EXISTS paperrust_search_gop ON guardian_groupobjectpermission;
CREATE TRIGGER paperrust_search_gop
    AFTER UPDATE OR DELETE ON guardian_groupobjectpermission
    FOR EACH ROW EXECUTE FUNCTION paperrust_search_guardian_trg();

DROP TRIGGER IF EXISTS paperrust_search_uop_trunc ON guardian_userobjectpermission;
CREATE TRIGGER paperrust_search_uop_trunc
    BEFORE TRUNCATE ON guardian_userobjectpermission
    FOR EACH STATEMENT EXECUTE FUNCTION paperrust_search_guardian_trunc_trg();

DROP TRIGGER IF EXISTS paperrust_search_gop_trunc ON guardian_groupobjectpermission;
CREATE TRIGGER paperrust_search_gop_trunc
    BEFORE TRUNCATE ON guardian_groupobjectpermission
    FOR EACH STATEMENT EXECUTE FUNCTION paperrust_search_guardian_trunc_trg();
