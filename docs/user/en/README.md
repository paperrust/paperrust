# PaperRust – user documentation

[Deutsch](../README.md)

PaperRust is a fast, lean and compatible replacement for
[paperless-ngx](https://github.com/paperless-ngx/paperless-ngx): its own web UI, but the same REST API
and the same database – as a single Rust program without Python, Redis, Celery, Tika and Gotenberg.
New in version 0.1.0 and known limits: [release notes](../../RELEASE-NOTES-0.1.0.en.md).

## Getting started

| Page | Content |
|---|---|
| [Getting started](getting-started.md) | choose how to run it, set up, first documents |
| [Installation](installation.md) | server with PostgreSQL, Docker, desktop app |
| [Switching from paperless-ngx – step by step](umstieg.md) | in plain words: one command, comparison at the end |
| [Switching from paperless-ngx (in depth)](migration.md) | same database, SQLite/MariaDB → PostgreSQL, export/import, going back |
| [Configuration](configuration.md) | all variables (`PAPERRUST_*` and compatible `PAPERLESS_*`) |

## Usage

| Page | Content |
|---|---|
| [Usage](usage.md) | terms, UI (PaperRust design and classic), consumption, search, saved views, sharing, trash, history |
| [Automation](automatik.md) | workflows with sentence view, templates, test run, history and otherwise branch; mail fetching; AI suggestions |
| [Mail](mail.md) | mail accounts and rules, processed mail |
| [Workflows](workflows.md) | reference: triggers, filters, actions, placeholders |
| [Custom fields](custom-fields.md) | own fields |
| [Folder view](folders.md) | file documents in folders (optional, PaperRust only) |
| [Users and permissions](permissions.md) | users, groups, global and object permissions, MFA |
| [File names and storage paths](storage-paths.md) | storage templates |
| [ASN labels](asn-labels.md) | print archive serial number labels, split batches automatically, separator sheets |
| [Connecting scanners](scanner.md) | scan button (AirScan/eSCL) and scan to email from the printer without a NAS password on the device |
| [Advanced topics](advanced.md) | matching, barcodes, consumption scripts, double-sided scans, SSO |
| [AI features](ai.md) | replaces paperless-ai/-gpt: suggestions with review list, custom fields, AI text recognition, chat, MCP server |
| [PDF tools](pdf-tools.md) | Redact, shrink, convert to PDF, repair on ingest |

## Operations

| Page | Content |
|---|---|
| [Administration](administration.md) | backup, `paperrust manage`, sanity checker, index, classifier |
| [Integrity check](integrity.md) | scheduled check of files and checksums, incremental mode, JSON report |
| [Ransomware protection](ransomware.md) | guard with read-only mode, encrypted versioned backups (also S3 with Object Lock), NAS advice |
| [REST API](api.md) | API (as paperless-ngx) and PaperRust extensions |
| [Security](security.md) | deployment, permissions, import, AI |
| [For companies](firmen.md) | running it with many staff: sign-in, roles, protection – and what is still missing |
| [FAQ](faq.md) | |
| [Troubleshooting](troubleshooting.md) | |

Mapping of the UI's documentation links: [LINKMAP](../LINKMAP.md) (German). Developer documentation:
[docs/ARCHITECTURE.md](../../ARCHITECTURE.md), [docs/UPSTREAM-SYNC.md](../../UPSTREAM-SYNC.md) (German), [CONTRIBUTING.md](../../../CONTRIBUTING.md).

---

PaperRust is based on paperless-ngx (GPL-3.0, © the paperless-ngx authors) and is also licensed under
the GPL-3.0. It is an independent project and not affiliated with the paperless-ngx team. This
documentation is our own text.
