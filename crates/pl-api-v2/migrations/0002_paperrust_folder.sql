-- pl_api_v2.0002_paperrust_folder – folder view (results/ideas/2026-11-22-papermerge-ordneransicht.md,
-- stage 1; contract docs/contract/pl-api-v2.md "Ordner"). PaperRust only, no paperless-ngx counterpart.
--
-- Expand-only and idempotent: runs via `manage migrate` (pl_cli::migrate::paperrust_migrations, after the
-- data model stages) and on the first call of the folder API (pl_api_v2::folders::ensure).
--
--   paperrust_folder           folder tree as adjacency list (like the tag tree), names unique per level
--                              (case-insensitive), owner_id NULL = for everyone
--   paperrust_document_folder  document → folder, at most one folder per document (PK document_id);
--                              no row = "Unsorted". A 1:1 side table instead of a column on
--                              paperrust_document: works in every data model stage (view or table) and
--                              keeps documents_document/the datamodel check and its rollback untouched
--   paperrust_folder_grant     shares of a folder, same encoding as paperrust_object_grant
--                              (subject > 0 user, < 0 -group; perms 1 view, 2 change, 4 delete)
--
-- Deleting a folder never deletes documents: the mapping rows go with ON DELETE CASCADE (documents become
-- "Unsorted"); the API moves contents to the parent first if requested.
-- Rollback: DROP TABLE paperrust_folder_grant, paperrust_document_folder, paperrust_folder;
--           DROP FUNCTION paperrust_document_folder_cleanup() CASCADE;
SELECT pg_advisory_xact_lock(hashtext('paperrust_folder'));

CREATE TABLE IF NOT EXISTS paperrust_folder (
    id bigserial PRIMARY KEY,
    uuid uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    rev bigint NOT NULL DEFAULT 1,
    name varchar(128) NOT NULL CHECK (btrim(name) <> ''),
    -- NO ACTION (not RESTRICT): checked at the end of the statement, so the extras import can empty the table
    parent_id bigint REFERENCES paperrust_folder (id),
    owner_id integer,
    sort integer NOT NULL DEFAULT 0,
    color varchar(7) CHECK (color IS NULL OR color ~ '^#[0-9a-f]{6}$'),
    created timestamptz NOT NULL DEFAULT now(),
    modified timestamptz NOT NULL DEFAULT now(),
    CHECK (parent_id IS DISTINCT FROM id)
);
CREATE UNIQUE INDEX IF NOT EXISTS paperrust_folder_name_uniq ON paperrust_folder (coalesce(parent_id, 0), lower(name));
CREATE INDEX IF NOT EXISTS paperrust_folder_parent ON paperrust_folder (parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS paperrust_folder_owner ON paperrust_folder (owner_id) WHERE owner_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS paperrust_document_folder (
    document_id integer PRIMARY KEY,
    folder_id bigint NOT NULL REFERENCES paperrust_folder (id) ON DELETE CASCADE,
    moved timestamptz NOT NULL DEFAULT now()
);
-- list per folder and counts: folder first, document id as join key
CREATE INDEX IF NOT EXISTS paperrust_document_folder_folder ON paperrust_document_folder (folder_id, document_id);

CREATE TABLE IF NOT EXISTS paperrust_folder_grant (
    folder_id bigint NOT NULL REFERENCES paperrust_folder (id) ON DELETE CASCADE,
    subject bigint NOT NULL,
    perms smallint NOT NULL CHECK (perms > 0 AND perms < 8),
    PRIMARY KEY (folder_id, subject)
);
CREATE INDEX IF NOT EXISTS paperrust_folder_grant_subject ON paperrust_folder_grant (subject, folder_id) INCLUDE (perms);

-- Owner deleted → folder for everyone (like Django's on_delete=SET_NULL of `owner`). No foreign key: auth_user
-- belongs to Django; a statement trigger keeps it lean.
CREATE OR REPLACE FUNCTION paperrust_folder_owner_cleanup() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    UPDATE paperrust_folder f SET owner_id = NULL, rev = f.rev + 1, modified = now()
      FROM paperrust_old_users o WHERE f.owner_id = o.id;
    DELETE FROM paperrust_folder_grant g USING paperrust_old_users o WHERE g.subject = o.id;
    RETURN NULL;
END
$fn$;
DROP TRIGGER IF EXISTS paperrust_folder_owner_cleanup ON auth_user;
CREATE TRIGGER paperrust_folder_owner_cleanup AFTER DELETE ON auth_user
    REFERENCING OLD TABLE AS paperrust_old_users FOR EACH STATEMENT EXECUTE FUNCTION paperrust_folder_owner_cleanup();

-- Group deleted → its folder shares go.
CREATE OR REPLACE FUNCTION paperrust_folder_group_cleanup() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    DELETE FROM paperrust_folder_grant g USING paperrust_old_groups o WHERE g.subject = -o.id;
    RETURN NULL;
END
$fn$;
DROP TRIGGER IF EXISTS paperrust_folder_group_cleanup ON auth_group;
CREATE TRIGGER paperrust_folder_group_cleanup AFTER DELETE ON auth_group
    REFERENCING OLD TABLE AS paperrust_old_groups FOR EACH STATEMENT EXECUTE FUNCTION paperrust_folder_group_cleanup();

-- Document deleted for good (trash emptied) → mapping row goes. The trigger sits on the base table of the
-- documents: paperrust_document from data model stage 1, otherwise the Django table. Re-running this
-- migration (ensure() once per process) moves it after a later conversion.
CREATE OR REPLACE FUNCTION paperrust_document_folder_cleanup() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    DELETE FROM paperrust_document_folder f USING paperrust_old_docs o WHERE f.document_id = o.id;
    RETURN NULL;
END
$fn$;
DO $mig$
DECLARE
    base text;
BEGIN
    IF to_regclass('public.paperrust_document') IS NOT NULL THEN
        base := 'paperrust_document';
    ELSIF (SELECT c.relkind FROM pg_class c WHERE c.oid = to_regclass('public.documents_document')) = 'r' THEN
        base := 'documents_document';
    ELSE
        RETURN;
    END IF;
    EXECUTE format('DROP TRIGGER IF EXISTS paperrust_document_folder_cleanup ON %I', base);
    EXECUTE format('CREATE TRIGGER paperrust_document_folder_cleanup AFTER DELETE ON %I
        REFERENCING OLD TABLE AS paperrust_old_docs FOR EACH STATEMENT EXECUTE FUNCTION paperrust_document_folder_cleanup()', base);
END
$mig$;
