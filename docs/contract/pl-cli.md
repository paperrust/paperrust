# Vertrag pl-cli (`paperrust manage …`)

Bibliothek mit allen Verwaltungsbefehlen, für die man bei paperless-ngx v3.4.2 `manage.py` braucht,
plus die OpenAPI-Routen. Upstream unverändert unter `upstream/` (Tag v3.4.2).

## Einbindung (W1)

```rust
// pl-server main: vor dem Start der Server-Runtime
if args.get(1).map(String::as_str) == Some("manage") {
    return pl_cli::run(args[2..].to_vec());          // -> std::process::ExitCode (eigene tokio-Runtime)
}
// innerhalb einer laufenden Runtime: pl_cli::run_async(args).await -> i32
// Routen: v.extend(pl_cli::routes());                // schema.get, schema.swagger_ui, static.swagger_ui_dist
// Mail-Abruf (W6) registrieren:  pl_cli::mail::set_mail_fetcher(Box::new(|| Box::pin(async { … })));
```

Einstellungen wie upstream aus `PAPERLESS_*` (über `pl_core::config::Config::from_env`), nur
PostgreSQL. Exit-Codes: 0 ok, 1 `CommandError` (Meldung `CommandError: …` auf stderr wie Django),
2 Aufruffehler (argparse). Django-Standardoptionen (`-v`, `--settings`, `--traceback`,
`--no-color`, `--skip-checks` …) und `--no-progress-bar`/`--processes` werden angenommen.
Fortschrittsbalken und Rich-Tabellen von upstream werden als einfache Textzeilen ausgegeben.
Testeinstieg ohne pl-server: `cargo run -p pl-cli --example manage -- <befehl> …`.

## Befehle

| Befehl | Upstream-Quelle | Status |
|---|---|---|
| `migrate [--no-input] [--check] [--plan] [--skip-paperrust] [--no-backup]` | django `migrate`, contenttypes/auth `post_migrate`, guardian `create_anonymous_user`, paperless-Datenmigration | fertig, Parität belegt |
| `showmigrations [app…]` | django `showmigrations` | fertig (nur Liste) |
| `createsuperuser [--username] [--email] [--no-input]` | django `createsuperuser` (DJANGO_SUPERUSER_*), Validatoren | fertig |
| `manage_superuser` | documents/management/commands/manage_superuser.py | fertig |
| `changepassword [user]` | django `changepassword` | fertig |
| `document_exporter …` | document_exporter.py, export/sinks.py, compression.py, mixins.py | fertig, Parität belegt |
| `document_importer …` | document_importer.py | fertig, Parität belegt |
| `document_sanity_checker` | sanity_checker.py, document_sanity_checker.py | fertig |
| `document_retagger …` | document_retagger.py, signals/handlers.py `set_*` | fertig (Klassifikator = PaperRust-Modell) |
| `document_index reindex\|optimize [--recreate] [--if-needed] [--heap-size-mb]` | document_index.py → pl-indexer | fertig |
| `document_create_classifier` | document_create_classifier.py, tasks.py `train_classifier` → pl-classifier | fertig |
| `document_thumbnails [-d ID] [--processes N]` | document_thumbnails.py → pl-consumer `NativeParsing` (pl-parsers/pl-ocr) | fertig (PDFium nötig), parallel (B7) |
| `document_archiver [-f] [-d ID] [--processes N]` | document_archiver.py → pl-consumer `update_document_content_maybe_archive_file` | fertig (B1), parallel (B7) |
| `document_fuzzy_match [--ratio R] [--delete] [--yes] [--url URL] [--processes N]` | document_fuzzy_match.py (rapidfuzz `default_process`/`fuzz.ratio`) | fertig (B2) |
| `document_llmindex rebuild\|update\|compact\|migrate` | document_llmindex.py → pl-ai `server::index` | fertig mit Feature `ai` (B3) |
| `document_consumer [ordner] [--oneshot] [--testing]` | document_consumer.py → pl-consumer `FolderWatcher::run_command` | fertig (B4) |
| `check [--fail-level L]` | paperless/checks.py, documents/checks.py, django `BaseCommand.check` | fertig (B5, PaperRust-Programme statt gs/pdftoppm) |
| `document_renamer` | document_renamer.py + `update_filename_and_move_files` | fertig |
| `prune_audit_logs` | prune_audit_logs.py | fertig |
| `empty_trash [--doc-ids 1,2]` | tasks.py `empty_trash` + `cleanup_document_deletion` (kein upstream-Befehl) | fertig |
| `mail_fetcher` | paperless_mail/management/commands/mail_fetcher.py | fertig (pl-server `cli.rs` registriert `process_mail_accounts` über `set_mail_fetcher`) |
| `decrypt_documents` | – | in v3.4.2 nicht vorhanden (GPG-Speicherung entfernt) |
| `convert_mariadb_uuid` | – | nicht umgesetzt (nur MariaDB) |

### migrate

Quelle des Schemas: `crates/pl-dbmigrate/schema/` (pre-/post-data aus `pg_dump` einer frisch mit
upstream migrierten DB, wiederverwendet per `include_str!`). Zusätzlich `crates/pl-cli/baseline/`
(erzeugt von `tests/parity/pl-cli/gen_baseline.sh`: upstream `migrate` auf leerer DB):
`v3.4.2-content-types.tsv` (42), `v3.4.2-permissions.tsv` (170), `v3.4.2-migrations-applied.tsv`
(247), `v3.4.2-applying.txt` (109 Ausgabezeilen in Planreihenfolge), `v3.4.2-sequences.tsv`,
`v3.4.2-applicationconfiguration.json`, `v3.4.2-nonempty.tsv` (Kontrolle: nur diese 5 Tabellen
haben Zeilen), `export-descriptor.json`.

1. Zustand: keine Relation in `public` → **Neuinstallation**; `django_migrations` vorhanden →
   Migrationsstand wie pl-dbmigrate (`classify`): alle 247 nötig; zusätzliche Zeilen erlaubt, wenn
   ersetzt (gesquasht), historisch (git-Historie ≤ v3.4.2) oder fremde App; sonst Abbruch mit
   „This database is not at paperless-ngx v3.4.2 …“ (fehlend → erst auf 3.4.2 aktualisieren;
   unbekannt → neuere Version, nicht unterstützt). Tabellen ohne `django_migrations` → Abbruch.
   Known newer migrations (`migrate::KNOWN_NEWER`, same list as pl-dbmigrate): `documents.0028`,
   `0029`, `0030_note_format` – a paperless-ngx 3.5.0/3.5.1 database counts as current.
   `pl_documents_write.0001_paperrust_note_format` (own migration) then adds `documents_note.format`
   if missing and sets the default `'plain'`.
2. Neuinstallation in **einer Transaktion**: pre-data, `django_migrations` (applied =
   `clock_timestamp()`), ContentTypes/Permissions mit den upstream-IDs, `AnonymousUser` (id 1,
   unbrauchbares Passwort `!`+40 Zeichen), ApplicationConfiguration-Singleton (id 1, `app_logo=''`),
   Sequenzstände, post-data (Index `unique_authenticator_type` mit Djangos `IN (…)`-Ausdruck, sonst
   speichert PostgreSQL einen anderen Ausdrucksbaum), Spaltenprüfung gegen `v3.4.2-columns.tsv`.
3. Bestehende v3.4.2-DB: `post_migrate`-Nachzug (fehlende ContentTypes/Permissions/AnonymousUser
   anlegen, nie ändern/löschen), Ausgabe „No migrations to apply.“
4. Danach `paperrust_*`-Migrationen (`pl_queue::migrations::ALL`, `crates/pl-ai/migrations/0001_paperrust_ai.sql`,
   … , zuletzt `pl_core::cache::migrations::ALL` = `pl_core.0001_paperrust_cache`: UNLOGGED-Tabelle
   `paperrust_cache` für `PAPERRUST_CACHE=postgres`, immer angelegt, seit 2026-11-14),
   expand-only/idempotent, protokolliert in `paperrust_migrations(name, applied)`; `--skip-paperrust`
   lässt sie aus. `--check`: Exit 1 bei leerer/abweichender DB oder offenen PaperRust-Migrationen.
   Seit 2026-11-13 nicht mehr nur expand-only: `pl_objects.0001_paperrust_tag_adjacency` (L6) entfernt
   die django-treenode-Spalten; paperless-ngx kann die Datenbank danach nicht mehr benutzen. Vor dem
   Umstieg daher sichern und paperless-ngx anhalten (docs/MIGRATION.md).
   ~~Sperre Mischbetrieb~~ (entfernt 2026-11-14): `migrate` verweigerte diese Migration, solange
   `PAPERRUST_DJANGO_URL` nicht `off` war (`MIXED_MODE_ENDS`). Die Sperre hing an der Proxy-Einstellung,
   die es nicht mehr gibt; Mischbetrieb ist ohne Proxy ohnehin unmöglich.
5. `document_exporter`/`document_importer` mit Tags ohne treenode-Spalten: `export::tagtree` (Export
   berechnet die 13 Felder für das v3.4.2-Manifest, Import überspringt sie). Test
   `export::tagtree::tests::export_records_identical_before_and_after_migration` (Referenzkopie,
   `#[ignore]`): Tag-Datensätze nach der Migration gleich denen vor der Migration, inkl. Feldreihenfolge.

6. **Umbaustufen des eigenen Datenmodells** (L6, `pl_store::datamodel::STAGES`, docs/DATAMODEL-V2.md
   §6) laufen immer zuletzt. Vor jeder Stufe, wenn `documents_document` Zeilen hat: Sicherung
   `pg_dump -Fc` nach `<PAPERLESS_DATA_DIR>/backups/paperrust-vor-<stufe>-<zeit>.pgdump`
   (`pg_dump` aus `PAPERRUST_PG_BIN`, neben dem Programm oder `PATH`; Zugang über `PG*`-Umgebung,
   nie auf der Befehlszeile; auch `PAPERRUST_DESKTOP_PG_BIN`). Fehlt `pg_dump`: **Abbruch vor der
   Stufe** mit Meldung „pg_dump was not found … Nothing was changed“ (seit 2026-11-16, Testprotokoll
   D-073; vorher Warnzeile und weiter ohne Sicherung). Weiter ohne Sicherung nur ausdrücklich:
   `PAPERRUST_MIGRATE_WITHOUT_BACKUP=true` (dann Warnzeile, Rückbau bleibt möglich). Scheitert
   `pg_dump`: Abbruch vor der Stufe. Ganz aus (schon selbst gesichert) mit `--no-backup` oder
   `PAPERRUST_DATAMODEL_BACKUP=off`. Leere DB (Neuinstallation, Ziel des Assistenten): keine Sicherung.
7. **`--plan`** listet nach den Django-Schritten auch die offenen eigenen Migrationen
   (`paperrust.<name>`, Umbaustufen mit Hinweis auf die Sicherung; seit 2026-11-16, D-070 –
   vorher „No planned migration operations.“ trotz offener PaperRust-Migrationen). `--skip-paperrust`
   lässt sie weg.
8. **Startprüfung für `paperrust serve`** (`migrate::startup_check`, seit 2026-11-16, A-181/B-312):
   gleiche Logik wie `--check`; der Server bricht ab, wenn v3.4.2 oder eine eigene Migration fehlt
   („Datenbank ist nicht auf dem neuesten Stand – zuerst `paperrust manage migrate` ausführen“).

Ausgabe wie Django: „Operations to perform: / Apply all migrations: account, admin, … / Running
migrations: / Applying x... OK“ (Systemcheck-Warnungen von upstream entfallen).

### datamodel (PaperRust)

`paperrust manage datamodel status | check | rollback --to <n>` (`crates/pl-cli/src/datamodel.rs`):
`status` listet die Stufen mit `[X]`; `check` vergleicht jede Kompatibilitäts-Sicht der eingespielten
Stufen mit `v3.4.2-columns.tsv` (Spaltenname, `udt_name`, Länge, Reihenfolge; Exit 1 mit Unterschied);
`rollback --to <n>` spielt die Rückbau-Skripte der Stufen > n neueste zuerst ein (je Stufe eine
Transaktion) und löscht ihre Zeilen in `paperrust_migrations`. Rundreise-Test:
`tests/datamodel/roundtrip.sh` (ngx-Export ↔ Umbau/Umstieg ↔ PaperRust-Export ↔ ngx-Import, Rückbau).

### createsuperuser / manage_superuser / changepassword

- `createsuperuser --no-input`: Benutzername aus `--username` oder `DJANGO_SUPERUSER_USERNAME`
  (sonst „You must use --username with --noinput.“), E-Mail aus `--email`/`DJANGO_SUPERUSER_EMAIL`,
  Passwort aus `DJANGO_SUPERUSER_PASSWORD` (fehlt → unbrauchbares Passwort, wie Django), **ohne**
  Validatoren (wie Django). Vergeben → „Error: That username is already taken.“
- interaktiv: Prompts wie Django (`Username (leave blank to use '…'):`, `Email address:`,
  `Password:`, `Password (again):`), Validatoren mit „Bypass password validation and create user
  anyway? [y/N]:“.
- Validatoren (`AUTH_PASSWORD_VALIDATORS`): UserAttributeSimilarity (0.7, `quick_ratio`,
  `exceeds_maximum_length_ratio`), MinimumLength 8, CommonPassword (Djangos Liste
  `common-passwords.txt.gz`, eingebettet, BSD), Numeric – Texte wie Django (englisch).
- Anlegen wie `create_superuser`: `is_staff`/`is_superuser`/`is_active` = true, `normalize_email`,
  PBKDF2-SHA256 über `pl_authz::hashers::make_password` (von Django geprüft, siehe Tests).
- `manage_superuser`: `PAPERLESS_ADMIN_USER` (admin), `PAPERLESS_ADMIN_MAIL` (root@localhost),
  `PAPERLESS_ADMIN_PASSWORD`; Meldungen exakt wie upstream (Name existiert / Superuser existiert /
  Passwort fehlt / „Created superuser "…" with provided password.“).
- `changepassword [user]` (Standard: OS-Benutzer), 3 Versuche, Validatoren ohne Umgehung,
  „Password changed successfully for user '…'“ bzw. CommandError „Aborting password change … after N attempts“.

### document_exporter

Optionen wie upstream: `target`, `-c/--compare-checksums`, `-cj/--compare-json`, `-d/--delete`,
`-f/--use-filename-format`, `-na/--no-archive`, `-nt/--no-thumbnail`, `-p/--use-folder-prefix`,
`-sm/--split-manifest`, `-z/--zip`, `-zn/--zip-name` (Standard `export-<lokales Datum>`),
`--zip-compression stored|deflated|bzip2|lzma|zstd`, `--zip-compression-level`, `--data-only`,
`--passphrase`, `--batch-size` (500). Fehlermeldungen wie upstream (Pfad fehlt/kein Verzeichnis/nicht
schreibbar, `--zip` mit `-c/-cj`, Kompressionsoptionen ohne `--zip`, Levelgrenzen; zstd: „zstd
compression requires Python 3.14 or newer“ entfällt, B6: alle fünf Methoden verfügbar).
ZIP-Methoden wie CPython `zipfile`: deflated (Stufe 0–9, Standard 6), bzip2 (Methode 12, Version 46,
Stufe 1–9, Standard 9), lzma (14, Version 63, Bit 0x02, Kopf `09 04 05 00` + 5 Byte Eigenschaften
der Voreinstellung 6 + Roh-LZMA1 mit Endmarke), zstd (93 wie Python 3.14 `ZIP_ZSTANDARD`, Version 63,
Stufe −22–22, Standard 3). Der Importer liest alle fünf (LZMA auch ohne Endmarke über die
angegebene Größe).

Ablauf (`dump`): `MEDIA_LOCK` (filelock-kompatibel), eine Lese-Transaktion **REPEATABLE READ**,
die 33 Manifest-Schlüssel in upstream-Reihenfolge mit den Abfragen aus dem Deskriptor (Django-SQL von
`gen_descriptor.py`: gleiche WHERE/ORDER BY, z. B. `Meta.ordering`, Ausschluss `consumer`/
`AnonymousUser`), serverseitiger Cursor mit `FETCH batch_size`. Datensatz = Djangos Serializer
„python“: `model`, `pk`, `fields` (local_fields mit `serialize=True` in Modellreihenfolge, FKs als
pk, m2m als pk-Liste über die Related-Manager-Abfrage mit Ziel-`Meta.ordering`, berechnete Spalten
wie `value_monetary_amount` als Decimal-Text), `DjangoJSONEncoder` (datetime → ISO mit
Millisekunden, `Z`; date ISO; UUID/Decimal/inet als Text). JSON wie `json.dumps(indent=2,
ensure_ascii=False)`, Manifest-Rahmen wie `StreamingManifestWriter` (`[`, `,\n`, `\n]`).
Dateinamen wie `generate_base_name` (`get_public_filename(max_bytes=240)` bzw. `generate_filename`
mit Zähler, eindeutig ohne Endung), `-thumbnail.webp`, `-archive.pdf`, `-manifest.json`, Ordnerpräfixe
`originals/ thumbnails/ archive/ json/`, Share-Link-Bündel unter `share_link_bundles/`.
Ordner-Senke: Snapshot, Kopie nur bei anderer Größe/mtime (bzw. SHA-256 mit `-c`), mtime/atime
übernommen, `manifest.json.tmp` → umbenennen, `-cj` byte-gleich → nicht schreiben, `-d` löscht
übrige Dateien und leere Ordner. ZIP-Senke: `<name>.zip.<uuid>.tmp` → `<name>.zip` (KD 14a), Verzeichniseinträge
wie `ZipFile.mkdir`, Dateien wie `ZipFile.write` (DOS-Zeit in `PAPERLESS_TIME_ZONE`), Manifest zuletzt,
`metadata.json` davor, ZIP64 bei Bedarf; `-d` leert das Ziel bis auf das ZIP (erst nach dem Umbenennen). `metadata.json`:
`{"version": "3.4.2"}` + `__crypto__` (`__key_algo__` pbkdf2_sha256, `__key_iters__` 1000000,
`__key_size__` 32, `__salt_hex__`). Passphrase: Fernet (AES-128-CBC + HMAC-SHA256, Token als Hex)
auf `paperless_mail.mailaccount.password/refresh_token` und `socialaccount.socialtoken.token/token_secret`
(nur nicht-leere Werte); ohne Passphrase und mit Mail-Konten/Tokens Hinweis „No passphrase was given,
sensitive fields will be in plaintext“.

### document_importer

`source` (Ordner oder ZIP), `--data-only`, `--passphrase`, `--batch-size`. Wie upstream: ZIP
entpacken (Pfade bereinigt wie `extractall`; nicht lesbare Methoden → „This archive uses compression
this Python version cannot read (…).“), Vorprüfung (Warnungen „Found file …“, „Found existing
user(s) …“, „Found existing documents(s) …“; fehlendes Manifest → CommandError), `metadata.json`/
`version.json` (Versionswarnung; verschlüsselt ohne Passphrase → CommandError), Manifeste
`manifest.json` + `**/*-manifest.json`, „Checking the manifest“ (Dateien vorhanden/lesbar), dann
**eine Transaktion**: alle ContentTypes und Permissions löschen (Django-Kaskade über `fk_refs` des
Deskriptors: CASCADE/SET_NULL/PROTECT), `SET CONSTRAINTS ALL DEFERRED`, je Modell Stapel zu
`batch_size` als `INSERT … ON CONFLICT (pk) DO UPDATE` aller nicht berechneten Spalten (Werte über
`Field.to_python`-Regeln; fehlende Felder → `get_default()`; `auto_now`/`auto_now_add` → jetzt, wie
Djangos `pre_save` in `bulk_create`), m2m `.set()`, `SET CONSTRAINTS ALL IMMEDIATE` (=
`check_constraints`), Sequenzen wie `sequence_reset_sql`. Kein Audit-Log, keine Signale (upstream
schaltet sie ab). Danach Dateien (`FileExistsError` bei vorhandenem Original), „Copy files into
paperless...“ bzw. „Data only import completed“, „Updating search index...“ + Neuaufbau (pl-indexer).
Manifeste werden gestreamt (Element für Element), nicht ganz geladen.

### Weitere Befehle

- `document_sanity_checker`: alle Dokumente (`global_objects`, `-created`) – Vorschaubild, Original
  (+SHA-256), Archiv (Prüfsumme/Dateiname-Paar, Datei, SHA-256), Inhalt leer (INFO); verwaiste Dateien
  unter `MEDIA_ROOT` (ohne `IGNORABLE_FILES`, `media.lock`, App-Logo). Meldetexte und Zusammenfassung
  („Found 2 document(s) with errors and 1 global warning(s).“) wie upstream, Tabelle als Text.
  PaperRust-Zusätze (Z2, `sanity.rs`, Benutzerdoku `docs/user/integrity.md`), Meldungen/Stufen/
  Zählungen unverändert: Prüfsummen parallel (`PAPERRUST_INTEGRITY_THREADS`, Standard ≤ 4), Meldungen
  danach in upstream-Reihenfolge; `content` wird nicht geladen (`COALESCE(content,'') = ''`), Dateiarbeit
  in `spawn_blocking`; `--incremental` bzw. `PAPERRUST_INTEGRITY_INCREMENTAL` (Task) überspringt
  Dateien mit gleichem Pfad/Größe/mtime/gespeicherter Prüfsumme wie bei der letzten Bestätigung in
  `paperrust_integrity_state` (Migration `pl_cli.0001_paperrust_integrity`, auch bei Bedarf angelegt),
  solange diese jünger als `PAPERRUST_INTEGRITY_FULL_HASH_DAYS` (28) ist; Abweichung/Lesefehler/fehlende
  Datei löschen die Bestätigung, Zeilen gelöschter Dokumente werden entfernt. JSON-Bericht je Lauf
  `DATA_DIR/integrity/integrity-<UTC>.json` (Format 1: `status` ok/issues/error, `result` = Task-Ergebnis,
  `files.hashed/skipped/bytes_hashed/orphaned`, `counts`, `issues[]` mit `level`, `document_id`,
  `title`, `message`), die neuesten `PAPERRUST_INTEGRITY_REPORT_KEEP` (10) bleiben. Start-/
  Abschlusszeile Logger `paperless.integrity`. Test: `tests/integrity_db.rs`
  (`PAPERRUST_INTEGRITY_TEST_DATABASE_URL`, DB `pl_integrity_test*`).
- `document_retagger -c -T -t -s [-i] [--use-first] [-f] [--suggest] [--base-url] [--id-range lo hi]`:
  wie `set_correspondent/set_document_type/set_tags/set_storage_path` (Kandidaten nach Besitzer
  rechtegefiltert, Klassifikator `PAPERRUST_CLASSIFIER_MODEL_FILE` bzw. `DATA_DIR/paperrust_classifier.model`,
  Schwelle `PAPERLESS_CLASSIFIER_MATCH_THRESHOLD`), Speichern mit Audit-Log (pl-consumer `docops`)
  und anschließendem `update_filename_and_move_files`.
- `document_index`: `reindex` = pl-indexer `rebuild` (Optionen 1:1), `optimize` = no-op mit Log.
- `document_create_classifier`: `train_classifier` (keine AUTO-Objekte → Modell löschen, Meldung
  „No automatic matching items, not training“; sonst laden/trainieren/speichern), Statuszeilen
  „  Gathering data from N document(s)...“ usw., „✓ Classifier training complete (Xs)“.
  `pl_cli::classifier::train_classifier(&ctx, notify)` ist für den Worker (W1) wiederverwendbar.
- `document_thumbnails [-d ID]`: Parser über die Registry, `get_thumbnail(source)` → WebP nach
  `THUMBNAIL_DIR/{pk:07}.webp`; ohne PDFium Standardbild (wie upstream bei Renderfehler).
- `--processes N` (B7, `par.rs`, base.py `PaperlessCommand`): Standard `max(1, CPUs // 4)`, `< 1` →
  `CommandError: --processes must be at least 1`; `process_parallel` startet höchstens N Aufgaben
  gleichzeitig (tokio + `spawn_blocking` statt `ProcessPoolExecutor`), Ergebnisse in
  Fertigstellungsreihenfolge (`as_completed`). Gilt für `document_thumbnails`, `document_archiver`,
  `document_fuzzy_match`; bei den übrigen Befehlen wie upstream angenommen und ohne Wirkung.
- `document_archiver [-f/--overwrite] [-d/--document ID]` (B1, `archiver.rs`): `SCRATCH_DIR` anlegen,
  Dokumente (ohne Papierkorb, `-created`) ohne `archive_filename` bzw. alle mit `-f`; `-d 0` gilt wie
  upstream als „alle“. Je Dokument dieselbe Aufgabe wie der Worker
  (`Consumer::update_document_content_maybe_archive_file(id, remote_ocr=False)`: Parser, Inhalt,
  Archiv-PDF + Prüfsumme, Vorschaubild, Audit „Update document content“, Barcodes, Suchindex); danach
  wie der Worker KI-Index (`update_document_in_llm_index`, Feature `ai`) und Fristen (pl-timeline).
  Aufgaben-Ausnahmen (Dokument fehlt, DB) → „Failed document {id}: {fehler}“; Parserfehler protokolliert
  die Aufgabe selbst (wie upstream, kein „Failed“).
- `document_fuzzy_match` (B2, `fuzzy.rs`): `--ratio` (Standard 85.0, sonst „The ratio must be between
  0 and 100“), Inhalte `Document.objects` nach id, jedes Paar einmal, Paare mit leerem `strip()`-Inhalt
  übersprungen, `default_process` (nicht alphanumerisch → Leerzeichen, klein, getrimmt) und
  `fuzz.ratio` mit `score_cutoff` (normierte Indel-Ähnlichkeit, LCS bitparallel wie
  pl-matching), Treffer absteigend; Ausgabe „Fuzzy Matches (threshold: 85.0%)“, je Paar „#id Titel“
  (bzw. `<url>/documents/<id>/details` mit `--url`) und Prozent, „Found N matching pair(s).“ (+ „M
  document(s) will be deleted.“), ohne Treffer „No duplicate documents found.“. `--delete` warnt vorab,
  fragt „Delete N document(s)? [y/N]“ (außer `--yes`) und legt das jeweils zweite Dokument in den
  Papierkorb (`QuerySet.delete()` des Soft-Delete-Managers = pl-documents-write
  `soft_delete_document` mit Audit), „Done.“ bzw. „Deletion cancelled.“.
- `document_llmindex` (B3, `llmindex.rs`): `rebuild`/`update` = Aufgabe `llmindex_index(rebuild=)`
  (nur mit aktiviertem KI-Index, sonst Log „LLM index is disabled, skipping update.“), `compact` =
  `llm_index_compact`, `migrate` = `llm_index_migrate` (nur aktiviert; ein Index mit altem Schema →
  Warnung „requires re-embedding … Run 'document_llmindex rebuild'“, kein Neuaufbau). Ohne Feature `ai`:
  `CommandError: This build of PaperRust has no AI support`.
- `document_consumer [ordner] [--oneshot]` (B4, `consumer.rs`): Ordner = Argument bzw.
  `CONSUMPTION_DIR` („CONSUMPTION_DIR is not configured“, „Consumption directory does not exist: …“,
  „Consumption path is not a directory: …“), `SCRATCH_DIR` anlegen, Einstellungen `CONSUMER_*`,
  vorhandene Dateien als `consume_file` in die PaperRust-Warteschlange (der Worker liest ein), mit
  `--oneshot` Ende, sonst Beobachtung (native Ereignisse bzw. Polling, siehe pl-consumer-Vertrag) bis
  Strg+C („Received interrupt, stopping consumer“). `--testing` wird angenommen und ignoriert;
  `PAPERLESS_CONSUMER_DISABLE` gilt wie upstream nur für den Dienst.
- `check [--fail-level CRITICAL|ERROR|WARNING|INFO|DEBUG]` (B5, `check.rs`): Prüfungen aus
  `paperless/checks.py` und `documents/checks.py` mit upstream-Texten: Ordner DATA_DIR,
  EMPTY_TRASH_DIR, MEDIA_ROOT, CONSUMPTION_DIR (vorhanden, Schreibtest
  `__paperless_write_test_<pid>__`), Programme, DEBUG, `OCR_OUTPUT_TYPE`/`OCR_MODE`/
  `ARCHIVE_FILE_GENERATION`/`OCR_CLEAN`/Zeitzone/E-Mail-Zertifikat, Audit-Tabelle bei
  abgeschaltetem Audit-Log, Mindestversion für v3 (`paperless.E002`), veraltete DB-Variablen
  (`paperless.W001`), veraltete v2-OCR-Variablen (`W002`/`W003`), OCR-Sprachen installiert
  (`tesseract --list-langs`), alte Dateinamen-Vorlage. Ausgabe wie Django („System check identified
  some issues:“, Gruppen ERRORS/WARNINGS, `?: (id) …`, `\tHINT: …`, „System check identified N issues
  (0 silenced).“); ab `--fail-level` (Standard ERROR) `SystemCheckError: …` auf stderr, Exit 1; nur
  Warnungen → stderr, Exit 0; nichts → stdout „System check identified no issues (0 silenced).“.
  `--deploy`, `--tag`, `--database` und App-Namen werden angenommen und ignoriert.
- `document_renamer`: für jedes Dokument (`Document.objects`, `-created`)
  `update_filename_and_move_files` (Original + Archiv, Konfliktlösung `generate_unique_filename`,
  „bereits verschoben“ per Prüfsumme, Rücksprung bei Fehlern, leere Ordner löschen, Versionen mitziehen).
- `prune_audit_logs`: „Deleted audit log entry for Document #12“ je gelöschtem Eintrag.
- `empty_trash [--doc-ids …]`: Papierkorb älter als `PAPERLESS_EMPTY_TRASH_DELAY` Tage (bzw. IDs),
  harte Löschung mit Kaskade (Versionen, Notizen, Custom-Field-Werte, Tags, Share-Links …), Dateien
  löschen oder nach `PAPERLESS_TRASH_DIR` verschieben (Zähler `_01`), Audit-Einträge der Dokumente löschen.

## Routen (`pl_cli::routes()`)

| id | Methode, Pfad | Upstream | Status |
|---|---|---|---|
| `schema.get` | GET `/api/schema/` | `SpectacularAPIView` | fertig (englisch) |
| `schema.swagger_ui` | GET `/api/schema/view/` | `SpectacularSwaggerView` | fertig |
| `static.swagger_ui_dist` | GET `/static/drf_spectacular_sidecar/swagger-ui-dist/{file}` | collectstatic/whitenoise | fertig (eingebettet) |

`/api/schema/`: Rechte `AllowAny`, Authentifizierung wird trotzdem ausgewertet (ungültiges Token →
401 „Invalid token header. No credentials provided.“ im ausgehandelten Format, `WWW-Authenticate:
Token`). Inhaltsaushandlung wie DRF `DefaultContentNegotiation` über die vier Renderer
(`application/vnd.oai.openapi` YAML, `application/yaml`, `application/vnd.oai.openapi+json`,
`application/json`), `?format=yaml|json` (sonst 404 „Not found.“), keine q-Werte; kein Treffer →
406 „Could not satisfy the request Accept header.“ (YAML). Version aus dem akzeptierten Media-Type
(`version=`), Standard 10, erlaubt 9/10, sonst 406 „Invalid version in "Accept" header.“. Antwort =
eingebettete, byte-gleiche Django-Antwort (`assets/schema/v3.4.2-api{9,10}.{json,yaml}.gz`),
`Content-Disposition: inline; filename="PaperRust REST API (10).yaml|json"` (upstream `Paperless-ngx REST API …`, Known difference 20). Fehlerkörper: JSON
`json.dumps(indent=4)`, YAML `detail:\n  string: …\n  code: …\n` (Darstellung von `ErrorDetail` durch
drf-spectacular).
`/api/schema/view/`: Swagger-HTML wie drf-spectacular (Sidecar), maskiertes CSRF-Token im Skript,
Cookie `csrftoken=<Geheimnis>; expires=…; Max-Age=31449600; Path=/; SameSite=Lax` (vorhandenes
Geheimnis wird übernommen), `Cross-Origin-Opener-Policy: unsafe-none`, `?lang`/`?version` an die
Schema-URL (escapejs). Ungültige Anmeldung → 401 `text/html` „401 Unauthorized“.
Reihenfolge wie `APIView.initial`: zuerst Content-Negotiation (`renderer_classes =
[TemplateHTMLRenderer]`, Media-Type `text/html`, Format `html`; `?format=` ≠ `html` → 404 „404 Not
Found“; kein passender `Accept`-Eintrag, z. B. `application/json` → 406 „406 Not Acceptable“), dann
`AcceptHeaderVersioning` auf dem akzeptierten Media-Type (ungültige Version → 406), erst dann
Authentifizierung. Fehlerkörper wie `TemplateHTMLRenderer.get_exception_template` ohne Vorlage
(`"%d %s" % (status, status_text.title())`, `text/html; charset=utf-8`), ohne COOP-Header der View
und ohne CSRF-Cookie. Leeres `?format=` wird wie in DRF ignoriert (auch beim Schema).
Akzeptierter Media-Type wie DRF `select_renderer`: ist der Renderer-Typ spezifischer als der
Client-Typ (`*/*`, `application/*`), Renderer-Typ + Client-Parameter
(`application/vnd.oai.openapi;version=9`), sonst der Client-Typ unverändert.
Swagger-UI-Dateien: drf-spectacular-sidecar 2026.10.1 (Swagger UI, Apache-2.0, Lizenz unter
`assets/swagger-ui-dist/LICENSE`), gzip eingebettet (~540 KB).

## Tests

- Sicherheit (Review Welle 3, je Befund ein Test mit präpariertem Manifest/ZIP): `importer::tests::
  h2_unsafe_stored_filenames_rejected`, `m4_source_paths_outside_import_dir_rejected`,
  `m5_share_link_bundle_target_contained`, `crypto_params_bounded` (C-N1),
  `manifest_walk_depth_limited` (C-N2), `zip::tests::zip64_hostile_counts_rejected_without_panic`,
  `unzip_limit_and_declared_size_enforced` (M6), `exporter::tests::zip_name_must_be_plain` (C-N3),
  `dir_sink_refuses_dotdot_arcname` (C-N4), `safepath::tests::*`.
- Unit (`cargo test -p pl-cli --lib`, 33 Tests, inkl. CPython-set-Reihenfolge): argparse-Ersatz, Baseline-Konsistenz, Migrationsstand,
  Python-JSON/float-repr, DjangoJSONEncoder-Zeiten, inet, Fernet (Spezifikationsvektor), ZIP schreiben/
  lesen, Manifest-Strom, `to_python`/Defaults, pathlib, Passwortvalidatoren, Negotiation, eingebettete
  Schemata. `cargo clippy -p pl-cli --all-targets -- -D warnings` sauber.
- Parität (`tests/parity/pl-cli/`, eigene DBs `pl_w7_*` aus `paperless_snapshot` bzw. leer, eigene
  Ordner unter `.work/`; upstream `manage.py` aus `reference/venv`): siehe Abschnitt Ergebnisse.

## Known differences

1. **Ausgabeformat**: Rich-Tabellen/Fortschrittsbalken als einfache Textzeilen; Django-Systemcheck-
   Warnungen fehlen. Inhalt (Meldungstexte, Zählungen) wie upstream.
2. **JSON-Zeilenenden**: Rust schreibt `\n` (wie upstream unter Linux/Docker); upstream unter Windows
   schreibt `\r\n` (Textmodus). Vergleiche normalisieren CRLF.
3. **ZIP**: alle Methoden (stored/deflated/bzip2/lzma/zstd) schreib- und lesbar (B6); upstream hängt
   bzip2/lzma von den Python-Modulen und zstd von Python ≥ 3.14 ab und meldet sonst „not available on
   this Python runtime“/„requires Python 3.14 or newer“ – PaperRust kann immer alle. Komprimierte Bytes
   (andere bzip2-/liblzma-/zstd-Implementierung bzw. -Version) und Zeitstempel der
   Verzeichniseinträge nicht byte-gleich, Inhalt, Methoden-IDs, Versionsfelder und Flags gleich.
4. **Importer entschlüsselt im Speicher** statt `*.decrypted.json` neben die Manifeste zu schreiben.
5. **OpenAPI nur englisch**: `?lang=`/`Accept-Language` ≠ en liefern die englische Fassung (upstream
   übersetzt Beschreibungen); `FORCE_SCRIPT_NAME` verändert die eingebetteten Pfade nicht
   (upstream: `SCHEMA_PATH_PREFIX_INSERT`). OPTIONS-Metadaten der Schema-Views nicht nachgebildet.
6. **Passwort-Eingabe** wird nicht verdeckt (kein getpass-Ersatz ohne neue Abhängigkeit).
7. **`empty_trash`**: Audit-„delete“-Einträge der mitgelöschten Notizen/Custom-Field-Werte (auditlog
   post_delete) werden nicht geschrieben; LLM-Index-Aufräumen (W8) nicht ausgelöst. Sobald
   `pl_documents_write::softdelete::empty_trash` öffentlich ist, sollte der Befehl es nutzen.
8. **Renamer/Retagger**: `clear_document_caches`/Suggestions-Cache (Djangos `default`-Cache) wird nicht
   geleert; Klassifikator ist das PaperRust-Modell (pl-classifier), nicht das Pickle.
9. **`prune_audit_logs`**: Einträge zu nicht mehr installierten Modellen werden übersprungen (upstream
   bricht mit AttributeError ab).
10. **Upstream-Nichtdeterminismus**: Reihenfolge der `*_squashed`-Zeilen in `django_migrations` und
    Gleichstände in `ORDER BY` (z. B. `-created`) schwanken schon bei upstream zwischen Läufen.
11. Python-`str.isdigit()` im Numeric-Validator angenähert (`char::is_numeric` ohne Brüche/römische
    Zahlzeichen).
12. **Import-Containment (Sicherheitsreview Welle 3, H2/M4/M5; Rust strenger als upstream)**,
    `export/safepath.rs`. Gültige Exporte (relative Pfade mit `/`) sind nicht betroffen:
    * `documents.document` `fields.filename`/`archive_filename` müssen relativ sein, ohne `..`, ohne
      führenden Trenner, ohne Laufwerk/Präfix (`C:x`) und ohne NUL – geprüft in `check_manifest_validity`
      **auch bei `--data-only`**, sonst `CommandError: Document <pk> has an unsafe filename "…"`
      (vor jeder DB-Änderung). Beim Kopieren zusätzlich `validate_path_in_root` gegen
      ORIGINALS_DIR bzw. ARCHIVE_DIR (`Refusing to import original|archive file …`). upstream übernimmt
      die Werte ungeprüft (`models.py:465-483`, `document_importer.py:647-666`) und kann so beliebige
      Host-Dateien überschreiben. `\` gilt dabei auf allen Plattformen als
      Trenner: upstream unter Windows speichert `ACME Corp\2019\x.pdf` – das bleibt gültig (im
      Referenz-Snapshot vorhanden); Folge: ein Linux-Name mit wörtlichem `\..\` oder führendem `\`
      wird abgelehnt.
    * `__exported_file_name__`/`__exported_archive_name__`/`__exported_thumbnail_name__`/
      `__exported_share_link_bundle_name__` müssen auf eine reguläre Datei (kein Symlink) innerhalb
      des Importverzeichnisses bzw. des entpackten ZIPs zeigen (kanonisch geprüft), sonst
      `The manifest file refers to "…" which is outside of the source directory or not a regular file.`
      upstream liest `self.source / name` ohne Prüfung (auch absolute Pfade).
    * Share-Link-Bündel (Import-Ziel und Export-Quelle) wie `ShareLinkBundle.absolute_file_path`:
      nicht absolut – unter Windows zählen auch `\x` und `C:x` als absolut –, nach Auflösen innerhalb
      von SHARE_LINK_BUNDLE_DIR; lexikalisches `a/../b` innerhalb bleibt wie upstream erlaubt.
13. **ZIP-Lesen gehärtet (M6)**: ZIP64-Eintragszahl muss zu `cd_size / 46` passen, Offsets/Größen
    mit `checked_add` gegen die Dateigröße, Zentralverzeichnis ≤ 256 MiB, keine Vorreservierung aus
    Eingabedaten; jeder Eintrag muss genau seine angegebene Größe liefern („larger than declared“/
    „truncated“ – CPython bricht dann mit CRC-/EOF-Fehler ab). Zusätzlich Entpackgrenze: Summe der
    Größen ≤ `PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES` (Standard 1 GiB + 100 × Archivgröße), sonst
    `ZIP archive expands to more than N bytes`; upstream `zipfile.extractall` hat keine Grenze.
14. **Niedrige Befunde (C-N1…C-N4)**: `__key_iters__` muss 1…10 000 000 und `__key_size__` genau 32
    sein (sonst `CommandError: Invalid key iteration count/key size`; upstream rechnet beliebig
    lange bzw. scheitert später). Verzeichnisdurchläufe (Manifest-Suche, Prüfung auf leere
    Installation, sanity_checker) folgen keinen Verzeichnis-Symlinks (wie `Path.glob("**")`) und
    enden nach 64 Ebenen; Symlinks werden bei der Manifest-Suche ganz übergangen.
    `--zip-name` muss ein reiner Dateiname sein (kein `/`, `\`, `..`, Laufwerk; upstream erlaubt
    Pfade relativ zum Ziel); `--delete` mit `--zip` schreibt eine Warnung auf stderr (Verhalten wie
    upstream). Beim Export werden Archivnamen mit `..`/`\`/Laufwerk abgelehnt statt außerhalb des
    Zielordners geschrieben.
14a. **ZIP-Export robuster** (2026-11-27, Review-Befunde 1/2): Die Temp-Datei heißt
    `<name>.zip.<uuid>.tmp` (je Lauf eindeutig, `create_new`) statt `<name>.zip.tmp`, und sie wird erst
    **nach** dem `MEDIA_LOCK` angelegt (wie upstream `with FileLock(...), sink:`); zwei gleichzeitige
    Exporte in dasselbe Ziel können sich das Archiv nicht mehr gegenseitig abschneiden. Mit `-d` wird
    zuerst umbenannt und erst danach der Rest des Ziels gelöscht (upstream `_finalize` löscht vorher:
    scheitert das Umbenennen, wären ältere Exporte schon weg). Tests
    `exporter::tests::concurrent_zip_sinks_use_separate_temp_files`,
    `zip_delete_keeps_old_exports_when_final_rename_fails`.
14b. **`showmigrations`** (2026-11-27, Review-Befund 8): Scheitert die Abfrage der offenen
    `paperrust_*`-Migrationen, endet der Befehl mit dem Fehler (vorher: leere Liste, alles `[X]`,
    Exit 0). Test `migrate::tests::showmigrations_propagates_pending_query_error`.
- `manage migrate` auf älteren Datenbanken (Echttest 2026-11-16, echte 3.1.3-DB): Die Schlussprüfung
  der Spalten vergleicht ohne Reihenfolge (nachgeholte Spalten stehen hinten; vorher Rollback des
  ganzen Nachholens). Vor ausstehenden `paperrust_*`-Migrationen bekommen Sequenzen der ngx-Tabellen
  ihren üblichen Namen `<tabelle>_<spalte>_seq` (alte DBs tragen nach Umbenennungen noch z. B.
  `documents_sender_id_seq`, `documents_comment_id_seq`; Django stört der Name nicht).

15. **Systemprüfungen (B5)**: `check` meldet fehlende PaperRust-Programme (Tesseract über
    `PAPERRUST_TESSERACT_PATH`/PATH, PDFium) statt `gs`, `pdftoppm`, `pdfinfo` (braucht PaperRust nicht;
    Text „PaperRust can't find …“). Nicht erreichbare Datenbank → Fehler in der Liste (upstream:
    Traceback). Die Django-eigenen Prüfungen (Modelle, Sicherheit `--deploy`) gibt es nicht, und andere
    Befehle führen die Prüfungen nicht vorab aus (upstream: `requires_system_checks` bei jedem Befehl).
    `OCR_MODE`/`ARCHIVE_FILE_GENERATION` mit ungültigem Wert erscheinen als Fehler (upstream bricht
    schon beim Laden der Einstellungen ab). Der Docker-Start führt `manage check` nach `migrate` aus
    (upstream `init-system-checks`), bricht bei Fehlern aber nicht ab (upstream: Container startet nicht).
16. **`document_fuzzy_match`**: Ausgabe als Textzeilen statt Rich-Tabelle/Panels. `default_process`
    nutzt Rusts `char::is_alphanumeric`/einfache Kleinschreibung statt rapidfuzz' Unicode-Tabelle (für
    lateinische Schrift gleich, bei seltenen Zeichen/Kombinationszeichen evtl. abweichend). Gelöscht wird
    wie upstream per Soft-Delete; Suchindex wird dabei (wie upstream) nicht angepasst.
17. **`document_archiver`**: Fortschrittsbalken fehlt; Django-Cache (`clear_document_caches`) wird
    nicht geleert (Konsument im Befehl ohne Redis-Cache-Anbindung, wie bei Renamer/Retagger).
18. **MEDIA_LOCK mit Zeitlimit, feste Sperrreihenfolge** (tests/stress/BEFUNDE.md B1, 2026-11-18,
    bewusst): `export::medialock::MediaLock` nutzt `pl_consumer::medialock` – asynchron (blockiert keinen
    Tokio-Thread, `empty_trash` läuft auch im Worker), Zeitlimit `PAPERRUST_MEDIA_LOCK_TIMEOUT`
    (Standard 120 s, `0` = ohne Limit) statt upstream `timeout=-1`; danach Fehler „timed out … waiting
    for the media lock“. Löschen der Sperrdatei beim Freigeben unter Windows wie filelock.
    `update_filename_and_move_files` (Renamer, Retagger) sperrt **erst die Dokumentzeile**
    (`lock_document_rows`, eigene Transaktion bzw. Sicherungspunkt), dann MEDIA_LOCK, und schreibt unter
    MEDIA_LOCK nur noch diese Zeile – gleiche Reihenfolge wie der Server, sonst Verklemmung über
    Prozessgrenzen. Exporter (nur lesende Transaktion), Importer und `empty_trash` (Dateien erst nach dem
    Commit) warten unter MEDIA_LOCK nie auf Zeilensperren.
19. **Dateinamen aus der DB nur über `safe_join`** (Sicherheitsprüfung 2026-11-19 F9): `Ctx::source_path`/
    `archive_path`, Renamer/Retagger (`filemove.rs`), `document_sanity_checker` und
    `document_thumbnails` verbinden `filename`/`archive_filename` über `pl_consumer::files::join`
    (`pl_core::safepath::safe_join`). Ein Name, der aus dem Medienordner hinausführt (`..`, absolut,
    unter Windows auch Laufwerk/Strom/Gerätename), gilt als fehlende Datei („Original of document does
    not exist.“, Renamer verschiebt nicht, Vorschaubild: Fehler „original file name is unsafe“), mit
    Warnung im Log. upstream liest/verschiebt solche Pfade ohne Prüfung.
20. **Schema-Titel `PaperRust REST API`** (Branding): `info.title` des Schemas und damit der
    Dateiname in `Content-Disposition` heißen `PaperRust REST API (<version>).json|yaml` statt
    `Paperless-ngx REST API …` (`openapi.rs` `TITLE`). Inhalt der Pfade/Schemas unverändert.
21. **Note `format` in export/import** (2026-11-22, paperless-ngx 3.5 #14626, `export::noteformat`):
    the importer accepts `format` on `documents.note` records (3.5 exports carry it on every note) and
    writes it when `documents_note.format` exists, else skips it; a missing field → `plain`. The
    exporter writes `format` (after `note`, 3.5 order) **only for non-plain notes**; upstream 3.5
    always writes it. Reason: plain notes stay byte-identical to a v3.4.2 export, so the export imports
    into paperless-ngx 3.4.2 (whose importer rejects unknown fields) and into 3.5 (missing field →
    model default). Exports with Markdown notes need paperless-ngx ≥ 3.5; the exporter then prints
    „N note(s) use Markdown … paperless-ngx before 3.5 cannot import this export“. `metadata.json`
    stays `{"version": "3.4.2"}`.
22. **Importer copies files before the commit** (2026-11-27, `export::importer::import`): upstream
    (`document_importer.py`, `handle` → `load_data_to_database` in `transaction.atomic()`, then
    `_import_files_from_manifest`) commits the database first and copies the files afterwards, so a
    failed copy (disk full, unreadable export file, existing target) leaves documents without files.
    PaperRust copies the files into their final place while the import transaction is still open
    (documents read via the same transaction, `ApplicationConfiguration` for the TIFF extension read
    on it too) and commits only afterwards. On a copy error the transaction is rolled back and every
    file this run created is removed again ("Copying the files failed; the database import was rolled
    back", then "Copied files were removed again" or a list of paths that could not be removed); on a
    commit error likewise ("Database commit failed"). Files that existed before (e.g. an overwritten
    thumbnail) are never removed. Residual risk: a process crash between copying and commit leaves
    orphan files without rows (found by `document_sanity_checker`), never rows without files. The
    transaction stays open during copying (long for big exports). Test:
    `importer::tests::failed_import_removes_only_own_copies`.
23. **`check` write probe without truncation** (2026-11-27, `check.rs` `write_probe`): upstream
    `paths_check` (`paperless/checks.py`) opens `__paperless_write_test_<pid>__` with mode `"w"`
    (truncates an existing file of that name) and deletes it. PaperRust creates the probe with
    `create_new`; if the name exists it tries `…_<pid>_<n>__` (up to 16 names) and deletes only the
    file it created; an existing file is never touched. Messages unchanged. Test:
    `check::tests::path_check_keeps_existing_probe_name`.

## CORE-REQUEST

1. **W1**: `paperrust manage …` → `pl_cli::run(args)`; `pl_cli::routes()` in `all_routes()`;
   `pl_cli::mail::set_mail_fetcher(...)` mit W6s `process_mail_accounts`.
2. **W8**: `pl_ai::migrations::ALL: &[(&str, &str)]` bereitstellen (pl-cli liest bis dahin
   `crates/pl-ai/migrations/0001_paperrust_ai.sql` per Pfad; die Datei trägt noch den Hinweis
   „NOCH NICHT AUSFÜHREN“ – `migrate` spielt sie wie beauftragt ein, `--skip-paperrust` lässt sie aus).
3. **W4**: `pl_documents_write::softdelete::empty_trash` öffentlich machen (Known difference 7).
4. Abhängigkeit `aes = "=0.9.4"` (Fernet) in `crates/pl-cli/Cargo.toml` – steht bereits in `Cargo.lock`
   (keine neue Kiste); bitte in `[workspace.dependencies]` übernehmen.
6. **pl-consumer**: `db::add_tags`/`docops::add_nested_tags` in CPython-`set`-Reihenfolge einfügen
   (`pl_cli::pyset::missing_targets_order` bzw. nach pl-consumer übernehmen), damit die m2m-IDs exakt
   wie upstream sind.
5. pl-cli nutzt `pl-consumer` (docops/audit/templating/matchables/NativeParsing), `pl-indexer`,
   `pl-classifier`, `pl-queue`, `pl-ocr` per Pfad (nicht im Workspace-Abschnitt).

## Ergebnisse (2026-11-08, Debug-Build, Referenz-PostgreSQL 18.6, upstream v3.4.2 aus reference/venv)

| Skript | Vergleich | Ergebnis |
|---|---|---|
| `test_migrate.sh` | Rust-`migrate` auf leerer DB vs. upstream `manage.py migrate` auf leerer DB: `pg_dump --schema-only`, alle Tabelleninhalte (django_migrations als Menge, AnonymousUser ohne Zufallspasswort/Zeit), Sequenzen, Ausgabe | **identisch**; upstream `migrate --check` auf der Rust-DB ok, 0 offene Migrationen laut `showmigrations`; Zweitlauf „No migrations to apply.“ + paperrust-Migrationen, idempotent; Django-angelegte DB wird als v3.4.2 erkannt; fehlende (`0027_documentbarcode`) / neuere (`0099_future`) Migration und fremde DB → klare CommandError |
| `test_users.sh` | createsuperuser `--no-input` (Ausgabe, DB-Zeile), Doppelname, manage_superuser (4 Fälle), changepassword (Validator, 3 Versuche) | **gleich**; upstream `authenticate()` akzeptiert die Rust-Hashes |
| `test_export.sh` | Rust- vs. upstream-Export derselben DB-Kopie: Standard, `-sm`, `--data-only -nt -na`, `--zip -zn`, `-f -p -sm` mit `PAPERLESS_FILENAME_FORMAT` | **0 Unterschiede** in 609 / 813 / 2 / 609 / 813 Dateien (Namen, Bytes; JSON CRLF-normalisiert); `-c -d` inkrementell entfernt Fremddateien; Fehlermeldungen wie upstream |
| `test_import.sh` | Ziel jeweils frische DB (Rust- bzw. Django-migriert) + leere Medien; Referenz = upstream importiert upstream-Export | Rust-Import upstream-Export, upstream-Import Rust-Export, ZIP in beide Richtungen, split (Referenz upstream-split), Passphrase in beide Richtungen, `--data-only`: **alle 75 Tabellen gleich** (ohne `documents_document.modified` = Importzeit), Sequenzen gleich, 607 Mediendateien gleich; ohne Passphrase → CommandError wie upstream |
| `test_commands.sh` | je eine Kopie für Rust und upstream mit denselben Schäden | sanity_checker (4 Meldungen, Zählungen gleich), prune_audit_logs, empty_trash (ohne auditlog-delete-Zeilen, Known difference 7), document_renamer (Vorlage), document_retagger `-c -T -t -s -f` (DB inkl. Audit-Log gleich, m2m-IDs ausgenommen): **gleich**; reindex (Sentinel byte-gleich), optimize, classifier, thumbnails `-d 1` laufen |
| `test_schema.sh` | eingebettete Schema-Antworten vs. laufendes Django (JSON/YAML, API 9/10), Swagger-HTML-Vorlage | **byte-gleich** |

Hinweise zu den Vergleichen: Unter Windows speichert upstream `str(Path)` mit `\` (Snapshot enthält solche
Dateinamen), Rust und upstream unter Linux `/` – die Tests normalisieren das. Die IDs von
`documents_document_tags` beim Retagger weichen ab: upstream fügt in CPython-`set`-Reihenfolge ein
(`add_nested_tags`), pl-consumer in Listenreihenfolge (Inhalt gleich; siehe CORE-REQUEST 6). Der Importer
bildet die `set`-Reihenfolge nach (`pyset.rs`), daher sind dort auch die Zwischentabellen-IDs gleich.
Upstream-`document_importer` bricht im Testaufbau ohne `DATA_DIR/index` beim abschließenden Reindex ab
(upstream legt das Verzeichnis beim Serverstart an); die DB ist dann bereits festgeschrieben.

Nicht geprüft: OpenAPI-Routen über einen laufenden pl-server (nur Handler-Logik per Unit-Test und
Byte-Vergleich der Körper), interaktive Prompts (`createsuperuser` ohne `--no-input`), MySQL/SQLite (nicht
unterstützt), `document_thumbnails` mit PDFium (im Testaufbau nicht geladen → Standardbild), Exporte mit
Share-Link-Bündeln (keine in den Referenzdaten), sehr große Bestände (Laufzeit/Speicher).
7. **Neue Abhängigkeiten (B6, B2/B3)**: `bzip2 = "=0.6.1"` (reines Rust), `xz2 = "=0.1.7"` (Feature
   `static`, liblzma wird mit cc gebaut), `zstd = "=0.13.4"` (schon in Cargo.lock), dazu
   `pl-documents-write.workspace` und optional `pl-ai` (Feature `ai`, Standard an). Bitte in
   `[workspace.dependencies]` übernehmen; in pl-server `pl-cli = { workspace = true,
   default-features = false }` und `ai = [..., "pl-cli/ai"]`, damit `--no-default-features` pl-ai
   wirklich weglässt.
8. **pl-matching**: `pub fn ratio(s1, s2, score_cutoff)` in `fuzz.rs` anbieten (LCS ist dort privat;
   `fuzzy.rs` hat bis dahin eine Kopie von `lcs_len`).
9. **Docker-Entrypoint**: Liste `MANAGE_COMMANDS` um `document_archiver document_consumer
   document_fuzzy_match document_llmindex check` erweitert (direkt als Containerbefehl aufrufbar);
   Verweise in `/usr/local/bin` (Dockerfile) bitte ergänzen.

## Ausfall-Befunde C6 (tests/chaos, 2026-11-18)

- **C6-1 PaperRust-Erweiterungen in Export/Import** (`export/extras.rs`): `document_exporter` schreibt
  zusätzlich `paperrust-extras.json` neben `manifest.json` (Ordner und ZIP): „Sonst“-Zweige und
  KI-Bedingungen der Workflows (`paperrust_workflow_else_action`, `paperrust_workflow_ext`),
  Workflow-Verlauf, nicht geheime Systemeinstellungen, Rollen, ASN-Bereiche/-Drucke, Lebensbereiche,
  Ordner der Ordneransicht (`paperrust_folder`, `paperrust_document_folder`, `paperrust_folder_grant`, seit 2026-11-22),
  Begriffs-Aliasse, erkannte Fristen, KI-Prompts/-Optionen/-Verlauf, Fragen-Verlauf, Scanner/Scan-Geräte
  und `paperless_mail_processedmail` (sichert upstream nicht – ohne sie holt der Mail-Abruf schon
  verarbeitete Mails erneut). `document_importer` spielt die Datei nach den upstream-Modellen in
  derselben Transaktion ein (Zeilen der enthaltenen Tabellen werden ersetzt, Sequenzen nachgestellt).
  paperless-ngx ignoriert die Datei beim Rückweg. Vorher gingen diese Daten bei Sicherung/Wiederherstellung
  verloren. **Nicht enthalten:** verschlüsselte Einstellungen (Schlüssel hängt am Zielsystem – nach der
  Wiederherstellung neu eingeben), Geräte-/MCP-Token, API-Token (`authtoken_token`, wie upstream),
  abgeleitete Zustände (Zähler, Freigabe-Index, Sync, Warteschlange, Sicherung, Schutz).
- **C6-3 `manage migrate` legte `pl_ai.0003` nicht an** (`paperrust_ai_prompt/_option/_job` entstanden erst
  beim Serverstart): jetzt in `paperrust_migrations()`; sonst verlor ein Import deren Inhalt.
- Wie upstream (keine Abweichung, nur dokumentiert): Zeitstempel im Manifest auf Millisekunden gekürzt
  (DjangoJSONEncoder), `modified` der Dokumente wird beim Import „jetzt“ (bulk_create mit auto_now),
  Sync-Identitäten (`uuid`, `rev`) der PaperRust-Tabellen entstehen beim Import neu.
- Nachweis: `python tests/chaos/chaos.py c6` – Vergleich aller Tabellen (normalisiert) und aller Dateien
  (SHA-256) nach Export→frische DB→Import für Ordner, Passphrase, geteiltes Manifest und ZIP
  stored/deflated/bzip2/lzma/zstd; Ergebnis `results/release/chaos/BERICHT.md`.
- **C2 `document_archiver` mit Exit-Code ≠ 0 bei Fehlschlägen** (bewusste Abweichung): scheitert ein
  Dokument (z. B. Datenbank kurz weg), endet der Befehl nach dem Lauf mit
  „N document(s) failed (see above); run the command again to retry them“ (upstream: immer 0).
