#!/usr/bin/env bash
# Fetches the static files that upstream obtains from Python packages via `collectstatic` and that
# PaperRust serves itself without Django (Swagger UI under /api/schema/view/, DRF statics), without
# Python/Django: download wheels, verify SHA-256, copy <package>/static/<name>/ to <target>/<name>/.
#
#   scripts/fetch-extra-static.sh <target>                   # e.g. results/pack/extra-static
#   EXTRA_STATIC=<target> bash scripts/build-static.sh        # afterwards: adoption into results/static
#
# Versions + checksums from upstream/uv.lock (v3.4.2). Keep identical with docker/Dockerfile (ADD --checksum).
# Requires: curl, sha256sum, unzip (or python3).
set -euo pipefail

DEST="${1:?Zielordner angeben}"
WHEELS=(
	"https://files.pythonhosted.org/packages/55/09/13509939a12e953759fa2f281b28dc14813c8c331addf3838fccdc71b364/drf_spectacular_sidecar-2026.10.1-py3-none-any.whl fe1d005d8ac9b5d0472f2fe5deeb2e7d6dcffaa24118044f22578b4b8c4b31f2"
	"https://files.pythonhosted.org/packages/c4/dd/de5f4543765bec94e036a5e9593a42f8fa7efec8de78eb56b9ca834385f3/djangorestframework-3.18.3-py3-none-any.whl 8544bb674846731b1e3c9b309236ee1dc412905a0aa725be2ec193ca950a7d12"
)

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$DEST"
for entry in "${WHEELS[@]}"; do
	url="${entry% *}"
	sha="${entry##* }"
	whl="$tmp/$(basename "$url")"
	curl -fsSL --retry 3 --proto '=https' -o "$whl" "$url"
	echo "$sha  $whl" | sha256sum -c --quiet - || { echo "SHA-256 falsch: $url" >&2; exit 1; }
	x="$tmp/x"
	rm -rf "$x"
	mkdir -p "$x"
	if command -v unzip >/dev/null; then
		unzip -q "$whl" -d "$x"
	else
		python3 -I -m zipfile -e "$whl" "$x"
	fi
	for s in "$x"/*/static/*; do
		rm -rf "${DEST:?}/$(basename "$s")"
		cp -r "$s" "$DEST/"
	done
done
ls "$DEST"
