-- PaperRust phase 6 (Postgres validation): scale up the reference seed deterministically to ~100,000 documents.
-- ONLY for the own copy (pl_pgvalidate), never for `paperless`.
--
--   createdb: CREATE DATABASE pl_pgvalidate TEMPLATE paperless_snapshot OWNER paperless;
--   psql -v ON_ERROR_STOP=1 -d pl_pgvalidate -f tests/pgvalidate/scale_100k.sql
--
-- Principle: the 200 root documents of the seed (id 1–200) are cloned 500 times
-- (clone k=1..500, new id = 204 + (k-1)*200 + o). This preserves the ratios of the seed
-- (owner, trash, tags/document, custom fields/document, notes, guardian permissions,
-- audit log). Additionally more master objects (200 tags, 100 correspondents,
-- 30 document types), to which the clones are distributed by hash, and 20,000 tasks.
-- No random(): everything via arithmetic/hashtext → bit-identical reproducible.
\set ON_ERROR_STOP 1
\timing on
BEGIN;

DO $$ BEGIN
  IF current_database() = 'paperless' OR current_database() = 'paperless_snapshot' THEN
    RAISE EXCEPTION 'refusing to scale the reference database %', current_database();
  END IF;
  IF (SELECT count(*) FROM documents_document) <> 204 THEN
    RAISE EXCEPTION 'expected the untouched seed (204 documents)';
  END IF;
END $$;

-- Helper function: deterministic hash in [0, n)
CREATE TEMP TABLE _k AS SELECT k FROM generate_series(1, 500) AS k;
CREATE OR REPLACE FUNCTION pg_temp.h(x bigint, salt text, n int) RETURNS int
  LANGUAGE sql IMMUTABLE AS $f$ SELECT (abs(hashtext(x::text || salt)::bigint) % n)::int $f$;

-- ---------------------------------------------------------------- Master objects
INSERT INTO documents_tag (id, tn_ancestors_pks, tn_ancestors_count, tn_children_pks, tn_children_count,
  tn_depth, tn_descendants_pks, tn_descendants_count, tn_index, tn_level, tn_priority, tn_order,
  tn_siblings_pks, tn_siblings_count, name, match, matching_algorithm, is_insensitive, color,
  is_inbox_tag, owner_id, tn_parent_id)
SELECT 20 + i, t.tn_ancestors_pks, t.tn_ancestors_count, '', 0, 0, '', 0, t.tn_index + i, 1, 0,
  t.tn_order + i, t.tn_siblings_pks, t.tn_siblings_count, 'Tag ' || lpad(i::text, 3, '0'),
  '', 0, true, '#' || lpad(to_hex((i * 9973) % 16777216), 6, '0'), false,
  CASE i % 5 WHEN 0 THEN 2 WHEN 1 THEN 3 ELSE NULL END, NULL
FROM generate_series(1, 180) i, documents_tag t WHERE t.id = 14;

INSERT INTO documents_correspondent (id, name, match, matching_algorithm, is_insensitive, owner_id)
SELECT 12 + i, 'Correspondent ' || lpad(i::text, 3, '0'), '', 0, true,
  CASE i % 4 WHEN 0 THEN 2 WHEN 1 THEN 3 ELSE NULL END
FROM generate_series(1, 88) i;

INSERT INTO documents_documenttype (id, name, match, matching_algorithm, is_insensitive, owner_id)
SELECT 8 + i, 'Type ' || lpad(i::text, 2, '0'), '', 0, true, CASE i % 3 WHEN 0 THEN 2 ELSE NULL END
FROM generate_series(1, 22) i;

-- ---------------------------------------------------------------- Documents
CREATE TEMP TABLE _docmap AS
SELECT o.id AS old_id, k, 204 + (k - 1) * 200 + o.id AS new_id
FROM documents_document o, _k WHERE o.id BETWEEN 1 AND 200;
CREATE INDEX ON _docmap (old_id, k);
CREATE INDEX ON _docmap (new_id);

INSERT INTO documents_document (id, deleted_at, restored_at, transaction_id, title, content, mime_type,
  checksum, archive_checksum, page_count, created, modified, added, filename, archive_filename,
  original_filename, archive_serial_number, correspondent_id, document_type_id, storage_path_id,
  owner_id, root_document_id, version_label, version_index)
SELECT m.new_id,
  CASE WHEN o.deleted_at IS NULL THEN NULL ELSE o.deleted_at + (m.k || ' hours')::interval END,
  NULL, NULL,
  left(o.title, 120) || ' ' || m.k,
  -- Text: own content + two other seed contents (~2.4 kB, realistic OCR volume)
  o.content || E'\n' || o2.content || E'\n' || o3.content || E'\nSeite ' || m.k,
  o.mime_type,
  md5(m.new_id::text) || md5(m.new_id::text || 'c'),
  CASE WHEN o.archive_checksum IS NULL THEN NULL ELSE md5(m.new_id::text || 'a') || md5(m.new_id::text || 'b') END,
  o.page_count,
  date '2025-11-01' - pg_temp.h(m.new_id, 'created', 3650),
  timestamptz '2016-01-01 08:00+00' + (m.new_id * interval '45 minutes') + interval '20 minutes',
  timestamptz '2016-01-01 08:00+00' + (m.new_id * interval '45 minutes'),
  'pgv/' || lpad(m.new_id::text, 7, '0') || CASE WHEN o.filename LIKE '%.txt' THEN '.txt' WHEN o.filename LIKE '%.png' THEN '.png' ELSE '.pdf' END,
  CASE WHEN o.archive_filename IS NULL THEN NULL ELSE 'pgv/' || lpad(m.new_id::text, 7, '0') || '.pdf' END,
  o.original_filename,
  CASE WHEN o.archive_serial_number IS NULL THEN NULL ELSE 1000 + m.new_id END,
  CASE WHEN o.correspondent_id IS NULL OR pg_temp.h(m.new_id, 'corr', 2) = 0 THEN o.correspondent_id
       ELSE 13 + pg_temp.h(m.new_id, 'corrx', 88) END,
  CASE WHEN o.document_type_id IS NULL OR pg_temp.h(m.new_id, 'dt', 2) = 0 THEN o.document_type_id
       ELSE 9 + pg_temp.h(m.new_id, 'dtx', 22) END,
  o.storage_path_id, o.owner_id, NULL, NULL, NULL
FROM _docmap m
JOIN documents_document o ON o.id = m.old_id
JOIN documents_document o2 ON o2.id = ((m.old_id + m.k) % 200) + 1
JOIN documents_document o3 ON o3.id = ((m.old_id + 7 * m.k) % 200) + 1;

-- ---------------------------------------------------------------- Tags (m2m)
INSERT INTO documents_document_tags (document_id, tag_id)
SELECT m.new_id, dt.tag_id FROM _docmap m JOIN documents_document_tags dt ON dt.document_id = m.old_id;
-- 60 % of the clones additionally get one of the 180 new tags
INSERT INTO documents_document_tags (document_id, tag_id)
SELECT m.new_id, 21 + pg_temp.h(m.new_id, 'tag', 180) FROM _docmap m
WHERE pg_temp.h(m.new_id, 'tagp', 10) < 6
ON CONFLICT DO NOTHING;

-- ---------------------------------------------------------------- Notes
INSERT INTO documents_note (deleted_at, restored_at, transaction_id, note, created, document_id, user_id)
SELECT n.deleted_at, NULL, NULL, n.note || ' (' || m.k || ')', n.created + (m.k || ' days')::interval,
  m.new_id, n.user_id
FROM _docmap m JOIN documents_note n ON n.document_id = m.old_id;

-- ---------------------------------------------------------------- Custom field instances
CREATE TEMP TABLE _cfimap AS
SELECT c.id AS old_id, m.k, m.new_id AS new_doc,
  407 + (m.k - 1) * 407 + row_number() OVER (PARTITION BY m.k ORDER BY c.id) AS new_id
FROM _docmap m JOIN documents_customfieldinstance c ON c.document_id = m.old_id;
-- (seed has 407 instances with id 1..407; new ids are thus gapless and unique)
CREATE INDEX ON _cfimap (old_id, k);

INSERT INTO documents_customfieldinstance (id, deleted_at, restored_at, transaction_id, created,
  value_text, value_bool, value_url, value_date, value_int, value_float, value_monetary,
  value_document_ids, value_select, value_long_text, field_id, document_id)
SELECT x.new_id, c.deleted_at, NULL, NULL, c.created + (x.k || ' days')::interval,
  c.value_text, c.value_bool, c.value_url,
  CASE WHEN c.value_date IS NULL THEN NULL ELSE c.value_date + pg_temp.h(x.new_id, 'd', 730) - 365 END,
  CASE WHEN c.value_int IS NULL THEN NULL ELSE c.value_int + x.k END,
  c.value_float, c.value_monetary, c.value_document_ids, c.value_select, c.value_long_text,
  c.field_id, x.new_doc
FROM _cfimap x JOIN documents_customfieldinstance c ON c.id = x.old_id;

-- ---------------------------------------------------------------- guardian object permissions (documents)
INSERT INTO guardian_userobjectpermission (object_pk, content_type_id, permission_id, user_id)
SELECT m.new_id::text, p.content_type_id, p.permission_id, p.user_id
FROM guardian_userobjectpermission p JOIN _docmap m ON p.content_type_id = 17 AND m.old_id = p.object_pk::int;
INSERT INTO guardian_groupobjectpermission (object_pk, content_type_id, group_id, permission_id)
SELECT m.new_id::text, p.content_type_id, p.group_id, p.permission_id
FROM guardian_groupobjectpermission p JOIN _docmap m ON p.content_type_id = 17 AND m.old_id = p.object_pk::int;

-- ---------------------------------------------------------------- Audit-Log
-- Documents (ct 17)
INSERT INTO auditlog_logentry (object_pk, object_id, object_repr, action, changes, timestamp, actor_id,
  content_type_id, remote_addr, additional_data, serialized_data, cid, changes_text, remote_port, actor_email)
SELECT m.new_id::text, m.new_id, l.object_repr, l.action, l.changes,
  l.timestamp + (m.k || ' days')::interval, l.actor_id, l.content_type_id, l.remote_addr,
  l.additional_data, l.serialized_data, l.cid, l.changes_text, l.remote_port, l.actor_email
FROM auditlog_logentry l JOIN _docmap m ON l.content_type_id = 17 AND m.old_id = l.object_pk::int;
-- Custom field instances (ct 20); additional_data points upstream to the document
INSERT INTO auditlog_logentry (object_pk, object_id, object_repr, action, changes, timestamp, actor_id,
  content_type_id, remote_addr, additional_data, serialized_data, cid, changes_text, remote_port, actor_email)
SELECT x.new_id::text, x.new_id, l.object_repr, l.action, l.changes,
  l.timestamp + (x.k || ' days')::interval, l.actor_id, l.content_type_id, l.remote_addr,
  l.additional_data, l.serialized_data, l.cid, l.changes_text, l.remote_port, l.actor_email
FROM auditlog_logentry l JOIN _cfimap x ON l.content_type_id = 20 AND x.old_id = l.object_pk::int;

-- ---------------------------------------------------------------- Tasks (20 000 consume_file)
INSERT INTO documents_paperlesstask (owner_id, task_id, task_type, trigger_source, status, date_created,
  date_started, date_done, duration_seconds, wait_time_seconds, input_data, result_data, acknowledged)
SELECT CASE i % 5 WHEN 0 THEN NULL WHEN 1 THEN 3 WHEN 2 THEN 4 ELSE 2 END,
  substr(md5('task' || i), 1, 8) || '-' || substr(md5('task' || i), 9, 4) || '-4' || substr(md5('task' || i), 14, 3)
    || '-a' || substr(md5('task' || i), 18, 3) || '-' || substr(md5('task' || i), 21, 12),
  'consume_file',
  (ARRAY['web_ui','api_upload','folder_consume','email_consume'])[1 + i % 4],
  CASE WHEN i % 50 = 0 THEN 'failure' WHEN i > 19990 THEN 'pending' ELSE 'success' END,
  timestamptz '2020-01-01 00:00+00' + i * interval '2 hours',
  timestamptz '2020-01-01 00:00+00' + i * interval '2 hours' + interval '3 seconds',
  CASE WHEN i > 19990 THEN NULL ELSE timestamptz '2020-01-01 00:00+00' + i * interval '2 hours' + interval '40 seconds' END,
  CASE WHEN i > 19990 THEN NULL ELSE 37.0 END, 3.0,
  jsonb_build_object('filename', 'scan_' || lpad(i::text, 6, '0') || '.pdf'),
  CASE WHEN i % 50 = 0 THEN jsonb_build_object('error_message', 'Fake failure')
       WHEN i > 19990 THEN NULL ELSE jsonb_build_object('document_id', 204 + i * 5) END,
  i < 19900
FROM generate_series(1, 20000) i;

-- ---------------------------------------------------------------- Sequences
DO $$ DECLARE r record; BEGIN
  FOR r IN SELECT unnest(ARRAY['documents_document','documents_document_tags','documents_note',
      'documents_customfieldinstance','guardian_userobjectpermission','guardian_groupobjectpermission',
      'auditlog_logentry','documents_paperlesstask','documents_tag','documents_correspondent',
      'documents_documenttype']) AS t LOOP
    EXECUTE format('SELECT setval(pg_get_serial_sequence(%L, ''id''), (SELECT max(id) FROM %I))', r.t, r.t);
  END LOOP;
END $$;

COMMIT;
VACUUM (ANALYZE);

SELECT 'documents' AS t, count(*) FROM documents_document
UNION ALL SELECT 'documents (trash)', count(*) FROM documents_document WHERE deleted_at IS NOT NULL
UNION ALL SELECT 'document_tags', count(*) FROM documents_document_tags
UNION ALL SELECT 'notes', count(*) FROM documents_note
UNION ALL SELECT 'customfieldinstance', count(*) FROM documents_customfieldinstance
UNION ALL SELECT 'guardian user perms', count(*) FROM guardian_userobjectpermission
UNION ALL SELECT 'guardian group perms', count(*) FROM guardian_groupobjectpermission
UNION ALL SELECT 'auditlog', count(*) FROM auditlog_logentry
UNION ALL SELECT 'tasks', count(*) FROM documents_paperlesstask
UNION ALL SELECT 'tags', count(*) FROM documents_tag
UNION ALL SELECT 'correspondents', count(*) FROM documents_correspondent
UNION ALL SELECT 'document types', count(*) FROM documents_documenttype;
SELECT pg_size_pretty(pg_database_size(current_database())) AS db_size;
