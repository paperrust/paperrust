#!/usr/bin/env bash
# Reference environment for parity runs on Linux with Docker: the OFFICIAL paperless-ngx v3.4.2 image
# (web server only, without Celery worker – as in reference/README.md), PostgreSQL 18, Valkey, test data from
# reference/seed.py. Counterpart to the Windows reference under reference/ (which runs without Docker).
#
#   scripts/parity-reference-docker.sh up       # start containers, migrate, seed, index, snapshot
#   scripts/parity-reference-docker.sh env      # print environment for PaperRust (B): eval "$(… env)"
#   scripts/parity-reference-docker.sh reset    # restore snapshot (PARITY_RESET_CMD)
#   scripts/parity-reference-docker.sh down     # remove containers + working folder
#
# Addresses (127.0.0.1 only, like the Windows reference): Django :8000, PostgreSQL :54329, Valkey :63799.
# Files are under $PARITY_REF_WORK (default: results/parity-ref-docker) and are mounted in the container
# under the SAME absolute path – Django and PaperRust see the same paths.
# All passwords/keys are local test values (reference/README.md).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
W="${PARITY_REF_WORK:-$REPO/results/parity-ref-docker}"
IMAGE="${PARITY_REF_IMAGE:-ghcr.io/paperless-ngx/paperless-ngx:3.4.2}"
PG_IMAGE="${PARITY_REF_PG_IMAGE:-docker.io/library/postgres:18}"
REDIS_IMAGE="${PARITY_REF_REDIS_IMAGE:-docker.io/valkey/valkey:9-alpine}"
P=prref
SECRET="paperless-rs-reference-secret-key-local-only-0123456789"
log() { echo "[ref-docker] $*"; }

psql_super() { docker exec -i "$P-db" psql -v ON_ERROR_STOP=1 -q -U postgres -d postgres "$@"; }
manage() { docker exec -w /usr/src/paperless/src "$P-web" python3 manage.py "$@"; }

django_env() {
	cat <<EOF
PAPERLESS_DBENGINE=postgresql
PAPERLESS_DBHOST=127.0.0.1
PAPERLESS_DBPORT=54329
PAPERLESS_DBNAME=paperless
PAPERLESS_DBUSER=paperless
PAPERLESS_DBPASS=paperless
PAPERLESS_REDIS=redis://127.0.0.1:63799
PAPERLESS_SECRET_KEY=$SECRET
PAPERLESS_TIME_ZONE=Europe/Zurich
PAPERLESS_DATA_DIR=$W/data
PAPERLESS_MEDIA_ROOT=$W/media
PAPERLESS_CONSUMPTION_DIR=$W/consume
PAPERLESS_LOGGING_DIR=$W/logs
PAPERLESS_URL=http://127.0.0.1:8000
PAPERLESS_ALLOWED_HOSTS=127.0.0.1,localhost
PAPERLESS_CSRF_TRUSTED_ORIGINS=http://127.0.0.1:8000,http://localhost:8000
PAPERLESS_DEBUG=false
PAPERLESS_OCR_LANGUAGE=eng
EOF
}

wait_http() {
	local url="$1" code=000
	for _ in $(seq 1 120); do
		code="$(curl -s -o /dev/null -w '%{http_code}' "$url" || true)"
		[[ "$code" =~ ^(200|302|401|403)$ ]] && return 0
		sleep 1
	done
	log "$url antwortet nicht (HTTP $code)"
	docker logs --tail 50 "$P-web" || true
	return 1
}

snapshot() {
	log "Snapshot (Template-DB paperless_snapshot + data.tar)"
	psql_super -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname='paperless' AND pid <> pg_backend_pid();" >/dev/null
	psql_super -c "DROP DATABASE IF EXISTS paperless_snapshot WITH (FORCE);" \
		-c "CREATE DATABASE paperless_snapshot OWNER paperless TEMPLATE paperless STRATEGY FILE_COPY;" \
		-c "ALTER DATABASE paperless_snapshot WITH ALLOW_CONNECTIONS false;"
	mkdir -p "$W/snapshot"
	tar -cf "$W/snapshot/data.tar" -C "$W" data media
}

cmd_up() {
	mkdir -p "$W/data/index" "$W/media" "$W/consume" "$W/logs"
	# Never version the working folder (by default it is under results/)
	printf '# scripts/parity-reference-docker.sh – Wegwerfdaten\n*\n' >"$W/.gitignore"
	log "PostgreSQL ($PG_IMAGE), Valkey ($REDIS_IMAGE)"
	docker run -d --name "$P-db" -p 127.0.0.1:54329:5432 \
		-e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres-local-only \
		"$PG_IMAGE" -c fsync=off -c synchronous_commit=off -c full_page_writes=off -c lc_messages=C >/dev/null
	docker run -d --name "$P-redis" -p 127.0.0.1:63799:6379 "$REDIS_IMAGE" >/dev/null
	for _ in $(seq 1 60); do docker exec "$P-db" pg_isready -q -U postgres && break; sleep 1; done
	psql_super -c "CREATE ROLE paperless LOGIN PASSWORD 'paperless' CREATEDB;" \
		-c "CREATE DATABASE paperless OWNER paperless ENCODING 'UTF8' TEMPLATE template0;"

	log "paperless-ngx ($IMAGE): nur Webserver (granian), kein Celery – wie reference/django-start.sh"
	local envf="$W/django.env"
	django_env >"$envf"
	# --network host: Django reaches 127.0.0.1:54329/63799 and listens on 127.0.0.1:8000 itself.
	# Own UID, so that PaperRust (B, on the host) can write the same files.
	docker run -d --name "$P-web" --network host --user "$(id -u):$(id -g)" \
		--env-file "$envf" -e HOME=/tmp -e GRANIAN_HOST=127.0.0.1 -e GRANIAN_PORT=8000 -e GRANIAN_WORKERS=1 \
		-v "$W:$W" -v "$REPO/reference:$REPO/reference:ro" \
		-w /usr/src/paperless/src --entrypoint granian "$IMAGE" \
		--interface asginl --ws --loop uvloop paperless.asgi:application >/dev/null

	log "migrate"
	manage migrate --no-input >"$W/logs/migrate.log" 2>&1
	log "seed.py (deterministisch, reference/README.md)"
	docker exec -w /usr/src/paperless/src -e DJANGO_SETTINGS_MODULE=paperless.settings \
		-e PYTHONPATH=/usr/src/paperless/src "$P-web" python3 "$REPO/reference/seed.py" | tee "$W/logs/seed.log"
	log "document_index reindex"
	manage document_index reindex --no-progress-bar >"$W/logs/index.log" 2>&1
	docker exec "$P-redis" valkey-cli FLUSHALL >/dev/null
	wait_http "http://127.0.0.1:8000/api/"
	snapshot
	log "fertig: Django http://127.0.0.1:8000 (Daten $W)"
}

cmd_reset() {
	psql_super -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname='paperless' AND pid <> pg_backend_pid();" >/dev/null
	psql_super -c "DROP DATABASE IF EXISTS paperless WITH (FORCE);" \
		-c "CREATE DATABASE paperless OWNER paperless TEMPLATE paperless_snapshot STRATEGY WAL_LOG;"
	rm -rf "$W/data" "$W/media"
	tar -xf "$W/snapshot/data.tar" -C "$W"
	mkdir -p "$W/consume"
	docker exec "$P-redis" valkey-cli FLUSHALL >/dev/null
	echo "[reset] Snapshot wiederhergestellt"
}

cmd_env() {
	# Environment for PaperRust (B) next to this reference (like results/bin/start-rs.sh); B needs no Redis
	django_env | grep -v '^PAPERLESS_REDIS=' | sed 's/^/export /'
	cat <<EOF
export PAPERRUST_BIND=127.0.0.1:8080
export PAPERRUST_ROUTES=$REPO/routes.toml
export PAPERRUST_BRANDING=off
export PARITY_A_URL=http://127.0.0.1:8000
export PARITY_B_URL=http://127.0.0.1:8080
export PARITY_RESET_CMD="bash $REPO/scripts/parity-reference-docker.sh reset"
EOF
}

cmd_down() {
	docker rm -f "$P-web" "$P-redis" "$P-db" >/dev/null 2>&1 || true
	rm -rf "$W"
}

case "${1:-}" in
up) cmd_up ;;
reset) cmd_reset ;;
env) cmd_env ;;
down) cmd_down ;;
*)
	sed -n '2,15p' "$0"
	exit 2
	;;
esac
