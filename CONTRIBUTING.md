# Contributing to PaperRust

Thank you for your interest! PaperRust is a Rust re-implementation of paperless-ngx v3.4.2. The first
goal is **behavioural compatibility** with upstream (API, database, files) so that people can switch
in both directions; improvements come on top of that and are always made explicit.

- Bugs in PaperRust: open an issue here, **not** at paperless-ngx.
- Security vulnerabilities: **never** as a public issue – see [SECURITY.md](SECURITY.md).
- Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).
- Larger changes: please open an issue first, so we can agree on the approach before you invest time.

**Languages:** code comments, commit messages, pull requests and issues are in **English**. Much of
the existing developer documentation (`docs/*.md`, `docs/contract/`) is still in German; you may
write new contracts and developer notes in English. User documentation is bilingual: German in
`docs/user/`, English in `docs/user/en/` – please update both when behaviour changes (if you can
only write one language, say so in the pull request).

## Getting the code

The `upstream/` git submodule (paperless-ngx v3.4.2) is required to build and test – some crates
embed files from it and tests use its sample files.

```sh
git clone --recurse-submodules https://github.com/paperrust/paperrust.git
cd paperrust
# or, in an existing clone:  git submodule update --init
```

`upstream/` is a reference and is **never modified**.

## Building and testing

Requirements:

- Current stable Rust – the toolchain, including `rustfmt` and `clippy`, is selected automatically
  by `rust-toolchain.toml` (install [rustup](https://rustup.rs/)).
- Node.js ≥ 22 with Corepack (for `pnpm`) for the web UI.
- Python 3 for a few helper scripts and the parity harness.
- Optional: Tesseract 5 and PDFium for the OCR/rendering tests (they skip themselves when missing),
  Docker for the parity reference and the container image.

A plain `cargo` setup is enough; CI runs the Rust tests on a stock Ubuntu runner without extra
system packages. Windows works as well; macOS is not tested regularly.

### Rust

The workspace is large (about 40 crates). Build and test the crates you touch instead of the whole
workspace:

```sh
cargo fmt --all -- --check
cargo clippy -p <crate> --all-targets -- -D warnings
cargo test -p <crate>
```

- Clippy must be clean with `-D warnings`.
- Tests that need a PostgreSQL database or a running paperless-ngx instance are marked
  `#[ignore = "<reason>"]` and do not run by default; see
  [Database and parity tests](#database-and-parity-tests).
- PDF tool isolation tests: `cargo test -p pl-pdftools --features engine`.
- The Tauri desktop shell is a separate workspace: `cd apps/desktop-shell && cargo test` (on Linux it
  needs the WebKitGTK 4.1 development packages, e.g. `libwebkit2gtk-4.1-dev`).
- Release builds are slow and memory-hungry (the final link of `paperrust` needs several GB of RAM);
  for development, debug builds and `cargo test` are enough.
- Further checks used before releases (fmt, audit, deny, machete, gitleaks, semgrep):
  `scripts/run-gates.sh`. New dependencies must be compatible with GPL-3.0 (`deny.toml`).

### Web UI (`frontend-light/`)

The UI is Svelte + Vite and uses **pnpm** (lockfile `pnpm-lock.yaml`):

```sh
cd frontend-light
corepack enable
pnpm install && pnpm test       # Vitest
pnpm check                      # svelte-check
pnpm i18n:check                 # all 10 languages complete
pnpm build                      # also runs the i18n check
pnpm budget -- --lang de-DE     # size budget of the first page load (after build)
node tools/budget-design2.mjs   # size budget of the PaperRust design (after build)
```

The build fails if translations are missing; the budget scripts exit with code 1 when a budget is
exceeded. New UI texts need entries in all 10 languages (the check fails otherwise); if you cannot
translate into a language, say so in the pull request.

CI runs the same commands on every pull request (`.github/workflows/ci.yml`).

<a id="database-and-parity-tests"></a>

### Database and parity tests

Two kinds of tests need more than `cargo test`:

- **Database tests** (marked `#[ignore]`, the reason names what is needed) run against a PostgreSQL
  with the deterministic test data of the reference environment. Some take a database URL from an
  environment variable named in the test file's header comment. Run them explicitly once the
  required service is running, e.g. `cargo test -p <crate> --test <file> -- --ignored`.
- **Parity tests** compare PaperRust's API answers with a running paperless-ngx v3.4.2
  (`tests/parity/`, harness `tests/parity/harness/run_parity.py`). How cases, normalisation and
  accepted deviations work: [docs/contract/testing.md](docs/contract/testing.md) (German).

The reference environment (paperless-ngx v3.4.2, PostgreSQL 18, Valkey, fixed test data from
`reference/seed.py`, local test passwords only) can be started in two ways:

- **Linux with Docker (easiest):** `scripts/parity-reference-docker.sh up`, then
  `eval "$(scripts/parity-reference-docker.sh env)"` for PaperRust, `… reset` between write tests and
  `… down` to remove everything. This is what the manual parity workflow
  (`.github/workflows/parity.yml`) does.
- **Windows without Docker:** see [reference/README.md](reference/README.md) (German).

Both listen on `127.0.0.1` only. Never run tests against real mail servers or other third-party
services.

## Rules for code

1. **Contract first.** Every ported feature has a contract in `docs/contract/<crate>.md`: route,
   method, permission check, parameters, response fields in order, status codes, error texts and side
   effects – with the source (file and class/function in `upstream/`).
2. **Then code**, naming the upstream source in the header comment, e.g.
   `// upstream: src/documents/filters.py@v3.4.2 (DocumentFilterSet)`. This is used by
   `scripts/upstream-diff.sh` and `UPSTREAM_MAP.md` and documents where GPL code came from.
3. **Parity:** API behaviour is compared against a running paperless-ngx instance (see above). If
   your change touches a paperless-ngx route, run the parity cases for it or say in the pull request
   that you could not.
4. **No silent deviations.** Every intentional difference from upstream – even when fixing an upstream
   bug – is written down as a "Known difference" in the contract, with a reason.
5. **Permissions are always checked on the server through `pl-authz`.** Never take filters or object
   access from request parameters in a way that bypasses permissions.
6. **No `unwrap()`, `expect()` or `panic!` in request handlers.** Errors use the DRF-compatible format
   from `pl-core`.
7. **No passwords, tokens or secrets** in responses or logs; error texts without internals.
8. Database access via `sea-query`, no compile-time sqlx macros.
9. New dependencies only via `[workspace.dependencies]` in the root `Cargo.toml`.
10. Own extensions use own names: environment variables `PAPERRUST_*` (read through
    `pl_core::config`, so the `PAPERLESS_*` aliases keep working), tables `paperrust_*` (only add,
    never change upstream tables), routes `/api/x-…`, `/mcp`, `/__rs/`. Product name and version come
    from `pl_core::brand`.
11. Tests only against local services – no real mail servers, no third-party services.

Architecture overview: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (German). Following new upstream
versions: [docs/UPSTREAM-SYNC.md](docs/UPSTREAM-SYNC.md) (German). Branding and naming:
[docs/BRANDING.md](docs/BRANDING.md) (German).

## License of contributions

PaperRust is licensed under **GPL-3.0-only**. There is **no CLA**: contributions are accepted under
the same license as the project (inbound = outbound). By submitting a pull request you confirm that
you have the right to submit the change under GPL-3.0-only.

Please add a `Signed-off-by` line to your commits (`git commit -s`), which certifies the
[Developer Certificate of Origin](https://developercertificate.org/).

- Files taken from third parties keep their license; name origin and license in a header line or a
  `LICENSE` file next to them, and add them to [NOTICE](NOTICE).
- Documentation based on the paperless-ngx documentation says so (paperless-ngx v3.4.2, GPL-3.0,
  © the paperless-ngx authors).
- New files may start with `// SPDX-License-Identifier: GPL-3.0-only`; it is not required, the
  license applies to the whole repository via `LICENSE` and `Cargo.toml`.

## Commits and pull requests

1. Fork the repository and create a branch for your change.
2. Keep it small and focused; one topic per pull request.
3. Run fmt, clippy and the tests of the crates you changed (and `pnpm test` for UI changes).
4. Open a pull request against `main` and fill in the template. CI must be green before review.

- Commit messages in English: a short summary line (≤ 72 characters), a blank line, then what
  changed and why. Reference issues with `#123`.
- In the pull request description name the affected crates/routes, the parity result, new or changed
  "Known differences", and how you tested it.
- No large binaries (programs, databases, raw measurement data) in the repository; measurement
  reports as Markdown under `results/` are welcome.
