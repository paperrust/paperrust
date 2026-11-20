# PaperRust-Design („zweite Oberfläche“)

Stand 2026-11-10. Entscheidung: docs/UI-ZWEITES-DESIGN.md (Konzepte A „Arbeitsplatz“, D „Lebensbereiche &
Zeitstrahl“, E „Assistent“; statische Menüführung, Akzent Rost). Technik und Infrastruktur wie die klassische
Light-UI (docs/FRONTEND-LIGHT.md), Backend-Vertrag für Zeitstrahl/Fragen: docs/contract/pl-api-v2.md
(Abschnitt „PaperRust-Oberfläche (Design 2)“).

**Standard (seit 2026-11-17, Entscheidung des Besitzers):** Ohne eigene Wahl öffnet jeder Benutzer das
PaperRust-Design; die klassische Oberfläche bleibt in Einstellungen → Darstellung wählbar
(`designOf` in `frontend-light/src/lib/design-switch.ts`).

## 1. Aufbau

- **Ort:** `frontend-light/src/design2/` – eigene Bausteine und eigenes CSS, **keine** Angular-Kapselung, kein
  Bildvergleich gegen Angular. Gemeinsam mit der klassischen Light-UI: API-Client (`src/api`), Stores
  (`src/stores`: Sitzung/ui_settings, Stammdaten, Uploads + Websocket, Meldungen), Filterlogik
  (`src/lib/filter-rule-type.ts`, `query-params.ts`), i18n (`src/lib/i18n.ts`, Katalog `i18n/<locale>.json`), Router.
- **Einstieg:** eigene Vite-Seite `frontend-light/m.html` → `src/design2/main.ts`. URL **`/rs/app/m/…`**.
  pl-server liefert für `/rs/app/m` und `/rs/app/m/…` `m.html` statt `index.html`
  (`crates/pl-server/src/light_ui.rs`, `spa_entry`; ohne `m.html` im Build bleibt alles beim Alten); `vite
  dev`/`preview` machen dasselbe (Plugin `paperrust-modern-entry` in `vite.config.ts`).
- **Gleiche Routen wie die klassische Oberfläche**, nur unter `/m`: `router.svelte.ts` setzt die Basis auf
  `/rs/app/m`, wenn die Seite dort geladen wurde. Dadurch funktionieren eingebettete klassische Bausteine
  (Verwaltung, Einstellungen) unverändert, und Lesezeichen lassen sich zwischen beiden Oberflächen
  übertragen. Eigene Routen: `inbox[/:id]`, `view/:id[/:doc]`, `assistant`; `dashboard` = Übersicht.
- **Nicht angefasst:** Markup/CSS der klassischen Bildschirme. Additive Haken: Eintrag „Zum PaperRust-Design
  wechseln“ im klassischen Benutzermenü, Umleitung des klassischen Einstiegs (`/rs/app/`, `dashboard`) bei
  gewähltem Design, `BASE`-Erkennung im Router, optionaler Kern-Loader in `loadLocale`, `ng-markup` überspringt
  `src/design2/`, `route-icons.test` prüft `design2` nicht gegen die Angular-Symbolliste.

| Ordner | Inhalt |
|---|---|
| `App.svelte`, `main.ts` | Rahmen, Bereichs-Lader, Tastatur, Upload-/Verarbeitungsmeldungen |
| `components/` | Header, Nav, TabBar (mobil), Menu, SideSheet, ObjectPicker, CommandPalette, ShortcutsHelp, Toasts, UploadTray, DropZone, Icon |
| `workspace/` | Arbeitsplatz: FilterBar, DocList, BulkBar, DocDetail (Preview, MetaForm, Notes, History) |
| `timeline/Overview.svelte` | Übersicht = „Demnächst“ + Jahres-Zeitstrahl |
| `assistant/Assistant.svelte` | Fragen ans Archiv |
| `manage/ManageHost.svelte`, `settings/SettingsHost.svelte` | klassische Bausteine im neuen Rahmen (`classic.css`) |
| `api/assist.ts`, `api/mock.ts`, `api/docs.ts` | v2-Client Zeitstrahl/Lebensbereiche/Fragen (+ Demo-Adapter), v1-Massenaktionen |
| `lib/` | reine Logik: `autosave`, `keys`, `palette`, `filters`, `timeline`, `theme`, `bus`, `icons` (+ Vitest) |
| `stores/` | `workspace` (Listen-Kontext, Auswahl), `ui` (Palette, Thema, Breakpoints), `counts` |

## 2. Design-System

Einzige Quelle: `src/design2/styles/tokens.css` (Hell auf `:root`, Dunkel per `[data-theme=dark]` bzw.
System bei `[data-theme=auto]`). Kontraste prüft `frontend-light/tools/contrast-design2.test.mjs` gegen diese Datei.

| Token | Hell | Dunkel | Einsatz |
|---|---|---|---|
| `--d2-bg` | `#F6F5F3` | `#141311` | Fläche hinter Panes, Navigation |
| `--d2-surface` / `-2` / `-3` | `#FFF` / `#F1EFEC` / `#E8E5E1` | `#1C1B19` / `#252320` / `#302D29` | Panes, Hover, Auswahl/Chips |
| `--d2-border` / `-strong` | `#DEDAD5` / `#B9B3AB` | `#36332E` / `#5A554E` | Trennlinien / Eingabefelder |
| `--d2-text` / `-2` / `-3` | `#1E1C1A` / `#57524C` / `#6B655E` | `#F2EFEB` / `#C9C3BB` / `#A39C93` | Text, sekundär, gedämpft (alle ≥ 4,5:1) |
| `--d2-accent` | `#B7410E` | `#B7410E` | Rost: Primärknopf, Fokus, Marken, aktive Linie |
| `--d2-accent-text` | `#A63B0D` | `#F08A5D` | Akzent als Text (Links, aktive Navigation) |
| `--d2-accent-soft` | `#FBEDE6` | `#3A2117` | aktive Zeile/Navigation |
| `--d2-ok/warn/danger/info` (+`-soft`) | grün/ocker/rot/blau | aufgehellt | Status, Dringlichkeit |

- **Schrift:** Inter (variabel, `@fontsource-variable/inter`, nur die benötigten Unicode-Bereiche werden geladen),
  Ersatz `system-ui`. Stufen 12/13/14/16/20/24 px (`--d2-fs-xs … -2xl`), Grundtext 14 px, Zeilenhöhe 1,45,
  Gewichte 400/500/600/700.
- **Abstände:** 4-px-Raster `--d2-s1…s6` = 4/8/12/16/24/32. **Radien:** 6 (Bedienelemente), 10 (Karten/Panes),
  Pille (Chips).
- **Maße:** Kopfleiste 52, Navigation 232 (Symbolleiste 64), Liste 380, Metadaten 340, Bedienelement 32,
  Mobile Reiterleiste 56 px.
- **Bausteine (global, `styles/base.css`):** `.d2-btn` (`primary`, `ghost`, `danger`, `icon`, `sm`),
  `.d2-input/-select/-textarea`, `.d2-label`, `.d2-check`, `.d2-chip`, `.d2-badge`, `.d2-pane`,
  `.d2-pane-head`, `.d2-menu(-item/-sep/-label)`, `.d2-kbd`, `.d2-empty`, `.d2-scroll`, `.d2-sr`.
  Komponenten: `Menu` (fest positioniert am Auslöser, Esc/Klick außen, Pfeiltasten), `SideSheet`
  (Dialog als Seitenblatt mit Fokusfalle), `ObjectPicker` (Suche, einfach/mehrfach, „nicht zugewiesen“, als
  Filter-Chip oder Formularfeld), `Icon` (bootstrap-icons per `?raw`, nur benutzte Symbole).
- **Barrierefreiheit:** echte Knöpfe/Links/Labels, sichtbarer Fokus (2 px Rost), `role`/`aria-*` für Listbox,
  Reiter, Dialoge, Live-Regionen (Speicherstatus, Meldungen), „Zum Inhalt springen“, `prefers-reduced-motion`.
  Kontrast ≥ 4,5:1 für allen Text (Tokens per Vitest, gerenderte Shell-Texte hell+dunkel per Playwright).

## 3. Layout-Regeln „nichts scrollt als Ganzes“

1. `html, body { height: 100%; overflow: hidden }`; Rahmen = Grid `52px | 1fr` (mobil `52px | 1fr | 56px`) in
   `100dvh`. Kopfleiste und Navigation sind fest – Inhalte darunter verschieben sie nie.
2. Jeder Bereich füllt `#d2-main` und teilt sich in **Panes** mit `min-height: 0`. Gescrollt wird nur in
   Elementen mit `.d2-scroll` (Dokumentliste, Metadaten-Reiter, Vorschau, Zeitstrahl-Bahnen, Gesprächsverlauf,
   klassischer Baustein in der Verwaltung). Kopfzeilen von Panes (`.d2-pane-head`) bleiben stehen.
3. Menüs/Auswahllisten sind `position: fixed` am Auslöser (kein Abschneiden in scrollenden Panes), Dialoge
   erscheinen als Seitenblatt rechts (eigene und – per `classic.css` – die der klassischen Bausteine).
4. Breakpoints: ≥ 1100 px Navigation mit Text; 768–1099 px Navigation als Symbolleiste; < 768 px Navigation als
   Schublade + untere Reiterleiste, Arbeitsplatz zeigt **entweder** Liste **oder** Dokument (Zurück-Knopf),
   Dokument mit Reitern Vorschau/Details/Notizen/Verlauf/Inhalt. Das Dokument-Pane schaltet ab < 760 px
   eigener Breite auf Reiter um (Vorschau als Reiter).
5. Geprüft: `document.scrollingElement.scrollHeight ≤ innerHeight` und keine horizontale Seitenrolle auf 9
   Bildschirmen in 1440×900, 1280×720 und 390×844 (`tests/design2/e2e.mjs`, Test `no-page-scroll`).

## 4. Bildschirme

| # | Route (`/rs/app/m/…`) | Inhalt |
|---|---|---|
| 4.1 | Rahmen | Logo, Suchfeld = Befehlspalette (Ctrl+K), Hochladen (Fortschritt), Glocke (Meldungen), Benutzermenü (Hell/Dunkel/System, Umschalter, Tastenkürzel, Einstellungen, Abmelden); Navigation: Übersicht, Posteingang (Zähler), Alle Dokumente (Zähler), Assistent, gespeicherte Ansichten der Seitenleiste (Zähler), Schnellfilter (ohne Korrespondent/Typ/Tags, diesen Monat), unten Verwaltung, Einstellungen; Ablegen von Dateien überall |
| 4.2 | `documents`, `inbox`, `view/:id` (+`/:doc`) | **Arbeitsplatz** (Konzept A): Liste mit Suche Titel & Inhalt, Filter-Chips Tags/Korrespondent/Typ/Speicherpfad (mehrfach, „nicht zugewiesen“), Zeitraum, Sortierung; URL-Parameter wie die klassische Liste (`text`, `tags__id__all`, `correspondent__id__in`, `…__isnull`, `created__date__gte`, `sort`, `reverse`); fortlaufendes Nachladen (50 je Seite); Auswahl (Kästchen, Shift/Strg-Klick, `x`, Ctrl+A = alle Treffer) und **Massenaktionen** (Tag hinzu/weg, Korrespondent, Typ, ZIP-Download, Löschen mit Bestätigung) über v1 `bulk_edit`/`bulk_download`. Regeln aus Ansichten/Schnellfiltern, die die Leiste nicht einzeln zeigt, erscheinen als „+n Regeln“ |
| 4.3 | `documents/:id` | Dokument rechts: Vorschau (pdf.js-Baustein der Light-UI, nachgeladen; Bild; Text; sonst Vorschaubild) mit Seite/Zoom; **Metadaten inline mit Autosave**: Titel, Datum, ASN, Korrespondent, Typ, Speicherpfad, Tags, Custom Fields (Text/Zahl/Datum/Ja-Nein/Auswahl/URL/Betrag/Langtext anlegen, ändern, entfernen); nur die geänderten Felder, **über API v2 mit `If-Match`** (UUID/rev per `POST /documents/query` mit Datum + Titel und Vergleich der `legacy_id` ermittelt, Referenzen über die v2-Objektlisten in UUIDs übersetzt; `api/v2docs.ts`), sonst – v2 nicht erreichbar, Dokument nicht gefunden, Dokumentverknüpfungs-Feld – v1-PATCH; 412 → „Dieses Dokument wurde inzwischen anderswo geändert“ + „Neu laden“; Text 700 ms entprellt + beim Verlassen, Auswahlfelder sofort, höchstens eine Anfrage gleichzeitig, Status „Speichert…/Gespeichert/Nicht gespeichert – Erneut versuchen“ (`lib/autosave.ts`); Reiter Notizen (anlegen/löschen), Verlauf (Audit-Log), Inhalt; Kopf: Herunterladen, „in klassischer Oberfläche öffnen“, Löschen, Schließen. Schreibschutz bei fehlendem Recht |
| 4.4 | `dashboard` | **Übersicht** (Konzept D): „Demnächst“ (`GET /timeline/upcoming?days=60&overdue_days=30`; Karten mit Resttagen/Überfällig, Art, Betrag, „Erledigt“, „7 Tage zurückstellen“ → `PUT /timeline/items/{id}/state`) und Jahres-Zeitstrahl (`GET /timeline?from&to`): Bahnen je Lebensbereich (+ „Sonstiges“), Monatsachse, Heute-Linie, Fristen als Raute, Laufzeiten/Garantien als Balken (`span`), Dokumentdaten als Punkt (zuschaltbar), Klick → Dokument; Jahr vor/zurück. Ohne Backend: Hinweis + „Demo ansehen“ |
| 4.5 | `assistant` | **Assistent** (Konzept E): Gesprächsliste (`GET /ask/conversations`), Frage → `POST /ask` (SSE `meta`/`citations`/`delta`/`actions`/`done`/`error`, oder JSON), Antwort wird gestreamt, `[n]` im Text springt zur Quelle, Quellen-Chips (Titel, Seite) öffnen das Dokument, Aktionen `open_documents` und `create_reminder` (führt die mitgelieferte v2-Anfrage aus), Hinweise (`notes`) bei Index/LLM ohne Antwort, Stopp-Knopf. **KI aus** (`ai_enabled` falsch bzw. `answer: null`): Hinweis schon auf der Startseite und je Antwort „KI ist aus – keine ausformulierte Antwort“, darunter die Fundstellen mit Ausschnitt; keine Treffer → eigener Hinweis. Gespräche aus dem Verlauf lassen sich wieder öffnen (`GET /ask/conversations/{uuid}`) |
| 4.6 | `attributes/:section`, `savedviews`, `workflows`, `mail`, `tools/asn`, `usersgroups`, `tasks`, `logs`, `trash`, `config` | **Verwaltung**: eigene Bereichsnavigation links (nach Rechten), rechts der Baustein der klassischen Light-UI in einem Scroll-Pane; Bootstrap-Stil wird erst hier nachgeladen, Bootstrap-Variablen auf die Tokens abgebildet, Dialoge als Seitenblatt |
| 4.6a | `automation[/workflows]`, `automation/workflows/new\|:id`, `automation/mail[/new\|:id]`, `automation/review` | **Automatik** (docs/WORKFLOWS-NEU.md, `src/design2/automation/`): Übersicht (Schalter, Satz, n× diese Woche, Reihenfolge), Vorlagen-Galerie + Vorschläge + „mit KI beschreiben“, Editor (Satz mit klickbaren Teilen, Karten Wenn → Nur wenn → Dann → Sonst, Seitenblätter mit allen Optionen, KI-Bedingung), Testlauf + Anwenden, Verlauf, E-Mail-Abruf (Mail-Regel als Auslöser, `/mail-flows`), KI-Prüfliste. `workflows` und `ai/review` leiten hierher um; Wächter-Test `automation/automation.test.ts` |
| 4.7 | `settings[/interface]`, `settings/{general,documents,permissions,notifications,system}` | **Einstellungen**: eigener Reiter „Oberfläche“ (Design-Umschalter als Standard, Hell/Dunkel/System, Tastenkürzel) + die klassischen Reiter (ihre eigene Reiterleiste ausgeblendet) |
| – | `?`, Ctrl+K | Tastenkürzel-Blatt, Befehlspalette (Ziele, Ansichten, Schnellfilter, Aktionen, Volltext „Dokumente nach … durchsuchen“, „Assistent fragen“, Dokument-/Objekt-Treffer aus v1 `/api/search/`) |

## 5. Tastatur

| Taste | Wirkung |
|---|---|
| Ctrl/⌘ + K | Befehlspalette und Suche (überall, auch in Eingabefeldern) |
| `/` | Suche der Liste fokussieren (sonst Palette) |
| `?` | Tastenkürzel |
| `j` / `k` | nächstes / vorheriges Dokument öffnen (lädt nach) |
| ↑ / ↓, Pos1/Ende, Enter/Leertaste | in der fokussierten Liste bewegen / öffnen |
| `x` | aktives Dokument aus-/abwählen; Ctrl+A alle Treffer |
| `e` | Titel bearbeiten |
| `u` | Hochladen |
| `t` | Hell → Dunkel → System |
| `g` dann `o`/`i`/`d`/`a`/`m`/`s` | Übersicht / Posteingang / Dokumente / Assistent / Verwaltung / Einstellungen |
| Esc | Menü/Blatt/Palette schließen, Auswahl aufheben, mobil zurück zur Liste |

Ein-Tasten-Kürzel wirken nicht, solange ein Eingabefeld den Fokus hat; offene Dialoge behandeln ihre Tasten
selbst. Logik: `lib/keys.ts` (`KeyInterpreter`), Weitergabe an den aktiven Bereich über `lib/bus.ts`.

## 6. Umschalten zwischen den Oberflächen

- Je Benutzer in ui_settings: `settings.paperrust.design = "classic" | "modern"` (Schlüssel
  `paperrust:design`, Standard klassisch). Gespeichert über v1 `POST /api/ui_settings/` wie alle UI-Einstellungen.
- **Klassisch → modern:** Benutzermenü „Zum PaperRust-Design wechseln“ (speichert `modern`, öffnet dieselbe
  Route unter `/m`). **Modern → klassisch:** Benutzermenü „Zur klassischen Oberfläche wechseln“ bzw.
  Einstellungen → Oberfläche (speichert `classic`, öffnet die gleiche Route ohne `/m`; eigene Routen:
  `inbox` → `documents?is_in_inbox=1`, `assistant` → `dashboard`). Logik: `src/lib/design-switch.ts`.
- Mit `modern` leitet die klassische Oberfläche ihren **Einstieg** (`/rs/app/`, `/rs/app/dashboard`) nach
  `/rs/app/m/` um; Deep-Links (`/rs/app/documents/12`) bleiben klassisch. Die URL entscheidet immer.
- `?design=classic|modern` auf einer beliebigen Route übernimmt die Wahl (speichert in ui_settings, entfernt den
  Parameter, klassisch → modern leitet um). Das nutzt das **Desktop-Tray** (`apps/desktop-shell/src/design.rs`):
  Einträge „Neue Oberfläche (PaperRust-Design)“ / „Klassische Oberfläche“ (Modus lokal und fern) öffnen im
  Hauptfenster dieselbe Route der anderen Oberfläche; auf lokalen App-Seiten wird nur das Fenster gezeigt.
- **Angular-Oberfläche unter `/`** (frontend/patches/13-paperrust-design-switch.patch): Benutzermenü oben rechts
  „Neue Oberfläche ausprobieren“ öffnet `<base>/rs/app/m/?design=modern` (speichert `modern`). Mit `modern` leitet
  der Angular-Einstieg (`/`, `/dashboard`) beim Laden nach `/rs/app/m/` um, Deep-Links bleiben klassisch.
- **Desktop-Start:** die Hülle merkt sich die letzte Wahl (Tray-Eintrag oder jede Navigation mit `?design=…`) in
  `app.toml` `[ui] design = "modern" | "classic"` und öffnet beim Start (lokal und Fern-Profile mit PaperRust-Server)
  bei `modern` direkt `<base>/rs/app/m/` statt `/` (`design::start_url`). Tray-Tooltip: „PaperRust – Rechtsklick für Menü“.

## 7. Backend Zeitstrahl / Fragen

`api/assist.ts` setzt den Vertrag aus docs/contract/pl-api-v2.md um (`/life-areas`, `/timeline`,
`/timeline/upcoming`, `/timeline/items/{id}/state`, `/ask` mit SSE, `/ask/conversations`) und übersetzt in
UI-Typen (Dokumente über `legacy_id`). Antwortet der Server mit 404/405/501/503 oder (ältere Server ohne v2)
mit einer HTML-/Umleitungsseite, zeigen Übersicht und Assistent „noch nicht verfügbar“. **Demo-Adapter**
(`api/mock.ts`, echte sichtbare Dokumente + erfundene Fristen, deutlich als „Demo-Daten“ markiert) nur mit
`?mock=1` oder `localStorage['paperrust.design2.mock']='1'` – die Ablauftests setzen das, solange der
Testserver die Endpunkte nicht hat.

## 8. Budget

`node tools/budget-design2.mjs [--dist dist] [--lang de-DE] [--out …]` (Exit 1 bei Überschreitung):
erster Aufruf = `m.html`-Einstieg (Rahmen + Arbeitsplatz) + Übersicht + Kern-Wörterbuch **≤ 150 KB brotli**
(JS + CSS). Kern-Wörterbuch: `tools/i18n-split.mjs` erzeugt zusätzlich `i18n/core-m/` (Texte aus `design2/`
ohne `manage`/`settings`, `lib`, `stores`, `api`); die volle Sprachdatei lädt im Hintergrund, Verwaltung und
Einstellungen warten darauf. Stand 2026-11-10 (de-DE): **82,7 KB** erster Aufruf; nachgeladen Assistent
4,7 KB, Befehlspalette 3,0 KB, Upload-Liste 1,6 KB, Verwaltung/Einstellungen je ≈ 25 KB (+ Baustein-Chunk der
klassischen Oberfläche, Bootstrap-CSS), pdf.js nur beim ersten PDF. Klassischer erster Aufruf unverändert im
Budget (`tools/budget.mjs`).

## 9. Tests

| Art | Befehl | Inhalt |
|---|---|---|
| Vitest | `pnpm test` | `src/design2/lib/design2.test.ts` (Autosave, Tastatur, Palette, Filter↔URL, Routen, Zeitstrahl-Geometrie/Demnächst, Vertrag-Normalisierung + SSE-Parser, Umschalter, Thema), `tools/contrast-design2.test.mjs` (Tokens hell/dunkel) |
| Typen | `pnpm check` | svelte-check ohne Fehler in `design2` |
| Playwright | `node tests/design2/e2e.mjs --base <preview> --api <server> [--ai off\|llm] [--out …] [--only id] [--no-shots\|--shots-only]` | Umschalten (Menü und `?design=`, ui_settings geprüft, Einstieg-Umleitung), **Autosave über v2 (`data-via=v2`, Titel per v2 geprüft) und Konflikt 412 → „Neu laden“**, „Demnächst“-Karten = Server, „Erledigt“ per API geprüft, Assistent mit KI aus (Hinweis + Fundstellen) bzw. Mock-LLM (gestreamte Antwort mit `[n]`), Verlauf öffnen, Dokument öffnen, **Autosave per API geprüft** (Titel, Korrespondent), Hochladen mit Fortschritt, Tastatur (j/k, x, Esc, Ctrl+K, g d, ?, ↑/↓ + Enter), **kein Seiten-Scrollen** (9 Bildschirme × 1440×900/1280×720/390×844), Zeitstrahl-Marke → Dokument, Assistent (Quellen → Dokument), Kontrast gerenderter Texte hell/dunkel; Bildschirmfotos hell/dunkel Desktop + mobil nach `results/e2e/design2/<datum>/` |
| Rust | `cargo test -p pl-server --lib light_ui`; `cargo test` in `apps/desktop-shell` | `/rs/app/m…` → `m.html`, sonst `index.html`; Tray-Einträge und Ziel-URL des Umschalters |

Stand 2026-11-10: Lauf gegen das **echte Backend** (Debug-Build von HEAD 40d5e24, DB-Kopie mit
`paperrust manage migrate`, 6 Datums-/Betrags-Custom-Fields an ~150 Feldwerten relativ zu heute, Lebensbereiche
aus `/life-areas/suggestions` als Instanz-Vorgabe übernommen) je einmal mit **KI aus** und mit **Mock-LLM**
(lokaler Ollama-Ersatz `/api/chat` mit NDJSON, wie `crates/pl-server/tests/design2_db.rs`): 10/10 Abläufe grün,
`results/e2e/design2/2026-11-10/report-ai-{off,llm}.json`.

Testserver wie die klassische Light-UI (`tests/server.sh` mit eigener DB-Kopie/Port/Arbeitsordner, z. B.
`F0_DB=pl_design2_ui F0_PORT=8351 F0_PREVIEW_PORT=4191`), Vorschau
`PAPERRUST_URL=http://127.0.0.1:8351 npx vite preview --outDir <dist> --port 4191`. `PW_CHROME=<exe>` nimmt
einen vorhandenen Chromium/Edge, falls die Playwright-Browser fehlen.

## 10. Offene Punkte

1. Autosave über v2 braucht je Dokument eine Abfrage zur UUID-Ermittlung (v2 kennt keinen Filter auf
   `legacy_id`); Dokumentverknüpfungs-Felder und nicht auflösbare Dokumente gehen weiter über v1 (ohne
   Konfliktschutz). Besser: v2-Dokumentliste mit Filtern im Arbeitsplatz oder ein `legacy_ids`-Filter in `DocQuery`.
2. Lebensbereiche: Einstellungen → Oberfläche (wirksame Liste, Vorschläge übernehmen, Name/Farbe/Tags/Typen,
   Reihenfolge; Speichern legt eigene Bereiche an, „Vorgabe der Instanz verwenden“ löscht sie). Instanz-Vorgabe
   bearbeiten (Superuser) und Regeln nach Speicherpfad/Korrespondent nur über die API. Erinnerungen manuell anlegen nur über Assistenten-Aktion.
3. Verwaltung zeigt bei Attributen zusätzlich die klassische Reiterleiste; klassische Seitenköpfe/Tabellen
   behalten Bootstrap-Optik (Farben/Schrift angeglichen).
4. Filter: Custom-Field-Abfragen, Besitzer/Rechte, ASN, Volltext-„Mehr wie dieses“ nur über Ansichten/URL
   (werden angezeigt als „+n Regeln“, bearbeitbar in der klassischen Liste).
5. PDF-Editor, Versionen, Freigabe-Links, E-Mail aus dem Dokument: „in klassischer Oberfläche öffnen“.
