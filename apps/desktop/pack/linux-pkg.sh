#!/usr/bin/env bash
# PaperRust Desktop – .deb and .rpm from the finished Linux package directory of pack.sh.
#
# Layout of the installed package (the portable tree stays as it is, because PostgreSQL and the
# bundled Tesseract need their fixed relative paths, see pack.sh):
#
#   /opt/paperrust/                        the pack.sh directory (programs, pgsql/, tesseract/, light/, …)
#   /usr/bin/paperrust                     -> /opt/paperrust/PaperRust.sh (resolves its own symlink)
#   /usr/share/applications/paperrust.desktop
#   /usr/share/icons/hicolor/512x512/apps/paperrust.png
#
# Data stays per user in ${XDG_DATA_HOME:-~/.local/share}/paperrust; removing the package keeps it.
# The window (paperrust-app) needs WebKitGTK 4.1, so it is only recommended: without it PaperRust
# opens in the browser. Asset names as the ArtCraft Launcher reads them:
#   PaperRust-<version>-linux-<arch>.deb / .rpm   (next to the package directory)
#
# Requires: dpkg-deb (deb), rpmbuild (rpm; Debian/Ubuntu: apt install rpm).
#
# Examples:
#   apps/desktop/pack/pack.sh --tar && apps/desktop/pack/linux-pkg.sh
#   apps/desktop/pack/linux-pkg.sh --dir results/desktop/PaperRust-0.1.1-linux-x86_64 --only deb
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$REPO/Cargo.toml" | head -1)"
ARCH="$(uname -m)"
DIR=""
ONLY="deb,rpm"
while (($#)); do
	case "$1" in
	--dir) DIR="$2"; shift 2 ;;
	--arch) ARCH="$2"; shift 2 ;;
	--only) ONLY="$2"; shift 2 ;;
	*) echo "unknown: $1 (--dir, --arch, --only deb,rpm)" >&2; exit 2 ;;
	esac
done
case "$ARCH" in
x86_64 | amd64) ARCH=x86_64; DEB_ARCH=amd64 ;;
aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
*) echo "unsupported architecture: $ARCH" >&2; exit 2 ;;
esac
DIR="${DIR:-$REPO/results/desktop/PaperRust-$VERSION-linux-$ARCH}"
[[ -x "$DIR/PaperRust.sh" && -x "$DIR/paperrust-desktop" ]] || { echo "$DIR is not a pack.sh directory (PaperRust.sh/paperrust-desktop missing)" >&2; exit 1; }
OUT_BASE="$(dirname "$DIR")/PaperRust-$VERSION-linux-$ARCH"
ICON="$REPO/apps/desktop-shell/icons/icon.png"
SUMMARY="Document archive (paperless-ngx compatible), local without a server"
DESCRIPTION="PaperRust keeps your documents: scan or drop files, it reads the text (OCR), assigns
correspondents, document types and tags and makes everything searchable. Compatible with the
paperless-ngx database, files and REST API; one program plus a bundled PostgreSQL."

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# ---------------------------------------------------------------- common file tree
ROOT="$WORK/root"
mkdir -p "$ROOT/opt" "$ROOT/usr/bin" "$ROOT/usr/share/applications" "$ROOT/usr/share/icons/hicolor/512x512/apps" \
	"$ROOT/usr/share/doc/paperrust"
cp -a "$DIR" "$ROOT/opt/paperrust"
ln -s /opt/paperrust/PaperRust.sh "$ROOT/usr/bin/paperrust"
install -m 0644 "$ICON" "$ROOT/usr/share/icons/hicolor/512x512/apps/paperrust.png"
install -m 0644 "$DIR/LICENSE" "$ROOT/usr/share/doc/paperrust/copyright"
cat >"$ROOT/usr/share/applications/paperrust.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=PaperRust
GenericName=Document archive
GenericName[de]=Dokumentenarchiv
Comment=$SUMMARY
Comment[de]=Dokumentenarchiv (paperless-ngx-kompatibel), lokal ohne Server
Exec=/usr/bin/paperrust
Icon=paperrust
Terminal=false
Categories=Office;Utility;
Keywords=documents;archive;scan;ocr;paperless;
StartupWMClass=paperrust-app
DESKTOP
SIZE_KB="$(du -sk "$ROOT" | cut -f1)"

# ---------------------------------------------------------------- .deb
if [[ ",$ONLY," == *",deb,"* ]]; then
	D="$WORK/deb"
	cp -a "$ROOT" "$D"
	mkdir -p "$D/DEBIAN"
	{
		echo "Package: paperrust"
		echo "Version: $VERSION"
		echo "Architecture: $DEB_ARCH"
		echo "Maintainer: PaperRust <contact@paperrust.org>"
		echo "Installed-Size: $SIZE_KB"
		echo "Depends: libc6 (>= 2.36)"
		echo "Recommends: libwebkit2gtk-4.1-0, libgtk-3-0, libayatana-appindicator3-1"
		echo "Section: utils"
		echo "Priority: optional"
		echo "Homepage: https://paperrust.org"
		echo "Description: $SUMMARY"
		sed 's/^/ /' <<<"$DESCRIPTION"
	} >"$D/DEBIAN/control"
	dpkg-deb --root-owner-group -Zxz --build "$D" "$OUT_BASE.deb" >/dev/null
	echo "deb: $OUT_BASE.deb ($(du -h "$OUT_BASE.deb" | cut -f1))"
fi

# ---------------------------------------------------------------- .rpm
if [[ ",$ONLY," == *",rpm,"* ]]; then
	R="$WORK/rpm"
	mkdir -p "$R"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
	RPM_VERSION="${VERSION//-/_}"
	cat >"$R/SPECS/paperrust.spec" <<SPEC
Name:           paperrust
Version:        $RPM_VERSION
Release:        1
Summary:        $SUMMARY
License:        GPL-3.0-only
URL:            https://paperrust.org
# Bundled programs and libraries (PostgreSQL, Tesseract, PDFium) must not become package dependencies.
AutoReqProv:    no
Recommends:     webkit2gtk4.1
Recommends:     gtk3

%description
$DESCRIPTION

%prep
%build
%install
mkdir -p %{buildroot}
cp -a $ROOT/. %{buildroot}/

%files
/opt/paperrust
/usr/bin/paperrust
/usr/share/applications/paperrust.desktop
/usr/share/icons/hicolor/512x512/apps/paperrust.png
%doc /usr/share/doc/paperrust/copyright
SPEC
	# No stripping/debug packages/build-id links: the binaries are release builds already, and
	# rewriting the bundled PostgreSQL/Tesseract files would change what pack.sh checked.
	rpmbuild --quiet -bb --target "$ARCH" \
		--define "_topdir $R" \
		--define "__os_install_post %{nil}" \
		--define "debug_package %{nil}" \
		--define "_build_id_links none" \
		--define "_binary_payload w6.xzdio" \
		"$R/SPECS/paperrust.spec"
	cp "$(find "$R/RPMS" -name '*.rpm' | head -1)" "$OUT_BASE.rpm"
	echo "rpm: $OUT_BASE.rpm ($(du -h "$OUT_BASE.rpm" | cut -f1))"
fi
