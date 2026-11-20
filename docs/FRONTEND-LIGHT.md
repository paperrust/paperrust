# PaperRust – leichte Oberfläche („Light-UI“)

Stand 2026-11-09. Entscheidungsvorlage. Rahmen: docs/ARCHITECTURE-V2.md, docs/SYNC.md.

> **Seit 2026-11-13 ist die Light-UI die einzige Oberfläche.** Die Angular-Oberfläche (`frontend/`) ist
> gelöscht, `/` und alle alten Angular-Adressen leiten auf `/rs/app/…`. Was die Light-UI gegenüber Angular
> v3.4.2 abdeckt, steht in **docs/FRONTEND-PARITY.md**. Hinweise unten auf „klassische Oberfläche öffnen“
> und den Bildvergleich gegen Angular beschreiben den Weg dorthin (Geschichte).

## 0. Bindende Vorgaben

- **Sieht genauso aus wie die heutige Oberfläche** (Angular aus `upstream/src-ui` + PaperRust-Patches in
  `frontend/patches/`): gleiches Layout, gleiche Navigation, gleiche Bildschirme, gleiche Farben/Themen
  (Bootstrap 5, Hell/Dunkel, Akzent Rost `#B7410E` aus Patch 07, Logos aus Patch 06), gleiche Texte und
  Übersetzungen. **Kein Redesign.** Nur leichter, schneller (kleines Bundle, schneller erster Aufruf),
  offline-fähig.
- Funktionsumfang wächst in Stufen; **jeder vorhandene Bildschirm muss Optik und Verhalten des Originals
  treffen** (Parität-Prüfliste §7, Bildvergleich §9).
- Die Angular-UI bleibt unter `/` (Kompatibilität). Light-UI spricht die native API v2, übergangsweise v1 (§6).
- Später auch in Tauri mobil.

## 1. Ausgangsmessung Angular (gemessen, `frontend/dist/frontend/en-US/`, Build vom 2026-11-09)

| Datei | roh | gzip | brotli |
|---|---:|---:|---:|
| `main.js` (ganze App, ein Chunk) | 3 107 324 B | 829 851 B | 650 528 B |
| `styles.css` (Bootstrap 5.3 + ng-select + paperless) | 280 550 B | 38 392 B | 28 244 B |
| `assets/js/pdf.min.mjs` (pdf.js) | 523 774 B | 151 438 B | 126 707 B |
| `assets/js/pdf.worker.min.mjs` | 1 321 307 B | 392 144 B | 322 590 B |

Erster Aufruf heute ≈ **680 KB brotli** (JS+CSS) bevor irgendetwas angezeigt wird; Angular 22,
ng-bootstrap 21, ng-select 24, pdfjs-dist 6.4 (`upstream/src-ui/package.json`).

**Budget Light-UI** (CI-Grenze, brotli): erster Aufruf (Rahmen + Dokumentliste, eine Sprache) **≤ 120 KB
JS + ≤ 30 KB CSS**; jede weitere Route ≤ 40 KB lazy; pdf.js unverändert, nur lazy in der Dokumentansicht.

## 2. Framework-Wahl: **Svelte 5** (ohne SvelteKit) + Vite

| | Svelte 5 | SolidJS | Preact (+signals) | Leptos (Rust/WASM) |
|---|---|---|---|---|
| Laufzeit-Grundlast (Literatur, min+gz) | ~3–10 KB (nur genutzte Teile) | ~7 KB | ~4–5 KB (+1,5 KB signals) | WASM ~60–150 KB br schon für kleine Apps, real oft 300 KB–1 MB roh |
| Wachstum je Bildschirm | kompiliert, etwas mehr Code je Komponente | klein | vDOM, klein | groß (WASM) |
| **Portierung der Angular-Templates** | HTML-Templates mit `{#if}`/`{#each}`, `class:`-Bindung → Markup und Klassen fast 1:1 übernehmbar | JSX | JSX | `view!`-Makro |
| Globales SCSS/Bootstrap übernehmen | ja (Vite + sass) | ja | ja | ja (Trunk/cargo-leptos) |
| Tauri Desktop/Mobil, PWA | ja | ja | ja | ja |
| Build auf 8-GB-Laptop | Sekunden, wenig RAM | dito | dito | Rust→WASM-Builds, hoher RAM-Bedarf (Projektregel: Builds nur bei freiem RAM) |
| Wartbarkeit/Personen | groß, stabil (Svelte 5 seit 10/2024) | mittel | groß (React-Wissen) | klein |

**Begründung:** Die Optik-Vorgabe macht das Übernehmen des **Markups** der Angular-Komponenten
(`*.component.html`) zur Hauptarbeit. Svelte-Templates sind HTML mit Steuerblöcken – Bootstrap-Klassen,
Struktur und `i18n`-Texte lassen sich Zeile für Zeile übertragen; bei JSX müsste jedes Template
umgeschrieben werden (`class`→Ausdrücke, Kontrollfluss in JS). Svelte kompiliert ohne vDOM, liefert kleine
Bundles und braucht keine Laufzeit-Bibliothek für Reaktivität (Runes). Leptos verworfen: Bundle-Größe,
Build-Ressourcen, kleinerer Kreis. Preact/Solid technisch gleichwertig klein, aber teurer in der Portierung.

**Vorbehalt:** Literaturwerte, nicht selbst gemessen. Paket F0 baut die Dokumentliste prototypisch in
Svelte 5 und misst gegen das Budget; reißt es das Budget um > 30 %, wird Solid mit demselben Markup
gemessen, bevor weitergebaut wird.

Zusatzbibliotheken (klein halten, je Bibliothek Begründung im PR):
- Bootstrap 5.3 **nur CSS/SCSS** (gleiche Version wie upstream), kein Bootstrap-JS, kein jQuery.
- Positionierung von Dropdowns/Popovern: `@floating-ui/dom` (~3 KB gz) mit den Optionen aus
  `upstream/src-ui/src/app/utils/popper-options.ts`.
- `bootstrap-icons` (gleiche Version 1.13.x): dieselbe Registrierliste wie Angular
  (`components/common/bootstrap-icon/icons.ts`, Inline-SVG je Symbol) – nur registrierte Symbole landen
  im Bundle, gleiche Darstellung wie `<pngx-bs-icon>`.
- pdf.js (`pdfjs-dist`, gleiche Version wie upstream) lazy; gleiche Ansicht wie `common/pdf-viewer`.
- Kein Router-Framework: eigener History-Router (~1 KB), Basis `/rs/app/`, gleiche Pfade wie Angular
  (`/dashboard`, `/documents`, `/documents/:id/:section`, `/view/:id`, `/attributes/:section`,
  `/tags` → `/attributes/tags` usw., `upstream/src-ui/src/app/app-routing.module.ts`), damit Lesezeichen
  übertragbar bleiben.
- Datum: `Intl` statt Bibliothek; Datumsformate wie `utils/ngb-date-parser-formatter.ts`.

## 3. Aussehen (eigene Stylesheets)

- **Eigene Styles, keine aus paperless-ngx** (seit 2026-11-13; vorher kopiert nach `src/styles/upstream/`, siehe
  Git-Verlauf). Das Bild blieb dabei gleich (Punkt „Eigene Styles“ in §8).
- Global: `bootstrap/dist/css/bootstrap.css` aus dem npm-Paket (MIT), beim Bauen gekürzt (`tools/bootstrap-css.mjs`),
  dann `src/styles/global.scss` = `theme.scss` (Farben, Dunkelmodus), `print.css`, `base.scss` (Grundregeln), danach
  `paperrust-touch.css` (eckige PaperRust-Formsprache). Variablen heißen `--pr-*` (Akzent: `--pr-primary`,
  `--pr-primary-lightness`, gesetzt von `lib/appearance.ts`).
- Je Baustein `src/styles/components/<name>.css` (gekapselt, §8), Styles der Bibliotheks-Nachbauten (Auswahlfeld,
  Overlay, Datumsauswahl, Dialog, Toast, Ziehen) in `src/styles/parts/`.
- Markup-Konventionen von ng-bootstrap (Dropdown `.dropdown-menu.show`, Modal `.modal.d-block` +
  `.modal-backdrop`, Toast, Datepicker-Klassen `ngb-dp-*`) und ng-select (`.ng-select`, `.ng-dropdown-panel`)
  werden **mit denselben Klassen** erzeugt, damit das CSS unverändert greift.
- Dunkel-/Hell-/Systemmodus und Farbthema wie `SettingsService` (`services/settings.service.ts`):
  `data-bs-theme` am `<html>`, Akzentfarbe aus UI-Settings → CSS-Variablen `--pngx-primary*`.
- Logos/Favicon aus `docs/design/assets` bzw. `/__rs/brand/…` (wie Branding in `pl-server`).
- Barrierefreiheit: gleiche ARIA-Attribute wie das Original, zusätzlich axe-Prüfung (§9) – Verbesserungen nur,
  wenn sie das Bild nicht ändern.

## 4. Übersetzungen (eigener Katalog)

Seit 2026-11-13 hat die Light-UI einen **eigenen** Übersetzungskatalog; der aus paperless-ngx übernommene
(XLF → `i18n/<locale>.json` mit englischem Quelltext als Schlüssel, 51 Sprachen, `tools/xlf2json.mjs`,
`i18n/_keys.json`, `i18n-extra.json`) ist gelöscht.

- **Sprachen (Produktentscheidung):** en-US, de-DE, fr-FR, it-IT, es-ES, nl-NL, pt-BR, pl-PL, cs-CZ, sv-SE.
  Jede andere Sprache bekommt die englischen Texte (`catalogFor`: genau → gleiche Grundsprache, z. B.
  de-CH → de-DE, pt-PT → pt-BR → sonst en-US). Datums-/Zahlenformat folgen weiter der gewünschten Sprache
  (`locale()`), Pluralregeln der Katalogsprache (`catalogLocale()`). **Weggefallen** gegenüber ngx: die
  übrigen 41 Sprachen (u. a. ar, da, el, fi, he, hu, ja, ko, no, ro, ru, sk, sl, sr, tr, uk, zh, en-GB,
  de-CH, pt-PT) – sie zeigen Englisch. Die Sprachauswahl bietet nur die 10 Sprachen an (plus eine früher
  gespeicherte andere, damit das Feld nicht leer ist); die Liste der Datumsformate bleibt vollständig.
- **Dateien:** `i18n/<locale>.json`, flach `{ "bereich.name": "Text" }`, sortiert. Schlüssel sind eigene
  Punkt-Schlüssel (`list.sort_title`, `common.save`, `dialog.workflow.add_trigger`, `m.*` = zweite
  Oberfläche); Form `^[a-z0-9_]+(\.[a-z0-9_]+)+$`. Englische Texte in PaperRust-Wortlaut (kurz, Satzanfang
  groß, sonst klein), die anderen 9 Sprachen eigens übersetzt (de mit „Sie“ und ß; kein de-CH-Katalog).
- **Platzhalter/Plural** unverändert: `{$NAME}` (Namen wie `PH`, `INTERPOLATION`, die die Aufrufer übergeben),
  ICU `{VAR_PLURAL, plural, =1 {…} one/few/many {…} other {…}}`, `select`, Rich-Text `{$START_LINK}…{$CLOSE_LINK}`.
  pl/cs nutzen `few`/`many`.
- **Code:** `t('list.sort_title')`, `tRich(…)`, `tParts(…)`; Tabellen markieren Schlüssel mit `msg('…')` oder
  enthalten das Schlüssel-Literal direkt (z. B. `RELATIVE_DATE_LABELS`). Unbekannte Schlüssel (Namen aus der
  API in `t(name)`) bleiben unverändert stehen. Die framework-freien Modelle in `lib/filter-editor/` übersetzen
  ohne eigenen Übersetzer mit `t`.
- **Build-Prüfung** `tools/i18n-check.mjs` (läuft in `pnpm build`): jeder in `src/` benutzte Schlüssel steht
  in en-US; in `i18n/` liegen genau die 10 Dateien, jede mit allen Schlüsseln und denselben Platzhaltern.
- **Test** `tools/i18n-check.test.mjs`: dieselbe Prüfung + „nicht von ngx“: Prüfsummen
  (`tools/ngx-i18n-fingerprints.json`, SHA-256 gekürzt, keine Texte) aller Sätze ab 8 Wörtern aus dem früheren
  ngx-Katalog (englisch und je Sprache, ohne unsere eigenen Zusätze) – kein eigener Satz darf einer gleichen.
  Kürzere Standardsätze („vor %s Tagen“, „1 Seite / n Seiten“) gleichen sich bei jeder Übersetzung und sind
  ausgenommen.
- **Neuer Text:** Schlüssel in `i18n/en-US.json` **und** allen 9 anderen Dateien eintragen, sonst bricht der Build ab.
- Auslieferung: nur die gewählte Sprache wird geladen (lazy, Kern zuerst, §P4); en-US wird jetzt auch geladen.
- Sprachwahl: UI-Setting `language`, sonst die erste Browser-Sprache mit eigenem Katalog, sonst die erste
  Browser-Sprache, sonst en-US.

## 5. Projektaufbau `frontend-light/`

```
frontend-light/
  package.json  pnpm-lock.yaml  vite.config.ts  svelte.config.js  tsconfig.json
  src/
    main.ts  App.svelte  router.ts
    api/        v2.ts (typisierter Client), v1.ts (Übergang), types.ts (aus OpenAPI v2 erzeugt)
    stores/     session.svelte.ts, settings.svelte.ts, openDocuments.svelte.ts, toasts.svelte.ts
    lib/        i18n.ts, dates.ts, filterRules.ts (Portierung utils/filter-rules.ts, query-params.ts)
    components/ (Spiegel von upstream/src-ui/src/app/components/…: app-frame/, document-list/,
                 document-detail/, manage/, admin/settings/, common/…) – Dateiname = Angular-Name
    styles/     global.scss, theme.scss, base.scss, print.css, paperrust-touch.css, components/, parts/ (§3)
    sw.ts       Service Worker (PWA)
  i18n/  <locale>.json (eigener Katalog, 10 Sprachen)
  tools/ i18n-check.mjs, i18n-split.mjs, budget.mjs, ng-scope.mjs, ng-markup.js, bootstrap-css.mjs, icons.mjs
  e2e/   playwright.config.ts, specs/, visual/
```

Gleiche Ordnernamen wie in Angular erleichtern Prüfung und spätere Upstream-Übernahmen (UPSTREAM-SYNC:
geänderte Angular-Komponente ⇒ gleichnamige Light-Komponente prüfen; Liste in `UPSTREAM_MAP.md`).

## 6. Auslieferung, Anmeldung, API

- **Pfad:** `/rs/app/` von `pl-server` (statisch aus `PAPERRUST_LIGHT_UI_DIR`, Standard
  `<STATICDIR>/../light` bzw. im Paket eingebettet; gleiche Vorkomprimate br/gz und ETag/Range wie
  `static.files`). SPA-Fallback auf `index.html`. CSP: `default-src 'self'; img-src 'self' blob: data:;
  worker-src 'self' blob:` (pdf.js-Worker), kein `unsafe-inline`.
- **Anmeldung im Browser:** Session. Ohne Session → `302 /accounts/login/?next=/rs/app/…` (vorhandene,
  gleich aussehende Login-Seite). CSRF aus Cookie `csrftoken` → Header `X-CSRFToken`.
- **In Tauri:** die App lädt die Light-UI aus dem Paket; API-Basis `prapi://localhost/rs/api/v2/`
  (Client-Kern, docs/REMOTE-APP.md §5), eigener Login-Bildschirm im Markup der Server-Login-Seite
  (Template aus `pl-accounts`), Token holt der Kern.
- **API je Bildschirm** (v2 aus docs/SYNC.md / ROADMAP P3; „v1“ = übergangsweise, bis v2 es kann):

| Bildschirm | v2 | übergangsweise v1 |
|---|---|---|
| Rahmen (Seitenleiste, Ansichten, Suche) | `GET /rs/api/v2/me`, `/saved_views`, `/ui_settings` | `/api/ui_settings/` (Schreiben), `/api/search/` (globale Suche) |
| Dokumentliste mit Filter | `POST /rs/api/v2/documents/query` (JSON-`DocQuery`), `/tags`, `/correspondents`, … | Volltext `query=` und `more_like_id` über `/api/documents/` bis L3; `selection_data` |
| Dokumentansicht | `GET /rs/api/v2/documents/{uuid}?include=content,notes`, `/files/*` | Metadaten-Tab `/api/documents/<id>/metadata/`, Verlauf `/history/`, Notizen schreiben |
| Hochladen | – | `POST /api/documents/post_document/`, Status über `/ws/status/` |
| Tags/Korrespondenten/Typen bearbeiten | `PATCH/POST/DELETE /rs/api/v2/{tags,correspondents,document_types}` mit `If-Match` | bis P3 fertig: v1-Endpunkte |
| Dokument bearbeiten | `PATCH /rs/api/v2/documents/{uuid}` | Massenbearbeitung `/api/documents/bulk_edit/` |
| Einstellungen | `GET/PUT /rs/api/v2/ui_settings` | `/api/ui_settings/`, `/api/profile/` |
| Sync-Auswahl (nur Tauri) | `POST /rs/api/v2/sync/resolve`, lokale Kern-Endpunkte `prapi://…/local/selection` | – |

## 7. MVP-Bildschirme und Paritäts-Prüfliste

Reihenfolge = Bau-Reihenfolge. „Quelle“ = Angular-Komponente unter `upstream/src-ui/src/app/components/`.
Ein Bildschirm gilt als fertig, wenn alle Punkte der Zeile erfüllt sind und der Bildvergleich (§9) besteht.

| # | Bildschirm | Quelle | Muss gleich sein |
|---|---|---|---|
| S1 | Rahmen: Navbar, Seitenleiste (ein-/ausklappbar), globale Suche, Toasts, Logo, Version/GPL-Hinweis | `app-frame/`, `app-frame/global-search/`, `common/toasts/` | Breiten, Abstände, Icons, aktive Markierung, mobile Ansicht (< 768 px), Tastenkürzel (`services/hot-key.service.ts`) |
| S2 | Anmeldung | Server-Template (pl-accounts) | im Browser identisch (selbe Seite); in Tauri pixelgleiche Kopie |
| S3 | Dokumentliste: Tabelle, kleine/große Karten, Sortierung, Seitenwechsel, Auswahl, Filterleiste (Titel/Inhalt, Tags, Korrespondent, Typ, Speicherpfad, Datum, Custom Fields, Rechte) | `document-list/`, `document-list/filter-editor/`, `common/filterable-dropdown/`, `common/dates-dropdown/`, `common/custom-fields-query-dropdown/` | Spalten/Anzeigefelder wie gespeicherte Einstellungen, Filter-Verhalten (inkl. „nicht zugewiesen“, Und/Oder/Ausschluss), URL-Parameter wie Angular |
| S4 | Gespeicherte Ansichten in Seitenleiste und Liste | `manage/saved-views/` (Anzeige) | Reihenfolge/Sichtbarkeit aus UI-Settings |
| S5 | Dokumentansicht: Kopf, Reiter Details/Inhalt/Metadaten/Notizen/Verlauf/Rechte, Vorschau (PDF/Bild/Text), Bearbeiten + Speichern/Schließen, Vor/Zurück | `document-detail/`, `common/pdf-viewer/`, `common/input/*`, `document-notes/` | Feldreihenfolge, Validierung, „ungespeicherte Änderungen“-Dialog, Tastenkürzel |
| S6 | Hochladen: Ziehen-Ablegen überall, Upload-Widget, Fortschritt | `file-drop/`, `common/upload-status-tray/` | gleiche Overlays/Meldungen |
| S7 | Verwaltung Tags, Korrespondenten, Dokumenttypen (Liste, Filter, Bearbeiten-Dialog, Rechte, Zusammenführen-Löschen) | `manage/document-attributes/`, `common/edit-dialog/` | Spalten, Farbwahl Tags, Zuordnungsalgorithmus-Felder |
| S8 | Einstellungen (Reiter Allgemein, Benachrichtigungen, Ansichten – soweit MVP) | `admin/settings/` | gleiche Felder; nicht umgesetzte Reiter ausgeblendet mit Link „in klassischer Oberfläche öffnen“ |
| S9 | Sync-Auswahl (neu, nur Tauri) | – (neu, im Stil von `admin/settings/`) | Bootstrap-Formularstil, Texte im eigenen Katalog `i18n/` |
| S10 | Dashboard (nach MVP) | `dashboard/` | – |

Nicht im MVP (Link „klassische Oberfläche“ öffnet Angular unter `/` bzw. in Tauri-Modus 2a): Workflows,
Mail, Benutzer/Gruppen, Papierkorb, Aufgaben, Logs, Konfiguration, Freigabe-Links, PDF-Editor, Chat/KI.

## 8. Offline/PWA

- Service Worker: App-Shell (JS/CSS/Sprache/Icons) **precache**, Versionswechsel → Hinweis „Neu laden“.
- Im Browser **keine** Dokumentdaten offline (geteilte Rechner, Risiko); einzig zuletzt geladene Listen im
  Speicher. Echte Offline-Nutzung = Tauri-App mit Replikat (docs/SYNC.md). Abmelden leert Caches.
- Offline-Zustand sichtbar (Banner im Navbar-Stil), Schreibaktionen deaktiviert (bis 3b).

## 9. Build, CI, Tests

- Build: `pnpm --dir frontend-light build` → `frontend-light/dist/` (+ `.br`/`.gz`). Node ≥ 22 wie
  `frontend/`. Kein Build auf dem Laptop parallel zu Rust-Builds (RAM-Regel).
- CI (`.github/workflows/frontend-light.yml`): Lint (eslint + svelte-check), Unit (Vitest), Budget
  (`tools/budget.mjs`, bricht bei Überschreitung §1), i18n-Prüfung (§4), Playwright e2e + Bildvergleich.
- **Unit (Vitest):** Filterregeln ↔ Query-Parameter (Testfälle aus `upstream/src-ui/src/app/utils/
  filter-rules.spec.ts`, `query-params.spec.ts` übernehmen), i18n-Formatierer (ICU), Datumsformat.
- **E2E (Playwright, `tests/e2e`-Laufzeit):** gegen einen **lokalen** PaperRust-Testserver mit Kopie der
  Referenzdaten (`reference/`, Testnutzer/Token aus `reference/README.md`), eigener Port; je Bildschirm
  Ablauf (Filtern, Öffnen, Bearbeiten, Hochladen, Tag anlegen).
- **Bildvergleich Angular ↔ Light (Pflicht je Bildschirm):** `frontend-light/e2e/visual/compare.spec.ts`
  öffnet denselben Zustand (gleiche Testdaten, gleicher Nutzer, gleiche UI-Settings, Viewport 1440×900 und
  390×844, Sprache de-DE und en-US, Hell und Dunkel) einmal unter `/…` (Angular) und einmal unter
  `/rs/app/…`, blendet Dynamisches aus (Uhrzeiten, Animationen aus, Caret, Zufalls-IDs) und vergleicht
  mit `pixelmatch` (Schwelle 0,1 je Pixel). **Freigabe:** ≤ 0,5 % abweichende Pixel je Bildschirm; darüber
  Diff-Bild in `results/e2e/light-visual/<datum>/` und Abweichung entweder beheben oder einzeln in
  `frontend-light/e2e/visual/accepted.json` begründen (Muster `tests/parity/accepted.json`). Grundlage
  ist das vorhandene Screenshot-Werkzeug `frontend/e2e/screens.mjs`.
- **Verhaltensparität:** dieselben Playwright-Abläufe laufen gegen beide Oberflächen (Seitenobjekte mit
  zwei Umsetzungen), Ergebnis = gleiche API-Wirkung (DB-Stand nach dem Ablauf gleich).
- **Barrierefreiheit:** `@axe-core/playwright`, keine neuen Verstöße gegenüber Angular.
- **Leistung:** Lighthouse/Playwright-Trace: erster sichtbarer Inhalt Dokumentliste ≤ 1 s (lokal), Vergleich
  zu Angular in `results/bench/light/`.

## 10. Stand nach Paket F0 (2026-11-09)

Ergebnis: `results/bench/light/f0.md` (Budget, Entscheidung Svelte bestätigt) und `results/e2e/light-f0/<datum>/`
(Bildvergleich 16/16 ≤ 0,03 %). Gebaut: Rahmen (S1, ohne Upload/Benachrichtigungen/Suche-Ergebnisse) und
Dokumentliste als Tabelle (Teil S3, nur lesend über v1, Textfilter „Titel & Inhalt“, Sortierung, Seitenwechsel,
Auswahl lokal); übrige Routen = Platzhalter mit Link in die klassische Oberfläche.

Befehle (in `frontend-light/`, Paketmanager wie `frontend/`: `npx pnpm@11.15.1`):

Frischer Klon: `frontend-light/pnpm-workspace.yaml` (`allowBuilds`) gibt Build-Skripte frei bzw. lehnt sie ab – sonst bricht
pnpm 11 mit `ERR_PNPM_IGNORED_BUILDS` (@parcel/watcher, optional über sass, wird nicht gebaut).

| Schritt | Befehl |
|---|---|
| Build (+ Icons, i18n-Prüfung, .br/.gz) | `pnpm build` |
| Budget | `pnpm budget -- --lang de-DE` (Exit 1 bei Überschreitung) |
| Unit-Tests / Typen | `pnpm test`, `pnpm check` |
| Testserver (DB-Kopie `pl_light_f0`, Port 8330) | `bash tests/server.sh reset && bash tests/server.sh start` … `bash tests/server.sh drop` |
| Light-UI ausliefern (Proxy auf 8330) | `pnpm preview` → `http://127.0.0.1:4173/rs/app/` |
| Bildvergleich | `node tests/visual/compare.mjs` (Playwright/pixelmatch aus `tests/e2e`) |
| Zweite Oberfläche (PaperRust-Design, `/rs/app/m/`) | docs/DESIGN2.md: `node tools/budget-design2.mjs`, `node tests/design2/e2e.mjs` |

Abweichungen/Ergänzungen gegenüber §2–§9 (bewusst, für P4 verbindlich):

- **Kapselung über Attribute statt Svelte-Scoping.** Eigene Baustein-Styles liegen als einfaches CSS (ohne
  Verschachtelung) in `src/styles/components/<name>.css`; das Vite-Plugin `scopedStyles` (`tools/ng-scope.mjs`)
  kapselt sie beim Bauen: `[sc-<name>]` je Verbundgruppe, `:host` → `[sh-<name>]`, `:host-context(…)`, ab `::deep`
  ungekapselt. Der Svelte-Präprozessor `tools/ng-markup.js` setzt die Attribute im Markup (Baustein importiert
  `styles/components/<name>.css`; Host-Tags `<pngx-x>` stehen im Markup des Eltern-Bausteins und tragen `sh-x`).
  Grund: Svelte kapselt über Klassen – das hebelt Bootstraps `a:not([href]):not([class])` aus (Links in der Tabelle
  wurden unterstrichen) und zählt Spezifität anders (`:where` ab dem 2. Vorkommen). Svelte-`<style>`-Blöcke daher
  nicht für Bausteine der klassischen Oberfläche verwenden.
- **Whitespace wie Angular** (`preserveWhitespaces: false`): `tools/ng-markup.js` entfernt reine
  Whitespace-Knoten zwischen Tags/Blöcken und hält Rand-Whitespace von Text als `{' '}` fest. Markup daher wie im
  Angular-Template formatieren (Zeilenumbrüche zwischen Tags sind bedeutungslos, Leerzeichen um Text nicht).
- **Bootstrap-CSS gekürzt:** `tools/bootstrap-css.mjs` (Vite-Plugin) kürzt `bootstrap.css` (5.3.8, npm) beim Bauen:
  eine Regel bleibt, wenn jede ihrer Klassen (außer in `:not(…)`) als Wort in `src/` bzw. `index.html`/`m.html`
  vorkommt oder zur Laufzeit zusammengesetzt wird (Liste `DYNAMIC`: `bg-*`, `alert-*`, `bs-popover-*`, Zustände …).
  Ohne Kürzen läge das CSS über dem Budget von 30 KB brotli. Wird eine neue Bootstrap-Klasse nur per
  String-Verkettung erzeugt, gehört ihr Präfix in `DYNAMIC`.
- **Eigene Styles (2026-11-13):** alle aus paperless-ngx kopierten Stylesheets (`src/styles/upstream/`, 135 Dateien
  inkl. Laufzeit-Styles von ng-select/CDK/ng-bootstrap) durch eigene ersetzt; tote Regeln (Klassen, die unser Markup
  nie erzeugt) entfielen. Prüfung: Selektoren nach dem Kapseln regelweise gegen die alte Kopie verglichen und
  Bildvergleich alter gegen neuer Build mit `tests/visual/compare.mjs` (`--angular` = alter Light-Build).
  Symbolprüfung (`tools/icons.mjs`) jetzt gegen das npm-Paket `bootstrap-icons` statt gegen die Angular-Liste.
- **i18n-Schlüssel** (bis 2026-11-13: englischer Quelltext aus den ngx-XLF-Dateien) sind seit dem eigenen
  Katalog Punkt-Schlüssel, siehe §4. Fest eingebaute englische Wörter ohne `t()` (z. B. `aria-label="Close"`,
  „selected“ für Screenreader) waren schon vorher nicht übersetzt und sind es weiterhin nicht.
- **Icons:** `tools/icons.mjs` erzeugt `icons.generated.ts` nur mit benutzten Symbolen (`icon('…')` + Liste
  `DYNAMIC`), geprüft gegen die Angular-Registrierliste; Host `<pngx-bs-icon>` steht im Eltern-Markup.
- **Pfade:** Bildvergleich als Node-Skript `tests/visual/compare.mjs` (+ `accepted.json`) mit Playwright aus
  `tests/e2e` statt `e2e/visual/compare.spec.ts`; Router `src/router.svelte.ts`. Light-UI startet in der
  Tabellenansicht (Angular-Standard: kleine Karten) – Karten folgen in P4; der Vergleich setzt Angular per
  `localStorage` auf Tabelle.
- Budget-Reserve CSS nur 1,4 KB: weitere Bildschirme brauchen Lazy-CSS je Route (Vite-CSS-Code-Splitting) oder
  weitere geprüfte Kürzungen.

### P4 – Stand (Zwischenstände, je Bildschirm eine Zeile)

Infrastruktur P4: Routen werden nachgeladen (`App.svelte` ROUTES, eigener JS+CSS-Chunk je Bildschirm;
`tools/budget.mjs` zählt Rahmen + Dokumentliste aus dem Vite-Manifest); `src/lib/dropdown.svelte.ts` = NgbDropdown-
Nachbau mit demselben Popper-Setup (@popperjs/core 2.11.8, offset [0,2], flip/preventOverflow, pngxPopperOptions);
`NgSelect.svelte` = ng-select-DOM-Nachbau; Styles der Bibliotheks-Nachbauten (Auswahlfeld, CDK-Scrolling, Dialog,
Toast …) als eigene Dateien in `src/styles/parts/`; Filter-/Listenlogik framework-frei in `src/lib/filter-editor/` und `src/lib/query-params.ts`
(Angular-Specs portiert, Vitest); `listView.svelte.ts` = DocumentListViewService (gleicher localStorage-Schlüssel,
gleiche URL-Parameter). Bootstrap-CSS zusätzlich nach Klassennutzung gekürzt (heute `tools/bootstrap-css.mjs`,
Wortliste aus der Light-UI; dynamische Familien wie `text-bg-*`, `alert-*` ausgenommen): erster Aufruf CSS 26,3 KB br.

P4 S3 Dokumentliste: grün (120/120 Fälle ≤ 0,22 %, `results/e2e/light-s5/2026-11-09-list/`; Tabelle, kleine/große Karten, offene Menüs Anzeigen/Sortieren/Ansichten/Suchziel, alle Filter-Dropdowns offen inkl. Custom-Field-Abfrage, Tag-Filter angewendet, Auswahl mit Massenbearbeitungsleiste; de/en, hell/dunkel, Desktop/Mobil). Abweichungen: Massenbearbeitung ohne Bestätigungsdialog (direkt `bulk_edit`), Aktionen/Senden/Rechte der Leiste öffnen die klassische Oberfläche; Volltext-Vorschläge ohne Popup; Datumsfelder im Datumsfilter ohne Kalender-Popup.
P4 S5 Dokumentansicht: grün (64/64 Fälle ≤ 0,10 %, `results/e2e/light-s5/2026-11-09/`; Reiter Details/Inhalt/Metadaten/
Notizen/Verlauf/Rechte/Vorschau + Aktionen-Menü, de/en, hell/dunkel, Desktop/Mobil), pdf.js 6.4.299 wie Angular (Canvas
pixelgleich), Route lazy 30,9 KB br (pdf.js eigene Chunks). Abweichungen: Kalender-Popup (ngbDatepicker) → Datumswähler
des Browsers; Neu verarbeiten, PDF-Editor, Passwort entfernen, Freigabe-Links, E-Mail, Versionen hochladen/umbenennen/
löschen und „neu anlegen“ (Korrespondent/Typ/Pfad/Tag/Feld) öffnen die klassische Oberfläche; keine Toasts, kein
WebSocket-Nachladen, TIFF-Vorschau per `<object>` statt UTIF, Text-Vorschau wird nur bei Text-Dokumenten geladen.

P4 S5b Dokumentansicht, offene Punkte: grün (96/96 Fälle ≤ 0,05 %, `results/e2e/light-s5b/2026-11-09/`; neu: Kalender-Popup offen, Anlegen-Dialog Korrespondent, Neu-verarbeiten-Bestätigung, Freigabe-Links; Fälle in `tests/visual/cases-s5b.mjs`, dazu `doc-dates-picker` in der Liste). Gebaut: ngbDatepicker-Nachbau `common/datepicker/` (Markup/Klassen `ngb-dp-*`, Monats-/Jahresauswahl, Pfeile, Fuß Heute/Schließen, Popper, Tastatur, erster Wochentag per `Intl.Locale`; nachgeladen) in `input/date`, Datumsfilter und Custom-Field-Abfrage (dort nur „fest“, ohne Umschalter fest/relativ); Fehler-/Erfolgs-Toasts wie Angular (Speichern, Löschen mit „Rückgängig“, Download, Drucken, Metadaten, Vorschläge, Versionsinhalt); WebSocket `document_updated` (Neuladen + „Document reloaded with latest changes.“ bzw. Dialog bei lokalen Änderungen; pl-server sendet das Ereignis bei PATCH derzeit nicht → nur im Code geprüft); TIFF-Vorschau per UTIF 3.1.0 (eigener Chunk 25 KB br); Anlegen-Dialoge (Korrespondent/Typ/Pfad/Tag/Feld aus `common/edit-dialog/`, neues Objekt gewählt); Freigabe-Links (inkl. PaperRust-Hinweis Desktop/LAN aus Patch 09, liest `<meta name="paperrust_share_mode">` – pl-server setzt es bisher nur im Angular-index.html), E-Mail-Dialog, Neu verarbeiten (Bestätigung + `POST /api/documents/reprocess/`). Dialoge je eigener Chunk; Route DocumentDetail 27,3 KB br JS + 1,9 KB CSS. Patch 10/11 übernommen (Vor/Zurück aus Dashboard-Widgets: `list.activateSavedViewWithDocuments`, Schließen → Dashboard; ID-Abzeichen bricht nicht um). Offen: PDF-Editor, Passwort entfernen, Versionen hochladen/umbenennen/löschen (klassische Oberfläche); E-Mail-Dialog ohne Bildvergleich (Testserver ohne E-Mail).
P4 S6 Hochladen/Benachrichtigungen: grün (Bildvergleich `dashboard-dragover`, `dashboard-notifications`, `dashboard-upload`, je 8 Fälle; 22/24 ≤ 0,28 %, 2 akzeptiert). Gebaut: Ablegen überall (`file-drop/`, Overlay), Navbar-Upload + Knopf „in Arbeit“, `common/upload-status-tray/`, `stores/uploads.svelte.ts` (POST `/api/documents/post_document/` per XHR mit Upload-Fortschritt, Status über Websocket `/ws/status/`), `stores/toasts.svelte.ts` + `common/toast(s)/` + Glocken-Menü mit echten Meldungen. Toast/Upload-Liste werden erst bei Bedarf nachgeladen (erster Aufruf JS 118,3 KB br). Abweichung (akzeptiert, `dashboard-upload/*/dark/mobile`, 1,5 %): Angular nutzt `withFetch()` und zeigt deshalb keinen Upload-Fortschritt (Balken 0 %), Light zeigt 20 % nach dem Hochladen. Testserver ohne Verbraucher → Endzustand „Upload complete, waiting...“ in beiden Oberflächen; Erfolgs-/Fehler-Toasts der Verarbeitung nur per Unit-Test geprüft.
P4 S10 Dashboard: grün (`dashboard-welcome`, `dashboard-views`, `dashboard-full`, 24/24 ≤ 0,025 %). Willkommens-Widget, Hinweis-Karte, Saved-View-Widgets (Tabelle/kleine/große Karten nach Ansicht), Statistik (`/api/statistics/`), Upload-Widget; Seitenleisten-Zähler der Ansichten (`maybeRefreshDocumentCounts`) jetzt auch im Rahmen. Hinweis: die Referenz-DB speichert die Dashboard-Ansichten von admin auf oberster Ebene (beide Oberflächen zeigen daher das leere Dashboard); `tests/visual/cases-s6s10.mjs` setzt `saved_views.dashboard_views_visible_ids` für die Widget-Fälle per API (danach `tests/server.sh reset`). Offen: Widgets per Ziehen umsortieren (cdkDrag), Tour (ngx-ui-tour), Popover der Dateityp-Leiste und Vorschau-Popover in Widget-Tabellen.

**S7 Verwaltung + S8 Einstellungen** (zweiter P4-Agent): Route `attributes/:section` (Tags, Korrespondenten,
Dokumenttypen, Speicherpfade, Custom Fields: Liste mit Namensfilter, Sortierung, Seiten, Auswahl, Massen-Rechte/-Löschen,
Bearbeiten-/Anlegen-/Löschen-Dialoge inkl. Farbwahl, Zuordnungsalgorithmus, Rechte-Akkordeon, Speicherpfad-Test) und
`settings/:section` (Allgemein, Dokumente, Berechtigungen, Benachrichtigungen; Tour/Systemstatus-Dialog → klassische
Oberfläche). Bildvergleich 152/152 ≤ 0,5 % (max. 0,10 %, `results/e2e/light-s7s8/2026-11-09/`, Fälle in
`tests/visual/cases-manage.mjs` – compare.mjs lädt `tests/visual/cases-*.mjs`, Aktionen zusätzlich `fill`/`waitFor`).
Gemeinsame Ergänzungen: `PageHeader` mit `subTitle`/`loading`/`info` (Info-Popover nachgeladen, nicht im ersten Aufruf),
`PermissionsForm accordion`, `ModalHost` setzt den Fokus wie NgbModal (erstes fokussierbares Element). Routen lazy:
Verwaltung 24,4 KB br, Einstellungen 16,1 KB br. Gespeicherte Ansichten (`/savedviews`) noch nicht portiert.

**P4 Agent B – Dashboard ziehen, Tour, Ablauftests:** Widgets per Ziehen umsortieren wie cdkDropList/cdkDrag
(`src/lib/cdk-drag.ts`: Zeiger-Ereignisse, Schwelle 5 px, Vorschau `.cdk-drag-preview` im Elternelement mit
`popover="manual"`, Platzhalter `.cdk-drag-placeholder`, `SingleAxisSortStrategy` per transform, Ablege-Animation,
Auto-Scroll am Fensterrand; Styles in `src/styles/parts/drag.css`), speichert
`saved_views.dashboard_views_sort_order` + Toast „Dashboard updated“. Tour (`src/lib/tour/`, erst beim Start
nachgeladen, 3,6 KB br; Dashboard-Route 11,7 KB br inkl. Ziehen): 11 Schritte aus app.component.ts über alle Routen, Popover-Markup wie NgbPopoverWindow +
`<tour-step-template>`, Hintergrund `.ngx-ui-tour_backdrop`, Pfeiltasten/Escape, Ende per Navigation → completeTour;
Anker = Attribut `tourAnchor` (Rahmen, Dashboard, Upload-Widget, Dokumentliste, Filter). Start aus dem
Willkommens-Widget; Einstellungen: `import('…/lib/tour/tour.svelte').then((m) => m.tour.start())` (Knopf gehört dem
Einstellungs-Agenten). Seitenleiste: ausgeblendete Einträge mit Angulars IDs (`saved_views`, `share_links`).
Saved-View-Widget übernimmt Patch 10 (`activateViewForNavigation` → `list.activateSavedViewWithDocuments`).
NgSelect: Enter geht nie mehr an das Formular (vorher speicherte Enter nach dem Filtern die Dokumentansicht).
Bildvergleich (`tests/visual/cases-p4b.mjs`, neue Aktionen `dragHold` + `keepMouse`, `upload` mit Funktion):
`dashboard-drag` 16/16 ≤ 0,008 %, `dashboard-tour(-docs)` 16/16 ≤ 0,003 %, `dashboard-consumed(-bell)` 16/16 ≤ 0,053 % (echte Verarbeitung), Rückfälle `dashboard-welcome/-views/-full/-notifications` 32/32 ≤ 0,025 % –
`results/e2e/light-p4b/2026-11-09/`. Ablauftests (`tests/flows/flows-detail.mjs`, Läufer `tests/flows/flows.mjs`):
Dokument bearbeiten (Titel/Korrespondent/Tag/Feld), Notizen anlegen/löschen, Hochladen bis „Document … was added“,
Dashboard umsortieren – je Angular und Light gleiche API-Wirkung, 8/8 ok. Aufruf (Server MIT Verarbeitung, PDFium):
`F0_ARGS="serve --worker" PAPERRUST_PDFIUM_PATH=<pdfium.dll> bash tests/server.sh reset && … start`, dann
`node tests/flows/flows.mjs --light <preview>/rs/app --angular http://127.0.0.1:<port> --ui both [--only <id>]`;
DB vor jedem Lauf zurücksetzen. Testdateien brauchen > 50 Zeichen Text (sonst OCR → Tesseract nötig).
`tools/budget.mjs --dist <ordner>` für parallele Builds. Budget erster Aufruf (dist-b, Arbeitsstand aller Agenten 21:50): JS 124,3 KB br ❌ (> 120), CSS 27,2 KB ✅ – Anteil Agent B daran nur Anker-Attribute + NgSelect-Fix (< 0,3 KB), Ziehen/Tour liegen in Lazy-Chunks; Mehrbedarf kommt aus neuen gemeinsamen Chunks (`index-client`, `lazy`, `ClearableBadge`, größeres `NgSelect`) → offen.
P4 Budget/Sprache: erster Aufruf wieder im Budget (de-DE 106,2 KB JS / 27,0 KB CSS br, vorher 124,3 KB): Sprachdatei zweistufig
– `tools/i18n-split.mjs` erzeugt beim Build `i18n/core/<locale>.json` (Texte von Rahmen, Dokumentliste, gemeinsamen Bausteinen,
lib/stores; 276 Texte, de 4,9 KB br), die volle Datei lädt im Hintergrund, nachgeladene Routen warten darauf (`fullLocale()`);
Massenbearbeitung und große Karten der Liste werden erst bei Bedarf geladen. Neue Texte außerhalb der CORE-Ordner sind
automatisch „lazy“; Texte aus Tabellen ohne `t('…')`-Literal mit `msg('…')` markieren, sonst fehlen sie im Kern.

**Verwaltung/Administration Teil 2** (zweiter P4-Agent): portiert und gegen Angular verglichen – Gespeicherte Ansichten
(`/savedviews`, inkl. `DragDropSelect` für Anzeigefelder, Symbolauswahl, Bestätigen-Knopf, Seitenleisten-Griffe zum Umsortieren),
Papierkorb, Protokolle, Aufgaben (Abschnitte, Filter, Details), Anwendungskonfiguration (alle Reiter, Datei-/Passwortfelder),
Benutzer & Gruppen (Dialoge mit Rechte-Raster `PermissionsSelect`), E-Mail (Konten, Regeln, verarbeitete Mails) und
Arbeitsabläufe (Liste + Dialog mit Auslösern, erweiterten Filtern, allen Aktionstypen, Ziehen zum Sortieren); Weiterleitungen
`settings/mail|usersgroups|savedviews`. Bildvergleich in `tests/visual/cases-manage.mjs` / `cases-admin.mjs` (Fälle können mit
`viewports: [...]` auf Desktop/Mobil beschränkt werden), Ablauftests in `tests/flows/flows-admin.mjs` (je Bildschirm speichern →
Stand per API prüfen, `--ui both`). Gemeinsame Korrekturen: `NgSelect` legt das Auswahlfeld wie ng-select 24 ins CDK-Overlay
(`lib/cdk-overlay.ts`, `.cdk-overlay-container` am `body`, oben/unten, markiert und scrollt zum gewählten Eintrag) – vorher war
das Panel unsichtbar (`opacity: 0`) und schob den Inhalt; `ModalHost` kennt `size` (`modal-lg`/`-xl`); `InputSelect.iconField`,
`InputSwitch.showUnsetNote`, `PermissionsDialog.note`, `CustomFieldsQueryDropdown.useDropdown`. Symbole nur einzelner Routen kommen
per `?raw` aus bootstrap-icons über `lib/route-icons.ts` (`ricon('…')`) in den Routen-Chunk statt in den ersten Aufruf
(geprüft von `tools/route-icons.test.mjs`).
Prüfmatrix ab jetzt (Entscheidung Eigentümer): neue Zustände (Dialoge der Dokumentansicht, Kalender, Dashboard-Umsortieren/
Tour, Freigabe-/E-Mail-/Neu-verarbeiten-Dialoge und künftige) werden nur noch in 1440×900, de-DE, hell + dunkel verglichen
(≤ 0,5 %; `compare.mjs --langs de-DE --viewports desktop`). Die volle Matrix (de/en × hell/dunkel × Desktop/Mobil) der
Bildschirme 1–4 bleibt als Regression. Ablauftests mit Speichern über API v1 und Prüfung des DB-Stands bleiben Pflicht.
(Die bis 2026-11-09 gebauten neuen Zustände sind bereits in der vollen Matrix grün.)
Prüfumfang Teil 2 (Entscheidung Eigentümer): für Gespeicherte Ansichten, E-Mail, Benutzer & Gruppen, Arbeitsabläufe, Aufgaben,
Protokolle, Papierkorb und Konfiguration ist der Pflicht-Bildvergleich auf **Desktop, de-DE, hell + dunkel** inkl. offener
Dialoge/Menüs reduziert (en-US und mobil entfallen); die Ablauftests (speichern über API v1, DB-Stand prüfen) bleiben vollständig.
Stand 2026-11-09 (`results/e2e/light-admin/2026-11-09/`): tatsächlich noch voll verglichen – 336/336 ≤ 0,5 % (max. 0,47 %
`mail` mobil de, sonst ≤ 0,16 %; Konfiguration nach Korrektur erneut im reduzierten Umfang 12/12); Ablauftests 13 Abläufe je
Angular und Light – alle ok außer `config-save`: `PATCH /api/config/{id}/` ist in PaperRust nicht nach Rust geroutet (404 ohne
Django), in Angular bleibt „Speichern“ dort deaktiviert → offener Backend-Punkt.
