# PaperRust 0.1.2 – Release-Notizen

[English](RELEASE-NOTES-0.1.2.en.md) · Stand 2026-11-23

Eine Version nach [0.1.1](RELEASE-NOTES-0.1.1.md) mit Ordneransicht, Umstieg von paperless-ngx 3.5, Notizen als
Markdown, parallelem Einlesen und Neuem in der Desktop-App. Beim ersten Start legt PaperRust zwei neue Dinge in der
Datenbank an (Notiz-Format, Ordner-Tabellen); beides ist nur eine Ergänzung, bestehende Daten bleiben unverändert und
paperless-ngx kann die Datenbank weiter lesen.

## Neu

- **Ordneransicht (nach Papermerge-Vorbild, optional):** Einstellungen → Darstellung → „Ordneransicht“. Ordnerbaum
  mit „Unsortiert“, Unterordner, Dokumente per Ziehen und Ablegen oder über „In Ordner verschieben“ einsortieren
  (mit Rückgängig), Kachelansicht mit Vorschaubildern, Hochladen direkt in den offenen Ordner. Ordner ergänzen Tags,
  sie ersetzen sie nicht. Beim Export landen sie in `paperrust-extras.json`, der Weg zurück zu paperless-ngx verliert
  nichts. ([Ordner](user/folders.md))
- **Umstieg von paperless-ngx 3.5.x:** Datenbanken von paperless-ngx 3.5.0 und 3.5.1 werden direkt übernommen
  (vorher: Abbruch wegen der neuen Migration `0030_note_format`), auch Exporte aus 3.5.
- **Notizen als Markdown** wie in paperless-ngx 3.5.1, mit Vorschau; Links nur http/https/mailto.
- **Paralleles Einlesen:** zum Einschalten mit `PAPERRUST_TASK_WORKERS=auto` (oder einer Zahl): mehrere
  Dokumente gleichzeitig. Auf einem Raspberry Pi 5 werden 255 Dokumente so in 162 statt 214 Sekunden eingelesen;
  ein einzelner langer Scan dauert dann aber länger (24 statt 15 Sekunden), darum bleibt der Standard eine
  Aufgabe nach der anderen. Die dafür nötigen Sperren gegen doppelte ASN-Nummern, doppelte Dokumente und doppelt
  angelegte Tags sind immer aktiv und formal geprüft ([Messung](../results/benchmark/2026-11-23-pi5-parallel/bericht.md)).
- `localhost`, `127.0.0.1` und `[::1]` sind immer erlaubte Hosts (wie paperless-ngx 3.5), Chat-Antworten erscheinen
  gleichmässiger.

## Neu in der Desktop-App

- **PaperRust-Oberfläche auch mit einem paperless-ngx-Server:** Ist die App mit einem Server verbunden, auf dem
  noch paperless-ngx läuft, zeigt sie auf Wunsch ihre eingebaute PaperRust-Oberfläche statt der Server-Oberfläche
  (Tray → „Neue Oberfläche“ oder Einstellungen → Oberfläche). Anmeldung einmal mit Benutzername und Passwort.
  Listen, Suche und Bearbeiten gehen; Funktionen, die es nur in PaperRust gibt (KI-Assistent, Zeitleiste,
  Workflows, Rollen), zeigen „nicht verfügbar“. Zurück über „Oberfläche des Servers“.
- **Betriebsart nachträglich wechseln:** Tray → „Betriebsart wechseln …“ oder Einstellungen → Betriebsart:
  lokal ↔ mit Server verbinden, mit Bestätigung und Neustart. Lokale Daten und Server-Zugänge bleiben erhalten.
- Die PaperRust-Oberfläche startet im zuletzt gewählten Design.

## Korrigiert

- **Doppelt-Erkennung mit „PDFs verkleinern“:** Wer diese Option nutzt und dasselbe Original nochmal hochlädt,
  bekommt wieder „Duplikat“ (vorher wurde es ein zweites Mal gespeichert).
- **Warteschlange robuster** (gefunden mit einer formalen Prüfung): Fällt die Datenbankverbindung kurz aus, kann
  eine Aufgabe nicht mehr kurzzeitig doppelt laufen; eine Aufgabe bleibt nach einer langen Pause eines Prozesses
  nicht mehr auf „gestartet“ hängen; eine verspätete Rückmeldung kann eine neuere nicht mehr überschreiben.
- **Hochladen in einen gerade gelöschten Ordner** landet in „Unsortiert“, statt abzubrechen.
- Eine unlesbare Einstellungsdatei der Desktop-App wird beim Wechsel der Betriebsart nicht mehr überschrieben
  (vorher wären alle Server-Profile verloren gegangen).
- Linux-Pakete werden wieder auf Debian 12 gebaut (glibc 2.36) und laufen auf Debian 12 und Ubuntu 22.04.

## Bekannte Grenzen

Wie [0.1.1](RELEASE-NOTES-0.1.1.md#bekannte-grenzen). Neu: Mit einem paperless-ngx-Server meldet man sich in der
PaperRust-Oberfläche nur mit Benutzername und Passwort an (kein Single Sign-on); Antworten des KI-Assistenten
über die Desktop-App erscheinen erst vollständig, nicht schrittweise. Ordner gibt es in der Weboberfläche; die
Offline-Kopie der Desktop-App und die klassische Oberfläche kennen sie noch nicht, Ordner-Freigaben gehen nur über die API.

## Update

- Server ohne Docker (Linux-Paket, `tar.gz`): nach dem Update einmal `paperrust manage migrate`. Docker, die Desktop-App
  und der FreeBSD-Dienst machen das beim Start selbst.
- Docker: Image auf `ghcr.io/paperrust/paperrust:0.1.2` stellen und neu starten.
- Desktop/Linux-Paket: neues Paket installieren. Wie immer vorher sichern.
