#!/usr/bin/env bash
# PaperRust Desktop for Linux: build the programs and the package inside a Debian 12 container.
#
# Why a container: binaries built on a new distro need its new glibc and would not start on older
# systems. Debian 12 (glibc 2.36) is the oldest supported base; programs and the bundled Tesseract
# libraries built there run on Debian 12, Ubuntu 24.04, Fedora 37 and newer.
#
#   apps/desktop/pack/linux-build.sh [--arch x86_64|aarch64] [--out DIR] [--jobs N]
#
# Needs: docker (the host's own architecture; cross builds are not supported). Uses the docker volumes
# paperrust-linux-cargo (registry/toolchain cache) and paperrust-linux-target (build cache); remove them with
# `docker volume rm paperrust-linux-cargo paperrust-linux-target` when no longer needed.
# Result: <out>/PaperRust-<version>-linux-<arch>.tar.gz (+ unpacked folder).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
ARCH="$(uname -m)"
OUT="$REPO/results/desktop/linux"
JOBS=2
IMAGE="debian:12"
while [[ $# -gt 0 ]]; do
	case "$1" in
	--arch) ARCH="$2"; shift 2 ;;
	--out) OUT="$2"; shift 2 ;;
	--jobs) JOBS="$2"; shift 2 ;;
	--inner) shift; break ;;
	-h | --help) sed -n '2,14p' "$0"; exit 0 ;;
	*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

if [[ "${PAPERRUST_LINUX_BUILD_INNER:-}" != 1 ]]; then
	command -v docker >/dev/null || { echo "docker missing" >&2; exit 1; }
	[[ -d "$REPO/frontend-light/dist" ]] || { echo "frontend-light/dist missing (cd frontend-light && pnpm build)" >&2; exit 1; }
	[[ -d "$REPO/results/static" ]] || { echo "results/static missing (scripts/build-static.sh)" >&2; exit 1; }
	mkdir -p "$OUT"
	OUT="$(cd "$OUT" && pwd)" # docker -v needs an absolute path (a relative one is taken as a volume name)
	exec docker run --rm \
		-e PAPERRUST_LINUX_BUILD_INNER=1 -e CARGO_BUILD_JOBS="$JOBS" -e ARCH="$ARCH" \
		-e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
		-v "$REPO:/src" -v "$OUT:/out" \
		-v paperrust-linux-cargo:/root/.cargo-cache -v paperrust-linux-target:/target \
		-w /src "$IMAGE" bash /src/apps/desktop/pack/linux-build.sh
fi

# ---------------------------------------------------------------------------- inside the container
export DEBIAN_FRONTEND=noninteractive CARGO_HOME=/root/.cargo-cache/cargo RUSTUP_HOME=/root/.cargo-cache/rustup
export CARGO_TARGET_DIR=/target PATH="/root/.cargo-cache/cargo/bin:$PATH"
echo "== apt"
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
	build-essential pkg-config libssl-dev ca-certificates curl git python3 file patchelf xz-utils \
	libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev \
	tesseract-ocr tesseract-ocr-eng tesseract-ocr-deu tesseract-ocr-osd >/dev/null
tesseract --version 2>&1 | head -1
echo "== rust"
if ! command -v cargo >/dev/null; then
	curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path >/dev/null
fi
rustup toolchain install stable --profile minimal >/dev/null
rustc --version
ldd --version | head -1
echo "== build"
cd /src
cargo build -q --release -p pl-server --bin paperrust
cargo build -q --release -p paperrust-desktop
cargo build -q --release -p pl-dbmigrate --bin paperrust-migrate-db
cargo build -q --release -p pl-pdftools --features engine --bin paperrust-pdftool
(cd apps/desktop-shell && cargo build -q --release)
echo "== pack"
ver="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
name="PaperRust-$ver-linux-$ARCH"
rm -rf "/out/$name" "/out/$name.tar.gz"
bash apps/desktop/pack/pack.sh --target-dir /target --cache /root/.cargo-cache/pack --out "/out/$name" --tar
# glibc requirement of the result (highest GLIBC_x.y symbol version)
need="$(for f in "/out/$name"/paperrust* "/out/$name"/tesseract/tesseract "/out/$name"/tesseract/*.so*; do
	[[ -f "$f" && ! -L "$f" ]] && objdump -T "$f" 2>/dev/null | grep -oE 'GLIBC_[0-9.]+' ; done | sort -uV | tail -1)"
echo "glibc needed: $need"
chown -R "$HOST_UID:$HOST_GID" /out
ls -la /out
echo "== done"
