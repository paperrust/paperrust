# Umstieg von paperless-ngx

[English](en/migration.md) · [Übersicht](README.md)

PaperRust übernimmt das Datenbankschema von paperless-ngx v3.4.2 und dieselbe Dateiablage. Ein
Umstieg ist deshalb kein Datenimport, sondern ein Wechsel des Programms, das auf Datenbank und
Dateien zeigt. **Danach** passt PaperRust das Schema schrittweise an sein eigenes Datenmodell an
(erster Schritt: Tags ohne die Hilfsspalten von django-treenode). Dann kann paperless-ngx dieselbe
Datenbank nicht mehr öffnen – der Rückweg geht über die Sicherung oder einen Export
([Zurück](#back)). Welcher Weg passt:

| Ausgangslage | Weg |
|---|---|
| **egal welche** – empfohlen | [Umstieg in einem Schritt mit Prüfbericht](#one-step) (erkennt den passenden Weg selbst) |
| paperless-ngx mit **PostgreSQL** | [Gleiche Datenbank weiterverwenden](#same-database) |
| paperless-ngx mit **SQLite** oder **MariaDB/MySQL** | [Datenbank umziehen mit `paperrust-migrate-db`](#sqlite-mariadb) |
| neuer Rechner, Dateien sollen mitwandern, oder paperless-ngx vor 2.19.6 bzw. mit einer Änderung nach 3.4.2, die PaperRust nicht kennt | [Export und Import](#export-import) |
| erst ausprobieren | auf einer **Kopie** der Datenbank ([Umstieg in einem Schritt](#one-step) mit neuer Ziel-DB); paperless-ngx bleibt unberührt. Einen [Mischbetrieb](installation.md#mixed-operation) gibt es nicht mehr |

> [!IMPORTANT]
> Vor jedem Schritt sichern: Datenbank (`pg_dump -Fc`), `data/` und `media/`
> ([Administration](administration.md#backup)). PaperRust verwendet das Schema von paperless-ngx
> **v3.4.2**. Datenbanken von paperless-ngx **2.20** (auch 2.19.6) und **3.0.0 bis 3.4.1** (z. B.
> 3.1.3) bringt PaperRust selbst auf diesen Stand (dieselben Django-Migrationen wie paperless-ngx,
> ohne Python, siehe [MIGRATION.md](../MIGRATION.md#bestand) und [Umstieg → 2.20](umstieg.md#v2));
> paperless-ngx vorher zu aktualisieren ist nicht nötig. Datenbanken einer neueren paperless-ngx mit
> den bekannten Änderungen nach 3.4.2 (neuer Tag-Baum) werden so übernommen. Ältere Stände gehen
> über einen Export.

> **Bitte gesund skeptisch sein.** PaperRust 0.1.0 ist eine erste Version. Der Kern ist gründlich geprüft –
> aber nicht mit Ihren Dokumenten. Vor dem Umstieg: eine **vollständige Sicherung** von paperless-ngx machen
> (Export oder Datenbank und Medienordner), PaperRust zuerst **mit einer Kopie ausprobieren** und paperless-ngx
> erst abschalten, wenn alles passt. Nichts blind übernehmen. Der Weg zurück zu paperless-ngx bleibt offen.

<a id="one-step"></a>

## Umstieg in einem Schritt (empfohlen)

Sie geben nur an, **wo** Ihre paperless-ngx-Daten liegen. PaperRust erkennt die Art der Quelle,
wählt den Weg, übernimmt alles und prüft das Ergebnis. Die Quelle wird dabei **nur gelesen**,
Dateien werden ohne Überschreiben kopiert.

| Sie geben an | PaperRust erkennt | Weg |
|---|---|---|
| Export-Ordner (mit `manifest.json`) oder Export-ZIP aus `document_exporter` | Export, jede neuere paperless-ngx-Version | Import (`document_importer`) |
| paperless-Ordner mit `data/` und `media/`, den `data`-Ordner oder `db.sqlite3` | paperless-ngx mit SQLite, Medienordner daneben | Datenbank-Umzug + Dateien kopieren |
| `postgres://…`, `mariadb://…`, `mysql://…`, `sqlite:///…` (+ Medienordner) | Datenbank | Datenbank-Umzug + Dateien kopieren |

Eine Datenbank von paperless-ngx **2.20 oder 3.0.0 bis 3.4.2** wird direkt übernommen; ist sie
älter als 3.4.2, holt PaperRust die fehlenden Änderungen nach – beim Kopieren in eine neue Datenbank
ohne die Quelle anzufassen (eine 2.x-Quelle wird mit ihrem eigenen Schema kopiert, geprüft und im
Ziel in derselben Transaktion umgebaut), bei derselben Datenbank nach einer automatischen Sicherung
(pg_dump-Datei oder Kopie der Datenbank auf dem Server). Bei 2.x den Medienordner angeben: die
Prüfsummen werden wie in paperless-ngx 3 aus den Dateien neu berechnet (SHA-256). Eine Datenbank mit
den bekannten Änderungen nach 3.4.2 – also auch paperless-ngx **3.5.0/3.5.1** (upstream
`documents.0028`/`0029`/`0030`) – wird übernommen. Ist sie
älter als 2.19.6 oder hat sie eine unbekannte neuere Änderung, bricht der Umstieg ab, bevor etwas
geschrieben wird, und erklärt den Ausweg: in paperless-ngx einen Export machen und diesen angeben.

### Desktop-App (Windows)

1. paperless-ngx beenden. (paperless-ngx 2.20 und 3.0 bis 3.4 werden direkt übernommen; bei
   älteren Versionen in paperless-ngx `document_exporter` ausführen und den Export nehmen.)
2. PaperRust installieren und starten. Beim ersten Start erscheint die Auswahl.
3. „Von paperless-ngx umsteigen“ wählen (Konsole: `2`) und **einen** Pfad bzw. eine URL eingeben,
   z. B. `D:\paperless` (Ordner mit `data` und `media`), `D:\export\export.zip` oder
   `postgres://paperless:PASSWORT@nas:5432/paperless`. Für eine Datenbank-URL wird nach dem alten
   Medienordner gefragt.
4. Warten. Danach öffnet sich PaperRust wie gewohnt; die Anmeldung erfolgt mit den bisherigen
   Benutzern und Passwörtern.
5. **Prüfbericht:** Nach dem ersten Start zeigt die App den Abgleich selbst an (Ergebnis,
   Kurzfassung, erste Hinweise, Knopf „Ganzen Bericht öffnen“), bevor sie die Oberfläche öffnet.
   Im Log-Ordner der App (`…\PaperRust\logs\`) liegt die Datei `umstieg-bericht.html` (dazu
   `.json` und `.txt`). Sie enthält die Gegenüberstellung
   aller Objektarten, die Prüfung jeder Datei und eine Stichprobe über die laufende App. Oben steht
   das Ergebnis **OK**, **WARN** oder **FAIL** (Bedeutung unten).

Ohne Dialog (verwaltete Installationen): `PAPERRUST_DESKTOP_IMPORT=<Pfad oder URL>`, optional
`PAPERRUST_DESKTOP_IMPORT_MEDIA=<Medienordner>`.

### Docker / Server

Der Befehl `paperrust-migrate-db switch` liegt im Image. Im Container stehen Ziel-Datenbank
(`PAPERRUST_DBHOST` …) und Medienordner (`PAPERRUST_MEDIA_ROOT`) schon in der Umgebung, der
Bericht landet in `data/umstieg/`.

1. **Sichern** (siehe oben) und paperless-ngx stoppen: `docker compose down`.
2. In der compose-Datei das Image auf PaperRust tauschen
   ([was sich ändert](installation.md#migrate)). Bei
   SQLite/MariaDB zusätzlich einen `db`-Dienst mit PostgreSQL und leerer Datenbank anlegen und
   `PAPERRUST_DBHOST=db` setzen. Die Volumes `data` und `media` bleiben, wie sie sind.
3. Probelauf (prüft nur, schreibt nichts):

   ```sh
   docker compose up -d db
   # paperless-ngx mit SQLite (Datenordner im Volume data):
   docker compose run --rm webserver paperrust-migrate-db switch --dry-run --from /usr/src/paperless/data
   # paperless-ngx-Export (Ordner oder ZIP auf dem Host):
   docker compose run --rm -v /pfad/zum/export:/export webserver paperrust-migrate-db switch --dry-run --from /export
   # MariaDB: --from "mariadb://paperless:PASS@mariadb:3306/paperless" --media /usr/src/paperless/media
   ```

4. Derselbe Befehl ohne `--dry-run` führt den Umstieg aus.
5. `docker compose up -d` und anschließend die Stichprobe über die laufende App nachholen:

   ```sh
   docker compose exec webserver paperrust-migrate-db verify --from <wie oben> \
       --api-url http://localhost:8000 --api-temp-token
   ```

**PostgreSQL ohne Umzug** (nur das Image getauscht, gleiche Datenbank): nach dem Start
`docker compose exec webserver paperrust-migrate-db switch --from "postgres://paperless:PASS@db:5432/paperless" --api-url http://localhost:8000 --api-temp-token`
– Quelle und Ziel sind dieselbe Datenbank, es wird nur geprüft.

Ohne Docker gilt dasselbe mit `--target postgres://…` und `--target-media <Ordner>`;
`paperrust-migrate-db switch --help` zeigt alle Optionen.

### Der Prüfbericht

| Teil | Was verglichen wird |
|---|---|
| Objekte | Anzahl je Art in Quelle und Ziel: Dokumente, Tag-Zuordnungen, Tags, Korrespondenten, Dokumenttypen, Speicherpfade, benutzerdefinierte Felder und Feldwerte, Notizen, gespeicherte Ansichten, Workflows (mit Auslösern/Aktionen), Benutzer, Gruppen, E-Mail-Konten und -Regeln, Freigabelinks |
| Dokumente | jede Zeile: Titel, Inhalt (SHA-256 des Texts), Tags, Prüfsummen, Papierkorb-Status |
| Dateien | jedes Original und jede Archivdatei gegen die Prüfsumme in der Datenbank (SHA-256, ältere Bestände MD5), Vorschaubilder vorhanden, fehlende und verwaiste Dateien – in Quelle **und** Ziel |
| Stichprobe | 10 Dokumente (`--sample`) über API v1 der laufenden App: Titel, Inhalt, Tags und Prüfsumme des heruntergeladenen Originals (bei Dokumenten mit Versionen: neueste Version, wie die App sie zeigt) |

| Ergebnis | Bedeutung |
|---|---|
| **OK** | alles übernommen und geprüft |
| **WARN** | übernommen, aber Hinweise: z. B. eine Datei war **schon in paperless-ngx** fehlerhaft („bereits in der Quelle“), Vorschaubild fehlt, verwaiste Dateien, Stichprobe nicht möglich, Export aus anderer Version |
| **FAIL** | etwas fehlt oder weicht ab (Anzahl, Dokumentzeile, Prüfsumme einer Datei) oder der Umstieg war nicht möglich – der Bericht nennt jede Stelle. Die Quelle ist unverändert; paperless-ngx kann wie bisher weiterlaufen |

Exit-Code des Befehls: `0` OK, `3` WARN, `1` FAIL. `--api-temp-token` nimmt den API-Token eines
aktiven Administrators aus der Ziel-Datenbank bzw. legt für die Stichprobe kurz einen an und löscht
ihn danach.

<a id="same-database"></a>

## Gleiche Datenbank weiterverwenden (PostgreSQL)

1. Sichern (`pg_dump -Fc`). paperless-ngx muss **mindestens 3.0.0** sein; ein Update auf 3.4.2 ist
   nicht nötig.
2. paperless-ngx stoppen (Webserver, Celery-Worker, Scheduler, Consumer). Offene Aufgaben vorher
   abarbeiten lassen.
3. PaperRust mit **derselben** Konfiguration starten: gleiche `PAPERLESS_DB*`-Werte, gleiche
   `PAPERLESS_DATA_DIR`, `PAPERLESS_MEDIA_ROOT`, `PAPERLESS_FILENAME_FORMAT` und vor allem
   **derselbe `PAPERLESS_SECRET_KEY`** (sonst werden Sitzungen, Freigabelinks und verschlüsselte
   Felder ungültig).
4. `paperrust manage migrate` – prüft den Migrationsstand, holt bei paperless-ngx 3.0.0–3.4.1 die
   fehlenden Django-Migrationen nach (eine Transaktion, wie Django beim Update; Abbruch bei 2.x oder
   neuer als 3.4.2), ergänzt fehlende Rechte/ContentTypes und legt die eigenen Tabellen
   `paperrust_*` an. Mit Abgleich und automatischer Sicherung: `paperrust migrate --from
   postgres://…` (Quelle = Ziel, siehe oben).
5. `paperrust serve --worker` ([Installation](installation.md#standalone)).

Was gleich bleibt: Benutzer, Passwörter, API-Tokens, Sitzungen, Dokumente, Tags, Workflows,
Mail-Regeln, gespeicherte Ansichten, Freigabelinks, Audit-Log, Aufgabenliste.

Beispiel aus dem Projekt (docs/STANDALONE.md): paperless-ngx 3.1.3 in Docker mit PostgreSQL 17.4 →
Sicherung → Image auf PaperRust tauschen → `paperrust migrate` (oder einfach starten) holt die fünf
fehlenden Migrationen von 3.1.3 nach. Kein Datenbankumzug, kein Zwischenschritt über 3.4.2 nötig.

<a id="sqlite-mariadb"></a>

## SQLite oder MariaDB/MySQL → PostgreSQL

PaperRust läuft nur auf PostgreSQL. Am einfachsten geht der Umzug mit `paperrust migrate --from …`
([Von paperless-ngx umsteigen](umstieg.md), Fall 4). Darunter arbeitet das Werkzeug
`paperrust-migrate-db` (Crate `pl-dbmigrate`), das man auch direkt aufrufen kann. Es kopiert eine
paperless-ngx-Datenbank (2.20/2.19.6 und 3.0 bis 3.4.2; fehlende Schritte holt es im Ziel nach)
vollständig nach PostgreSQL. Die Quelle wird **nur
gelesen**, das Ziel muss eine neue, leere Datenbank sein. Der Umzug läuft in **einer**
Transaktion: bei jedem Fehler (auch Strg+C) bleibt das Ziel leer.

```sh
# 1. paperless-ngx komplett stoppen
# 2. leere Ziel-DB
psql -U postgres -c "CREATE DATABASE paperless OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
# 3. Probelauf: liest alle Zeilen, prüft jede Umwandlung, schreibt nichts
paperrust-migrate-db --dry-run --source sqlite:///pfad/zu/data/db.sqlite3 \
    --target postgres://paperless:PASS@localhost:5432/paperless
# 4. Umzug mit Bericht
paperrust-migrate-db --source sqlite:///pfad/zu/data/db.sqlite3 \
    --target postgres://paperless:PASS@localhost:5432/paperless --report umzug.json
# 5. optional später erneut vergleichen
paperrust-migrate-db --verify-only --source … --target …
```

- Quellen: `sqlite:///absoluter/pfad/db.sqlite3` (Windows `sqlite:///C:/pfad/db.sqlite3`),
  `mariadb://user:pass@host:3306/paperless`, `mysql://…`, `postgres://…`. Passwörter besser über
  `PAPERRUST_MIGRATE_SOURCE` / `PAPERRUST_MIGRATE_TARGET` übergeben (alte Namen `PAPERLESS_MIGRATE_*` gelten weiter).
- Geprüft wird: Zeilenzahl und Prüfsumme je Tabelle, Sequenzen, alle Constraints und Indizes.
- Danach `PAPERRUST_DBENGINE=postgresql` und die `PAPERRUST_DB*`-Werte setzen, **alles andere
  unverändert lassen** (Pfade, `PAPERLESS_SECRET_KEY`). War der Lese-Cache aktiv
  (`PAPERLESS_DB_READ_CACHE_ENABLED`) und läuft paperless-ngx noch: vor dem ersten Start
  `manage.py invalidate_cachalot`.
- Sonderfälle (NUL-Zeichen, ungültiges UTF-8, zu lange Texte, fremde Tabellen) meldet der
  Probelauf einzeln; Optionen `--nul replace|strip`, `--invalid-utf8 replace`,
  `--skip-unknown-tables`. Nichts wird still verändert.

Die vollständige Anleitung mit allen Optionen und Sonderfällen: [docs/MIGRATION.md](../MIGRATION.md).


Nach dem Umzug verhält sich PostgreSQL in Kleinigkeiten anders als SQLite/MariaDB (gilt genauso für
paperless-ngx auf PostgreSQL): Sortierung nach Kollation, `NULL` bei aufsteigender Sortierung am
Ende, `icontains` mit Umlauten (ü = Ü, ü ≠ u), JSON wird normalisiert.

<a id="export-import"></a>

## Export und Import

Der Weg von paperless-ngx selbst funktioniert auch mit PaperRust, in beide Richtungen:

```sh
# auf der alten Installation (paperless-ngx)
docker compose exec webserver document_exporter ../export        # bzw. manage.py document_exporter
# auf der neuen Installation (PaperRust, leere DB nach "paperrust manage migrate")
paperrust manage document_importer /pfad/zum/export
```

Vorteile: Dateien wandern mit, Quelle muss nicht genau 3.4.2 sein (gleiche Version empfohlen).
Nachteile gegenüber dem Datenbankumzug: API-Tokens, Sitzungen, Aufgabenliste, Mail-Verarbeitungsstand
(`processedmail`), Workflow-Läufe und allauth-E-Mail-Adressen werden nicht exportiert; doppelter
Platzbedarf. Vergleich: [docs/MIGRATION.md](../MIGRATION.md#welcher-weg).

<a id="files"></a>

## Was außerhalb der Datenbank liegt

| Ort | Nach dem Umstieg |
|---|---|
| `MEDIA_ROOT` (Originale, Archiv, Vorschaubilder) | unverändert weiterverwendet |
| `DATA_DIR/index` (Volltextindex, tantivy) | gleiches Format, wird weiter gelesen und geschrieben; Neuaufbau mit `paperrust manage document_index reindex` möglich |
| `DATA_DIR/classification_model.pickle` | wird von PaperRust **nicht** gelesen. PaperRust trainiert ein eigenes Modell (`DATA_DIR/paperrust_classifier.model`), automatisch per Zeitplan oder sofort mit `paperrust manage document_create_classifier` |
| `DATA_DIR/llm_index` (KI-Vektorindex) | gleiches Format (SQLite + sqlite-vec), gemeinsam nutzbar |
| Redis/Valkey, Celery-Worker | werden nicht mehr gebraucht und können nach dem Umstieg weg (`PAPERLESS_REDIS` ebenso; gesetzt bleibt es wirkungslos, mit Hinweis im Log) |

<a id="real-db"></a>

## Test mit einer echten Datenbank (vorbereitet)

Alle automatischen Tests laufen mit Testdaten (echte paperless-ngx-Datenbanken 2.20.15, 3.1.3 und
3.4.2, erzeugt mit dem unveränderten upstream-Code). Der Test mit einem großen, echten Bestand ist
vorbereitet und wartet auf den Besitzer, weil nur er seine Datenbank hat. So geht er, auf dem
eigenen Rechner:

1. Sicherung der echten Datenbank erstellen (`pg_dump -Fc … > nas.pgdump`) und auf den Rechner
   kopieren. Den Medienordner nur lesbar bereitstellen (z. B. Netzlaufwerk), er wird nicht kopiert.
2. `cargo build -p pl-dbmigrate`, dann
   `bash tests/dbmigrate/real_db_check.sh nas.pgdump //nas/paperless/media`.

Das Skript spielt die Sicherung in eine **Wegwerf-Datenbank** zurück und löscht dort zuerst alle
Geheimnisse (Mail-Passwörter, OAuth- und API-Token, Sitzungen, Passwort-Hashes, MFA, Webhook-
Kopfzeilen, KI-/OCR-Schlüssel, Freigabe-Links). Dann laufen Erkennung, Probelauf, Umstieg in eine
neue Datenbank und Umstieg an Ort und Stelle (auf einer zweiten Kopie). Auf dem Bildschirm und in
`zahlen.txt` stehen **nur Zahlen**: Version, Anzahlen je Objektart, Datei-Befunde als Anzahl,
Dauer – keine Titel, Inhalte, Namen oder Dateinamen. Die vollständigen Berichte bleiben im privaten
Ordner `tests/dbmigrate/.work/real` und gehören nicht weitergegeben. Am Ende werden die
Wegwerf-Datenbanken gelöscht (`KEEP=1` behält sie).

<a id="back"></a>

## Zurück zu paperless-ngx

**Nicht mehr mit derselben Datenbank.** PaperRust ändert beim ersten `migrate` das Schema
(Tags ohne die django-treenode-Spalten, Migration `pl_objects.0001_paperrust_tag_adjacency`);
paperless-ngx 3.4.2 kann diese Datenbank danach nicht mehr öffnen. Zwei Wege zurück:

1. **Sicherung zurückholen** (alles seit dem Umstieg geht verloren): die Kopie bzw. Datei aus dem
   Umstieg, siehe [Umstieg → Zurück](umstieg.md#zurück-zu-paperless-ngx).
2. **Export aus PaperRust, Import in paperless-ngx** (behält alles, was Sie in PaperRust gemacht
   haben): `paperrust manage document_exporter /export` schreibt das Exportformat von
   paperless-ngx **3.4.2** (die Baumfelder der Tags werden dabei berechnet). Dann in einer
   **leeren** paperless-ngx-3.4.2-Installation `document_importer /export`.

Zu beachten: PaperRust-eigene Daten (Sync-Stand, KI-Verlauf, Lebensbereiche, Einstellungen aus
`paperrust_setting`) sind nicht Teil des Exports. Dokumente, die PaperRust eingelesen hat, sind
fachlich gleich, aber nicht byte-gleich (z. B. Vorschaubilder verlustfrei, Barcode-geteilte PDFs mit
anderen Prüfsummen); der Klassifikator von paperless-ngx trainiert sein Pickle neu.

<a id="differences"></a>

## Unterschiede, die man nach dem Umstieg bemerken kann

- **Oberfläche:** eigenes PaperRust-Design als Standard; die klassische Oberfläche im Stil von
  paperless-ngx ist unter *Einstellungen → Darstellung* wählbar. 10 statt über 30 Sprachen.
- **Office-Dokumente und Mails:** Text ohne Tika; das PDF-Abbild (bei paperless-ngx von Gotenberg)
  macht PaperRust selbst mit `paperrust-pdftool`. Office-Metadaten sind spärlicher als bei Tika.
- **Texterkennung:** Tesseract/PDFium statt OCRmyPDF; Säubern (`PAPERLESS_OCR_CLEAN`) und
  Geraderichten (`PAPERLESS_OCR_DESKEW`) sind eingebaut (kein `unpaper` nötig) und behalten ihre
  Bedeutung. Unterschied: Bei eingeschaltetem Geraderichten behalten schon gerade Seiten ihr
  Originalbild im Archiv (OCRmyPDF rastert jede gescannte Seite neu). PDF/A wird erzeugt und selbst
  geprüft, aber nicht mit veraPDF; bettet das Original Schriften nicht ein, entsteht ein normales PDF.
  Einige `OCR_USER_ARGS`-Optionen von OCRmyPDF wirken nicht (Hinweis im Log).
- **Klassifikator:** eigenes Modell, lernt nach dem Umstieg neu; Vorschläge können bei einzelnen
  Dokumenten abweichen (2–4,5 % der Korrespondenten im Test).
- **Verschlüsselte Mails (GPG):** gehen, wenn `gpg` installiert ist (nicht im Docker-Image).
- **Anmeldung:** OpenID Connect, GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea; andere allauth-Anbieter
  nicht. Kein `/admin/`.
- **Ohne Django** fehlen sonst keine Adressen ([Liste](installation.md#standalone-limits)).
- Vollständige Liste mit Nutzerwirkung: [Release-Notizen → Bekannte Grenzen](../RELEASE-NOTES-0.1.0.md#grenzen).

Alle Abweichungen je Baustein: `docs/contract/*.md`, Abschnitt „Known differences“.
