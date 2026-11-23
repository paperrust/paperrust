# PaperRust 0.1.3 – Release-Notizen

[English](RELEASE-NOTES-0.1.3.en.md) · Stand 2026-11-23

Eine kleine Version nach [0.1.2](RELEASE-NOTES-0.1.2.md): schnelleres Einlesen von Stapeln, ohne dass einzelne
Dokumente langsamer werden. Datenbank und Dateien bleiben unverändert – ein Update ist ein Austausch des Images
bzw. Programms.

## Neu

- **Mehrere Dokumente gleichzeitig, mit gemeinsamem Kern-Vorrat (jetzt Standard):** Alle Dokumente teilen sich
  die Prozessorkerne seitenweise. Ein Dokument, das allein kommt, bekommt alle Kerne; kommen mehrere, wird nach
  jeder Seite neu verteilt. Gemessen auf 3 Kernen (NAS, wie ein Raspberry Pi 5): ein einzelner Scan mit 8 Seiten
  15,5 s wie bisher, die Texterkennung von 30 Scans im Stapel 55 statt 95 Sekunden, keine Fehler
  ([Messung](../results/benchmark/2026-11-23-nas-adaptive/bericht.md)). Wer eins nach dem anderen will:
  `PAPERRUST_TASK_WORKERS=1`.
- **Container mit Speicherlimit:** PaperRust richtet sich nach dem Speicherlimit des Containers (cgroup v1/v2)
  statt nach dem Speicher des ganzen Rechners und startet so nicht zu viel gleichzeitig.
- Die Startmeldung zeigt die Zahl der Aufgaben und die Grösse des gemeinsamen Kern-Vorrats.

## Bekannte Grenzen

Wie [0.1.2](RELEASE-NOTES-0.1.2.md#bekannte-grenzen). Arbeiten ausserhalb der Seiten-Texterkennung (PDF/A,
Vorschaubilder, Textextraktion) zählen noch nicht zum gemeinsamen Vorrat.

## Update

- Docker: Image auf `ghcr.io/paperrust/paperrust:0.1.3` stellen und neu starten.
- Desktop/Linux-Paket: neues Paket installieren. Wie immer vorher sichern.
