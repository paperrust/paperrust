-- PaperRust own data model, stage 3 "typed field values" (docs/DATAMODEL-V2.md §1.7, §6).
-- Name in paperrust_migrations: pl_store.dm3_paperrust_field_value. Rollback: dm3_paperrust_field_value.down.sql.
-- Contract: docs/contract/pl-store.md "Stage 3". Presupposes stages 1 and 2. Included before:
-- dm_label_count_fns.sql (counters learn kind 6 custom field).
--
-- documents_customfieldinstance (13 value columns, at most one set per row) → paperrust_field_value:
--   t        smallint  value type of the row (0 string, 1 url, 2 longtext, 3 select, 4 monetary, 5 boolean,
--                      6 date, 7 integer, 8 float, 9 documentlink) – from the set column, for empty
--                      values from the data_type of the field
--   v_text   text      string/url/longtext/select (option id)/monetary (raw text, e.g. "EUR12.50")
--   v_bool, v_date, v_int (int4), v_float (float8)   per type, lossless as upstream
--   v_num    numeric(65,2) GENERATED from the monetary raw text (same expression as upstream
--                      value_monetary_amount)
--   v_refs   jsonb     documentlink (as upstream, lossless; plan: integer[] – see DATAMODEL-V2 §6)
-- PK (document_id, field_id) (= upstream UNIQUE), id UNIQUE (audit log, export). Partial indexes per type
-- (field_id, v_*, document_id) WHERE v_* IS NOT NULL: sorting/range filters on an own field
-- run via the index; the view maps value_date/value_int/value_float/value_monetary_amount
-- 1:1 onto these columns so that existing queries (pl_store::docsql, pl_docfilter) use them.
-- View documents_customfieldinstance with exactly the v3.4.2 columns, INSTEAD-OF triggers, Django
-- table as an empty template to ngx_legacy, sync capture on the native table.

SET LOCAL maintenance_work_mem = '256MB';
SET LOCAL synchronize_seqscans = off;

-- Value type from the data_type of the field
CREATE OR REPLACE FUNCTION paperrust_cf_kind(p_data_type text) RETURNS smallint
LANGUAGE sql IMMUTABLE AS $fn$
    SELECT (CASE p_data_type WHEN 'string' THEN 0 WHEN 'url' THEN 1 WHEN 'longtext' THEN 2 WHEN 'select' THEN 3
                             WHEN 'monetary' THEN 4 WHEN 'boolean' THEN 5 WHEN 'date' THEN 6 WHEN 'integer' THEN 7
                             WHEN 'float' THEN 8 WHEN 'documentlink' THEN 9 ELSE 0 END)::smallint
$fn$;

-- Value type of a row in Django format: the set column, otherwise the type of the field
CREATE OR REPLACE FUNCTION paperrust_cf_row_kind(p_text text, p_url text, p_long text, p_select text, p_monetary text,
    p_bool boolean, p_date date, p_int integer, p_float double precision, p_refs jsonb, p_field integer) RETURNS smallint
LANGUAGE sql STABLE AS $fn$
    SELECT (CASE WHEN p_text IS NOT NULL THEN 0 WHEN p_url IS NOT NULL THEN 1 WHEN p_long IS NOT NULL THEN 2
                 WHEN p_select IS NOT NULL THEN 3 WHEN p_monetary IS NOT NULL THEN 4 WHEN p_bool IS NOT NULL THEN 5
                 WHEN p_date IS NOT NULL THEN 6 WHEN p_int IS NOT NULL THEN 7 WHEN p_float IS NOT NULL THEN 8
                 WHEN p_refs IS NOT NULL THEN 9
                 ELSE coalesce((SELECT paperrust_cf_kind(f.data_type) FROM documents_customfield f WHERE f.id = p_field), 0) END)::smallint
$fn$;

CREATE OR REPLACE FUNCTION paperrust_field_value_view_ins() RETURNS trigger
LANGUAGE plpgsql AS $fn$
DECLARE
    k smallint;
BEGIN
    IF num_nonnulls(NEW.value_text, NEW.value_url, NEW.value_long_text, NEW.value_select, NEW.value_monetary,
                    NEW.value_bool, NEW.value_date, NEW.value_int, NEW.value_float, NEW.value_document_ids) > 1 THEN
        RAISE EXCEPTION 'documents_customfieldinstance: more than one value column set' USING ERRCODE = 'check_violation';
    END IF;
    IF NEW.id IS NULL THEN
        NEW.id := nextval('paperrust_field_value_id_seq');
    END IF;
    k := paperrust_cf_row_kind(NEW.value_text, NEW.value_url, NEW.value_long_text, NEW.value_select, NEW.value_monetary,
                               NEW.value_bool, NEW.value_date, NEW.value_int, NEW.value_float, NEW.value_document_ids, NEW.field_id);
    INSERT INTO paperrust_field_value (id, deleted_at, restored_at, transaction_id, created, document_id, field_id, t,
        v_text, v_bool, v_date, v_int, v_float, v_refs)
    VALUES (NEW.id, NEW.deleted_at, NEW.restored_at, NEW.transaction_id, NEW.created, NEW.document_id, NEW.field_id, k,
        coalesce(NEW.value_text, NEW.value_url, NEW.value_long_text, NEW.value_select, NEW.value_monetary),
        NEW.value_bool, NEW.value_date, NEW.value_int, NEW.value_float, NEW.value_document_ids)
    RETURNING v_num INTO NEW.value_monetary_amount;
    RETURN NEW;
END
$fn$;

CREATE OR REPLACE FUNCTION paperrust_field_value_view_upd() RETURNS trigger
LANGUAGE plpgsql AS $fn$
DECLARE
    k smallint;
BEGIN
    IF num_nonnulls(NEW.value_text, NEW.value_url, NEW.value_long_text, NEW.value_select, NEW.value_monetary,
                    NEW.value_bool, NEW.value_date, NEW.value_int, NEW.value_float, NEW.value_document_ids) > 1 THEN
        RAISE EXCEPTION 'documents_customfieldinstance: more than one value column set' USING ERRCODE = 'check_violation';
    END IF;
    k := paperrust_cf_row_kind(NEW.value_text, NEW.value_url, NEW.value_long_text, NEW.value_select, NEW.value_monetary,
                               NEW.value_bool, NEW.value_date, NEW.value_int, NEW.value_float, NEW.value_document_ids, NEW.field_id);
    UPDATE paperrust_field_value SET id = NEW.id, deleted_at = NEW.deleted_at, restored_at = NEW.restored_at,
        transaction_id = NEW.transaction_id, created = NEW.created, document_id = NEW.document_id, field_id = NEW.field_id,
        t = k, v_text = coalesce(NEW.value_text, NEW.value_url, NEW.value_long_text, NEW.value_select, NEW.value_monetary),
        v_bool = NEW.value_bool, v_date = NEW.value_date, v_int = NEW.value_int, v_float = NEW.value_float,
        v_refs = NEW.value_document_ids
    WHERE id = OLD.id
    RETURNING v_num INTO NEW.value_monetary_amount;
    RETURN NEW;
END
$fn$;

CREATE OR REPLACE FUNCTION paperrust_field_value_view_del() RETURNS trigger
LANGUAGE plpgsql AS $fn$
BEGIN
    DELETE FROM paperrust_field_value WHERE id = OLD.id;
    RETURN OLD;
END
$fn$;

DO $mig$
DECLARE
    r       record;
    sync_on boolean;
    bad     bigint;
BEGIN
    IF to_regclass('public.paperrust_field_value') IS NOT NULL THEN
        RETURN;  -- already converted
    END IF;
    IF to_regclass('public.paperrust_document_tag') IS NULL THEN
        RAISE EXCEPTION 'stage 3 needs stages 1 and 2';
    END IF;
    SELECT count(*) INTO bad FROM documents_customfieldinstance
     WHERE num_nonnulls(value_text, value_url, value_long_text, value_select, value_monetary, value_bool, value_date,
                        value_int, value_float, value_document_ids) > 1;
    IF bad > 0 THEN
        RAISE EXCEPTION 'stage 3: % custom field value row(s) have more than one value column set – not convertible without loss', bad;
    END IF;
    sync_on := EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'public.documents_customfieldinstance'::regclass
                                                  AND tgname = 'paperrust_sync_upd');
    IF sync_on THEN
        PERFORM paperrust_sync_drop_triggers();
    END IF;

    CREATE TABLE paperrust_field_value (
        id             integer       GENERATED BY DEFAULT AS IDENTITY,
        deleted_at     timestamptz,
        restored_at    timestamptz,
        transaction_id uuid,
        created        timestamptz   NOT NULL,
        document_id    integer       NOT NULL,
        field_id       integer       NOT NULL,
        t              smallint      NOT NULL,
        v_text         text,
        v_bool         boolean,
        v_date         date,
        v_int          integer,
        v_float        double precision,
        v_num          numeric(65,2) GENERATED ALWAYS AS (
                           CASE WHEN t <> 4 THEN NULL
                                WHEN v_text ~ '^\d+' THEN substring(v_text, 1)::numeric(65,2)
                                ELSE substring(v_text, 4)::numeric(65,2) END) STORED,
        v_refs         jsonb
    );
    PERFORM set_config('enable_seqscan', 'off', true);
    PERFORM set_config('enable_sort', 'off', true);
    INSERT INTO paperrust_field_value (id, deleted_at, restored_at, transaction_id, created, document_id, field_id, t,
        v_text, v_bool, v_date, v_int, v_float, v_refs)
    SELECT i.id, i.deleted_at, i.restored_at, i.transaction_id, i.created, i.document_id, i.field_id,
           paperrust_cf_row_kind(i.value_text, i.value_url, i.value_long_text, i.value_select, i.value_monetary,
                                 i.value_bool, i.value_date, i.value_int, i.value_float, i.value_document_ids, i.field_id),
           coalesce(i.value_text, i.value_url, i.value_long_text, i.value_select, i.value_monetary),
           i.value_bool, i.value_date, i.value_int, i.value_float, i.value_document_ids
      FROM documents_customfieldinstance i
     -- heap order = order in which Django delivers the rows (Meta.ordering = created, via
     -- the index documents_customfieldinstance_created_*; ties in index order): this way
     -- ORDER BY created via the view (the sort recognises the presorted input) delivers the same
     -- order, e.g. in the export
     ORDER BY i.created;
    PERFORM set_config('enable_seqscan', 'on', true);
    PERFORM set_config('enable_sort', 'on', true);
    PERFORM setval('paperrust_field_value_id_seq', s.last_value, s.is_called) FROM documents_customfieldinstance_id_seq s;

    ALTER TABLE paperrust_field_value ADD CONSTRAINT paperrust_field_value_pkey PRIMARY KEY (document_id, field_id);
    ALTER TABLE paperrust_field_value ADD CONSTRAINT paperrust_field_value_id_key UNIQUE (id);
    CREATE INDEX paperrust_field_value_field ON paperrust_field_value (field_id, document_id);
    CREATE INDEX paperrust_field_value_date ON paperrust_field_value (field_id, v_date, document_id) WHERE v_date IS NOT NULL;
    CREATE INDEX paperrust_field_value_int ON paperrust_field_value (field_id, v_int, document_id) WHERE v_int IS NOT NULL;
    CREATE INDEX paperrust_field_value_float ON paperrust_field_value (field_id, v_float, document_id) WHERE v_float IS NOT NULL;
    CREATE INDEX paperrust_field_value_num ON paperrust_field_value (field_id, v_num, document_id) WHERE v_num IS NOT NULL;
    CREATE INDEX paperrust_field_value_refs ON paperrust_field_value USING gin (v_refs jsonb_path_ops) WHERE v_refs IS NOT NULL;
    ALTER TABLE paperrust_field_value
        ADD CONSTRAINT paperrust_field_value_document_fk FOREIGN KEY (document_id)
            REFERENCES paperrust_document (id) DEFERRABLE INITIALLY DEFERRED,
        ADD CONSTRAINT paperrust_field_value_field_fk FOREIGN KEY (field_id)
            REFERENCES documents_customfield (id) DEFERRABLE INITIALLY DEFERRED;

    -- re-point incoming foreign keys (v3.4.2: none)
    FOR r IN SELECT c.conname, c.conrelid::regclass AS tbl, pg_get_constraintdef(c.oid) AS def FROM pg_constraint c
              WHERE c.contype = 'f' AND c.confrelid = 'public.documents_customfieldinstance'::regclass
                AND c.conrelid <> c.confrelid LOOP
        EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', r.tbl, r.conname);
        EXECUTE format('ALTER TABLE %s ADD CONSTRAINT %I %s', r.tbl, r.conname,
                       regexp_replace(r.def, 'REFERENCES (public\.)?documents_customfieldinstance\(', 'REFERENCES paperrust_field_value('));
    END LOOP;

    -- Django table as an empty template
    CREATE SCHEMA IF NOT EXISTS ngx_legacy;
    FOR r IN SELECT conname FROM pg_constraint WHERE conrelid = 'public.documents_customfieldinstance'::regclass AND contype = 'f' LOOP
        EXECUTE format('ALTER TABLE public.documents_customfieldinstance DROP CONSTRAINT %I', r.conname);
    END LOOP;
    ALTER TABLE public.documents_customfieldinstance SET SCHEMA ngx_legacy;
    TRUNCATE ngx_legacy.documents_customfieldinstance;

    -- compatibility view (columns, types and order as v3.4.2)
    CREATE VIEW documents_customfieldinstance AS
    SELECT v.id, v.deleted_at, v.restored_at, v.transaction_id, v.created,
           (CASE WHEN v.t = 0 THEN v.v_text END)::varchar(128) AS value_text,
           v.v_bool AS value_bool,
           (CASE WHEN v.t = 1 THEN v.v_text END)::varchar(200) AS value_url,
           v.v_date AS value_date,
           v.v_int AS value_int,
           v.v_float AS value_float,
           (CASE WHEN v.t = 4 THEN v.v_text END)::varchar(128) AS value_monetary,
           v.v_num AS value_monetary_amount,
           v.v_refs AS value_document_ids,
           (CASE WHEN v.t = 3 THEN v.v_text END)::varchar(16) AS value_select,
           CASE WHEN v.t = 2 THEN v.v_text END AS value_long_text,
           v.field_id, v.document_id
      FROM paperrust_field_value v;
    ALTER VIEW documents_customfieldinstance ALTER COLUMN id SET DEFAULT nextval('paperrust_field_value_id_seq'::regclass);
    CREATE TRIGGER paperrust_field_value_view_ins INSTEAD OF INSERT ON documents_customfieldinstance
        FOR EACH ROW EXECUTE FUNCTION paperrust_field_value_view_ins();
    CREATE TRIGGER paperrust_field_value_view_upd INSTEAD OF UPDATE ON documents_customfieldinstance
        FOR EACH ROW EXECUTE FUNCTION paperrust_field_value_view_upd();
    CREATE TRIGGER paperrust_field_value_view_del INSTEAD OF DELETE ON documents_customfieldinstance
        FOR EACH ROW EXECUTE FUNCTION paperrust_field_value_view_del();

    -- counters: add kind 6, recreate triggers; sync on the native table
    PERFORM paperrust_label_count_rebuild();
    PERFORM paperrust_label_count_install();
    IF sync_on THEN
        PERFORM paperrust_sync_install_triggers();
    END IF;
END
$mig$;

ANALYZE paperrust_field_value;
