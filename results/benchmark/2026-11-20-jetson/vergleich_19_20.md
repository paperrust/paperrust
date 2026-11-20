# PaperRust 19.11. (5f5de7c) gegen 20.11. (e546b8f)

Gleiches Gerät und Messprogramm, je eine Runde. Werte aus `../2026-11-19-jetson/tabellen.md` und `tabellen.md`
(PaperRust-Spalte), Zusatzwerte aus `rohdaten.json` (19.11.) bzw. `raw/run_1_rs.json` (20.11.). Änderung in %
bezogen auf den 19.11.; bei Zeiten und Speicher ist **minus besser**. Mehr als 10 % schlechter ist als
**Regression** markiert. Commits dazwischen: 15 (`git log 5f5de7c..e546b8f`), leistungsrelevant davon vor allem
808c72b (Oberfläche beim Start nicht mehr komplett einlesen), 9dbb039 (Säubern lässt Fotos aus),
62d7a2f (Geraderichten auf mehrere Kerne verteilt).

## Hauptwerte

| Szenario | Messgröße | 19.11. | 20.11. | Änderung | Bewertung |
|---|---|---|---|---|---|
| S1 | Erststart bis Anmeldung [s] | 3,3 | 3,3 | 0 % | gleich |
| S1 | Neustart bis Liste [s] (3 Neustarts) | 0,82 (0,80–0,82) | 0,57 (0,54–0,59) | −30 % | besser; passt zu 808c72b ¹ |
| S1 | RAM Dienste leer, Median [MiB] | 107 | 74 | −31 % | besser; passt zu 808c72b (Oberfläche nicht mehr im Speicher) |
| S1 | RAM Datenbank leer, Median [MiB] | 64 | 64 | 0 % | gleich |
| S1 | CPU-Zeit Dienste / DB in 10 min [CPU-s] | 1,10 / 1,25 | 1,09 / 1,21 | −1 % / −3 % | gleich |
| S1 | Prozesse der Dienste (Maximum) | 3 | 6 | +100 % | kein echter Rückschritt: ein einzelner 2-s-Messpunkt, sonst 3 ² |
| S7 | PDF mit Text, Median [s] | 5,6 | 5,6 | 0 % | gleich |
| S7 | Scan 1 Seite, Median [s] | 8,1 | 8,1 | 0 % | gleich |
| S7 | Scan 8 Seiten, Median [s] | 11,8 | 12,3 | +4 % | im Rauschen |
| S7 | Foto JPG, Median [s] / gefunden | – / 0 von 4 | 9,4 / 4 von 4 | – | **Fehler L1 behoben** (9dbb039) |
| S7 | Bild PNG, Median [s] / gefunden | – / 0 von 2 | 12,0 / 2 von 2 | – | **behoben**; Wortquote 98,7 % |
| S7 | Office DOCX, Median [s] | 5,6 | 5,6 | 0 % | gleich |
| S7 | RAM-Spitze Dienste beim Einzeldokument [MiB] | 983 | 813 | −17 % | besser (obwohl jetzt auch 6 Bilder durchlaufen) |
| S4 | Import 230 Dokumente [s] | 110 | 106 | −4 % | etwas besser |
| S4 | Durchsatz [Dok./min] | 125,0 | 129,7 | +4 % | etwas besser |
| S4 | Scan je Dokument im Stapel, Median [s] | 2,8 | 2,6 | −7 % | besser; passt zu 62d7a2f |
| S4 | Text-PDF je Dokument, Median [s] | 0,11 | 0,10 | −9 % | gleich/besser |
| S4 | CPU-Zeit Import, Dienste [CPU-s] | 101 | 98 | −3 % | gleich |
| S4 | Bedienung während Import, Median [ms] | 10 | 10 | 0 % | gleich |
| S4 | **Bedienung während Import, p95 [ms]** | 18 | 32 | **+78 %** | **Regression**, Ursache unklar ³ |
| S4 | Bedienung nach Import, Median / p95 [ms] | 10 / 13 | 9 / 13 | −10 % / 0 % | gleich |
| S4 | RAM-Spitze Dienste / DB [MiB] | 766 / 113 | 800 / 115 | +4 % / +2 % | im Rauschen |
| S4 | **Massenbearbeitung „Tag an alle“, Median [ms]** | 100 | 177 | **+77 %** | **Regression**, Ursache unklar ⁴ |
| S4 | Massenbearbeitung, p95 [ms] | 197 | 192 | −3 % | gleich |
| S4 | „Alle auswählen“ (selection_data), Median [ms] | 6 | 5 | −17 % | besser (absolut 1 ms) |
| S4 | Liste mit `all`-Feld (select_all_list), Median [ms] (nur Rohdaten) | 7,3 | 11,4 | **+56 %** | **Regression** (absolut 4 ms), Ursache unklar |
| S6 | RAM Leerlauf mit Dokumenten, Min. 5–10, Dienste [MiB] | 465 | 420 | −10 % | besser |
| S6 | RAM Leerlauf mit Dokumenten, Min. 5–10, DB [MiB] | 72 | 74 | +3 % | gleich |
| S6 | Summe Dienste + DB [MiB] | 537 | 494 | −8 % | besser |
| S6 | Datenbank-Volume [MiB] | 96,5 | 97,8 | +1 % | gleich |
| S6 | Logische DB-Größe [MB] | 27,0 | 28,4 | +5 % | leicht mehr, unter 10 % |
| S6 | Daten + Medien [MiB] | 54,7 | 54,7 | 0 % | gleich |
| alle | Fehler (5xx/Zeitüberschreitung) | 0 | 0 | – | gleich |

## Nicht vergleichbar: S2/S3 und Leerlauf am Ende

Am 20.11. fehlte der Upload-Ordner `~/prrel/docs/extra`; jeder der 15 Upload-Versuche beendete die jeweilige
simulierte Person (Einzelheiten `bericht.md`). Deshalb:

| Messgröße | 19.11. | 20.11. | Hinweis |
|---|---|---|---|
| S2 Anfragen / Dauer | 453 / 900 s | 39 / 63 s | abgebrochen beim ersten Upload |
| S3/5 Anfragen / Dauer | 846 / 304 s | 223 / 105 s | abgebrochen |
| S3/10 Anfragen / Dauer | 1 636 / 304 s | 728 / 299 s | 9 von 10 Personen abgebrochen |
| S3/10 Median / p95 / p99 [ms] | 5 / 18 / 30 | 4 / 16 / 34 | ohne Uploads |
| S3/10 nur Lesen, Median / p95 [ms] | 4 / 16 | 4 / 13 | am ehesten vergleichbar: kein Rückschritt erkennbar |
| S3/10 RAM-Spitze Dienste / DB [MiB] | 694 / 147 | 428 / 112 | niedriger, weil keine Uploads/OCR |
| Leerlauf am Ende Dienste / DB [MiB] | 633 / 74 | 391 / 76 | niedriger, weil vorher keine Uploads/OCR |

## Fußnoten

¹ Im Rauchlauf (Runde 0, gleiches Image e546b8f, 01:15, Grundlast 1,25) lagen die drei Neustarts bei
0,79–0,83 s. Der Neustartwert hängt also merklich von der Grundlast ab; der 19.11.-Befund L2 könnte teilweise
auch daran gelegen haben. Im ruhigen Hauptlauf 0,57 s.

² `raw/samples_1_rs.jsonl`: nur ein Messpunkt rund 9 min nach Messbeginn mit 6 Prozessen, drei weitere mit 4,
sonst 3 (`tini`, Entrypoint-Shell, `paperrust`). Vermutlich ein kurzlebiger Hilfsprozess; nicht untersucht.

³ 107 Anfragen, Median 10,3 ms, p95 31,9 ms, Maximum 56,9 ms (19.11.: 111 Anfragen, 9,5 / 18,2 / 51,3 ms). Nur 6
Anfragen lagen über 30 ms, p95 ist bei dieser Anzahl empfindlich. Denkbarer Zusammenhang mit 62d7a2f
(Geraderichten nutzt jetzt mehrere Kerne und konkurriert mit dem Webserver um die 4 Kerne) – aus den Daten
nicht belegt (CPU-Spitze 1,07 statt 1,02 Kerne). Am 18.11. lag p95 bei 21 ms.

⁴ Die 30 Einzelwerte liegen in zwei Lagen: 75–111 ms und darüber (126–202 ms, meist 165–200 ms). Am 19.11. waren
16 von 30 unter 120 ms, am 20.11. 4 von 30. p95 und Maximum sind unverändert (197/202 gegen 192/199 ms); am 18.11. lag der
Median bei 174 ms. Kein Commit zwischen den Ständen betrifft die Massenbearbeitung erkennbar. Wahrscheinlich
zeitabhängiges Verhalten (z. B. Hintergrundarbeit nach dem Import), nicht belegt – Ursache unklar.
