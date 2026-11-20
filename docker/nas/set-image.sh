#!/usr/bin/env bash
# Setzt den Image-Namen in allen NAS-Vorlagen (docker/nas/**) auf einmal.
#
#   docker/nas/set-image.sh ghcr.io/<org>/paperrust          # Tag :latest bleibt
#   docker/nas/set-image.sh docker.io/<org>/paperrust
#
# Platzhalter bis zur Entscheidung des Inhabers: ghcr.io/paperrust/paperrust. Ersetzt wird der
# Image-Name (ohne Tag) in docker-compose.yml, casaos/docker-compose.yml und unraid/paperrust.xml
# sowie die Projekt-/Icon-URLs github.com/paperrust/paperrust, falls ein zweites Argument
# (neues GitHub-Repo "<org>/<repo>") angegeben ist.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OLD_IMAGE="ghcr.io/paperrust/paperrust"
OLD_REPO="paperrust/paperrust"
new_image="${1:?Aufruf: set-image.sh <registry/org/name> [github-org/repo]}"
new_repo="${2:-}"
new_image="${new_image%:latest}"

files=("$HERE/docker-compose.yml" "$HERE/casaos/docker-compose.yml" "$HERE/unraid/paperrust.xml")
for f in "${files[@]}"; do
	sed -i "s#${OLD_IMAGE}:#${new_image}:#g" "$f"
	if [[ -n "$new_repo" ]]; then
		sed -i -e "s#github.com/${OLD_REPO}#github.com/${new_repo}#g" \
			-e "s#githubusercontent.com/${OLD_REPO}/#githubusercontent.com/${new_repo}/#g" "$f"
	fi
	echo "angepasst: ${f#"$HERE"/}"
done
grep -n "image:\|<Repository>" "${files[@]}"
