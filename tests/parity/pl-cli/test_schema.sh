#!/usr/bin/env bash
# Embedded schema responses = current Django responses (reference :8000, read only).
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
A="$REPO_DIR/crates/pl-cli/assets/schema"; fail=0
for v in 9 10; do
  curl -s "http://127.0.0.1:8000/api/schema/?format=json" -H "Accept: */*; version=$v" | cmp - <(gzip -dc "$A/v3.4.2-api$v.json.gz") && echo "OK   json v$v" || fail=1
  curl -s "http://127.0.0.1:8000/api/schema/" -H "Accept: application/vnd.oai.openapi; version=$v" | cmp - <(gzip -dc "$A/v3.4.2-api$v.yaml.gz") && echo "OK   yaml v$v" || fail=1
done
curl -s "http://127.0.0.1:8000/api/schema/view/" | sed -E 's/"X-CSRFTOKEN"\] = "[A-Za-z0-9]{64}"/"X-CSRFTOKEN"] = "{{CSRF_TOKEN}}"/; s#url: "/api/schema/"#url: "{{SCHEMA_URL}}"#; s#(href|src)="/static/#\1="{{STATIC_URL}}#' \
  | cmp - "$A/swagger-view.html" && echo "OK   swagger view template" || fail=1
exit $fail
