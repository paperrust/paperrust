# Vertrag pl-dbmigrate (`paperless-rs-migrate-db`)

Werkzeug für den verlustfreien Umzug einer paperless-ngx-**v3.4.2**-Datenbank (SQLite,
MariaDB/MySQL, PostgreSQL) in eine **leere PostgreSQL-Datenbank** mit exakt dem Schema, das
Django v3.4.2 auf PostgreSQL erzeugt. Keine HTTP-Routen. Nutzeranleitung: [`docs/MIGRATION.md`](../MIGRATION.md).

Upstream-Quellen (v3.4.2, unverändert):
- Schema: `src/documents/migrations`, `src/paperless/migrations`, `src/paperless_mail/migrations`
  plus Django-/Drittanbieter-Apps (auth, contenttypes, sessions, admin, authtoken, guardian,
  auditlog, account, socialaccount, mfa) – ermittelt durch echtes `manage.py migrate` (siehe unten).
- DB-Konfiguration: `src/paperless/settings/custom.py::parse_db_settings` (SQLite =
  `DATA_DIR/db.sqlite3` mit WAL; MariaDB `utf8mb4`/`utf8mb4_unicode_ci`).
- `src/documents/migrations/0020_drop_celery_results.py` (löscht `django_celery_results_*`
  und deren `django_migrations`-Zeilen → in v3.4.2 nicht mehr vorhanden).
- Django 5.2 Feldspeicherung je Backend (`django/db/backends/{sqlite3,mysql,postgresql}/operations.py`):
  SQLite/MySQL speichern `DateTimeField` naiv in UTC (USE_TZ=True), bool als 0/1 bzw. tinyint(1),
  `JSONField` als Text (MariaDB: LONGTEXT + `json_valid`), `UUIDField` als char(32),
  `GenericIPAddressField` als char(39) (PostgreSQL: `inet`).
- Exporter-Vergleich: `src/documents/management/commands/document_exporter.py`
  (`manifest_key_to_object_query`), `document_importer.py`, `docs/administration.md`.

## Aufruf

```
paperless-rs-migrate-db --source <URL> --target <URL> [--dry-run | --verify-only]
    [--report <datei.json>] [--nul fail|replace|strip] [--invalid-utf8 fail|replace]
    [--skip-unknown-tables] [--batch-bytes N] [--max-issues N] [--quiet]
```

| Option | Wirkung |
|---|---|
| `--source` / `PAPERLESS_MIGRATE_SOURCE` | `sqlite:///pfad/db.sqlite3`, `mysql://…`, `mariadb://…`, `postgres://…` |
| `--target` / `PAPERLESS_MIGRATE_TARGET` | `postgres://…` (Pflicht außer bei `--dry-run`) |
| `--dry-run` | Vorprüfung + vollständiger Lese-/Umwandlungsdurchlauf, Zeilen/Größen; schreibt nichts |
| `--verify-only` | Quelle vs. befülltes Ziel (beide nur lesend, je ein Schnappschuss) |
| `--report F` | JSON-Bericht nach `F`, Text nach `F.txt` (Text zusätzlich auf stdout) |
| `--nul` | U+0000 in Text/JSON: `fail` (Standard) / `replace` (→ U+FFFD) / `strip`; jede Zelle gemeldet |
| `--invalid-utf8` | `fail` (Standard) / `replace` (→ U+FFFD), gemeldet |
| `--skip-unknown-tables` | Tabellen außerhalb des Schemas bewusst weglassen (sonst Abbruch) |
| `--batch-bytes` | COPY-Puffer, Standard 4 MiB (64 KiB … 256 MiB) |

Exit-Code: 0 Erfolg, 1 Fehler/Probleme, 2 Aufruffehler, 130 Strg+C. Passwörter werden in Bericht
und Ausgabe durch `***` ersetzt.

## Ablauf und Garantien

1. **Quelle öffnen, nur lesend, ein Schnappschuss.** SQLite: `SQLITE_OPEN_READONLY`,
   `PRAGMA query_only=1`, `BEGIN` (WAL wird mitgelesen). MySQL/MariaDB:
   `START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY` (REPEATABLE READ), `time_zone='+00:00'`,
   `utf8mb4`. PostgreSQL: `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY`, `TimeZone=UTC`.
   Die Transaktion wird nie festgeschrieben (ROLLBACK am Ende).
2. **Vorprüfung** (`migrate.rs::precheck_source`):
   - `django_migrations` (siehe „Migrationsstand“) – Abweichung → Abbruch mit Hinweis
     „erst paperless-ngx auf 3.4.2 aktualisieren und einmal starten“.
   - Tabellenmenge = Schema (75 Tabellen). Fehlende → Abbruch; zusätzliche → Abbruch, außer
     `--skip-unknown-tables` (dann Warnung mit Namen). SQLite-interne `sqlite_*` zählen nicht.
   - Spalten je Tabelle = Schema (berechnete Spalten ausgenommen).
   - SQLite: `PRAGMA foreign_key_check` (Problem, wenn Verletzungen).
3. **Ziel**: UTF8 nötig, sonst Abbruch; < PostgreSQL 14 → Warnung; nicht leer (irgendeine Relation
   in `public`) → Abbruch. Danach **eine Transaktion**:
   1. `schema/v3.4.2-postgres-pre.sql` (Tabellen, CHECKs, Identity-Sequenzen, berechnete Spalten),
      Strukturvergleich `information_schema.columns` ↔ `v3.4.2-columns.tsv`.
   2. je Tabelle `COPY public."t" (Spalten ohne GENERATED) FROM STDIN` (Textformat), Zeilen
      gestreamt, Puffer `--batch-bytes`; jede Zelle über `value::convert` (Typ aus dem Zielschema).
      Umwandlungsfehler → Tabelle zu Ende lesen (alle Befunde sammeln), COPY abbrechen, Abbruch.
   3. Sequenzen: `setval(seq, coalesce(max(col),0)+1, false)` für jede Identity-/Serial-Spalte.
   4. `schema/v3.4.2-postgres-post.sql` (PK, UNIQUE, FK `DEFERRABLE INITIALLY DEFERRED`, Indizes,
      partielle Unique-Indizes) – PostgreSQL prüft dabei alle Zeilen; `ANALYZE`.
   5. Prüfung (unten); nur wenn fehlerfrei → `COMMIT`, sonst `ROLLBACK`.
   Abbruch/Absturz/Verbindungsverlust an beliebiger Stelle → PostgreSQL rollt zurück, Ziel leer.
   „Ziel als ungültig markieren“ ist damit „Ziel bleibt leer“ (Exit 1, `result=failed`); ein
   halbfertiges Ziel kann es nicht geben. `--verify-only` schreibt nichts und meldet `verify-failed`.
4. **Prüfung** (`migrate.rs::verify_target`): je Tabelle `count(*)` Quelle = Ziel, Prüfsumme
   Quelle (aus den umgewandelten Werten beim Lesen) = Prüfsumme Ziel (Ziel **neu gelesen** per
   `col::text`, erneut kanonisiert); bei Abweichung Diagnose mit bis zu 20 abweichenden PKs;
   jede Sequenz: nächster Wert > max(id); alle Constraint- und Indexnamen aus post-data vorhanden,
   keiner `NOT convalidated`, kein Index `NOT indisvalid`. Die Prüfsummen über alle Zeilen ersetzen
   Stichproben (jede Zeile, jede Spalte geprüft); die API-Endprobe (unten) ist die semantische Stichprobe.

### Migrationsstand (`schema.rs::check_migrations`)

- erwartet = `django_migrations` einer frisch migrierten v3.4.2-DB (247 Zeilen, inkl. der von
  Django mitgeschriebenen ersetzten Namen gesquashter Migrationen); **alle** müssen in der Quelle sein.
- zusätzliche Zeilen sind erlaubt (und werden mitkopiert), wenn sie (a) laut `MigrationLoader`
  durch eine angewandte gesquashte Migration ersetzt sind (`v3.4.2-replaced.tsv`, 138), (b) aus der
  git-Historie der paperless-Apps bis v3.4.2 stammen (`v3.4.2-historical.tsv`, 250; ältere,
  schrittweise aktualisierte Installationen) oder (c) zu einer App gehören, die v3.4.2 nicht
  installiert (z. B. `django_q`; Warnung). Genau diese Zeilen ignoriert Django ebenfalls.
- (d) bekannte Migrationen einer neueren paperless-ngx-Version (`schema::KNOWN_NEWER`:
  `documents.0028_replace_tag_tree_fields`, `0029_alter_workflowrun_type_alter_workflowtrigger_type`,
  `0030_note_format` – paperless-ngx 3.5.0/3.5.1) → `MigrationCheck.newer`, angenommen (siehe „Neuer als 3.4.2“).
- alles andere (unbekannte Migration einer installierten App) → Abbruch mit Export-Weg
  (`catchup::newer_message`).

### Typumwandlung (`value.rs`)

Zielart aus `udt_name`; Ausgabe ist die kanonische PostgreSQL-Textform (zugleich Prüfsummen-Eingabe).

| Ziel (udt) | SQLite (Speicherklasse) | MySQL/MariaDB (SELECT-Ausdruck) | Kanonisch |
|---|---|---|---|
| int2/int4/int8 | INTEGER (REAL ganzzahlig, TEXT-Zahl akzeptiert) | `CAST(c AS CHAR)` | Dezimal |
| float4/float8 | REAL / INTEGER | Binär `f64` (exakt) | kürzeste exakte Darstellung, `NaN`/`±Infinity` |
| bool | INTEGER 0/1 (TEXT `0/1/t/f/true/false`) | `CAST(c AS CHAR)` | `t`/`f`; andere Werte → Fehler |
| varchar(n)/text | TEXT (Bytes → UTF-8-Prüfung) | Bytes (utf8mb4) | unverändert; Länge > n Zeichen → Fehler; U+0000 → Policy |
| timestamptz | TEXT `YYYY-MM-DD HH:MM:SS[.f]` naiv = UTC; `T`, `Z`, `±HH:MM` akzeptiert (→ UTC) | `DATE_FORMAT(c,'%Y-%m-%d %H:%i:%s.%f')` naiv = UTC | `YYYY-MM-DD HH:MM:SS.ffffff+00`; > µs-Genauigkeit → Fehler |
| date | TEXT `YYYY-MM-DD` (mit Uhrzeit → Fehler, Django läse dort None) | `DATE_FORMAT(c,'%Y-%m-%d')` | `YYYY-MM-DD` |
| jsonb | TEXT → JSON-Parser | Bytes | Schlüssel rekursiv sortiert, kompakt; `\u0000` → Policy; NaN/ungültig → Fehler |
| uuid | TEXT 32 Hex / mit Bindestrichen, BLOB(16) | Bytes | klein, mit Bindestrichen |
| inet | TEXT | Bytes | `std::net::IpAddr`-Form, `/32` bzw. `/128` entfällt |
| numeric | (nur berechnete Spalte, nicht kopiert) | – | ohne überflüssige Nullen |
| bytea | BLOB | `HEX(c)` | `\x` + Hex (im Schema v3.4.2 nicht vorhanden) |

NULL in einer NOT-NULL-Spalte → Fehler (vor PostgreSQL gemeldet). PostgreSQL-Quelle: `c::text`.
Berechnete Spalten (`documents_document.content_length`, `documents_customfieldinstance.value_monetary_amount`)
werden nicht kopiert (PostgreSQL verbietet Werte für `GENERATED ALWAYS`) und nicht verglichen.

### Prüfsumme (`hash.rs`)

Zeile: SHA-256 über alle kopierten Spalten in Schema-Reihenfolge (NULL = `0x00`, Wert = `0x01` +
Länge u64 LE + UTF-8 der kanonischen Form). Tabelle: SHA-256(XOR aller Zeilen-Hashes ‖ Summe der
ersten 16 Bytes mod 2^128 ‖ Zeilenzahl) – **reihenfolgeunabhängig**, weil Engines Text-PKs
(`django_session.session_key`) verschieden sortieren.

### Bericht (`report.rs`)

JSON: `tool`, `tool_version`, `mode`, `result` (`migrated|verified|dry-run-ok|dry-run-problems|verify-failed|failed|aborted`),
`ok`, `error`, `schema_version`, `source{url,dialect,version}`, `target{url,version}`, `started`,
`finished`, `duration_s`, `policies`, `migrations_replaced_rows`, `skipped_tables`, `totals{…}`,
`problems[]`, `warnings[]`, `tables[]{table, source_rows, source_bytes_approx, copied_rows,
target_rows, source_checksum, target_checksum, verified, conversion_errors, values_changed_by_policy,
issues[]{pk,column,fatal,message}, differing_rows[]}`. Text: Tabelle je DB-Tabelle + Befunde + Ergebnis.

## Umstieg mit Prüfbericht: `switch` / `verify` / `detect` (Paket Z1)

Module `detect.rs` (Quellart: `export-dir`, `export-zip`, `sqlite-folder`, `database-url`; Medienordner
neben `data/`; Ziel aus `PAPERLESS_DB*`; gleiche DB), `export.rs` (Manifest **gestreamt** inkl.
`*-manifest.json`, `metadata.json`-Version, Dateien aus Ordner oder ZIP), `zipread.rs` (nur lesend,
stored/deflated/ZIP64, Entpackgrenze = angegebene Größe), `verify.rs` (Zahlen je Objektart,
Dokumentzeilen-Abgleich, Prüfsumme jeder Original-/Archivdatei – 32 Zeichen MD5, sonst SHA-256 –,
Vorschaubilder, verwaiste Dateien, Urteil, JSON/Text/HTML), `api.rs` (Stichprobe über API v1:
Titel, Inhalts-Hash, Tags, Download des Originals; Wurzel-Dokumente, Inhalt/Datei der neuesten
Version wie upstream `versions_newest_first`; `--api-temp-token` legt höchstens einen Token an und
löscht ihn), `switch.rs` (Ablauf, Export-Weg über `paperrust manage migrate` + `document_importer`
mit `PAPERLESS_DB*` aus der Ziel-URL). Urteil: FAIL bei abweichender Anzahl/Dokumentzeile/Datei
nur im Ziel/Stichprobe/Schema ≠ 3.4.2; WARN bei Dateifehlern, die schon in der Quelle bestehen,
fehlenden Vorschaubildern, verwaisten Dateien, Export anderer Version, Stichprobe nicht möglich.
Exit 0/3/1. JSON: `verdict`, `source{kind,label,location,paperless_version,media}`, `target`,
`steps[]`, `counts[]{object,label,source,target,level}`, `documents_compared`,
`files{source,target}{…,problem_list[]{document,kind,path,issue,expected,actual}}`,
`spot_check{status,api_url,documents[]}`, `findings[]{level,area,message}`.
E2E: `bash tests/dbmigrate/run_switch.sh`.

## Ältere Quellen nachholen (v3.0.0 … v3.4.1, `catchup.rs`, 2026-11-10)

upstream: `src/documents/migrations/0023…0027`, `src/paperless/migrations/0014…0018` @v3.4.2;
SQL in `schema/catchup-v3.0.0-v3.4.2.sql` (Django `schema_editor(collect_sql=True)` aus dem
Referenz-venv, RunPython `paperless.0016` von Hand: `ai_enabled = false → NULL`). Fremd-Apps haben
seit v3.0.0 keine neuen Migrationen (uv.lock v3.0.0…v3.4.2, Dateidaten).

- `check_migrations` fehlend ⊆ Nachhol-Schritte und nichts unbekannt → nachholbar; sonst Abbruch
  wie bisher (2.x: „zu alt“, neuer als 3.4.2: „neuer“, je mit Hinweis auf den Export-Weg).
- **Kopie** (`--source` älter): Quelle unverändert; je Zieltabelle `Adapt` (gelesene Spalten,
  feste Werte für neue Spalten, `FalseToNull`, neue Tabellen leer, zusätzliche
  `django_migrations`-Zeilen mit id ab max+1 und `applied` = max(applied) der Quelle →
  deterministisch für Probelauf/Umzug/`--verify-only`). Prüfsumme gegen die erwarteten Werte.
  Bericht: `catchup_migrations`, Warnung mit Liste.
- **An Ort und Stelle** (`switch`, Quelle = Ziel): Sicherung (`pg_dump -Fc` über `PAPERRUST_PG_BIN`,
  `PAPERRUST_DESKTOP_PG_BIN`, neben dem Programm, PATH; sonst `CREATE DATABASE <db>_vor_umstieg_<zeit>
  TEMPLATE <db>`; sonst Abbruch ohne Änderung, `--no-backup` bewusst), dann SQL + Zeilen in einer
  Transaktion. pl-cli `manage migrate` holt beim Start ebenfalls nach (ohne Sicherung, wie Django).
- `switch` neu: Schritte „Verbindung“ (Ziel erreichbar), „Platz“ (Dateien passen in den Ziel-
  Medienordner, +5 % +64 MiB, sonst FAIL vor jeder Änderung), „Sicherung“, Fortschritt beim
  Tabellenkopieren und Dateikopieren, „Kurzfassung“ am Ende des Textberichts.
- post-data: mfa-Index mit Djangos Ausdruck (`schema::post_data`), Ergebnis-Schema jetzt
  byte-gleich mit upstream `migrate` (vorher semantisch gleich, Textform anders).
- Tests: `catchup::tests` (Unit), `tests/dbmigrate/run_catchup.sh` (E2E gegen echte 3.1.3-DB aus
  `make_v313_fixture.sh`: Kopie, gleiche DB mit pg_dump und mit Server-Kopie, Rückspielen, Export),
  `crates/pl-cli/tests/catchup_db.rs` (`#[ignore]`, braucht Test-DB).

## paperless-ngx 2.20 direkt (`catchup.rs`, `schema::v220`, 2026-11-13)

upstream: `src/{documents,paperless,paperless_mail}/migrations` @v3.4.2 (genau der Plan von
`manage.py migrate --plan` auf einer 2.20.15-DB: documents 0003–0027, paperless 0005–0018,
paperless_mail 0002–0004), `src/documents/migrations/1075_workflowaction_order.py` @v2.20.15,
`src/paperless/checks.py` (v3 nur von 2.20.15), `docs/migration-v3.md` (Entschlüsseln vorher),
Django `MigrationExecutor.check_replacements` (Squash-Zeilen am Ende), `MigrationLoader`
(Squash gilt als angewandt, wenn alle ersetzten angewandt sind).

- SQL: `schema/catchup-v2.20-v3.0.sql` (33 Schritte, erzeugt mit `tests/dbmigrate/gen_catchup_v2.py`
  = `schema_editor(collect_sql=True)` wie `sqlmigrate`, aber mit aufgelösten Squashes), dann
  `catchup-v3.0.0-v3.4.2.sql`. Introspektionsabhängig (Django sucht Constraint-Namen zur Laufzeit):
  `0005` löscht die Unique-Constraints auf `checksum` per DO-Block über den Katalog.
- RunPython von Hand: `1075` order = id; `0007` ContentType + vier Rechte `sharelinkbundle` (Reihenfolge
  add/change/delete/view), an Benutzer und Gruppen mit direktem `add_document`; `0014` UiSettings je
  Benutzer (get_or_create), `saved_views.{dashboard,sidebar}_views_visible_ids` nur wenn fehlend/null,
  IDs sortiert nach `name` (SavedView.Meta.ordering); `0018` Regeltyp 0→48, 19→49; `paperless.0007`
  Kappen > 32767; `paperless.0008` mode/skip_archive_file → mode/archive_file_generation;
  `paperless_mail.0002` maximum_age kappen, order nur bei Überlauf neu nummerieren.
  In Rust (`-- @rust`): `sha256_checksums` (0016: jede Original-/Archivdatei, auch Papierkorb,
  `filename` leer → `{pk:07}.pdf`, fehlende Datei → Prüfsumme bleibt + Hinweis; ohne Medienordner →
  alle bleiben MD5 + Hinweis) und `fulltext_query_prefixes` (0017: `(?<![.\w])note:` wie Python `re`,
  `\w` = Unicode-alphanumerisch oder `_`).
- `catchup::plan`: fehlende Zeilen ⊆ bekannte Schritte ∪ Squash-Namen (`*squashed*`, laut
  `v3.4.2-replaced.tsv`) → nachholbar (`Plan.marks` = Squash-Zeilen); 2.x erkannt an
  `documents.0003_remove_document_storage_type`; `1075` ohne 2.x-Umbau → Fehler. Vorprüfung
  `precheck_v2`: `storage_type <> 'unencrypted'` → Abbruch („verschlüsselt“).
- **an Ort und Stelle** (`apply_in_place(conn, plan, media)`): eine Transaktion (bzw. Savepoint),
  `SET CONSTRAINTS ALL IMMEDIATE` (statt `atomic = False`), Schritte + Hooks, `django_migrations`-Zeile
  je Schritt (`now()`), Squash-Zeilen zuletzt. `switch` übergibt `--media` der Quelle.
- **Kopie** (`migrate.rs`, `Shape::v220`): Ziel bekommt das 2.20-Schema (`schema/v2.20-postgres-*.sql`,
  `v2.20-columns.tsv`, aus einer frisch migrierten 2.20.15-DB, ohne `django_celery_results_*`),
  Quelle muss alle `v2.20-migrations.tsv`-Zeilen haben (außer `1075`), Tabellen
  `django_celery_results_*` werden nicht kopiert (Warnung), sonst wie immer (COPY, Prüfsummen,
  Sequenzen, post-data, Prüfung); danach im selben Ziel-Transaktion `apply_in_place` (Medienordner =
  `--media`), Spaltenprüfung gegen `v3.4.2-columns.tsv` und Gültigkeit aller Constraints/Indizes,
  dann COMMIT. 1075 fehlt: `documents_workflowaction.order` = `id` beim Lesen. `--verify-only` mit
  2.x-Quelle → Abbruch mit Hinweis auf `verify`.
- `verify.rs`: `db_docs` liest `root_document_id`/`version_index` nur, wenn vorhanden (2.x);
  `same_checksum`: MD5 (32) in der Quelle gegen SHA-256 (64) im Ziel ist kein Unterschied (die
  Dateiprüfung des Ziels prüft die SHA-256).
- pl-cli `manage migrate`: 2.x wird nicht beim Start umgebaut; Meldung verweist auf `paperrust migrate`.

## Neuer als 3.4.2 (`schema::KNOWN_NEWER`)

- gleiche Datenbank: `switch` → Schema „neuer als 3.4.2“, Hinweis, nichts zu tun; pl-cli
  `classify` zählt die Zeilen als „extra“ (Start wie bei 3.4.2, `pl_objects.0001` ist idempotent).
- Kopie: `Shape::v342(drop_tag_tree, note_format)` – nach pre-data (`Shape::newer_ddl`)
  `ALTER TABLE documents_tag DROP COLUMN` der 13 treenode-Spalten (`schema::TREENODE_DERIVED`),
  Spaltenliste ohne sie; with documents.0030 (paperless-ngx 3.5) `ALTER TABLE documents_note ADD
  COLUMN "format" varchar(16) DEFAULT 'plain' NOT NULL` + `DROP DEFAULT` (exactly Django's SQL; the
  column is appended to the copy column list, `schema::note_format_column`). Daten und
  `django_migrations` unverändert. The expected 3.5.1 schema is the v3.4.2 snapshot + 0028 + 0030
  (`migrate::shape_tests`); no separate v3.5.1 snapshot file.
- PaperRust's own `pl_documents_write.0001_paperrust_note_format` (on `manage migrate`) adds the
  column on 3.4.2-and-older databases and sets the default `'plain'` where 3.5 left none.
- unbekannt: `switch` FAIL („neueren paperless-ngx-Version … Export“), Ziel leer bzw. unverändert.

## Desktop: Abgleich im Erst-Start (apps/desktop, apps/desktop-shell)

- `verify`-Bericht (`umstieg-bericht.json`) hat zusätzlich `summary` (Kurzfassung als Zeilen).
- `apps/desktop` `supervisor::SwitchCheck {verdict: RUNNING|OK|WARN|FAIL, summary, report}`,
  `Event::SwitchCheck`: `RUNNING` vor `Ready` (wenn `paperrust-migrate-db` beiliegt), Ergebnis nach dem
  Hintergrund-`verify` (liest JSON, Rückfall Exit-Code 0/3/sonst, plus bis zu drei WARN/FAIL-Befunde).
  Alte `umstieg-bericht.*` werden vorher gelöscht. `migrate_db` gibt `--media` weiter (SHA-256 bei 2.x).
- Hülle: `View.switch {verdict, summary, has_report}`; die Seite zeigt nach dem Umstieg Ergebnis,
  Kurzfassung, „Ganzen Bericht öffnen“ (`open_log("umstieg-bericht.html")`, in `KNOWN_LOGS`) und
  „PaperRust öffnen“ (bei RUNNING „Schon öffnen“), statt sofort zur Oberfläche zu wechseln.

## Test mit einer echten Datenbank (vorbereitet, offen)

`tests/dbmigrate/real_db_check.sh <dump> [<media>]`: zurückspielen in `pl_mig_real_src`, Geheimnisse
löschen (Mail-Passwörter/Refresh-Token, Passwort-Hashes, Social-/API-Token, Sitzungen, MFA-Daten,
Webhook-Kopfzeilen/-Inhalte, Workflow-Passwörter, KI-/OCR-Schlüssel, Freigabe-Slugs, Geräte-Token),
dann `detect`, Probelauf, Umstieg in neue DB, Umstieg an Ort und Stelle (`--no-backup`, zweite Kopie);
Ausgabe nur Zahlen (`zahlen.txt`), Berichte privat in `.work/real`; Wegwerf-DBs werden gelöscht.
Geprüft mit dem 2.20-Fixture als Stellvertreter; der echte Bestand fehlt (Besitzer).

## Eingebettetes Schema (`crates/pl-dbmigrate/schema/`)

Erzeugt mit `bash tests/dbmigrate/gen_schema.sh`: eigene DB `pl_dbmigrate_schema` auf dem
Referenz-PostgreSQL 18.6 (127.0.0.1:54329), upstream `manage.py migrate` (DJANGO_SETTINGS_MODULE
`paperless.settings`), `pg_dump --schema-only --no-owner --no-privileges --no-comments
--section=pre-data|post-data`, bereinigt um `SET`, `set_config('search_path', …)`, `\restrict`
(pg_dump 18) und Kommentare. Danach DB wieder gelöscht. Dateien: `v3.4.2-postgres-pre.sql`
(75 Tabellen, 73 Identity-Sequenzen), `v3.4.2-postgres-post.sql` (261 Constraints, 195 Indizes),
`v3.4.2-columns.tsv`, `v3.4.2-migrations.tsv`, `v3.4.2-replaced.tsv`, `v3.4.2-historical.tsv`.
pg_dump 18 schreibt benannte NOT-NULL-Constraints (`col int CONSTRAINT x NOT NULL`); diese Syntax
akzeptiert PostgreSQL seit jeher (Name wird vor 18 ignoriert) – auf PG 14–17 aber **nicht getestet**.
`reference/schema/postgres.sql` des Referenz-Agenten ist dasselbe Django-Schema (dort mit
`--restrict-key`), wird aber nicht eingebettet, damit das Binary von der Referenzumgebung unabhängig ist.

## Tests

Unit-Tests (`cargo test -p pl-dbmigrate -j 2`, ohne DB): 13 Tests – Umwandlung aller Typen inkl.
Zeitstempelformate, JSON-Normalisierung (`-0.0`, `1e2`, große Zahlen), NUL/UTF-8-Policies,
varchar-Länge in Zeichen, COPY-Escaping, Prüfsumme reihenfolgeunabhängig, Migrationsstand-Logik
(fehlend/unbekannt/historisch/fremde App), eingebettetes Schema parsebar, CLI. `cargo clippy
-p pl-dbmigrate --all-targets -- -D warnings` sauber.

Ende-zu-Ende (`tests/dbmigrate/`, Ziel-DBs `pl_dbmigrate_test_*` auf 127.0.0.1:54329, nie `paperless`):

| Skript | Quelle | Ergebnis (2026-11-08, Debug-Build) |
|---|---|---|
| `run_sqlite.sh` | upstream `migrate` auf SQLite + `reference/seed.py` + `seed_edge.py` (Unicode/Emoji/RTL/Steuerzeichen, 3,3-MB-`content`, µs-Zeitstempel, IPv4/IPv6 im Audit-Log, Sitzungen, alle Custom-Field-Typen inkl. float-Grenzwerte, JSON-Sonderfälle) | 75 Tabellen, 2 819 Zeilen: dry-run ok, Umzug ok (19 s), verify-only ok, 2. Umzug auf befülltes Ziel bricht ab, `db.sqlite3`+`-wal` byte-gleich |
| `run_sqlite.sh` NUL-Fall | Kopie mit U+0000 in `documents_note.note`, `documents_tag.name` | Standard: Abbruch, Ziel leer, dry-run meldet beide Zellen; `--nul replace`: Umzug ok, beide Zellen als GEÄNDERT im Bericht, Wert `vor�nach` |
| `run_sqlite.sh` Masse (`WORK_NAME=bulk EDGE_BULK=100000`) | wie oben + 100 000 Dokumente à ~2,2 KB | 100 211 Dokumente, 102 819 Zeilen, Quelle ~403 MiB: Umzug + Prüfung **150–159 s, Spitzen-RAM des Werkzeugs 43 MB** (kleiner Test: 42 MB → Speicher unabhängig von der Datenmenge), verify-only ok |
| `run_mariadb.sh` | offizielle portable MariaDB 11.4.8 (ZIP, SHA-256 geprüft) nach `tests/dbmigrate/.mariadb`, `mysqlclient` 2.3.0 isoliert in `.work/pydeps`; upstream `migrate` (utf8mb4_unicode_ci) + beide Seeds; Werkzeug liest mit **reinem SELECT-Nutzer** | 75 Tabellen, 2 819 Zeilen: dry-run, Umzug (38 s), verify-only ok; `CHECKSUM TABLE` aller Quelltabellen vorher = nachher |
| `run_extra.sh` | [1] PostgreSQL → PostgreSQL (Quelle = Ziel des SQLite-Tests); [2] upstream `manage.py migrate --check` auf dem Ziel; [3] `taskkill /F` des Werkzeugs nach 25 s mitten im Massen-COPY; [4] Quelle mit fehlender (`0027_documentbarcode`) bzw. neuerer (`0028_future`) Migration | [1] Umzug + verify-only ok; [2] keine offenen Migrationen; [3] Ziel danach 0 Relationen; [4] beide brechen mit Hinweis „zuerst paperless-ngx auf 3.4.2 …“ und Liste ab |
| `api_compare.sh sqlite` / `mariadb` | upstream paperless-ngx v3.4.2 (granian, unverändert) nacheinander auf Quelle (:8101) und Ziel (:8102), gleiches DATA_DIR/MEDIA_ROOT, Suchindex einmal aus der Quelle gebaut; 145 GET-Antworten je Lauf (admin/alice/bob: Listen, Details, Notizen, Historie, Metadaten, Suche, Autocomplete, Statistik, Profil, UI-Settings, Mail, Workflows, Tasks, Trash …) | **0 unerwartete Unterschiede**. SQLite: 122 identisch, 17 nur Reihenfolge, 6 backend-abhängig; MariaDB: 120 / 19 / 6 (siehe Known differences). Suche/Autocomplete identisch → Index von der DB-Engine unabhängig |

| `run_v2_newer.sh` (2026-11-13, Debug-Build) | `make_v220_fixture.sh`: echte 2.20.15-DB (upstream-Code v2.20.15, `seed_v220.py`: jede Datenmigration abgedeckt) auf PostgreSQL und SQLite, 2.20.4-Variante, upstream-3.4.2-`migrate` als Erwartungswert, upstream `dev` (0028/0029) auf dem 3.4.2-Schnappschuss | **A** 2.20.15 gleiche DB: Sicherung zurückspielbar, Schema = upstream, `compare_db.py` 75 Tabellen, 0 Abweichungen; **B** 2.20.4: 0 Abweichungen zum upstream-Weg 2.20.15 → 3.4.2; **C** Kopie PG → neue DB: Quelle unverändert, 0 Abweichungen; **D** SQLite → PG: Schema = upstream, umgewandelte Werte wie upstream; **E** Probelauf/detect, GPG-Dokument → Abbruch ohne Änderung; **N1** dev gleiche DB + Kopie (Kopie = Quelle, 0 Abweichungen); **N2** unbekannte Migration → FAIL mit Export-Weg, Ziel leer |

Der Paritäts-Harness `tests/parity/harness` wurde nicht benutzt (vergleicht Django ↔ paperless-rs
mit Nutzerkatalog und Reset-Befehl der Referenz); `api_fetch.py`/`api_compare.py` sind ein kleiner,
eigenständiger Vergleich (stdlib) mit Normalisierung „nur Reihenfolge“.

Nicht getestet: **MySQL** (nur MariaDB; MySQL 8 hat nativen JSON-Typ, der Pfad liest ihn als
Bytes – ungeprüft), Strg+C (nur harter Abbruch per taskkill geprüft), Ziel-PostgreSQL
14–17, SQLite-Datei auf nur lesbarem Datenträger, Klassifikator-Datei (nicht trainiert),
KI-Vektorindex.

## Known differences

00. **Export aus paperless-ngx 1.x wird vorab abgelehnt** (arm64-Prüfung B2, 2026-11-19): Version
    `1.*`/`0.*` in `metadata.json` oder (ohne Version) Feld `storage_type` im Manifest → Schritt FAIL
    „Version“, nichts verändert, Hinweis auf den Weg über ngx 2.20. Upstream warnt nur („Version
    mismatch“) und bricht dann im Import ab (`document has no field named 'storage_type'`). Test
    `export::tests::detects_v1_exports`.

0. **2.x-Umbau**: Zeilen-IDs in `auth_permission`, `django_content_type`, den Rechte-Zuordnungen und
   `documents_uisettings` können von upstream abweichen (Einfügereihenfolge); Inhalt gleich
   (E2E vergleicht über natürliche Schlüssel). ContentTypes/Rechte, die Django erst per post_migrate
   anlegt (`documentbarcode`, `view_global_statistics`, `view_system_monitoring`), entstehen bei
   PaperRust beim ersten Start (`paperrust manage migrate`), nicht im Umstieg. 2.19.6/2.20.0–2.20.4
   übernimmt PaperRust direkt, upstream 3.4.2 verweigert sie (paperless.E002); Ergebnis = upstream-Weg
   über 2.20.15. `\w` vor `note:` folgt Rusts `char::is_alphanumeric` statt Pythons `str.isalnum`
   (Unterschied nur bei seltenen Unicode-Zeichen wie kombinierenden Zeichen).

1. **jsonb normalisiert JSON**: Schlüsselreihenfolge, doppelte Schlüssel (letzter gewinnt),
   Zahlen als numeric (`1e2` → `100`, `-0.0` → `0.0`). Die Prüfsumme vergleicht deshalb eine
   kanonische Form (sortierte Schlüssel, Zahlen über f64, ±0 gleich). Inhaltlich gleich; das
   Vorzeichen einer negativen Null in JSON geht verloren (gilt für jede paperless-ngx-PostgreSQL-DB).
2. **Backend-Verhalten nach dem Umzug** (kein Datenverlust; paperless-ngx verhält sich auf
   PostgreSQL genauso): Textsortierung nach PostgreSQL-Kollation; `NULL` bei ASC zuletzt (SQLite/
   MariaDB zuerst) → andere Seiteninhalte bei `ordering=archive_serial_number`; `icontains` mit
   Umlauten (SQLite: nur ASCII case-insensitiv, MariaDB `_ci` zusätzlich akzentunabhängig, PG:
   case-insensitiv); Reihenfolge bei Gleichstand (z. B. Custom-Field-Instanzen mit gleichem
   `created`, Rechtelisten ohne ORDER BY) unbestimmt. In der API-Endprobe als „nur Reihenfolge“ bzw.
   „backend-abhängig“ ausgewiesen.
3. `/api/config/` → `externally_configured_variables` enthält auf dem Ziel zusätzlich die
   `PAPERLESS_DB*`-Variablen (Konfiguration, keine Daten).
4. SQLite: Auch ein rein lesender Zugriff aktualisiert bei WAL-Datenbanken `db.sqlite3-shm`
   (WAL-Index im gemeinsamen Speicher, keine Daten). `db.sqlite3` und `db.sqlite3-wal` bleiben
   byte-gleich (geprüft). Bewusst kein `immutable=1`: das würde Sperren und WAL ignorieren.
5. Berechnete Spalten werden von PostgreSQL neu berechnet statt kopiert (SQLite/MariaDB könnten
   bei `value_monetary_amount` wegen anderer Regex-/Cast-Semantik abweichende Werte gespeichert
   haben – auf PostgreSQL gilt dann der PostgreSQL-Wert, wie bei upstream).

6. **Suchindex nach dem Umstieg neu aufbauen** (2026-11-21): nach erfolgreichem `switch` (Ergebnis nicht FAIL) legt das Werkzeug die Rebuild-Markierung `.rebuilding` in den Indexordner (`PAPERRUST_/PAPERLESS_INDEX_DIR`, sonst `<DATA_DIR>/index`, sonst Docker-Standard), sofern er auf diesem Rechner existiert; der nächste Start baut den Index aus der neuen Datenbank auf.

## CORE-REQUEST

- Keine Änderung an pl-core/pl-authz/pl-server nötig.
- Abhängigkeiten: nur Workspace-Dependencies (`tokio`, `sqlx`, `serde_json`, `anyhow`, `futures`,
  `sha2`, `chrono`, `url`), dazu in `crates/pl-dbmigrate/Cargo.toml` die **sqlx-Features
  `sqlite` (gebündeltes libsqlite3) und `mysql`** zusätzlich zum Workspace. `Cargo.lock` erhält
  dadurch `libsqlite3-sys`, `sqlx-sqlite`, `sqlx-mysql` (+ `rsa` u. a.). Nur dieses Crate aktiviert sie;
  andere `cargo check -p <crate>` sind nicht betroffen. Workspace-`Cargo.toml` unverändert.

## Offene Punkte / Risiken

- MySQL 8 (nicht MariaDB) ungetestet (Pfad implementiert).
- Ziel-PostgreSQL < 18 ungetestet (Schema-Dump stammt von pg_dump 18).
- Sehr große Einzeltransaktion: der gesamte Umzug ist eine Transaktion (WAL-Volumen ≈ Datenmenge +
  Indizes); auf dem Ziel muss entsprechend Platz sein.
- Debug-Build gemessen; Release ist deutlich schneller.
