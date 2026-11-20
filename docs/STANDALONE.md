# PaperRust ohne äußere Abhängigkeiten (Standalone / native App)

Ziel: Ein Programm, das ohne Docker, Redis, Celery, Python, Tika und Gotenberg läuft – als Server
(`paperrust serve`) **und** als native Desktop-App (Windows/macOS/Linux) ohne separaten Server.

> **Nur noch Standalone (seit 2026-11-14):** PaperRust enthält keinen Django-, Celery- oder
> Redis-Code mehr. Die Weiterleitung an Django ist entfernt; jede Route bedient Rust, Pfade ohne
> Route antworten 404 (HTML, unter `/api/` DRF-JSON `{"detail":"Not found."}`).
> `PAPERRUST_DJANGO_URL`, `PAPERRUST_QUEUE` und `PAPERLESS_REDIS` sind wirkungslos (Warnung bzw.
> Hinweis beim Start). Einzige Warteschlange ist `paperrust_task_queue` in PostgreSQL.
>
> Vorgeschichte (2026-11-13): Mit der Migration `pl_objects.0001_paperrust_tag_adjacency` (Tags ohne
> django-treenode-Spalten, docs/contract/pl-objects.md „Tag-Baum“) kann paperless-ngx v3.4.2 dieselbe
> Datenbank nicht mehr nutzen. Vor dem Umstieg daher **sichern und paperless-ngx stoppen**; ein
> gemeinsamer Betrieb beider Programme auf einer Datenbank ist nicht möglich. Die Lücken unten sind
> nicht über Django zu schließen; was fehlt, antwortet 404.

## Heute (paperless-ngx) → PaperRust Standalone

| Heute | Zweck | PaperRust Standalone |
|---|---|---|
| Python/Django/Celery-Worker | Verarbeitung | Rust-Worker im selben Prozess |
| Redis | Warteschlange, Status, Cache | eigene Warteschlange in der DB + In-Prozess-Ereignisse (WebSocket-Status direkt) |
| PostgreSQL-Server | Datenbank | Server: PostgreSQL wie bisher. Desktop: **eingebettetes PostgreSQL** (mitgeliefert, vom Programm gestartet; Entscheidung 2026-11-07) |
| OCRmyPDF + Tesseract + Ghostscript | OCR, durchsuchbares PDF/A | Tesseract als Bibliothek mitgeliefert, Seiten rendern mit PDFium (mitgeliefert), Textebene selbst schreiben |
| Tika | Text aus Office/Mail | Rust-Parser für DOCX/XLSX/PPTX/ODF/EML/HTML/RTF (Text) |
| Gotenberg (LibreOffice/Chromium) | Office/Mail → PDF | optional: lokales LibreOffice, falls installiert; sonst Original ohne Archiv-PDF, aber durchsuchbar |
| scikit-learn | automatische Zuordnung | Rust-Klassifikator |
| dateparser | Datum aus Text | `pl-matching` |
| tantivy (Python) | Volltext | tantivy (Rust), schon eingebettet |
| llama-index/torch | KI | Ollama/OpenAI per HTTP, lokale Embeddings über candle/ort (optional) |
| Angular-UI vom Server | Oberfläche | dieselbe UI, im Desktop in einem nativen Fenster (Tauri/WebView) |

## Leitplanken ab sofort (damit nichts verbaut wird)

1. **Keine Abhängigkeit auf Redis/Celery im Rust-Code.** Aufgaben laufen über einen Trait
   (`TaskQueue`), umgesetzt von der PostgreSQL-Warteschlange (der Celery-Producer für den
   Übergangsbetrieb ist seit 2026-11-14 entfernt).
2. **Datenbank-Code dialektneutral** über sea-query (gilt schon). Kein PostgreSQL-only-SQL ohne
   Alternative.
3. **Externe Dienste hinter Traits** (OCR, Office-Konvertierung, Textextraktion, LLM), damit sie
   austauschbar und optional sind.
4. **Pfade/Konfiguration plattformneutral** (Datenverzeichnis je OS, keine Linux-Pfade fest im Code).
5. **Kein Netzwerkzwang:** alles muss offline funktionieren; Server-Funktionen (Mail-Abruf, Remote-
   Version, Cloud-LLM) nur optional.

## Reihenfolge

1. Phase 1: API-Parität (läuft) – Übergang mit Django/Celery im Hintergrund.
2. Python-freier Rust-Worker (Einlesen, OCR, Parser, Klassifikator, Mail, Workflows) mit
   Paritätsprüfung gegen upstream-Ergebnisse.
3. Redis/Celery entfernen (eigene Warteschlange), Server-Binary ohne äußere Abhängigkeiten –
   erledigt 2026-11-14 (auch Django-Weiterleitung entfernt).
4. Desktop-App (Tauri) mit eingebetteter Datenbank, Installer, Auto-Update (signiert).
5. Optimierungen nach Skill laufen begleitend für Server und Desktop.

## Stand: Betrieb ohne Django (W22, 2026-11-08)

PaperRust läuft ohne Django, Python, Celery und Redis – API **und** Angular-Oberfläche. (Damals
noch mit `PAPERRUST_DJANGO_URL=off` und `PAPERRUST_QUEUE=postgres`; seit 2026-11-14 ist beides der
einzige Betrieb und die Variablen sind wirkungslos.) Heutiger Start:

```bash
PAPERLESS_STATICDIR=<Angular-Build + collectstatic>                    # z. B. reference/data/static
# PAPERRUST_ROUTES=<routes.toml>       # optional, nur zum Abschalten einzelner Routen ("off")
PAPERRUST_STOP_ON_STDIN_EOF=1          # optional: Stopp, wenn der Supervisor die stdin-Pipe schließt
paperrust serve --worker
```

* Oberfläche: `ui.index` (Index mit Sprachwahl wie `IndexView`, Login-Weiterleitung, CSRF-Cookie,
  Django-Messages), `static.files` (WhiteNoise-Verhalten: Vorkomprimate br/gzip, ETag, 304, Range),
  `ui.api_root`/`ui.assets`/`ui.fetch_*` (RedirectViews). Branding greift wie vorher.
* Startmeldung listet, was ohne Django fehlt (Stand: Dokument-Aktionen `delete`, `rotate`, `merge`,
  `merge_as_versions`, `edit_pdf`, `remove_password`; `documents/<id>/{email,metadata,suggestions,
  update_version,versions/<id>}`, `documents/email/`, `storage_paths/test/`,
  `profile/social_account_providers/`, Mail-OAuth-Callback, allauth headless, `/admin/`,
  Social-Provider außer OIDC, DRF-`OPTIONS`-Metadaten). Die Startmeldung prüft nur Pfade, nicht
  Methoden – vollständiger, methodengenauer Abgleich in **docs/STANDALONE-GAPS.md** (2026-11-09:
  Konfiguration speichern/Logo, Speicherpfade anlegen/ändern, Nutzer löschen, Mail-OAuth-Callback
  nachgezogen; offene Teilfälle dort §4/§5).
* Stopp geordnet: Ctrl+C, SIGTERM, Windows Ctrl-Break/Konsole schließen/Abmelden, stdin-EOF.
* **Ohne Redis**: Djangos `default`-Cache liegt zentral in `pl_core::cache`
  (`AppState::cache()`). Standard ist der prozesslokale Cache (Semantik `LocMemCache`: TTL, LRU,
  300 Einträge) für Tag-Baum-Invalidierung, Dokument-Caches, Login-Throttle, TOTP-Replay-Sperre,
  allauth-Rate-Limits, Session-Cache und LLM-Vorschläge. Für den Mehrprozessbetrieb
  (`serve --no-worker` + `serve --worker-only` oder mehrere Instanzen) gibt es
  `PAPERRUST_CACHE=postgres`: gemeinsamer Cache in der UNLOGGED-Tabelle `paperrust_cache`
  (Migration `pl_core.0001_paperrust_cache`). `PAPERLESS_CACHE_BACKEND=…RedisCache` bzw.
  `…DatabaseCache` wird als `postgres` gelesen; eine Redis-Verbindung gibt es nie.
  Geschichte: In W23 gab es noch den Redis-Weg mit `PAPERRUST_REDIS_CONNECT_TIMEOUT`/
  `PAPERRUST_REDIS_RETRY_AFTER` (vorher, W22: 5–54 s Wartezeit pro Schreibanfrage); beides ist
  seit 2026-11-14 entfernt.
* Prüfung: Parität des Frontends gegen Django (`tests/parity/pl-server/cases.json`), Browser-Durchlauf
  ohne Django unter `results/e2e/standalone/`. Details: `docs/contract/pl-server.md`
  („Frontend und Betrieb ohne Django“).

## Entscheidungen

- 2026-11-07: Desktop-App bringt **eingebettetes PostgreSQL** mit (ein Codepfad, volle Parität, Umzug Server↔Desktop per Kopie). SQLite bleibt optional für später.
- Echte Nutzerdaten als Testdatensatz: vorerst nicht.

## Beispiel-Bestand des Nutzers (Umzugsweg prüfen)

paperless-ngx 3.1.3 (Docker), PostgreSQL 17.4, Redis 7.4.2, Gotenberg 8, Tika 2.9.2.1.
Weg: Backup → paperless-ngx-Image auf 3.4.2 heben und einmal starten (Migrationen) → PaperRust auf
dieselbe DB und denselben Datenordner zeigen lassen. Kein DB-Umzug nötig (schon PostgreSQL).
