# Umzug einer bestehenden paperless-ngx-Datenbank nach PostgreSQL

PaperRust läuft (Phase 1) nur auf **PostgreSQL** mit dem unveränderten Django-Schema von
paperless-ngx **v3.4.2**. Wer paperless-ngx mit **SQLite** (Standard, wenn `PAPERLESS_DBHOST` fehlt)
oder **MariaDB/MySQL** betreibt, zieht die Datenbank mit `paperrust-migrate-db` um. Die Quelle
wird dabei **nur gelesen**; das Ziel ist eine neue, leere PostgreSQL-Datenbank.

Anleitung für Benutzer (Deutsch/Englisch): [docs/user/migration.md](user/migration.md); in
einfachem Deutsch: [docs/user/umstieg.md](user/umstieg.md) („Von paperless-ngx umsteigen“).

<a id="bestand"></a>

## Bestand und Lücken des Umstiegs (Stand 2026-11-13)

Produktrichtung seit 2026-11-10: PaperRust entfernt sich von paperless-ngx; **der Umstieg muss
einfach und sicher bleiben**. Der Assistent ist so gebaut, dass er beim späteren Umbau des
Datenmodells (eigene Tabellen) zum Importer werden kann: Erkennen (`detect.rs`), Lesen
(`source.rs`/`export.rs`, nur lesend), Nachholen/Anpassen (`catchup.rs`, Spalten-Abbildung je
Zieltabelle), Schreiben (`migrate.rs`, eine Transaktion) und Abgleich (`verify.rs`, fachliche
Zählung unabhängig vom Schema) sind getrennt; für ein neues Ziel-Schema werden nur Schreiben und
die Abbildung ersetzt.

| Baustein | Was es gibt | Stand |
|---|---|---|
| `paperrust migrate …` (= `paperrust-migrate-db switch`) | ein Befehl: erkennt die Quelle, prüft, sichert bei Bedarf, führt durch, zeigt Fortschritt, Abgleich mit Kurzfassung (Dokumente, Tags, Benutzer, Dateien gefunden/fehlend/beschädigt), Bericht HTML/JSON/Text, Exit 0/3/1 | fertig |
| Weg (a) PostgreSQL direkt, **neue Datenbank** | Kopie mit Prüfsumme je Tabelle; **ältere Quelle 3.0.0…3.4.1 wird beim Kopieren nachgeholt** (`catchup.rs`), **2.20-Quelle** mit dem 2.20-Schema kopiert, geprüft und im Ziel in derselben Transaktion umgebaut; Quelle bleibt unverändert | fertig, E2E mit echter 3.1.3- und 2.20.15-DB |
| Weg (a) PostgreSQL direkt, **gleiche Datenbank** (Docker: Image tauschen) | `switch` mit Quelle = Ziel: Sicherung (pg_dump-Datei, sonst Server-Kopie `CREATE DATABASE … TEMPLATE`), dann Nachholen in einer Transaktion; `paperrust manage migrate` holt beim Start ebenfalls nach (wie Django) | fertig, E2E |
| Weg (b) Export (`document_exporter`, Ordner/ZIP) | `manage document_importer` + Abgleich gegen das Manifest | fertig, E2E (Export aus PaperRust, Format wie upstream) |
| Weg (c) Medienordner | Kopie ohne Überschreiben mit Fortschritt, vorher Platzprüfung; Prüfung jeder Datei gegen die Prüfsumme in Quelle und Ziel | fertig |
| SQLite / MariaDB / MySQL | Kopie nach PostgreSQL (wie Weg a), auch mit Nachholen ab 2.20 | fertig (Tests aus `run_sqlite.sh`/`run_mariadb.sh`; 2.20 auf SQLite E2E in `run_v2_newer.sh`; 2.20 auf MariaDB nicht getestet) |
| Vorab-Prüfungen | Version/Migrationsstand, Verbindung zum Ziel, freier Platz für Dateien, Ziel leer | fertig |
| paperless-ngx **2.20** (auch 2.19.6, 2.20.0–2.20.4) | Umbau 2.20 → 3.4.2 ohne Python, gleiche Schritte wie upstream 3.4.2 `migrate` (Abschnitt unten) | fertig, E2E: Schema und Daten Zeile für Zeile gleich wie upstream |
| paperless-ngx **neuer als 3.4.2** | bekannte Migrationen `documents.0028`/`0029` (upstream `dev`) angenommen; unbekannte → Abbruch mit Export-Weg | fertig, E2E mit echter dev-DB |
| Desktop-Erststart | Auswahl „Von paperless-ngx umsteigen“ (eine Eingabe); danach zeigt die Hülle den Abgleich (Ergebnis, Kurzfassung, erste Hinweise, „Ganzen Bericht öffnen“) vor der Oberfläche | fertig (Unit-Test Supervisor; Hülle nicht im Fenster geprüft) |

**Lücken (offen):**

- paperless-ngx **vor 2.19.6**: keine direkte Übernahme (upstream verlangt für v3 selbst 2.20.15);
  Weg über Aktualisieren auf 2.20.15 oder Export. Meldung erklärt das.
- paperless-ngx mit **unbekannten** Migrationen nach 3.4.2 → Abbruch mit Hinweis auf Export.
- Test mit einer großen, **echten** Datenbank: vorbereitet (`tests/dbmigrate/real_db_check.sh`,
  nur Zahlen, Geheimnisse vorher gelöscht). Lauf 2026-11-16 mit der echten paperless-ngx-3.1.3-DB
  des Besitzers (Postgres 17.4, ohne Mediendateien): Probelauf OK, Umstieg in neue DB und an Ort und
  Stelle ohne Fehler, alle Zählungen Quelle = Ziel, 5 Migrationen bis 3.4.2 nachgeholt; danach
  `paperrust manage migrate`, Suchindex-Neuaufbau (23 s) und `serve` auf der Kopie: Listen,
  Filter, Statistik, Volltextsuche, Autovervollständigung und „Ähnliche“ antworten mit 200 und den
  erwarteten Zahlen (Suche 0,2 s). Ein in ngx gelöschter Freigabelink bleibt verborgen (wie upstream).
  Nicht geprüft: Dateien gegen den echten Medienordner (Mediendateien blieben auf dem NAS).
  Zweiter Lauf (Kopie zum Ansehen, `paperrust manage migrate` direkt auf der 3.1.3-DB wie beim
  Docker-Image-Tausch) fand zwei Fehler, beide behoben: Spaltenprüfung abhängig von der Reihenfolge
  und alte Sequenznamen (`documents_sender_id_seq`) – siehe docs/contract/pl-cli.md.
  Kopie, Wegwerf-Datenbanken und Berichte danach gelöscht.
- Export aus einer **echten** paperless-ngx 3.1.x wurde nicht getestet (kein laufendes 3.1.x mit
  Celery/OCR in der Testumgebung); getestet ist ein Export im upstream-Format aus PaperRust.
- Die Platzprüfung kennt nur den Medienordner; den Platz auf dem Datenbank-Server prüft sie nicht.
- Docker-Bild enthält kein `pg_dump` (Server-Version-Bindung); Sicherung dort als Server-Kopie
  (braucht CREATEDB – beim Standard-Compose ist der Benutzer Superuser).
- Fremd-Apps (allauth, auditlog, guardian) im 2.20-Schema stammen aus dem Referenz-venv; ob eine
  echte 2.20.15-Installation (uv.lock: auditlog 3.3.0, guardian 3.2.0, allauth 65.12.1) genau
  dieselben Fremd-Migrationen hat, ist nicht offline geprüft. Fehlt eine, meldet der Umstieg sie
  mit Namen und bricht ab (nichts verändert).

### paperless-ngx 2.20 übernehmen

upstream v3.0 hat die Migrationsgeschichte gesquasht (`documents.0001_squashed`/`0002_squashed`,
`paperless_mail.0001_squashed`) und verlangt 2.20.15 (`paperless.checks`, `docs/migration-v3.md`).
Django hält die Squashes für angewandt, wenn alle ersetzten 2.x-Migrationen da sind, und führt dann
42 Migrationen aus (documents 0003–0027, paperless 0005–0018, paperless_mail 0002–0004). PaperRust
macht genau das: SQL in `crates/pl-dbmigrate/schema/catchup-v2.20-v3.0.sql` (erzeugt mit
`tests/dbmigrate/gen_catchup_v2.py` aus upstream v3.4.2), danach `catchup-v3.0.0-v3.4.2.sql`;
Datenmigrationen von Hand übersetzt (Freigabe-Bündel-Rechte, Sichtbarkeit gespeicherter Ansichten →
UI-Einstellungen, Regeltypen, OCR-Modus, Kappen von Zahlen), SHA-256 aus den Dateien und die
Volltext-Präfixe in Rust. Die Squash-Zeilen werden am Ende eingetragen (wie `check_replacements`).
2.19.6/2.20.0–2.20.4 fehlt nur `documents.1075_workflowaction_order` – PaperRust holt es mit nach
(upstream verweigert diese Stände; das Ergebnis ist dasselbe wie der upstream-Weg 2.20.15 → 3.4.2).
Verschlüsselte Dokumente (`storage_type = gpg`) → Abbruch vor jeder Änderung.

- **gleiche Datenbank**: Sicherung, dann alles in einer Transaktion (`SET CONSTRAINTS ALL IMMEDIATE`
  statt Djangos `atomic = False` in `paperless_mail.0002`).
- **neue Datenbank** (auch von SQLite/MariaDB): eingebettetes 2.20-Schema (`schema/v2.20-*`, aus
  `make_v220_fixture.sh`), Kopie + Prüfsummen wie immer, dann Umbau im Ziel in derselben
  Transaktion; danach Spaltenprüfung gegen v3.4.2 (Constraint-Namen einer umgebauten 2.x-DB weichen
  wie bei upstream von einer frischen 3.4.2-DB ab).
- `paperrust manage migrate` (Start auf vorhandener DB) baut 2.x **nicht** um, sondern verweist auf
  `paperrust migrate` (Sicherung, Medienordner für SHA-256).
- Test: `tests/dbmigrate/make_v220_fixture.sh` (echte 2.20.15-DB aus upstream-Code, SQLite und
  PostgreSQL, upstream-3.4.2-Umbau als Erwartungswert) + `run_v2_newer.sh`.

### paperless-ngx neuer als 3.4.2

upstream `dev` hat nach 3.4.2 `documents.0028_replace_tag_tree_fields` (#14612, treenode-Spalten
weg – wie PaperRusts `pl_objects.0001`) und `0029_…` (nur Auswahlliste). Beide sind bekannt
(`schema::KNOWN_NEWER`, pl-cli `migrate::KNOWN_NEWER`): gleiche Datenbank wird angenommen, beim
Kopieren entsteht `documents_tag` ohne die treenode-Spalten. Jede andere unbekannte Migration einer
installierten App → klare Meldung mit Export-Weg, nichts verändert.

### Nachholen älterer Versionen (3.0.0 … 3.4.1)

Seit v3.0.0 sind zehn Migrationen dazugekommen (`documents` 0023–0027, `paperless` 0014–0018);
die Migrationen der Fremd-Apps (auth, allauth, guardian, auditlog, …) haben sich seit v3.0.0 nicht
geändert (Abgleich `uv.lock` v3.0.0…v3.4.2, alle Migrationsdateien älter). Das SQL steht in
`crates/pl-dbmigrate/schema/catchup-v3.0.0-v3.4.2.sql`, erzeugt mit Djangos `schema_editor`
(wie `sqlmigrate`) aus dem Referenz-venv; die eine Datenmigration (`paperless.0016`, `ai_enabled`
false → NULL) ist von Hand übersetzt. paperless-ngx 3.1.3 fehlen davon fünf.

- **Kopie** (`migrate.rs` + `catchup.rs`): fehlende Spalten bekommen den Wert, den die Migration
  gesetzt hätte (`icon='funnel'`, `ai_create_missing=false`, sonst NULL), fehlende Tabellen bleiben
  leer, `django_migrations` bekommt die fehlenden Zeilen (`applied` = jüngster Zeitpunkt der Quelle,
  damit Probelauf, Umzug und spätere Prüfung gleich rechnen). Die Prüfsumme vergleicht gegen genau
  diese erwarteten Werte.
- **An Ort und Stelle** (`switch` mit Quelle = Ziel, `paperrust manage migrate`): dasselbe SQL in
  einer Transaktion, danach ContentTypes/Rechte ergänzen (post_migrate).
- **Test:** `tests/dbmigrate/make_v313_fixture.sh` baut aus dem Referenz-Snapshot mit upstream-Django
  (rückwärts migriert) eine 3.1.3-Datenbank; ihr Schema ist identisch mit einer frisch mit dem
  Code von v3.1.3 migrierten Datenbank. `tests/dbmigrate/run_catchup.sh` prüft Kopie, gleiche
  Datenbank (mit und ohne pg_dump), Zurückspielen der Sicherung und den Export-Weg; das Ergebnis-Schema
  ist jeweils identisch mit frischem v3.4.2 (`pg_dump --schema-only`).

Technische Details, Prüfverfahren und Testergebnisse: [`docs/contract/pl-dbmigrate.md`](contract/pl-dbmigrate.md).

## Kurzfassung

```sh
# 0. Sicherung (immer): data/ und media/ kopieren bzw. DB-Dump
# 1. paperless-ngx auf 3.4.2 bringen und einmal starten (führt die Migrationen aus)
# 2. paperless-ngx komplett stoppen (Webserver, Worker, Scheduler, Consumer)
# 3. leere Ziel-DB anlegen
psql -U postgres -c "CREATE DATABASE paperless OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
# 4. Probelauf (schreibt nichts, liest alle Zeilen und prüft jede Umwandlung)
paperrust-migrate-db --dry-run --source sqlite:///pfad/zu/data/db.sqlite3 \
    --target postgres://paperless:PASS@localhost:5432/paperless
# 5. Umzug (eine Transaktion; bei jedem Fehler bleibt das Ziel leer)
paperrust-migrate-db --source sqlite:///pfad/zu/data/db.sqlite3 \
    --target postgres://paperless:PASS@localhost:5432/paperless --report umzug.json
# 6. optional später erneut vergleichen
paperrust-migrate-db --verify-only --source … --target …
# 7. paperless-ngx/PaperRust auf PostgreSQL umstellen (PAPERLESS_DBENGINE=postgresql, DBHOST …),
#    gleiche Pfade, gleicher PAPERLESS_SECRET_KEY; bei aktivem Lese-Cache: invalidate_cachalot
```

Quell-URLs: `sqlite:///absoluter/pfad/db.sqlite3` (Windows `sqlite:///C:/pfad/db.sqlite3`),
`mariadb://user:pass@host:3306/paperless` bzw. `mysql://…`, `postgres://…` (Kopie einer
PostgreSQL-DB in eine neue, mit Prüfung). Passwörter lieber per Umgebung übergeben:
`PAPERRUST_MIGRATE_SOURCE`, `PAPERRUST_MIGRATE_TARGET` (alte Namen `PAPERLESS_MIGRATE_*` gelten weiter). Für MariaDB genügt ein Nutzer mit
`SELECT`-Recht.

## Schritt für Schritt

### 1. Versionsstand 3.4.2

Das Werkzeug kennt genau das Schema von v3.4.2. Es vergleicht `django_migrations` der Quelle mit
der Liste einer frisch migrierten v3.4.2-Datenbank und bricht bei Abweichung ab:

> Migrationsstand der Quelle entspricht nicht paperless-ngx 3.4.2. Bitte zuerst paperless-ngx auf
> 3.4.2 aktualisieren und einmal starten (führt `migrate` aus), dann erneut versuchen.

Dabei wird aufgelistet, was **fehlt** (Quelle älter) oder **unbekannt** ist (Quelle neuer, oder eine
Zusatz-App). Harmlos und zugelassen sind – wie bei Django selbst – Zeilen alter Migrationen, die in
gesquashten Migrationen aufgegangen sind oder deren Dateien es nur in älteren paperless-Versionen
gab, sowie Zeilen von Apps, die v3.4.2 gar nicht mehr installiert (z. B. `django_q` aus paperless-ng).
Sie werden unverändert mitkopiert.

### 2. paperless-ngx stoppen

Webserver, Celery-Worker, Scheduler (beat) und Consumer müssen stehen. Das Werkzeug liest zwar in
einer lesenden Transaktion (konsistenter Schnappschuss), Änderungen nach dem Schnappschuss wären im
Ziel aber nicht enthalten. Offene Aufgaben in Redis (Warteschlange) vorher abarbeiten lassen.

SQLite: Die Datei wird **nur lesend** geöffnet (`SQLITE_OPEN_READONLY` + `PRAGMA query_only`).
paperless-ngx betreibt SQLite im WAL-Modus; liegen `db.sqlite3-wal`/`-shm` neben der Datei, werden
sie mitgelesen (nichts geht verloren). Der Ordner muss dafür lesbar sein; bei einem nur lesbaren
Datenträger ohne `-shm`-Datei kann SQLite die Datei im WAL-Modus nicht öffnen → dann eine Kopie
des ganzen `data/`-Ordners verwenden.

### 3. Ziel anlegen

Neue, **leere** Datenbank mit Kodierung UTF8 auf PostgreSQL ≥ 14 (Mindestversion von paperless-ngx
v3.4.2; getestet mit 18). Der Ziel-Nutzer braucht `CREATE` im Schema `public` (am einfachsten:
Eigentümer der Datenbank). Ist das Ziel nicht leer, bricht das Werkzeug ab – es überschreibt nie.

### 4. Probelauf `--dry-run`

Prüft Migrationsstand, Tabellen und Spalten, liest **alle** Zeilen, wandelt jeden Wert um wie beim
echten Umzug (ohne zu schreiben) und meldet jedes Problem mit Tabelle, Primärschlüssel und Spalte.
Zusätzlich: Zeilenzahlen, ungefähre Größe je Tabelle, bei SQLite `PRAGMA foreign_key_check`
(verwaiste Fremdschlüssel, die PostgreSQL ablehnen würde). `--target` ist optional; wenn angegeben,
wird nur geprüft, ob das Ziel leer und UTF8 ist.

### 5. Umzug

Ablauf, vollständig in **einer** Ziel-Transaktion:

1. eingebettetes Schema v3.4.2 anlegen (Tabellen, Identity-Sequenzen; exakt das, was Django
   v3.4.2 auf PostgreSQL erzeugt),
2. alle 75 Tabellen per `COPY` streamen (Puffer 4 MiB, auch für 100 000+ Dokumente mit großem
   `content`), Primärschlüssel unverändert, berechnete Spalten (`content_length`,
   `value_monetary_amount`) rechnet PostgreSQL selbst,
3. alle Sequenzen auf `max(id) + 1`,
4. Primärschlüssel, Unique-, Fremdschlüssel-Constraints und Indizes anlegen (PostgreSQL prüft dabei
   jede Zeile),
5. **Prüfung**: Zeilenzahl je Tabelle, Prüfsumme je Tabelle (Ziel neu gelesen) gegen Quelle,
   Sequenzen, alle Constraints/Indizes vorhanden und gültig,
6. erst dann `COMMIT`.

Schlägt irgendetwas fehl – auch Strg+C, Absturz oder Verbindungsabbruch –, rollt PostgreSQL alles
zurück: das Ziel bleibt leer, die Quelle ist unberührt. Einfach Ursache beheben und neu starten.
Der Bericht (`--report umzug.json` und `umzug.json.txt`) enthält Ergebnis je Tabelle, alle Befunde
und bei Prüfsummenabweichung die betroffenen Primärschlüssel. Exit-Code 0 nur bei Erfolg.

### 6. Danach

- `PAPERLESS_DBENGINE=postgresql` und `PAPERLESS_DBHOST`, `PAPERLESS_DBPORT`, `PAPERLESS_DBNAME`,
  `PAPERLESS_DBUSER`, `PAPERLESS_DBPASS` setzen. **Alles andere unverändert lassen** – vor allem
  `PAPERLESS_DATA_DIR`, `PAPERLESS_MEDIA_ROOT`, `PAPERLESS_FILENAME_FORMAT` und
  `PAPERLESS_SECRET_KEY` (sonst sind Sitzungen, Freigabelinks und verschlüsselte Felder ungültig).
- Nur falls danach weiter **paperless-ngx** auf der neuen DB läuft und dessen Lese-Cache aktiv ist
  (`PAPERLESS_DB_READ_CACHE_ENABLED`): **vor dem ersten Start** `manage.py invalidate_cachalot`
  ausführen (sonst liefert Redis noch Einträge der alten DB). PaperRust nutzt kein Redis.
- Nach dem Umstieg auf PaperRust werden **Redis (Broker) und die Celery-Dienste nicht mehr
  gebraucht**: Dienst und `PAPERLESS_REDIS` können entfernt werden (bleibt die Variable gesetzt,
  steht nur ein Hinweis im Log).
- Die alte Datenbank als Sicherung aufbewahren, bis alles läuft.

## Was nicht in der Datenbank liegt (bleibt, wie es ist)

| Ort | Inhalt | Nach dem Umzug |
|---|---|---|
| `MEDIA_ROOT` (`media/documents/originals`, `archive`, `thumbnails`) | Dokumentdateien | unverändert; Dateinamen stehen in der DB und werden 1:1 übernommen |
| `DATA_DIR/index` | Volltextindex (tantivy) | **kein Neuaufbau nötig**: er enthält nur Dokument-IDs, Inhalte und Rechte-IDs, die alle unverändert bleiben; die `--if-needed`-Prüfung hängt nur an Schema-Version/Fingerprint/Sprache, nicht an der DB-Engine. Im Test liefern `/api/search/` und `?query=` auf dem Ziel exakt dieselben Antworten wie auf der Quelle (gleicher Index) |
| `DATA_DIR/classification_model.pickle` | Klassifikator | bleibt gültig (verweist auf Tag-/Korrespondenten-/Typ-IDs, die gleich bleiben); wird ohnehin regelmäßig neu trainiert |
| `DATA_DIR/llm_index` | KI-Vektorindex (eigene sqlite-vec-Datei) | unabhängig von der Haupt-DB, bleibt |
| `DATA_DIR/db.sqlite3` | alte SQLite-DB | wird nach der Umstellung nicht mehr benutzt; als Sicherung behalten |
| Redis | Warteschlange, Lese-Cache, WebSocket-Kanäle (paperless-ngx) | flüchtig; vor dem Umzug Aufgaben abarbeiten. PaperRust braucht kein Redis (Warteschlange in PostgreSQL) – nach dem Umstieg kann es weg |

## Sonderfälle

**NUL-Zeichen (U+0000).** SQLite und MariaDB erlauben sie in Texten, PostgreSQL nicht (weder in
`text` noch in `jsonb`). Standard: Abbruch mit Liste aller betroffenen Zellen (Tabelle, PK, Spalte).
Bewusst übernehmen mit `--nul replace` (U+0000 → U+FFFD „�“) oder `--nul strip` (entfernen); jede
veränderte Zelle steht dann im Bericht (`values_changed_by_policy`, `issues`). Nie stilles Verwerfen.
Die Prüfsumme vergleicht in diesem Fall gegen den bewusst veränderten Wert.

**Ungültiges UTF-8** (nur SQLite möglich): Standard Abbruch, `--invalid-utf8 replace` ersetzt durch
U+FFFD und meldet jede Zelle.

**Zu lange Texte, ungültige Daten.** SQLite erzwingt keine `varchar`-Längen und keine Typen.
Werte, die PostgreSQL ablehnen würde (Text länger als `varchar(n)`, kein gültiges Datum, kein
JSON, NULL in NOT-NULL-Spalte, verwaiste Fremdschlüssel), meldet der Probelauf einzeln. Sie müssen in
der Quelle (bzw. einer Kopie davon) korrigiert werden – das Werkzeug verändert keine Daten still.

**Fremde Tabellen.** Tabellen, die nicht zum Django-Schema v3.4.2 gehören (eigene Erweiterungen,
Reste alter Apps), führen zum Abbruch mit Namensliste; `--skip-unknown-tables` lässt sie bewusst weg.

**Verhalten nach dem Umzug** (kein Datenverlust, aber anderes Verhalten der Engine, gilt genauso für
paperless-ngx selbst auf PostgreSQL):
- Sortierung nach Text folgt der PostgreSQL-Kollation (SQLite: Bytefolge; MariaDB: `utf8mb4_unicode_ci`).
- Bei aufsteigender Sortierung stehen `NULL`-Werte in PostgreSQL am Ende (SQLite/MariaDB: am Anfang),
  z. B. `ordering=archive_serial_number`.
- `…__icontains` mit Umlauten: SQLite vergleicht nur ASCII ohne Groß/Klein (ü ≠ Ü), MariaDB
  (`_ci`) zusätzlich akzentunabhängig (ü = u), PostgreSQL nur ohne Groß/Klein (ü = Ü, ü ≠ u).
- Reihenfolge bei Gleichstand (z. B. gleiche `created`-Zeit) ist nicht festgelegt.
- `jsonb` normalisiert JSON (Schlüsselreihenfolge, doppelte Schlüssel, `-0.0` → `0.0`). Werte sind
  gleich, die Textform kann abweichen.

<a id="welcher-weg"></a>

## Welcher Weg? `paperrust-migrate-db` oder `document_exporter` → `document_importer`

paperless-ngx bringt selbst einen Weg mit: auf der alten Installation `document_exporter`
(optional `--data-only`), neue Installation auf PostgreSQL aufsetzen, dort `document_importer`.

| | `paperrust-migrate-db` | `document_exporter` → `document_importer` |
|---|---|---|
| Was wird kopiert | **jede Zeile jeder der 75 Tabellen**, auch API-Tokens, Sitzungen, Aufgabenliste, Mail-Verarbeitungsstand (`processedmail`), Workflow-Läufe, E-Mail-Adressen (allauth), `django_migrations` | nur die Modelle im Exporter-Manifest (`document_exporter.py`, `manifest_key_to_object_query`). **Nicht** dabei: API-Tokens (`authtoken_token` – Apps/Skripte brauchen neue Tokens), Sitzungen, `PaperlessTask`, `ProcessedMail` (Mail-Regeln können bereits verarbeitete Mails erneut holen), `WorkflowRun`, allauth-`EmailAddress`; Audit-Log nur mit `PAPERLESS_AUDIT_LOG_ENABLED` |
| Dateien | bleiben, wo sie sind (nur DB) | werden mit exportiert/importiert (ohne `--data-only`): doppelter Platz, lange Laufzeit |
| Prüfung | Zeilenzahl + Prüfsumme je Tabelle, Sequenzen, Constraints; JSON-Bericht | keine Gegenprobe |
| Abbruchsicherheit | eine Transaktion, Ziel bleibt bei Fehler leer | Import bricht ggf. halb ab → Ziel neu aufsetzen |
| Speicher | gestreamt (Puffer 4 MiB) | Manifest im Speicher (`--batch-size`), bei sehr großen Installationen viel RAM |
| Versionen | Quelle **muss** genau v3.4.2 sein | Export/Import sollten dieselbe Version haben; leichte Abweichungen „may work“ |
| Ziel | nur PostgreSQL | jede von paperless unterstützte DB (auch SQLite/MariaDB als Ziel) |
| Laufzeitumgebung | ein Programm, kein Python nötig; Quelle per URL | braucht laufende paperless-Installation(en) |
| Daten bereinigen | nie still: Probleme (NUL, zu lange Texte) werden gemeldet, Korrektur nur per Option | der ORM-Import lehnt problematische Werte mit Python-Fehler ab |

**Empfehlung:** Für den Wechsel SQLite/MariaDB → PostgreSQL auf derselben Maschine bzw. mit
denselben Dateien `paperrust-migrate-db` (vollständig, geprüft, schnell, keine Dateikopie).
`document_exporter`/`document_importer` nehmen, wenn ohnehin auf einen neuen Rechner umgezogen
wird und die Dateien mitwandern sollen, wenn die Quelle nicht auf v3.4.2 gebracht werden kann,
wenn das Ziel nicht PostgreSQL ist – und als **zusätzliche** Sicherung vor jedem Umzug.
