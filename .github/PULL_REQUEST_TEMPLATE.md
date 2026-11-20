## What and why

<!-- What does this change do, and why? Link issues with "Fixes #123". -->

## Affected crates / routes / UI

<!-- e.g. pl-documents, GET /api/documents/, frontend-light -->

## Compatibility with paperless-ngx

- [ ] No change in API/database behaviour compared to paperless-ngx v3.4.2
- [ ] Intentional difference, documented as "Known difference" in `docs/contract/<crate>.md`
- Parity result (if applicable):

## How was it tested?

<!-- Commands you ran, e.g. cargo test -p <crate>, pnpm test, manual steps. -->

## Checklist

- [ ] `cargo fmt --all -- --check` and `cargo clippy -p <crate> --all-targets -- -D warnings` are clean
- [ ] Tests added or updated (`cargo test -p <crate>` / `pnpm test`)
- [ ] UI texts: translations complete (`pnpm i18n:check`)
- [ ] Permissions are checked on the server via `pl-authz`; no `unwrap()`/`expect()` in handlers
- [ ] No secrets, personal data or large binaries in the change
- [ ] User docs updated where behaviour changed (`docs/user/` and `docs/user/en/`)
- [ ] Commits are signed off (`git commit -s`); I agree that the change is licensed under GPL-3.0-only
