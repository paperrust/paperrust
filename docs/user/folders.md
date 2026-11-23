# Ordneransicht

[English](en/folders.md)

Die **Ordneransicht** sortiert Dokumente wie in einem Dateimanager: Jedes Dokument liegt in **höchstens einem**
Ordner. Ordner können Unterordner haben. Dokumente ohne Ordner stehen unter **Unsortiert**. Tags,
Korrespondenten, Suche und gespeicherte Ansichten funktionieren unverändert weiter – der Ordner ist nur eine
zusätzliche Ordnung.

Die Ordneransicht gibt es nur in PaperRust (nicht in paperless-ngx) und nur im PaperRust-Design.

<a id="einschalten"></a>
## Einschalten

Die Ansicht ist zunächst aus. Einschalten unter **Einstellungen → Darstellung → Ordneransicht** (gilt je
Benutzer). Danach erscheint in der Navigation der Punkt **Ordner**. Ausschalten blendet alles wieder aus; die
Ordner und Zuordnungen bleiben erhalten.

<a id="bedienen"></a>
## Bedienen

- **Links** steht der Ordnerbaum. Ganz oben **Unsortiert (Anzahl)**, darunter Ihre Ordner mit der Zahl der
  Dokumente, die direkt darin liegen. Mit dem Pfeil klappen Sie Unterordner auf und zu.
- **Rechts** steht die gewohnte Dokumentliste, eingeschränkt auf den gewählten Ordner. Filter und Suche wirken
  innerhalb des Ordners. Oben zeigt die Brotkrume den Pfad (`Ordner › Rechnungen › 2026`).
- **Neuer Ordner:** Ordner-Schaltfläche über dem Baum. Ist ein Ordner gewählt, entsteht ein Unterordner.
- **Umbenennen** und **Löschen:** Stift- bzw. Papierkorb-Schaltfläche, sie wirken auf den gewählten Ordner.
  Ordnernamen sind je Ebene eindeutig (Groß-/Kleinschreibung egal).
- **Löschen löscht nie Dokumente.** Sie wählen, wohin der Inhalt geht: in den übergeordneten Ordner oder nach
  „Unsortiert“ (Unterordner wandern dann auf die oberste Ebene).
- **Ordner umhängen:** einen Ordner im Baum mit der Maus auf einen anderen Ordner ziehen. Auf „Unsortiert“
  gezogen, wandert er auf die oberste Ebene. In sich selbst oder einen eigenen Unterordner geht das nicht.

<a id="kacheln"></a>
## Liste oder Kacheln

Oben rechts über der Liste schalten Sie zwischen **Liste** und **Kacheln** um. PaperRust merkt sich die Wahl für
Ihr Benutzerkonto.

- In der Kachelansicht stehen zuerst die **Unterordner** des geöffneten Ordners (unter „Unsortiert“: Ihre
  obersten Ordner), dann die **Dokumente** als Vorschaubilder mit Titel.
- **Doppelklick** oder **Enter** auf eine Ordnerkachel öffnet den Ordner. Auf dem Handy genügt ein Tippen.
- Ein **Klick** auf ein Dokument öffnet es. Die Kacheln rücken dann zusammen und rechts erscheint das Dokument.
- **Auswählen** wie in der Liste: **Strg-Klick** (Mac: Cmd-Klick) wählt einzelne Dokumente dazu oder ab,
  **Umschalt-Klick** wählt alles zwischen dem zuletzt angeklickten und dem neuen Dokument. Die Pfeiltasten
  bewegen sich durch die Kacheln, die Leertaste wählt aus.

<a id="verschieben"></a>
## Dokumente einsortieren

- **Ziehen und ablegen:** ein Dokument (oder mehrere ausgewählte) aus der Liste oder den Kacheln mit der Maus auf
  einen Ordner im Baum, auf eine Ordnerkachel oder auf „Unsortiert“ ziehen. Das Ziel wird beim Darüberziehen
  hervorgehoben. Danach können Sie mit **Rückgängig** im Hinweis alles zurückholen.
- **Mehrere Dokumente:** in der Liste auswählen, dann in der Leiste **In Ordner verschieben** wählen. „Unsortiert“
  nimmt die Dokumente aus ihrem Ordner. Das geht auch mit der Tastatur und auf dem Handy (dort gibt es kein
  Ziehen).
- **Ein Dokument:** in den Details das Feld **Ordner** ändern.

Dafür brauchen Sie das Recht „Dokumente ändern“ für jedes Dokument und das Änderungsrecht am Zielordner.

<a id="hochladen"></a>
## Direkt in einen Ordner hochladen

Ist ein Ordner geöffnet, landen neu hochgeladene Dokumente **in diesem Ordner** – egal ob Sie die Dateien ins
Fenster ziehen oder **Hochladen** wählen. Beim Ziehen zeigt PaperRust „In den Ordner …“ an. Unter „Unsortiert“
oder außerhalb der Ordneransicht landen sie wie bisher ohne Ordner.

Dafür brauchen Sie das Änderungsrecht am Ordner. Wird der Ordner gelöscht, bevor die Verarbeitung fertig ist,
landet das Dokument in „Unsortiert“.

<a id="rechte"></a>
## Wer sieht welche Ordner?

- Ein neuer Ordner gehört Ihnen. Ordner ohne Besitzer sehen alle.
- Freigaben für Ordner gibt es bisher nur über die API (`/rs/api/v2/folders`, Feld `set_permissions`).
- Ordnerrechte **vererben sich nicht** auf die Dokumente: Wer ein Dokument nicht sehen darf, sieht es auch im
  Ordner nicht. Liegt ein Dokument in einem Ordner, den Sie nicht sehen, finden Sie es unter „Alle Dokumente“,
  aber nicht unter „Unsortiert“.

<a id="sicherung"></a>
## Sicherung und Rückweg zu paperless-ngx

- `document_exporter` schreibt die Ordner in `paperrust-extras.json`; `document_importer` von PaperRust stellt sie
  wieder her.
- paperless-ngx kennt keine Ordner. Nach einem Wechsel zu paperless-ngx sind die Ordner dort nicht sichtbar
  (Tags, Korrespondenten usw. bleiben vollständig).
- Programme, die über die REST-API hochladen, können das zusätzliche Feld `folder` (Ordner-ID) mitschicken. Das
  Feld gibt es nur in PaperRust; paperless-ngx übergeht es.
- In der REST-API (v1) hat jedes Dokument das zusätzliche Lesefeld `folder`, sobald es mindestens einen Ordner
  gibt; die Filter heißen `folder__id`, `folder__id__in`, `folder__id__tree` (mit Unterordnern) und
  `folder__isnull=1` (Unsortiert).
