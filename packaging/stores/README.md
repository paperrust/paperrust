# PaperRust in app stores – submission packages

Prepared 2026-11-22 for PaperRust 0.1.2 (image `ghcr.io/paperrust/paperrust`, multi-arch amd64 + arm64). Nothing here
has been submitted. Every store needs an account and a pull request or form submission by the owner; the files
below are ready to copy.

| Store | Folder | Ready | Still missing before submitting |
|---|---|---|---|
| [Unraid Community Applications](#1-unraid-community-applications) | `unraid/` (= content of a new repo `paperrust/unraid-templates`) | templates, `ca_profile.xml`, icon, LICENSE | public repo, CA account, test on an Unraid server, optional forum support thread |
| [CasaOS / ZimaOS AppStore](#2-casaos--zimaos-appstore) | `casaos/Apps/PaperRust/` | compose with `x-casaos`, `icon.svg`, `icon.png` (256), `thumbnail.png` (784×442) | **3 screenshots** (`screenshot-1..3.png`, 1280×720), test on CasaOS/ZimaOS |
| [Umbrel App Store](#3-umbrel-app-store) | `umbrel/paperrust/` | manifest, compose, `exports.sh`, `data/` scaffolding, `pin-digests.sh` | **image digests** (run `pin-digests.sh` after the release), screenshots + logo for the PR body, test on umbrelOS (amd64 + arm64) |
| [Proxmox VE community-scripts](#4-proxmox-ve-community-scripts) | `proxmox/` | `ct/paperrust.sh`, `install/paperrust-install.sh`, `json/paperrust.json` | **eligibility (1,000+ stars, 6+ months old)**, test on a real Proxmox host, arm64 release tarball (optional) |
| [Docker Hub](#5-docker-hub) | `dockerhub/` | full + short description | Docker Hub org `paperrust`, access token, workflow change (described below, not applied) |

Common facts used everywhere (checked in this repository, not guessed):

- Image entrypoint `tini → paperrust-entrypoint → paperrust serve --worker`; port 8000; volumes
  `/usr/src/paperless/{data,media,consume,export}`; built-in health check; default user UID/GID 1000; with
  `user: "0:0"` it applies `USERMAP_UID/GID` or `PUID/PGID` and drops privileges (`docker/rootfs/usr/local/lib/paperrust/common.sh`).
- Required: `PAPERRUST_SECRET_KEY` (rejected when empty or `change-me`, `crates/pl-core/src/config.rs`; `*_FILE` variants
  work), `PAPERRUST_DBHOST`/`DBNAME`/`DBUSER`/`DBPASS`, PostgreSQL ≥ 14. First admin via `PAPERRUST_ADMIN_USER`/`PASSWORD`
  (`manage manage_superuser`, only creates a missing user). There is **no first-run setup page**, so every package
  must create the admin user.
- All `PAPERLESS_*` names work as aliases.

---

## 1. Unraid Community Applications

**Where:** submission portal <https://ca.unraid.net/submit/new> (new portal since June 2026, replaces the old
pull-request/forum process; it validates `ca_profile.xml` and checks for duplicate listings). Starter repo used as
model: <https://github.com/unraid/unraid-community-apps-starter>.

**Steps**

1. Create a **public** GitHub repository `paperrust/unraid-templates` (any name works, but the URLs in the XML files
   assume this one – search/replace `paperrust/unraid-templates` if you choose another).
2. Copy the content of `packaging/stores/unraid/` into the repository root:
   `ca_profile.xml`, `icon.png`, `LICENSE` (GPL-3.0, OSI-approved as required), `README.md`, `.gitignore`,
   `templates/paperrust.xml`, `templates/paperrust-postgres.xml`. Push to `main`.
3. Check that `https://raw.githubusercontent.com/paperrust/unraid-templates/main/templates/paperrust.xml` and the
   icon URL in it (`…/paperrust/paperrust/main/docs/design/assets/png/icon-512.png`) open in a browser.
4. Sign in at <https://ca.unraid.net/submit/new> (Unraid account), enter the repository URL, run **Validate** and
   **Scan**, fix what it reports, confirm. Existing submissions are listed under `/submissions`.

**What reviewers require** (starter README + portal help): public, active repo; OSI license at the root;
`ca_profile.xml` in the root with a non-empty `<Profile>`; one XML per app under `templates/`; every Docker template
needs `<Repository>`, a readable `<Name>` and `<Overview>`, `<Support>` or `<Project>`, and `<TemplateURL>` pointing at
the raw URL of that exact file. Run Validate + Scan after every XML change.

**Decisions in the templates**

- Image `ghcr.io/paperrust/paperrust:latest` (Unraid's update check compares the digest of the tag; the starter uses
  `:latest`). Users who want to pin can edit the tag.
- PostgreSQL ships as a separate template `paperrust-postgres` (`postgres:17`). If the portal's duplicate check flags it
  (there are generic PostgreSQL templates in CA already), remove it and keep the `<Requires>` text pointing to any
  PostgreSQL 14+ container – the PaperRust template works with either.
- Category `Productivity:`; paths `/mnt/user/appdata/paperrust/{data,media,consume,export,pgdata}`; PUID 99 / PGID 100
  (Unraid defaults) with `--user 0:0` so the image can apply them; consumer polling 10 s because user shares do not
  deliver inotify events reliably.
- Support link is GitHub issues. CA moderators like an Unraid forum support thread (Docker Containers sub-forum);
  if you open one, add `<Support>https://forums.unraid.net/topic/…</Support>` and `<Forum>` in `ca_profile.xml`.

**Test before submitting** (on a real Unraid 6.12+/7.x server): copy both XMLs to
`/boot/config/plugins/dockerMan/templates-user/my-paperrust*.xml`, Docker → Add Container → template; start
postgres, then PaperRust with *DB host* = server IP; first start reaches the login page, admin login works; files in
`consume` are imported (polling); `docker exec paperrust document_exporter ../export` writes files owned by 99:100;
container update (Check for updates) and restart keep the data; also test the custom-network variant
(*DB host* `paperrust-postgres`).

**Updates:** nothing to do per release – the template follows `:latest`, Unraid shows "update ready" when the tag's
digest changes. Edit the XML only when settings change (and the `<Changes>` text if you want release notes in the
Apps tab); CA re-reads submitted repositories by itself, a new submission is not needed. Optional automation: a
workflow in `paperrust/unraid-templates` (or a step in `release.yml` with a token for that repo) that rewrites
`<Changes>`/`<Date>` from the GitHub release and pushes – no PR to Unraid involved.

---

## 2. CasaOS / ZimaOS AppStore

**Where:** <https://github.com/IceWhaleTech/CasaOS-AppStore> (now "ZimaOS AppStore Source", protocol v2; still
builds the legacy v1 zip for older CasaOS). New apps and updates are pull requests.

**Steps**

1. Fork `IceWhaleTech/CasaOS-AppStore`, branch e.g. `add-paperrust`.
2. Copy `packaging/stores/casaos/Apps/PaperRust/` to `Apps/PaperRust/` in the fork.
3. Add screenshots `screenshot-1.png`, `screenshot-2.png`, `screenshot-3.png` (English UI with demo data, ~1280×720 –
   existing apps use 1280×720; the compose file already references these three names). Suggested: document list,
   document detail with OCR text, search/saved views.
4. Run their local build check: `./scripts/build_dist.sh` (needs Docker; must finish without errors and produce
   `dist/apps/org.paperrust.paperrust/`) and `docker compose -f Apps/PaperRust/docker-compose.yml config -q`.
5. Open the PR against `main`. CONTRIBUTING asks the PR to say: new app, what changed, how it was validated
   (tested-on device/OS version and architecture).

**What reviewers/CI require** (`docs/specs/compose-and-x-casaos.md`, `.github/actions/validate-compose`): top-level
`name` matching `^[a-z0-9_-]+$`; top-level `x-casaos` with `id` in reverse-domain form (`org.paperrust.paperrust`),
`main` (= `paperrust`, not the DB), `index`, `port_map` as a **quoted string**, `icon`, `title` (locale object,
`en_US` required), `category` from the fixed list (`Productivity`), `version` (semver); recommended `tagline`,
`description`, `author`, `developer`, `architectures`, `website`/`repo`/`support`/`docs`. Locale keys `ll_CC`
(`en_US`, `de_DE`). Compose must pass `docker compose config -q`. Assets: `icon.svg` preferred (+ PNG), optional
`thumbnail` and `screenshot-N`. Service-level `x-casaos` blocks are legacy (dropped in v2 output, still used by v1
clients) – kept for env/volume descriptions.

**Decisions**

- Image pinned to `ghcr.io/paperrust/paperrust:0.1.2` (store apps pin versions; `version` must match). Submit after the
  0.1.2 image is published, or change both to `0.1.1`.
- CasaOS has no secret generator. The compose file overrides the entrypoint with a short `bash -c` that creates a
  random `data/.paperrust_secret_key` on first start (owned by PUID/PGID, no trailing newline) and passes it via
  `PAPERRUST_SECRET_KEY_FILE`, then `exec`s the normal entrypoint. Simulated locally (64 chars, created once).
  A cleaner long-term fix is in the report (image generates the key itself).
- Default login `admin` / `casaos` (CasaOS convention, shown in `tips.before_install`); DB password `paperrust` (DB has
  no published port). The admin password is only used when the user is created, so changing it in the UI sticks.
- Bind mounts under `/DATA/AppData/$AppID/…`; `$PUID`, `$PGID`, `$TZ` are filled in by CasaOS.

**Test before submitting** (CasaOS 0.4.x and ZimaOS, ideally amd64 + arm64, e.g. Raspberry Pi 4/5): App Store →
custom install → import the compose file; install without edits → app opens on port 8000, `admin`/`casaos` works; the
key file exists; upload a PDF and a scan, OCR text appears; restart the app and the device, data stays; "update"
with a newer tag keeps data; uninstall/reinstall with existing `/DATA/AppData/paperrust` reuses data and key.

**Updates:** one PR per release that changes in `Apps/PaperRust/docker-compose.yml`: `image:` tag, `x-casaos.version`,
`update_at`, `release_notes`. Can be automated from `release.yml` (on a published, non-prerelease tag): check out the
fork with a fine-grained PAT (`CASAOS_FORK_TOKEN`, contents + pull-requests on the fork), `sed` the four fields,
push a branch, `gh pr create --repo IceWhaleTech/CasaOS-AppStore`. Keep it opt-in (skip when the secret is missing);
the PR still needs a human review at IceWhale.

---

## 3. Umbrel App Store

**Where:** <https://github.com/getumbrel/umbrel-apps> (pull requests; title `Add PaperRust`). Rules come from the repo's
`AGENTS.md` and `.agents/skills/umbrel-package-app|umbrel-test-app|umbrel-update-app/SKILL.md`, and the PR template.

**Steps**

1. After the release image exists: `bash packaging/stores/umbrel/pin-digests.sh 0.1.2` (needs `docker buildx` + `jq`
   or `crane`; reads only). It replaces the `@sha256:000…` placeholders with the **multi-arch index digests** of
   `ghcr.io/paperrust/paperrust:0.1.2` and `postgres:17`, refuses images without `linux/amd64` + `linux/arm64`, and sets
   `version`. Review the diff.
2. Fork `getumbrel/umbrel-apps`, branch `add-paperrust`, copy `packaging/stores/umbrel/paperrust/` to `paperrust/`
   (including the `data/*/.gitkeep` files). Do **not** commit icons or gallery images.
3. In the fork: `npm ci && npm run lint:apps -- paperrust --check-images` and `git diff --check`; fix errors, explain
   remaining warnings in the PR.
4. Open the PR with their template: type *New app*, app id `paperrust`, upstream URL, version, test environment and
   architectures, default credentials (admin + Umbrel-generated password), and **screenshots plus a logo reference**
   in the PR body (Umbrel creates the final gallery assets; logo:
   `docs/design/assets/paperrust-app-icon.svg` / `png/icon-512.png`). Then set `submission:` in `umbrel-app.yml` to the
   PR URL (currently `…/pull/TODO`) and push.

**What reviewers require** (packaging skill): opens to a useful web UI; maintained images for amd64 **and** arm64;
every image pinned as `repo:tag@sha256:<index digest>` (no `latest`, no digest-only); all state under
`${APP_DATA_DIR}/data/…` with committed `.gitkeep`s; `app_proxy` service with `APP_HOST=<app-id>_<service>_1` and
`APP_PORT`; no raw web port; unique manifest `port`; `storage: dataRoot: data`; `gallery: []`, no `icon`;
`releaseNotes: ""` for new apps; generated secrets via `APP_SEED`, `APP_PASSWORD`, `derive_entropy`; no Docker
socket, no privileged mode; `restart: on-failure`.

**Decisions**

- App id `paperrust`, category `files` (as paperless), manifest port **2350** (checked free against every app manifest
  `port` and every published compose port in a clone from 2026-11-22; paperless uses 2349). Re-check before the PR –
  the linter does it too.
- Secrets: `PAPERRUST_SECRET_KEY=${APP_SEED}`, admin password `${APP_PASSWORD}` with `deterministicPassword: true`,
  DB password `derive_entropy "…-db-password"` in `exports.sh`.
- `user: "0:0"` + `PUID/PGID=1000` (the image's supported root→drop path; needed for extra OCR languages; files end up
  owned by 1000 like Umbrel's app data).
- Umbrel login stays on (guidance), with `PROXY_AUTH_WHITELIST: "/api/*"` so paperless-ngx mobile apps and scripts work
  with their token (paperless itself turns proxy auth off completely – reviewers may prefer either; the API has its own auth).
- `folderAccess` for consume/export and `environment` entries for OCR language(s) and polling, modelled on the
  current paperless package; `manifestVersion: 1.1` as paperless.

**Test before submitting** (`umbrel-test-app` skill): install through umbrelOS (device or the `ghcr.io/getumbrel/umbrelos`
container on a Linux host – not WSL), on amd64 and arm64 (Raspberry Pi 5); app opens via the Umbrel dashboard;
login `admin` + shown password; upload and consume a document; **POST requests/login work through app_proxy**
(CSRF/Origin check – not verified yet); restart and update keep data; folder settings (consume on a network share
with polling) and environment settings appear in app settings; `/api/` reachable with a token while Umbrel auth is on.

**Updates:** PR per release `Update PaperRust to x.y.z`: bump `version`, write `releaseNotes` for upgraders, update the
image tag **and** digest (`pin-digests.sh x.y.z`). Automatable like CasaOS (release workflow → script → push to fork →
`gh pr create --repo getumbrel/umbrel-apps`), but Umbrel expects runtime testing notes in every PR, so a draft PR that
a human completes is the realistic level.

---

## 4. Proxmox VE community-scripts

**Where:** new scripts go to <https://github.com/community-scripts/DevScripts> (formerly ProxmoxVED), **not** to
ProxmoxVE – PRs with new scripts against ProxmoxVE are closed without review. After testing there, maintainers move
the script to <https://github.com/community-scripts/ProxmoxVE>. The engine (`build.func`, `tools.func`) lives in
<https://github.com/community-scripts/core>. Metadata is the JSON file in DevScripts `json/`; later changes to
name/description/logo go through the website ("Report Issue"), not repo files.

**Blocker – eligibility** (DevScripts PR template and `AGENTS.md`): the application must be **at least 6 months old,
actively maintained, have 1,000+ stars** (GitHub/GitLab/Codeberg) and publish official release tarballs. PaperRust is
younger than 6 months; check the star count. Until both are met, do not submit (they close such PRs). Users can still
run the scripts locally (see test steps).

**Steps (when eligible)**

1. Fork `community-scripts/DevScripts`, branch `feat/paperrust`.
2. Copy `proxmox/ct/paperrust.sh` → `ct/paperrust.sh`, `proxmox/install/paperrust-install.sh` →
   `install/paperrust-install.sh`, `proxmox/json/paperrust.json` → `json/paperrust.json`. Set the `Author:` line to
   the submitting GitHub account; update `date_created`.
3. Open the PR with their template (two to five sentences; name the source: "Install from the PaperRust release
   tarball, systemd unit from docs/user/installation.md#standalone"; state arm64 not supported; state the AI model used –
   the template requires that).
4. Answer the testing issue/Discord thread they open.

**What reviewers require** (strict, `AGENTS.md`): only `tools.func` helpers (`fetch_and_deploy_gh_release`,
`check_for_gh_release`, `setup_postgresql`, `setup_postgresql_db`, `random_password`); bare metal, no Docker; no own
functions besides `update_script()`; `$STD` before commands; `apt` not `apt-get`; no core packages as dependencies;
no comments besides the header; no service user (runs as root); every `msg_info` closed by one `msg_ok`; standard
footer `motd_ssh`/`customize`/`cleanup_lxc`; `var_arm64` decided with a reason; JSON `architectures` consistent with it;
at most 3 short notes.

**How the scripts work** (verified against this repo, not guessed):

- `fetch_and_deploy_gh_release "paperrust" "paperrust/paperrust" "prebuild" "latest" "/opt/paperrust" "PaperRust-*-linux-x86_64.tar.gz"`
  – the release tarball from `release.yml` (`PaperRust-<ver>-linux-x86_64.tar.gz`); the helper strips its single top
  directory. Used from it: `paperrust`, `paperrust-pdftool` (found next to `paperrust`), `static/`, `light/`
  (found as `STATICDIR/../light`), `pdfium/libpdfium.so`, `routes.toml`. The bundled PostgreSQL/Tesseract of the desktop
  package stay unused (about 100 MB extra on disk). The `.deb` was not used: it is the desktop package (installs a
  launcher as `/usr/bin/paperrust`, recommends WebKitGTK).
- PostgreSQL 17 via `setup_postgresql` (PGDG; also provides `pg_dump`, which `manage migrate` uses for its backup before
  schema changes); Tesseract 5 + `eng`/`deu` and `fonts-liberation` from Debian 13.
- Config `/etc/paperrust.conf` (read automatically by PaperRust, `crates/pl-core/src/config.rs`, and used as
  `EnvironmentFile`), data in `/opt/paperrust_data/` (outside the deploy dir, so `CLEAN_INSTALL=1` updates keep it).
- Setup: `paperrust manage migrate`, `manage manage_superuser` (admin password via `random_password`, appended to
  `~/paperrust.creds`, the file `setup_postgresql_db` already writes), `manage document_index reindex`.
- Service `paperrust.service`: `paperrust serve --worker`, `PAPERRUST_BIND=0.0.0.0:8000`, `RestartForceExitStatus=75`
  + `PAPERRUST_RESTART_SUPERVISED=1` (restart from the UI) as in `docs/user/installation.md#standalone`.
- Update: `check_for_gh_release` → stop → clean deploy → `paperrust manage migrate` (serve itself never migrates) → start.
- `var_arm64="no"`: releases contain no linux-aarch64 tarball. If one is added later (pack.sh supports `--arch aarch64`),
  use `"PaperRust-*-linux-$(arch_resolve "x86_64" "aarch64").tar.gz"`, set `var_arm64` after an arm64 test and
  `architectures: ["amd64","arm64"]`.

**Test before submitting – required, on a real Proxmox VE 8/9 host** (cannot be done on this laptop):
push the branch to your DevScripts fork, then on the Proxmox host run

```sh
B=https://raw.githubusercontent.com/<you>/DevScripts/feat/paperrust
COMMUNITY_SCRIPTS_URL="$B" dev_mode="trace,keep" bash -c "$(curl -fsSL "$B/ct/paperrust.sh")"
```

(`COMMUNITY_SCRIPTS_URL` makes the engine fetch `install/paperrust-install.sh` from the fork instead of
DevScripts `main`, `core/core/build.func`). Check: container builds unprivileged on
Debian 13; `systemctl status paperrust` active; `http://<ip>:8000` shows the login; admin from `~/paperrust.creds`
works; OCR of a scan (Tesseract found at `/usr/bin/tesseract`), thumbnails (PDFium), redaction/shrink (pdftool);
reboot the CT; run the update path (`update` inside the CT, or install 0.1.1 first and update to 0.1.2); the
unattended path (`var_*` env, no prompts – the script has none). Their maintainers are strict and review line by line;
expect change requests.

**Updates:** none per release – users run `update` in the container, `check_for_gh_release` picks up new GitHub
releases. Only script changes (new paths, new dependencies, arm64) need a PR to ProxmoxVE once it lives there.

---

## 5. Docker Hub

**Files:** `dockerhub/README.md` (full description, English), `dockerhub/short-description.txt` (97 characters; limit
is 100).

**Accounts:** create the Docker Hub organisation `paperrust` and repository `paperrust/paperrust` (public), and an
access token (Account settings → Personal access tokens, scope *Read & Write*; *Read, Write, Delete* if the
description is synced by the workflow, see below). Store as repository secrets `DOCKERHUB_USERNAME` and
`DOCKERHUB_TOKEN`.

**Workflow change (not applied – `.github/workflows/release-image.yml` is outside this task):** keep building and pushing
by digest to `ghcr.io` exactly as now (`vars.PAPERRUST_IMAGE=ghcr.io/paperrust/paperrust`), and copy the finished
multi-arch manifest to Docker Hub in the `manifest` job. `imagetools create` copies the index with both platforms and
the SBOM/provenance attestations between registries, no rebuild. Add to the `manifest` job:

```yaml
  manifest:
    # ...existing keys...
    env:
      # secrets are not allowed in step `if:`; expose a flag instead. Empty secrets -> 'false' -> steps skipped.
      DOCKERHUB_ENABLED: ${{ secrets.DOCKERHUB_USERNAME != '' && secrets.DOCKERHUB_TOKEN != '' }}
      DOCKERHUB_IMAGE: docker.io/paperrust/paperrust
    steps:
      # ...existing steps up to and including "Verify"...
      - name: Log in to Docker Hub
        if: env.DOCKERHUB_ENABLED == 'true'
        uses: docker/login-action@v4
        with:
          registry: docker.io
          username: ${{ secrets.DOCKERHUB_USERNAME }}
          password: ${{ secrets.DOCKERHUB_TOKEN }}
      - name: Copy manifest to Docker Hub
        if: env.DOCKERHUB_ENABLED == 'true'
        run: |
          src="${{ vars.PAPERRUST_IMAGE }}:${{ steps.meta.outputs.version }}"
          tags=""
          for t in $(jq -r '.tags[]' <<<"$DOCKER_METADATA_OUTPUT_JSON"); do
            tags="$tags -t $DOCKERHUB_IMAGE:${t##*:}"
          done
          docker buildx imagetools create $tags "$src"
          docker buildx imagetools inspect "$DOCKERHUB_IMAGE:${{ steps.meta.outputs.version }}"
      - name: Docker Hub description
        if: env.DOCKERHUB_ENABLED == 'true' && github.ref_type == 'tag'
        uses: peter-evans/dockerhub-description@v4
        with:
          username: ${{ secrets.DOCKERHUB_USERNAME }}
          password: ${{ secrets.DOCKERHUB_TOKEN }}
          repository: paperrust/paperrust
          readme-filepath: packaging/stores/dockerhub/README.md
          short-description: ${{ steps.desc.outputs.short }}
```

The description step needs the repository checked out (the `manifest` job has no `actions/checkout` today: add
`- uses: actions/checkout@v7` with `sparse-checkout: packaging/stores/dockerhub` before it) and a small step
`id: desc` that runs `echo "short=$(cat packaging/stores/dockerhub/short-description.txt)" >> "$GITHUB_OUTPUT"`. Pin
`peter-evans/dockerhub-description` to a commit SHA like the other third-party actions if the repo policy asks for
it (check its README for the token scope it needs). Without the two secrets the job behaves exactly as today.
Also update the header comment of the workflow and the `release-image.yml` row in `docs/DEPLOY.md` (it currently
describes Docker Hub as an *alternative* via `REGISTRY_USERNAME`/`REGISTRY_TOKEN`). The README examples can then
mention `docker pull paperrust/paperrust`.

**Test:** run the workflow manually (`workflow_dispatch`, version empty → `edge`) once the secrets exist; check that
`docker buildx imagetools inspect docker.io/paperrust/paperrust:edge` lists amd64 + arm64, and that a run without
secrets still succeeds.

**Updates:** automatic with every `v*` tag once the change is in (tags `x.y.z`, `x.y`, `latest`).
