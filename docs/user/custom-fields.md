# Benutzerdefinierte Felder

[English](en/custom-fields.md) · [Übersicht](README.md)

<a id="custom-fields"></a>

## Benutzerdefinierte Felder (Custom Fields)

Titel, Datum, Tags und Korrespondent reichen nicht immer. Mit benutzerdefinierten Feldern geben
Sie Dokumenten eigene Angaben mit, etwa eine Rechnungsnummer, einen Betrag oder ein Fälligkeitsdatum.

### So geht's

1. **Feld anlegen:** im Verwaltungsbereich unter **Benutzerdefinierte Felder** ein neues Feld
   anlegen. Es braucht einen **Namen** (z. B. „Rechnungsnummer“) und einen **Datentyp** (siehe unten).
2. **Am Dokument verwenden:** in der Dokumentansicht auf **Benutzerdefinierte Felder** klicken und
   das Feld wählen. Es erscheint im Formular, und Sie tragen den Wert ein. PaperRust prüft, ob der
   Wert zum Typ passt.
3. **Wieder entfernen:** mit der Maus auf den Feldnamen zeigen und **Entfernen** wählen.
4. **Speichern:** Neue oder entfernte Felder und geänderte Werte gelten erst nach **Speichern** –
   wie alle anderen Änderungen in der Dokumentansicht.

Ein Dokument kann viele Felder haben, jedes Feld aber nur einmal.

Weitere Wege:

- **Viele Dokumente auf einmal:** in der Dokumentliste Dokumente auswählen und dort Felder setzen
  oder entfernen.
- **Automatisch:** ein [Workflow](workflows.md#workflow-action-assignment) setzt oder entfernt Felder.
- **Suchen und filtern:** In der Dokumentliste können Sie nach Feldwerten filtern (z. B. „Betrag größer
  100“). Dieselbe Bedingung gibt es als Filter in Workflows.
- **In Dateinamen und Titeln:** Feldwerte lassen sich in [Speicherpfaden](storage-paths.md#additional-variables)
  und [Workflow-Platzhaltern](workflows.md#workflow-placeholders) verwenden.

> [!NOTE]
> Den Datentyp wählen Sie beim Anlegen. Danach lässt er sich nicht mehr ändern. Den Namen können
> Sie jederzeit ändern.

### Datentypen

| Typ in der Oberfläche | Technischer Name | Was hinein darf |
|---|---|---|
| **Text** | `string` | kurzer Text, eine Zeile |
| **Langer Text** | `longtext` | Text über mehrere Zeilen |
| **Ja/Nein** | `boolean` | Häkchen gesetzt oder nicht |
| **Datum** | `date` | ein Datum |
| **URL** | `url` | eine gültige Web-Adresse |
| **Ganze Zahl** | `integer` | Zahl ohne Komma, z. B. `12` |
| **Zahl** | `float` | Zahl mit Nachkommastellen, z. B. `12.3456` |
| **Betrag** | `monetary` | Geldbetrag (siehe unten) |
| **Dokumentlink** | `documentlink` | Verweise auf andere Dokumente (siehe unten) |
| **Auswählen** | `select` | ein Eintrag aus einer festen Liste (siehe unten) |

**Betrag:** eine Zahl mit höchstens zwei Nachkommastellen. Davor darf ein dreistelliger
[Währungscode nach ISO 4217](https://en.wikipedia.org/wiki/ISO_4217#List_of_ISO_4217_currency_codes)
stehen, z. B. `EUR49.90` oder `CHF-12.5` für einen negativen Betrag. Mit Code braucht der Betrag
einen Punkt und Nachkommastellen (`EUR49.00`, nicht `EUR49`). Ohne Code gilt die
**Standardwährung** des Feldes; ist keine eingetragen, die aus Ihren Spracheinstellungen.

**Dokumentlink:** verknüpft das Dokument mit einem oder mehreren anderen. In der Ansicht erscheinen
sie als Links. Die Verknüpfung gilt in beide Richtungen: Verweist A auf B, bekommt B automatisch
einen Verweis auf A.

**Auswählen:** Beim Anlegen tragen Sie mit **Auswahlwert hinzufügen** die erlaubten Einträge ein.
Am Dokument wählt man dann einen davon. Löschen Sie später einen Eintrag aus der Liste, verlieren
Dokumente, die genau diesen Eintrag hatten, ihren Wert in diesem Feld.
