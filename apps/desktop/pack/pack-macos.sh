#!/usr/bin/env bash
# PaperRust Desktop – build the macOS package (portable directory + tar.gz, PaperRust.app + .dmg).
# Counterpart to pack.sh (Linux) and pack.ps1 (Windows). Unsigned: every Mach-O file and the app are
# only ad-hoc signed (`codesign -s -`), which Apple Silicon needs for programs to run at all; there is
# no Developer ID and no notarization, so Gatekeeper asks once (docs/DESKTOP.md "macOS").
#
# Must run on macOS (GitHub runner macos-15 / macos-15-intel, see .github/workflows/macos.yml) and the
# same architecture as the package; bash 3.2 compatible (macOS /bin/bash).
#
# Layout of the portable directory (as apps/desktop/src/layout.rs expects):
#
#   <out>/                          = PaperRust-<ver>-macos-<arch>/
#     paperrust-app                 Tauri shell (window, tray); in the .app: Contents/MacOS/paperrust-app
#     paperrust-desktop             supervisor/console (run, backup, manage, stop, paths)
#     paperrust                     server + worker
#     paperrust-migrate-db          database migration (first start "adopt database")
#     paperrust-pdftool             PDF tools (redact/shrink/repair)
#     routes.toml, static/, light/  as on Linux
#     pgsql/{bin,lib,share}/        PostgreSQL 17 (theseus-rs, *-apple-darwin; @loader_path/../lib), thinned out
#     pdfium/libpdfium.dylib        PDFium (bblanchon/pdfium-binaries)
#     tesseract/                    Homebrew Tesseract of the build machine + every non-system dylib it
#                                   needs, references rewritten to @loader_path (macos-bundle-dylibs.py)
#     tesseract/tessdata/           eng/deu/osd (tessdata_fast, pinned in sources.json)
#     fonts/LiberationSerif-Regular.ttf
#     PaperRust.sh, README.txt, LICENSE, NOTICE, THIRD-PARTY-LICENSES(.txt|/), PACKAGE.json
#
# PaperRust.app (--app): built by the Tauri bundler (`cargo tauri bundle --bundles app`, tauri-cli pinned
# in sources.json) with only the main program; afterwards this directory (without paperrust-app and
# PaperRust.sh) is copied into Contents/Resources/, where layout::install_root finds it. Then every
# Mach-O and finally the bundle are ad-hoc signed (no hardened runtime: library validation would refuse
# the dynamically loaded libpdfium/libtesseract). The .dmg is made with hdiutil (app + link to
# /Applications + README), not with Tauri's bundle_dmg.sh, which drives the Finder via AppleScript and is
# unreliable on headless CI machines.
#
# Minimum macOS version: the highest LC_BUILD_VERSION minos of all bundled files (PostgreSQL and the
# Homebrew libraries are built for the runner's macOS), written to Info.plist (LSMinimumSystemVersion)
# and PACKAGE.json, so older systems get a clear message instead of a crash.
#
# Requires: Xcode command line tools (otool, install_name_tool, codesign, lipo), curl, tar, python3,
# Homebrew tesseract (`brew install tesseract`; or --no-tesseract), cargo (license list).
#
# Examples:
#   cargo build --release -p pl-server --bin paperrust
#   cargo build --release -p paperrust-desktop
#   cargo build --release -p pl-dbmigrate --bin paperrust-migrate-db
#   cargo build --release -p pl-pdftools --features engine --bin paperrust-pdftool
#   (cd apps/desktop-shell && cargo build --release)
#   bash scripts/build-static.sh && (cd frontend-light && pnpm install && pnpm build)
#   apps/desktop/pack/pack-macos.sh --tar --app --dmg
set -euo pipefail
# PAPERRUST_PACK_TRACE=1: trace every command (diagnosing CI failures)
[[ "${PAPERRUST_PACK_TRACE:-}" == 1 ]] && set -x

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"

ARCH="$(uname -m)"
OUT=""
CACHE=""
TARGET_DIR=""
STATIC_DIR=""
LIGHT_DIR=""
ROUTES_FILE=""
WITH_TESSERACT=1
SKIP_SELF_CHECK=0
MAKE_TAR=0
MAKE_APP=0
MAKE_DMG=0

usage() { sed -n '2,50p' "$0"; }
while [[ $# -gt 0 ]]; do
	case "$1" in
	--out) OUT="$2"; shift 2 ;;
	--cache) CACHE="$2"; shift 2 ;;
	--target-dir) TARGET_DIR="$2"; shift 2 ;;
	--static-dir) STATIC_DIR="$2"; shift 2 ;;
	--light-dir) LIGHT_DIR="$2"; shift 2 ;;
	--routes-file) ROUTES_FILE="$2"; shift 2 ;;
	--arch) ARCH="$2"; shift 2 ;;
	--no-tesseract) WITH_TESSERACT=0; shift ;;
	--skip-self-check) SKIP_SELF_CHECK=1; shift ;;
	--tar) MAKE_TAR=1; shift ;;
	--app) MAKE_APP=1; shift ;;
	--dmg) MAKE_APP=1; MAKE_DMG=1; shift ;;
	-h | --help) usage; exit 0 ;;
	*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

case "$ARCH" in
x86_64 | amd64) ARCH=x86_64; LIPO_ARCH=x86_64 ;;
aarch64 | arm64) ARCH=aarch64; LIPO_ARCH=arm64 ;;
*) echo "architecture $ARCH not supported (x86_64, aarch64)" >&2; exit 2 ;;
esac
TARGET="$ARCH-apple-darwin"
HOST_ARCH="$(uname -m)"
[[ "$HOST_ARCH" == arm64 ]] && HOST_ARCH=aarch64

step() { printf '\033[36m==> %s\033[0m\n' "$*"; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 missing${2:+ ($2)}"; }
sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
fsize() { stat -f %z "$1"; }
lower() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }

[[ "$(uname -s)" == Darwin ]] || die "pack-macos.sh must run on macOS (uname: $(uname -s))"
need curl
need tar
need shasum
need python3
need otool "xcode-select --install"
need install_name_tool "xcode-select --install"
need codesign "xcode-select --install"
need lipo "xcode-select --install"

VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$REPO/Cargo.toml" | sed -n 1p)"
[[ -n "$VERSION" ]] || die "version not readable from Cargo.toml"
PACK_NAME="PaperRust-$VERSION-macos-$ARCH"
OUT="${OUT:-$REPO/results/desktop/$PACK_NAME}"
CACHE="${CACHE:-$REPO/results/pack/cache}"
TARGET_DIR="${TARGET_DIR:-${CARGO_TARGET_DIR:-$REPO/target}}"
ROUTES_FILE="${ROUTES_FILE:-$REPO/routes.toml}"
STATIC_DIR="${STATIC_DIR:-$REPO/results/static}"
LIGHT_DIR="${LIGHT_DIR:-$REPO/frontend-light/dist}"
SOURCES="$HERE/sources.json"
DYLIBS="$HERE/macos-bundle-dylibs.py"
NOTICES="$OUT/THIRD-PARTY-LICENSES"
mkdir -p "$CACHE"
WORK="$(cd "$CACHE" && pwd -P)/work-$$"
OUT_PARENT="$(mkdir -p "$(dirname "$OUT")" && cd "$(dirname "$OUT")" && pwd)"
OUT="$OUT_PARENT/$(basename "$OUT")"
NOTICES="$OUT/THIRD-PARTY-LICENSES"

# value from sources.json: src_get postgresql targets aarch64-apple-darwin url
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

# Download a pinned file (or take it from the cache) and check SHA-256. Output: path.
get_pinned() {
	local name="$1" url="$2" sha file part got
	sha="$(lower "$3")"
	[[ ${#sha} -eq 64 ]] || die "$name has no pinned SHA-256 in sources.json – abort (never download unchecked)."
	file="$CACHE/${sha:0:16}-$(basename "$url")"
	if [[ -f "$file" ]]; then
		if [[ "$(sha256 "$file")" == "$sha" ]]; then
			echo "$file"
			return 0
		fi
		warn "$name in the cache is damaged – downloading again"
		rm -f "$file"
	fi
	echo "    downloading $url" >&2
	part="$file.part"
	curl -fsSL --retry 5 --retry-delay 10 --retry-all-errors --proto '=https' --tlsv1.2 -o "$part" "$url" || die "$name: download failed ($url)"
	got="$(sha256 "$part")"
	if [[ "$got" != "$sha" ]]; then
		rm -f "$part"
		die "$name SHA-256 wrong: expected $sha, got $got – abort."
	fi
	mv -f "$part" "$file"
	echo "$file"
}

clean_dir() { rm -rf "$1"; mkdir -p "$1"; }

# Mach-O candidates below a directory (executables and libraries; data files are skipped by name/mode).
macho_files() {
	find "$1" -type f \( -perm -0100 -o -name '*.dylib' -o -name '*.so' \) -print | while IFS= read -r f; do
		case "$(file -b "$f")" in *Mach-O*) echo "$f" ;; esac
	done
}

# Ad-hoc sign every Mach-O below $1 (after all install_name_tool changes; replaces linker signatures).
adhoc_sign_tree() {
	local n=0 f
	while IFS= read -r f; do
		codesign --force --sign - --timestamp=none "$f" >/dev/null 2>&1 || die "codesign failed: $f ($(codesign --force --sign - --timestamp=none "$f" 2>&1 | tail -1))"
		n=$((n + 1))
	done < <(macho_files "$1")
	echo "    $n Mach-O files ad-hoc signed"
}

LICENSES_TSV="$WORK/licenses.tsv"
SEP=$'\x1f'
add_license() { printf "%s${SEP}%s${SEP}%s${SEP}%s${SEP}%s\n" "$@" >>"$LICENSES_TSV"; }
PARTS_TSV="$WORK/parts.tsv"
add_part() { printf '%s\t%s\n' "$1" "$2" >>"$PARTS_TSV"; }

case "$OUT" in / | "$HOME" | "$REPO" | "$OUT_PARENT") die "refusing to empty --out $OUT" ;; esac
clean_dir "$OUT"
mkdir -p "$NOTICES" "$WORK"
: >"$LICENSES_TSV"
: >"$PARTS_TSV"
trap 'rm -rf "$WORK"' EXIT
for d in "$OUT_PARENT" "$(dirname "$CACHE")"; do
	if [[ -d "$d" && ! -f "$d/.gitignore" && "$d" == "$REPO/results/"* ]]; then
		printf '# package build output (apps/desktop/pack/pack-macos.sh) – do not version\n*\n' >"$d/.gitignore"
	fi
done

# ---------------------------------------------------------------------------- own programs + UI
BIN="$TARGET_DIR/release"
[[ -x "$BIN/paperrust" ]] || [[ ! -x "$TARGET_DIR/$TARGET/release/paperrust" ]] || BIN="$TARGET_DIR/$TARGET/release"
step "PaperRust programs from $BIN"
for exe in paperrust paperrust-desktop paperrust-pdftool; do
	[[ -x "$BIN/$exe" ]] || die "$BIN/$exe missing (cargo build --release, see the header of this script)"
	install -m 0755 "$BIN/$exe" "$OUT/$exe"
done
if [[ -x "$BIN/paperrust-migrate-db" ]]; then
	install -m 0755 "$BIN/paperrust-migrate-db" "$OUT/"
else
	warn "paperrust-migrate-db missing – first start without \"adopt database\""
fi
APP_BIN="$BIN/paperrust-app"
[[ -x "$APP_BIN" ]] || APP_BIN="$REPO/apps/desktop-shell/target/release/paperrust-app"
if [[ -x "$APP_BIN" ]]; then
	install -m 0755 "$APP_BIN" "$OUT/paperrust-app"
elif ((MAKE_APP)); then
	die "paperrust-app (Tauri shell) missing, needed for --app/--dmg (cd apps/desktop-shell && cargo build --release)"
else
	warn "paperrust-app (Tauri shell) missing – package starts in the browser (paperrust-desktop run)"
fi
for f in "$OUT"/paperrust*; do
	a="$(lipo -archs "$f" 2>/dev/null || true)"
	[[ " $a " == *" $LIPO_ARCH "* ]] || die "$(basename "$f") is not built for $LIPO_ARCH (lipo: ${a:-not Mach-O})"
done
progs="{"
for f in "$OUT"/paperrust*; do
	progs+="\"$(basename "$f")\": {\"size\": $(fsize "$f"), \"sha256\": \"$(sha256 "$f")\"},"
done
add_part programs "${progs%,}}"

step "routes.toml from $ROUTES_FILE"
[[ -f "$ROUTES_FILE" ]] || die "$ROUTES_FILE missing"
cp "$ROUTES_FILE" "$OUT/routes.toml"
if grep -Eq '^"[^"]+"[[:space:]]*=[[:space:]]*"django"' "$OUT/routes.toml"; then die "routes.toml still contains django entries"; fi
add_part routes "{\"source\": \"$(basename "$ROUTES_FILE")\", \"count\": $(grep -Ec '^"[^"]+"[[:space:]]*=' "$OUT/routes.toml" || true)}"

step "UI (static) from $STATIC_DIR"
[[ -f "$STATIC_DIR/base.css" ]] || die "$STATIC_DIR does not look like STATIC_ROOT (base.css missing) – bash scripts/build-static.sh"
clean_dir "$OUT/static"
cp -R "$STATIC_DIR/." "$OUT/static/"
find "$OUT/static" -type f \( -name '*.gz' -o -name '*.br' \) -delete
add_part static "{\"source\": \"results/static\"}"

step "UI (light) from $LIGHT_DIR"
[[ -f "$LIGHT_DIR/index.html" ]] || die "$LIGHT_DIR contains no UI build (index.html missing; cd frontend-light; pnpm build)"
clean_dir "$OUT/light"
cp -R "$LIGHT_DIR/." "$OUT/light/"
find "$OUT/light" -type f \( -name '*.gz' -o -name '*.br' \) -delete
add_part light "{\"source\": \"frontend-light/dist\"}"

step "start script, README, LICENSE"
install -m 0644 "$REPO/LICENSE" "$OUT/LICENSE"
[[ -f "$REPO/NOTICE" ]] && install -m 0644 "$REPO/NOTICE" "$OUT/NOTICE"
mkdir -p "$NOTICES/paperless-ngx"
[[ -f "$REPO/upstream/LICENSE" ]] && cp "$REPO/upstream/LICENSE" "$NOTICES/paperless-ngx/"
cat >"$OUT/PaperRust.sh" <<'SH'
#!/bin/bash
# Start PaperRust (portable folder). Data: ~/Library/Application Support/PaperRust
#   ./PaperRust.sh             window + tray (paperrust-app), otherwise browser
#   ./PaperRust.sh --portable  data in ./data next to the program (do not put it in iCloud/Dropbox!)
#   ./PaperRust.sh --console   without window: paperrust-desktop run (Ctrl+C quits)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
console=0
for a in "$@"; do
	case "$a" in
	--portable) export PAPERRUST_DESKTOP_HOME="$here/data" ;;
	--console) console=1 ;;
	*) echo "unknown: $a (--portable, --console)" >&2; exit 2 ;;
	esac
done
if xattr -p com.apple.quarantine "$here/paperrust-desktop" >/dev/null 2>&1; then
	echo "Note: the folder is still quarantined (downloaded file). If macOS blocks a program:" >&2
	echo "      xattr -dr com.apple.quarantine \"$here\"" >&2
fi
if ((console == 0)) && [[ -x "$here/paperrust-app" ]]; then
	exec "$here/paperrust-app"
fi
exec "$here/paperrust-desktop" run
SH
chmod 0755 "$OUT/PaperRust.sh"
cat >"$OUT/README.txt" <<TXT
PaperRust $VERSION für macOS ($ARCH, portabel, unsigniert)
=====================================================

Empfohlen: PaperRust-$VERSION-macos-$ARCH.dmg (App zum Ziehen nach "Programme").
Dieser Ordner ist die portable Variante für das Terminal.

Erstes Öffnen (App ist nicht notarisiert, nur ad-hoc signiert):
  xattr -dr com.apple.quarantine "<dieser Ordner>"     (bzw. /Applications/PaperRust.app)
  oder: Systemeinstellungen > Datenschutz & Sicherheit > "Dennoch öffnen".

Start:      ./PaperRust.sh            (Fenster mit paperrust-app, sonst Browser)
Daten:      ~/Library/Application Support/PaperRust  (Datenbank, Dokumente, Logs, Sicherungen)
Portabel:   ./PaperRust.sh --portable -> Daten in ./data (nicht in iCloud/Dropbox ablegen!)
Konsole:    ./paperrust-desktop run | backup | stop | paths | manage <befehl>

Erster Start: Formular "Willkommen" (Fenster) bzw. Rückfragen in der Konsole - neu beginnen,
paperless-ngx-Export importieren oder bestehende paperless-ngx-Datenbank übernehmen.
Texterkennung (Tesseract mit Deutsch/Englisch) ist enthalten.

Für einen Server statt eines Desktops: Docker-Image bzw. docs/user/installation.md#docker.
Lizenz: GPL-3.0 (LICENSE). Fremdteile: THIRD-PARTY-LICENSES.txt
TXT
add_license "PaperRust (paperrust, paperrust-desktop, paperrust-app, paperrust-migrate-db, paperrust-pdftool)" "$VERSION" "GPL-3.0-only" "https://github.com/paperrust/paperrust" "LICENSE, NOTICE"
add_license "paperless-ngx (Oberfläche static/, Schema, Verhalten)" "3.4.2" "GPL-3.0" "https://github.com/paperless-ngx/paperless-ngx" "THIRD-PARTY-LICENSES/paperless-ngx/LICENSE"

# ---------------------------------------------------------------------------- PostgreSQL
pg_version="$(src_get postgresql version)"
url="$(src_get postgresql targets "$TARGET" url)"
sha="$(src_get postgresql targets "$TARGET" sha256)"
prefix="$(src_get postgresql targets "$TARGET" strip_prefix)"
[[ -n "$url" ]] || die "sources.json: no PostgreSQL for $TARGET"
step "PostgreSQL $pg_version"
arc="$(get_pinned PostgreSQL "$url" "$sha")"
tmp="$WORK/pg"
clean_dir "$tmp"
tar -xzf "$arc" -C "$tmp"
root="$tmp/$prefix"
[[ -d "$root/bin" ]] || die "PostgreSQL archive: $prefix/bin missing"
pg="$OUT/pgsql"
clean_dir "$pg"
for d in bin lib share; do mv "$root/$d" "$pg/"; done
# Thinning: developer files, Python (plpython is not used; lib/Python is the Python runtime), static
# libraries, JIT (libLLVM; jit = off), tools that are not needed
rm -rf "$pg/lib/pgxs" "$pg/lib/postgresql/pgxs" "$pg/lib/pkgconfig" "$pg/lib/Python" "$pg/bin/ecpg" "$pg/bin/pgbench"
find "$pg/lib" \( -name '*.a' -o -name '*plpython*' -o -name 'libLLVM*' -o -name 'llvmjit*' -o -name '*.bc' \) -prune -exec rm -rf {} +
find "$pg/lib" -type d -name bitcode -prune -exec rm -rf {} +
find "$pg/share" -path '*extension/*plpython*' -exec rm -rf {} + 2>/dev/null || true
mkdir -p "$NOTICES/postgresql"
[[ -f "$root/LICENSE" ]] && cp "$root/LICENSE" "$NOTICES/postgresql/"
[[ -f "$root/COPYRIGHT" ]] && cp "$root/COPYRIGHT" "$NOTICES/postgresql/"
[[ -d "$root/dependency-notices" ]] && cp -R "$root/dependency-notices" "$NOTICES/postgresql/"
rm -rf "$NOTICES/postgresql/dependency-notices/python" 2>/dev/null || true
mb="$(du -sm "$pg" | cut -f1)"
echo "    pgsql thinned out: $mb MB"
add_part postgresql "{\"version\": \"$pg_version\", \"sha256\": \"$sha\", \"url\": \"$url\", \"size_mb\": $mb}"
add_license "PostgreSQL (pgsql/)" "$pg_version" "PostgreSQL License" "$url" "THIRD-PARTY-LICENSES/postgresql/LICENSE"
add_license "  gebündelte Bibliotheken von PostgreSQL" "" "ICU (Unicode-3.0), OpenSSL (Apache-2.0), libxml2/libxslt (MIT), readline (GPL-3.0), lz4 (BSD-2), zstd (BSD-3)" "$url" "THIRD-PARTY-LICENSES/postgresql/dependency-notices/"

# ---------------------------------------------------------------------------- PDFium
ver="$(src_get pdfium version)"
url="$(src_get pdfium targets "$TARGET" url)"
sha="$(src_get pdfium targets "$TARGET" sha256)"
file_in="$(src_get pdfium targets "$TARGET" file)"
[[ -n "$url" ]] || die "sources.json: no PDFium for $TARGET"
step "PDFium $ver"
arc="$(get_pinned PDFium "$url" "$sha")"
tmp="$WORK/pdfium"
clean_dir "$tmp"
tar -xzf "$arc" -C "$tmp"
clean_dir "$OUT/pdfium"
install -m 0755 "$tmp/$file_in" "$OUT/pdfium/libpdfium.dylib"
mkdir -p "$NOTICES/pdfium"
cp -R "$tmp"/LICENSE* "$NOTICES/pdfium/" 2>/dev/null || true
add_part pdfium "{\"version\": \"$ver\", \"sha256\": \"$sha\", \"url\": \"$url\"}"
add_license "PDFium (pdfium/libpdfium.dylib)" "$ver" "BSD-3-Clause; enthält FreeType (FTL), ICU (Unicode-3.0), libjpeg-turbo, libpng, zlib, OpenJPEG (BSD-2), lcms2 (MIT), abseil (Apache-2.0) …" "$url" "THIRD-PARTY-LICENSES/pdfium/"

# ---------------------------------------------------------------------------- Tesseract (bundled)
if ((WITH_TESSERACT)); then
	need brew "Homebrew, for tesseract"
	sys_tess="$(command -v tesseract || true)"
	[[ -n "$sys_tess" ]] || die "tesseract missing on the build machine (brew install tesseract) – or --no-tesseract"
	tess_prefix="$(brew --prefix tesseract)"
	libtess="$tess_prefix/lib/libtesseract.5.dylib"
	[[ -f "$libtess" ]] || die "$libtess missing (Homebrew tesseract without libtesseract.5?) – ls $tess_prefix/lib: $(ls "$tess_prefix/lib" | tr '\n' ' ')"
	tver="$(tesseract --version 2>&1 | sed -n 1p)"
	step "Tesseract ($tver) with its dylibs to tesseract/ (@loader_path)"
	python3 -I "$DYLIBS" bundle --dest "$OUT/tesseract" --manifest "$WORK/tesseract.json" "$sys_tess" "$libtess"
	[[ -x "$OUT/tesseract/tesseract" && -f "$OUT/tesseract/libtesseract.5.dylib" ]] || die "tesseract bundle incomplete"
	# licenses: license files of every Homebrew formula the files come from (keg root)
	mkdir -p "$NOTICES/tesseract"
	formulas="$(python3 -I -c 'import json,sys
for f in json.load(open(sys.argv[1]))["files"]:
    if f["formula"]: print(f["formula"] + " " + f["version"])' "$WORK/tesseract.json" | sort -u)"
	cellar="$(brew --cellar)"
	echo "$formulas" | while read -r fm fv; do
		[[ -n "$fm" ]] || continue
		mkdir -p "$NOTICES/tesseract/$fm"
		found=0
		for lf in "$cellar/$fm/$fv"/LICENSE* "$cellar/$fm/$fv"/COPYING* "$cellar/$fm/$fv"/COPYRIGHT* "$cellar/$fm/$fv"/NOTICE* "$cellar/$fm/$fv"/LICENCE*; do
			[[ -f "$lf" ]] && cp "$lf" "$NOTICES/tesseract/$fm/" && found=1
		done
		((found)) || warn "no license file in $cellar/$fm/$fv (formula $fm) – see https://formulae.brew.sh/formula/$fm"
		echo "$fm $fv" >>"$NOTICES/tesseract/FORMULAS.txt"
	done
	cp "$WORK/tesseract.json" "$NOTICES/tesseract/bundle.json"
	nlibs="$(ls "$OUT/tesseract" | grep -c '\.dylib$' || true)"
	add_part tesseract "{\"version\": \"$tver\", \"source\": \"Homebrew on the build machine (macOS $(sw_vers -productVersion))\", \"libs\": $nlibs}"
	add_license "Tesseract + Leptonica + Bildbibliotheken (tesseract/)" "$tver" "Apache-2.0 (Tesseract), BSD-2 (Leptonica), weitere je Homebrew-Formel" "https://github.com/tesseract-ocr/tesseract" "THIRD-PARTY-LICENSES/tesseract/"

	ver="$(src_get tessdata version)"
	step "language data $ver (tesseract/tessdata)"
	dst="$OUT/tesseract/tessdata"
	mkdir -p "$dst"
	done_files=""
	for f in eng.traineddata deu.traineddata osd.traineddata; do
		u="$(src_get tessdata files "$f" url)"
		[[ -n "$u" ]] || continue
		p="$(get_pinned "$f" "$u" "$(src_get tessdata files "$f" sha256)")"
		install -m 0644 "$p" "$dst/$f"
		done_files+="\"$f\","
	done
	add_part tessdata "{\"version\": \"$ver\", \"files\": [${done_files%,}]}"
	add_license "Tesseract-Sprachdaten (tesseract/tessdata)" "$ver" "Apache-2.0" "https://github.com/tesseract-ocr/tessdata_fast" "THIRD-PARTY-LICENSES/common/Apache-2.0.txt"
else
	warn "without Tesseract: scans get no text unless a system tesseract is configured (PAPERRUST_TESSERACT_PATH)"
fi

# ---------------------------------------------------------------------------- font (text thumbnails)
ver="$(src_get fonts version)"
step "font $ver"
arc="$(get_pinned Liberation-fonts "$(src_get fonts url)" "$(src_get fonts sha256)")"
tmp="$WORK/fonts"
clean_dir "$tmp"
tar -xzf "$arc" -C "$tmp"
froot="$tmp/$(src_get fonts strip_prefix)"
clean_dir "$OUT/fonts"
install -m 0644 "$froot/LiberationSerif-Regular.ttf" "$OUT/fonts/"
mkdir -p "$NOTICES/liberation-fonts"
[[ -f "$froot/LICENSE" ]] || die "Liberation: LICENSE (SIL OFL 1.1) missing in the archive"
cp "$froot/LICENSE" "$NOTICES/liberation-fonts/"
add_part fonts "{\"version\": \"$ver\", \"files\": [\"LiberationSerif-Regular.ttf\"]}"
add_license "Liberation-Schriften (LiberationSerif-Regular.ttf; fonts/)" "$ver" "SIL Open Font License 1.1" "https://github.com/liberationfonts/liberation-fonts" "THIRD-PARTY-LICENSES/liberation-fonts/LICENSE"

# ---------------------------------------------------------------------------- Rust crates + overview
step "licenses: Rust dependencies (cargo metadata), standard texts, overview"
mkdir -p "$NOTICES/common"
cp "$HERE"/licenses/*.txt "$NOTICES/common/" 2>/dev/null || true
crates_file="$NOTICES/rust-crates.txt"
if command -v cargo >/dev/null; then
	{
		for ws in "$REPO" "$REPO/apps/desktop-shell"; do
			[[ -f "$ws/Cargo.toml" ]] || continue
			(cd "$ws" && { cargo metadata --format-version 1 --locked --offline --filter-platform "$TARGET" 2>/dev/null ||
				cargo metadata --format-version 1 --locked --filter-platform "$TARGET" 2>/dev/null; }) ||
				die "cargo metadata in $ws failed – no package without a complete license list"
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
	warn "cargo missing – rust-crates.txt not generated"
fi
[[ -x "$OUT/paperrust-app" ]] && add_license "Tauri 2, wry, tao" "" "MIT/Apache-2.0" "https://tauri.app" "THIRD-PARTY-LICENSES/rust-crates.txt"

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

# ---------------------------------------------------------------------------- references + signatures
step "check library references (nothing may point to the build machine)"
if ! problems="$(python3 -I "$DYLIBS" check "$OUT")"; then
	echo "$problems" >&2
	die "Mach-O files reference libraries outside the package (see above)"
fi
xattr -cr "$OUT" 2>/dev/null || true
step "ad-hoc signatures (codesign -s -)"
adhoc_sign_tree "$OUT"
MINOS="$(python3 -I "$DYLIBS" minos $(macho_files "$OUT"))"
[[ -n "$MINOS" ]] || MINOS="11.0"
echo "    minimum macOS version of the bundled files: $MINOS"

# ---------------------------------------------------------------------------- self tests
if ((SKIP_SELF_CHECK == 0)) && [[ "$HOST_ARCH" != "$ARCH" ]]; then
	warn "self tests skipped (package $ARCH, machine $HOST_ARCH)"
elif ((SKIP_SELF_CHECK == 0)); then
	step "self test: programs start"
	"$OUT/paperrust-desktop" --version || die "paperrust-desktop --version failed"
	"$OUT/paperrust-desktop" --help >/dev/null || die "paperrust-desktop --help failed"
	"$OUT/pgsql/bin/postgres" --version || die "postgres --version failed"

	step "self test PostgreSQL (initdb ICU, start, SELECT 1, stop)"
	data="$WORK/pgtest"
	port=$((55100 + RANDOM % 800))
	"$pg/bin/initdb" -D "$data" -U selftest --auth=trust --encoding=UTF8 \
		--locale-provider=icu --icu-locale=und --locale=C >"$WORK/initdb.log" 2>&1 || { cat "$WORK/initdb.log" >&2; die "self test: initdb failed"; }
	"$pg/bin/pg_ctl" start -D "$data" -w -t 60 -s -l "$WORK/pgtest.log" \
		-o "-p $port -c listen_addresses=127.0.0.1 -c unix_socket_directories='' -c jit=off" >/dev/null ||
		{ cat "$WORK/pgtest.log" >&2; die "self test: start failed"; }
	r="$("$pg/bin/psql" -h 127.0.0.1 -p "$port" -U selftest -d postgres -tAc "select 1, current_setting('server_version')" || true)"
	"$pg/bin/pg_ctl" stop -D "$data" -m fast -w -s || true
	[[ "$r" == 1\|17.* ]] || die "self test: query failed ($r)"
	echo "    ok: $r"

	if ((WITH_TESSERACT)); then
		step "self test Tesseract (bundled, no Homebrew libraries loaded)"
		t="$OUT/tesseract/tesseract"
		env -i HOME="$HOME" "$t" --version >/dev/null 2>&1 || { env -i HOME="$HOME" "$t" --version >&2 || true; die "bundled tesseract does not start"; }
		brew_prefix="$(brew --prefix)"
		leaked="$(DYLD_PRINT_LIBRARIES=1 "$t" --version 2>&1 | grep -F "$brew_prefix/" || true)"
		[[ -z "$leaked" ]] || die "bundled tesseract still loads Homebrew libraries: $leaked"
		langs="$("$t" --tessdata-dir "$OUT/tesseract/tessdata" --list-langs 2>&1 | tail -n +2 | tr '\n' ',')"
		echo "    languages: $langs"
		[[ "$langs" == *deu* && "$langs" == *eng* ]] || die "Tesseract self test: deu/eng missing ($langs)"
		sample="$REPO/upstream/src/documents/tests/samples/simple.png"
		if [[ -f "$sample" ]]; then
			txt="$("$t" --tessdata-dir "$OUT/tesseract/tessdata" "$sample" - -l eng 2>/dev/null | tr '\n' ' ')"
			echo "    OCR simple.png: $txt"
			[[ "$(lower "$txt")" == *test* ]] || die "Tesseract self test: OCR of simple.png did not find 'test'"
		fi
		# libtesseract as the server loads it (dlopen). Only a warning: the python3 on the build machine
		# may use library validation and refuse ad-hoc signed libraries for its own reasons.
		lt="$(python3 -I -c 'import ctypes,sys
l = ctypes.CDLL(sys.argv[1]); l.TessVersion.restype = ctypes.c_char_p; print(l.TessVersion().decode())' "$OUT/tesseract/libtesseract.5.dylib" 2>&1 || true)"
		echo "    dlopen libtesseract.5.dylib: $lt"
	fi
fi

# ---------------------------------------------------------------------------- PACKAGE.json
total="$(($(du -sk "$OUT" | cut -f1) * 1024))"
python3 -I - "$PARTS_TSV" "$OUT/PACKAGE.json" "$VERSION" "$TARGET" "$total" "$MINOS" <<'PY'
import datetime, json, sys
parts_file, out, version, target, total, minos = sys.argv[1:7]
parts = {}
for line in open(parts_file, encoding="utf-8"):
    name, _, js = line.rstrip("\n").partition("\t")
    if name:
        parts[name] = json.loads(js)
m = {"name": "PaperRust", "version": version, "target": target,
     "created": datetime.datetime.now(datetime.timezone.utc).isoformat(), "signed": False,
     "codesign": "ad-hoc", "minimum_macos": minos, "parts": parts, "size_bytes": int(total)}
open(out, "w", encoding="utf-8").write(json.dumps(m, indent=2, ensure_ascii=False) + "\n")
PY
step "directory done: $OUT ($((total / 1024 / 1024)) MB)"

if ((MAKE_TAR)); then
	tarball="$OUT_PARENT/$(basename "$OUT").tar.gz"
	step "tar.gz $tarball"
	rm -f "$tarball"
	# bsdtar: owner neutral, no AppleDouble/xattr files (._*)
	COPYFILE_DISABLE=1 tar --uid 0 --gid 0 --numeric-owner -czf "$tarball" -C "$OUT_PARENT" "$(basename "$OUT")"
	echo "    $(($(fsize "$tarball") / 1024 / 1024)) MB, SHA-256 $(sha256 "$tarball")"
fi

# ---------------------------------------------------------------------------- PaperRust.app (Tauri bundler)
if ((MAKE_APP)); then
	cli_ver="$(src_get tools tauri_cli version)"
	cli_url="$(src_get tools tauri_cli targets "$TARGET" url)"
	cli_sha="$(src_get tools tauri_cli targets "$TARGET" sha256)"
	[[ -n "$cli_url" ]] || die "sources.json: no tauri-cli for $TARGET (tools.tauri_cli.targets)"
	cli_dir="$CACHE/tauri-cli-$cli_ver-$TARGET"
	cli="$cli_dir/cargo-tauri"
	if [[ ! -x "$cli" ]]; then
		step "tauri-cli $cli_ver"
		z="$(get_pinned tauri-cli "$cli_url" "$cli_sha")"
		clean_dir "$cli_dir"
		(cd "$cli_dir" && unzip -q "$z")
		if [[ ! -x "$cli" ]]; then
			f="$(find "$cli_dir" -type f -name cargo-tauri | sed -n 1p)"
			if [[ -n "$f" ]]; then mv "$f" "$cli" && chmod 0755 "$cli"; fi
		fi
		[[ -x "$cli" ]] || die "tauri-cli: cargo-tauri not found in $z"
	fi
	"$cli" --version

	step "bundle configuration (main program only; resources are copied afterwards)"
	conf="$WORK/tauri.bundle.json"
	python3 -I - "$conf" "$VERSION" "$MINOS" <<'PY'
import json, sys
out, version, minos = sys.argv[1:4]
conf = {
    "productName": "PaperRust",
    "mainBinaryName": "paperrust-app",
    "version": version,
    "bundle": {
        "active": True,
        "targets": ["app"],
        "publisher": "PaperRust",
        "copyright": "GPL-3.0-only; based on paperless-ngx",
        "category": "Productivity",
        "shortDescription": "Dokumentenarchiv (paperless-ngx-kompatibel), lokal ohne Server",
        "icon": ["icons/icon.png"],
        "macOS": {"minimumSystemVersion": minos},
    },
}
open(out, "w", encoding="utf-8").write(json.dumps(conf, indent=2, ensure_ascii=False) + "\n")
PY
	cat "$conf"
	# tauri bundle takes target/release/paperrust-app; make sure it is the one packed above
	[[ -x "$TARGET_DIR/release/paperrust-app" ]] || die "$TARGET_DIR/release/paperrust-app missing (tauri bundle does not build)"
	bundle_dir="$TARGET_DIR/release/bundle/macos"
	rm -rf "$bundle_dir"
	step "cargo tauri bundle --bundles app"
	# No signing identity: unset anything the environment might pass to the bundler.
	(cd "$REPO/apps/desktop-shell" && env -u APPLE_SIGNING_IDENTITY -u APPLE_CERTIFICATE CARGO_TARGET_DIR="$TARGET_DIR" \
		"$cli" bundle --bundles app --config "$conf") || die "tauri bundle failed"
	built_app="$(find "$bundle_dir" -maxdepth 1 -name '*.app' -type d | sed -n 1p)"
	[[ -n "$built_app" ]] || die "no .app in $bundle_dir"
	APP="$OUT_PARENT/PaperRust.app"
	rm -rf "$APP"
	ditto "$built_app" "$APP"
	[[ -x "$APP/Contents/MacOS/paperrust-app" ]] || die "$APP/Contents/MacOS/paperrust-app missing (bundle layout: $(ls -R "$APP/Contents" | head -20))"

	step "package into PaperRust.app/Contents/Resources"
	res="$APP/Contents/Resources"
	mkdir -p "$res"
	ditto "$OUT" "$res"
	rm -rf "$res/paperrust-app" "$res/PaperRust.sh" "$res/data"
	# README for the app: the portable instructions do not apply
	sed -e 's/^Dieser Ordner ist die portable Variante für das Terminal./Programmdateien in PaperRust.app\/Contents\/Resources./' "$OUT/README.txt" >"$res/README.txt"
	[[ -x "$res/paperrust" && -x "$res/pgsql/bin/postgres" ]] || die "resources incomplete in $res"
	xattr -cr "$APP" 2>/dev/null || true
	step "ad-hoc signatures for PaperRust.app"
	adhoc_sign_tree "$res"
	codesign --force --sign - --timestamp=none "$APP/Contents/MacOS/paperrust-app"
	codesign --force --sign - --timestamp=none "$APP" || die "codesign of $APP failed"
	codesign --verify --verbose=2 "$APP" || die "codesign --verify of $APP failed"
	codesign --verify --deep --strict --verbose=2 "$APP" 2>&1 || warn "codesign --verify --deep --strict reports problems (see above)"
	codesign -dv "$APP" 2>&1 | sed 's/^/    /'
	/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' -c 'Print :CFBundleShortVersionString' -c 'Print :LSMinimumSystemVersion' "$APP/Contents/Info.plist" | sed 's/^/    Info.plist: /' || true
	if ((SKIP_SELF_CHECK == 0)) && [[ "$HOST_ARCH" == "$ARCH" ]]; then
		# Same path resolution as the window program: a copy of paperrust-desktop in Contents/MacOS of a
		# copy of the app must find everything in Contents/Resources (layout::install_root).
		step "self test: install root inside the app bundle"
		t="$WORK/apptest"
		clean_dir "$t"
		ditto "$APP" "$t/PaperRust.app"
		cp "$res/paperrust-desktop" "$t/PaperRust.app/Contents/MacOS/paperrust-desktop"
		paths_out="$(PAPERRUST_DESKTOP_HOME="$WORK/home" "$t/PaperRust.app/Contents/MacOS/paperrust-desktop" paths)" || die "paperrust-desktop paths failed"
		echo "$paths_out" | sed 's/^/    /'
		grep -q '^Installation:.*PaperRust\.app/Contents/Resources$' <<<"$paths_out" || die "install root is not Contents/Resources (layout::install_root)"
		[[ "$paths_out" != *FEHLT* ]] || die "paperrust-desktop paths reports missing parts in the app"
		if ((WITH_TESSERACT)) && [[ "$paths_out" == *"nicht gefunden"* ]]; then die "paperrust-desktop paths: optional parts not found in the app"; fi
		rm -rf "$t" "$WORK/home"
	fi
	app_mb="$(du -sm "$APP" | cut -f1)"
	echo "    $APP ($app_mb MB)"

	if ((MAKE_DMG)); then
		dmg="$OUT_PARENT/$PACK_NAME.dmg"
		step "dmg $dmg (hdiutil)"
		stage="$WORK/dmg"
		clean_dir "$stage"
		ditto "$APP" "$stage/PaperRust.app"
		ln -s /Applications "$stage/Applications"
		cat >"$stage/README.txt" <<TXT
PaperRust $VERSION für macOS ($ARCH) – unsigniert (nur ad-hoc signiert, nicht notarisiert)

1. PaperRust.app auf "Applications" (Programme) ziehen.
2. Einmalig im Terminal:  xattr -dr com.apple.quarantine /Applications/PaperRust.app
   (oder: App öffnen, dann Systemeinstellungen > Datenschutz & Sicherheit > "Dennoch öffnen").
3. PaperRust starten. Daten: ~/Library/Application Support/PaperRust

English: drag PaperRust.app to Applications, then run
   xattr -dr com.apple.quarantine /Applications/PaperRust.app
(or System Settings > Privacy & Security > "Open Anyway"). Requires macOS $MINOS or newer.
TXT
		rm -f "$dmg"
		ok=0
		for attempt in 1 2 3; do
			if hdiutil create -volname "PaperRust $VERSION" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"; then
				ok=1
				break
			fi
			warn "hdiutil create failed (attempt $attempt) – retrying in 10 s"
			sleep 10
		done
		((ok)) || die "hdiutil create failed"
		hdiutil verify "$dmg" >/dev/null || die "hdiutil verify $dmg failed"
		mnt="$WORK/mnt"
		mkdir -p "$mnt"
		hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$dmg" >/dev/null || die "hdiutil attach failed"
		dmg_ok=1
		codesign --verify --verbose=1 "$mnt/PaperRust.app" || dmg_ok=0
		[[ -L "$mnt/Applications" ]] || dmg_ok=0
		hdiutil detach "$mnt" >/dev/null || hdiutil detach -force "$mnt" >/dev/null || true
		((dmg_ok)) || die "mounted dmg: app signature invalid or Applications link missing"
		echo "    $(($(fsize "$dmg") / 1024 / 1024)) MB, SHA-256 $(sha256 "$dmg")"
	fi
fi
step "done"
