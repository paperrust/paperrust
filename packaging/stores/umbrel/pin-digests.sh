#!/usr/bin/env bash
# Pin the images of the Umbrel package to multi-arch index digests (Umbrel requires
# "registry/repo:tag@sha256:<index digest>" for every image) and set the app version.
#
#   packaging/stores/umbrel/pin-digests.sh 0.1.2            # PaperRust tag 0.1.2, postgres:17
#   PG_TAG=17.7 packaging/stores/umbrel/pin-digests.sh 0.1.3
#
# Needs: docker with buildx (docker buildx imagetools) and jq, or crane. Only reads from the
# registries (no login, no push). Fails if an image is missing linux/amd64 or linux/arm64.
# Edits paperrust/docker-compose.yml and paperrust/umbrel-app.yml in place; review the diff, then
# copy the folder into the umbrel-apps fork.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP="$HERE/paperrust"
VERSION="${1:?usage: pin-digests.sh <paperrust-version> (e.g. 0.1.2)}"
VERSION="${VERSION#v}"
PG_TAG="${PG_TAG:-17}"
PAPERRUST_IMAGE="${PAPERRUST_IMAGE:-ghcr.io/paperrust/paperrust}"
PG_IMAGE="docker.io/library/postgres"

index_json() {
	docker buildx imagetools inspect --format '{{json .Manifest}}' "$1"
}

digest_of() {
	local ref="$1" json platforms digest p
	if command -v crane >/dev/null 2>&1; then
		platforms="$(crane manifest "$ref" | jq -r '.manifests[]?.platform | select(.) | "\(.os)/\(.architecture)"')"
		digest="$(crane digest "$ref")"
	else
		json="$(index_json "$ref")"
		platforms="$(jq -r '.manifests[]?.platform | select(.) | "\(.os)/\(.architecture)"' <<<"$json")"
		digest="$(jq -r '.digest' <<<"$json")"
	fi
	for p in linux/amd64 linux/arm64; do
		grep -qx "$p" <<<"$platforms" || { echo "$ref: platform $p missing (found: $(tr '\n' ' ' <<<"$platforms"))" >&2; exit 1; }
	done
	[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "$ref: no index digest ($digest)" >&2; exit 1; }
	printf '%s' "$digest"
}

pr_digest="$(digest_of "$PAPERRUST_IMAGE:$VERSION")"
pg_digest="$(digest_of "$PG_IMAGE:$PG_TAG")"

sed -i -E \
	-e "s#image: ${PAPERRUST_IMAGE}:[^@[:space:]]+@sha256:[0-9a-f]{64}#image: ${PAPERRUST_IMAGE}:${VERSION}@${pr_digest}#" \
	-e "s#image: ${PG_IMAGE}:[^@[:space:]]+@sha256:[0-9a-f]{64}#image: ${PG_IMAGE}:${PG_TAG}@${pg_digest}#" \
	"$APP/docker-compose.yml"
sed -i -E "s#^version: \".*\"#version: \"${VERSION}\"#" "$APP/umbrel-app.yml"

if grep -q 'sha256:0\{64\}' "$APP/docker-compose.yml"; then
	echo "placeholder digest still present in docker-compose.yml" >&2
	exit 1
fi
grep -n 'image:' "$APP/docker-compose.yml"
grep -n '^version:' "$APP/umbrel-app.yml"
echo "Now update releaseNotes in umbrel-app.yml (empty for the first submission)."
