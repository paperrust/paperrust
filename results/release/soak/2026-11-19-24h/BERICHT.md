# Dauerlauf PaperRust – Bericht

Image `prrel_paperrust:5f5de7c`, Start 2026-11-18T23:05:29+01:00, geplant 24 h, Gerät arm64-Testgerät (geteilt, ohne Stilllegung fremder Dienste – Rauschen möglich).

Last: 5 Nutzer im Alltag (Liste, Filter, Suche, Öffnen, Vorschau, Herunterladen, Bearbeiten, Notizen), 1 Such-Client (1/s), alle 10 min ein Stapel (3 Dateien Eingangsordner + 2 per API, jeder 3. Stapel mit Scan), alle 30 min Massenänderungen (200 Dokumente), alle 2 h Papierkorb (löschen/wiederherstellen/leeren).

## Ergebnis

- Laufzeit 24.06 h von 24.0 h; Abbruch: nein
- Anfragen gesamt 441969, Fehler (5xx/Verbindung) **768** {'autocomplete:0': 768}
- Dokumente: Start 300, eingereicht 725, Aufgaben erfolgreich 1358 (davon vor Start 302), endgültig gelöscht 24, erwartet 1332, Ende 1001 → **weicht ab**
- Fehlgeschlagene Aufgaben: 0 []
- Suchindex-Stichprobe: 40 geprüft, 0 nicht gefunden []
- Papierkorb am Ende: 0
- `document_sanity_checker`: letzte Zeilen:

```
2026-11-19T22:09:32.053498Z  INFO paperrust.integrity: Integrity check started (mode full, 4 thread(s), full re-hash after 28 day(s))
Sanity Check: No issues detected.
rc=0

```

## Verlauf (Messung alle 5 min)

| Größe | Anfang (1. h) | Ende (letzte h) | Min | Max | Trend ab Stunde 1 je h |
|---|---|---|---|---|---|
| RAM PaperRust (PSS) | 506 MiB | 503 MiB | 410 MiB | 700 MiB | -0.91 MiB |
| RAM PostgreSQL (PSS) | 144 MiB | 160 MiB | 76 MiB | 207 MiB | +0.32 MiB |
| Offene Dateien PaperRust | 38 | 37 | 34 | 48 | -0.02 |
| Threads PaperRust | 17 | 16 | 16 | 23 | +0.01 |
| DB-Verbindungen | 14 | 14 | 11 | 20 | -0.02 |

## Stunden

| h | Anfragen | Fehler | 4xx | p50 ms | p95 ms | p99 ms | max ms | Log-ERROR | Aufg. fehlg. | Dok. | RAM App MiB | RAM DB MiB | FDs | Threads | DB-Verb. |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 18483 | 31 | 117 | 9.9 | 22.8 | 35.5 | 155.9 | 0 | 0 | 332 | 573 | 160 | 39 | 16 | 16 |
| 2 | 18501 | 33 | 245 | 10.0 | 22.6 | 36.7 | 251.4 | 0 | 0 | 357 | 467 | 158 | 38 | 16 | 15 |
| 3 | 18437 | 50 | 379 | 10.1 | 22.7 | 37.2 | 178.1 | 0 | 0 | 390 | 452 | 155 | 38 | 16 | 15 |
| 4 | 18404 | 28 | 475 | 10.1 | 21.9 | 37.7 | 224.9 | 0 | 0 | 420 | 604 | 158 | 37 | 16 | 14 |
| 5 | 18554 | 38 | 541 | 10.2 | 22.8 | 37.2 | 27267.1 | 0 | 0 | 451 | 455 | 144 | 36 | 16 | 13 |
| 6 | 18358 | 36 | 610 | 10.3 | 22.2 | 37.4 | 179.7 | 0 | 0 | 481 | 516 | 198 | 40 | 16 | 17 |
| 7 | 18295 | 25 | 738 | 10.4 | 21.9 | 37.0 | 201.5 | 0 | 0 | 509 | 495 | 178 | 39 | 16 | 16 |
| 8 | 18359 | 31 | 821 | 10.4 | 22.2 | 37.5 | 201.1 | 0 | 0 | 539 | 498 | 154 | 36 | 16 | 13 |
| 9 | 18428 | 19 | 812 | 10.4 | 22.1 | 37.4 | 149.7 | 0 | 0 | 567 | 499 | 177 | 38 | 16 | 15 |
| 10 | 18513 | 38 | 895 | 10.5 | 22.4 | 38.0 | 214.7 | 0 | 0 | 595 | 509 | 192 | 39 | 16 | 16 |
| 11 | 18472 | 28 | 921 | 10.5 | 22.4 | 37.3 | 215.3 | 0 | 0 | 625 | 522 | 148 | 36 | 16 | 13 |
| 12 | 18346 | 29 | 921 | 10.5 | 21.9 | 37.8 | 346.6 | 0 | 0 | 653 | 513 | 158 | 37 | 16 | 14 |
| 13 | 18518 | 30 | 1024 | 10.5 | 22.2 | 37.5 | 176.5 | 0 | 0 | 683 | 522 | 156 | 36 | 16 | 13 |
| 14 | 18378 | 32 | 980 | 10.5 | 22.1 | 37.0 | 229.5 | 0 | 0 | 711 | 523 | 145 | 37 | 16 | 14 |
| 15 | 18494 | 45 | 1067 | 10.6 | 22.7 | 38.9 | 248.5 | 0 | 0 | 741 | 526 | 161 | 36 | 16 | 13 |
| 16 | 18431 | 33 | 1134 | 10.6 | 21.8 | 38.1 | 166.4 | 0 | 0 | 769 | 522 | 175 | 37 | 16 | 13 |
| 17 | 18464 | 27 | 1132 | 10.6 | 21.9 | 38.1 | 179.5 | 0 | 0 | 799 | 509 | 148 | 36 | 16 | 13 |
| 18 | 18527 | 28 | 1166 | 10.6 | 22.3 | 37.7 | 366.8 | 0 | 0 | 827 | 516 | 156 | 37 | 16 | 14 |
| 19 | 18292 | 30 | 1172 | 10.6 | 22.2 | 37.7 | 173.1 | 0 | 0 | 857 | 510 | 140 | 35 | 16 | 12 |
| 20 | 18402 | 34 | 1296 | 10.6 | 22.1 | 38.2 | 340.8 | 0 | 0 | 885 | 514 | 146 | 36 | 16 | 13 |
| 21 | 18333 | 21 | 1186 | 10.6 | 22.2 | 38.1 | 291.0 | 0 | 0 | 915 | 530 | 152 | 36 | 16 | 13 |
| 22 | 18374 | 36 | 1218 | 10.7 | 22.0 | 38.4 | 20267.7 | 0 | 0 | 943 | 505 | 175 | 37 | 16 | 14 |
| 23 | 18352 | 36 | 1211 | 10.7 | 22.2 | 38.3 | 256.9 | 0 | 0 | 973 | 516 | 161 | 38 | 16 | 15 |

## Fehlerbeispiele

Log-ERROR (erste 15):

- keine

Anfragen mit 5xx/Verbindungsfehler (erste 15):

- h1: ['autocomplete', 0]
- h1: ['autocomplete', 0]
- h1: ['autocomplete', 0]
- h1: ['autocomplete', 0]
- h1: ['autocomplete', 0]
- h1: ['autocomplete', 0]
- h1: ['autocomplete', 0]
- h1: ['autocomplete', 0]
- h1: ['autocomplete', 0]
- h1: ['autocomplete', 0]
- h2: ['autocomplete', 0]
- h2: ['autocomplete', 0]
- h2: ['autocomplete', 0]
- h2: ['autocomplete', 0]
- h2: ['autocomplete', 0]

Fehlgeschlagene Aufgaben (Beispiele):

- keine

