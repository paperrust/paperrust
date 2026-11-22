#!/bin/sh
# PaperRust server package for FreeBSD (no desktop app; the web UI is served by `paperrust`).
#
# Runs INSIDE a FreeBSD 14.x machine (CI: .github/workflows/freebsd.yml in a vmactions/freebsd-vm VM;
# by hand: any FreeBSD 14 amd64 box with network), as root, from the repository root:
#
#   sh packaging/freebsd/build.sh
#
# Input (all optional, environment):
#   PAPERRUST_VERSION   version for file names (default: [workspace.package] version in Cargo.toml)
#   FRONTEND_TAR        tar with frontend-light/dist + results/static (CI job "frontend-dist");
#                       default ./frontend-dist.tar. Without it, frontend-light/dist and results/static
#                       must already exist (built on Linux/macOS: pnpm build + scripts/build-static.sh).
#   LTO, CODEGEN_UNITS  release profile override (default Cargo.toml: fat, 1); small machines: thin, 16
#   BUILD_JOBS          cargo -j (default: all cores)
#   CARGO_TARGET_DIR    default /var/tmp/paperrust-target (outside the work tree: CI copies the work
#                       tree back to the runner after every step)
#   OUT_DIR             default ./out
#   SKIP_PKG_INSTALL=1  do not install build/runtime packages with pkg(8)
#   MAKE_PKG=0          only the tar.gz, no native pkg(8) package
#
# Output in OUT_DIR:
#   PaperRust-<ver>-freebsd-<arch>.tar.gz   tree for PREFIX=/usr/local (bin/, share/paperrust/, etc/rc.d/ …)
#   paperrust-<pkgver>.pkg                  native package (pkg add), same files + user/group setup
#   SHA256SUMS-freebsd-<arch>.txt
#
# Layout (both artifacts):
#   bin/paperrust, bin/paperrust-migrate-db, bin/paperrust-pdftool   (pdftool must sit next to paperrust)
#   share/paperrust/static/   STATIC_ROOT (PAPERRUST_STATICDIR)
#   share/paperrust/light/    web UI, found as <STATICDIR>/../light (pl-server light_ui::configured_dir)
#   share/paperrust/routes.toml
#   etc/rc.d/paperrust, etc/paperrust.conf.sample
#   share/doc/paperrust/README.md, share/licenses/paperrust/
set -eu
set -x

REPO="$(pwd)"
[ -f "$REPO/Cargo.toml" ] && [ -d "$REPO/crates/pl-server" ] || { echo "run from the repository root" >&2; exit 1; }
HERE="$REPO/packaging/freebsd"

log() { printf '\n[freebsd-build] %s\n' "$*" >&2; }
die() { printf '[freebsd-build] ERROR: %s\n' "$*" >&2; exit 1; }

log "system"
freebsd-version || true
uname -a
sysctl -n hw.ncpu hw.physmem || true
df -h / /var/tmp || true

case "$(uname -m)" in
amd64 | x86_64) ARCH=x86_64 ;;
arm64 | aarch64) ARCH=aarch64 ;;
*) ARCH="$(uname -m)" ;;
esac

VERSION="${PAPERRUST_VERSION:-}"
if [ -z "$VERSION" ]; then
	VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$REPO/Cargo.toml" | sed -n 1p)"
fi
[ -n "$VERSION" ] || die "version not found in Cargo.toml"
# pkg(8) versions must not contain '-' (0.1.3-rc1 -> 0.1.3.rc1)
PKGVERSION="$(printf '%s' "$VERSION" | tr '-' '.')"
OUT_DIR="${OUT_DIR:-$REPO/out}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/var/tmp/paperrust-target}"
WORK="${WORK_DIR:-/var/tmp/paperrust-pack}"
NAME="PaperRust-$VERSION-freebsd-$ARCH"
log "version $VERSION (pkg $PKGVERSION), arch $ARCH, target dir $CARGO_TARGET_DIR"

# ------------------------------------------------------------------------------------------ packages
# Build: a C compiler is in base (clang); ring, zstd-sys, lzma-sys, libmimalloc-sys and sqlite-vec
# compile C with it. git for the git dependencies (PdfCraft/WordCraft), curl for rustup.
# Runtime (also needed for the pkg dependency versions and the smoke test): tesseract (+ tesseract-data),
# pdfium (graphics/pdfium ships libpdfium.so), liberation-fonts-ttf, ca_root_nss.
RUNTIME_PKGS="tesseract pdfium liberation-fonts-ttf ca_root_nss"
if [ "${SKIP_PKG_INSTALL:-0}" != 1 ]; then
	log "pkg install"
	export ASSUME_ALWAYS_YES=yes
	pkg -N >/dev/null 2>&1 || pkg bootstrap -f
	pkg update -f
	# shellcheck disable=SC2086
	pkg install -y curl git pkgconf $RUNTIME_PKGS
fi

# ------------------------------------------------------------------------------------------ Rust
# rustup instead of pkg's rust: the toolchain is pinned in rust-toolchain.toml (same compiler as
# the Linux/Windows/macOS releases). x86_64-unknown-freebsd is a tier-2 host.
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
	log "rustup"
	curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
	sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain none --no-modify-path
	rm -f /tmp/rustup-init.sh
fi
# no argument: installs the toolchain named in rust-toolchain.toml (rustup >= 1.28)
rustup toolchain install || rustup show
rustc --version
cargo --version

# ------------------------------------------------------------------------------------------ build
if [ -n "${LTO:-}" ]; then export CARGO_PROFILE_RELEASE_LTO="$LTO"; fi
if [ -n "${CODEGEN_UNITS:-}" ]; then export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="$CODEGEN_UNITS"; fi
JOBS=""
if [ -n "${BUILD_JOBS:-}" ]; then JOBS="-j $BUILD_JOBS"; fi
# sqlite-vec 0.1.9 uses u_int8_t/u_int16_t/u_int64_t, which FreeBSD only declares in <sys/types.h>
# (its #ifdef for Linux/macOS does not cover FreeBSD). Force-include the header for C builds (cc crate).
export CFLAGS="${CFLAGS:-} -include sys/types.h"

log "cargo build paperrust + paperrust-migrate-db"
start=$(date +%s)
# shellcheck disable=SC2086
cargo build --release --locked $JOBS -p pl-server --bin paperrust -p pl-dbmigrate --bin paperrust-migrate-db
log "cargo build paperrust-pdftool"
# shellcheck disable=SC2086
cargo build --release --locked $JOBS -p pl-pdftools --features engine --bin paperrust-pdftool
log "cargo done in $(( $(date +%s) - start )) s"
REL="$CARGO_TARGET_DIR/release"
for b in paperrust paperrust-migrate-db paperrust-pdftool; do
	[ -x "$REL/$b" ] || die "$REL/$b missing"
done
"$REL/paperrust" --version

# ------------------------------------------------------------------------------------------ web UI
FRONTEND_TAR="${FRONTEND_TAR:-$REPO/frontend-dist.tar}"
if [ -f "$FRONTEND_TAR" ]; then
	log "unpack $FRONTEND_TAR"
	tar -xf "$FRONTEND_TAR" -C "$REPO"
fi
[ -f "$REPO/frontend-light/dist/index.html" ] || die "frontend-light/dist missing (FRONTEND_TAR or pnpm build)"
[ -f "$REPO/results/static/base.css" ] || die "results/static missing (FRONTEND_TAR or scripts/build-static.sh)"

# ------------------------------------------------------------------------------------------ tree
log "assemble tree"
rm -rf "$WORK"
ROOT="$WORK/root"              # pkg create -r
P="$ROOT/usr/local"            # PREFIX
mkdir -p "$P/bin" "$P/share/paperrust" "$P/etc/rc.d" "$P/share/doc/paperrust" \
	"$P/share/licenses/paperrust/paperless-ngx"
install -m 0755 "$REL/paperrust" "$REL/paperrust-migrate-db" "$REL/paperrust-pdftool" "$P/bin/"
cp -R "$REPO/results/static" "$P/share/paperrust/static"
cp -R "$REPO/frontend-light/dist" "$P/share/paperrust/light"
install -m 0644 "$REPO/routes.toml" "$P/share/paperrust/routes.toml"
install -m 0755 "$HERE/rc.d/paperrust" "$P/etc/rc.d/paperrust"
install -m 0644 "$HERE/paperrust.conf.sample" "$P/etc/paperrust.conf.sample"
install -m 0644 "$HERE/README.md" "$P/share/doc/paperrust/README.md"
install -m 0644 "$REPO/LICENSE" "$REPO/NOTICE" "$P/share/licenses/paperrust/"
if [ -f "$REPO/upstream/LICENSE" ]; then
	install -m 0644 "$REPO/upstream/LICENSE" "$P/share/licenses/paperrust/paperless-ngx/LICENSE"
fi
printf '%s\n' "$VERSION" >"$P/share/paperrust/VERSION"
find "$P" -type d -exec chmod 0755 {} +
find "$P/share" -type f -exec chmod 0644 {} +

# every shared library resolvable (libpdfium/libtesseract are loaded at runtime, not linked)
for b in "$P"/bin/*; do
	ldd "$b" | tee "$WORK/ldd.txt"
	if grep -q 'not found' "$WORK/ldd.txt"; then die "$b: unresolved libraries"; fi
done
rm -f "$WORK/ldd.txt"
du -sh "$P/bin"/* "$P/share/paperrust/static" "$P/share/paperrust/light"

mkdir -p "$OUT_DIR"
rm -f "$OUT_DIR/$NAME.tar.gz" "$OUT_DIR"/paperrust-*.pkg "$OUT_DIR/SHA256SUMS-freebsd-$ARCH.txt"

# ------------------------------------------------------------------------------------------ tar.gz
# One top directory that mirrors PREFIX: `tar -xzf … --strip-components 1 -C /usr/local`
log "tar.gz"
mkdir -p "$WORK/tar"
cp -Rp "$P" "$WORK/tar/$NAME"
tar -czf "$OUT_DIR/$NAME.tar.gz" -C "$WORK/tar" "$NAME"
rm -rf "$WORK/tar"

# ------------------------------------------------------------------------------------------ pkg
if [ "${MAKE_PKG:-1}" != 0 ]; then
	log "pkg create"
	ABI="$(pkg config abi)"
	deps=""
	for d in $RUNTIME_PKGS; do
		o="$(pkg query '%o' "$d" 2>/dev/null || true)"
		v="$(pkg query '%v' "$d" 2>/dev/null || true)"
		[ -n "$o" ] && [ -n "$v" ] || die "runtime package $d is not installed (needed for the pkg dependency list)"
		deps="$deps  \"$d\": { origin: \"$o\", version: \"$v\" },
"
	done
	cat >"$WORK/+MANIFEST" <<EOF
name: paperrust
version: "$PKGVERSION"
origin: "local/paperrust"
comment: "Document management server, drop-in replacement for paperless-ngx"
www: "https://paperrust.org"
maintainer: "contact@paperrust.org"
prefix: "/usr/local"
abi: "$ABI"
licenselogic: "single"
licenses: [ "GPLv3" ]
categories: [ "sysutils" ]
desc: <<EOD
PaperRust is a fast, lean server for paperless-ngx (v3.4.2) databases: same API and
database, no Python, Redis, Celery, Tika or Gotenberg. Web server, worker, scheduler and
consumption folder in one process (paperrust serve --worker). Needs PostgreSQL 14 or newer.
EOD
deps: {
$deps}
scripts: {
  post-install: <<EOD
if ! pw groupshow paperrust >/dev/null 2>&1; then
  pw groupadd paperrust
fi
if ! pw usershow paperrust >/dev/null 2>&1; then
  pw useradd paperrust -g paperrust -d /var/db/paperrust -s /usr/sbin/nologin -c "PaperRust"
fi
for d in /var/db/paperrust /var/db/paperrust/data /var/db/paperrust/media /var/db/paperrust/consume /var/db/paperrust/export /var/log/paperrust; do
  install -d -o paperrust -g paperrust -m 0750 "\$d"
done
EOD
}
messages: [
  { message: <<EOD
PaperRust is installed. Next steps (details: /usr/local/share/doc/paperrust/README.md):
  1. PostgreSQL >= 14 with an empty database (UTF8) and a user for PaperRust
  2. cp /usr/local/etc/paperrust.conf.sample /usr/local/etc/paperrust.conf, set DB and SECRET_KEY
     chown root:paperrust /usr/local/etc/paperrust.conf && chmod 0640 /usr/local/etc/paperrust.conf
  3. sysrc paperrust_enable=YES && service paperrust start   (runs manage migrate first)
  4. su -m paperrust -c 'paperrust manage createsuperuser'
Data lives in /var/db/paperrust; it is kept when the package is removed.
EOD
  }
]
EOF
	cat "$WORK/+MANIFEST"
	# plist: paths relative to PREFIX; programs and rc script executable
	(
		cd "$P"
		find . -type f | sed 's|^\./||' | sort | while IFS= read -r f; do
			case "$f" in
			bin/* | etc/rc.d/*) printf '@(root,wheel,0755) %s\n' "$f" ;;
			*) printf '@(root,wheel,0644) %s\n' "$f" ;;
			esac
		done
	) >"$WORK/plist"
	wc -l "$WORK/plist"
	pkg create -M "$WORK/+MANIFEST" -p "$WORK/plist" -r "$ROOT" -o "$OUT_DIR"
	ls -l "$OUT_DIR"
	pkgfile="$(ls "$OUT_DIR"/paperrust-*.pkg 2>/dev/null | sed -n 1p)"
	[ -n "$pkgfile" ] || die "pkg create produced no paperrust-*.pkg"
	pkg info -F "$pkgfile"
	pkg info -d -F "$pkgfile"
fi

# ------------------------------------------------------------------------------------------ sums
(cd "$OUT_DIR" && sha256 -r "$NAME.tar.gz" paperrust-*.pkg 2>/dev/null || sha256 -r "$NAME.tar.gz") >"$OUT_DIR/SHA256SUMS-freebsd-$ARCH.txt"
cat "$OUT_DIR/SHA256SUMS-freebsd-$ARCH.txt"
df -h / /var/tmp || true
log "done: $OUT_DIR"
