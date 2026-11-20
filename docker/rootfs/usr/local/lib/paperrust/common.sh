# shellcheck shell=bash
# Gemeinsame Hilfsfunktionen für paperrust-entrypoint und paperrust-healthcheck (nur bash).

pr_log() { echo "[paperrust-init] $*" >&2; }

# Wert einer Einstellung wie pl_core::config: PAPERRUST_<NAME> vor PAPERLESS_<NAME> (nur Umgebung;
# Werte aus paperrust.conf/paperless.conf sieht das Skript nicht).
pr_env() {
	local name="$1" v
	v="PAPERRUST_${name}"
	if [[ -n "${!v:-}" ]]; then
		printf '%s' "${!v}"
		return 0
	fi
	v="PAPERLESS_${name}"
	printf '%s' "${!v:-}"
}

# Adresse, auf der paperrust lauscht. Reihenfolge:
#   1. PAPERRUST_BIND (bzw. Alias PAPERLESS_RS_BIND) wie gesetzt
#   2. PAPERRUST_BIND_ADDR/PAPERRUST_PORT (bzw. PAPERLESS_BIND_ADDR/PAPERLESS_PORT aus dem upstream-Image;
#      Standard hier 0.0.0.0, Port 8000)
# Ergebnis z. B. "0.0.0.0:8000" oder "[::]:8000".
pr_bind() {
	if [[ -n "${PAPERRUST_BIND:-}" ]]; then
		printf '%s' "$PAPERRUST_BIND"
		return 0
	fi
	if [[ -n "${PAPERLESS_RS_BIND:-}" ]]; then
		printf '%s' "$PAPERLESS_RS_BIND"
		return 0
	fi
	local addr port
	addr="$(pr_env BIND_ADDR)"
	port="$(pr_env PORT)"
	addr="${addr:-0.0.0.0}"
	port="${port:-8000}"
	addr="${addr#[}"
	addr="${addr%]}"
	if [[ "$addr" == *:* ]]; then
		printf '[%s]:%s' "$addr" "$port"
	else
		printf '%s:%s' "$addr" "$port"
	fi
}

# Host und Port, unter denen der Healthcheck den Server im Container erreicht.
# Platzhalter-Adressen (0.0.0.0, ::) -> Loopback; /__rs/health antwortet nur lokalen Aufrufern.
pr_probe_target() {
	local bind host port
	bind="$(pr_bind)"
	port="${bind##*:}"
	host="${bind%:*}"
	host="${host#[}"
	host="${host%]}"
	case "$host" in
	"" | 0.0.0.0 | :: | "::0") host=127.0.0.1 ;;
	esac
	printf '%s %s' "$host" "$port"
}

# Installationswurzel und Ordner wie pl_core::config (Standard: PAPERRUST_BASE_DIR/<name>).
pr_base_dir() { printf '%s' "${PAPERRUST_BASE_DIR:-/usr/src/paperless}"; }
pr_data_dir() {
	local d
	d="$(pr_env DATA_DIR)"
	printf '%s' "${d:-$(pr_base_dir)/data}"
}
pr_media_dir() {
	local d
	d="$(pr_env MEDIA_ROOT)"
	printf '%s' "${d:-$(pr_base_dir)/media}"
}
pr_consume_dir() {
	local d
	d="$(pr_env CONSUMPTION_DIR)"
	printf '%s' "${d:-$(pr_base_dir)/consume}"
}

# Ziel-UID/GID, wenn der Container als root startet (upstream USERMAP_UID/USERMAP_GID). PUID/PGID
# (Konvention von Unraid, CasaOS, linuxserver.io) gelten, wenn USERMAP_* nicht gesetzt ist.
pr_target_uid() { printf '%s' "${USERMAP_UID:-${PUID:-1000}}"; }
pr_target_gid() { printf '%s' "${USERMAP_GID:-${PGID:-1000}}"; }

# upstream init-env-file (C1): PAPERLESS_<X>_FILE (hier auch PAPERRUST_/PAPERLESS_RS_) nennt eine Datei,
# deren Inhalt als <X> exportiert wird (Docker-Secrets). Pfad-Einstellungen *_OCR_SKIP_ARCHIVE_FILE,
# *_MODEL_FILE, *_CLASSIFIER_MODEL_FILE bleiben unberührt. Wie upstream wird der Inhalt vollständig
# übernommen (abschließender Zeilenumbruch nur mit Warnung). paperrust selbst wertet *_FILE beim Start
# ebenso aus (pl_core::config::plan_file_env); hier nur, damit die Prüfungen dieses Skripts die Werte sehen.
pr_env_files() {
	local name target file rest value
	while IFS= read -r name; do
		case "$name" in
		PAPERLESS_RS_*_FILE) rest="${name#PAPERLESS_RS_}" ;;
		PAPERRUST_*_FILE) rest="${name#PAPERRUST_}" ;;
		PAPERLESS_*_FILE) rest="${name#PAPERLESS_}" ;;
		*) continue ;;
		esac
		case "$rest" in
		OCR_SKIP_ARCHIVE_FILE | MODEL_FILE | CLASSIFIER_MODEL_FILE | FILE) continue ;;
		esac
		target="${name%_FILE}"
		file="${!name}"
		if [[ -n "$file" && -f "$file" ]]; then
			if [[ $(tail -n1 "$file" | wc -l) != 0 ]]; then
				echo "[env-init] Your secret: ${name} contains a trailing newline and may not work as expected" >&2
			fi
			# $(...) entfernt Zeilenumbrüche am Ende; mit Platzhalter bleibt der Inhalt vollständig
			value="$(cat "$file"; printf x)"
			export "$target=${value%x}"
			echo "[env-init] ${target} set from ${name}" >&2
		else
			echo "[env-init] cannot find secret in ${name}" >&2
		fi
	done < <(compgen -e)
}
