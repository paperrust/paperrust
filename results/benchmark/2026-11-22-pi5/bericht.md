# Leistungsvergleich PaperRust 0.1.1 und paperless-ngx 3.4.2 auf einem Raspberry Pi 5 (2026-11-22)

Alle Tabellen: `tabellen.md`. Abgeleitete Effizienz-Zahlen: `analyse.py` → `analyse.json`.
Rohdaten: `rohdaten.json` (Zusammenfassung aller Läufe) und `raw/` (Dateiliste am Ende).

## Kurzfassung

- **Die Kernaussage: gleiche Arbeit, ein Zehntel der Zeit am Anschlag.** 230 Dokumente (200 PDFs mit Text,
  30 Scans) waren bei PaperRust nach **3 min 05 s** eingelesen, bei paperless-ngx minimal nach **27 min 07 s**
  (ngx mit Tika/Gotenberg ohne Säubern: 24 min 40 s). Danach ist der Pi bei PaperRust wieder im Leerlauf; bei ngx
  läuft er noch fast eine halbe Stunde mit vollem Takt.
- **Rechenarbeit:** Alle Container zusammen brauchten dafür bei PaperRust **100 CPU-Sekunden**, bei ngx minimal
  **1 253** (12,6×), bei ngx mit Tika/Gotenberg 1 103 (11,1×). Das sind **138 Dokumente je CPU-Minute** gegen 11
  bzw. 12,5.
- **Voller Takt und Wärme:** Der Prozessor lief während des Imports fast durchgehend mit 2 400 MHz – bei PaperRust
  **2,9 Minuten**, bei ngx 26,7 bzw. 24,2 Minuten. Die Temperatur stieg bei allen auf im Mittel 57–58 °C (Leerlauf
  48 °C); die Fläche über dem Leerlauf („Temperatur-Minuten“) ist bei PaperRust **27 K·min**, bei ngx 271 bzw. 236.
  Gedrosselt hat der Pi in keiner Phase (`throttled=0x0` in allen 3 141 Proben).
- **Starten:** PaperRust ist nach 5,2 s bedienbar (ngx 44,5–51,1 s), ein Neustart dauert 0,91 s (ngx 28,9–29,6 s).
- **Speicher im Leerlauf:** frisch gestartet 123 MiB (ngx 646 bzw. 842 MiB), mit 230 Dokumenten **387 MiB**
  (ngx 762 bzw. 1 322 MiB). Die Datenbank allein braucht bei PaperRust mehr (72–81 statt 40–54 MiB).
- **Bedienen:** 1, 5 und 10 Personen gleichzeitig: Median 5–6 ms (ngx 53–61 ms), 95 % schneller als 20 / 19 /
  43 ms (ngx minimal 441 / 181 / 292 ms). Keine Fehler.
- **Schwächer als erwartet:** Während des Stapel-Imports antwortete PaperRust zwar im Median schneller (10 statt
  104 ms), aber 95 % erst unter **439 ms** (ngx 146–151 ms; Jetson am 20.11.: 32 ms). Auch der langsamste 1 % mit
  5 und 10 Personen (256 / 345 ms) liegt deutlich über dem Jetson. Ursache nicht untersucht (Verdacht: Schreiben
  auf die microSD-Karte), siehe „Auffälligkeiten“.

## Gerät und Aufbau

| | |
|---|---|
| Gerät | Raspberry Pi 5 Model B, 8 GB RAM (4 Kerne Cortex-A76, max. 2 400 MHz) |
| Gehäuse/Strom | HutoPi-Starterkit: Gehäuse mit eingebautem Lüfter, offizielles 27-W-Netzteil |
| Speicher | 16-GB-microSD-Karte (kein SSD/NVMe); vor dem Lauf 5,4 GB frei, nach dem Laden von Tika/Gotenberg 3,3 GB |
| System | Raspberry Pi OS Lite 64-bit (Debian 13), Docker 26.1 |
| Kerne | Alle Container auf Kerne 0–2 festgelegt (`--cpuset-cpus 0-2`), Messprogramm auf Kern 3 (`taskset -c 3`) |
| Threads | `THREADS_PER_WORKER=2` bei allen (das Messprogramm setzt 2, sobald nicht 4 Kerne frei sind) |
| Datenbank | PostgreSQL 17 (`postgres:17`), je System eine eigene, frisch angelegt |
| Sprache/Zeit | OCR `deu+eng`, Zeitzone Europe/Zurich, sonst Standard-Einstellungen |

Der sonst auf dem Pi laufende PaperRust-Stack des Besitzers war während jedes Messlaufs gestoppt und wurde danach
wieder gestartet (`raw/results/stack_restore.txt`). Grundlast vor den Läufen PaperRust und ngx A 0,10 bzw. 0,00 (1-min-Mittel; vor ngx C nicht protokolliert).

| Kürzel | Aufstellung | Container | Lauf (Ortszeit) |
|---|---|---|---|
| **PaperRust** | `ghcr.io/paperrust/paperrust:0.1.1` + PostgreSQL 17; säubert und richtet Scans gerade (Standard) | 2 | 15:03–16:08 |
| **ngx minimal (A)** | paperless-ngx 3.4.2 + Valkey + PostgreSQL 17; säubert und richtet Scans gerade (Standard); liest kein Word | 3 | 16:09–17:46 |
| **ngx Tika/Gotenberg (C)** | wie A + Tika 3.3.1.0 + Gotenberg 8.38; liest Word; Säubern und Geraderichten **aus** (`PAPERLESS_OCR_CLEAN=none`, `PAPERLESS_OCR_DESKEW=false`) | 5 | 17:53–19:26 |

**Fairer Vergleich:** ngx minimal (A) macht dieselbe Arbeit wie PaperRust (Säubern + Geraderichten). C zeigt ngx mit
Word-Unterstützung, spart sich aber das Säubern und ist deshalb bei Scans schneller als A.

## Methode

- Messprogramm `results/release/tools/bench.py` (gleich wie bei den Jetson-Messungen), Testdokumente gleich erzeugt
  (`gen_docs.py`): 200 PDFs mit Text, 30 Bild-Scans (200 dpi, leichtes Rauschen, 1–4,5° schief), 25 Einzeldokumente.
- Ablauf je System: Erststart → 3 Neustarts → S1 Leerlauf 10 min → S7 Einzeldokumente nacheinander → S4 Import von
  230 Dokumenten auf einmal (dabei jede Sekunde Liste/Suche) → Massenbearbeitung → S6 Leerlauf mit Dokumenten
  10 min → S2 1 Person 15 min → S3 5 und 10 Personen je 5 min → Leerlauf 5 min.
- Arbeitsspeicher: PSS aller Prozesse eines Containers (geteilte Seiten anteilig) alle ~2 s
  (`memsample.py` → `samples_1_*.jsonl`). „Dienste“ = alles außer PostgreSQL. 1 MiB = 1 048 576 Byte.
- CPU-Zeit: `cpu.stat usage_usec` der Container-cgroup, kumuliert, in denselben Proben.
- Temperatur, Takt, Drosselung, Last: `thermal.sh` alle 5 s (`vcgencmd measure_temp`, `measure_clock arm`,
  `get_throttled`, `/proc/loadavg`) → `thermal.csv`, Zeit in Ortszeit (MEZ, UTC+1).
- **Je System ein Lauf** (Runde 1). Die Streuung ist nicht belegt; Unterschiede unter etwa 10 % nicht überbewerten.
  Runde 0 (`run_0_rs.*`) war ein verkürzter Rauchlauf (`smoke: true`) und ist nicht ausgewertet.

## Ergebnisse

Werte: Median. RAM-Summen addieren die gerundeten Teile. Faktor = ngx-Wert / PaperRust-Wert.

### Start und Leerlauf (leere Datenbank)

| | ngx A | ngx C | PaperRust |
|---|---|---|---|
| Erststart bis Anmeldung möglich | 44,5 s | 51,1 s | **5,2 s** (HTTP nach 3,3 s) |
| Neustart bis Dokumentliste antwortet (Median aus 3) | 28,86 s | 29,63 s | **0,91 s** (0,87–2,90 s) |
| RAM frisch, 10 min Leerlauf: Dienste / Datenbank | 606 / 40 MiB | 797 / 45 MiB | **51** / 72 MiB |
| Summe | 646 MiB | 842 MiB | **123 MiB** |
| CPU-Zeit in 10 min Leerlauf: Dienste / Datenbank | 2,79 / 0,34 s | 4,57 / 0,41 s | **0,96** / 1,10 s |
| Prozesse der Dienste | 20 | 24 | 3 |

### Einzelne Dokumente (S7: Ablegen bis über die Suche auffindbar, Median)

Beide Systeme warten standardmäßig 5 s, bis eine abgelegte Datei „ruhig“ ist; das steckt in jeder Zeit.
„Reine Verarbeitung“ = laut App-Protokoll von Beginn bis Ende der Verarbeitung.

| Art (Anzahl) | ngx A | ngx C | PaperRust | Faktor vs. A |
|---|---|---|---|---|
| PDF mit Text (8) | 8,3 s | 8,4 s | **6,1 s** | 1,4× |
| Scan 1 Seite (5) | 21,8 s | 17,0 s | **8,6 s** | 2,5× |
| Scan 8 Seiten (3) | 48,3 s | 29,3 s | **16,4 s** | 2,9× |
| Foto JPG, schief (4) | 30,3 s | 17,1 s | **9,2 s** | 3,3× |
| Bild PNG, schief (2) | 30,0 s | 17,3 s | **11,4 s** | 2,6× |
| Word DOCX (3) | – (kann kein Word) | 7,9 s | **6,1 s** | – (vs. C 1,3×) |
| *reine Verarbeitung:* PDF / Scan 1 S. / Scan 8 S. | 2,39 / 16,20 / 42,57 s | 2,47 / 11,37 / 24,02 s | **0,46 / 2,88 / 10,95 s** | 5,1× / 5,6× / 3,9× |
| *reine Verarbeitung:* JPG / PNG / DOCX | 24,57 / 24,12 / – s | 11,62 / 11,51 / 2,07 s | **3,62 / 5,83 / 0,43 s** | 6,8× / 4,1× / – |
| Wortquote (gefundene Wörter) | 100 % | 100 % | 100 % | |
| RAM-Spitze Dienste beim Einzeldokument | 924 MiB | 1 331 MiB | **629 MiB** | |

Alle 25 Einzeldokumente wurden bei PaperRust und C eingelesen; A lehnt die 3 Word-Dateien ab (erwartet).

### 230 Dokumente auf einmal (S4)

200 PDFs mit Text und 30 Scans auf einmal in den Eingangsordner, währenddessen jede Sekunde abwechselnd Liste und
Suche (ein „Prüfnutzer“).

| | ngx A | ngx C | PaperRust | Faktor vs. A / C |
|---|---|---|---|---|
| **Alle 230 eingelesen nach** | 1 627 s (**27 min 07 s**) | 1 480 s (24 min 40 s) | **185 s (3 min 05 s)** | 8,8× / 8,0× |
| Durchsatz | 8,5 Dok./min | 9,3 Dok./min | **74,4 Dok./min** | |
| Zeit je Text-PDF im Stapel (Median) | 5,11 s | 5,12 s | **0,40 s** | 12,8× |
| Zeit je Scan im Stapel (Median) | 18,9 s | 14,0 s | **2,9 s** | 6,5× / 4,8× |
| OCR-Wortquote der 30 Scans: Median (schlechtester Scan) | 100 % (97,9 %) | 100 % (97,6 %) | 100 % (96,6 %) | |
| Liste/Suche während des Imports: Median / p95 | 104 / 146 ms | 105 / 151 ms | **10** / 439 ms | p95: 3,0× **schlechter** |
| Liste/Suche danach: Median / p95 | 103 / 151 ms | 105 / 140 ms | **11 / 16 ms** | |
| RAM-Spitze Dienste / Datenbank | 860 / 57 MiB | 1 416 / 58 MiB | **554** / 153 MiB | |
| Fehler (Serverfehler, Zeitüberschreitung) | 0 | 0 | 0 | |
| Aufgaben laut Server | 256 erfolgreich, 1 „REVOKED“ | 260 erfolgreich, 1 „REVOKED“ | 259 erfolgreich | |

Die „REVOKED“-Aufgabe bei ngx gab es auch auf dem Jetson; es fehlte kein Dokument.

### Massenbearbeitung (nach dem Import, 255 Dokumente, je 30 Wiederholungen)

| | ngx A | ngx C | PaperRust |
|---|---|---|---|
| Tag an alle Dokumente: Median / p95 | 1 534 / 2 324 ms | 1 715 / 2 376 ms | **210 / 1 097 ms** |
| „Alle auswählen“ (`selection_data`): Median | 56 ms | 58 ms | **5 ms** |

### Leerlauf mit Dokumenten (S6, Minuten 5–10 nach dem Import) und Platte

| | ngx A | ngx C | PaperRust |
|---|---|---|---|
| RAM Dienste / Datenbank | 708 / 54 MiB | 1 268 / 54 MiB | **306** / 81 MiB |
| **Summe** | **762 MiB** | **1 322 MiB** | **387 MiB** |
| Datenbank-Volume | 72,6 MiB | 72,3 MiB | 96,0 MiB (1,3× **mehr**) |
| Daten + Medien | 51,3 MiB | 56,1 MiB | 54,7 MiB |

### Damit arbeiten (S2/S3, gemischter Ablauf wie in der Oberfläche, inkl. Hochladen)

Liste, Filter, Suche, Öffnen, Ändern, ab und zu Hochladen. 1 Person 15 min mit Pausen, 5 und 10 Personen je 5 min
gleichzeitig. Hochladen hat diesmal funktioniert (58–63 Uploads je System).

| | ngx A | ngx C | PaperRust |
|---|---|---|---|
| **1 Person:** Median / p95 / p99 | 56 / 441 / 632 ms | 53 / 196 / 583 ms | **6 / 20 / 67 ms** |
| **5 Personen:** Median / p95 / p99 | 53 / 181 / 505 ms | 53 / 191 / 480 ms | **5 / 19 / 256 ms** |
| **10 Personen:** Median / p95 / p99 | 60 / 292 / 705 ms | 61 / 355 / 648 ms | **5 / 43 / 345 ms** |
| Anfragen (1 / 5 / 10 Personen) | 443 / 808 / 1 560 | 440 / 817 / 1 560 | 453 / 842 / 1 628 |
| Fehlerquote | 0 % | 0 % | 0 % |
| RAM Dienste bei 10 Personen: Median / Spitze | 868 / 983 MiB | 1 441 / 1 547 MiB | **347 / 400 MiB** |
| CPU-Zeit Dienste bei 10 Personen (5 min) | 179 s | 182 s | **11 s** |

Einzelne Schritte (1 Person, Median): Volltextsuche 167 ms (A) gegen **19 ms**, Dokumentliste 118 gegen **9 ms**,
Dokument öffnen 72 gegen **7 ms**, Änderung speichern 500 gegen **52 ms**. Alle Endpunkte: `tabellen.md`.

## Effizienz: Zeit × Last × Dauer beim Import

Das ist die eigentliche Aussage dieser Messung. Für die 230 Dokumente zählt nicht nur, **wie lange** es dauert,
sondern **wie lange der Pi dabei arbeitet**: wie viel Rechenzeit verbraucht wird, wie lange der Prozessor mit
vollem Takt läuft und wie lange er warm ist. Alle Zahlen erzeugt `analyse.py` reproduzierbar aus den Rohdateien.

### Wie die Zeitfenster bestimmt wurden

- **Import-Fenster** (je System): beginnt, wenn `bench.py` die 230 Dateien in den Eingangsordner verschiebt
  (`t_imp` in `drop_files`), und endet, wenn die Dokumentzahl das Ziel erreicht (`s4.import.seconds`). Die
  JSON-Datei enthält nur relative Zeiten; deshalb sucht `analyse.py` in `samples_1_<system>.jsonl` die eine Folge von
  Proben, deren CPU-Zeiten genau zu `s4.import_window.cpu_s` passen (so hat `bench.py` dieses Fenster selbst
  berechnet; Treffer jeweils eindeutig). Genauigkeit des Beginns: < 2 s (Probenabstand).
  Ergebnis (Ortszeit): PaperRust 15:20:22–15:23:27, ngx A 16:33:26–17:00:34, ngx C 18:15:13–18:39:53.
- **Leerlauf-Bezug** für die Temperatur: S1-Leerlauf desselben Systems (frisch, leere Datenbank, 10 min, Beginn
  `s1.conditions.time`): PaperRust 15:05–15:15, ngx A 16:13–16:23, ngx C 17:57–18:07 Uhr.
- Im Import-Fenster steckt bei allen Systemen auch der Prüfnutzer (1 Anfrage/s), also gleiche Nebenlast.

### Ergebnis

| | ngx A | ngx C | PaperRust | Faktor A / C |
|---|---|---|---|---|
| Dauer des Imports | 27,1 min | 24,7 min | **3,1 min** | 8,8× / 8,0× |
| **CPU-Sekunden aller Container** | 1 253 s (20,9 CPU-min) | 1 103 s (18,4 CPU-min) | **100 s (1,7 CPU-min)** | 12,6× / 11,1× |
| davon Dienste / Datenbank | 1 198 / 55 s | 1 051 / 51 s | 95 / 5 s | |
| **Dokumente je CPU-Minute** | 11,0 | 12,5 | **138** | |
| CPU-Sekunden je Dokument | 5,4 s | 4,8 s | **0,43 s** | |
| Mittlere Auslastung der 3 Kerne | 26 % (0,77 Kerne) | 25 % (0,75 Kerne) | 18 % (0,54 Kerne) | |
| Spitze (10-s-Mittel) | 1,37 Kerne | 1,36 Kerne | 0,95 Kerne | |
| **Minuten mit vollem Takt** (2 400 MHz) | 26,7 min (98 %) | 24,2 min (98 %) | **2,9 min** (97 %) | 9,1× / 8,3× |
| Temperatur im Fenster: Mittel / Spitze | 57,7 / 61,5 °C | 57,9 / 60,9 °C | 56,7 / 60,4 °C | |
| Leerlauf-Temperatur (Median S1) | 47,7 °C | 48,3 °C | 47,7 °C | |
| **Temperatur-Minuten über Leerlauf** | 271 K·min | 236 K·min | **27 K·min** | 10,2× / 8,9× |

Zum Vergleich im Leerlauf (S1): voller Takt in 0–5 % der Proben, CPU-Zeit aller Container 0,2–0,5 s je Minute.

**Was das heißt:**
- Alle drei Systeme bringen den Pi beim Import gleichermaßen auf vollen Takt und auf etwa 57–58 °C. Der Unterschied
  ist die **Dauer**: PaperRust ist nach drei Minuten fertig, ngx hält diesen Zustand 24–27 Minuten lang.
- PaperRust braucht für dieselben 230 Dokumente **rund ein Zwölftel der Rechenzeit**. Rechenzeit ist auf so einem
  Gerät ungefähr gleichbedeutend mit Strom und Wärme; den Stromverbrauch selbst haben wir **nicht** gemessen.
- Die Kerne sind bei keinem System ausgelastet (Mittel 18–26 % der 3 Kerne). Bei PaperRust heißt das: der Stapel
  wird hier nicht von der Rechenleistung begrenzt, sondern davon, dass die Dokumente nacheinander verarbeitet werden
  (bei 2 Threads je Dokument). Da ist noch Luft.
- Gedrosselt hat der Pi nie; die Kühlung des Gehäuses reicht für alle drei.

### Was mit der frei werdenden Zeit und dem Speicher möglich wäre (Veranschaulichung, nicht gemessen)

Mit PaperRust ist der Pi nach einem Stapel von 230 Dokumenten nach 3 Minuten wieder frei statt nach einer knappen
halben Stunde, und im Leerlauf mit Dokumenten bleiben **375 MiB mehr frei als mit ngx minimal** (387 statt 762 MiB)
bzw. **935 MiB mehr als mit ngx mit Tika/Gotenberg** (387 statt 1 322 MiB). Auf demselben Pi könnten damit weitere,
ebenfalls sparsame Dienste laufen, **zum Beispiel** ein DNS-Werbeblocker, ein VPN-Zugang, ein kleiner
Dateisync-/Backup-Dienst oder eine Hausautomation. Wie viel Speicher solche Dienste brauchen, hängt stark von
Programm und Einrichtung ab (grob von einigen zehn MiB bis einige hundert MiB); das haben wir **nicht** gemessen –
es ist nur eine Veranschaulichung, keine Zusage.

## Auffälligkeiten

1. **Prüfnutzer während des Stapel-Imports bei PaperRust langsamer als bei ngx (p95 439 ms):** 58 von 186 Anfragen
   dauerten über 100 ms, 51 davon zwischen Sekunde 95 und 185 – also in der Phase, in der die 200 Text-PDFs schnell
   hintereinander gespeichert werden (die Scans kommen zuerst). Auf dem Jetson (NVMe) war p95 32 ms. Verdacht:
   viele Schreibvorgänge auf die microSD-Karte; **nicht untersucht**. Der Median bleibt bei 10 ms.
2. **Langsamste 1 % mit 5 und 10 Personen** bei PaperRust 256 / 345 ms (Jetson 30–34 ms): 10 von 842 bzw. 38 von
   1 628 Anfragen über 200 ms, vor allem Dokumentliste und Öffnen. Vermutlich dieselbe Ursache, nicht untersucht.
3. **Massenbearbeitung bei PaperRust zweigeteilt:** die meisten Wiederholungen 85–310 ms, die Wiederholungen 18–28
   aber 700–1 570 ms (daher p95 1 097 ms). Ursache unklar.
4. **Geringe Kernauslastung beim Import** bei allen (siehe oben). `THREADS_PER_WORKER=2` kam vom Messprogramm, weil
   nur 3 Kerne zugeteilt waren – bei den Jetson-Läufen waren es 4.
5. **Erster Neustart von PaperRust 2,9 s**, die beiden folgenden 0,87 und 0,91 s (Median 0,91 s).
6. `memory.stat`/`memory.current` waren in allen Proben 0 (auf Raspberry Pi OS ist die Speicher-Abrechnung der
   cgroups offenbar aus). PSS kommt aus `/proc/<pid>/smaps_rollup` und ist davon nicht betroffen; die Spalten
   „anon“ in den Rohdaten sind deshalb ohne Aussage.
7. Bei ngx war der Pi zu Beginn des Imports schon wärmer (57 °C nach dem längeren S7) als bei PaperRust (55 °C).
   Die Temperatur-Minuten beziehen sich deshalb auf den frischen Leerlauf desselben Systems, nicht auf den Wert
   kurz vor dem Import.

## Grenzen und Ehrlichkeit

- Je System **ein Lauf**; Abweichungen unter etwa 10 % nicht überbewerten. Die großen Unterschiede (Faktor 8 und mehr)
  sind davon nicht berührt.
- Die drei Läufe liefen nacheinander über den Nachmittag (15:03–19:26); die Raumtemperatur kann sich verändert haben.
  Leerlauf-Temperaturen lagen bei allen bei 47,7–48,3 °C.
- Speicher-Medium war eine microSD-Karte. Mit SSD/NVMe sind andere (vermutlich bessere) Antwortzeiten unter
  Schreiblast zu erwarten; nicht gemessen.
- Strom haben wir nicht gemessen. CPU-Zeit, Minuten mit vollem Takt und Temperatur-Minuten sind Hilfsgrößen.
- Die Testdokumente sind erfunden und sauber; schlechte echte Scans, Handschrift und größere Archive wurden nicht
  geprüft. Mail, Workflows und die Darstellung im Browser wurden nicht gemessen.
- ngx C spart sich Säubern und Geraderichten und ist deshalb bei Scans schneller als A; für gleiche Arbeit gilt A.

## Rohdaten

`raw/` (vom Pi kopiert):

| Datei | Inhalt |
|---|---|
| `pi_queue.sh`, `pi_queue2.sh` | Ablaufskripte (Runden 0/1 PaperRust + ngx A; danach ngx C) |
| `queue.log`, `queue2.log` | Protokoll der Abläufe (Zeiten, freier Speicher, Temperatur, Drosselung vor jedem Lauf) |
| `thermal.sh` | Temperatur-/Takt-Logger |
| `results/run_1_rs.json`, `run_1_ngxa.json`, `run_1_ngxc.json` | Ergebnisse je System (alle Szenarien, alle Einzelanfragen) |
| `results/samples_1_*.jsonl` (+ `.stop`) | Proben alle ~2 s: PSS und CPU-Zeit je Container |
| `results/thermal.csv` | alle 5 s: Ortszeit, Temperatur, ARM-Takt, Drosselung, Last |
| `results/log_1_*.txt` | Protokoll des Messprogramms je System |
| `results/run_1_*_applog.txt` | App-Protokolle (Container-Logs) je System |
| `results/stack_restore.txt` | Wiederanlauf des sonst laufenden PaperRust-Stacks nach den Messungen |
| `results/run_0_rs.*`, `samples_0_rs.jsonl`, `log_0_rs.txt` | Rauchlauf, nicht ausgewertet |

Ausgewertet: `rohdaten.json` (von `results/release/tools/summarize.py`), `tabellen.md`, `analyse.py` →
`analyse.json` (Effizienz, Zeitreihen für die Grafiken der Webseite: `site/scripts/charts-pi.mjs`).
