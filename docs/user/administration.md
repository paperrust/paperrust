# Administration

[English](en/administration.md) · [Übersicht](README.md)

<a id="backup"></a>

## Sicherung

Zu sichern sind drei Dinge – zusammen, im selben Zustand:

1. **Datenbank** (PostgreSQL): `pg_dump -Fc -U paperless paperless > paperless.pgdump`.
   Läuft konsistent auch bei laufendem PaperRust (MVCC).
2. **`MEDIA_ROOT`** (`originals/`, `archive/`, `thumbnails/`): die Dokumentdateien.
3. **`DATA_DIR`**: Suchindex, Klassifikator, KI-Index. Lässt sich notfalls neu aufbauen, eine
   Sicherung spart aber Zeit.

Zusätzlich oder alternativ: der [Exporter](#exporter). Er schreibt alle Daten und Dateien in einen
Ordner (oder ein ZIP) im Format von paperless-ngx – die Sicherung lässt sich also auch in
paperless-ngx wieder einlesen.

```sh
paperrust manage document_exporter /sicherung/export --zip
```

Desktop-App: `paperrust-desktop backup [ZIEL]` erstellt `pg_dump` und Export in einem Schritt
([docs/DESKTOP.md](../DESKTOP.md)).

<a id="migrating-restoring"></a>

### Wiederherstellen

- Aus `pg_dump`: leere Datenbank anlegen, `pg_restore --no-owner -d paperless paperless.pgdump`,
  `media/` und `data/` zurückkopieren, `paperrust manage migrate`, starten.
- Aus einem Export: leere Datenbank, `paperrust manage migrate`, dann
  `paperrust manage document_importer /sicherung/export`.
- Docker: zurückkopierte `media/`- und `data/`-Ordner müssen dem Benutzer des Containers gehören (UID 1000
  bzw. `USERMAP_UID`), z. B. `chown -R 1000:1000 media data`. Sonst kann PaperRust keine Dateien schreiben; die
  Integritätsprüfung (`document_sanity_checker`) meldet das.
- Getestet am 2026-11-20 auf einem frischen System: Export, `pg_dump` und `backup restore` stellen alle Dokumente,
  Dateien (byte-gleich), Metadaten, Rechte und die Suche vollständig wieder her.

<a id="updating"></a>

## Aktualisieren

Siehe [Installation → Aktualisieren](installation.md#updating). Kurz: sichern, Binary ersetzen,
`paperrust manage migrate`, neu starten.

<a id="management-commands"></a><a id="manage"></a>

## Verwaltungsbefehle: `paperrust manage`

Wartungsarbeiten erledigen Sie mit demselben Programm, das auch den Server startet:

```sh
paperrust manage <befehl> [optionen]
```

Im Docker-Container: `docker compose exec <dienst> paperrust manage <befehl>`.

- Die Einstellungen kommen wie beim Server aus der Umgebung und aus `paperrust.conf` (`paperless.conf` wird weiter gelesen).
- Rückgabewert: `0` alles gut, `1` Fehler (Meldung beginnt mit `CommandError:`), `2` falscher
  Aufruf.
- Die Ausgabe besteht aus einfachen Textzeilen, ohne Fortschrittsbalken.
- Für Umsteiger: Die Befehle heißen wie bei paperless-ngx (`manage.py …`); übliche Zusatzoptionen
  wie `-v` oder `--no-color` werden angenommen.

| Befehl | Zweck | Stand |
|---|---|---|
| `migrate [--check] [--plan] [--skip-paperrust]` | Schema anlegen bzw. prüfen, `paperrust_*`-Tabellen einspielen | fertig |
| `showmigrations` | Migrationsliste | fertig |
| `createsuperuser`, `manage_superuser`, `changepassword` | Administratoren und Passwörter | fertig |
| `document_exporter` | [Export](#exporter) | fertig |
| `document_importer` | [Import](#importer) | fertig |
| `document_sanity_checker` | [Konsistenzprüfung](#sanity-checker) | fertig |
| `document_retagger` | [Zuordnung neu anwenden](#retagger) | fertig |
| `document_index reindex\|optimize` | [Suchindex](#index) | fertig |
| `document_create_classifier` | Klassifikator trainieren | fertig |
| `document_thumbnails [-d ID]` | [Vorschaubilder neu erzeugen](#thumbnails) (PDFium nötig) | fertig |
| `document_renamer` | [Dateinamen neu anwenden](#renamer) | fertig |
| `prune_audit_logs` | [Verlauf aufräumen](#prune-history) | fertig |
| `empty_trash [--doc-ids 1,2]` | Papierkorb leeren | fertig (PaperRust-Zusatz) |
| `mail_fetcher` | [Mails sofort abrufen](#mail) | fertig |
| `document_archiver [-f] [-d ID] [--processes N]` | Archiv-PDFs (neu) erzeugen | fertig |
| `document_consumer [ordner] [--oneshot]` | Eingangsordner einmalig einlesen bzw. beobachten | fertig |
| `document_fuzzy_match [--ratio R] [--delete]` | fast gleiche Dokumente finden | fertig |
| `document_llmindex rebuild\|update\|compact\|migrate` | KI-Index pflegen (nur mit KI) | fertig |
| `check [--fail-level LEVEL]` | Systemprüfungen (Ordner, Programme, Einstellungen) | fertig |
| `convert_mariadb_uuid` | – | entfällt (nur MariaDB, siehe unten) |

Dazu kommen Befehle, die es nur in PaperRust gibt:

| Befehl | Zweck |
|---|---|
| `settings list\|get\|set\|reset\|export\|import\|rekey\|origin\|adopt\|slim-env\|check-file` | Systemeinstellungen ohne Oberfläche ändern, sichern, umziehen ([Konfiguration](configuration.md#settings-ui)) |
| `backup init\|run\|list\|check\|restore` | verschlüsselte Sicherung mit Versionen ([Schutz vor Ransomware](ransomware.md)) |
| `guard status\|lift [--note TEXT]\|scan` | Ransomware-Wächter: Zustand, Nur-Lese-Modus aufheben, Archiv prüfen ([Schutz vor Ransomware](ransomware.md)) |
| `datamodel status\|check\|rollback --to <stufe>` | Stand des PaperRust-Datenmodells anzeigen, prüfen, Umbaustufen zurückdrehen ([docs/DATAMODEL-V2.md](../DATAMODEL-V2.md)) |
| `sync status\|enable\|disable [--purge]\|rebuild\|reset-identity` | Abgleich für Desktop- und Mobil-Apps (`reset-identity` nach dem Kopieren einer Datenbank) |
| `grants status\|check\|rebuild` | Freigabe-Index prüfen bzw. neu aufbauen (nur zur Fehlersuche) |
| `timeline_backfill [--force]` | Fristen im Text aller Dokumente für den Zeitstrahl erkennen (`PAPERRUST_TIMELINE_EXTRACT_DATES`) |

Die Liste aller Befehle zeigt `paperrust manage --help`. Hilfe zu einem einzelnen Befehl
(`manage <befehl> --help`) gibt es bisher nur bei `sync` und `grants`; die Optionen stehen hier.

Den Eingangsordner überwacht im Betrieb `paperrust serve --worker` selbst; `document_consumer`
braucht man nur für einmalige Läufe (z. B. per Cron mit `--oneshot`).

<a id="exporter"></a>

### Exporter

```
paperrust manage document_exporter ZIEL [-c] [-cj] [-d] [-f] [-na] [-nt] [-p] [-sm] [-z]
    [-zn NAME] [--zip-compression stored|deflated|bzip2|lzma|zstd] [--zip-compression-level N]
    [--data-only] [--passphrase PASS] [--batch-size 500]
```

| Option | Wirkung |
|---|---|
| `-c`, `--compare-checksums` | Dateien per SHA-256 statt Größe/Zeit vergleichen |
| `-cj`, `--compare-json` | Manifest nur schreiben, wenn es sich geändert hat |
| `-d`, `--delete` | Dateien im Ziel löschen, die nicht mehr zum Bestand gehören |
| `-f`, `--use-filename-format` | Dateinamen nach `PAPERRUST_FILENAME_FORMAT` |
| `-na`, `-nt` | ohne Archivdateien bzw. ohne Vorschaubilder |
| `-p`, `--use-folder-prefix` | Unterordner `originals/`, `archive/`, `thumbnails/`, `json/` |
| `-sm`, `--split-manifest` | ein Manifest je Dokument |
| `-z`, `--zip`, `-zn` | als ZIP (Standardname `export-<Datum>`) |
| `--data-only` | nur Datenbankinhalt, keine Dateien |
| `--passphrase` | Mail-Passwörter und Tokens verschlüsseln (ohne: Klartext, mit Hinweis) |

Der Export läuft in einer lesenden Transaktion (`REPEATABLE READ`) und ist mit paperless-ngx v3.4.2
austauschbar. ZIP-Kompression wie bei paperless-ngx wählbar (`--zip-compression`, auch bzip2, lzma,
zstd). Abweichung: `--zip-name` muss ein reiner Dateiname sein.

<a id="importer"></a>

### Importer

```
paperrust manage document_importer QUELLE [--data-only] [--passphrase PASS] [--batch-size 500]
```

`QUELLE` ist ein Exportordner oder ein ZIP. Ziel sollte eine frische Installation sein (leere DB
nach `paperrust manage migrate`). Der Import läuft in **einer** Transaktion, danach werden die
Dateien kopiert und der Suchindex neu aufgebaut. PaperRust prüft dabei streng:
Dateinamen im Manifest müssen relativ und innerhalb der Medienordner bleiben, verwiesene Dateien
innerhalb des Importordners; ZIPs werden höchstens bis `PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES`
entpackt (Standard 1 GiB + 100 × Archivgröße). Siehe [Sicherheit](security.md).

<a id="sanity-checker"></a>

### Konsistenzprüfung

```sh
paperrust manage document_sanity_checker
```

Prüft für alle Dokumente Vorschaubild, Original (inkl. SHA-256), Archivdatei, leeren Inhalt und
findet verwaiste Dateien unter `MEDIA_ROOT`. Am Ende steht eine Zusammenfassung, z. B. „Found 2
document(s) with errors and 1 global warning(s).“ Die Prüfung läuft außerdem jede Woche von selbst
(siehe [Aufgaben und Zeitplan](#tasks)); Ergebnisse unter *Aufgaben* in der Oberfläche.
Inkrementeller Modus für große Archive, JSON-Bericht je Lauf und Einstellungen:
[Integritätsprüfung](integrity.md).

<a id="retagger"></a>

### Zuordnung neu anwenden (Retagger)

```sh
paperrust manage document_retagger [-c] [-T] [-t] [-s] [-i] [--use-first] [-f] [--suggest] [--id-range VON BIS]
```

`-c` Korrespondent, `-T` Tags, `-t` Dokumenttyp, `-s` Speicherpfad; `-i` nur Posteingang, `-f`
überschreibt vorhandene Werte, `--suggest` zeigt nur an. Nutzt das PaperRust-Klassifikatormodell.

<a id="classifier"></a>

### Klassifikator

```sh
paperrust manage document_create_classifier
```

Trainiert das Modell für die automatische Zuordnung (`DATA_DIR/paperrust_classifier.model`). Ohne
Tags/Korrespondenten/Typen mit Zuordnung „Automatisch“ wird nichts trainiert („No automatic
matching items, not training“). Läuft auch stündlich von selbst.

<a id="thumbnails"></a>

### Vorschaubilder

```sh
paperrust manage document_thumbnails [-d ID]
```

Erzeugt Vorschaubilder neu (WebP). Ohne PDFium entsteht das Standardbild.

<a id="index"></a>

### Suchindex

```sh
paperrust manage document_index reindex [--recreate] [--if-needed]
paperrust manage document_index optimize      # tut nichts, nur aus Kompatibilität vorhanden
```

Der Suchindex wird bei jeder Änderung sofort nachgeführt. Er hat dasselbe Format wie bei
paperless-ngx (tantivy). Neu aufbauen bei beschädigtem Index oder nach Wechsel von `PAPERRUST_SEARCH_LANGUAGE`.

<a id="llm-index"></a>

### KI-Index

Den Index für die KI-Funktionen (Chat, Vorschläge mit ähnlichen Dokumenten) hält PaperRust selbst
aktuell: bei jeder Dokumentänderung und einmal täglich nach Zeitplan
(`PAPERRUST_LLM_INDEX_TASK_CRON`). Ändern Sie die Embedding-Einstellungen in der Oberfläche, wird
er neu aufgebaut. Von Hand anstoßen: im Dialog *Systemstatus* beim Eintrag *KI-Index* auf
„Aufgabe starten“ klicken oder `paperrust manage document_llmindex update` (bzw. `rebuild`). Mehr dazu: [KI](ai.md).

<a id="renamer"></a>

### Dateinamen neu anwenden

```sh
paperrust manage document_renamer
```

Nach einer Änderung von `PAPERRUST_FILENAME_FORMAT` oder Speicherpfaden: benennt Originale und
Archivdateien aller Dokumente nach der aktuellen Vorlage um ([Dateinamen](storage-paths.md)).

<a id="mail"></a>

### Mails abrufen

```sh
paperrust manage mail_fetcher
```

Ruft alle Mail-Konten sofort ab, statt auf den nächsten Zeitpunkt nach `PAPERRUST_EMAIL_TASK_CRON`
zu warten ([E-Mail-Eingang](mail.md)).

<a id="prune-history"></a>

### Verlauf aufräumen

```sh
paperrust manage prune_audit_logs
```

Löscht Audit-Log-Einträge von Objekten, die es nicht mehr gibt.

<a id="empty-trash"></a>

### Papierkorb leeren

```sh
paperrust manage empty_trash [--doc-ids 1,2,3]
```

Löscht Dokumente endgültig, die länger als `PAPERRUST_EMPTY_TRASH_DELAY` Tage im Papierkorb liegen
(bzw. die genannten IDs). Läuft auch täglich von selbst.

<a id="create-superuser"></a>

### Administrator anlegen

```sh
paperrust manage createsuperuser                # interaktiv
DJANGO_SUPERUSER_PASSWORD=… paperrust manage createsuperuser --username admin --email a@b.ch --no-input
paperrust manage manage_superuser               # aus PAPERRUST_ADMIN_USER/_MAIL/_PASSWORD
paperrust manage changepassword admin
```

Achtung: Das Passwort ist beim Eintippen sichtbar. Niemand sollte mitlesen; alternativ den Weg
über `DJANGO_SUPERUSER_PASSWORD` bzw. `PAPERRUST_ADMIN_PASSWORD` nehmen.

<a id="archiver"></a>

### Archiv-PDFs neu erzeugen

```sh
paperrust manage document_archiver [-f] [-d ID] [--processes N]
```

Erzeugt Archiv-PDFs für Dokumente, die noch keins haben. `-f` erzeugt sie für alle neu (z. B. nach
dem Einschalten von PDF/A oder Säubern), `-d ID` nur für ein Dokument, `--processes N` mit N
gleichzeitigen Läufen.

<a id="fuzzy_duplicate"></a>

### Fast gleiche Dokumente finden

```sh
paperrust manage document_fuzzy_match [--ratio 85] [--delete [--yes]]
```

Vergleicht die Texte aller Dokumente und listet Paare, die sich zu mindestens `--ratio` Prozent
gleichen (Standard 85). `--delete` legt jeweils das zweite Dokument in den Papierkorb und fragt vorher
nach (`--yes` ohne Nachfrage). Erst ohne `--delete` ansehen!

<a id="consumer"></a>

### Eingangsordner von Hand einlesen

```sh
paperrust manage document_consumer [ordner] [--oneshot]
```

Liest den Eingangsordner (oder den genannten Ordner) ein. Mit `--oneshot` einmal und dann Ende,
sonst beobachtet der Befehl den Ordner weiter. Im Normalbetrieb erledigt das `paperrust serve --worker`.

<a id="check"></a>

### Systemprüfung

```sh
paperrust manage check [--fail-level WARNING]
```

Prüft Ordner, Programme (Tesseract, PDFium) und Einstellungen und nennt fehlende Teile mit Hinweis.
Mit `--fail-level WARNING` endet der Befehl schon bei Warnungen mit Fehlercode (für Skripte).

### Entfallen

| Befehl (paperless-ngx) | Ersatz in PaperRust |
|---|---|
| `convert_mariadb_uuid` | entfällt (nur MariaDB) |
| `decrypt_documents` | entfällt (gibt es auch in paperless-ngx 3.x nicht mehr) |

<a id="tasks"></a>

## Aufgaben und Zeitplan

Für Arbeit im Hintergrund braucht PaperRust keine zusätzlichen Dienste – kein Celery, kein Redis.
Mit `paperrust serve --worker` erledigt dasselbe Programm neben der Weboberfläche auch:

- das Einlesen neuer Dateien (Eingangsordner, Upload, E-Mail, Scanner),
- Mail-Abruf, Workflows und KI-Verarbeitung,
- die regelmäßigen Aufgaben aus der Tabelle unten.

**Die Warteschlange liegt in PostgreSQL**, in eigenen Tabellen (`paperrust_*`). Das heißt:

- Startet PaperRust neu, geht keine Aufgabe verloren; offene Aufgaben werden danach abgearbeitet.
- Bricht ein Worker mitten in einer Aufgabe ab, wird sie erneut zugestellt – höchstens
  `PAPERRUST_QUEUE_MAX_DELIVERIES` Mal (Standard 3). Danach gilt sie als fehlgeschlagen.
- Wie viele Aufgaben gleichzeitig laufen, legt `PAPERRUST_TASK_WORKERS` fest (Standard 1); eine
  einzelne Aufgabe darf höchstens `PAPERRUST_WORKER_TIMEOUT` Sekunden dauern (Standard 1800).

Laufende und erledigte Aufgaben sehen Sie unter *Aufgaben* und im Dialog *Systemstatus*.

**Zeitplan**

| Aufgabe | Standard | Einstellung |
|---|---|---|
| Mails abrufen | alle 10 Minuten | `PAPERRUST_EMAIL_TASK_CRON` |
| Klassifikator trainieren | stündlich (Minute 5) | `PAPERRUST_TRAIN_TASK_CRON` |
| Geplante Workflows prüfen | stündlich (Minute 5) | `PAPERRUST_WORKFLOW_SCHEDULED_TASK_CRON` |
| Papierkorb leeren | täglich 01:00 | `PAPERRUST_EMPTY_TRASH_TASK_CRON` |
| KI-Index aktualisieren | täglich 02:10 | `PAPERRUST_LLM_INDEX_TASK_CRON` |
| Abgelaufene Anmeldungen löschen | täglich 03:00 | `PAPERRUST_SESSION_CLEANUP_TASK_CRON` |
| Konsistenzprüfung | sonntags 00:30 | `PAPERRUST_SANITY_TASK_CRON` |

Die Werte sind Cron-Ausdrücke (z. B. `0 */2 * * *` = alle zwei Stunden); `disable` schaltet eine
Aufgabe ab. Die Zeiten gelten in der Zeitzone `PAPERRUST_TIME_ZONE`.

**Mehrere Rechner:** Auf weiteren Rechnern kann `paperrust serve --worker-only` laufen (nur
Hintergrundarbeit, keine Weboberfläche). Alle müssen dieselbe Datenbank und dieselben
Dokumentordner sehen. Ein Zeitplan-Eintrag läuft trotzdem nur einmal, egal wie viele Worker es
gibt.

Ohne `--worker` (das ist der Standard, ausdrücklich: `--no-worker`) läuft nur die Weboberfläche.
Aufgaben bleiben dann in der Warteschlange liegen, bis irgendwo ein Worker läuft.
