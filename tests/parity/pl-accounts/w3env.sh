#!/usr/bin/env bash
# Environment of the own Django reference instance of W3 (pl-accounts):
# DB pl_w3_ref (copy of paperless_snapshot), Django on 127.0.0.1:8120, SMTP sink 127.0.0.1:8125,
# fake OIDC IdP 127.0.0.1:8126, Redis prefix w3ref (own cache namespace).
# Sourced by w3-django.sh; the Rust parity tests read the same values.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../reference" && pwd)/env.sh"
W3_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export W3_WORK="${W3_WORK:-$REPO_DIR/target/w3-ref}"
export PAPERLESS_DBNAME=pl_w3_ref
export DJANGO_PORT=8120
export PAPERLESS_URL="http://127.0.0.1:8120"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8120,http://localhost:8120"
export PAPERLESS_REDIS_PREFIX=w3ref
export PAPERLESS_DATA_DIR="$(cygpath -w "$W3_WORK/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W3_WORK/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W3_WORK/consume")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W3_WORK/logs")"
export PAPERLESS_EMAIL_HOST=127.0.0.1
export PAPERLESS_EMAIL_PORT=8125
export PAPERLESS_EMAIL_FROM="paperless@example.invalid"
export PAPERLESS_ACCOUNT_ALLOW_SIGNUPS="${PAPERLESS_ACCOUNT_ALLOW_SIGNUPS:-true}"
_W3_PROVIDERS='{"openid_connect":{"OAUTH_PKCE_ENABLED":true,"APPS":[{"provider_id":"fakeidp","name":"Fake IdP","client_id":"w3client","secret":"w3secret","settings":{"server_url":"http://127.0.0.1:8126"}}]}}'
export PAPERLESS_SOCIALACCOUNT_PROVIDERS="${PAPERLESS_SOCIALACCOUNT_PROVIDERS:-$_W3_PROVIDERS}"
export PAPERLESS_SOCIAL_AUTO_SIGNUP="${PAPERLESS_SOCIAL_AUTO_SIGNUP:-false}"
export PAPERLESS_TRUSTED_PROXIES=""
export PAPERLESS_APPS="allauth.socialaccount.providers.openid_connect"
export DJANGO_SETTINGS_MODULE=w3_settings
export PYTHONPATH="$(cygpath -w "$W3_DIR");$PYTHONPATH"
REF_SUFFIX="-w3"
