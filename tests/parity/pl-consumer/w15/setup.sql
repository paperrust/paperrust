-- W15 parity: same initial state for pl_w15_ref (upstream) and pl_w15_rs (Rust).
-- Both DBs are copies of paperless_snapshot (reset_db.sh). Times relative to now(), with
-- day offsets so that the two runs (minutes apart) make the same decisions.
-- Files are created by each runner itself in its media folder (fixtures.json).
BEGIN;
UPDATE documents_workflow SET enabled = false;

-- Tags (root tags, treenode fields like a single tag)
INSERT INTO documents_tag (id, tn_ancestors_pks, tn_ancestors_count, tn_children_pks, tn_children_count, tn_depth,
  tn_descendants_pks, tn_descendants_count, tn_index, tn_level, tn_priority, tn_order, tn_siblings_pks, tn_siblings_count,
  name, match, matching_algorithm, is_insensitive, color, is_inbox_tag, owner_id, tn_parent_id)
SELECT id, '', 0, '', 0, 0, '', 0, 90 + (id % 100), 1, 0, 90 + (id % 100), '', 0, name, '', 0, true, '#a6cee3', false, NULL, NULL
FROM (VALUES (9111, 'w15-s1'), (9112, 'w15-s2'), (9113, 'w15-s3'), (9114, 'w15-s4'), (9115, 'w15-s5'),
             (9121, 'w15-s1-done'), (9131, 'w15-upd')) AS t(id, name);

INSERT INTO documents_correspondent (id, name, match, matching_algorithm, is_insensitive, owner_id) VALUES
  (9001, 'W15 Corr A', '', 0, true, NULL), (9002, 'W15 Corr B', '', 0, true, NULL);
INSERT INTO documents_storagepath (id, name, match, matching_algorithm, is_insensitive, path, owner_id) VALUES
  (9001, 'w15 sp', '', 0, true, 'w15/{{ correspondent }}/{{ title }}', NULL);
INSERT INTO documents_customfield (id, created, name, data_type, extra_data) VALUES
  (9001, now(), 'w15 due', 'date', '{"select_options": [], "default_currency": null}');

-- Documents (checksums are placeholders; re-ingesting does not change them)
INSERT INTO documents_document (id, title, content, mime_type, checksum, created, modified, added, filename,
  archive_filename, archive_checksum, original_filename, storage_path_id, correspondent_id) VALUES
  (9001, 'W15 text', 'old text', 'text/plain', 'w15-9001', current_date - 3, now() - interval '3 days', now() - interval '3 days', 'w15_text.txt', NULL, NULL, 'w15_text.txt', NULL, NULL),
  (9002, 'W15 pdf', 'old pdf', 'application/pdf', 'w15-9002', current_date - 3, now() - interval '3 days', now() - interval '3 days', 'w15_pdf.pdf', NULL, NULL, 'w15_pdf.pdf', NULL, NULL),
  (9003, 'W15 arch', 'old arch', 'application/pdf', 'w15-9003', current_date - 3, now() - interval '3 days', now() - interval '3 days', 'w15_arch.pdf', NULL, NULL, 'w15_arch.pdf', NULL, NULL),
  (9011, 'S1 due', 'c', 'text/plain', 'w15-9011', current_date - 10, now() - interval '10 days', now() - interval '10 days', 'w15_9011.txt', NULL, NULL, 'w15_9011.txt', NULL, NULL),
  (9012, 'S1 future', 'c', 'text/plain', 'w15-9012', current_date, now(), now() + interval '5 days', 'w15_9012.txt', NULL, NULL, 'w15_9012.txt', NULL, NULL),
  (9013, 'S2 due', 'c', 'text/plain', 'w15-9013', current_date - 40, now() - interval '1 day', now() - interval '1 day', 'w15_9013.txt', NULL, NULL, 'w15_9013.txt', NULL, NULL),
  (9014, 'S2 early', 'c', 'text/plain', 'w15-9014', current_date - 10, now() - interval '1 day', now() - interval '1 day', 'w15_9014.txt', NULL, NULL, 'w15_9014.txt', NULL, NULL),
  (9015, 'S3 old run', 'c', 'text/plain', 'w15-9015', current_date - 5, now() - interval '5 days', now() - interval '5 days', 'w15_9015.txt', NULL, NULL, 'w15_9015.txt', NULL, NULL),
  (9016, 'S3 recent run', 'c', 'text/plain', 'w15-9016', current_date - 5, now() - interval '5 days', now() - interval '5 days', 'w15_9016.txt', NULL, NULL, 'w15_9016.txt', NULL, NULL),
  (9017, 'S3 never run', 'c', 'text/plain', 'w15-9017', current_date - 5, now() - interval '5 days', now() - interval '5 days', 'w15_9017.txt', NULL, NULL, 'w15_9017.txt', NULL, NULL),
  (9018, 'S4 soon', 'c', 'text/plain', 'w15-9018', current_date - 5, now() - interval '5 days', now() - interval '5 days', 'w15_9018.txt', NULL, NULL, 'w15_9018.txt', NULL, NULL),
  (9019, 'S4 later', 'c', 'text/plain', 'w15-9019', current_date - 5, now() - interval '5 days', now() - interval '5 days', 'w15_9019.txt', NULL, NULL, 'w15_9019.txt', NULL, NULL),
  (9020, 'S4 ancient', 'c', 'text/plain', 'w15-9020', current_date - 5, now() - interval '5 days', now() - interval '5 days', 'w15_9020.txt', NULL, NULL, 'w15_9020.txt', NULL, NULL),
  (9031, 'Bulk one', 'c', 'text/plain', 'w15-9031', current_date - 5, now() - interval '5 days', now() - interval '5 days', 'w15_b1.txt', NULL, NULL, 'w15_b1.txt', 9001, NULL),
  (9021, 'S5 trash', 'c', 'text/plain', 'w15-9021', current_date - 2, now() - interval '2 days', now() - interval '2 days', 'w15_9021.txt', NULL, NULL, 'w15_9021.txt', NULL, NULL),
  (9032, 'Bulk two', 'c', 'text/plain', 'w15-9032', current_date - 5, now() - interval '5 days', now() - interval '5 days', 'w15_b2.txt', NULL, NULL, 'w15_b2.txt', NULL, NULL);

INSERT INTO documents_document_tags (document_id, tag_id) VALUES
  (9011, 9111), (9012, 9111), (9013, 9112), (9014, 9112), (9015, 9113), (9016, 9113), (9017, 9113),
  (9018, 9114), (9019, 9114), (9020, 9114), (9021, 9115), (9031, 9131);

INSERT INTO documents_customfieldinstance (id, created, field_id, document_id, value_date) VALUES
  (9001, now(), 9001, 9018, current_date + 3),
  (9002, now(), 9001, 9019, current_date + 20),
  (9003, now(), 9001, 9020, current_date - 400),
  (9004, now(), 9001, 9021, current_date);

-- Workflows: S1 added/0/once, S2 created/+30/once, S3 modified/0/repeated 1 day,
-- S4 field "w15 due"/-7/once, S5 added/0 → trash, U "Document changed" (type 3)
INSERT INTO documents_workflow (id, name, "order", enabled) VALUES
  (9001, 'W15 S1 added', 1, true), (9002, 'W15 S2 created', 2, true), (9003, 'W15 S3 modified', 3, true),
  (9004, 'W15 S4 custom field', 4, true), (9005, 'W15 U updated', 5, true), (9006, 'W15 S5 trash', 6, true);
INSERT INTO documents_workflowtrigger (id, type, sources, match, matching_algorithm, is_insensitive,
  schedule_offset_days, schedule_is_recurring, schedule_recurring_interval_days, schedule_date_field, schedule_date_custom_field_id) VALUES
  (9001, 4, '1,2,3,4', '', 0, true, 0, false, 1, 'added', NULL),
  (9002, 4, '1,2,3,4', '', 0, true, 30, false, 1, 'created', NULL),
  (9003, 4, '1,2,3,4', '', 0, true, 0, true, 1, 'modified', NULL),
  (9004, 4, '1,2,3,4', '', 0, true, -7, false, 1, 'custom_field', 9001),
  (9005, 3, '1,2,3,4', '', 0, true, 0, false, 1, 'added', NULL),
  (9006, 4, '1,2,3,4', '', 0, true, 0, false, 1, 'added', NULL);
INSERT INTO documents_workflowtrigger_filter_has_tags (workflowtrigger_id, tag_id) VALUES
  (9001, 9111), (9002, 9112), (9003, 9113), (9004, 9114), (9005, 9131), (9006, 9115);
INSERT INTO documents_workflow_triggers (workflow_id, workflowtrigger_id) VALUES
  (9001, 9001), (9002, 9002), (9003, 9003), (9004, 9004), (9005, 9005), (9006, 9006);
INSERT INTO documents_workflowaction (id, type, "order", assign_title, remove_all_tags, remove_all_document_types,
  remove_all_correspondents, remove_all_storage_paths, remove_all_owners, remove_all_permissions, remove_all_custom_fields,
  assign_correspondent_id, ai_create_missing, ai_overwrite_existing) VALUES
  (9001, 1, 0, NULL, false, false, false, false, false, false, false, NULL, false, false),
  (9002, 8, 1, NULL, false, false, false, false, false, false, false, NULL, false, false),
  (9003, 1, 0, NULL, false, false, false, false, false, false, false, 9001, false, false),
  (9004, 1, 0, 'S3 done', false, false, false, false, false, false, false, NULL, false, false),
  (9005, 1, 0, 'S4 done', false, false, false, false, false, false, false, NULL, false, false),
  (9006, 1, 0, NULL, false, false, false, false, false, false, false, 9002, false, false),
  (9007, 6, 0, NULL, false, false, false, false, false, false, false, NULL, false, false);
INSERT INTO documents_workflowaction_assign_tags (workflowaction_id, tag_id) VALUES (9001, 9121);
INSERT INTO documents_workflow_actions (workflow_id, workflowaction_id) VALUES
  (9001, 9001), (9001, 9002), (9002, 9003), (9003, 9004), (9004, 9005), (9005, 9006), (9006, 9007);

-- previous scheduled runs for S3
INSERT INTO documents_workflowrun (id, type, run_at, document_id, workflow_id) VALUES
  (9001, 4, now() - interval '2 days', 9015, 9003),
  (9002, 4, now() - interval '1 hour', 9016, 9003);
COMMIT;
