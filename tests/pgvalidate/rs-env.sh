#!/usr/bin/env bash
# Environment for the own PaperRust server (phase 6) against the DB copy pl_pgvalidate.
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
source reference/env.sh
export PAPERLESS_DBNAME=pl_pgvalidate
PGV_RS_PORT="${PGV_RS_PORT:-8090}"
export PAPERRUST_BIND=127.0.0.1:$PGV_RS_PORT
export PAPERRUST_ROUTES="$(pwd)/results/pgvalidate/routes-all-rust.toml"
export PAPERRUST_LOG="${PAPERRUST_LOG:-info,sqlx::query=debug}"
export PAPERLESS_URL=http://127.0.0.1:$PGV_RS_PORT
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:$PGV_RS_PORT"
export NO_COLOR=1
