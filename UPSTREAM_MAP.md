# UPSTREAM_MAP – Rust-Modul ↔ upstream-Python-Dateien ↔ Routen

Maschinenlesbar für `scripts/upstream-diff.sh`: Jede Tabellenzeile mit den vier Spalten
**Crate | Rust-Modul | Python (upstream v3.4.2) | Routen** ordnet geänderte upstream-Dateien
einem Crate zu. Regeln:

- Spalte *Python*: Pfade relativ zu `upstream/`, kommagetrennt, je in Backticks; Glob erlaubt
  (`*`, `**`, `?`), z. B. `` `src/documents/views.py`, `src/documents/filters.py` ``.
  Eine Datei darf mehreren Crates zugeordnet sein (z. B. `documents/views.py`).
- Spalte *Rust-Modul*: Pfad unter `crates/` (Datei oder `src/`), `-` wenn crate-weit.
- Spalte *Routen*: Routen-ids aus `routes()` / `routes.toml`, kommagetrennt, `-` wenn keine.
- Jede Crate pflegt **ihren** Abschnitt (Fach-Coder); die erste Zeile je Abschnitt ist die
  grobe crate-weite Zuordnung aus `docs/ARCHITECTURE.md` und wird durch feinere Zeilen ergänzt
  (nicht löschen – sie fängt neue Dateien auf).
- Nicht zugeordnete geänderte Dateien meldet `upstream-diff.sh` separat.

## pl-core

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-core | - | `src/paperless/settings/**`, `src/paperless/views.py`, `src/paperless/celery.py` | - |

## pl-authz

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-authz | - | `src/paperless/auth.py`, `src/documents/permissions.py`, `src/paperless/serialisers.py` | - |

## pl-server

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-server | - | `src/paperless/urls.py`, `src/paperless/middleware.py`, `src/paperless/asgi.py`, `src/paperless/consumers.py` | - |

## pl-auth-api

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-auth-api | - | `src/paperless/views.py`, `src/paperless/serialisers.py`, `src/paperless/filters.py`, `src/documents/views.py` | - |
| pl-auth-api | `pl-auth-api/src/token.rs` | `src/paperless/views.py`, `src/paperless/serialisers.py`, `src/paperless/settings/__init__.py` | auth.token |
| pl-auth-api | `pl-auth-api/src/profile.rs` | `src/paperless/views.py`, `src/paperless/serialisers.py`, `src/paperless_mail/serialisers.py` | profile.get, profile.patch, profile.generate_auth_token, profile.disconnect_social_account, profile.totp.get, profile.totp.post, profile.totp.delete |
| pl-auth-api | `pl-auth-api/src/social.rs` | `src/paperless/serialisers.py` | profile.get, profile.patch |
| pl-auth-api | `pl-auth-api/src/users.rs` | `src/paperless/views.py`, `src/paperless/serialisers.py`, `src/paperless/filters.py`, `src/documents/filters.py`, `src/documents/permissions.py` | users.list, users.retrieve, users.create, users.update, users.partial_update, users.deactivate_totp |
| pl-auth-api | `pl-auth-api/src/groups.rs` | `src/paperless/views.py`, `src/paperless/serialisers.py`, `src/paperless/filters.py`, `src/documents/signals/handlers.py` | groups.list, groups.retrieve, groups.create, groups.update, groups.partial_update, groups.destroy |
| pl-auth-api | `pl-auth-api/src/ui_settings.rs` | `src/documents/views.py`, `src/documents/serialisers.py`, `src/paperless/config.py`, `src/paperless/parsers/remote.py`, `src/paperless/version.py` | ui_settings.get, ui_settings.post |
| pl-auth-api | `pl-auth-api/src/password.rs` | `src/paperless/serialisers.py`, `src/paperless/settings/__init__.py` | profile.patch, users.create, users.update, users.partial_update |
| pl-auth-api | `pl-auth-api/src/qr.rs`, `pl-auth-api/src/totp.rs` | `pyproject.toml`, `uv.lock` | profile.totp.get, profile.totp.post, auth.token |

## pl-objects

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-objects | - | `src/documents/views.py`, `src/documents/serialisers.py`, `src/documents/filters.py`, `src/documents/matching.py` | - |

## pl-documents

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-documents | - | `src/documents/views.py`, `src/documents/serialisers.py`, `src/documents/models.py` | - |

## pl-docfilter

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-docfilter | - | `src/documents/filters.py` | - |

## pl-files

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-files | - | `src/documents/views.py`, `src/documents/bulk_download.py`, `src/documents/file_handling.py` | - |

## pl-documents-write

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-documents-write | - | `src/documents/views.py`, `src/documents/bulk_edit.py`, `src/documents/serialisers.py`, `src/documents/signals/**` | - |

## pl-fields-views

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-fields-views | - | `src/documents/views.py`, `src/documents/serialisers.py` | - |
| pl-fields-views | `pl-fields-views/src/custom_fields.rs` | `src/documents/views.py`, `src/documents/serialisers.py`, `src/documents/filters.py`, `src/documents/models.py`, `src/documents/signals/handlers.py`, `src/documents/permissions.py` | custom_fields.list, custom_fields.create, custom_fields.retrieve, custom_fields.update, custom_fields.partial_update, custom_fields.destroy |
| pl-fields-views | `pl-fields-views/src/saved_views.rs` | `src/documents/views.py`, `src/documents/serialisers.py`, `src/documents/models.py` | saved_views.list, saved_views.create, saved_views.retrieve, saved_views.update, saved_views.partial_update, saved_views.destroy |
| pl-fields-views | `pl-fields-views/src/share_links.rs` | `src/documents/views.py`, `src/documents/serialisers.py`, `src/documents/filters.py`, `src/documents/models.py` | share_links.list, share_links.create, share_links.retrieve, share_links.destroy |
| pl-fields-views | `pl-fields-views/src/bundles.rs` | `src/documents/views.py`, `src/documents/serialisers.py`, `src/documents/filters.py`, `src/documents/models.py`, `src/documents/tasks.py` | share_link_bundles.list, share_link_bundles.create, share_link_bundles.retrieve, share_link_bundles.destroy, share_link_bundles.rebuild |
| pl-fields-views | `pl-fields-views/src/audit.rs` | `src/documents/models.py` | custom_fields.create, custom_fields.update, custom_fields.partial_update, custom_fields.destroy |

## pl-workflows

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-workflows | - | `src/documents/views.py`, `src/documents/serialisers.py`, `src/documents/workflows/**` | - |

## pl-search

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-search | - | `src/documents/search/**`, `src/documents/views.py` | - |

## pl-system

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-system | - | `src/documents/views.py`, `src/paperless/views.py`, `src/documents/tasks.py` | - |

## pl-mail

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| pl-mail | - | `src/paperless_mail/**` | - |

## Schema (alle Crates)

| Crate | Rust-Modul | Python (upstream v3.4.2) | Routen |
|---|---|---|---|
| (alle) | - | `src/*/migrations/**`, `src/documents/models.py`, `src/paperless/models.py`, `src/paperless_mail/models.py` | - |
