"""Generates the raw draft of crates/pl-dbmigrate/schema/catchup-v2.20-v3.0.0.sql.

Run (Git Bash, development time only, without changing any database):
    source reference/env.sh && cd upstream/src && PAPERLESS_DBNAME=<any v2.20 DB> \
        "$PY" <repo>/tests/dbmigrate/gen_catchup_v2.py > roh.sql

For every migration that upstream v3.4.2 runs on a paperless-ngx 2.20.15 database
(`manage.py migrate --plan`), the SQL is collected as with `sqlmigrate` (Django schema_editor collect_sql).
RunPython steps appear as `-- Raw Python operation` and are translated by hand in the SQL file
(marked with `RunPython … (translated by hand)`).
"""

import os
import sys

import django

os.environ.setdefault("DJANGO_SETTINGS_MODULE", "paperless.settings")
django.setup()

from django.db import connection  # noqa: E402
from django.db.migrations.loader import MigrationLoader  # noqa: E402

PLAN = [
    ("documents", n)
    for n in """0003_remove_document_storage_type 0004_workflowtrigger_filter_has_any_correspondents_and_more
0005_alter_document_checksum_unique 0006_document_content_length 0007_sharelinkbundle
0008_workflowaction_passwords_alter_workflowaction_type 0009_alter_document_content_length
0010_optimize_integer_field_sizes 0011_alter_workflowaction_type 0012_document_root_document
0013_alter_paperlesstask_task_name 0014_savedview_visibility_to_ui_settings 0015_document_version_index_and_more
0016_sha256_checksums 0017_migrate_fulltext_query_field_prefixes 0018_saved_view_simple_search_rules
0019_task_system_redesign 0020_drop_celery_results 0021_widen_workflow_integer_fields 0022_add_perf_indexes""".split()
] + [
    ("paperless", n)
    for n in """0005_applicationconfiguration_ai_enabled_and_more 0006_applicationconfiguration_barcode_tag_split
0007_optimize_integer_field_sizes 0008_replace_skip_archive_file 0009_alter_applicationconfiguration_options
0010_alter_applicationconfiguration_llm_embedding_backend 0011_applicationconfiguration_llm_embedding_chunk_size
0012_applicationconfiguration_llm_output_language 0013_applicationconfiguration_llm_request_timeout""".split()
] + [
    ("paperless_mail", n)
    for n in "0002_optimize_integer_field_sizes 0003_mailrule_stop_processing 0004_processedmail_uid_validity".split()
]

# like `sqlmigrate`, but with resolved squash migrations (the replaced 2.x files no longer exist
# in v3.4.2; sqlmigrate loads without replacement and fails because of that)
loader = MigrationLoader(connection)
for app, name in PLAN:
    state = loader.project_state((app, name), at_end=False)
    migration = loader.get_migration(app, name)
    with connection.schema_editor(collect_sql=True, atomic=False) as se:
        migration.apply(state, se, collect_sql=True)
    print(f"-- @@ {app} {name}")
    for line in se.collected_sql:
        print(line)
    print()
sys.stdout.flush()
