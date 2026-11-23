# Von paperless-ngx umsteigen

[Übersicht](README.md) · Technische Einzelheiten: [Umstieg (ausführlich)](migration.md)

Diese Seite erklärt in einfachen Schritten, wie Sie Ihre Dokumente aus paperless-ngx in PaperRust
übernehmen. Sie brauchen dafür **einen Befehl**. PaperRust prüft vorher alles, zeigt den
Fortschritt und sagt Ihnen am Ende genau, ob alles angekommen ist.

> **Bitte gesund skeptisch sein.** PaperRust 0.1.0 ist eine erste Version. Der Kern ist gründlich geprüft –
> aber nicht mit Ihren Dokumenten. Vor dem Umstieg: eine **vollständige Sicherung** von paperless-ngx machen
> (Export oder Datenbank und Medienordner), PaperRust zuerst **mit einer Kopie ausprobieren** und paperless-ngx
> erst abschalten, wenn alles passt. Nichts blind übernehmen. Der Weg zurück zu paperless-ngx bleibt offen.

## Das Wichtigste in Kürze

- **Ihre alten Daten bleiben erhalten.** PaperRust liest paperless-ngx nur. Ändert PaperRust doch
  einmal etwas an Ihrer bestehenden Datenbank, macht es vorher selbst eine Sicherung.
- **Welche Version?** paperless-ngx **2.20**, **3.0 bis 3.4** und **3.5 (bis 3.5.1)** übernimmt
  PaperRust direkt. Sie müssen paperless-ngx vorher
  **nicht** aktualisieren. Nur noch ältere 2.x-Versionen (vor 2.19.6) gehen über einen Export.
  **paperless-ngx 1.x:** zuerst auf 2.20 aktualisieren und einmal starten; ein Export aus 1.x lässt
  sich nicht übernehmen (auch nicht in paperless-ngx 3).
- **Was kommt mit?** Alle Dokumente mit Dateien, Tags, Korrespondenten, Dokumenttypen,
  Speicherpfade, eigene Felder, Notizen, gespeicherte Ansichten, Workflows, Mail-Regeln,
  Benutzer **mit ihren Passwörtern** und Rechte.
- **Am Ende steht ein Abgleich**: wie viele Dokumente, Tags und Benutzer vorher da waren und wie
  viele jetzt da sind, und ob jede Datei gefunden wurde.

## Vorher: zwei Minuten Vorbereitung

1. **paperless-ngx anhalten.** Damit während des Umstiegs nichts Neues dazukommt.
   Docker: `docker compose down`.
2. **Sichern** (empfohlen, auch wenn PaperRust Ihre Daten nicht verändert): die Datenbank und die
   Ordner `data` und `media`. Docker mit PostgreSQL:

   ```bash
   docker compose up -d db
   docker compose exec db pg_dump -U paperless -Fc paperless > paperless-vorher.pgdump
   ```

3. **Ausprobieren ohne Risiko:** Jeder Befehl unten kann mit `--dry-run` laufen. Dann prüft
   PaperRust nur und schreibt nichts.

## Fall 1: paperless-ngx in Docker mit PostgreSQL (häufigster Fall)

So läuft es zum Beispiel bei paperless-ngx 3.1.3 mit PostgreSQL 17.

1. Vorbereitung wie oben (anhalten, sichern).
2. In Ihrer `docker-compose.yml` das Bild von paperless-ngx durch PaperRust ersetzen. Eine fertige
   Vorlage liegt unter `docker/docker-compose.migrate-from-paperless.yml`. Die Volumes `data`,
   `media` und die Datenbank bleiben, wie sie sind. Wichtig: **derselbe `PAPERLESS_SECRET_KEY`**
   wie bisher.
3. Probelauf:

   ```bash
   docker compose run --rm webserver paperrust migrate --dry-run \
       --from "postgres://paperless:PASSWORT@db:5432/paperless" --media /usr/src/paperless/media
   ```

4. Umstieg (derselbe Befehl ohne `--dry-run`). Ist Ihre paperless-ngx-Version älter als 3.4.2
   (auch 2.20),
   macht PaperRust zuerst **automatisch eine Sicherung** und bringt die Datenbank dann auf den
   neuen Stand. Die Sicherung ist eine vollständige Kopie Ihrer Datenbank auf dem
   Datenbank-Server (Name z. B. `paperless_vor_umstieg_20261110_143000`) – oder, wenn das
   Programm `pg_dump` vorhanden ist, eine Datei `paperless-vor-umstieg-….pgdump` im Ordner
   `data/umstieg`. Der Abgleich nennt den Namen.
5. Starten: `docker compose up -d`. Anmelden mit Ihren bisherigen Benutzern und Passwörtern.

Statt Schritt 3 und 4 können Sie PaperRust bei paperless-ngx 3.x auch einfach starten: beim Start
holt PaperRust fehlende Änderungen einer älteren Version selbst nach (wie paperless-ngx bei einem
Update). Den Abgleich bekommen Sie dann aber nur über den Befehl oben. **paperless-ngx 2.20** stellt
PaperRust nur mit dem Befehl oben um, weil dabei mehr passiert (siehe
[2.20 übernehmen](#v2)); beim einfachen Start erscheint ein Hinweis darauf.

**Lieber eine Kopie?** Wenn Ihre bisherige Datenbank gar nicht angefasst werden soll, legen Sie
eine neue, leere Datenbank an und geben Sie sie als Ziel an:
`--target "postgres://paperless:PASSWORT@db:5432/paperrust"`. PaperRust kopiert dann alles hinein
und prüft jede Tabelle. paperless-ngx kann jederzeit mit der alten Datenbank weiterlaufen.

<a id="fall-2-desktop-app-windows"></a><a id="desktop"></a>

## Fall 2: Desktop-App (Windows)

1. paperless-ngx anhalten.
2. PaperRust installieren und starten. Beim ersten Start fragt PaperRust, ob Sie neu anfangen
   oder **von paperless-ngx umsteigen** wollen.
3. „Von paperless-ngx umsteigen“ wählen und **einen** Ort angeben: den paperless-Ordner (mit
   `data` und `media`), eine Export-Datei (`.zip`) oder die Adresse Ihrer Datenbank.
4. Warten. Danach zeigt PaperRust den **Abgleich**: Ergebnis (OK, WARN oder FAIL), eine
   Kurzfassung (Dokumente, Tags, Benutzer, Dateien) und die ersten Hinweise. Mit **„Ganzen Bericht
   öffnen“** sehen Sie alle Einzelheiten, mit **„PaperRust öffnen“** geht es los. Läuft der Abgleich
   noch, können Sie PaperRust schon öffnen; der Bericht liegt danach als `umstieg-bericht.html` im
   Log-Ordner der App.
5. Bei einer Datenbank von paperless-ngx 2.x den alten Medienordner mit angeben: PaperRust braucht
   die Dateien, um die Prüfsummen neu zu berechnen.

## Fall 3: Umzug auf einen neuen Rechner (Export)

Wenn PaperRust auf einem anderen Rechner laufen soll, ist der Export der einfachste Weg. Dabei
wandern die Dateien mit.

1. Auf dem alten Rechner in paperless-ngx exportieren:

   ```bash
   docker compose exec webserver document_exporter ../export
   ```

2. Den Ordner `export` (oder eine ZIP-Datei davon) auf den neuen Rechner kopieren.
3. Auf dem neuen Rechner:

   ```bash
   paperrust migrate --from /pfad/zum/export
   ```

Hinweis: Ein Export enthält keine API-Schlüssel (Apps müssen sich neu verbinden) und nicht die
Liste schon abgeholter Mails. Alles andere ist dabei.

## Fall 4: paperless-ngx mit SQLite oder MariaDB/MySQL

PaperRust arbeitet mit PostgreSQL. Ihre Daten werden dabei in eine neue PostgreSQL-Datenbank
kopiert; die alte bleibt unverändert.

- **SQLite** (Standard bei paperless-ngx ohne eigene Datenbank): geben Sie einfach den
  paperless-Ordner an, PaperRust findet `db.sqlite3` und `media` selbst:
  `paperrust migrate --from /pfad/zu/paperless --target "postgres://…/paperrust"`.
- **MariaDB/MySQL**: `--from "mariadb://benutzer:passwort@rechner:3306/paperless" --media
  /pfad/zu/media --target "postgres://…/paperrust"`.

Eine leere PostgreSQL-Datenbank brauchen Sie dafür vorher (bei der Desktop-App ist sie schon
dabei).

<a id="v2"></a>

## paperless-ngx 2.20 übernehmen

paperless-ngx 3 hat seine Datenbank gegenüber 2.x deutlich umgebaut. PaperRust macht diesen Umbau
selbst – mit denselben Schritten wie paperless-ngx 3 –, Sie müssen also **nicht** erst auf
paperless-ngx 3 aktualisieren.

- **Welche 2.x?** 2.20 (alle Unterversionen) und 2.19.6. paperless-ngx selbst verlangt für Version
  3 sogar genau 2.20.15; PaperRust holt den einen fehlenden Schritt älterer 2.20-Stände mit nach.
  Ältere 2.x: zuerst in paperless-ngx auf 2.20.15 aktualisieren und einmal starten – oder ein
  Export (Fall 3).
- **Wie?** Genau wie oben, mit dem Befehl `paperrust migrate` (Fall 1), in der Desktop-App (Fall 2)
  oder mit SQLite/MariaDB (Fall 4). Geben Sie den **Medienordner** mit an (`--media …`): PaperRust
  berechnet dabei für jede Datei die Prüfsumme neu (SHA-256 statt MD5, wie paperless-ngx 3). Bei
  vielen Dokumenten dauert das etwas.
- **Was ändert sich?** Gespeicherte Ansichten: ob eine Ansicht auf der Startseite oder in der
  Seitenleiste erscheint, steht danach in den Benutzereinstellungen (wie in paperless-ngx 3).
  Alte Einträge der Aufgabenliste („Aufgaben“) entfallen – auch paperless-ngx 3 löscht sie.
  Der OCR-Modus „skip“ heißt danach „auto“.
- **Verschlüsselte Dokumente** (alte GPG-Verschlüsselung) kann weder paperless-ngx 3 noch PaperRust
  lesen. PaperRust bricht dann ab, bevor etwas verändert wird. Bitte zuerst in paperless-ngx 2.20
  entschlüsseln (`decrypt_documents`).
- **Gleiche Datenbank:** auch hier sichert PaperRust vorher automatisch (siehe Fall 1).

<a id="newer"></a>

## Neuere paperless-ngx-Versionen (3.5)

paperless-ngx 3.5.0 und 3.5.1 speichern den Tag-Baum schon so, wie PaperRust es auch tut, und kennen
Notizen als Markdown – PaperRust auch. Eine solche Datenbank übernimmt PaperRust direkt, auf allen
Wegen oben; Markdown-Notizen bleiben Markdown-Notizen. Auch ein Export aus paperless-ngx 3.5 lässt
sich übernehmen. Kommt
paperless-ngx mit einer Änderung, die PaperRust noch nicht kennt, bricht der Umstieg ab, **bevor**
etwas verändert wird, und nennt den Ausweg: einen Export machen (Fall 3).

## Der Abgleich am Ende

Nach jedem Lauf zeigt PaperRust eine Kurzfassung, zum Beispiel:

```text
Ergebnis: OK – Alles übernommen und geprüft.

Kurzfassung:
  Dokumente: alle 204 übernommen
  Tags: alle 20 übernommen
  Benutzer: alle 5 übernommen
  Dateien im Ziel: 590 gefunden und in Ordnung, 0 fehlen, 0 beschädigt
```

| Ergebnis | Bedeutung | Was tun? |
|---|---|---|
| **OK** | Alles ist angekommen und geprüft. | Nichts. PaperRust starten. |
| **WARN** | Übernommen, aber mit Hinweisen. Oft war eine Datei **schon in paperless-ngx** kaputt oder es fehlt ein Vorschaubild. | Hinweise lesen. Meist harmlos. |
| **FAIL** | Etwas fehlt oder stimmt nicht – oder der Umstieg war nicht möglich. | Ihre alten Daten sind unverändert. Den Bericht lesen oder weitergeben. |

Den vollständigen Bericht finden Sie als `umstieg-bericht.html` (zum Anschauen),
`umstieg-bericht.txt` und `umstieg-bericht.json` im Berichtsordner (Docker: `data/umstieg`,
sonst der aktuelle Ordner oder `--report-dir`).

## Wenn etwas nicht klappt

| Meldung | Bedeutung | Lösung |
|---|---|---|
| „zu wenig Platz für die Dateien“ | Auf dem Ziel ist nicht genug Speicher frei. | Platz schaffen oder mit `--target-media` einen anderen Ordner wählen. |
| „Ziel-Datenbank nicht erreichbar“ | Adresse, Passwort oder Datenbank stimmt nicht, oder der Datenbankdienst läuft nicht. | Angaben prüfen; Docker: `docker compose up -d db`. |
| „Ziel-Datenbank ist nicht leer“ | Beim Kopieren muss das Ziel leer sein. PaperRust überschreibt nie etwas. | Eine neue, leere Datenbank anlegen. |
| „zu alten paperless-ngx-Version (älter als 2.20)“ | paperless-ngx vor 2.19.6 wird nicht direkt übernommen. | paperless-ngx zuerst auf 2.20.15 aktualisieren und einmal starten – oder einen Export machen (Fall 3). |
| „neueren paperless-ngx-Version“ | Ihre paperless-ngx hat eine Änderung, die PaperRust noch nicht kennt. | Export machen (Fall 3). Ihre Datenbank ist unverändert. |
| „verschlüsselt (GPG)“ | paperless-ngx 2.x hat Dokumente verschlüsselt gespeichert. | In paperless-ngx 2.20 `decrypt_documents` ausführen, dann erneut. |
| „This is a paperless-ngx 2.x database“ (beim Start) | PaperRust wurde direkt auf einer 2.x-Datenbank gestartet. | Einmal `paperrust migrate --from … --media …` ausführen (Fall 1). |
| „konnte nicht selbst sichern“ | Weder `pg_dump` noch eine Kopie auf dem Server war möglich (z. B. fehlendes Recht oder paperless-ngx läuft noch). | paperless-ngx ganz anhalten und erneut versuchen – oder selbst sichern (siehe oben) und den Befehl mit `--no-backup` wiederholen. |

<a id="after"></a>

## Nach dem Umstieg: was anders ist

- **Oberfläche:** PaperRust öffnet sich im PaperRust-Design. Wer die gewohnte Ansicht möchte, wählt
  unter *Einstellungen → Darstellung* die **klassische Oberfläche** – sie sieht aus wie paperless-ngx.
  Der **Rundgang** (Benutzermenü) zeigt die neuen Stellen. Workflows und Mail-Regeln stehen im neuen
  Bereich [Automatik](automatik.md).
- **Automatische Zuordnung neu lernen:** PaperRust speichert das gelernte Modell in einem eigenen
  Format. Es lernt nach dem Umstieg von selbst neu (stündlich); sofort geht es mit
  `paperrust manage document_create_classifier`. Ihre Regeln (Text enthält …) gelten unverändert.
- **Zusatzprogramme fallen weg:** Redis/Valkey, Tika, Gotenberg und die Celery-Worker werden nicht mehr
  gebraucht. paperless-ai oder paperless-gpt können Sie durch die eingebaute [KI](ai.md#replaces)
  ersetzen (Einstellungen einmal neu vornehmen).
- **Einstellungen:** Ihre `PAPERLESS_*`-Variablen und `paperless.conf` gelten weiter. Was ohne
  Wirkung ist (z. B. `PAPERLESS_REDIS`), meldet das Log einmal. In die Oberfläche übernehmen:
  [Konfiguration → Umzug](configuration.md#settings-ui).
- **Sprachen:** Die Oberfläche gibt es in 10 Sprachen (siehe unten).
- **API und Apps:** dieselbe REST-API wie paperless-ngx 3.4.2. Apps und Skripte für paperless-ngx
  verbinden sich wie bisher (nach einem Export-Umzug mit neuem API-Schlüssel).
- Bekannte Unterschiede: [Release-Notizen → Bekannte Grenzen](../RELEASE-NOTES-0.1.0.md#grenzen).

## Zurück zu paperless-ngx

- **Kopie (neue Datenbank) oder Export:** paperless-ngx einfach wieder mit der alten Datenbank
  starten. Dort hat sich nichts verändert.
- **Gleiche Datenbank:** PaperRust hat die Datenbank geändert (Tags ohne die Hilfsspalten von
  django-treenode). paperless-ngx kann sie **nicht mehr öffnen** – auch nicht paperless-ngx 3.4.2.
  Entweder die Sicherung zurückholen: bei einer Kopie auf dem Server in der `docker-compose.yml`
  `PAPERLESS_DBNAME` auf den Namen der Kopie setzen (z. B. `paperless_vor_umstieg_20261110_143000`);
  bei einer Datei `pg_restore -U paperless -d paperless --clean paperless-vor-umstieg-….pgdump`.
  Dabei geht alles verloren, was seit dem Umstieg in PaperRust dazukam.
- **Alles mitnehmen, was Sie in PaperRust gemacht haben:** `paperrust manage document_exporter
  /export`, dann in einer **leeren** paperless-ngx-3.4.2-Installation `document_importer /export`.
  Der Export hat das Format von paperless-ngx 3.4.2 (siehe [Umstieg → Zurück](migration.md#back)).

## Was (noch) nicht geht

- paperless-ngx **vor 2.19.6** direkt übernehmen – bitte zuerst auf 2.20.15 aktualisieren oder
  über einen Export (Fall 3). Das gilt für 2.x; bei **1.x** geht nur der Weg über 2.20 (ein Export
  aus 1.x wird mit einer klaren Meldung abgelehnt, nichts wird verändert).
- Eine **künftige** paperless-ngx-Version mit Änderungen, die PaperRust noch nicht kennt – bitte
  über einen Export.
- Getestet ist der Umstieg mit Testdatenbanken von paperless-ngx 2.20.15, 3.1.3 und 3.4.2 und mit
  einer echten paperless-ngx-3.1.3-Datenbank (Probelauf, Kopie und an Ort und Stelle, alle Zählungen
  gleich). Bei diesem Echttest wurden die Dateien selbst nicht mitgeprüft, und ein Export aus einem
  echten paperless-ngx 3.1 wurde nicht ausprobiert (siehe [Umstieg (ausführlich)](migration.md#real-db)).
- Die neue Oberfläche gibt es in 10 Sprachen (siehe [Sprache der Oberfläche](usage.md#sprache)). Wer
  in paperless-ngx eine andere Sprache gewählt hatte, sieht die Texte auf Englisch.
