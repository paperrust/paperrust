# Kerne teilen beim Einlesen – Messung auf dem NAS (2026-11-23)

Frage: Hält der neue gemeinsame Seiten-Pool (Commit b8a9b2eb) was er verspricht? Ein einzelnes Dokument soll
so schnell sein wie mit einer Aufgabe, ein Stapel so schnell wie mit `PAPERRUST_TASK_WORKERS=auto`.

## Aufbau

- Gerät: Synology DS1525+ (AMD Ryzen Embedded V1500B, 4 Kerne / 8 Threads, 16 GB), Docker 24.
  Wie auf dem Pi bekommt PaperRust **3 Kerne**: `--cpuset-cpus 1,3,5` (drei verschiedene echte Kerne) für App
  und Datenbank, Messskript auf Kern 7. App höchstens 3 GB RAM, Datenbank 1 GB.
- Abbilder:
  - **0.1.2** = `ghcr.io/paperrust/paperrust:0.1.2` (veröffentlicht, ohne Pool).
  - **neu** = `main` b8a9b2eb, auf dem NAS gebaut (x86-64, thin LTO, 16 Codegen-Units).
- Einstellungen: „Standard“ = `TASK_WORKERS` ungesetzt (= 1), „auto“ = `PAPERRUST_TASK_WORKERS=auto`.
  `PAPERRUST_THREADS_PER_WORKER` war in allen Läufen **ungesetzt** (wie auf dem Pi; das Messskript setzt
  sonst 2). Laut App-Log: 3 Kerne erkannt, auto ⇒ 2 Aufgaben gleichzeitig; im neuen Abbild zusätzlich
  „OCR page pool: 3 page tasks at the same time“.
- Messung: `results/release/tools/bench.py`, aber nur **S7** (25 einzelne Dokumente nacheinander, darunter
  drei 8-seitige Scans) und **S4-Import** (230 Dokumente auf einmal: 30 Scans, dann 200 Text-PDFs, dazu jede
  Sekunde eine Listen-/Suchabfrage). Dieselben Testdokumente wie auf dem Pi (`gen_docs.py`, gleicher Seed).
- Jede Einstellung **zweimal**, abwechselnd (a c b d, dann nochmal a c b d).

## Ergebnisse

Zeiten in Sekunden, „einzeln“ = Median von 3–8 gleichen Dokumenten, je Lauf. Zwei Werte = Lauf 1 / Lauf 2.

| | Einstellung | Scan 1 Seite einzeln | Scan 8 Seiten einzeln | Text-PDF einzeln | 30 Scans im Stapel | 230 Dokumente gesamt | p95 Liste/Suche beim Import |
|---|---|---|---|---|---|---|---|
| a | 0.1.2, Standard | 8,7 / 8,7 | 15,3 / 15,5 | 6,1 / 6,1 | 95 / 95 | 210 / 236 (Ø 223) | 524 / 1057 ms |
| b | 0.1.2, auto | 8,7 / 8,5 | **25,5 / 25,5** | 5,9 / 6,1 | 55 / 56 | 174 / 185 (Ø 179) | 505 / 591 ms |
| c | neu, Standard | 8,7 / 8,7 | 15,1 / 15,7 | 5,7 / 6,1 | 95 / 98 | 218 / 213 (Ø 216) | 749 / 709 ms |
| d | neu, auto | 8,7 / 8,7 | **15,6 / 15,4** | 6,1 / 6,1 | **55 / 55** | **160 / 198 (Ø 179)** | 535 / 556 ms |

„30 Scans im Stapel“ = Zeit vom Einwerfen, bis die 30 Scans des Stapels eingelesen sind (sie liegen vorne).
Das ist der reine OCR-Teil und schwankt kaum. Die Gesamtzeit schwankt dagegen stark (±10–20 %), weil der hintere
Teil (200 Text-PDFs) vor allem auf Platte und Datenbank wartet – das NAS hat nebenbei eigene Plattenlast.

Alle 8 Läufe: 255 von 255 Dokumenten, 0 fehlgeschlagene Aufgaben, 0 Fehler bei den Abfragen, OCR-Wortquote 1,0,
keine Fehler im App-Log. Höchster App-Speicher beim Import (RSS laut cgroup): a 660–720 MiB, b ~620 MiB,
c 650–680 MiB, d ~765 MiB.

Vergleich mit dem Pi 5 (gleiche Dokumente, 3 Kerne): Das NAS liegt fast gleichauf (Pi: 8-Seiten-Scan 14,8 s seriell,
24,1 s mit auto; 255 Dokumente 214 s bzw. 162 s). Das Muster ist dasselbe.

## Was das heißt

- **Ziel erreicht.** Mit dem Pool und `auto` (d) ist ein einzelner 8-seitiger Scan genauso schnell wie mit einer
  Aufgabe (15,5 s statt 25,5 s ohne Pool), und der Stapel ist genauso schnell wie mit dem alten `auto`
  (OCR-Teil 55 s statt 95 s, gesamt im Schnitt 179 s statt 216–223 s, also rund 20 % schneller).
- Einseitige Scans und Text-PDFs sind in allen Einstellungen gleich schnell – dort gibt es nichts zu teilen.
- Die Bedienung während des Imports (Liste/Suche) wird mit auto nicht schlechter; die p95-Werte schwanken stark
  (bis 1 s in einem Standard-Lauf), ein Nachteil für auto ist nicht zu sehen.
- Ohne `auto` (c) bringt der Pool nichts und schadet nichts (c ≈ a).

**Empfehlung:** Auf diesem Gerät spricht nichts mehr dagegen, `auto` mit dem Pool zum Standard zu machen:
einzelne Dokumente bleiben gleich schnell, Stapel werden schneller. Vorher sollte es noch auf dem Pi bestätigt
werden (siehe Vorbehalte), und das Log sollte die echte Seitenzahl je Dokument nennen (siehe unten).

## Vorbehalte

- NAS ≠ Pi: x86-64 (Ryzen) statt ARM, Docker auf Synology (cgroup v1), eigene Hintergrundlast des NAS (Platte).
  Die Zahlen liegen zufällig nah am Pi, der Pi selbst wurde mit dem Pool **nicht** gemessen.
- Das neue Abbild ist mit **thin LTO** gebaut, 0.1.2 ist das veröffentlichte Abbild (Release-Profil). Bei
  einzelnen Dokumenten ist kein Unterschied zu sehen (c ≈ a), der Einfluss ist also klein; ganz sauber wäre ein
  Vergleich zweier gleich gebauter Abbilder.
- Nur zwei Läufe je Einstellung, und nur S7 + S4-Import (kein Leerlauf, keine 1/5/10-Nutzer-Last wie auf dem Pi –
  der p95-Wert hier ist darum nicht mit dem p95 „10 Nutzer“ der Pi-Tabelle vergleichbar).
- Nicht gemessen: ein einzelner großer Scan, der eingeworfen wird, **während** ein Stapel läuft (dann teilen sich
  die Dokumente die Kerne – das ist so gewollt).
- Speicher: Die Werte sind RSS der cgroup (ohne root kein PSS wie in `memsample.py`), also nicht direkt mit den
  PSS-Werten früherer Berichte vergleichbar.
- Auf dem NAS gab es kein buildx/BuildKit; das Abbild wurde mit einer Kopie des Dockerfiles für den alten Builder
  gebaut (`tools/Dockerfile.legacy`: gleiche Schritte, ohne Cache-Mounts, Prüfsummen per `sha256sum` statt
  `ADD --checksum`). Der Produktcode ist unverändert.

## Nebenbefunde (nicht geändert)

- Das App-Log meldet mit auto weiterhin „OCR threads per worker: 1“, obwohl ein allein laufendes Dokument dank
  Pool 3 Seiten gleichzeitig rechnet (`pl_core::resources`). Der Text ist irreführend.
- `available memory` im selben Log zeigt den Speicher des ganzen Rechners (8 GB), nicht die Grenze des Containers
  (3 GB). Die Automatik rechnet also unter cgroup v1 mit zu viel Speicher. Auf dem NAS hat das nicht geschadet
  (Höchstwert ~770 MiB), bei knapper Grenze könnte es zu viel werden.

## Dateien

- `raw/run_<lauf>.json` – Ergebnisse je Lauf (a1 … d2), `raw/log_<lauf>.txt` – Ablaufprotokoll,
  `raw/run_<lauf>_applog.txt.gz` – App-Log, `raw/samples_<lauf>.jsonl.gz` – Speicher/CPU alle 2 s,
  `raw/run_all.log`, `raw/run_all.sh`, `raw/build.sh`, `raw/build_log_excerpt.txt`.
- `tools/bench_nas.py` – Kopie von `bench.py` mit NAS-Pfaden, Namen `prbench_ad_*`, Docker-Pfad, Python-3.8-
  Anpassung (`dict | dict`), Speichergrenzen; `tools/bench_ad.py` – verkürzter Ablauf (S7 + S4-Import);
  `tools/memsample_v1.py` – Abtaster für cgroup v1 ohne root; `tools/Dockerfile.legacy`.
