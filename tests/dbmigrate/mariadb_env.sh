# Environment for upstream Django against the test MariaDB (sourced by run_mariadb.sh / api_compare.sh).
export PAPERLESS_DBENGINE=mariadb PAPERLESS_DBHOST=127.0.0.1 PAPERLESS_DBPORT=33069
export PAPERLESS_DBNAME=paperless_src PAPERLESS_DBUSER=paperless PAPERLESS_DBPASS=paperless
# mysqlclient (Django driver) lives isolated under .work/pydeps, not in the reference venv
export PYTHONPATH="$(cygpath -w "$HERE/.work/pydeps");$(cygpath -w "$REPO_DIR/upstream/src")"
