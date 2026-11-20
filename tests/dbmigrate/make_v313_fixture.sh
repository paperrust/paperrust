#!/usr/bin/env bash
# Creates a real paperless-ngx 3.1.3 database with the reference test data for the catch-up test:
#   pl_mig_v313   = copy of paperless_snapshot (v3.4.2), migrated backwards with upstream Django
#                   to the state of v3.1.3 (documents → 0025, paperless → 0015) – i.e. exactly the
#                   operations that paperless-ngx itself runs; ContentType/permissions of
#                   documentbarcode removed (did not exist in 3.1.3).
#   pl_mig_fresh313 (optional, MIG_FRESH=1) = empty DB, freshly migrated with the code of upstream v3.1.3
#                   (`git archive v3.1.3`) – for the schema comparison.
# Test data only, own databases only (prefix pl_mig_). Clean up: tests/dbmigrate/run_catchup.sh --cleanup
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN="$(cd "$(git -C "$HERE" rev-parse --git-common-dir)/.." && pwd)"   # Main checkout (also from a worktree)
source "$MAIN/reference/env.sh" >/dev/null
UP="${UPSTREAM_DIR:-$REPO_DIR/upstream}"

export PGPASSWORD="$PG_SUPERPASS"
su() { "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d "${DB:-postgres}" "$@"; }
su -c "DROP DATABASE IF EXISTS pl_mig_v313 WITH (FORCE)" -c "CREATE DATABASE pl_mig_v313 OWNER paperless TEMPLATE paperless_snapshot"
(cd "$UP/src" && PAPERLESS_DBNAME=pl_mig_v313 "$PY" manage.py migrate documents 0025_workflowaction_apply_ai_suggestions --no-input --skip-checks -v 0 \
  && PAPERLESS_DBNAME=pl_mig_v313 "$PY" manage.py migrate paperless 0015_applicationconfiguration_remote_ocr_mode --no-input --skip-checks -v 0) 2>&1 \
  | grep -v "HINT\|can't find\|WARNINGS\|System check" || true
DB=pl_mig_v313 su -c "DELETE FROM auth_group_permissions WHERE permission_id IN (SELECT p.id FROM auth_permission p JOIN django_content_type c ON c.id=p.content_type_id WHERE c.app_label='documents' AND c.model='documentbarcode');
  DELETE FROM auth_user_user_permissions WHERE permission_id IN (SELECT p.id FROM auth_permission p JOIN django_content_type c ON c.id=p.content_type_id WHERE c.app_label='documents' AND c.model='documentbarcode');
  DELETE FROM auth_permission WHERE content_type_id IN (SELECT id FROM django_content_type WHERE app_label='documents' AND model='documentbarcode');
  DELETE FROM django_content_type WHERE app_label='documents' AND model='documentbarcode';"
echo "pl_mig_v313: $(DB=pl_mig_v313 su -tA -c "SELECT count(*) FROM django_migrations") Migrationen, $(DB=pl_mig_v313 su -tA -c "SELECT count(*) FROM documents_document") Dokumente"
if [[ "${MIG_FRESH:-0}" == 1 ]]; then
  SRC313="${TMPDIR:-/tmp}/pl_mig_upstream_v313"; rm -rf "$SRC313"; mkdir -p "$SRC313"
  git -C "$UP" archive v3.1.3 src | tar -x -C "$SRC313"
  su -c "DROP DATABASE IF EXISTS pl_mig_fresh313 WITH (FORCE)" -c "CREATE DATABASE pl_mig_fresh313 OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
  (cd "$SRC313/src" && PAPERLESS_DBNAME=pl_mig_fresh313 "$PY" manage.py migrate --no-input --skip-checks -v 0) 2>&1 | grep -v "HINT\|can't find\|WARNINGS\|System check" || true
  echo "pl_mig_fresh313: $(DB=pl_mig_fresh313 su -tA -c "SELECT count(*) FROM django_migrations") Migrationen (frisch mit v3.1.3)"
fi
