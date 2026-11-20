# Zuordnung der Doku-Links der Oberfläche (LINKMAP)

Die upstream-Oberfläche (`upstream/src-ui`, paperless-ngx v3.4.2) verlinkt auf
`https://docs.paperless-ngx.com/…`. PaperRust leitet diese Links auf die eigene Benutzer-Doku um
(Umsetzung damals: `frontend/doc-links.json` und `frontend/patches/02-doc-links.patch`; seit der
Ablösung von Angular am 2026-11-13 nutzt nur noch die eigene Oberfläche diese Zuordnung,
`frontend-light/src/lib/docs-url.ts`).

- **Basis-URL:** `rs/docs/` – die in `paperrust` eingebaute Doku (pl-server `/rs/docs/`, offline;
  der Server wählt `de/` oder `en/` nach der Oberflächen-Sprache). Online-Doku: `PAPERRUST_DOCS_URL`
  am Server (Umleitung) oder `PAPERRUST_DOCS_BASE_URL` beim Frontend-Build.
- **Sprachen:** Deutsch liegt direkt in `docs/user/`, Englisch in `docs/user/en/` mit **gleichen
  Dateinamen und gleichen Ankern**. Ein Sprachwechsel ist also nur ein Präfix: `…/docs/user/<seite>`
  bzw. `…/docs/user/en/<seite>`.
- **Anker:** überall explizit als `<a id="…"></a>` gesetzt (GitHub erzeugt aus deutschen
  Überschriften andere Slugs). Konfigurationsschlüssel gibt es in **beiden** Schreibweisen
  (`#PAPERRUST_OCR_LANGUAGE` und `#paperrust_ocr_language`); empfohlen ist die kleingeschriebene.
  Die alten Anker (`#PAPERLESS_OCR_LANGUAGE`, `#paperless_ocr_language`, wie in `doc-links.json`)
  bleiben bestehen.

## Fundstellen in der upstream-Oberfläche

Gesucht mit `grep -rn "docs.paperless-ngx.com\|infoLink" upstream/src-ui/src/app` (ohne Tests,
ohne Übersetzungsdateien).

| Fundstelle (`upstream/src-ui/src/app/…`) | upstream-Ziel |
|---|---|
| `components/app-frame/app-frame.component.html` (Zeile 77, 366: Logo-Bereich, Hilfe-Symbol) | `/` |
| `components/dashboard/widgets/welcome-widget/welcome-widget.component.html` | `/` |
| `components/admin/config/config.component.html` (`infoLink`) | `configuration` |
| `components/admin/config/config.component.ts::getDocsUrl` (je Einstellung) | `configuration/#<CONFIG_KEY>` |
| `components/admin/trash/trash.component.html` | `usage/#document-trash` |
| `components/admin/users-groups/users-groups.component.html` | `usage/#users-and-groups` |
| `components/common/permissions-select/permissions-select.component.html` | `usage/#global-permissions` |
| `components/manage/document-attributes/document-attributes.component.ts` (Tags, Korrespondenten, Dokumenttypen, Speicherpfade) | `usage/#terms-and-definitions` |
| `components/manage/document-attributes/document-attributes.component.ts` (Custom Fields) | `usage/#custom-fields` |
| `components/manage/mail/mail.component.html` | `usage/#incoming-mail` |
| `components/manage/mail/processed-mail-dialog/processed-mail-dialog.component.html` | `usage#processed-mail` |
| `components/manage/workflows/workflows.component.html` | `usage/#workflows` |
| `components/common/edit-dialog/workflow-edit-dialog/workflow-edit-dialog.component.html` (Hinweis „Titel zuweisen“) | `usage/#workflows` |
| `components/common/edit-dialog/storage-path-edit-dialog/storage-path-edit-dialog.component.html` (Hinweis „Pfad“) | `advanced_usage/#file-name-handling` |

Die Hinweistexte der beiden Bearbeitungsdialoge stehen auch in den Übersetzungen
(`src/locale/messages.*.xlf`); eine Übersetzung enthält den Tippfehler
`advanced_usage/#skedar-name-handling` – gleich behandeln wie `file-name-handling`.

## Zuordnung upstream-Anker → PaperRust-Seite

| upstream (`docs.paperless-ngx.com/…`) | PaperRust (relativ zur Basis-URL) | Stand `frontend/doc-links.json` |
|---|---|---|
| `/` (Startseite) | `README.md` | gleich |
| `configuration` | `configuration.md` | gleich |
| `configuration/#<CONFIG_KEY>` (45 Schlüssel aus `data/paperless-config.ts`) | `configuration.md#<config_key kleingeschrieben>` | gleich (`configuration#*`) |
| `usage` | `usage.md` | gleich |
| `usage/#document-trash` | `usage.md#papierkorb` (auch `#document-trash`) | gleich |
| `usage/#users-and-groups` | `permissions.md#benutzer-und-gruppen` (auch `#users-and-groups`) | `permissions.md` (Seitenanfang) – Anker kann ergänzt werden |
| `usage/#global-permissions` | `permissions.md#global-permissions` | `permissions.md` – Anker kann ergänzt werden |
| `usage/#terms-and-definitions` | `usage.md#begriffe` (auch `#terms-and-definitions`) | `usage.md` – Anker kann ergänzt werden |
| `usage/#custom-fields` | `custom-fields.md` (Anker `#custom-fields`) | gleich |
| `usage/#incoming-mail` | `mail.md` (Anker `#incoming-mail`) | gleich |
| `usage#processed-mail` | `mail.md#verarbeitete-mails` (auch `#processed-mail`) | `mail.md` – Anker kann ergänzt werden |
| `usage/#workflows` | `workflows.md` (Anker `#workflows`; Platzhalter: `#platzhalter`) | gleich |
| `advanced_usage` | `advanced.md` | gleich |
| `advanced_usage/#file-name-handling` | `storage-paths.md#dateinamen` (auch `#file-name-handling`) | `storage-paths.md` – Anker kann ergänzt werden |

Alle Zielseiten und Anker existieren in `docs/user/` und `docs/user/en/` (geprüft per Skript,
2026-11-09). Die 45 Konfigurationsschlüssel der Oberfläche
(`PAPERRUST_OCR_*`, `PAPERRUST_REMOTE_OCR_*`, `PAPERRUST_APP_*`, `PAPERRUST_CONSUMER_*BARCODE*`,
`PAPERRUST_AI_*`) haben je einen Eintrag mit Anker in beiden Konfigurationsseiten.

## Weitere Seiten (nicht von der Oberfläche verlinkt)

| upstream-Seite | PaperRust |
|---|---|
| `setup` | `installation.md` |
| `administration` | `administration.md` |
| `advanced_usage` (Matching, Barcodes, Hooks, SSO) | `advanced.md`; KI-Teil → `ai.md` |
| `api` | `api.md` |
| `faq` | `faq.md` |
| `troubleshooting` | `troubleshooting.md` |
| `migration-v3`, `development`, `changelog` | nicht übernommen (upstream-spezifisch); Entwickler-Doku: `docs/ARCHITECTURE.md`, `docs/UPSTREAM-SYNC.md` |
| – | neu: `getting-started.md`, `migration.md` (Umstieg), `security.md`, `permissions.md`, `mail.md`, `workflows.md`, `custom-fields.md`, `storage-paths.md` |

## Prüfen

```sh
python -I - <<'EOF'
import re, pathlib
base = pathlib.Path("docs/user")
need = {"usage.md": ["papierkorb", "document-trash", "begriffe", "terms-and-definitions"],
        "permissions.md": ["benutzer-und-gruppen", "users-and-groups", "global-permissions"],
        "mail.md": ["incoming-mail", "verarbeitete-mails", "processed-mail"],
        "workflows.md": ["workflows"], "custom-fields.md": ["custom-fields"],
        "storage-paths.md": ["dateinamen", "file-name-handling"]}
for d in (base, base / "en"):
    for page, ids in need.items():
        text = (d / page).read_text(encoding="utf-8")
        for i in ids:
            assert f'<a id="{i}"></a>' in text, (d / page, i)
print("ok")
EOF
```
