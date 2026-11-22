#!/usr/bin/env bash
# PaperRust Desktop – build the Linux package (portable directory + tar.gz). Counterpart to pack.ps1.
# No signature, no AppImage (see "Warum tar.gz" below).
#
# Layout (as apps/desktop/src/layout.rs expects):
#
#   <out>/
#     paperrust-app            Tauri shell (window, tray) – optional, needs WebKitGTK 4.1 on the system
#     paperrust-desktop        supervisor/console (run, backup, manage, stop, paths)
#     paperrust                server + worker (crates/pl-server)
#     paperrust-migrate-db     database migration (crates/pl-dbmigrate), first start "adopt database"
#     paperrust-pdftool        PDF tools redact/shrink/repair (crates/pl-pdftools, PdfCraft)
#     routes.toml              repo routes.toml unchanged (at most switches routes off; no Django)
#     static/                  server statics (scripts/build-static.sh → results/static, PAPERLESS_STATICDIR)
#     light/                   UI (frontend-light/dist) – /rs/app/, / redirects there
#     pgsql/{bin,lib,share}/   PostgreSQL 17 (theseus-rs, x86_64/aarch64-unknown-linux-gnu), thinned out
#     pdfium/libpdfium.so      PDFium (bblanchon/pdfium-binaries)
#     tesseract/               Tesseract from the build machine incl. its shared libraries (RPATH $ORIGIN)
#     tesseract/tessdata/      eng/deu/osd (tessdata_fast, pinned in sources.json)
#     PaperRust.sh             start script (--portable = data next to the program, --console = without window)
#     README.txt, LICENSE, NOTICE, THIRD-PARTY-LICENSES(.txt|/), PACKAGE.json
#
# Tesseract: there is no portable upstream build for Linux. The package therefore bundles the Tesseract of
# the build machine together with every shared library it needs (except the glibc core), with RPATH
# $ORIGIN, so OCR works right after unpacking without `apt install`. Build on the oldest supported distro
# (Debian 12, glibc 2.36; see apps/desktop/pack/linux-build.sh) so programs and bundled libraries run on
# every newer one. `--no-tesseract` falls back to the system Tesseract.
#
# Why tar.gz and no AppImage: PostgreSQL writes its data directory outside the package
# (~/.local/share/paperrust/pgdata) and needs executables with fixed relative paths
# (pgsql/bin -> ../lib); an AppImage is remounted on every start (FUSE needed, paths
# change) and brings no update/signature model that Tauri would not already have. .deb/.rpm of this
# directory: apps/desktop/pack/linux-pkg.sh (installs it unchanged under /opt/paperrust).
#
# Third-party binaries only from sources.json (URL + SHA-256 pinned), cache file name = checksum, checked again before every
# use; archives are only unpacked, never executed (except the self-test of the
# unpacked PostgreSQL programs).
#
# Requires: bash, coreutils (sha256sum), curl, tar, gzip, python3 (JSON), cargo (license list only).
#
# Examples:
#   # Build the programs beforehand (release, little RAM: -j 1):
#   #   cargo build --release -p pl-server --bin paperrust
#   #   cargo build --release -p paperrust-desktop
#   #   cargo build --release -p pl-dbmigrate --bin paperrust-migrate-db
#   #   cargo build --release -p pl-pdftools --features engine --bin paperrust-pdftool
#   #   (optional) cd apps/desktop-shell && cargo build --release
#   #   bash scripts/build-static.sh && (cd frontend-light && pnpm install && pnpm build)
#   apps/desktop/pack/pack.sh --tar
#   apps/desktop/pack/pack.sh --only pdfium,pg --out /tmp/paperrust-pack-test
#   apps/desktop/pack/pack.sh --arch aarch64 --target-dir target --tar     # build the programs for aarch64 beforehand
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"

ARCH="$(uname -m)"
OUT=""
CACHE=""
PROFILE=release
TARGET_DIR=""
STATIC_DIR=""
ROUTES_FILE=""
ONLY="app,pg,pdfium,tesseract,tessdata,licenses"
WITH_TESSERACT=1
MAX_DOWNLOAD_MB=0
SKIP_SELF_CHECK=0
KEEP_PRECOMPRESSED=0
WITH_TESSDATA=0
MAKE_TAR=0

usage() { sed -n '2,45p' "$0"; }
while [[ $# -gt 0 ]]; do
	case "$1" in
	--out) OUT="$2"; shift 2 ;;
	--cache) CACHE="$2"; shift 2 ;;
	--profile) PROFILE="$2"; shift 2 ;;
	--target-dir) TARGET_DIR="$2"; shift 2 ;;
	--static-dir) STATIC_DIR="$2"; shift 2 ;;
	--light-dir) LIGHT_DIR="$2"; shift 2 ;;
	--routes-file) ROUTES_FILE="$2"; shift 2 ;;
	--only) ONLY="$2"; shift 2 ;;
	--arch) ARCH="$2"; shift 2 ;;
	--max-download-mb) MAX_DOWNLOAD_MB="$2"; shift 2 ;;
	--skip-self-check) SKIP_SELF_CHECK=1; shift ;;
	--keep-precompressed) KEEP_PRECOMPRESSED=1; shift ;;
	--with-tessdata) WITH_TESSDATA=1; shift ;;
	--no-tesseract) WITH_TESSERACT=0; shift ;;
	--tar) MAKE_TAR=1; shift ;;
	-h | --help) usage; exit 0 ;;
	*) echo "unbekanntes Argument: $1" >&2; exit 2 ;;
	esac
done

case "$ARCH" in
x86_64 | amd64) ARCH=x86_64 ;;
aarch64 | arm64) ARCH=aarch64 ;;
*) echo "Architektur $ARCH nicht unterstützt (x86_64, aarch64)" >&2; exit 2 ;;
esac
case "$PROFILE" in release | debug) ;; *) echo "--profile release|debug" >&2; exit 2 ;; esac
TARGET="$ARCH-unknown-linux-gnu"

VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$REPO/Cargo.toml" | head -1)"
[[ -n "$VERSION" ]] || { echo "Version aus Cargo.toml nicht lesbar" >&2; exit 1; }
PACK_NAME="PaperRust-$VERSION-linux-$ARCH"
OUT="${OUT:-$REPO/results/desktop/$PACK_NAME}"
CACHE="${CACHE:-$REPO/results/pack/cache}"
TARGET_DIR="${TARGET_DIR:-${CARGO_TARGET_DIR:-$REPO/target}}"
ROUTES_FILE="${ROUTES_FILE:-$REPO/routes.toml}"
STATIC_DIR="${STATIC_DIR:-$REPO/results/static}"
LIGHT_DIR="${LIGHT_DIR:-$REPO/frontend-light/dist}"
SOURCES="$HERE/sources.json"
NOTICES="$OUT/THIRD-PARTY-LICENSES"
WORK="$CACHE/work-$$"

step() { printf '\033[36m==> %s\033[0m\n' "$*"; }
warn() { printf 'WARNUNG: %s\n' "$*" >&2; }
die() { printf 'FEHLER: %s\n' "$*" >&2; exit 1; }
want() { [[ ",$ONLY," == *",$1,"* ]]; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 fehlt"; }

need curl
need tar
need sha256sum
need python3

# value from sources.json: src_get postgresql targets x86_64-unknown-linux-gnu url
src_get() {
	python3 -I - "$SOURCES" "$@" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
for k in sys.argv[2:]:
    d = d.get(k) if isinstance(d, dict) else None
    if d is None:
        sys.exit(0)
print(d if not isinstance(d, (dict, list)) else json.dumps(d))
PY
}

# Downloads a pinned file (or takes it from the cache) and checks SHA-256. Output: path, empty =
# skipped (--max-download-mb).
get_pinned() {
	local name="$1" url="$2" sha="$3" size="$4" file part got
	[[ ${#sha} -eq 64 ]] || die "$name hat keine gepinnte SHA-256 in sources.json – Abbruch (nie ungeprüft herunterladen)."
	sha="${sha,,}"
	mkdir -p "$CACHE"
	file="$CACHE/${sha:0:16}-$(basename "$url")"
	if [[ -f "$file" ]]; then
		if [[ "$(sha256sum "$file" | cut -d' ' -f1)" == "$sha" ]]; then
			echo "$file"
			return 0
		fi
		warn "$name im Cache beschädigt – neu laden"
		rm -f "$file"
	fi
	if ((MAX_DOWNLOAD_MB > 0 && size > MAX_DOWNLOAD_MB * 1024 * 1024)); then
		warn "$name: $((size / 1024 / 1024)) MB > --max-download-mb $MAX_DOWNLOAD_MB – übersprungen"
		return 0
	fi
	echo "    lade $url ($((size / 1024 / 1024)) MB)" >&2
	part="$file.part"
	curl -fsSL --retry 3 --proto '=https' --tlsv1.2 -o "$part" "$url"
	got="$(sha256sum "$part" | cut -d' ' -f1)"
	if [[ "$got" != "$sha" ]]; then
		rm -f "$part"
		die "$name SHA-256 falsch: erwartet $sha, erhalten $got – Abbruch."
	fi
	mv -f "$part" "$file"
	echo "$file"
}

clean_dir() { rm -rf "$1"; mkdir -p "$1"; }

# Overview for THIRD-PARTY-LICENSES.txt: part, version, license, source, texts one per line, separated by
# the control character US (0x1f) – no whitespace, otherwise empty fields (version "") would be dropped when reading.
LICENSES_TSV="$WORK/licenses.tsv"
SEP=$'\x1f'
add_license() { printf "%s${SEP}%s${SEP}%s${SEP}%s${SEP}%s\n" "$@" >>"$LICENSES_TSV"; }
# Manifest parts as JSON snippets (name<TAB>JSON)
PARTS_TSV="$WORK/parts.tsv"
add_part() { printf '%s\t%s\n' "$1" "$2" >>"$PARTS_TSV"; }

mkdir -p "$OUT" "$NOTICES" "$WORK"
: >"$LICENSES_TSV"
: >"$PARTS_TSV"
trap 'rm -rf "$WORK"' EXIT
# Never version the output
for d in "$(dirname "$OUT")" "$(dirname "$CACHE")"; do
	if [[ -d "$d" && ! -f "$d/.gitignore" && "$d" == "$REPO/results/"* ]]; then
		printf '# Paketbau-Ausgabe (apps/desktop/pack/pack.sh) – nicht versionieren\n*\n' >"$d/.gitignore"
	fi
done

# ---------------------------------------------------------------------------- own programs + UI
if want app; then
	BIN="$TARGET_DIR/$PROFILE"
	# Cross-Build (--target): target/<triple>/<profile>
	[[ -x "$BIN/paperrust" ]] || [[ ! -x "$TARGET_DIR/$TARGET/$PROFILE/paperrust" ]] || BIN="$TARGET_DIR/$TARGET/$PROFILE"
	step "PaperRust-Programme ($PROFILE) aus $BIN"
	build_flag=""
	[[ "$PROFILE" == release ]] && build_flag="--release "
	for exe in paperrust paperrust-desktop; do
		[[ -x "$BIN/$exe" ]] || die "$BIN/$exe fehlt. Bauen: cargo build ${build_flag}-p $([[ $exe == paperrust ]] && echo 'pl-server --bin paperrust' || echo paperrust-desktop)"
		install -m 0755 "$BIN/$exe" "$OUT/$exe"
	done
	# PDF tools: required (otherwise redact/shrink 503, ingestion without repair)
	[[ -x "$BIN/paperrust-pdftool" ]] || die "$BIN/paperrust-pdftool fehlt. Bauen: cargo build ${build_flag}-p pl-pdftools --features engine --bin paperrust-pdftool"
	install -m 0755 "$BIN/paperrust-pdftool" "$OUT/paperrust-pdftool"
	if [[ -x "$BIN/paperrust-migrate-db" ]]; then
		install -m 0755 "$BIN/paperrust-migrate-db" "$OUT/"
	else
		warn "paperrust-migrate-db fehlt – Erst-Start ohne „Datenbank übernehmen“"
	fi
	# The Tauri shell is not a workspace member: without a shared CARGO_TARGET_DIR it lands in apps/desktop-shell/target
	APP_BIN="$BIN/paperrust-app"
	[[ -x "$APP_BIN" ]] || APP_BIN="$REPO/apps/desktop-shell/target/$PROFILE/paperrust-app"
	if [[ -x "$APP_BIN" ]]; then
		install -m 0755 "$APP_BIN" "$OUT/paperrust-app"
	else
		warn "paperrust-app (Tauri-Hülle) fehlt – Paket startet im Browser (paperrust-desktop run)"
	fi
	# Check the architecture of the programs (file is optional)
	if command -v file >/dev/null; then
		kind="$(file -b "$OUT/paperrust")"
		want_kind="x86-64"
		[[ "$ARCH" == aarch64 ]] && want_kind="aarch64"
		[[ "${kind,,}" == *"${want_kind,,}"* ]] || die "paperrust ist nicht für $ARCH gebaut: $kind"
	fi
	progs="{"
	for f in "$OUT"/paperrust*; do
		[[ -f "$f" && -x "$f" ]] || continue
		progs+="\"$(basename "$f")\": {\"size\": $(stat -c %s "$f"), \"sha256\": \"$(sha256sum "$f" | cut -d' ' -f1)\"},"
	done
	add_part programs "${progs%,}}"

	# Since 2026-11-14 routes.toml only switches routes off ("off"); ids not mentioned are on.
	# The file is copied unchanged; an old value "django" would only be a warning → abort.
	step "routes.toml aus $ROUTES_FILE"
	[[ -f "$ROUTES_FILE" ]] || die "$ROUTES_FILE fehlt"
	cp "$ROUTES_FILE" "$OUT/routes.toml"
	if grep -Eq '^"[^"]+"[[:space:]]*=[[:space:]]*"django"' "$OUT/routes.toml"; then die "routes.toml enthält noch django-Einträge"; fi
	add_part routes "{\"source\": \"$(basename "$ROUTES_FILE")\", \"count\": $(grep -Ec '^"[^"]+"[[:space:]]*=' "$OUT/routes.toml")}"

	step "Oberfläche (static) aus $STATIC_DIR"
	[[ -f "$STATIC_DIR/base.css" ]] || die "$STATIC_DIR sieht nicht nach STATIC_ROOT aus (base.css fehlt) – bash scripts/build-static.sh"
	clean_dir "$OUT/static"
	cp -a "$STATIC_DIR/." "$OUT/static/"
	# Pre-compressed .gz/.br bring nothing over 127.0.0.1 and almost double the size (as pack.ps1)
	((KEEP_PRECOMPRESSED)) || find "$OUT/static" -type f \( -name '*.gz' -o -name '*.br' \) -delete
	add_part static "{\"source\": \"results/static\"}"

	step "Oberfläche (light) aus $LIGHT_DIR"
	[[ -f "$LIGHT_DIR/index.html" ]] || die "$LIGHT_DIR enthält keinen Build der Oberfläche (index.html fehlt; cd frontend-light; pnpm build)"
	clean_dir "$OUT/light"
	cp -a "$LIGHT_DIR/." "$OUT/light/"
	((KEEP_PRECOMPRESSED)) || find "$OUT/light" -type f \( -name '*.gz' -o -name '*.br' \) -delete
	add_part light "{\"source\": \"frontend-light/dist\"}"

	step "Startskript, README, LICENSE"
	install -m 0644 "$REPO/LICENSE" "$OUT/LICENSE"
	[[ -f "$REPO/NOTICE" ]] && install -m 0644 "$REPO/NOTICE" "$OUT/NOTICE"
	mkdir -p "$NOTICES/paperless-ngx"
	[[ -f "$REPO/upstream/LICENSE" ]] && cp "$REPO/upstream/LICENSE" "$NOTICES/paperless-ngx/"
	cat >"$OUT/PaperRust.sh" <<'SH'
#!/usr/bin/env bash
# Start PaperRust. Data: ${XDG_DATA_HOME:-~/.local/share}/paperrust
#   ./PaperRust.sh             window + tray (paperrust-app), otherwise browser
#   ./PaperRust.sh --portable  data in ./data next to the program (do not put it in a cloud folder!)
#   ./PaperRust.sh --console   without window: paperrust-desktop run (Ctrl+C quits)
set -euo pipefail
here="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"
console=0
for a in "$@"; do
	case "$a" in
	--portable) export PAPERRUST_DESKTOP_HOME="$here/data" ;;
	--console) console=1 ;;
	*) echo "unbekannt: $a (--portable, --console)" >&2; exit 2 ;;
	esac
done
if ! command -v tesseract >/dev/null 2>&1 && [[ ! -x "$here/tesseract/tesseract" ]]; then
	echo "Hinweis: tesseract fehlt – Scans bekommen keinen Text (z. B. sudo apt install tesseract-ocr tesseract-ocr-deu)." >&2
fi
if ((console == 0)) && [[ -x "$here/paperrust-app" ]] && [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
	exec "$here/paperrust-app"
fi
exec "$here/paperrust-desktop" run
SH
	chmod 0755 "$OUT/PaperRust.sh"
	cat >"$OUT/README.txt" <<TXT
PaperRust $VERSION für Linux $ARCH (portabel, unsigniert)
=====================================================

Start:      ./PaperRust.sh            (Fenster, falls paperrust-app + WebKitGTK vorhanden, sonst Browser)
Daten:      \${XDG_DATA_HOME:-~/.local/share}/paperrust  (Datenbank, Dokumente, Logs, Sicherungen)
Portabel:   ./PaperRust.sh --portable -> Daten in ./data (nicht in Nextcloud/Dropbox ablegen!)
Konsole:    ./paperrust-desktop run | backup | stop | paths | manage <befehl>

Erster Start: Formular "Willkommen" (Fenster) bzw. Rückfragen in der Konsole - neu beginnen,
paperless-ngx-Export importieren oder bestehende paperless-ngx-Datenbank übernehmen.

Benötigt: glibc-basiertes Linux ($ARCH), glibc ab 2.36 (z. B. Debian 12, Ubuntu 24.04, Fedora 37 oder neuer).
Texterkennung (Tesseract mit Deutsch/Englisch) ist enthalten. Für das Fenster:
WebKitGTK 4.1 und libayatana-appindicator (Debian/Ubuntu: libwebkit2gtk-4.1-0, libayatana-appindicator3-1).
Nicht als root starten (PostgreSQL verweigert das).

Für einen Server statt eines Desktops: Docker-Image bzw. docs/user/installation.md#docker.
Lizenz: GPL-3.0 (LICENSE). Fremdteile: THIRD-PARTY-LICENSES.txt
TXT
	add_license "PaperRust (paperrust, paperrust-desktop, paperrust-app, paperrust-migrate-db, paperrust-pdftool)" "$VERSION" "GPL-3.0-only" "https://github.com/paperrust/paperrust" "LICENSE, NOTICE"
	add_license "paperless-ngx (Oberfläche static/, Schema, Verhalten)" "3.4.2" "GPL-3.0" "https://github.com/paperless-ngx/paperless-ngx" "THIRD-PARTY-LICENSES/paperless-ngx/LICENSE"
fi

# ---------------------------------------------------------------------------- PostgreSQL
if want pg; then
	pg_version="$(src_get postgresql version)"
	url="$(src_get postgresql targets "$TARGET" url)"
	sha="$(src_get postgresql targets "$TARGET" sha256)"
	size="$(src_get postgresql targets "$TARGET" size)"
	prefix="$(src_get postgresql targets "$TARGET" strip_prefix)"
	[[ -n "$url" ]] || die "sources.json: kein PostgreSQL für $TARGET"
	step "PostgreSQL $pg_version"
	arc="$(get_pinned PostgreSQL "$url" "$sha" "$size")"
	if [[ -n "$arc" ]]; then
		tmp="$WORK/pg"
		clean_dir "$tmp"
		tar -xzf "$arc" -C "$tmp"
		root="$tmp/$prefix"
		[[ -d "$root/bin" ]] || die "PostgreSQL-Archiv: $prefix/bin fehlt"
		pg="$OUT/pgsql"
		clean_dir "$pg"
		for d in bin lib share; do mv "$root/$d" "$pg/"; done
		# Thinning: developer files, Python (plpython is not used), static libraries,
		# JIT (libLLVM; jit = off), tools that are not needed
		rm -rf "$pg/lib/pgxs" "$pg/lib/postgresql/pgxs" "$pg/bin/ecpg" "$pg/bin/pgbench"
		find "$pg/lib" \( -name '*.a' -o -name 'plpython*' -o -name 'libLLVM*' -o -name 'llvmjit*' -o -name '*.bc' \) -prune -exec rm -rf {} +
		find "$pg/lib" -type d -name bitcode -prune -exec rm -rf {} +
		find "$pg/share" -path '*extension/plpython*' -exec rm -rf {} + 2>/dev/null || true
		mkdir -p "$NOTICES/postgresql"
		[[ -f "$root/LICENSE" ]] && cp "$root/LICENSE" "$NOTICES/postgresql/"
		[[ -d "$root/dependency-notices" ]] && cp -r "$root/dependency-notices" "$NOTICES/postgresql/"
		rm -rf "$NOTICES/postgresql/dependency-notices/python" "$NOTICES/postgresql/dependency-notices/vcpkg/vcpkg-"* \
			"$NOTICES/postgresql/dependency-notices/vcpkg/pkgconf" 2>/dev/null || true
		mb="$(du -sm "$pg" | cut -f1)"
		echo "    pgsql ausgedünnt: $mb MB"
		# Report missing system libraries early (only sensible on a matching architecture)
		if [[ "$(uname -m)" == "$ARCH" ]] && command -v ldd >/dev/null; then
			if ldd "$pg/bin/postgres" | grep 'not found' >&2; then
				die "postgres: Bibliotheken fehlen (siehe oben)"
			fi
		fi
		add_part postgresql "{\"version\": \"$pg_version\", \"sha256\": \"$sha\", \"url\": \"$url\", \"size_mb\": $mb}"
		add_license "PostgreSQL (pgsql/)" "$pg_version" "PostgreSQL License" "$url" "THIRD-PARTY-LICENSES/postgresql/LICENSE"
		add_license "  gebündelte Bibliotheken von PostgreSQL" "" "ICU (Unicode-3.0), OpenSSL (Apache-2.0), libxml2/libxslt (MIT), zlib (zlib), lz4 (BSD-2), zstd (BSD-3)" "$url" "THIRD-PARTY-LICENSES/postgresql/dependency-notices/"

		if ((SKIP_SELF_CHECK == 0)); then
			if [[ "$(uname -m)" != "$ARCH" ]]; then
				warn "Selbsttest PostgreSQL übersprungen (Paket $ARCH, Rechner $(uname -m))"
			elif [[ "$(id -u)" == 0 ]]; then
				warn "Selbsttest PostgreSQL übersprungen (läuft als root; postgres verweigert root)"
			else
				step "Selbsttest PostgreSQL (initdb ICU, Start, SELECT 1, Stopp)"
				data="$WORK/pgtest"
				port=$((55100 + RANDOM % 800))
				"$pg/bin/initdb" -D "$data" -U selftest --auth=trust --encoding=UTF8 \
					--locale-provider=icu --icu-locale=und --locale=C >/dev/null || die "Selbsttest: initdb fehlgeschlagen"
				# Output to a file, not to a pipe: the detached postgres would inherit it
				"$pg/bin/pg_ctl" start -D "$data" -w -t 60 -s -l "$WORK/pgtest.log" \
					-o "-p $port -c listen_addresses=127.0.0.1 -c unix_socket_directories='' -c jit=off" >/dev/null ||
					die "Selbsttest: Start fehlgeschlagen, siehe $WORK/pgtest.log"
				r="$("$pg/bin/psql" -h 127.0.0.1 -p "$port" -U selftest -d postgres -tAc "select 1, current_setting('server_version')" || true)"
				"$pg/bin/pg_ctl" stop -D "$data" -m fast -w -s || true
				[[ "$r" == 1\|17.* ]] || die "Selbsttest: Abfrage fehlgeschlagen ($r)"
				echo "    ok: $r"
			fi
		fi
	fi
fi

# ---------------------------------------------------------------------------- PDFium
if want pdfium; then
	ver="$(src_get pdfium version)"
	url="$(src_get pdfium targets "$TARGET" url)"
	sha="$(src_get pdfium targets "$TARGET" sha256)"
	size="$(src_get pdfium targets "$TARGET" size)"
	file_in="$(src_get pdfium targets "$TARGET" file)"
	[[ -n "$url" ]] || die "sources.json: kein PDFium für $TARGET"
	step "PDFium $ver"
	arc="$(get_pinned PDFium "$url" "$sha" "$size")"
	if [[ -n "$arc" ]]; then
		tmp="$WORK/pdfium"
		clean_dir "$tmp"
		tar -xzf "$arc" -C "$tmp"
		clean_dir "$OUT/pdfium"
		install -m 0644 "$tmp/$file_in" "$OUT/pdfium/libpdfium.so"
		mkdir -p "$NOTICES/pdfium"
		cp -r "$tmp"/LICENSE* "$NOTICES/pdfium/" 2>/dev/null || true
		add_part pdfium "{\"version\": \"$ver\", \"sha256\": \"$sha\", \"url\": \"$url\"}"
		add_license "PDFium (pdfium/libpdfium.so)" "$ver" "BSD-3-Clause; enthält FreeType (FTL), ICU (Unicode-3.0), libjpeg-turbo, libpng, zlib, OpenJPEG (BSD-2), lcms2 (MIT), abseil (Apache-2.0) …" "$url" "THIRD-PARTY-LICENSES/pdfium/"
	fi
fi

# ---------------------------------------------------------------------------- Tesseract (bundled)
if want tesseract && ((WITH_TESSERACT)); then
	need ldd
	need patchelf
	sys_tess="$(command -v tesseract || true)"
	[[ -n "$sys_tess" ]] || die "tesseract fehlt auf dem Baurechner (apt install tesseract-ocr libtesseract-dev) – oder --no-tesseract"
	tver="$(tesseract --version 2>&1 | head -1)"
	step "Tesseract ($tver) mit Bibliotheken nach tesseract/ (RPATH \$ORIGIN)"
	dst="$OUT/tesseract"
	mkdir -p "$dst"
	install -m 0755 "$(readlink -f "$sys_tess")" "$dst/tesseract"
	libtess="$(ldd "$sys_tess" | awk '/libtesseract\.so/ {print $3; exit}')"
	[[ -n "$libtess" ]] || die "libtesseract nicht gefunden (ldd $sys_tess)"
	# every library of tesseract + libtesseract (pl-ocr loads libtesseract.so.5 directly), without the glibc core
	tess_libs="$( { ldd "$sys_tess"; ldd "$libtess"; } | awk '/=> \// {print $3}' | sort -u)"
	tess_pkgs=""
	for l in $tess_libs; do
		b="$(basename "$l")"
		case "$b" in
		libc.so* | libm.so* | libdl.so* | libpthread.so* | librt.so* | libresolv.so* | ld-linux* | libutil.so*) continue ;;
		esac
		install -m 0644 "$(readlink -f "$l")" "$dst/$b"
		if command -v dpkg >/dev/null; then
			pk="$(dpkg -S "$(readlink -f "$l")" 2>/dev/null | head -1 | cut -d: -f1 || true)"
			[[ -z "$pk" ]] && pk="$(dpkg -S "$l" 2>/dev/null | head -1 | cut -d: -f1 || true)"
			[[ -n "$pk" ]] && tess_pkgs+="$pk "
		fi
	done
	[[ -e "$dst/libtesseract.so.5" ]] || ln -sf "$(basename "$libtess")" "$dst/libtesseract.so.5"
	for f in "$dst/tesseract" "$dst"/*.so*; do
		[[ -L "$f" ]] && continue
		patchelf --set-rpath '$ORIGIN' "$f"
	done
	# licenses: Debian copyright file of every package the files come from
	mkdir -p "$NOTICES/tesseract"
	for pk in $(printf '%s\n' tesseract-ocr $tess_pkgs | sort -u); do
		c="/usr/share/doc/${pk%%:*}/copyright"
		[[ -f "$c" ]] && cp "$c" "$NOTICES/tesseract/${pk%%:*}.copyright"
	done
	add_part tesseract "{\"version\": \"$tver\", \"source\": \"build machine ($(. /etc/os-release && echo "$PRETTY_NAME"))\", \"libs\": $(ls "$dst" | grep -c '\.so')}"
	add_license "Tesseract + Leptonica + Bildbibliotheken (tesseract/)" "$tver" "Apache-2.0 (Tesseract), BSD-2 (Leptonica), weitere je Paket" "https://github.com/tesseract-ocr/tesseract" "THIRD-PARTY-LICENSES/tesseract/"
	if ((SKIP_SELF_CHECK == 0)); then
		missing="$(env -i LD_LIBRARY_PATH= ldd "$dst/tesseract" "$dst/libtesseract.so.5" | grep -c 'not found' || true)"
		((missing == 0)) || die "Tesseract im Paket: $missing Bibliotheken fehlen"
		outside="$(env -i ldd "$dst/tesseract" | awk '/=> \// {print $3}' | grep -v "^$dst/" | grep -vE '/(libc|libm|libdl|libpthread|librt|libresolv|libutil)\.so' || true)"
		[[ -z "$outside" ]] || die "Tesseract im Paket lädt noch Systembibliotheken: $outside"
		env -i "$dst/tesseract" --version >/dev/null 2>&1 || die "Tesseract im Paket startet nicht"
	fi
	WITH_TESSDATA=1
fi

# ---------------------------------------------------------------------------- Sprachdaten (optional)
if want tessdata && ((WITH_TESSDATA)); then
	ver="$(src_get tessdata version)"
	step "Sprachdaten $ver (tesseract/tessdata – überdeckt die Sprachpakete des System-Tesseract)"
	dst="$OUT/tesseract/tessdata"
	mkdir -p "$dst"
	done_files=""
	for f in eng.traineddata deu.traineddata osd.traineddata; do
		u="$(src_get tessdata files "$f" url)"
		[[ -n "$u" ]] || continue
		p="$(get_pinned "$f" "$u" "$(src_get tessdata files "$f" sha256)" "$(src_get tessdata files "$f" size)")"
		[[ -n "$p" ]] && install -m 0644 "$p" "$dst/$f" && done_files+="\"$f\","
	done
	add_part tessdata "{\"version\": \"$ver\", \"files\": [${done_files%,}]}"
	add_license "Tesseract-Sprachdaten (tesseract/tessdata)" "$ver" "Apache-2.0" "https://github.com/tesseract-ocr/tessdata_fast" "THIRD-PARTY-LICENSES/common/Apache-2.0.txt"
	if command -v tesseract >/dev/null && ((SKIP_SELF_CHECK == 0)); then
		langs="$(tesseract --tessdata-dir "$dst" --list-langs 2>&1 | tail -n +2 | tr '\n' ',')"
		echo "    Sprachen (System-tesseract mit Paketdaten): $langs"
	fi
fi

# ---------------------------------------------------------------------------- Rust crates + overview
if want licenses; then
	step "Lizenzen: Rust-Abhängigkeiten (cargo metadata), Standardtexte, Übersicht"
	mkdir -p "$NOTICES/common"
	cp "$HERE"/licenses/*.txt "$NOTICES/common/" 2>/dev/null || true
	crates_file="$NOTICES/rust-crates.txt"
	if command -v cargo >/dev/null; then
		{
			for ws in "$REPO" "$REPO/apps/desktop-shell"; do
				[[ -f "$ws/Cargo.toml" ]] || continue
				# offline first; metadata also needs the sources of crates for other platforms, so fall back to online
				(cd "$ws" && { cargo metadata --format-version 1 --locked --offline --filter-platform "$TARGET" 2>/dev/null ||
					cargo metadata --format-version 1 --locked --filter-platform "$TARGET" 2>/dev/null; }) ||
					die "cargo metadata in $ws fehlgeschlagen – ohne vollständige Lizenzliste kein Paket"
				echo
			done
		} | python3 -I -c '
import json, sys
sys.stdout.reconfigure(encoding="utf-8")
seen = {}
for chunk in sys.stdin.read().split("\n"):
    chunk = chunk.strip()
    if not chunk:
        continue
    try:
        meta = json.loads(chunk)
    except ValueError:
        continue
    for p in meta.get("packages", []):
        if p.get("source") is None:
            continue
        lic = p.get("license") or ("siehe " + str(p.get("license_file")))
        seen[(p["name"], p["version"])] = "%-45s %-12s %s" % (p["name"], p["version"], lic)
print("Rust-Abhängigkeiten von paperrust, paperrust-desktop, paperrust-app und paperrust-migrate-db")
print("(aus Cargo.lock beider Workspaces, Ziel '"$TARGET"'; Quelle: crates.io). Lizenztexte der")
print("häufigsten Lizenzen: common/.")
print()
for k in sorted(seen):
    print(seen[k])
print("CRATES=%d" % len(seen), file=sys.stderr)
' >"$crates_file" 2>"$WORK/crates.count"
		n="$(sed -n 's/^CRATES=//p' "$WORK/crates.count")"
		add_license "Rust-Bibliotheken (${n:-?} Crates)" "" "überwiegend MIT/Apache-2.0, außerdem BSD-2/BSD-3, ISC, Zlib, Unicode-3.0, MPL-2.0, CDLA-Permissive-2.0 (Liste je Crate)" "crates.io" "THIRD-PARTY-LICENSES/rust-crates.txt, THIRD-PARTY-LICENSES/common/"
	else
		warn "cargo fehlt – rust-crates.txt nicht erzeugt"
	fi
	[[ -x "$OUT/paperrust-app" ]] && add_license "Tauri 2, wry, tao, WebKitGTK-Bindings" "" "MIT/Apache-2.0" "https://tauri.app" "THIRD-PARTY-LICENSES/rust-crates.txt"
	add_license "Tesseract OCR, WebKitGTK" "" "nicht mitgeliefert (Systempakete der Distribution)" "-" "-"
fi

if [[ -s "$LICENSES_TSV" ]]; then
	{
		echo "PaperRust $VERSION – mitgelieferte Teile und ihre Lizenzen"
		printf '=%.0s' $(seq 1 60); echo
		echo "PaperRust selbst steht unter GPL-3.0-only (LICENSE). Quellcode: siehe NOTICE."
		echo "Alle unten genannten Lizenzen sind mit GPL-3.0 verträglich. Texte im Ordner THIRD-PARTY-LICENSES/."
		echo
		while IFS="$SEP" read -r part ver lic src texts; do
			if [[ -n "$ver" ]]; then echo "$part  [$ver]"; else echo "$part"; fi
			echo "    Lizenz: $lic"
			echo "    Quelle: $src"
			echo "    Texte:  $texts"
			echo
		done <"$LICENSES_TSV"
	} >"$OUT/THIRD-PARTY-LICENSES.txt"
fi

total="$(du -sb "$OUT" | cut -f1)"
python3 -I - "$PARTS_TSV" "$OUT/PACKAGE.json" "$VERSION" "$TARGET" "$total" <<'PY'
import datetime, json, sys
parts_file, out, version, target, total = sys.argv[1:6]
parts = {}
for line in open(parts_file, encoding="utf-8"):
    name, _, js = line.rstrip("\n").partition("\t")
    if name:
        parts[name] = json.loads(js)
m = {"name": "PaperRust", "version": version, "target": target,
     "created": datetime.datetime.now(datetime.timezone.utc).isoformat(), "signed": False,
     "parts": parts, "size_bytes": int(total)}
open(out, "w", encoding="utf-8").write(json.dumps(m, indent=2, ensure_ascii=False) + "\n")
PY
step "Verzeichnis fertig: $OUT ($((total / 1024 / 1024)) MB)"

if ((MAKE_TAR)); then
	# Name = directory name (default PaperRust-<version>-linux-<arch>), root in the archive likewise
	tarball="$(dirname "$OUT")/$(basename "$OUT").tar.gz"
	step "tar.gz $tarball"
	rm -f "$tarball"
	# Root in the archive = $PACK_NAME/, owner neutral (no user name of the build machine)
	tar --owner=0 --group=0 --numeric-owner -czf "$tarball" -C "$(dirname "$OUT")" "$(basename "$OUT")"
	echo "    $(($(stat -c %s "$tarball") / 1024 / 1024)) MB, SHA-256 $(sha256sum "$tarball" | cut -d' ' -f1)"
fi
