#!/usr/bin/env bash
# Environment of the own Django reference instance of W12 (pl-accounts, social/OIDC):
# DB pl_w12_ref (copy of paperless_snapshot), Django on 127.0.0.1:8140, fake OIDC IdP
# 127.0.0.1:8141, SMTP sink 127.0.0.1:8142, Redis prefix w12 (own cache namespace).
# Variant via W12_VARIANT: a = signup form, b = auto signup + group sync + SSO + amr,
# c = social signup closed + DISABLE_REGULAR_LOGIN + session until browser end, d = mandatory.
# Sourced by w12-django.sh; the Rust parity tests (tests/social_parity.rs) use the same values.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../reference" && pwd)/env.sh"
W12_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export W12_WORK="${W12_WORK:-$REPO_DIR/target/w12-ref}"
export W3_WORK="$W12_WORK"
export PAPERLESS_DBNAME=pl_w12_ref
export DJANGO_PORT=8140
export PAPERLESS_URL="http://127.0.0.1:8140"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8140,http://localhost:8140"
export PAPERLESS_REDIS_PREFIX=w12
export PAPERLESS_DATA_DIR="$(cygpath -w "$W12_WORK/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W12_WORK/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W12_WORK/consume")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W12_WORK/logs")"
export PAPERLESS_EMAIL_HOST=127.0.0.1
export PAPERLESS_EMAIL_PORT=8142
export PAPERLESS_EMAIL_FROM="paperless@example.invalid"
export PAPERLESS_ACCOUNT_ALLOW_SIGNUPS=true
_W12_PROVIDERS='{"openid_connect":{"OAUTH_PKCE_ENABLED":true,"APPS":[{"provider_id":"fakeidp","name":"Fake IdP","client_id":"w12client","secret":"w12secret","settings":{"server_url":"http://127.0.0.1:8141"}}]}}'
export PAPERLESS_SOCIALACCOUNT_PROVIDERS="$_W12_PROVIDERS"
export PAPERLESS_TRUSTED_PROXIES=""
export PAPERLESS_APPS="allauth.socialaccount.providers.openid_connect"
case "${W12_VARIANT:-a}" in
  a) export PAPERLESS_SOCIAL_AUTO_SIGNUP=false ;;
  b) export PAPERLESS_SOCIAL_AUTO_SIGNUP=true PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS=true \
       PAPERLESS_SOCIAL_ACCOUNT_DEFAULT_GROUPS=viewers PAPERLESS_ACCOUNT_DEFAULT_GROUPS=auditors \
       PAPERLESS_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP=idp-admins PAPERLESS_REDIRECT_LOGIN_TO_SSO=true \
       PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR=pwd ;;
  c) export PAPERLESS_SOCIAL_AUTO_SIGNUP=true PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS=false \
       PAPERLESS_DISABLE_REGULAR_LOGIN=true PAPERLESS_ACCOUNT_SESSION_REMEMBER=false ;;
  d) export PAPERLESS_SOCIAL_AUTO_SIGNUP=true PAPERLESS_ACCOUNT_EMAIL_VERIFICATION=mandatory ;;
esac
export DJANGO_SETTINGS_MODULE=w12_settings
export PYTHONPATH="$(cygpath -w "$W12_DIR");$PYTHONPATH"
REF_SUFFIX="-w12"
