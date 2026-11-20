# Auswertung Dauerlauf 24 h (Koordinator, 2026-11-20)

Image `prrel_paperrust:5f5de7c` (Stand vor den Korrekturen vom 19.11.), Jetson Orin Nano, geteilt.

**Ergebnis: bestanden** (mit zwei geklärten Auffälligkeiten).

- 441 969 Anfragen, **0 Server-Fehler**. Die 768 gezählten „Fehler“ sind alle `autocomplete` mit Status 0:
  das Lastprogramm schickte Umlaute unkodiert, die Anfrage verließ den Client nie (bekannt, Skript korrigiert).
- 0 fehlgeschlagene Aufgaben, 0 Log-ERROR, Suchindex-Stichprobe 40/40, `document_sanity_checker`: keine Probleme.
- Kein Wachstum: RAM PaperRust −0,9 MiB/h, PostgreSQL +0,3 MiB/h, Dateien/Threads/Verbindungen flach.
- **Dokumentzahl „weicht ab“ (1001 statt 1332): Rechenfehler im Prüfskript.** `expected` zählte alle
  erfolgreichen Aufgaben (auch Massenänderungen, Papierkorb) als Einlesevorgänge. Richtig:
  300 + 725 eingereicht − 24 endgültig gelöscht = **1001** = Endstand.
- **Zwei Einzelspitzen (27 s in h5, 20 s in h22):** h22 im App-Log nachgesehen – drei PATCH auf Dokumente
  endeten gleichzeitig nach 20,3 / 7,6 / 8,0 s; Ursache laut `sqlx::query slow statement`: das **COMMIT** der
  Datenbank selbst dauerte 20,2 / 7,9 / 7,5 s, gleichzeitig ein einfaches UPDATE der Aufgabenliste → Schreib-
  stillstand des geteilten Speichermediums (fsync), kein Warten auf PaperRust-Sperren. h5 vermutlich gleich
  (im Log nicht mehr enthalten). p99 blieb in allen Stunden unter 39 ms.
- 4xx stiegen von 117 auf ~1200/h mit der Dokumentzahl: Zugriffe des Lastprogramms auf Dokumente, die es
  selbst gelöscht/in den Papierkorb gelegt hatte (404 auf `/api/documents/<id>/`), gewollt.

Offen: Wiederholung mit dem Release-Stand (alle Korrekturen vom 19.11.) – bei der nächsten Jetson-Runde.
