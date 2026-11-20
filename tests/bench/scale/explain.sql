-- EXPLAIN (ANALYZE, BUFFERS) of the most expensive SQL shapes (from PAPERRUST_LOG=sqlx::query=debug, measurement run
-- 2026-11-10, docs/PERF-SCALE.md). Parameters substituted; restricted = user 22 (groups 6, 9).
-- Invocation: psql -d pl_scale_1m -f tests/bench/scale/explain.sql > explain.txt
\timing on
SET jit = on;  -- Instance default (as measured)

\echo '=== E1 Dokumentliste admin: Seite + COUNT-InitPlan (docs_default) ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
SELECT d.id, (SELECT COUNT(*) FROM (SELECT d2.id FROM documents_document d2
              WHERE d2.root_document_id IS NULL AND d2.deleted_at IS NULL) c) AS total
FROM documents_document d WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL
ORDER BY d.created DESC LIMIT 25 OFFSET 0;

\echo '=== E2 Dokumentliste restricted: Sichtbarkeit owner/NULL/guardian (docs_default) ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
WITH vis AS (
  SELECT d.id, d.created FROM documents_document d
  WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL
    AND (d.owner_id = 22 OR d.owner_id IS NULL OR CAST(d.id AS varchar) IN (
          SELECT object_pk FROM guardian_userobjectpermission WHERE user_id = 22 AND permission_id IN
            (SELECT id FROM auth_permission WHERE codename = 'view_document')
          UNION ALL
          SELECT object_pk FROM guardian_groupobjectpermission WHERE group_id IN
            (SELECT group_id FROM auth_user_groups WHERE user_id = 22) AND permission_id IN
            (SELECT id FROM auth_permission WHERE codename = 'view_document'))))
SELECT id, (SELECT count(*) FROM vis) FROM vis ORDER BY created DESC LIMIT 25;

\echo '=== E3 Tag-Liste: document_count über alle Tags (COUNT DISTINCT über 5 Mio. Zeilen) ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
SELECT o.id, COALESCE(dc.n, 0) FROM documents_tag o LEFT JOIN (
  SELECT dt.tag_id k, COUNT(DISTINCT dt.document_id) n FROM documents_document_tags dt
  WHERE dt.document_id IN (SELECT p.id FROM documents_document p
        LEFT JOIN documents_document pp ON pp.id = p.root_document_id WHERE p.deleted_at IS NULL)
  GROUP BY dt.tag_id) dc ON dc.k = o.id ORDER BY o.name;

\echo '=== E3b gleiche Zahl mit COUNT(*) (document_id,tag_id ist UNIQUE) ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
SELECT o.id, COALESCE(dc.n, 0) FROM documents_tag o LEFT JOIN (
  SELECT dt.tag_id k, COUNT(*) n FROM documents_document_tags dt
  WHERE dt.document_id IN (SELECT p.id FROM documents_document p WHERE p.deleted_at IS NULL)
  GROUP BY dt.tag_id) dc ON dc.k = o.id ORDER BY o.name;

\echo '=== E4 selection_data (POST, 100 ids): LEFT JOIN über die ganze Zwischentabelle + CASE je Zeile ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
SELECT o.id, COUNT(CASE WHEN j.document_id = ANY(string_to_array(
         (SELECT string_agg(id::text, ',') FROM (SELECT id FROM documents_document ORDER BY id LIMIT 100) s), ',')::bigint[])
       THEN 1 END)
FROM documents_tag o LEFT JOIN documents_document_tags j ON o.id = j.tag_id GROUP BY o.id;

\echo '=== E4b selection_data als Aggregat nur über die Auswahl (Vorschlag) ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
SELECT o.id, COALESCE(s.n, 0) FROM documents_tag o LEFT JOIN (
  SELECT tag_id, count(*) n FROM documents_document_tags
  WHERE document_id = ANY(ARRAY(SELECT id FROM documents_document ORDER BY id LIMIT 10000))
  GROUP BY tag_id) s ON s.tag_id = o.id;

\echo '=== E5 Sortierung nach Custom Field: korrelierte Unterabfragen je Zeile ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
SELECT d.id FROM documents_document d WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL
ORDER BY EXISTS(SELECT 1 FROM documents_customfieldinstance h WHERE h.document_id = d.id AND h.field_id = 16 AND h.deleted_at IS NULL) DESC,
         (SELECT o.value_int FROM documents_customfieldinstance o WHERE o.document_id = d.id AND o.field_id = 16
           AND o.deleted_at IS NULL ORDER BY o.created LIMIT 1) DESC
LIMIT 25;

\echo '=== E6 title_content: UPPER(content) LIKE über 2,2 GB Text ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
SELECT d.id, COUNT(*) OVER () FROM documents_document d
WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL
  AND (UPPER(d.title::text) LIKE UPPER('%zzqxnotfound%') OR UPPER(d.content::text) LIKE UPPER('%zzqxnotfound%'))
ORDER BY d.created DESC LIMIT 25;

\echo '=== E7 Statistik restricted: mime_type-Gruppierung mit Sichtbarkeit ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
SELECT d.mime_type, COUNT(d.id), SUM(d.content_length), MAX(d.archive_serial_number)
FROM documents_document d WHERE d.deleted_at IS NULL AND d.root_document_id IS NULL
  AND (d.owner_id = 22 OR d.owner_id IS NULL OR CAST(d.id AS varchar) IN (
        SELECT object_pk FROM guardian_groupobjectpermission WHERE group_id IN (6, 9) AND permission_id = (SELECT id FROM auth_permission WHERE codename = 'view_document')
        UNION ALL SELECT object_pk FROM guardian_userobjectpermission WHERE user_id = 22 AND permission_id = (SELECT id FROM auth_permission WHERE codename = 'view_document')))
GROUP BY d.mime_type;

\echo '=== E8 Volltext: alle sichtbaren IDs für die Schnittmenge (admin) ==='
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
SELECT id FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL;
