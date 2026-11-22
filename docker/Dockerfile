# syntax=docker/dockerfile:1
#
# PaperRust – Container-Image (Server ohne Django, Redis, Celery, Tika, Gotenberg).
#
# Bauen (aus der Repo-Wurzel, Submodul upstream/ muss ausgecheckt sein):
#
#   git submodule update --init
#   docker build -f docker/Dockerfile -t paperrust/paperrust:dev .
#
# Wichtige Build-Argumente (alle optional):
#   TESSERACT_LANGS        Tesseract-Sprachpakete (Debian-Namen, Standard wie upstream: eng deu fra ita spa)
#   CARGO_BUILD_JOBS       parallele rustc-Jobs (leer = alle Kerne; fat LTO braucht viel RAM, ggf. 2)
#   PAPERRUST_UID/GID      Benutzer im Image (Standard 1000:1000 wie upstream USERMAP_UID/GID)
#
# Ergebnis: /usr/local/bin/paperrust (+ paperrust-migrate-db), Oberfläche (frontend-light) unter
# /usr/src/paperless/light (/rs/app/, Standard unter /), Server-Statics unter /usr/src/paperless/static,
# Volumes data/media/consume/export unter /usr/src/paperless – dieselben
# Pfade wie im paperless-ngx-Image, damit Umsteiger in ihrer compose-Datei nur das Image tauschen.
# Ausführlich: docs/user/installation.md#docker.

ARG RUST_IMAGE=docker.io/library/rust:1.99-slim-trixie
ARG NODE_IMAGE=docker.io/library/node:24-trixie-slim
ARG RUNTIME_IMAGE=docker.io/library/debian:trixie-slim

# ------------------------------------------------------------------------------------------------
# PDFium (bblanchon/pdfium-binaries chromium/8111) – gleiche Version und Prüfsummen wie
# apps/desktop/pack/sources.json. ADD --checksum bricht bei falscher Prüfsumme ab.
# ------------------------------------------------------------------------------------------------
FROM scratch AS pdfium-amd64
ADD --checksum=sha256:bdc60126399f39fe768566a18d4be055a7b3681bc9e248e8a6656b4f6cae0aa2 \
    https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F8111/pdfium-linux-x64.tgz /pdfium.tgz

FROM scratch AS pdfium-arm64
ADD --checksum=sha256:042de247138f96341534cccf24ed21c2550a7245b936bb4a60d2340120000f05 \
    https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F8111/pdfium-linux-arm64.tgz /pdfium.tgz

# TARGETARCH setzt BuildKit automatisch im globalen Bereich (amd64 | arm64); andere Architekturen
# haben keine PDFium-Stufe und brechen hier ab.
FROM pdfium-${TARGETARCH} AS pdfium

# ------------------------------------------------------------------------------------------------
# Server-Statics (STATIC_ROOT, scripts/build-static.sh): Login-CSS, Favicon, Swagger-UI/DRF.
# Die Angular-Oberfläche ist seit 2026-11-13 abgelöst (docs/FRONTEND-PARITY.md).
# Architekturunabhängig, darum auf der Build-Plattform.
# ------------------------------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM ${NODE_IMAGE} AS static

SHELL ["/bin/bash", "-o", "pipefail", "-c"]
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update  && apt-get install --yes --no-install-recommends unzip ca-certificates  && rm -rf /var/lib/apt/lists/*

# Statische Dateien, die upstream per collectstatic aus Python-Paketen holt und die PaperRust ohne
# Django ausliefert (Swagger-UI unter /api/schema/view/, DRF). Versionen + SHA-256 aus
# upstream/uv.lock (v3.4.2), gleich wie scripts/fetch-extra-static.sh.
ADD --checksum=sha256:fe1d005d8ac9b5d0472f2fe5deeb2e7d6dcffaa24118044f22578b4b8c4b31f2     https://files.pythonhosted.org/packages/55/09/13509939a12e953759fa2f281b28dc14813c8c331addf3838fccdc71b364/drf_spectacular_sidecar-2026.10.1-py3-none-any.whl     /tmp/wheels/
ADD --checksum=sha256:8544bb674846731b1e3c9b309236ee1dc412905a0aa725be2ec193ca950a7d12     https://files.pythonhosted.org/packages/c4/dd/de5f4543765bec94e036a5e9593a42f8fa7efec8de78eb56b9ca834385f3/djangorestframework-3.18.3-py3-none-any.whl     /tmp/wheels/
RUN set -eux; \
    mkdir -p /tmp/extra-static; \
    for w in /tmp/wheels/*.whl; do \
      d="$(mktemp -d)"; unzip -q "$w" -d "$d"; \
      # wie Djangos AppDirectoriesFinder: <paket>/static/<name>/… -> STATIC_ROOT/<name>/…
      for s in "$d"/*/static/*; do cp -r "$s" /tmp/extra-static/; done; \
      rm -rf "$d"; \
    done; \
    ls /tmp/extra-static

WORKDIR /src
COPY upstream/src/documents/static upstream/src/documents/static
COPY scripts/build-static.sh scripts/compress-static.mjs scripts/
COPY scripts/static-assets scripts/static-assets
RUN set -eux;     EXTRA_STATIC=/tmp/extra-static bash scripts/build-static.sh /out/static;     test -f /out/static/base.css;     test -d /out/static/drf_spectacular_sidecar

# ------------------------------------------------------------------------------------------------
# Oberfläche (frontend-light, Svelte/Vite) für /rs/app/ – eigenständig (eigene Styles unter
# frontend-light/src/styles, Bootstrap aus npm), architekturunabhängig.
# ------------------------------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM ${NODE_IMAGE} AS frontend-light

SHELL ["/bin/bash", "-o", "pipefail", "-c"]
WORKDIR /src/frontend-light
COPY frontend-light/ ./
RUN --mount=type=cache,id=paperrust-pnpm-store,target=/pnpm-store \
    set -eux; \
    corepack enable; \
    export npm_config_store_dir=/pnpm-store; \
    pnpm install --frozen-lockfile; \
    pnpm build; \
    test -f dist/index.html

# ------------------------------------------------------------------------------------------------
# Rust: paperrust (Server + Worker), paperrust-migrate-db und paperrust-pdftool, Release (fat LTO, siehe Cargo.toml)
# ------------------------------------------------------------------------------------------------
FROM ${RUST_IMAGE} AS rust

WORKDIR /build
# Nur, was der Build braucht (Änderungen an Doku/Frontend bauen Rust nicht neu).
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY apps/desktop apps/desktop
COPY docs/design/assets docs/design/assets
# Benutzer-Doku: in paperrust eingebaut (pl-server docs.rs, /rs/docs/)
COPY docs/user docs/user
COPY upstream/src/documents/resources/document.webp upstream/src/documents/resources/document.webp
# rust-toolchain.toml bewusst nicht kopiert: das Image bringt seine Toolchain mit; sonst lädt rustup
# beim Build "stable" + rustfmt/clippy nach.

ARG CARGO_BUILD_JOBS=
# Leer = Cargo.toml [profile.release] (fat LTO, codegen-units 1). Für kleine Builder (wenig RAM,
# CI-Läufe ohne Veröffentlichung) z. B. CARGO_RELEASE_LTO=thin CARGO_RELEASE_CODEGEN_UNITS=16.
ARG CARGO_RELEASE_LTO=
ARG CARGO_RELEASE_CODEGEN_UNITS=
ARG TARGETARCH
RUN --mount=type=cache,id=paperrust-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=paperrust-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=paperrust-target-${TARGETARCH},target=/build/target \
    set -eux; \
    if [ -z "${CARGO_BUILD_JOBS}" ]; then unset CARGO_BUILD_JOBS; fi; \
    if [ -n "${CARGO_RELEASE_LTO}" ]; then export CARGO_PROFILE_RELEASE_LTO="${CARGO_RELEASE_LTO}"; fi; \
    if [ -n "${CARGO_RELEASE_CODEGEN_UNITS}" ]; then export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="${CARGO_RELEASE_CODEGEN_UNITS}"; fi; \
    rustc --version; \
    cargo build --release --locked ${CARGO_BUILD_JOBS:+-j ${CARGO_BUILD_JOBS}} \
      -p pl-server --bin paperrust \
      -p pl-dbmigrate --bin paperrust-migrate-db; \
    # PDF-Werkzeuge (Schwärzen/Verkleinern/Reparieren, PdfCraft): eigener Kindprozess neben paperrust
    cargo build --release --locked ${CARGO_BUILD_JOBS:+-j ${CARGO_BUILD_JOBS}} \
      -p pl-pdftools --features engine --bin paperrust-pdftool; \
    mkdir -p /out; \
    cp target/release/paperrust target/release/paperrust-migrate-db target/release/paperrust-pdftool /out/

# routes.toml: Repo-Datei unverändert (seit 2026-11-14 schaltet sie Routen nur noch ab; nicht
# genannte ids sind an). Prüfung: kein alter "django"-Wert mehr (würde nur eine Warnung erzeugen).
COPY routes.toml /out/routes.toml
RUN set -eu; \
    if grep -Eq '^"[^"]+"[[:space:]]*=[[:space:]]*"django"' /out/routes.toml; then \
      echo "routes.toml enthält noch django-Einträge" >&2; exit 1; \
    fi; \
    echo "routes.toml: $(grep -Ec '^"[^"]+"[[:space:]]*=' /out/routes.toml) Einträge"

# ------------------------------------------------------------------------------------------------
# Laufzeit
# ------------------------------------------------------------------------------------------------
FROM ${RUNTIME_IMAGE} AS runtime

SHELL ["/bin/bash", "-o", "pipefail", "-c"]
ARG DEBIAN_FRONTEND=noninteractive
# Wie das upstream-Image: Englisch, Deutsch, Französisch, Italienisch, Spanisch. Debian-Paketnamen
# (tesseract-ocr-<sprache>, z. B. chi-sim); Unterstriche werden zu Bindestrichen.
ARG TESSERACT_LANGS="eng deu fra ita spa"
# pg_dump für die Sicherung vor den Datenmodell-Umbaustufen (`manage migrate`, Testprotokoll
# 2026-11-16 D-073). pg_dump muss mindestens die Hauptversion des Servers haben; die Compose-Dateien
# nutzen postgres:17 (= Debian trixie). Für einen neueren Server: --build-arg PG_CLIENT_MAJOR=…
# (das Paket muss in den Debian-Quellen sein, sonst die PGDG-Quelle ergänzen).
ARG PG_CLIENT_MAJOR=17
ARG PAPERRUST_UID=1000
ARG PAPERRUST_GID=1000
ARG PAPERRUST_VERSION=0.1.2
ARG VCS_REF=

# ca-certificates: TLS für Mail/OIDC/LLM/Webhooks (rustls liest die System-Zertifikate)
# tini: PID 1, gibt SIGTERM weiter, räumt Zombies ab (tesseract-Kindprozesse)
# fonts-liberation: Schrift für Text-Vorschaubilder (PAPERLESS_THUMBNAIL_FONT_NAME wie upstream)
# postgresql-client-N: pg_dump (Sicherung vor den Datenmodell-Umbaustufen, siehe PG_CLIENT_MAJOR)
# tzdata: wie upstream (PAPERLESS_TIME_ZONE wird intern über chrono-tz aufgelöst)
RUN set -eux; \
    langs=""; \
    for l in ${TESSERACT_LANGS}; do l="${l,,}"; langs="$langs tesseract-ocr-${l//_/-}"; done; \
    apt-get update; \
    apt-get install --yes --no-install-recommends \
      ca-certificates tini tzdata fonts-liberation tesseract-ocr $langs \
      postgresql-client-${PG_CLIENT_MAJOR}; \
    # setpriv (Rechte abgeben bei Start als root/USERMAP_UID): util-linux, in neueren Debian-Versionen
    # ggf. eigenes Paket
    command -v setpriv >/dev/null || apt-get install --yes --no-install-recommends setpriv; \
    command -v setpriv; \
    rm -rf /var/lib/apt/lists/*; \
    tesseract --version; \
    tesseract --list-langs; \
    pg_dump --version

# upstream-Standard PAPERLESS_THUMBNAIL_FONT_NAME=/usr/share/fonts/liberation/LiberationSerif-Regular.ttf;
# Debian legt die Schrift unter truetype/liberation ab -> Verweis, damit der Standard ohne Variable passt.
RUN set -eux; \
    f="$(find /usr/share/fonts -name LiberationSerif-Regular.ttf -print -quit)"; \
    test -n "$f"; \
    if [[ ! -e /usr/share/fonts/liberation/LiberationSerif-Regular.ttf ]]; then \
      ln -s "$(dirname "$f")" /usr/share/fonts/liberation; \
    fi; \
    test -r /usr/share/fonts/liberation/LiberationSerif-Regular.ttf

RUN --mount=type=bind,from=pdfium,target=/mnt/pdfium \
    set -eux; \
    tmp="$(mktemp -d)"; \
    tar -xzf /mnt/pdfium/pdfium.tgz -C "$tmp"; \
    mkdir -p /opt/pdfium/lib /usr/share/licenses/paperrust/pdfium; \
    cp "$tmp/lib/libpdfium.so" /opt/pdfium/lib/; \
    cp -r "$tmp"/LICENSE* /usr/share/licenses/paperrust/pdfium/; \
    rm -rf "$tmp"; \
    ldd /opt/pdfium/lib/libpdfium.so >/tmp/ldd.txt; \
    if grep 'not found' /tmp/ldd.txt; then exit 1; fi; \
    rm -f /tmp/ldd.txt

RUN set -eux; \
    groupadd --gid "${PAPERRUST_GID}" paperrust; \
    useradd --uid "${PAPERRUST_UID}" --gid "${PAPERRUST_GID}" --no-create-home \
      --home-dir /usr/src/paperless --shell /usr/sbin/nologin paperrust; \
    mkdir -p /usr/src/paperless/src /usr/src/paperless/data /usr/src/paperless/media \
             /usr/src/paperless/consume /usr/src/paperless/export /usr/src/paperless/static; \
    chown "${PAPERRUST_UID}:${PAPERRUST_GID}" /usr/src/paperless/data /usr/src/paperless/media \
      /usr/src/paperless/consume /usr/src/paperless/export

# paperrust-pdftool muss neben paperrust liegen (pl-pdftools runner::tool_path)
COPY --from=rust /out/paperrust /out/paperrust-migrate-db /out/paperrust-pdftool /usr/local/bin/
COPY --from=rust /out/routes.toml /usr/src/paperless/routes.toml
COPY --from=static /out/static/ /usr/src/paperless/static/
# Oberfläche: <STATICDIR>/../light (pl-server light_ui::configured_dir) → /rs/app/ (und / leitet dorthin)
COPY --from=frontend-light /src/frontend-light/dist/ /usr/src/paperless/light/
COPY docker/rootfs/ /
COPY LICENSE NOTICE /usr/share/licenses/paperrust/
COPY upstream/LICENSE /usr/share/licenses/paperrust/paperless-ngx/LICENSE

# Kommandos wie im upstream-Image (`docker compose exec webserver document_exporter ../export`):
# jedes ruft `paperrust manage <befehl>` auf (Liste: pl_cli::COMMANDS).
RUN set -eux; \
    chmod 0755 /usr/local/bin/paperrust-entrypoint /usr/local/bin/paperrust-healthcheck \
               /usr/local/bin/paperrust-manage-wrapper; \
    chmod 0644 /usr/local/lib/paperrust/common.sh; \
    for c in migrate showmigrations createsuperuser manage_superuser changepassword \
             document_exporter document_importer document_sanity_checker document_retagger \
             document_index document_create_classifier document_thumbnails document_renamer \
             document_archiver document_fuzzy_match document_llmindex document_consumer check \
             prune_audit_logs empty_trash mail_fetcher; do \
      ln -s paperrust-manage-wrapper "/usr/local/bin/$c"; \
    done; \
    # alle Bibliotheken auflösbar (Build- und Laufzeitstufe beide Debian trixie)
    for b in /usr/local/bin/paperrust /usr/local/bin/paperrust-migrate-db; do \
      ldd "$b" >/tmp/ldd.txt; \
      if grep 'not found' /tmp/ldd.txt; then exit 1; fi; \
    done; \
    rm -f /tmp/ldd.txt

# PNGX_CONTAINERIZED=1 wie im upstream-Image: /api/status/ meldet install_type "docker".
ENV PAPERRUST_BASE_DIR=/usr/src/paperless \
    PAPERRUST_ROUTES=/usr/src/paperless/routes.toml \
    PAPERRUST_PDFIUM_PATH=/opt/pdfium/lib/libpdfium.so \
    NO_COLOR=1 \
    PNGX_CONTAINERIZED=1

LABEL org.opencontainers.image.title="PaperRust" \
      org.opencontainers.image.description="PaperRust – schneller, schlanker, kompatibler Ersatz für paperless-ngx (v3.4.2)" \
      org.opencontainers.image.source="https://github.com/paperrust/paperrust" \
      org.opencontainers.image.licenses="GPL-3.0-only" \
      org.opencontainers.image.version="${PAPERRUST_VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}"

WORKDIR /usr/src/paperless/src
USER ${PAPERRUST_UID}:${PAPERRUST_GID}

VOLUME ["/usr/src/paperless/data", \
        "/usr/src/paperless/media", \
        "/usr/src/paperless/consume", \
        "/usr/src/paperless/export"]

EXPOSE 8000

# /__rs/health antwortet nur lokalen Aufrufern ohne Proxy-Header – passt für den Check im Container.
HEALTHCHECK --interval=30s --timeout=10s --start-period=120s --retries=5 CMD ["/usr/local/bin/paperrust-healthcheck"]

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/paperrust-entrypoint"]
CMD ["serve", "--worker"]
