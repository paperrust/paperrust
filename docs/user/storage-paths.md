# Dateinamen und Speicherpfade

[English](en/storage-paths.md) · [Übersicht](README.md)

Diese Seite erklärt, wie PaperRust die Dateien im Medienordner benennt und wie Sie das selbst
festlegen: für alle Dokumente auf einmal oder mit Speicherpfaden für einzelne Gruppen von Dokumenten.

<a id="file-name-handling"></a><a id="dateinamen"></a>

## So benennt PaperRust Dateien

Ohne eigene Einstellung bekommt jede Datei die Nummer des Dokuments als Namen, siebenstellig mit
Nullen aufgefüllt: Dokument 42 liegt als `0000042.pdf` im Medienordner. Für die Arbeit mit PaperRust
reicht das völlig, denn Sie öffnen Dokumente über die Oberfläche, nicht über den Ordner.

Lesbare Namen und Ordner bekommen Sie auf zwei Wegen:

- **Für alle Dokumente:** die Einstellung
  [`PAPERRUST_FILENAME_FORMAT`](configuration.md#PAPERRUST_FILENAME_FORMAT).
- **Für bestimmte Dokumente:** einen [Speicherpfad](#storage-paths). Er hat Vorrang vor der
  Einstellung.

Beide sind kleine Vorlagen aus festem Text und [Platzhaltern](#placeholders) in doppelten
geschweiften Klammern. Ein `/` legt einen Ordner an. Die Dateiendung (`.pdf`, `.png` …) hängt
PaperRust selbst an, Sie schreiben sie nicht in die Vorlage.

Ein Beispiel:

```bash
PAPERRUST_FILENAME_FORMAT={{ created_year }}/{{ document_type }}/{{ title }}
```

Damit sieht der Medienordner etwa so aus:

```
2024/
  Rechnung/
    Stromabrechnung.pdf
    Werkstatt Ölwechsel.pdf
  Vertrag/
    Mietvertrag.pdf
2025/
  Rechnung/
    Stromabrechnung.pdf
    Stromabrechnung_01.pdf
```

Die durchsuchbare Archivfassung eines Dokuments bekommt denselben Namen mit `.pdf` und liegt im
Archivordner. Hat ein Dokument mehrere Dateiversionen, gilt für jede Version derselbe Name mit
einem Zusatz `_v1`, `_v2` usw.

### Was PaperRust dabei automatisch erledigt

- **Verbotene Zeichen** wie `/ \ : * ? " < > |` in Titeln oder Namen werden zu `-`.
- **Gleiche Namen:** Ergäbe sich für zwei Dokumente derselbe Name, bekommt das zweite `_01`, das
  nächste `_02` und so weiter. Bei Versionen steht der Zähler hinten: `vertrag_v2_01.pdf`.
- **Fehler in der Vorlage:** Lässt sich die Vorlage nicht auswerten, nimmt PaperRust den
  Standardnamen (`0000042.pdf`) und schreibt den Fehler ins Protokoll.
- **Zu lange Pfade:** Wird der Pfad zu lang (über 1024 Zeichen, oder länger als Ihr Dateisystem
  erlaubt), bleibt die Datei, wo sie ist. Der Grund steht im Protokoll. Vorsicht vor allem mit
  `{{ tag_list }}` bei vielen Tags.
- **Nur innerhalb des Medienordners:** Pfade, die aus dem Medienordner hinausführen (`..`, ein
  absoluter Pfad oder ein `\`), lässt PaperRust nicht zu und nimmt dann den Standardnamen.
- **Alte Schreibweise:** Vorlagen mit einfachen Klammern wie `{created_year}/{title}` versteht
  PaperRust weiterhin und wandelt sie selbst um.

### Wann Dateien umbenannt werden

PaperRust prüft den Dateinamen jedes Mal, wenn ein Dokument gespeichert wird, und verschiebt die
Datei bei Bedarf. Ändern oder löschen Sie einen Speicherpfad, werden alle betroffenen Dokumente
von selbst neu abgelegt.

Ändern Sie dagegen `PAPERRUST_FILENAME_FORMAT`, passiert mit vorhandenen Dokumenten erst etwas,
wenn Sie den [Umbenenner `document_renamer`](administration.md#renamer) einmal laufen lassen.

> [!WARNING]
> Verschieben oder benennen Sie Dateien im Medienordner nie von Hand. PaperRust merkt sich zu
> jedem Dokument den Dateinamen. Liegt die Datei woanders, findet PaperRust sie nicht mehr und
> meldet sie als fehlend.

<a id="filename-format-variables"></a><a id="placeholders"></a>

## Platzhalter

Diese Platzhalter können Sie in jeder Vorlage verwenden. Die Beispiele gehen von einem Dokument
aus, das am 7. März 2025 erstellt und am 12. März 2025 hinzugefügt wurde.

| Platzhalter | Inhalt | Beispiel |
|---|---|---|
| `{{ title }}` | Titel | `Stromabrechnung` |
| `{{ correspondent }}` | Name des Korrespondenten | `Stadtwerke` |
| `{{ document_type }}` | Name des Dokumenttyps | `Rechnung` |
| `{{ tag_list }}` | alle Tags, nach Name sortiert, mit Komma getrennt | `Haus,Strom` |
| `{{ asn }}` | Archivnummer (ASN) | `1207` |
| `{{ owner_username }}` | Benutzername des Eigentümers | `anna` |
| `{{ original_name }}` | Name der hochgeladenen Datei, ohne Endung | `scan_0815` |
| `{{ doc_pk }}` | Nummer des Dokuments, siebenstellig | `0000042` |
| `{{ created }}` | Erstellungsdatum | `2025-03-07` |
| `{{ created_year }}` | Jahr, vierstellig | `2025` |
| `{{ created_year_short }}` | Jahr, zweistellig | `25` |
| `{{ created_month }}` | Monat als Zahl | `03` |
| `{{ created_month_name }}` | Monatsname (englisch) | `March` |
| `{{ created_month_name_short }}` | Monatsname kurz (englisch) | `Mar` |
| `{{ created_day }}` | Tag | `07` |
| `{{ added }}` | Datum des Hinzufügens | `2025-03-12` |
| `{{ added_year }}` | Jahr des Hinzufügens | `2025` |
| `{{ added_year_short }}` | Jahr, zweistellig | `25` |
| `{{ added_month }}` | Monat als Zahl | `03` |
| `{{ added_month_name }}` | Monatsname (englisch) | `March` |
| `{{ added_month_name_short }}` | Monatsname kurz (englisch) | `Mar` |
| `{{ added_day }}` | Tag | `12` |

Hinweise:

- Fehlt ein Wert (kein Korrespondent, keine ASN, kein Eigentümer …), steht an seiner Stelle
  `none`. Wie Sie das vermeiden, steht im nächsten Abschnitt.
- Die Monatsnamen sind immer englisch. Deutsche Monatsnamen bekommen Sie mit dem Filter
  [`localize_date`](#date-localization).
- Das Datum des Hinzufügens richtet sich nach der Zeitzone aus `PAPERRUST_TIME_ZONE`.
- Für mehr Möglichkeiten gibt es [Vorlagen mit Bedingungen und Filtern](#filename-templates) und
  dort [weitere Variablen](#additional-variables).

<a id="empty-placeholders"></a>

### Leere Platzhalter weglassen

Nehmen wir die Vorlage `{{ correspondent }}/{{ title }}` und ein Dokument ohne Korrespondent.
Normalerweise landet es unter `none/Stromabrechnung.pdf`.

Schalten Sie
[`PAPERRUST_FILENAME_FORMAT_REMOVE_NONE`](configuration.md#PAPERRUST_FILENAME_FORMAT_REMOVE_NONE)
ein, fällt `none` weg: Das Dokument liegt dann direkt als `Stromabrechnung.pdf` im Medienordner.
Ein leerer Ordner entsteht nicht, und ein Leerzeichen direkt vor dem leeren Platzhalter
verschwindet mit.

<a id="storage-paths"></a>

## Speicherpfade

Ein Speicherpfad ist eine benannte Vorlage, die nur für die Dokumente gilt, denen er zugeordnet
ist. So können Sie verschiedene Arten von Dokumenten unterschiedlich ablegen.

Sie finden die Speicherpfade in der Seitenleiste unter **Attribute → Speicherpfade**. Ein
Speicherpfad hat:

- einen **Namen**, unter dem Sie ihn auswählen,
- einen **Pfad**, also die Vorlage (gleiche Regeln und Platzhalter wie oben),
- Einstellungen zur [automatischen Zuordnung](advanced.md#matching), genau wie bei Tags oder
  Korrespondenten.

Unter **Vorschau** wählen Sie ein vorhandenes Dokument aus und sehen sofort, welcher Dateiname
sich daraus ergäbe. Beim Speichern prüft PaperRust die Vorlage. Enthält sie einen unbekannten
Platzhalter oder einen Fehler, lässt sie sich nicht speichern.

Einem Dokument wird ein Speicherpfad beim Einlesen automatisch zugeordnet, wenn die Regeln
passen. Sie können ihn aber jederzeit am Dokument von Hand ändern. Dokumente ohne Speicherpfad
folgen `PAPERRUST_FILENAME_FORMAT` oder bekommen den Standardnamen.

Ein Beispiel mit zwei Speicherpfaden:

```
Steuer    = Steuer/{{ created_year }}/{{ correspondent }} {{ created }} {{ title }}
Haushalt  = Haushalt/{{ document_type }}/{{ title }}
```

Ergebnis:

```text
Steuer/
  2024/
    Finanzamt 2024-06-18 Bescheid.pdf
    Krankenkasse 2024-02-01 Beitragsbescheinigung.pdf
Haushalt/
  Rechnung/
    Kühlschrank.pdf
  Garantie/
    Kühlschrank.pdf
```

<a id="filename-templates"></a>

## Vorlagen mit Bedingungen und Filtern

Vorlagen können mehr als Platzhalter einsetzen. Die Schreibweise ist die von Jinja, einer
verbreiteten Vorlagensprache:

- `{{ … }}` gibt einen Wert aus.
- `{% if … %} … {% elif … %} … {% else %} … {% endif %}` wählt je nach Bedingung einen Teil aus.
- Ein `|` schickt einen Wert durch einen Filter, z. B. `{{ title | upper }}` für Großbuchstaben.
  Die üblichen Jinja-Filter wie `upper`, `lower`, `replace`, `truncate` oder `default` sind da,
  dazu die [eigenen Filter von PaperRust](#custom-jinja2-filters).

Eine Vorlage darf über mehrere Zeilen gehen. PaperRust macht daraus am Ende eine Zeile:
Zeilenumbrüche fallen weg, ebenso Leerzeichen direkt vor und nach einem `/`. Sie können
Vorlagen also übersichtlich einrücken.

Vorlagen laufen in einem geschützten Bereich. Sie können nur lesen, was PaperRust bereitstellt,
und nichts am Dokument oder an Dateien ändern. Sehr aufwendige Vorlagen bricht PaperRust ab; die
Datei bekommt dann den Standardnamen.

Neben den Platzhaltern gibt es das Objekt `document` mit diesen Feldern:

- `document.id` bzw. `document.pk`, `document.title`, `document.content` (der erkannte Text),
  `document.page_count`
- `document.created` (ein Datum), `document.added`, `document.modified` (Datum mit Uhrzeit)
- `document.archive_serial_number`, `document.mime_type` (z. B. `application/pdf`)
- `document.checksum`, `document.archive_checksum`
- `document.filename`, `document.archive_filename`, `document.original_filename`
- `document.correspondent`, `document.document_type`, `document.storage_path`: jeweils mit
  `.name` und `.id`, oder leer, wenn nichts zugeordnet ist
- `document.owner.username`, `document.tags` (Liste mit `.name` und `.id`)

<a id="additional-variables"></a>

### Weitere Variablen

- `{{ tag_name_list }}`: die Namen aller Tags als **Liste**, nicht als Text. Gut für Abfragen wie
  `{% if "Steuer" in tag_name_list %}`.
- `{{ custom_fields }}`: alle [benutzerdefinierten Felder](custom-fields.md) des Dokuments,
  nach Feldname. Jedes Feld hat `.type` und `.value`. Mit `{% if "Projekt" in custom_fields %}`
  prüfen Sie, ob ein Feld gesetzt ist; mit `{{ custom_fields.Projekt.value }}` lesen Sie es.
  Bei Namen mit Leerzeichen nehmen Sie besser den Filter [`get_cf_value`](#custom-field-access).

<a id="custom-jinja2-filters"></a>

### Eigene Filter von PaperRust

<a id="custom-field-access"></a>

#### `get_cf_value`: Wert eines benutzerdefinierten Felds

```jinja
{{ custom_fields | get_cf_value('Rechnungsnummer') }}
{{ custom_fields | get_cf_value('Rechnungsnummer', 'ohne-Nummer') }}
```

Links steht immer `custom_fields`, in den Klammern der Feldname und auf Wunsch ein Ersatzwert.
Der Ersatzwert gilt, wenn das Feld am Dokument fehlt oder leer ist. Gibt es weder Wert noch
Ersatzwert, steht `None` im Namen. Der Filter funktioniert auch mit Leerzeichen im Feldnamen.

<a id="datetime-formatting"></a>

#### `datetime`: Datum frei formatieren

```jinja
{{ created | datetime('%d.%m.%Y') }}          → 07.03.2025
{{ created | datetime('%Y/%m') }}             → 2025/03
```

Der Filter nimmt ein Datum oder einen Datumstext wie `2025-03-07` und formatiert ihn mit
`%`-Kürzeln. Die wichtigsten: `%Y` Jahr, `%y` Jahr zweistellig, `%m` Monat, `%d` Tag, `%H` Stunde,
`%M` Minute, `%j` Tag im Jahr. Uhrzeiten von `document.added` und `document.modified` sind in UTC. Namen (`%B` Monat, `%A` Wochentag) kommen auf Englisch. Ein Text,
der kein Datum ist, führt zu einem Fehler und damit zum Standardnamen.

<a id="date-localization"></a>

#### `localize_date`: Datum in einer Sprache ausgeben

```jinja
{{ document.created | localize_date('long', 'de_DE') }}       → 7. März 2025
{{ document.created | localize_date('MMMM yyyy', 'de_DE') }}  → März 2025
{{ document.created | localize_date('medium', 'en_US') }}     → Mar 7, 2025
```

Der erste Wert in den Klammern ist das Format, der zweite die Sprache mit Land (`de_DE`,
`de_AT`, `en_GB`, `fr_FR` …). Als Format geht eine der [Voreinstellungen](#format-presets) oder
ein eigenes Muster. In eigenen Mustern stehen Buchstaben für Teile des Datums: `d` Tag, `dd` Tag
zweistellig, `MM` Monat als Zahl, `MMM` kurzer Monatsname, `MMMM` voller Monatsname, `yy` und
`yyyy` Jahr, `EEEE` Wochentag.

Der Filter braucht ein echtes Datum. Nehmen Sie also `document.created` oder `document.added`
statt der Text-Platzhalter. Ein Datumstext im ISO-Format (`2025-03-07`) geht auch. Eine
unbekannte Sprache führt zu einem Fehler und damit zum Standardnamen.

<a id="format-presets"></a>

#### Voreinstellungen für `localize_date`

| Format | Deutsch (`de_DE`) | Englisch (`en_US`) |
|---|---|---|
| `short` | `07.03.25` | `3/7/25` |
| `medium` | `07.03.2025` | `Mar 7, 2025` |
| `long` | `7. März 2025` | `March 7, 2025` |
| `full` | `Freitag, 7. März 2025` | `Friday, March 7, 2025` |

#### `slugify`: Text für Dateinamen vereinfachen

```jinja
{{ correspondent | slugify }}   → aus „Müller & Söhne GmbH“ wird muller-sohne-gmbh
```

Macht alles klein, ersetzt Umlaute durch einfache Buchstaben, entfernt Sonderzeichen und setzt
Bindestriche statt Leerzeichen. Praktisch, wenn die Dateien auch auf Systemen mit strengen
Regeln für Dateinamen landen.

<a id="examples"></a>

### Beispiele

**Ordner nach Dokumenttyp, mit Sammelordner für den Rest.** Ein fehlender Dokumenttyp zählt als
„nicht gesetzt“:

```jinja
{% if document_type %}{{ document_type }}{% else %}Unsortiert{% endif %}/{{ created_year }}/{{ title }}
```

Eine Rechnung von 2025 landet unter `Rechnung/2025/Stromabrechnung.pdf`, ein Dokument ohne Typ
unter `Unsortiert/2025/…`.

**Steuerunterlagen anhand eines Tags heraussuchen:**

```jinja
{% if "Steuer" in tag_name_list %}
  Steuer/{{ created_year }}
{% else %}
  Ablage/{{ correspondent }}
{% endif %}
/{{ title }}
```

Mit dem Tag „Steuer“ ergibt das z. B. `Steuer/2024/Bescheid.pdf`, sonst
`Ablage/Stadtwerke/Stromabrechnung.pdf`.

**Nach Archivnummer, fünfstellig aufgefüllt:**

```jinja
{% if document.archive_serial_number %}
  ASN/{{ "%05d" | format(document.archive_serial_number) }} {{ title }}
{% else %}
  ohne-ASN/{{ title }}
{% endif %}
```

ASN 1207 ergibt `ASN/01207 Stromabrechnung.pdf`.

**Nach Dateiart:**

```jinja
{% if document.mime_type == "application/pdf" %}PDF{% else %}Bilder{% endif %}/{{ title }}
```

**Rechnungen nach Rechnungsdatum aus einem benutzerdefinierten Feld:**

```jinja
Rechnungen/
{{ custom_fields | get_cf_value('Rechnungsdatum', created) | datetime('%Y/%m') }}/
{{ correspondent }} {{ custom_fields | get_cf_value('Rechnungsnummer', 'ohne-Nummer') }}
```

Steht im Feld „Rechnungsdatum“ der 28. Februar 2025 und in „Rechnungsnummer“ `R-4711`, wird
daraus `Rechnungen/2025/02/Stadtwerke R-4711.pdf`. Fehlt das Rechnungsdatum, nimmt die Vorlage
das Erstellungsdatum.

**Monatsordner mit deutschen Namen:**

```jinja
{{ created_year }}/{{ created_month }} {{ document.created | localize_date('MMMM', 'de_DE') }}/{{ title }}
```

Ergibt `2025/03 März/Stromabrechnung.pdf`. Die Zahl vorne sorgt dafür, dass die Ordner richtig
sortiert sind.

> [!NOTE]
> Für Umsteiger von paperless-ngx: Vorhandene Vorlagen und Speicherpfade können Sie
> übernehmen, PaperRust wertet sie gleich aus. Sehr selten genutzte Jinja-Bausteine fehlen (die Filter `random`,
> `urlize`, `wordwrap`, `xmlattr`); eine Vorlage damit führt zum Standardnamen.
