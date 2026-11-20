# Integritätsprüfung

[English](en/integrity.md) · [Administration](administration.md#sanity-checker)

PaperRust prüft die Ablage regelmäßig auf Datenprobleme – wie paperless-ngx mit der
Konsistenzprüfung (*sanity checker*), plus einige Ergänzungen für große und langlebige Archive.

## Was geprüft wird

Für **jedes** Dokument (auch im Papierkorb und ältere Versionen):

| Prüfung | Meldung (unverändert wie paperless-ngx) | Stufe |
|---|---|---|
| Vorschaubild vorhanden und lesbar | `Thumbnail of document does not exist.` / `Cannot read thumbnail file of document: …` | Fehler |
| Original vorhanden, lesbar, SHA-256 = gespeicherte Prüfsumme | `Original of document does not exist.` / `Cannot read original file of document: …` / `Checksum mismatch. Stored: …, actual: ….` | Fehler |
| Archivdatei: Dateiname und Prüfsumme passen zusammen | `Document has an archive file checksum, but no archive filename.` / `Document has an archive file, but its checksum is missing.` | Fehler |
| Archivdatei vorhanden, lesbar, SHA-256 stimmt | `Archived version of document does not exist.` / `Cannot read archive file of document: …` / `Checksum mismatch of archived document. Stored: …, actual: ….` | Fehler |
| Inhalt (OCR-Text) leer | `Document contains no OCR data` | Info |

Außerdem: jede Datei unter `MEDIA_ROOT`, die zu keinem Dokument gehört (ohne `media.lock`, App-Logo
und Systemdateien wie `Thumbs.db`, `desktop.ini`, `.DS_Store`) → Warnung
`Orphaned file in media dir: <Pfad>`.

## Wann sie läuft

- **Automatisch** als Aufgabe `sanity_check`, Standard **sonntags 00:30**
  ([`PAPERRUST_SANITY_TASK_CRON`](configuration.md#paperrust_sanity_task_cron), `disable` = aus).
- **Von Hand** in der Oberfläche: *Einstellungen → Systemstatus → Sanity Check → Ausführen*, oder
  auf der Kommandozeile:

  ```sh
  paperrust manage document_sanity_checker               # vollständig (wie paperless-ngx)
  paperrust manage document_sanity_checker --incremental # nur geänderte Dateien hashen
  ```

## Wo das Ergebnis erscheint

- **Systemstatus** (*Einstellungen → Systemstatus*): „Sanity Check“ ist *OK*, wenn der letzte Lauf
  ohne Fehler endete, *Fehler* mit dem Ergebnistext (z. B. „2 document(s) with errors found. Check
  logs for details.“), *Warnung*, wenn noch nie geprüft wurde.
- **Aufgaben** (*Aufgaben* in der Oberfläche, Benachrichtigungen): Ein geplanter Lauf mit Fehlern
  erscheint als fehlgeschlagene Aufgabe.
- **Protokoll** `paperrust.log` (*Protokolle* in der Oberfläche): jede Einzelmeldung (Logger
  `paperrust.sanity_checker`, bei paperless-ngx `paperless.sanity_checker`) und von PaperRust zusätzlich eine Start- und eine
  Abschlusszeile (Logger `paperrust.integrity`), z. B.

  ```text
  [paperrust.integrity] Integrity check (incremental) finished in 41.2 s: 100204 document(s), 12 file(s) hashed (3.1 MiB), 197891 unchanged file(s) skipped, 0 orphaned file(s). Result: No issues detected. Report: …/data/integrity/integrity-20261108T003041.204Z.json
  ```

- **Bericht** (JSON, maschinenlesbar) je Lauf unter `DATA_DIR/integrity/`, siehe unten.

## Inkrementeller Modus (große Archive)

Standardmäßig liest jeder Lauf **alle** Originale und Archivdateien vollständig und berechnet
SHA-256 – wie paperless-ngx. Bei vielen Gigabyte dauert das. Mit

```sh
PAPERRUST_INTEGRITY_INCREMENTAL=true
```

merkt sich PaperRust nach jeder erfolgreichen Prüfung einer Datei Pfad, Größe, Änderungszeit und
Prüfsumme (Tabelle `paperrust_integrity_state` in der Datenbank). Beim nächsten Lauf wird eine Datei
nur dann nicht neu gelesen, wenn **alles** davon gleich ist und die Bestätigung jünger als
`PAPERRUST_INTEGRITY_FULL_HASH_DAYS` Tage (Standard 28) ist. Danach wird sie wieder vollständig
gehasht – so ist jede Datei spätestens nach dieser Frist wieder Bit für Bit geprüft.

Immer vollständig geprüft werden: Vorhandensein aller Dateien, Vorschaubilder, verwaiste Dateien,
leerer Inhalt, Dateiname/Prüfsumme-Paare. Wird eine Datei ersetzt, umbenannt, verkürzt oder
verlängert oder ändert sich die gespeicherte Prüfsumme (Dokument neu verarbeitet), wird sie sofort
neu gehasht.

**Grenze:** Eine stille Veränderung, die Größe *und* Änderungszeit unverändert lässt (z. B. „Bit
rot“ auf dem Datenträger), findet der inkrementelle Modus erst bei der nächsten vollständigen
Prüfung der Datei, also spätestens nach `PAPERRUST_INTEGRITY_FULL_HASH_DAYS` Tagen. Wer das nicht
möchte, setzt den Wert auf `0` (jeder Lauf hasht alles) oder lässt den Modus aus.

Die Kommandozeile prüft ohne `--incremental` immer vollständig. Ist der Modus eingeschaltet, frischt
auch ein vollständiger Lauf die gespeicherten Bestätigungen auf.

## Bericht

Jeder Lauf (geplant und von Hand) schreibt `DATA_DIR/integrity/integrity-<Zeit in UTC>.json`; die
neuesten `PAPERRUST_INTEGRITY_REPORT_KEEP` (Standard 10) bleiben, ältere werden gelöscht. Inhalt:

```json
{
  "format": 1,
  "started_at": "2026-11-08T00:30:00.012Z",
  "finished_at": "2026-11-08T00:30:41.204Z",
  "duration_seconds": 41.19,
  "trigger": "task",
  "mode": "incremental",
  "full_hash_days": 28,
  "threads": 4,
  "status": "error",
  "result": "1 document(s) with errors found. Check logs for details.",
  "documents": 100204,
  "files": { "hashed": 12, "skipped": 197891, "bytes_hashed": 3250585, "orphaned": 0 },
  "counts": { "document_errors": 1, "document_warnings": 0, "document_infos": 0, "global_warnings": 0, "messages": 1 },
  "issues": [
    { "level": "ERROR", "document_id": 4711, "title": "Rechnung 2019", "message": "Original of document does not exist." }
  ]
}
```

`status` ist `ok` (keine Meldung), `issues` (nur Warnungen/Infos) oder `error`. `trigger` ist
`task` (Worker) oder `manage` (Kommandozeile). Verwaiste Dateien stehen mit `document_id: null`.

## Einstellungen

| Variable | Standard | Bedeutung |
|---|---|---|
| `PAPERRUST_SANITY_TASK_CRON` | `30 0 * * sun` | Zeitplan (wie paperless-ngx), `disable` = aus |
| `PAPERRUST_INTEGRITY_INCREMENTAL` | `false` | inkrementeller Modus für den geplanten Lauf |
| `PAPERRUST_INTEGRITY_FULL_HASH_DAYS` | `28` | spätestens nach so vielen Tagen wird jede Datei wieder gehasht; `0` = immer |
| `PAPERRUST_INTEGRITY_THREADS` | `0` (automatisch, höchstens 4) | Dateien parallel lesen; `1` = nacheinander wie paperless-ngx |
| `PAPERRUST_INTEGRITY_REPORT_KEEP` | `10` | Anzahl aufbewahrter JSON-Berichte; `0` = keine Berichte |

Ungültige Werte lassen den geplanten Lauf nicht ausfallen: PaperRust schreibt eine Warnung ins
Protokoll und prüft vollständig.

## Laufzeit

Gemessen auf einem Windows-Laptop (Debug-Build, SSD, Virenscanner aktiv, parallel laufende Builds):

| Bestand | Lauf | Dauer |
|---|---|---|
| 204 Dokumente, 403 Dateien à 2 MiB (806 MiB) | vollständig, 1 Thread | 1,0 s |
| | vollständig, 4 Threads (Dateien im Cache) | 0,35 s (≈ 2,3 GB/s) |
| | inkrementell, nichts geändert | 0,18 s |
| 100 204 Dokumente, 300 000 kleine Dateien (synthetisch) | vollständig, 4 Threads | ≈ 20 min |

Bei kleinen Dateien bestimmt das Öffnen jeder Datei die Dauer (unter Windows mit Virenscanner
einige Millisekunden je Datei) – das fällt in beiden Modi an, weil Vorhandensein und Vorschaubilder
immer geprüft werden. Der inkrementelle Modus spart das Lesen der Inhalte: bei 100 000 Dokumenten
mit je ~1 MB Original und Archiv rund 200 GB je Lauf, also je nach Datenträger (HDD ~150 MB/s,
SSD 1–2 GB/s) zwischen einigen Minuten und einigen Stunden.

## Was tun bei Meldungen?

- **Checksum mismatch / Datei fehlt:** Datei aus der Sicherung zurückholen (siehe
  [Administration](administration.md)); ist das Original verloren, aber das Archiv in Ordnung, lässt
  sich die Archivdatei als Original verwenden.
- **Thumbnail fehlt:** `paperrust manage document_thumbnails -d <ID>` erzeugt es neu.
- **Verwaiste Datei:** prüfen und von Hand löschen oder als neues Dokument einlesen.
- **No OCR data:** Dokument in der Oberfläche *Erneut verarbeiten*.
