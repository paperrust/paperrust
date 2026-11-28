# Häufige Fragen

[English](en/faq.md) · [Übersicht](README.md)

<a id="what"></a>

## Was ist PaperRust – und was nicht?

Ein Dokumentenarchiv, das aus paperless-ngx v3.4.2 hervorgegangen ist, neu geschrieben in Rust:
gleiche REST-API, gleiche Datenbank, gleiche Dateien – aber ein einzelnes Programm ohne Python, Redis,
Celery, Tika und Gotenberg. Dazu eine eigene Oberfläche (PaperRust-Design) und eigene Funktionen wie
die Automatik mit Testlauf, eingebaute KI, PDF-Werkzeuge und Ransomware-Schutz. Die klassische
Oberfläche im Stil von paperless-ngx bleibt wählbar. PaperRust ist ein **unabhängiges Projekt** und nicht mit dem paperless-ngx-Team verbunden;
Fehler in PaperRust bitte nicht bei paperless-ngx melden.

<a id="apps"></a>

## Laufen meine Apps und Skripte weiter?

Ja, wenn sie die REST-API von paperless-ngx nutzen: Pfade, Felder, Statuscodes und Fehlertexte sind
gleich (geprüft mit Paritätstests gegen Django). Es fehlen nur `/admin/` und die allauth-„headless“-API
([Liste](installation.md#standalone-limits)).

<a id="data"></a>

## Muss ich meine Daten umwandeln?

Nein. Ein Befehl (`paperrust migrate --from …`) oder der Assistent der Desktop-App übernimmt alles,
bei PostgreSQL auf Wunsch direkt in derselben Datenbank (mit automatischer Sicherung). SQLite und
MariaDB werden dabei nach PostgreSQL kopiert. Siehe [Von paperless-ngx umsteigen](umstieg.md).

<a id="back"></a>

## Kann ich zurück zu paperless-ngx?

Ja, aber nicht mehr mit derselben Datenbank: PaperRust passt das Schema nach dem Umstieg an (zuerst
die Tags). Zurück geht es über die Sicherung vom Umstieg oder über einen Export aus PaperRust
(`document_exporter`, Format von paperless-ngx 3.4.2) und einen Import in ein leeres paperless-ngx.
Siehe [Umstieg → Zurück](migration.md#back).

<a id="where"></a>

## Wo liegen meine Dokumente?

Wie bei paperless-ngx unter `PAPERRUST_MEDIA_ROOT` (`documents/originals`, `documents/archive`,
`documents/thumbnails`), benannt nach `PAPERRUST_FILENAME_FORMAT` bzw. Speicherpfaden
([Dateinamen](storage-paths.md)). Die Desktop-App legt alles unter `%LOCALAPPDATA%\PaperRust`
(Windows) ab.

<a id="file-types"></a>

## Welche Dateitypen werden unterstützt?

Wie paperless-ngx: PDF, Bilder (PNG, JPEG, TIFF, GIF, WebP, HEIC/iPhone-Fotos), Text, Office (DOCX,
XLSX, PPTX, ODT/ODS/ODP/ODG, RTF und die alten Formate DOC, XLS, PPT), E-Mails (EML). Office und Mails
werden **ohne Tika** gelesen; das PDF-Abbild (bei paperless-ngx von Gotenberg) setzt PaperRust selbst
mit WordCraft im Hilfsprogramm `paperrust-pdftool` (siehe
[Office-Dateien beim Einlesen](pdf-tools.md#office-beim-einlesen)). Fehlt das Hilfsprogramm, bleibt
das Original ohne PDF-Abbild, aber durchsuchbar.

<a id="duplicates"></a>

## Werden doppelte Dokumente abgelehnt?

Wie in paperless-ngx 3.x: Duplikate werden standardmäßig eingelesen und markiert; mit
`PAPERRUST_CONSUMER_DELETE_DUPLICATES` werden sie verworfen. Die Dokumentliste hat einen Filter für
Duplikate.

<a id="resources"></a>

## Wie viel Speicher braucht PaperRust? Läuft es auf einem Raspberry Pi?

Gemessen auf einem ARM-Kleinrechner (6 Kerne, 8 GB RAM, Docker): frisch
gestartet rund 100 MiB für PaperRust plus rund 70 MiB für PostgreSQL. Nach dem Einlesen mit
Texterkennung hielt PaperRust in dieser Messung rund 900 MiB fest; das wurde inzwischen verbessert
(Sprachmodelle werden nach Leerlauf freigegeben), eine neue Messung folgt. Die Datenbank braucht
bei PaperRust mehr Speicher und Platz als bei paperless-ngx. Ein Raspberry Pi 4/5 mit 64-Bit-System
sollte gehen (gleiche Architektur), ist aber nicht eigens gemessen. Zahlen und Bedingungen:
[Release-Notizen → Leistung](../RELEASE-NOTES-0.1.0.md#leistung).

<a id="ai"></a>

## Nutzt PaperRust KI, bleiben meine Daten privat?

KI ist standardmäßig aus. Eingeschaltet gehen Dokumentinhalte an das konfigurierte Modell – lokal
(Ollama) bleiben sie auf dem eigenen Rechner. Siehe [KI](ai.md).

<a id="broker"></a>

## Brauche ich Redis/Valkey?

Nein. Warteschlange, Zeitplaner und Statusmeldungen laufen über PostgreSQL. Wer von paperless-ngx
umsteigt, kann den Redis-/Valkey-Dienst (und `PAPERLESS_REDIS`) danach entfernen. Für mehrere
Prozesse (getrennter Worker) gibt es einen gemeinsamen Cache in PostgreSQL: `PAPERRUST_CACHE=postgres`.

<a id="docker"></a>

## Gibt es ein Docker-Image?

Das Image lässt sich aus dem Repository bauen (`docker/Dockerfile`, amd64 und arm64); veröffentlicht
ist es noch nicht ([Installation → Docker](installation.md#docker)).

<a id="why-rust"></a>

## Warum ist das schneller?

Ein kompiliertes Programm mit asynchronem Webserver statt Python-Interpreter; dazu Abfragen ohne N+1
(die Dokumentliste braucht eine feste Zahl Datenbank-Abfragen je Seite, egal wie viele Dokumente).
Messwerte und ihre Grenzen: [Release-Notizen → Leistung](../RELEASE-NOTES-0.1.0.md#leistung).
