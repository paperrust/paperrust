# Erste Schritte

[English](en/getting-started.md) · [Übersicht](README.md)

PaperRust verwaltet Ihre Dokumente: Papier einscannen oder Dateien ablegen, PaperRust erkennt den
Text, ordnet Korrespondent, Dokumenttyp und Tags zu und macht alles durchsuchbar. Die REST-API ist
dieselbe wie bei paperless-ngx, Apps für paperless-ngx passen also weiterhin. Die Oberfläche ist
eigen (PaperRust-Design); wer die gewohnte Ansicht von paperless-ngx möchte, wählt unter
*Einstellungen → Darstellung* die klassische Oberfläche.

<a id="choose"></a>

## 1. Betriebsart wählen

- **Schon paperless-ngx im Einsatz?** → [Von paperless-ngx umsteigen](umstieg.md). Ein Befehl bzw.
  ein Klick in der Desktop-App; auf Wunsch in eine Kopie, dann bleibt paperless-ngx unberührt.
- **Neu anfangen auf einem Server** → [Installation ohne Django](installation.md#standalone).
- **Nur ein Rechner, kein Server** → [Desktop-App](installation.md#desktop) (Windows-Installer, noch nicht signiert).

<a id="first-login"></a>

## 2. Anmelden und einrichten

1. Administrator anlegen (`paperrust manage createsuperuser`) und unter `PAPERRUST_URL` anmelden.
2. Der **Rundgang** (Benutzermenü → *Rundgang starten*) zeigt die wichtigsten Stellen. Unter
   *Einstellungen* wählen Sie Sprache, Datumsformat und Darstellung.
3. Die wichtigsten Ordnungsbegriffe anlegen ([Begriffe](usage.md#terms-and-definitions)):
   - **Korrespondenten** – von wem ein Dokument stammt (Bank, Versicherung …),
   - **Dokumenttypen** – was es ist (Rechnung, Vertrag …),
   - **Tags** – freie Schlagworte; ein Tag kann als *Posteingang* markiert werden,
     Fehlendes können Sie später auch direkt im Feld eines Dokuments anlegen,
   - **Speicherpfade** – wohin die Datei abgelegt wird ([Dateinamen](storage-paths.md)).
4. Für jedes Objekt die **Zuordnung** wählen (*Automatisch* lernt aus Ihren Zuordnungen,
   alternativ Wortlisten oder reguläre Ausdrücke – [Details](advanced.md#matching)).

<a id="add-documents"></a>

## 3. Dokumente hinzufügen

- **Hochladen** per Ziehen in die Oberfläche,
- **Eingangsordner**: Dateien in `PAPERRUST_CONSUMPTION_DIR` legen (z. B. als Ziel des Scanners),
- **E-Mail**: Konten unter *Verwalten → E-Mail*, Regeln unter *Automatik → E-Mail-Abruf* ([Mail](mail.md)),
- **API / Apps**: `POST /api/documents/post_document/` ([API](api.md#file-uploads)).

Der Fortschritt erscheint oben rechts; fertige Dokumente landen mit dem Posteingangs-Tag in der Liste.

<a id="next"></a>

## 4. Weiter

- Suchen und Filtern, gespeicherte Ansichten, Freigabelinks: [Nutzung](usage.md)
- Abläufe automatisieren: [Automatik](automatik.md) (Nachschlagen: [Workflows](workflows.md))
- Eigene Felder (Betrag, Fälligkeit …): [Custom Fields](custom-fields.md)
- Benutzer und Rechte: [Rechte](permissions.md)
- Sicherung: [Administration](administration.md#backup)
- Optionale KI: [KI](ai.md)
- Mit mehreren Personen: [Für Firmen](firmen.md)
