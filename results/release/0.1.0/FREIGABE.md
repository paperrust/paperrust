# Freigabebericht PaperRust 0.1.0 (Entwurf, Stand 2026-11-20 morgens)

Grundlage: [docs/RELEASE-GATE.md](../../../docs/RELEASE-GATE.md). Freigabe nur durch den Besitzer.
Legende: ✅ bestanden mit Nachweis · ⏳ läuft / folgt · ⚠️ teilweise · ❌ offen · 👤 braucht den Besitzer.

## 1. Funktion

| Punkt | Stand | Nachweis |
|---|---|---|
| Rust-Tests aller Crates, Clippy `-D warnings` | ✅ 41 Crates, 2 124 Tests, Clippy überall sauber (1 Fund behoben) | `results/release/tests/2026-11-20.md` |
| DB-Tests (Sperren, Gleichzeitigkeit, Einlesen, Einstellungen) | ✅ alle grün (Wegwerf-Datenbanken) | ebd. |
| Linux-Tests (arm64) | ✅ 41 Crates, 1 769 bestanden; 1 Fehlschlag war eine Windows-Annahme im Test (`\` als Trenner), korrigiert und unter Linux nachgeprüft | `results/release/linux-tests/2026-11-20.md` |
| Oberflächen-Tests (vitest), svelte-check, i18n 10 Sprachen, Budgets | ✅ (svelte-check 0 Fehler, Budget 115,9/120 KB) | Commits 20.11. |
| Klicktests neues Design | ✅ 11/11, zweimal in Folge (1 echter Fehler gefunden und behoben: Design-Umschaltung; 2 wackelige Tests stabilisiert) | `results/e2e/design2/2026-11-20*` |
| Klicktests klassische Oberfläche | ✅ 17/17, zweimal in Folge (6 Abläufe an die überarbeitete Oberfläche angepasst: Texte, Testdaten, Design-Vorgabe; kein Oberflächenfehler) | `results/e2e/light-flows/2026-11-20-r1`, `-r2` |
| Parität zu paperless-ngx (Paritäts-Suiten gegen Django-Referenz) | ❌ in dieser Runde nicht gelaufen (Django-Referenz nicht gestartet) | – |
| Lückenliste `docs/LUECKEN-NGX.md` | ✅ 50 geschlossen, 1 bewusst (C4) | `docs/LUECKEN-NGX.md` |
| Alle Dateitypen mit echten Beispielen | ⚠️ Jetson-Messung (PDF digital/Scan, Fotos, Office); Fotos 20.11. wieder 3/3 gefunden | `results/benchmark/*` |

## 2. Daten und Umstieg

| Punkt | Stand | Nachweis |
|---|---|---|
| Umstieg von ngx 3.4.2 / 2.20 / 1.17 | ✅ 3.4.2 ohne Abweichung; 2.20 erwartete Unterschiede; 1.x über 2.20 (Export aus 1.x wird klar abgelehnt) | `results/release/migration/BERICHT.md` |
| Suche nach Umstieg (B1) | ✅ behoben + Test | Commit 9dbb039 |
| Rückweg zu ngx (Export) | ⚠️ Format 3.4.2, in früheren Läufen geprüft; in dieser Runde nicht wiederholt | `docs/user/umstieg.md` |
| Sicherung/Wiederherstellung | ✅ komplette Wiederherstellung auf frischem System (Jetson, bf77585): 40 Dokumente über drei Wege (Export/Import, pg_dump, `backup restore`), Dateien byte-gleich, Metadaten/Rechte/Suche/Anmeldung gleich; Hinweis zu Dateirechten in die Doku | `results/release/restore/2026-11-20.md` |
| Konsistenzprüfung nach Last | ✅ nach 24 h sauber | `results/release/soak/2026-11-19-24h/` |

## 3. Last, Gleichzeitigkeit, Ausfälle

| Punkt | Stand | Nachweis |
|---|---|---|
| Last-Suite `tests/stress` | ✅ (18.11.) fehlerfrei | `tests/stress/BEFUNDE.md` |
| Dauerlauf 24 h | ✅ 0 Server-Fehler, Speicher stabil | `results/release/soak/2026-11-19-24h/AUSWERTUNG.md` |
| Ausfälle (Chaos) | ✅ (18.11.) | `results/release/chaos/` |
| Großes Archiv | ⚠️ 2 230 Dokumente gemessen; 100 000 / 1 Mio. nur DB-Stichprobe | `docs/PERF-SCALE.md` |
| Leistungsvergleich gegen ngx (Release-Stand) | ✅ e546b8f: 230 Dok. in 106 s statt 1 796 s, Start 3,3 s statt 53 s, Leerlauf nach Einlesen 494 statt 751 MiB; Schwäche DB-RAM/-Größe benannt. S2/S3 (Mehrbenutzer, mit Uploads) im Nachlauf bf77585: Median 4–6 ms statt 68–84 ms bei ngx, 0 Fehler (`s2s3-rerun/bericht.md`) | `results/benchmark/2026-11-20-jetson/bericht.md` |
| Plattenbedarf | ✅ nach Entscheidung des Besitzers (pro Dokument, Sockel erlaubt) | `results/release/2026-11-19-platte-und-start.md` |

## 4. Sicherheit

| Punkt | Stand | Nachweis |
|---|---|---|
| Unabhängige Sicherheitsprüfung (anderes Modell) | ✅ 0 kritisch/hoch; 4 mittel + 10 niedrig + 3 ngx-Punkte behoben | `results/release/review/security-2026-11-19.md` |
| Sicherheits-Suite, Rechte-Matrix | ✅ (18.11.) | `results/release/security/` |
| Fuzzing | ✅ 2 Runden (18.11. ≈ 4,9 h, 20.11. ≈ 5,8 h CPU), 7 Ziele; 20.11. 1 neuer Fund (Überlauf HEIC-Kopf, niedrig) behoben + Test, Nachlauf sauber | `results/release/security/BERICHT.md` §4 |
| `cargo audit`/`deny`, npm audit, Lizenzen | ✅ `cargo deny`: advisories/bans/licenses/sources ok (20.11.); 14 Meldungen alle begründet in `deny.toml` (u. a. zip nur lesend im Kindprozess, rsa nur sqlx-mysql); `pnpm audit --prod`: keine bekannten Lücken | `deny.toml` |
| Keine Geheimnisse in Repo/Historie | ⚠️ lokale Pfade, NAS-IP, Gitea-Host in ~20 Dateien → vor GitHub bereinigen | `docs/GITHUB-RELEASE-PREP.md` |

## 5. Plattformen und Betrieb

| Punkt | Stand | Nachweis |
|---|---|---|
| Docker amd64 | ✅ gebaut + Rauchtest | `results/release/docker/2026-11-20-amd64.md` |
| Docker arm64 | ✅ gebaut (Jetson, 20.11.) und gemessen | `results/benchmark/2026-11-20-jetson/` |
| Desktop Linux (x86_64) | ✅ Paket im Debian-12-Container gebaut (`apps/desktop/pack/linux-build.sh`, glibc ≥ 2.36), **Tesseract 5.3 + deu/eng/osd eingebaut**, 135 MB; frisches Ubuntu 24.04 und Debian 12 ohne System-Tesseract: Start, automatische Anmeldung, Scan mit Texterkennung ✅; Fenster (WebKitGTK) in WSL ✅. Funde behoben: Anmeldeschlüssel im Protokoll, unvollständige Lizenzliste | `results/desktop/linux/` (nicht versioniert) |
| Desktop Windows: echte Installation/Update/Deinstallation | ✅ (Fehler „alte Dateien bleiben“ gefunden und behoben) | `results/release/desktop/2026-11-19-installtest.md` |
| Desktop ohne Anmeldung | ✅ automatisch angemeldet (Protokoll), „Abmelden“ ausgeblendet; Sichtprüfung durch Besitzer OK (20.11.) | Commits 6b283c6, 43776d9 |
| NAS-Vorlagen echt gestartet | ⚠️ bekannte Grenze (Entscheidung Besitzer 20.11.), in den Release-Notizen genannt | – |
| Browser: Chromium/Edge | ✅ Klicktests | – |
| Browser: Firefox, Android | ⚠️ bekannte Grenze (Entscheidung Besitzer 20.11.) | – |
| Browser: Safari (macOS/iOS) | ⚠️ bekannte Grenze (Entscheidung Besitzer 20.11.) | – |
| Barrierefreiheit | ⚠️ Kontraste ✅ (≥ 4,5:1 hell+dunkel), Tastatur ✅ (j/k/x/Strg+K/g d/?), Screenreader: bekannte Grenze (Entscheidung Besitzer 20.11.) | e2e `contrast`, `keyboard` |

## 6. Doku und Auftritt

| Punkt | Stand | Nachweis |
|---|---|---|
| Doku de/en | ✅ nachgeführt (Teilen/Schwärzen, Desktop-Anmeldung, Umstieg 1.x, Docker amd64) | `docs/user/` |
| Release-Notizen | ✅ Stand 20.11.; Leistungsteil wird mit Jetson-Zahlen aktualisiert | `docs/RELEASE-NOTES-0.1.0*.md` |
| Website | ✅ Benchmark-Bereich, Dauertest, Betriebsarten, Firmen ohne Planung (nicht hochgeladen) | Branch `claude/reverent-lalande-2a5f0c` |
| GitHub-Vorbereitung | ✅ README/CONTRIBUTING/SECURITY/CoC (CoC mit offiziellem Text 2.1 verglichen: identisch), CI, Vorlagen; 👤 2FA, Repo anlegen | `docs/GITHUB-RELEASE-PREP.md` |
| Downloads mit Prüfsummen | ❌ erst mit echtem Release | – |

## Vom Besitzer zu tun / zu entscheiden

Erledigt am 20.11.: Sichtprüfung Desktop ✅; NAS, Safari, Firefox/Android, Screenreader als bekannte Grenzen
akzeptiert ✅; GitHub-Organisation und privates Repo `paperrust/paperrust` angelegt ✅; Historie: frischer
Ein-Commit-Stand als `PaperRust <contact@paperrust.org>`, ohne Claude-Zeilen ✅; Docker-Image wird unter
`ghcr.io/paperrust/paperrust` veröffentlicht ✅; Code-Signatur: SignPath Foundation nach dem Release.

Offen:
1. Hostpoint-Zugang für die Website (SSH-Schlüssel) – später.
2. Das finale „Los“ für GitHub (Push, öffentlich schalten, Release mit Dateien, Image) und die Website.

## Ungeklärt (niedrig)

- Während der Klicktests fehlte einmal die gespeicherte Design-Wahl des Testnutzers (`ui_settings.paperrust`). Gezielte Nachstellung mit allen Abläufen, die Einstellungen speichern (klassisch: Tags, Korrespondenten, Einstellungen; neu: Startseite, Autosave, Suche, Assistent): Wert bleibt erhalten. Vermutlich Folge der eigenen Fehlersuche im Browser. Beobachten.

## Freigabe-Pakete (gebaut 2026-11-20 abends, Stand f4cc121)

Alle Pakete aus demselben Commit `f4cc121` gebaut. Spätere Commits (`dda9cad` und davor) ändern nur Tests, Doku und
das interne Publish-Skript, nicht das Programmverhalten.

| Paket | Bau | Rauchtest | SHA-256 / ID |
|---|---|---|---|
| Windows-Installer `PaperRust-0.1.0-windows-x64-setup.exe` (70 MB, unsigniert) | Laptop, `pack.ps1 -Zip -Installer` | Programme starten (`--version`/`--help`), PostgreSQL- und Tesseract-Selbsttest im Pack-Lauf ✅ | `SHA256SUMS` |
| Windows portabel `PaperRust-0.1.0-windows-x64.zip` (109 MB) | wie oben | wie oben | `SHA256SUMS` |
| Linux `PaperRust-0.1.0-linux-x86_64.tar.gz` (142 MB) | WSL, Debian-12-Container (`linux-build.sh`) | Bau inkl. Tesseract-Selbstprüfung ✅ | `SHA256SUMS` |
| Docker amd64 `prrel_paperrust:0.1.0-amd64` | WSL, Dockerfile-Standard (fat LTO) | `paperrust --version`, Tesseract 6 Sprachen ✅ | Image `9fe47e808ee4` |
| Docker arm64 `prrel_paperrust:0.1.0-arm64` | Jetson, aus Gitea geklont, **thin LTO / 16 codegen-units** (fat LTO brauchte beim Linken mehr RAM, als der Jetson frei hat; mit thin LTO sind alle Jetson-Messungen gemacht) | `paperrust --version`, Tesseract 6 Sprachen ✅ | Image `476b16533b6f` |

Nicht wiederholt mit diesem Stand: volle Installations-/Update-Tests unter Windows und auf frischen Linux-Systemen
(zuletzt mit dem Stand vom 19./20.11. bestanden; seither nur kleine Änderungen in pl-system und Tests).
Öffentlicher Baum: `tools/publish/make-public-tree.sh` → 1 Commit `PaperRust <contact@paperrust.org>`, 3 356 Dateien,
Prüfung auf private Daten (Pfade, Hosts, E-Mail, Name) ohne Treffer.
