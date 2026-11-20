# Automatik (Workflows)

[English](en/automatik.md) · [Übersicht](README.md) · Nachschlagen aller Optionen: [Workflows](workflows.md)

Im PaperRust-Design gibt es links in der Navigation den Bereich **Automatik**. Dort steht alles, was
PaperRust von selbst erledigt:

| Reiter | Wofür |
|---|---|
| **Workflows** | Regeln nach dem Muster „Wenn …, dann …“ – etwa Tags setzen, wenn ein Dokument hinzugefügt wird |
| **E-Mail-Abruf** | welche E-Mails abgeholt werden und welche Workflows danach laufen |
| **KI-Vorschläge** | Prüfliste mit den Vorschlägen der KI (nur sichtbar, wenn die [KI](ai.md) eingeschaltet ist) |

Den Bereich sehen Sie nur, wenn Sie Workflows oder E-Mail-Regeln ansehen dürfen. Bei eingeschalteter
KI steht neben **Automatik** die Zahl der offenen KI-Vorschläge.

Die klassische Oberfläche zeigt dieselben Workflows weiter unter *Verwalten → Workflows*. Beide
Oberflächen speichern genau gleich; Sie können also jederzeit wechseln.

<a id="list"></a>

## Die Liste der Workflows

Jeder Workflow steht als **ein Satz** in der Liste, zum Beispiel:

> Wenn ein Dokument hinzugefügt wurde und der Text „Rechnung“ enthält, dann den Tag „Rechnung“
> hinzufügen und den Dokumenttyp „Rechnung“ setzen.

Darunter steht, wie oft er diese Woche gelaufen ist und wann zuletzt (mit Fehler: rot markiert). Mit
dem Schalter links schalten Sie einen Workflow an oder aus. Das Menü **Weitere Aktionen** bietet
*Duplizieren* (die Kopie ist zuerst aus), *Verlauf* und *Löschen*.

Workflows laufen **von oben nach unten**. Die Reihenfolge ändern Sie am Griff (ziehen) oder mit
**Alt + Pfeil nach oben/unten**.

<a id="new"></a>

## Einen Workflow anlegen

**Neuer Workflow** öffnet drei Wege:

- **Vorlagen** – fertige Bausteine zum Anpassen: *Rechnungen sortieren*, *Steuerordner*,
  *Benachrichtigen*, *Passwort-PDFs öffnen*, *Mit KI vorsortieren* (nur mit KI) und *Leer beginnen*.
- **Vorschläge aus Ihrem Archiv** – PaperRust schaut ohne KI nach, was Sie bisher fast immer gleich
  eingeordnet haben („Bei Korrespondent X immer Dokumenttyp Y“). Mit **Übernehmen** wird daraus ein
  Entwurf. Vorschläge, die ein Workflow schon abdeckt, sind ausgeblendet.
- **Mit KI beschreiben** (nur mit KI) – in eigenen Worten schreiben, was passieren soll, z. B.
  „Rechnungen von Swisscom bekommen den Tag ‚Rechnung‘ und landen im Ordner ‚Telefon‘“.
  **Entwurf erstellen** macht daraus einen Workflow, den Sie prüfen und anpassen. Gespeichert wird
  erst, wenn Sie **Speichern** drücken.

<a id="editor"></a>

## Der Editor: Karten von oben nach unten

Oben steht der Workflow als Satz. Die Teile des Satzes sind anklickbar; einfache Workflows lassen
sich ganz im Satz bauen (**+ Bedingung**, **+ Aktion**). Darunter stehen die Karten:

| Karte | Frage | Hinweis |
|---|---|---|
| **Wenn** | Wann soll der Workflow starten? | ein oder mehrere Auslöser |
| **Nur wenn** | Für welche Dokumente? | Bedingungen; ohne Bedingung: „Immer“ |
| **Nur wenn die KI meint** | Passt das Dokument laut KI? | nur mit KI, siehe unten |
| **Dann** | Was soll passieren? | Aktionen, von oben nach unten |
| **Sonst** | Was, wenn die Bedingungen nicht zutreffen? | freiwillig (**+ Sonst**) |

Ein Klick auf eine Karte öffnet rechts alle Optionen. Selten gebrauchte Optionen liegen unter
**Erweitert**. Fehlt etwas (Name, Auslöser, Aktion), markiert der Editor die Stelle.

<a id="triggers"></a>

### Auslöser („Wenn“)

| Im PaperRust-Design | In der klassischen Oberfläche | Wann |
|---|---|---|
| **Einlesen beginnt** | Import gestartet | bevor eine Datei verarbeitet wird – nur Dateiname, Pfad und Quelle sind bekannt |
| **Dokument hinzugefügt** | Dokument hinzugefügt | sobald ein neues Dokument fertig verarbeitet ist |
| **Dokument geändert** | Dokument geändert | wenn jemand ein Dokument speichert |
| **Nach Zeitplan** | Zeitgesteuert | eine Zahl von Tagen vor oder nach einem Datum, auf Wunsch wiederholt |
| **Von Hand** | Manuell | nur, wenn jemand den Workflow bei einem Dokument startet |
| **E-Mail kommt an** | Import gestartet mit Filter „Mail-Regel“ | wenn eine E-Mail-Regel ein Dokument abruft |

Bei **E-Mail kommt an** wählen Sie eine E-Mail-Regel oder legen gleich eine neue an. Intern bleibt
das eine Mail-Regel plus ein Workflow – so wie paperless-ngx es speichert.

<a id="conditions"></a>

### Bedingungen („Nur wenn“)

Die häufigen Bedingungen gibt es als Knöpfe: **Text enthält**, **Tag**, **Korrespondent**,
**Dokumenttyp**, **Speicherpfad**, **Dateiname**, **Quelle**. Unter **Erweitert** liegen Pfad beim
Einlesen, E-Mail-Regel, Bedingungen für benutzerdefinierte Felder und die Art des Textvergleichs
(genau, ungefähr, alle Wörter, eines der Wörter, regulärer Ausdruck, gelernt). Der Workflow läuft
nur, wenn **alle** Bedingungen zutreffen.

Beim Auslöser **Einlesen beginnt** gibt es noch keinen Text. Bedingungen zum Inhalt stellen Sie deshalb
mit **Dokument hinzugefügt**.

<a id="ai-condition"></a>

### KI-Bedingung („Nur wenn die KI meint“)

Nur sichtbar, wenn die KI eingeschaltet ist. Die KI liest das Dokument und prüft zum Beispiel, ob es
eine Rechnung ist oder zu einem Korrespondenten gehört. Mit dem Regler legen Sie fest, wie sicher sie
sein muss („Mindestens so sicher: 80 %“). Höher heißt seltener, aber zuverlässiger. Ist die KI sicher
genug, laufen die Aktionen unter **Dann**, sonst die unter **Sonst**.

- Beim Auslöser **Einlesen beginnt** gibt es noch kein Dokument; dort wird die KI-Bedingung übergangen.
- Ist die KI aus oder nicht erreichbar, wird die Bedingung nicht geprüft; der Verlauf vermerkt das.

<a id="actions"></a>

### Aktionen („Dann“ und „Sonst“)

Schnell wählbar: **Tag hinzufügen**, **Korrespondent setzen**, **Dokumenttyp setzen**, **Speicherpfad
setzen**, **Titel setzen**, **E-Mail senden**, **In den Papierkorb legen**, **KI-Vorschläge holen**
(nur mit KI). Unter **Andere Aktion …** gibt es den Rest: Werte entfernen, Eigentümer und Rechte,
benutzerdefinierte Felder, Webhook, **Passwort entfernen** und **Text mit dem externen Dienst
erkennen** (Remote-OCR). Was jede Aktion genau tut und welche Platzhalter es im Titel gibt, steht
unter [Workflows → Aktionen](workflows.md#workflow-actions).

Die Reihenfolge der Aktionen ändern Sie am Griff oder mit Alt + Pfeiltaste.

Der **Sonst-Zweig** ist eine Erweiterung von PaperRust. paperless-ngx kennt ihn nicht; die klassische
Oberfläche zeigt nur den Hauptteil.

<a id="dryrun"></a>

## Testlauf und Anwenden

**Testlauf** zeigt, was der Workflow mit Ihren neuesten Dokumenten machen würde – ohne etwas zu
ändern, ohne E-Mail und ohne Webhook.

- Sie wählen, wie viele Dokumente geprüft werden und **als welcher Auslöser** getestet wird.
- Das Ergebnis: wie viele Dokumente geändert würden, wie viele in den Sonst-Zweig fallen, wie viele
  nicht betroffen sind und wo ein Fehler auftrat. Je Dokument sehen Sie **vorher/nachher**.
- **Einlesen beginnt** wird nachgestellt: Geprüft werden vorhandene Dokumente, als würden sie gerade
  eingelesen. Bei **Nach Zeitplan** prüft der Testlauf nur die Bedingungen, nicht den Zeitpunkt.
- Die KI-Bedingung gilt im Testlauf als erfüllt. Mit **KI-Bedingung wirklich prüfen** fragt PaperRust
  die KI (höchstens 10 Dokumente).

Danach können Sie gewählte Dokumente auswählen und **Auf N anwenden**. Dann laufen die Aktionen im
Hintergrund wirklich, auch E-Mails und Webhooks. Das lässt sich nicht automatisch rückgängig machen;
PaperRust fragt deshalb vorher nach. Ein neuer Workflow muss dazu zuerst gespeichert sein.

<a id="history"></a>

## Verlauf

**Verlauf** zeigt je Workflow jeden Lauf: wann, bei welchem Dokument, mit welchem Ergebnis (OK,
übersprungen, Fehler, Sonst-Zweig) und was sich geändert hat. Fehler stehen deutlich da; der Filter
zeigt nur Fehler, nur Erfolge oder nur Übersprungenes. Dokumente, die Sie nicht sehen dürfen,
erscheinen ohne Titel.

Wie lange der Verlauf aufbewahrt wird, legt
[`PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS`](configuration.md#PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS)
fest (Standard 90 Tage, 0 = unbegrenzt).

<a id="mail"></a>

## E-Mail-Abruf

Der Reiter **E-Mail-Abruf** zeigt je E-Mail-Regel einen **Ablauf**: welche E-Mails abgerufen werden
(Konto, Ordner, Filter), was übernommen wird (Anhänge, ganze Mail) und welche Workflows danach laufen.

- **Neuer E-Mail-Ablauf** legt eine Regel an. Die Fragen heißen *Welche E-Mails?*, *Was wird
  übernommen?*, *Werte für das Dokument* und *Was passiert danach mit der E-Mail?*.
- **Workflow für diese Regel hinzufügen** legt einen Workflow mit dem Auslöser *E-Mail kommt an* an.
- E-Mail-**Konten** richten Sie unter *Verwalten → E-Mail* ein (**Konten verwalten**). Einzelheiten
  zu Konten und Regeln: [Mail](mail.md).
- Löschen Sie eine Regel, bleiben ihre Workflows erhalten; sie starten aber nicht mehr bei dieser Regel.

<a id="ai-review"></a>

## KI-Vorschläge

Nur mit eingeschalteter KI. Hier prüfen Sie Feld für Feld, was die KI vorschlägt: übernehmen, ändern
oder verwerfen; Übernommenes lässt sich rückgängig machen. Einzelheiten:
[KI → KI-Vorschläge prüfen](ai.md#ai-review).

<a id="rights"></a>

## Rechte

Wer Workflows ansehen, anlegen oder ändern darf, regeln die normalen Rechte für Workflows (und für
E-Mail-Regeln beim E-Mail-Abruf), siehe [Workflow-Rechte](workflows.md#workflow-permissions). Beim
Anwenden prüft PaperRust, ob Sie jedes gewählte Dokument ändern dürfen.
