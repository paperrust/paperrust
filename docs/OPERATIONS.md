# PaperRust – Betrieb

## Branding im Übergangsbetrieb (PaperRust vor Django)

PaperRust ist standardmäßig gebrandet (`PAPERRUST_BRANDING=on`, Akzent `PAPERRUST_ACCENT`, Standard
`#B7410E`; Details docs/BRANDING.md „Umsetzung ohne Fork“). Name und Logo laufen über die upstream-
Variablen `PAPERLESS_APP_TITLE` / `PAPERLESS_APP_LOGO`, und die liest **jeder Prozess selbst**:

| Prozess | liest | wirkt auf |
|---|---|---|
| PaperRust | eigene Umgebung; nicht gesetzt + Branding an → „PaperRust“ / `/__rs/brand/logo.svg` | Rust-Route `ui_settings.get` (Sidebar/Navbar, Fenstertitel im Angular-Frontend) |
| Django (granian) | eigene Umgebung (`settings.APP_TITLE/APP_LOGO`) | Login-/Konto-Seiten (`context_processors.settings` → `base.html`), `/api/ui_settings/` falls diese Route noch auf `django` steht |

Darum gilt: **Beide Prozesse brauchen dieselben `PAPERLESS_APP_*`-Werte.** Für das PaperRust-Branding
also in der gemeinsamen Umgebung (bzw. `paperless.conf`) setzen:

```sh
PAPERLESS_APP_TITLE=PaperRust
PAPERLESS_APP_LOGO=/__rs/brand/logo.svg
```

`/__rs/brand/logo.svg` liefert PaperRust selbst aus; Browser erreichen Django nur über PaperRust,
deshalb funktioniert der Pfad auch in Djangos Login-Seite (`<img src="{{APP_LOGO}}">`). Djangos
eigener `/logo/`-Endpunkt antwortet für diesen Wert 404 (Pfad liegt nicht unter `MEDIA_ROOT/logo/`) –
das Frontend nutzt `/logo/` dafür nicht. Hinter einem Unterpfad (`PAPERLESS_FORCE_SCRIPT_NAME`) den
Pfad entsprechend voranstellen.

Warum PaperRust die Werte Django nicht über den Proxy unterschiebt: Die einzigen Wege wären
(a) Djangos JSON von `/api/ui_settings/` umzuschreiben – API-Antworten verändert PaperRust nie –
oder (b) Logo/Titel im HTML der Login-Seite auszutauschen – das wäre Template-Nachbau per Textersatz
und bräche bei jeder upstream-Änderung. Eine Umgebungsvariable mehr für Django ist der saubere Weg.
Ohne sie zeigt die Login-Seite das paperless-ngx-Logo, aber schon die Rost-Akzentfarbe und das
PaperRust-Favicon (die kommen aus der Style-Injektion).

Branding abschalten: `PAPERRUST_BRANDING=off` (PaperRust wieder byte-transparent) und
`PAPERLESS_APP_*` aus der Umgebung beider Prozesse entfernen. Paritätsläufe immer so (docs/contract/testing.md).
