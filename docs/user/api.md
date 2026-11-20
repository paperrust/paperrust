# REST-API

[English](en/api.md) · [Übersicht](README.md)

Alles, was die Oberfläche kann, geht auch über die REST-API: Dokumente suchen, hochladen, ändern,
Tags pflegen und so weiter. Die API spricht JSON über HTTP.

**Kompatibel zu paperless-ngx v3.4.2:** Pfade, Felder, Statuscodes und Fehlertexte sind dieselben.
Apps und Skripte, die mit paperless-ngx v3.4.2 laufen, laufen auch mit PaperRust weiter.

Eine Liste aller Endpunkte zum Ausprobieren im Browser gibt es unter `/api/schema/view/`
(nur auf Englisch). Das Schema selbst (OpenAPI) liegt unter `/api/schema/`.

Diese Seite erklärt die Teile, die man nicht sofort aus der Liste versteht.

> [!NOTE]
> Ein paar Randbereiche von paperless-ngx gibt es in PaperRust nicht und sie antworten mit 404:
> der Django-Admin (`/admin/`), die „headless“-Anmeldung von allauth (`/api/auth/headless/…`),
> Social-Login über andere Anbieter als OpenID Connect, `OPTIONS`-Anfragen mit Feldbeschreibungen
> und die Endungen `.json`/`.api` an Pfaden (z. B. `/api/tags.json`). Mehr dazu unter
> [Installation](installation.md#standalone-limits).

<a id="authorization"></a>

## Anmeldung

Jede Anfrage muss zeigen, wer sie stellt. Dafür gibt es vier Wege.

1.  **Benutzername und Passwort (Basic)**

    Schicken Sie den Header `Authorization: Basic …` mit. Dahinter steht `benutzername:passwort`,
    base64-kodiert. Mit curl geht das bequem über `-u`:

    ```bash
    curl -u anna:geheim https://paperrust.example/api/documents/
    ```

2.  **Browser-Sitzung**

    Wer im Browser bei PaperRust angemeldet ist, ist es auch für die API. Ein zusätzlicher Header
    ist nicht nötig. Praktisch für Tests im API-Browser.

3.  **API-Token** (empfohlen für Apps und Skripte)

    Ein Token ist ein langer Schlüssel, der statt des Passworts dient. So bekommen Sie eins:

    - In der Oberfläche: Benutzermenü → **Mein Profil** → Feld **API-Token**. Der Knopf mit dem
      runden Pfeil („Neues API-Token erstellen“) erzeugt ein neues; das alte gilt dann nicht mehr.
    - Über die API: `POST /api/token/` mit `username` und `password` (als JSON oder Formular). Die
      Antwort enthält das Token.

    ```bash
    curl -X POST -H "Content-Type: application/json" \
         -d '{"username": "anna", "password": "geheim"}' \
         https://paperrust.example/api/token/
    # {"token": "9f2c…"}
    ```

    Danach jede Anfrage mit diesem Header:

    ```
    Authorization: Token 9f2c…
    ```

4.  **Anmeldung durch einen vorgeschalteten Proxy (Remote-User)**

    Meldet ein Reverse Proxy den Benutzer schon an, kann PaperRust den Namen aus einem Header
    übernehmen. Das ist standardmäßig aus; einschalten mit
    [`PAPERRUST_ENABLE_HTTP_REMOTE_USER_API`](configuration.md#PAPERRUST_ENABLE_HTTP_REMOTE_USER_API).

Anmeldung über OpenID Connect ist in der Oberfläche möglich, siehe
[OpenID Connect](advanced.md#openid-connect-and-social-authentication). Die „headless“-API von
allauth, über die Fremd-Apps diesen Weg nutzen könnten, gibt es in PaperRust nicht.

<a id="searching-for-documents"></a>

## Dokumente suchen

Gesucht wird am normalen Listen-Endpunkt `/api/documents/`. Alle vier Parameter fragen den
Suchindex ab; welche Art Suche, bestimmt der Parameter:

| Parameter | Was er tut |
|---|---|
| `text=…` | sucht den Begriff in Titel und Inhalt (einfache Teilwort-Suche) |
| `title_search=…` | sucht nur im Titel |
| `query=…` | Volltextsuche mit Suchsprache (Wörter, `AND`/`OR`, Felder), siehe [Suche](usage.md#basic-usage_searching) |
| `more_like_id=…` | findet Dokumente, die dem Dokument mit dieser ID ähnlich sind |

Beispiel: alle Rechnungen von 2024, die „Strom“ erwähnen:

```
GET /api/documents/?query=strom%20created:2024&page_size=25
```

Seiten blättern Sie wie sonst auch mit `page` und `page_size`.

Bei einer solchen Suche hängt an jedem Treffer zusätzlich ein Feld `__search_hit__`:

```json
{
  "id": 42,
  "title": "Stromrechnung März",
  "__search_hit__": {
    "score": 7.81,
    "highlights": "… Abschlag für <span class=\"match\">Strom</span> im …",
    "rank": 0
  }
}
```

- `score`: wie gut der Treffer passt. Größer ist besser; sinnvoll nur im Vergleich mit den
  anderen Treffern derselben Suche.
- `highlights`: ein kurzer Ausschnitt aus dem Text, die Fundstellen stehen in
  `<span class="match">`.
- `rank`: Platz in der Ergebnisliste, gezählt ab 0.

<a id="filtering-by-custom-fields"></a>

### Nach Benutzerfeldern filtern

Mit `custom_field_query` filtern Sie nach den Werten von Benutzerfeldern (Custom Fields). Der Wert
ist ein kleiner JSON-Ausdruck: `[feld, operator, wert]`. Das Feld geben Sie per Name oder ID an.

Beispiele (vor dem Senden URL-kodieren):

| Gesucht | Ausdruck |
|---|---|
| Feld „Kundennummer“ ist genau „K-1001“ | `["Kundennummer", "exact", "K-1001"]` |
| Feld „Betrag“ größer als 100 | `["Betrag", "gt", 100]` |
| Feld „Fällig“ im Januar 2025 | `["Fällig", "range", ["2025-01-01", "2025-01-31"]]` |
| Feld „Bezahlt“ ist ja | `["Bezahlt", "exact", true]` |
| Auswahlfeld „Konto“ ist „Giro“ oder „Tagesgeld“ | `["Konto", "in", ["Giro", "Tagesgeld"]]` |
| Dokument hat das Feld „Vertrag“ gar nicht | `["Vertrag", "exists", false]` |
| Dokumentlink „Anlagen“ zeigt auf 12 und 15 | `["Anlagen", "contains", [12, 15]]` |
| „Notiz“ leer oder nicht gesetzt | `["OR", [["Notiz", "isnull", true], ["Notiz", "exact", ""]]]` |

Ausdrücke lassen sich mit `"AND"`, `"OR"` und `"NOT"` verbinden, wie in der letzten Zeile.

Welche Operatoren zu welchem Feldtyp passen:

| Feldtyp | Operatoren |
|---|---|
| alle | `exact`, `in`, `isnull`, `exists` |
| Text, URL, Geldbetrag | zusätzlich `icontains`, `istartswith`, `iendswith` (ohne Groß-/Kleinschreibung) |
| Ganzzahl, Zahl, Datum | zusätzlich `gt`, `gte`, `lt`, `lte`, `range` |
| Dokumentlink | zusätzlich `contains` (enthält alle genannten Dokumente) |

**Relative Datumsangaben:** Bei Datumsfeldern dürfen Sie statt eines festen Datums auch etwas wie
`"today"` oder `"in 30 days"` schreiben. Beispiel – fällig in den nächsten zwei Wochen:

```json
["Fällig", "range", ["today", "in 14 days"]]
```

Der Ausdruck wird bei jeder Anfrage neu ausgerechnet, in der Zeitzone aus `PAPERRUST_TIME_ZONE`.
PaperRust versteht einfache englische und deutsche Formen: `today`, `yesterday`, `tomorrow`, `now`,
`in N days`, `N weeks ago`, `heute`, `gestern`, `morgen`, `vorgestern`, `übermorgen`,
`vor N Tagen` (Einheiten von Sekunde bis Jahr). Deutsch gilt, wenn
`PAPERRUST_DATE_PARSER_LANGUAGES` leer ist oder `de` enthält. Andere Formen lehnt PaperRust mit
einer Fehlermeldung ab. In Workflow-Auslösern (`filter_custom_field_query`) gelten nur die
englischen Formen.

<a id="apisearchautocomplete"></a>

### `/api/search/autocomplete/`

Schlägt passende Wörter vor, während jemand tippt.

- `term`: der Wortanfang, z. B. `rech`.
- `limit`: wie viele Vorschläge höchstens (Standard 10).

Antwort ist eine einfache Liste. Vorne steht das Wort, das in den meisten Dokumenten vorkommt, die
der Benutzer sehen darf:

```json
["rechnung", "rechnungsnummer", "rechtsschutz"]
```

<a id="file-uploads"></a><a id="posting-documents"></a>

## Dokumente hochladen

Neue Dateien schicken Sie an `POST /api/documents/post_document/`, als Multipart-Formular. Die
Datei steht im Feld `document`, alles andere ist freiwillig:

| Feld | Inhalt |
|---|---|
| `title` | Titel; ohne Angabe nimmt PaperRust den Dateinamen |
| `created` | Datum, z. B. `2025-03-14` oder `2025-03-14 09:30:00+01:00` |
| `correspondent`, `document_type`, `storage_path` | jeweils die ID |
| `tags` | Tag-ID; für mehrere Tags das Feld mehrfach senden |
| `archive_serial_number` | Archivnummer (ASN) |
| `custom_fields` | Liste von Feld-IDs (Felder werden leer angelegt) oder ein Objekt `{"feld-id": wert}` |

```bash
curl -H "Authorization: Token 9f2c…" \
     -F document=@stromrechnung.pdf \
     -F title="Stromrechnung März" \
     -F tags=3 -F tags=8 \
     https://paperrust.example/api/documents/post_document/
# "b1d7c0e2-5a4f-4c1e-9a77-2f3e8d6c1a90"
```

Die Antwort kommt sofort (HTTP 200) und enthält nur die ID der Einlese-Aufgabe. Das eigentliche
Einlesen (Text erkennen, Tags vergeben usw.) läuft danach im Hintergrund. Den Stand fragen Sie mit
dieser ID ab:

```
GET /api/tasks/?task_id=b1d7c0e2-5a4f-4c1e-9a77-2f3e8d6c1a90
```

Ist die Aufgabe fertig, steht dort auch die ID des neuen Dokuments.

Große Dateien sind kein Problem: PaperRust schreibt den Upload direkt auf die Platte, statt ihn
ganz in den Arbeitsspeicher zu laden. Um den Dateityp zu erkennen, liest es nur die ersten 7 MiB.

<a id="document-versions"></a>

## Dokumentversionen

Ein Dokument kann mehrere Fassungen der Datei haben, z. B. das Original und eine geschwärzte
Kopie. Die erste Fassung ist das **Wurzeldokument**, weitere hängen als Versionen daran.

- **Gemeinsam für alle Versionen:** Titel, Tags, Korrespondent, Dokumenttyp, Speicherpfad,
  Benutzerfelder, Rechte.
- **Je Version eigen:** die Datei selbst, Dateityp, Prüfsummen, Archivdatei und erkannter Text.

Ohne weitere Angabe gilt immer die neueste Version. Eine bestimmte Version wählen Sie mit
`?version={version_id}`.

| Anfrage | Wirkung |
|---|---|
| `GET /api/documents/{id}/` | Daten des Dokuments; `content` ist der Text der neuesten (oder gewählten) Version |
| `PATCH /api/documents/{id}/` | Änderungen am Text gehen an die neueste (oder gewählte) Version, alles andere an das Wurzeldokument |
| `GET …/download/`, `…/preview/`, `…/thumb/`, `…/metadata/` | nehmen `?version=` an |
| `POST /api/documents/{id}/update_version/` | neue Version hochladen: Feld `document`, optional `version_label` |
| `POST /api/documents/merge_as_versions/` | vorhandene Dokumente zu Versionen eines Dokuments zusammenlegen: `documents` (mindestens zwei IDs), `root_document_id` (eine davon); `version_label` nur bei genau einem Quelldokument |
| `PATCH /api/documents/{id}/versions/{version_id}/` | Bezeichnung (`version_label`) einer Version ändern |
| `DELETE /api/documents/{id}/versions/{version_id}/` | eine Version löschen (nicht die Wurzel) |

<a id="permissions"></a>

## Rechte

Bei Dokumenten, Tags, Korrespondenten und allen anderen Objekten können Sie beim Anlegen oder
Ändern Eigentümer und Rechte mitschicken. Benutzer und Gruppen stehen als IDs drin:

```json
{
  "owner": 4,
  "set_permissions": {
    "view":   { "users": [7, 9], "groups": [2] },
    "change": { "users": [7],    "groups": [] }
  }
}
```

`view` heißt sehen, `change` heißt ändern. Wer `set_permissions` mitschickt, **ersetzt** die
bisherigen Rechte des Objekts. Das darf nur der Eigentümer oder ein Superuser.

<a id="retrieving-full-permissions"></a>

### Vollständige Rechte abrufen

Normalerweise liefert die API nur ein Kurzfeld: `user_can_change` sagt, ob der angemeldete
Benutzer das Objekt ändern darf. Hängen Sie `full_perms=true` an, kommt stattdessen das ganze
Feld `permissions` im selben Aufbau wie `set_permissions` oben.

<a id="bulk-editing"></a>

## Massenbearbeitung

Viele Objekte auf einmal ändern geht über zwei Endpunkte. Die Arbeit läuft im Hintergrund.

<a id="documents"></a>

### Dokumente

`POST /api/documents/bulk_edit/` mit einer Liste von Dokumenten, einer Methode und ihren
Parametern. Beispiel – Tag 5 dazu, Tag 2 weg, bei drei Dokumenten:

```json
{
  "documents": [101, 102, 117],
  "method": "modify_tags",
  "parameters": { "add_tags": [5], "remove_tags": [2] }
}
```

| `method` | `parameters` |
|---|---|
| `set_correspondent` | `{"correspondent": id}` |
| `set_document_type` | `{"document_type": id}` |
| `set_storage_path` | `{"storage_path": id}` |
| `add_tag` / `remove_tag` | `{"tag": id}` |
| `modify_tags` | `{"add_tags": [ids], "remove_tags": [ids]}` |
| `modify_custom_fields` | `{"add_custom_fields": {feld-id: wert}` (oder Liste von IDs, dann leer), `"remove_custom_fields": [ids]}` |
| `set_permissions` | `{"set_permissions": {…}, "owner": id oder null, "merge": false}` – Format wie unter [Rechte](#permissions). `merge: true` ergänzt die bestehenden Rechte, `false` (Standard) ersetzt sie. |
| `delete` | keine |
| `reprocess` | keine; optional `{"remote_ocr": true}` für [Remote-OCR](usage.md#remote-ocr) |

<a id="document-editing-operations"></a>

#### Dateien bearbeiten (Drehen, Zusammenfügen usw.)

Seit API-Version 10 haben diese Aktionen eigene Endpunkte, z. B. `/api/documents/rotate/`,
`/api/documents/merge/`, `/api/documents/edit_pdf/`, `/api/documents/remove_password/` und
`/api/documents/delete/`. Die Felder stehen im API-Browser. Über `bulk_edit` funktionieren sie noch,
aber nur aus Rücksicht auf alte Apps; neue Apps sollten die eigenen Endpunkte nehmen.

<a id="objects"></a>

### Objekte

`POST /api/bulk_edit_objects/` ändert Rechte oder löscht – für Tags, Korrespondenten, Dokumenttypen
oder Speicherpfade:

```json
{
  "objects": [3, 4, 9],
  "object_type": "tags",
  "operation": "set_permissions",
  "owner": 4,
  "permissions": { "view": { "users": [7], "groups": [] }, "change": { "users": [], "groups": [] } },
  "merge": false
}
```

- `object_type`: `tags`, `correspondents`, `document_types` oder `storage_paths`.
- `operation`: `set_permissions` oder `delete`.
- `owner`, `permissions` und `merge` braucht nur `set_permissions`; `merge` wirkt wie oben.
- Statt einer langen ID-Liste geht auch `"all": true` mit optionalen `"filters"` (API-Version 10).

<a id="api-versioning"></a>

## API-Versionen

Damit alte Apps nicht kaputtgehen, wenn sich die API weiterentwickelt, hat sie Versionsnummern.
Die App sagt bei jeder Anfrage, welche Version sie spricht, und bekommt Antworten genau in diesem
Format.

- PaperRust kennt die Versionen **9** und **10**.
- Ohne Angabe gilt **10**.
- Die Version steht im `Accept`-Header:

  ```
  Accept: application/json; version=9
  ```

- Eine unbekannte Version beantwortet PaperRust mit `406 Not Acceptable` und einer Erklärung.

**Passt meine App zum Server?** Schicken Sie irgendeine angemeldete Anfrage. In der Antwort stehen
zwei Header:

```
X-Api-Version: 10
X-Version: 3.4.2
```

`X-Api-Version` ist die neueste API-Version des Servers. `X-Version` meldet **3.4.2**, also die
paperless-ngx-Version, zu der PaperRust kompatibel ist – nicht die eigene Versionsnummer von
PaperRust. So erkennen Apps, die nach paperless-ngx-Versionen fragen, den Server richtig.
Antworten ohne Anmeldung (z. B. Freigabelinks unter `/share/…`) haben diese Header nicht.

<a id="api-version-deprecation-policy"></a>

### Wie lange alte Versionen bleiben

Kommt eine neue API-Version, bleibt die vorige noch mindestens ein Jahr nutzbar. Danach darf sie
wegfallen.

<a id="api-changelog"></a>

### Änderungen der API

Kurzüberblick, was sich mit welcher Version geändert hat:

| Version | Was ist neu |
|---|---|
| <a id="version-1"></a>1 | Ausgangsstand. |
| <a id="version-2"></a>2 | Tags haben `color` (Hex-Farbe, z. B. `#4caf50`) und das nur lesbare `text_color` (Schwarz oder Weiß, je nach Hintergrund). Das alte Feld `colour` ist weg. |
| <a id="version-3"></a>3 | Endpunkte für Rechte; `/api/ui_settings/` hat einen neuen Aufbau. |
| <a id="version-4"></a>4 | Aus den Einlese-Vorlagen wurden Workflows, mit eigenen Endpunkten. |
| <a id="version-5"></a>5 | Dokumente und Objekte lassen sich gesammelt löschen. |
| <a id="version-6"></a>6 | Aufgaben bestätigen jetzt über `/api/tasks/acknowledge/`. |
| <a id="version-7"></a>7 | Auswahlfelder liefern ihre Optionen als Objekte mit `id` und `label`. Gesetzt wird ein Wert über die `id` der Option, nicht mehr über ihre Position. |
| <a id="version-8"></a>8 | Bei Notizen steht im Feld `user` ein kleines Benutzerobjekt statt nur der ID. |
| <a id="version-9"></a>9 | `created` am Dokument ist ein reines Datum ohne Uhrzeit. `created_date` ist veraltet. |
| <a id="version-10"></a>10 | Siehe Liste unten. |

**Version 10 im Einzelnen** (Version 9 behält jeweils das alte Verhalten, solange sie unterstützt
wird):

- Gespeicherte Ansichten haben kein `show_on_dashboard`/`show_in_sidebar` mehr; das steht jetzt in
  den Oberflächen-Einstellungen (`/api/ui_settings/`).
- Drehen, Zusammenfügen, PDF bearbeiten usw. haben eigene Endpunkte (siehe
  [oben](#document-editing-operations)).
- `bulk_edit_objects` nimmt `all` und `filters` an.
- Der Listen-Parameter `all` ist veraltet.
- Der Suchparameter `title_content` ist veraltet; stattdessen `text` (Titel und Inhalt) oder
  `title_search` (nur Titel).
- Aufgaben: `/api/tasks/` ist seitenweise. Felder heißen `task_type` (statt `task_name`) und
  `trigger_source` (statt `type`). Neu sind `/api/tasks/summary/`, `/api/tasks/status_counts/`,
  `/api/tasks/active/` (nur lesen) und `POST /api/tasks/run/` zum Starten erlaubter Aufgaben.

<a id="paperrust-extensions"></a>

## PaperRust-Erweiterungen

Zusätzlich zur kompatiblen API bringt PaperRust eigene Endpunkte mit. Die kompatiblen Endpunkte
ändern sie nicht; Apps für paperless-ngx merken davon nichts.

| Pfad | Wofür |
|---|---|
| `GET /__rs/health` | Lebenszeichen: 200, solange der Server läuft (für Docker, Monitoring) |
| `/__rs/brand/…` | Logo und Favicon (eigenes Branding) |
| `/api/x-ai/…` | KI: Prüfliste (`/api/x-ai/review/`), Lauf von Hand starten (`POST /api/x-ai/run/`), MCP-Freigaben – siehe [KI](ai.md) |
| `/mcp` | eingebauter MCP-Server für KI-Assistenten (Anmeldung per API-Token); nur mit `PAPERRUST_AI_MCP_ENABLED=yes` |
| `/api/x-scan/…` | Scanner, Scan-Aufträge und Scan-Geräte – siehe [Scanner](scanner.md) |
| `/api/documents/{id}/redact/…`, `/api/documents/{id}/shrink/` | PDF-Werkzeuge, siehe unten |
| `/rs/api/v2/…` | neue PaperRust-API (u. a. [ASN-Etiketten](asn-labels.md)); abschaltbar mit `PAPERRUST_API_V2=off` |

<a id="pdf-werkzeuge"></a>

### PDF-Werkzeuge

Schwärzen und Verkleinern, wie unter [PDF-Werkzeuge](pdf-tools.md) beschrieben.

- **Recht:** „Dokument ändern“, sowohl allgemein als auch an diesem Dokument.
- **Quelle:** immer die neueste Version des Dokuments, und sie muss ein PDF sein.
- **Ergebnis:** eine neue Version desselben Dokuments. Sie wird wie ein Upload im Hintergrund
  eingelesen; die Antwort enthält die `task_id`.
- **Seiten** zählen ab 1. **Rechtecke** sind `[x0, y0, x1, y1]` in PDF-Punkten (1/72 Zoll), Ursprung
  unten links – dieselben Werte, die pdf.js mit `convertToPdfPoint` liefert.

| Anfrage | Body (Beispiel) | Antwort |
|---|---|---|
| `POST /api/documents/{id}/redact/search/` – Fundstellen suchen, nichts ändern | `{"terms": ["Erika Muster", "DE89 3704"]}` | `document`, `version`, `source` (`original` oder `archive`), `truncated`, `hits`: Liste mit `page`, `term`, `context`, `rects` |
| `POST /api/documents/{id}/redact/` – schwärzen | `{"areas": [{"page": 2, "rect": [72, 600, 300, 620]}], "terms": ["Erika Muster"], "remove_metadata": true, "version_label": "Geschwärzt"}` | `task_id`, `document`, `source_version`, `report` (gezählt: `marks`, `pages`, `glyphs`, `images_removed`, `images_cleared`, `paths_removed`, `paths_clipped`, `annotations_removed`, `metadata_removed`, `term_hits`) |
| `POST /api/documents/{id}/shrink/` – verkleinern | `{"preset": "standard", "version_label": "Klein"}` | `task_id`, `document`, `new_version` (`true`/`false`), `report` (`preset`, `size_before`, `size_after`, `images`, `images_resampled`, `images_recompressed`, `unused_objects_removed`, `streams_compressed`) |
| `POST /api/documents/{id}/convert_pdf/` | `{"ocr": false, "version_label": "PDF"}` – Bild oder Office-Datei als neue PDF-Version ([In PDF umwandeln](pdf-tools.md#in-pdf-umwandeln)); `ocr` nur für Bilder | `{"task_id", "document", "source_version", "kind": "image"\|"office", "ocr"}`; 400 `This document is already a PDF.` / `This document cannot be converted to PDF.`, 503 ohne OCR-Engine bzw. Office-Umwandlung aus |

Ohne `version_label` heißt die neue Version `Redacted` bzw. `Shrunk`. Stufen beim Verkleinern:
`lossless`, `standard` (Standard), `strong`. Wird die Datei nicht kleiner, entsteht keine neue
Version; dann ist `task_id` `null` und `new_version` `false`.

**Fehler:**

| Code | Bedeutung |
|---|---|
| 400 | Eingabe falsch (Feldfehler) oder `{"detail": "…"}`, z. B. Seite gibt es nicht, nichts zu schwärzen, Schwärzung ließ sich nicht nachweisen, kein PDF |
| 403 | Recht fehlt |
| 404 | Dokument unbekannt |
| 422 | Werkzeug abgebrochen (Datei kaputt oder zu groß) |
| 503 | Werkzeug `paperrust-pdftool` nicht installiert |
| 504 | Zeitgrenze überschritten |
