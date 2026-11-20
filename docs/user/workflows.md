# Workflows

[English](en/workflows.md) · [Übersicht](README.md)

<a id="workflows"></a>

## Workflows

Ein Workflow ist eine Regel nach dem Muster **„Wenn das passiert, dann tu das“**. Damit erledigt
PaperRust Handgriffe von selbst: Tags setzen, einen Eigentümer eintragen, eine Mail verschicken,
ein Dokument in den Papierkorb legen und vieles mehr.

Jeder Workflow besteht aus zwei Teilen:

- **Auslöser** – *wann* der Workflow startet, und für *welche* Dokumente (Filter).
- **Aktionen** – *was* dann mit dem Dokument geschieht, Schritt für Schritt.

Ein Workflow kann mehrere Auslöser und mehrere Aktionen haben. Im PaperRust-Design verwalten Sie
Workflows im Bereich **Automatik** – mit Satz-Ansicht, Vorlagen, Testlauf, Verlauf und Sonst-Zweig;
das erklärt die Seite [Automatik](automatik.md). In der klassischen Oberfläche stehen sie unter
*Verwalten → Workflows*. Jeder Workflow hat einen Namen, eine **Reihenfolge** und einen Schalter **An**.

Diese Seite ist das Nachschlagewerk für alle Auslöser, Filter, Aktionen und Platzhalter. Sie nennt
die Begriffe der klassischen Oberfläche; die Namen im PaperRust-Design stehen in der Tabelle unter
[Automatik → Auslöser](automatik.md#triggers).

**Reihenfolge:** Passen mehrere Workflows auf ein Dokument, laufen sie nach ihrer Reihenfolge-Zahl
nacheinander, und innerhalb eines Workflows die Aktionen von oben nach unten. Setzen zwei Aktionen
denselben Einzelwert (etwa den Korrespondenten), gewinnt die spätere. Listen wie Tags, benutzerdefinierte
Felder und Rechte werden dagegen gesammelt, nicht ersetzt.

<a id="workflow-triggers"></a>

### Auslöser

<a id="workflow-trigger-types"></a><a id="types"></a>

#### Arten

Es gibt fünf Arten von Auslösern. Die Namen stehen so im Workflow-Dialog:

| Auslöser | Wann er startet | Was man schon weiß |
|---|---|---|
| **Import gestartet** | Eine neue Datei kommt herein, *bevor* sie gelesen wird. | Quelle, Pfad, Dateiname, Mail-Regel. Noch kein Text, keine Tags. |
| **Dokument hinzugefügt** | Die neue Datei ist fertig verarbeitet. | Text, Tags, Typ, Korrespondent usw. Quelle und Pfad nicht mehr. |
| **Dokument geändert** | Jemand speichert Änderungen an einem Dokument. | Alles über das Dokument. |
| **Zeitgesteuert** | Zu einem Zeitpunkt, der sich aus einem Datum des Dokuments ergibt. | Alles über das Dokument. |
| **Manuell** | Nur, wenn Sie den Workflow selbst starten. | Alles über das Dokument. |

**Zeitgesteuert** genauer: Sie wählen, ab welchem Datum gerechnet wird (**Hinzugefügt**,
**Erstellt**, **Geändert** oder ein **Benutzerdefiniertes Feld** vom Typ Datum), und eine
**Verschiebung in Tagen**. Eine positive Zahl heißt „so viele Tage danach“, eine negative „so viele
Tage davor“. Beispiel: Feld „Kündigungsfrist“, Verschiebung `-14` – der Workflow läuft zwei Wochen vor
der Frist. Normalerweise läuft ein zeitgesteuerter Workflow für jedes Dokument nur einmal. Mit
**Wiederholend** läuft er erneut, sobald die eingestellte Zahl an Tagen seit dem letzten Lauf um ist.
PaperRust prüft zeitgesteuerte Workflows einmal pro Stunde (fünf Minuten nach der vollen Stunde;
änderbar mit `PAPERRUST_WORKFLOW_SCHEDULED_TASK_CRON`).

**Manuell** läuft nie von selbst. Wie Sie ihn starten, steht unter
[Workflow von Hand ausführen](#running-a-workflow-manually).

<a id="workflow-trigger-filters"></a><a id="filters"></a>

#### Filter

Ohne Filter gilt ein Auslöser für jedes Dokument. Mit Filtern grenzen Sie ein. Ein Dokument muss
**alle** gesetzten Filter erfüllen.

**Filter beim Hereinkommen** (nur bei **Import gestartet**):

- **Quellen** – woher das Dokument kommt: **Import-Ordner**, **API-Upload**, **E-Mail-Import**,
  **Weboberfläche**. Gut zu wissen: Ein Scan über den Scannen-Knopf zählt als Weboberfläche, ein
  Scan per Scan-to-Email als API-Upload (siehe [Scanner](scanner.md)).
- **Pfad** – Pfad der Datei, `*` steht für beliebige Zeichen. Zusammen mit
  `PAPERRUST_CONSUMER_RECURSIVE` (Unterordner im Import-Ordner) können Sie so z. B. jedem Unterordner
  einen anderen Eigentümer geben: Pfad `*/anna/*` → Eigentümerin Anna.
- **E-Mail-Regel** – nur Dokumente, die diese Mail-Regel geholt hat. Damit passen automatisch nur
  noch Dokumente aus dem E-Mail-Import.

**Dateiname** (alle Auslöser): z. B. `*.pdf` oder `*rechnung*`. Groß- und Kleinschreibung spielt
keine Rolle. Bei bestehenden Dokumenten wird der ursprüngliche Dateiname geprüft.

**Filter auf das Dokument** (bei **Dokument hinzugefügt**, **Dokument geändert**, **Zeitgesteuert**
und **Manuell**):

- **Inhalt** – ein Suchmuster für den Text des Dokuments, mit denselben Zuordnungsverfahren wie bei
  Tags und Korrespondenten (z. B. **Ein beliebiges Wort**, **Genauer Text**, **Regulärer Ausdruck**).
- **Weitere Filter** (über **Filter hinzufügen**):
  - Tags: **Hat einen dieser Tags**, **Hat alle diese Tags**, **Hat keinen dieser Tags**
  - Korrespondent: **Hat Korrespondent**, **Hat einen dieser Korrespondenten**, **Hat keinen dieser
    Korrespondenten**
  - Dokumenttyp und Speicherpfad: jeweils dieselben drei Möglichkeiten
  - **Erfüllt Bedingung für benutzerdefinierte Felder** – eine Feld-Bedingung, aufgebaut wie im
    Filter der Dokumentliste (z. B. „Betrag größer 100“).

Bei relativen Datumsangaben in Feld-Bedingungen versteht PaperRust nur einfache englische Formen:
`today`, `yesterday`, `tomorrow`, `now`, `3 days ago`, `in 30 days` und Ähnliches.

<a id="running-a-workflow-manually"></a>

#### Workflow von Hand ausführen

Workflows mit dem Auslöser **Manuell** starten Sie selbst:

- **Mehrere Dokumente:** in der Dokumentliste Dokumente auswählen, dann im Menü **Aktionen** auf
  **Workflow ausführen** (im PaperRust-Design: Knopf **Workflow ausführen** in der Leiste über der
  Liste).
- **Ein Dokument:** in der Dokumentansicht im Menü **Aktionen** auf **Workflow ausführen** (im
  PaperRust-Design: Blitz-Symbol oben).

Dann wählen Sie den Workflow aus. Zur Wahl stehen nur eingeschaltete Workflows mit Auslöser
„Manuell“. Die Filter des Auslösers gelten weiter: Dokumente, die nicht passen, werden ausgelassen.
Möchten Sie das nicht, setzen Sie das Häkchen **Auf jedes ausgewählte Dokument anwenden**.

Der Workflow läuft im Hintergrund. Die Dokumentansicht zeigt das Ergebnis von selbst, eine Liste
müssen Sie eventuell neu laden.

**Wer darf das?** Sie brauchen das Recht, Workflows anzusehen. Außerdem müssen Sie alle gewählten
Dokumente ändern dürfen und ihr **Eigentümer** sein (oder sie haben keinen Eigentümer). Der Grund:
Ein Workflow kann Eigentümer und Rechte ändern oder Dokumente löschen. Superuser dürfen immer.

> [!NOTE]
> Für Umsteiger: paperless-ngx 3.4.2 kennt den Auslöser „Manuell“ noch nicht. Die Datenbank bleibt
> trotzdem verträglich. Gehen Sie zurück zu paperless-ngx, bleiben solche Workflows gespeichert,
> laufen dort aber nie.

<a id="workflow-actions"></a>

### Aktionen

<a id="workflow-action-types"></a>

#### Arten

Eine Aktion ist ein Schritt, den der Workflow ausführt. Diese Arten gibt es:

<a id="workflow-action-assignment"></a><a id="assignment"></a>

##### Werte setzen

Trägt Werte am Dokument ein:

- **Titel setzen** – mit [Platzhaltern](#workflow-placeholders), z. B. `{{ correspondent }} {{ added_year }}`
- **Tags hinzufügen**, **Korrespondent**, **Dokumenttyp**, **Speicherpfad setzen**
- **Eigentümer setzen**
- **Leserechte vergeben** und **Bearbeitungsrechte vergeben** – an Benutzer und/oder Gruppen
- **Benutzerdefinierte Felder setzen** – mit oder ohne Wert. Ohne Wert hängt der Workflow nur das
  leere Feld an; hat das Dokument das Feld schon, bleibt sein Wert, wie er ist. Mit Wert wird ein
  vorhandener Wert ersetzt.

<a id="workflow-action-removal"></a><a id="removal"></a>

##### Werte entfernen

Nimmt Werte vom Dokument weg – entweder bestimmte oder mit **Alle entfernen** alle:

- Tags, Korrespondenten, Dokumenttypen, Speicherpfade
- Eigentümer
- Lese- und Bearbeitungsrechte
- Benutzerdefinierte Felder

<a id="workflow-action-email"></a><a id="email"></a>

##### E-Mail

Verschickt eine Mail. Dafür muss der [Mailversand eingerichtet](configuration.md#email-sending) sein.
Sie geben an:

- **E-Mail-Empfänger** – eine oder mehrere Adressen, durch Komma getrennt
- **E-Mail-Betreff** und **E-Mail-Text** – mit [Platzhaltern](#workflow-placeholders)
- **Dokument anhängen** – ob die Datei mitgeschickt wird

PaperRust stellt die Mail im Moment der Aktion zusammen; das eigentliche Senden läuft danach im
Hintergrund weiter. Die Verarbeitung des Dokuments wartet also nicht auf den Mailserver.

<a id="workflow-action-webhook"></a><a id="webhook"></a>

##### Webhook

Schickt eine Nachricht (HTTP-POST) an eine Adresse, z. B. an ein Hausautomations- oder Chat-System.
Sie geben an:

- **Webhook-URL** – nur `http://` oder `https://`
- den Inhalt: entweder **Werte als Formularfelder senden** (dann füllen Sie **Webhook-Felder** als
  Name/Wert-Paare) oder freien Text im **Webhook-Inhalt**. Beides darf
  [Platzhalter](#workflow-placeholders) enthalten.
- **Nutzdaten als JSON senden** – sonst als Formulardaten
- **Webhook-Header** – zusätzliche Kopfzeilen als Name/Wert-Paare
- **Dokument anhängen** – ob die Datei mitgeschickt wird

Webhooks können missbraucht werden, um Rechner im eigenen Netz anzusprechen. Mit den
[Webhook-Einstellungen](configuration.md#workflow-webhooks) legen Sie fest, welche Ports und Ziele
erlaubt sind. Dürfen bei Ihnen auch normale Benutzer Workflows anlegen, sollten Sie diese Einstellungen
eng halten.

<a id="workflow-action-move-to-trash"></a><a id="move-to-trash"></a>

##### In den Papierkorb

Legt das Dokument in den Papierkorb. Dort bleibt es, bis der Papierkorb geleert wird (von Hand oder
nach der eingestellten Frist) – so lange lässt es sich wiederherstellen.

- Diese Aktion kommt immer **zuletzt**, egal wo sie in der Liste steht.
- Danach laufen keine weiteren Workflows mehr für dieses Dokument.
- Beim Auslöser **Import gestartet** gibt es noch kein Dokument: Der Import wird abgebrochen und die
  Datei gelöscht.

<a id="workflow-action-password-removal"></a><a id="password-removal"></a>

##### Passwort entfernen

Gedacht für PDFs mit Passwortschutz: Sie hinterlegen ein oder mehrere **Passwörter** (eines pro
Zeile), der Workflow probiert sie der Reihe nach. Die Passwörter werden im Klartext gespeichert.

So läuft es ab: Die Originaldatei bleibt unverändert. Die entsperrte Fassung kommt als neue
[Version](usage.md#document-file-versions) zum Dokument, die verschlüsselte bleibt im Verlauf. Beim
Auslöser **Import gestartet** wird die Datei zuerst normal (verschlüsselt, meist ohne lesbaren Text)
abgelegt und gleich danach entsperrt als zweite Version eingelesen.

Passt keines der Passwörter, bleibt das Dokument wie es ist; die Meldung steht im Log.
Für verschiedene PDF-Absender mit verschiedenen Passwörtern legen Sie am besten getrennte Workflows an.

<a id="workflow-action-remote-ocr"></a><a id="remote-ocr"></a>

##### Externe OCR

Schickt das Dokument zur Texterkennung an einen externen Dienst statt an die eingebaute OCR. Die
Aktion hat keine Einstellungen; dass sie im Workflow steht, genügt. Sie wirkt nur beim Auslöser
**Import gestartet**.

Typischer Einsatz: [`PAPERRUST_REMOTE_OCR_MODE`](configuration.md#PAPERRUST_REMOTE_OCR_MODE) auf
`workflow_only` stellen. Dann geht nur an den Dienst, was ein solcher Workflow auswählt – alles andere
bleibt lokal. Ist kein Dienst eingerichtet oder kann er den Dateityp nicht, nimmt PaperRust die
eingebaute OCR und vermerkt das im Log. Einrichtung des Dienstes: [Remote-OCR](usage.md#remote-ocr).

<a id="workflow-action-apply-ai-suggestions"></a><a id="apply-ai-suggestions"></a>

##### KI-Vorschläge übernehmen

Fragt den eingestellten KI-Dienst nach Vorschlägen und trägt sie ein – dieselben Vorschläge, die Sie
sonst auf der Detailseite sehen, nur automatisch. Die [KI-Funktionen](configuration.md#ai) müssen
eingeschaltet sein. Sie geben an:

- **Vorschläge übernehmen für** – beliebig viele von: Titel, Tags, Korrespondent, Dokumenttyp, Speicherpfad,
  Erstellt am. Vorschläge für nicht gewählte Felder werden verworfen.
- **Fehlendes anlegen** – aus: nur vorhandene Tags, Korrespondenten und Dokumenttypen werden
  eingetragen. An: fehlende werden auch neu angelegt (außer Speicherpfaden, die legt die Aktion nie an).
- **Vorhandene Werte ersetzen** – aus: nur leere Felder werden gefüllt. Da fast jedes Dokument
  schon einen Titel und ein Datum hat, brauchen Sie diese Option meist, wenn Sie Titel oder Datum
  wählen. Tags werden immer nur hinzugefügt, nie ersetzt.

Die Aktion braucht den Text des Dokuments. Deshalb wirkt sie bei allen Auslösern **außer Import
gestartet**, und Dokumente ohne Text werden übersprungen. Die Anfrage an die KI dauert, darum läuft die
Aktion im Hintergrund. Die Vorschläge kommen eventuell erst an, wenn die übrigen Aktionen schon fertig
sind.

> [!WARNING]
> Jedes passende Dokument ist eine Anfrage an den KI-Dienst. Das kann Geld kosten, und der Text des
> Dokuments verlässt bei einem fremden Dienst Ihren Rechner. Viele Anfragen auf einmal können außerdem
> die Warteschlange füllen und neue Importe bremsen. Filtern Sie eng und arbeiten Sie in kleinen
> Mengen.

Mehr dazu: [KI-Funktionen](ai.md).

<a id="workflow-placeholders"></a><a id="platzhalter"></a>

#### Workflow-Platzhalter

Im **Titel**, im **Betreff und Text einer E-Mail** und im **Inhalt und in den Feldern eines Webhooks**
können Sie Platzhalter verwenden. PaperRust setzt beim Lauf die Werte des Dokuments ein. Ein
Platzhalter steht in doppelten geschweiften Klammern, z. B. `{{ correspondent }}`.

Die Schreibweise ist die der Vorlagensprache [Jinja](https://jinja.palletsprojects.com/en/3.1.x/templates/).
Sie können also auch Bedingungen (`{% if … %}`) und Filter (`{{ title | upper }}`) nutzen.
PaperRust prüft eine Vorlage schon beim Speichern des Workflows und meldet Tippfehler und unbekannte
Platzhalter.

**Immer verfügbar:**

| Platzhalter | Bedeutung |
|---|---|
| `{{ correspondent }}` | Name des Korrespondenten (leer, wenn keiner) |
| `{{ document_type }}` | Name des Dokumenttyps |
| `{{ owner_username }}` | Benutzername des Eigentümers |
| `{{ added }}` | Zeitpunkt des Hinzufügens, vollständig mit Uhrzeit und Zeitzone |
| `{{ added_year }}` | Jahr, vierstellig (`2025`) |
| `{{ added_year_short }}` | Jahr, zweistellig (`25`) |
| `{{ added_month }}` | Monat als Zahl (`03`) |
| `{{ added_month_name }}` | Monatsname, englisch (`March`) |
| `{{ added_month_name_short }}` | Monatsname kurz, englisch (`Mar`) |
| `{{ added_day }}` | Tag (`07`) |
| `{{ added_time }}` | Uhrzeit als `HH:MM` |
| `{{ original_filename }}` | Name der hochgeladenen Datei, ohne Endung |
| `{{ filename }}` | Aktueller Dateiname im Archiv, ohne Endung. Direkt nach dem Hinzufügen steht er evtl. noch nicht fest – dann lieber `{{ original_filename }}`. |
| `{{ custom_fields }}` | Die benutzerdefinierten Felder des Dokuments (Erklärung unten) |
| `{{ doc_title }}` | Aktueller Titel. In „Titel setzen“ selbst ist er leer. |

**Nur wenn das Dokument schon existiert** (also bei allen Auslösern außer **Import gestartet**):

| Platzhalter | Bedeutung |
|---|---|
| `{{ created }}` | Erstellungsdatum (`2025-03-07`) |
| `{{ created_year }}`, `{{ created_year_short }}` | Jahr, vier- bzw. zweistellig |
| `{{ created_month }}` | Monat als Zahl |
| `{{ created_month_name }}`, `{{ created_month_name_short }}` | Monatsname lang bzw. kurz, englisch |
| `{{ created_day }}` | Tag |
| `{{ created_time }}` | Uhrzeit als `HH:MM`; das Erstellungsdatum hat keine Uhrzeit, daher immer `00:00` |
| `{{ doc_id }}` | Nummer (ID) des Dokuments |
| `{{ doc_url }}` | Link zum Dokument in der Oberfläche. Funktioniert nur mit gesetztem [`PAPERRUST_URL`](configuration.md#PAPERRUST_URL). |

**Benutzerdefinierte Felder:** `custom_fields` enthält zu jedem Feldnamen den Typ und den Wert.
Ein Feld ohne Leerzeichen im Namen erreichen Sie direkt, z. B. `{{ custom_fields.Rechnungsnummer.value }}`.
Sicherer ist der Filter `get_cf_value`, der auch Namen mit Leerzeichen kann und einen Ersatzwert
erlaubt, falls das Feld fehlt: `{{ custom_fields | get_cf_value("Kunden-Nr", "ohne") }}`. Bei „Titel
setzen“ kennt die Vorlage auch die Felder, die dieselbe Aktion gerade setzt. Mehr zu diesen Werten:
[Dateinamen-Vorlagen](storage-paths.md#additional-variables).

<a id="examples"></a>

##### Beispiele

Monatsnamen in einer anderen Sprache liefert der Filter `localize_date` (Format und Sprache angeben,
siehe [Datum in Landessprache](storage-paths.md#date-localization)):

```jinja2
{{ created | localize_date('MMMM', 'de_DE') }}
{# im Oktober: "Oktober" #}

{{ created | localize_date('dd/MM/yyyy', 'en_GB') }}
{# "26/10/2023" #}
```

Ein Titel beim Import, mit Ersatz, falls kein Korrespondent erkannt wurde:

```jinja2
{% if correspondent %}{{ correspondent }}{% else %}Unbekannt{% endif %} – {{ added_year }}-{{ added_month }}
```

Mail-Text für eine Benachrichtigung:

```jinja2
Neues Dokument „{{ doc_title }}“ von {{ correspondent }}: {{ doc_url }}
```

<a id="workflow-permissions"></a>

### Workflow-Rechte

Workflows gehören niemandem. Wer das Recht hat, Workflows zu bearbeiten, sieht und ändert **alle**
Workflows. Einzelne Workflows lassen sich also nicht für bestimmte Benutzer sperren.

Weil ein Workflow viel bewirken kann – etwa die Rechte vieler Dokumente ändern –, sollten Sie dieses
Recht nur wenigen geben. Wer von paperless-ngx umsteigt, behält die dort vergebenen Rechte.
