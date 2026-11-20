# KI-Funktionen

[English](en/ai.md) · [Übersicht](README.md)

PaperRust kann ein Sprachmodell (LLM) nutzen, um Dokumente schneller abzulegen und Fragen an das
Archiv zu beantworten. Zusatzprogramme wie paperless-gpt oder paperless-ai braucht es dafür nicht.
Alle KI-Funktionen sind **standardmäßig aus**. Die gewohnte automatische Zuordnung ohne KI
([Zuordnung](advanced.md#matching)) bleibt immer erhalten; die KI kommt nur dazu.
Hintergrund und Entwurf: [docs/AI.md](../AI.md).

> [!IMPORTANT]
> Eingeschaltet gibt PaperRust den Text Ihrer Dokumente samt Metadaten an das eingestellte Modell. Bei
> einem Cloud-Anbieter verlassen Ihre Dokumente also den eigenen Server, und es können Kosten
> entstehen. Für vertrauliche Unterlagen ein lokales Modell verwenden (Ollama oder ein eigener
> OpenAI-kompatibler Server).

<a id="enable"></a>

## Einschalten

Am einfachsten in der Oberfläche: im PaperRust-Design unter *Einstellungen → KI* (mit
**Verbindung testen**, siehe [unten](#ai-settings)), in der klassischen Oberfläche unter
*Einstellungen → Konfiguration → KI* (im PaperRust-Design auch *Verwalten → Konfiguration → Anwendung*). Oder über Umgebungsvariablen. Ein Wert, der in der Oberfläche
gesetzt ist, hat Vorrang vor der Umgebungsvariable.

```sh
PAPERRUST_AI_ENABLED=true
PAPERRUST_AI_LLM_BACKEND=ollama                 # oder openai-like (OpenAI oder kompatibler Dienst)
PAPERRUST_AI_LLM_MODEL=llama3.1
PAPERRUST_AI_LLM_ENDPOINT=http://127.0.0.1:11434
PAPERRUST_AI_LLM_EMBEDDING_BACKEND=ollama        # schaltet den KI-Index ein (Chat, bessere Vorschläge)
PAPERRUST_AI_LLM_EMBEDDING_MODEL=nomic-embed-text
```

- **Sprachmodell:** `ollama` braucht `PAPERRUST_AI_LLM_ENDPOINT` (Adresse des Ollama-Servers).
  `openai-like` braucht meist `PAPERRUST_AI_LLM_API_KEY` und – außer bei OpenAI selbst – den
  Endpunkt des Dienstes.
- **Embeddings** (für den KI-Index): `ollama` oder `openai-like`. Ohne eigene Angaben nehmen sie
  Schlüssel und Endpunkt des Sprachmodells; eigene Werte gehen über
  [`PAPERRUST_AI_LLM_EMBEDDING_API_KEY`](configuration.md#PAPERRUST_AI_LLM_EMBEDDING_API_KEY) und
  [`PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT`](configuration.md#PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT).
  `huggingface` geht auch, aber anders als in paperless-ngx: PaperRust rechnet das Modell nicht
  selbst, sondern fragt einen Dienst mit HuggingFace-Schnittstelle – einen eigenen
  `text-embeddings-inference`-Server (z. B. als Docker-Container im eigenen Netz) oder die
  HuggingFace-Inference-API. Dafür ist `PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT` Pflicht; ohne kommt eine
  klare Fehlermeldung, und kein Dokument geht an einen fremden Dienst. Mit demselben Modell (Standard
  `sentence-transformers/all-MiniLM-L6-v2`) passt ein von paperless-ngx übernommener KI-Index weiter.
  Ein Modell direkt im PaperRust-Prozess gibt es bewusst nicht (kostet viel Speicher und Tempo). Wer
  ohnehin Ollama nutzt, nimmt einfacher `ollama` mit dem Modell `all-minilm` (gleiche Modellfamilie;
  danach den KI-Index neu aufbauen).
- **Sprache der Vorschläge:** [`PAPERRUST_AI_LLM_OUTPUT_LANGUAGE`](configuration.md#PAPERRUST_AI_LLM_OUTPUT_LANGUAGE);
  ohne Angabe die Sprache der Oberfläche des Benutzers.

Alle Variablen: [Konfiguration → KI](configuration.md#ai).

<a id="replaces"></a>

## Statt paperless-ai und paperless-gpt

Viele Nutzer von paperless-ngx betreiben dafür Zusatzprogramme wie **paperless-ai** oder
**paperless-gpt**. Deren Kernfunktionen sind in PaperRust eingebaut – ohne zweiten Container, ohne
eigenes API-Token und mit denselben Rechten wie in der Oberfläche:

| Wofür man es nutzte | In PaperRust |
|---|---|
| Titel, Tags, Korrespondent, Typ, Datum automatisch setzen | [Automatische Verarbeitung](#auto) (*Nur vorschlagen* oder *Sichere Vorschläge übernehmen*) |
| Vorschläge erst prüfen, dann übernehmen | [KI-Vorschläge](#ai-review) in der Automatik: je Feld übernehmen, ändern, verwerfen, rückgängig |
| Nur bestimmte Dokumente bearbeiten (Tag „ai“, „paperless-gpt“) | **per Tag verarbeiten** (Tag danach entfernen, ersetzen oder behalten) |
| Altbestand nachbearbeiten | **Bestehendes Archiv nachbearbeiten** (langsam im Hintergrund, anhaltbar) |
| Benutzerfelder (Betrag, Rechnungsnummer …) füllen | **Benutzerfelder ausfüllen**, je Feld einschaltbar, Werte nach Feldtyp geprüft |
| Texterkennung mit einem Bildmodell (LLM-OCR) | **KI-Texterkennung** (nur schwierige Seiten oder immer) |
| Eigene Prompts | **Eigene Prompts** je Aufgabe, mit Test an einem Dokument |
| Chat mit den Dokumenten | [Chat](#chat) bzw. „Frage stellen“ in der Suche, mit Quellen |
| Regeln mit KI | KI-Bedingung und Aktion „KI-Vorschläge holen“ in [Workflows](automatik.md#ai-condition) |
| Anbindung an Agenten | [MCP-Server](#mcp) |

Was anders ist: PaperRust legt nie ungefragt viele neue Tags an (nur mit *Fehlende … anlegen*, mit
Obergrenze je Lauf und Prüfung auf Doppel), jede KI-Änderung steht mit Vorher/Nachher im Verlauf und
lässt sich rückgängig machen, und der Text eines Dokuments darf die KI nie zum Löschen, zu
Rechteänderungen oder zum Versand bringen ([Sicherheit](#security)). Einstellungen aus paperless-ai
oder paperless-gpt übernimmt PaperRust nicht; Sie stellen sie einmal unter *Einstellungen → KI* ein.

<a id="upstream-features"></a><a id="ai-features"></a>

## Was die KI macht

<a id="ai-suggestions"></a>

### Vorschläge im Dokument

In der Dokumentansicht schlägt die KI Titel, Datum, Tags, Korrespondent, Dokumenttyp und
Speicherpfad vor. Die Vorschläge werden nur **angezeigt** – übernommen wird erst, wenn Sie sie anklicken.
Vorhandene Tags, Korrespondenten usw. erkennt PaperRust am Namen (auch bei kleinen
Schreibunterschieden).

Unter *Einstellungen* (Bereich Dokumente) legen Sie fest:

- **Vorschläge kommen von:** *Beide*, *Nur ML* (die eingebaute Zuordnung ohne KI) oder *Nur KI*.
  Bei *Beide* werden die Ergebnisse zusammengeführt.
- **Vorschläge für Dokumente im Posteingang automatisch abrufen:** ist das aus, holen Sie
  Vorschläge mit dem Knopf **Vorschlagen**.

Ist der KI-Index eingeschaltet, sieht das Modell zusätzlich ähnliche Dokumente aus Ihrem Archiv
und trifft dadurch bessere Vorschläge.

<a id="chat"></a>

### Chat mit Ihren Dokumenten

Bei eingeschalteter KI gibt es einen Chat: im PaperRust-Design über „Frage stellen“ im Suchfeld
(Strg + K) bzw. den **Assistenten** und am Dokument („Frage zu diesem Dokument“), in der klassischen
Oberfläche oben in der Leiste. In der Dokumentansicht fragen Sie
dieses eine Dokument, sonst alle Dokumente, die Sie sehen dürfen. Die Antwort stützt sich auf die
passenden Stellen aus dem KI-Index und verlinkt die Quellen. Der Chat braucht den KI-Index.

<a id="llm-index"></a>

### Der KI-Index

Der KI-Index speichert für jedes Dokument eine Art „Bedeutungs-Fingerabdruck“ (Embeddings). So
findet PaperRust Stellen, die inhaltlich zu einer Frage passen, auch wenn andere Wörter darin
stehen (Fachbegriff: RAG).

- Er wird nur gebaut, wenn die KI an **und** ein Embedding-Dienst eingestellt ist.
- Er liegt unter `DATA_DIR/llm_index` im selben Format wie bei paperless-ngx; beim Umstieg kann
  ein vorhandener Index weiterverwendet werden.
- Aktuell gehalten wird er bei jeder Dokumentänderung und täglich nach Zeitplan
  ([`PAPERRUST_LLM_INDEX_TASK_CRON`](configuration.md#PAPERRUST_LLM_INDEX_TASK_CRON)). Neu aufbauen
  von Hand: siehe [Administration → KI-Index](administration.md#llm-index).

<a id="workflow-ai"></a>

### In Workflows

Die Workflow-Aktion **KI-Vorschläge übernehmen** wendet die Vorschläge automatisch an, etwa direkt
nach dem Einlesen ([Workflows](workflows.md#workflow-action-apply-ai-suggestions)).

Vorschläge, Chat und Workflow-Aktion verhalten sich wie in paperless-ngx; das wurde gegen eine
paperless-ngx-Instanz mit Test-Modell verglichen (Einzelheiten in docs/AI.md).

<a id="paperrust-extensions"></a>

## Erweiterungen von PaperRust

Diese Funktionen gibt es nur in PaperRust. Sie liegen unter eigenen Pfaden (`/api/x-ai/…`, `/mcp`);
die übrigen API-Pfade bleiben wie bei paperless-ngx.

<a id="taxonomy"></a>

### Keine doppelten Korrespondenten, Tags und Typen

Bevor über einen KI-Weg (MCP, automatische Verarbeitung) ein Korrespondent, Tag, Dokumenttyp,
Speicherpfad oder Custom Field angelegt wird, sucht PaperRust nach gleichen oder ähnlichen
vorhandenen Objekten: Groß/klein, Umlaute/Akzente, Satzzeichen, Rechtsformen (GmbH, AG, SA, Sàrl,
Ltd, Inc …), Füllwörter, Wortreihenfolge, Ähnlichkeitsmaße. Liegt ein Kandidat über der Schwelle
(`PAPERRUST_AI_MATCH_THRESHOLD`, Standard 0,8), wird **nicht** angelegt, sondern der vorhandene
Eintrag verwendet. Bestätigte Zuordnungen („Swisscom (Schweiz) AG“ = „Swisscom“) merkt sich
PaperRust als Alias.

<a id="mcp"></a>

### MCP-Server für externe Agenten („zweites Gehirn“)

Mit `PAPERRUST_AI_MCP_ENABLED=yes` bietet PaperRust unter `/mcp` einen MCP-Server (Streamable HTTP,
Servername `paperrust`). Damit können KI-Agenten (Claude, Cursor, VS Code, eigene Agenten) das
vorhandene Dokumentarchiv als Wissensquelle nutzen und neue Dokumente ablegen – ohne zweites
Wissenssystem. Anmeldung mit einem API-Token (`Authorization: Bearer <token>` oder
`Token <token>`); der Agent arbeitet **immer mit den Rechten dieses Benutzers** (Modell- und
Objektrechte wie in der Oberfläche). **Kein Löschen**, keine Eigentümer-/Rechteänderung über MCP.

**Werkzeuge** (nach Gruppen):

| Gruppe | Werkzeuge |
|---|---|
| `read` | `search_documents`, `get_document`, `get_document_content` (seiten-/abschnittsweise), `list_recent_documents`, `list_saved_views`, `run_saved_view`, `ask_documents` (Fragen mit Quellen), `get_custom_fields`, `list_notes`, `list_taxonomy`, `find_similar`, `suggest_metadata`, `get_task_status` |
| `write` | `update_document`, `set_custom_field`, `add_note`, `create_taxonomy` (mit erzwungenem Abgleich), `add_alias` |
| `ingest` | `add_document` (Datei, Text/Markdown oder – nur wenn erlaubt – URL) |

Dazu **Ressourcen** `paperrust://document/{id}` (Text) und `paperrust://document/{id}/metadata`
(JSON) sowie die **Prompts** `summarize_document`, `file_document` und `ask_archive`.

**Fragen an das Archiv (`ask_documents`).** Es gibt keinen zweiten Index: PaperRust nutzt den
**vorhandenen [KI-Index](#llm-index)** (derselbe, den der Dokument-Chat verwendet;
`PAPERRUST_AI_ENABLED`, `PAPERRUST_AI_LLM_EMBEDDING_BACKEND` usw., Aufbau über die Aufgabe
`llmindex_index`). Die Suche ist hybrid: Volltextindex + KI-Index (dieselben Chunks, dasselbe
Ähnlichkeitsmaß, dieselbe Rechtefilterung wie der Chat), die Antwort schreibt das konfigurierte
LLM mit den Chat-Vorlagen. Jede Antwort hat Quellen `[n]` mit Dokument-ID, Titel, Seite (wenn der
Text Seitenumbrüche hat) und Ausschnitt.

| Zustand | Verhalten |
|---|---|
| KI an, KI-Index gebaut | hybrid (Volltext + KI-Index), Antwort vom LLM, Quellen |
| KI an, Index noch nicht gebaut | Hinweis, Aufbau wird angestoßen, Antwort aus Volltext-Passagen |
| KI aus | nur die gewichteten Volltext-Passagen mit Quellen, keine LLM-Antwort |

Neu abgelegte Dokumente kommen über den normalen Weg in den Index (Aktualisierung nach dem
Einlesen bzw. periodische Aufgabe `llmindex_index`), es gibt keine eigene Embedding-Pipeline.

**Dokumente ablegen (`add_document`).** Die Datei geht durch die **normale Verarbeitung**
(dieselbe Aufgabe wie ein Upload: Workflows, OCR, automatische Zuordnung, KI-Index). Erlaubt
sind dieselben Typen wie beim Upload (PDF, Bilder, Text/CSV, Office/ODF), erkannt am
Inhalt, nicht am Namen. Text und Markdown werden als Textdokument gespeichert. Korrespondent,
Dokumenttyp, Tags und Custom Fields dürfen per ID **oder per Name** angegeben werden – Namen
werden mit demselben Abgleich wie `create_taxonomy` vorhandenen Objekten zugeordnet; was nicht
passt, ist ein Fehler (nichts wird still neu angelegt), außer mit `create_missing=true` (dann
duplikatgeschützt angelegt, braucht zusätzlich die Gruppe `write`). Rückgabe: eine Aufgaben-ID,
Status mit `get_task_status`. URLs nur mit `PAPERRUST_AI_MCP_ALLOW_URL_FETCH=yes` (Standard aus):
nur http(s), nur öffentliche Adressen (gleiche Prüfung wie für LLM-Endpunkte, Adresse gegen
DNS-Rebinding festgenagelt), Weiterleitungen einzeln geprüft, Größe begrenzt.

**Einstellungen**

| Variable | Standard | Bedeutung |
|---|---|---|
| `PAPERRUST_AI_MCP_ENABLED` | `no` | `/mcp` ausliefern |
| `PAPERRUST_AI_MCP_TOOL_GROUPS` | `read,write` | freigeschaltete Gruppen (Admin-Schalter); `ingest` nur auf Wunsch |
| `PAPERRUST_AI_MCP_ALLOW_URL_FETCH` | `no` | `add_document` mit `url` |
| `PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES` | `1572864` | größte Datei für `add_document` (base64 im Aufruf; größere Dateien über `POST /api/documents/post_document/` mit demselben Token) |
| `PAPERRUST_AI_MCP_MAX_INGESTS_PER_HOUR` | `30` | `add_document` je Token und Stunde |
| `PAPERRUST_AI_MCP_RATE_LIMIT` | `120` | Anfragen je Minute und Token |
| `PAPERRUST_AI_MCP_MAX_CREATES_PER_HOUR` | `30` | Neuanlagen je Token und Stunde |
| `PAPERRUST_AI_MCP_ALLOWED_ORIGINS` | leer | erlaubte Browser-Ursprünge |

Höchstens 20 Sitzungen je Benutzer, Fehlversuche bei der Anmeldung begrenzt.

**Rechte für Firmen: Scopes je Token.** Zusätzlich zu den Benutzerrechten kann ein Administrator
je Token (oder als Vorgabe je Benutzer) festlegen, welche Gruppen erlaubt sind – z. B. ein
Token nur `read` für einen Recherche-Agenten, ein anderes `read,ingest` für einen Ablage-Agenten.
Wirksam ist immer die Schnittmenge aus `PAPERRUST_AI_MCP_TOOL_GROUPS` und den Scopes des Tokens;
Scopes erweitern nie die Rechte des Benutzers. Ohne Eintrag gelten alle freigeschalteten Gruppen.
Gespeichert wird nur der SHA-256 des Tokens (Tabelle `paperrust_ai_mcp_scope`, gilt für API- und
Geräte-Tokens). Verwaltung (nur Superuser):

```sh
# Token nur lesend
curl -X POST https://paperrust.example.com/api/x-ai/mcp-scopes/ \
  -H "Authorization: Token <admin-token>" -H "Content-Type: application/json" \
  -d '{"token": "<agent-token>", "scopes": ["read"], "label": "Recherche-Agent"}'
# Vorgabe für alle Tokens eines Benutzers
curl -X POST .../api/x-ai/mcp-scopes/ ... -d '{"user_id": 7, "scopes": ["read", "ingest"]}'
curl .../api/x-ai/mcp-scopes/ -H "Authorization: Token <admin-token>"          # Liste
curl -X DELETE .../api/x-ai/mcp-scopes/<id>/ -H "Authorization: Token <admin-token>"
```

Tipp: Für Agenten einen eigenen Benutzer mit genau den nötigen Rechten anlegen und dessen Token
verwenden.

**Protokoll.** Jede schreibende Aktion über MCP (`add_document`, `update_document`,
`set_custom_field`, `add_note`, `create_taxonomy`, `add_alias`) und jede Scope-Änderung landet mit
Benutzer, Token-Anfang (nie das Token) und Vorher/Nachher in `paperrust_ai_audit`.

<a id="mcp-clients"></a>

#### Agenten verbinden

Adresse `https://<ihr-server>/mcp`, Anmeldung mit `Authorization: Bearer <token>`.

**Claude Code**

```sh
claude mcp add --transport http paperrust https://paperrust.example.com/mcp \
  --header "Authorization: Bearer <token>"
```

**Claude Desktop** (`claude_desktop_config.json`; Brücke stdio → HTTP mit `mcp-remote`, braucht Node.js):

```json
{
  "mcpServers": {
    "paperrust": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "https://paperrust.example.com/mcp", "--header", "Authorization:${PAPERRUST_AUTH}"],
      "env": { "PAPERRUST_AUTH": "Bearer <token>" }
    }
  }
}
```

**Cursor** (`.cursor/mcp.json`):

```json
{ "mcpServers": { "paperrust": { "url": "https://paperrust.example.com/mcp",
  "headers": { "Authorization": "Bearer <token>" } } } }
```

**VS Code** (`.vscode/mcp.json`, Token wird beim Start abgefragt):

```json
{
  "servers": { "paperrust": { "type": "http", "url": "https://paperrust.example.com/mcp",
    "headers": { "Authorization": "Bearer ${input:paperrust-token}" } } },
  "inputs": [ { "type": "promptString", "id": "paperrust-token", "description": "PaperRust API-Token", "password": true } ]
}
```

**ChatGPT (Connectors)** verlangt für eigene MCP-Server OAuth; PaperRust kennt (noch) nur
Bearer-Tokens. Bis dahin ChatGPT nicht direkt anbinden.

Andere Clients, die nur stdio können, verbinden sich ebenfalls über `npx mcp-remote <url> --header …`.
Clients im Browser senden einen `Origin`-Kopf – diesen in `PAPERRUST_AI_MCP_ALLOWED_ORIGINS`
eintragen. Den Server nur über HTTPS erreichbar machen (Token im Kopf).

<a id="auto"></a>

### Automatische Verarbeitung

`PAPERRUST_AI_AUTO_MODE`:

| Wert | Wirkung |
|---|---|
| `off` (Standard) | nichts automatisch |
| `suggest` | Vorschläge landen in den [KI-Vorschlägen](#ai-review) (`/api/x-ai/review/`) |
| `apply` | Vorschläge ab `PAPERRUST_AI_AUTO_MIN_CONFIDENCE` (0,9) werden übernommen, der Rest in die Prüfliste |

Auslöser: nach dem Einlesen, per Tag (`PAPERRUST_AI_AUTO_TRIGGER_TAG`, geprüft nach
`PAPERRUST_AI_TRIGGER_TAG_CRON`) oder manuell (`POST /api/x-ai/run/`, höchstens 10 Dokumente je
Aufruf und `PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR` je Benutzer). Neue Objekte nur mit
`PAPERRUST_AI_AUTO_CREATE_MISSING=yes`, höchstens `PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS` je Lauf und nur
mit dem `add_*`-Recht des Auslösers bzw. Eigentümers. Jede KI-Änderung steht mit Vorher/Nachher im
Protokoll und lässt sich rückgängig machen (mit Konfliktprüfung).

<a id="ai-settings"></a>

### Einstellungen → KI (PaperRust-Design)

Administratoren stellen alles an einer Stelle ein: *Einstellungen → KI*.

- **KI verwenden** – der Hauptschalter. Aus = keine KI-Funktion wirkt.
- **Anbieter** – *Ollama* (eigener Rechner oder Netzwerk, Dokumente bleiben bei Ihnen) oder ein
  *OpenAI-kompatibler Dienst* (OpenAI, LM Studio, vLLM, LocalAI …). **Verbindung testen** holt die
  Modell-Liste vom Anbieter; das Modell lässt sich daraus wählen.
- **Was automatisch passieren darf** – *Nichts automatisch*, *Nur vorschlagen* (alles landet in den
  KI-Vorschlägen) oder *Sichere Vorschläge übernehmen* (Schwelle wählbar; der Rest wartet auf Sie).
  Dazu: fehlende Tags/Korrespondenten/Typen anlegen (aus = nur Vorhandenes), **per Tag verarbeiten**
  (z. B. Tag „KI“; danach Tag entfernen, durch einen anderen ersetzen – etwa „KI erledigt“ – oder behalten).
- **Benutzerfelder ausfüllen** – je Benutzerfeld einschaltbar. Die KI liest z. B. Betrag,
  Rechnungsnummer oder Fälligkeit aus dem Dokument; Werte werden passend zum Feldtyp geprüft
  (Datum, Betrag mit Währung, Zahl, Ja/Nein, Auswahl, Text). Nur leere Felder werden gefüllt.
- **KI-Texterkennung** – ein Bildmodell (z. B. `qwen2.5vl`, `minicpm-v`, `llava`, `gpt-4o-mini`) liest
  schwierige Scans und Handschrift; mit Grenze für Seiten je Dokument und je Tag. *Wann:* aus, nur bei
  schwierigen Seiten (wenig erkannter Text, Standard) oder immer. Sie läuft nach der normalen
  Texterkennung beim Einlesen und beim **Neu verarbeiten**; beim Hochladen und Neu verarbeiten lässt
  sie sich je Dokument dazuwählen. Ohne eigenes Bildmodell nimmt PaperRust das Textmodell. Einstellungen:
  [`PAPERRUST_AI_VISION_*`](configuration.md#PAPERRUST_AI_VISION_ENABLED).
- **Erweitert** – Sprache der Antworten, Grenzen je Lauf, Tempo der Nachbearbeitung.
- **Eigene Prompts** – je Aufgabe (Einordnung, Titel, Benutzerfelder, KI-Texterkennung, Fragen ans
  Archiv) bearbeitbar, mit Platzhalter-Hilfe (`{{content}}`, `{{filename}}` …), **Zurücksetzen** und
  **An Dokument testen** (Prompt ansehen oder mit KI ausprobieren).
- **Bestehendes Archiv nachbearbeiten** – die KI geht vorhandene Dokumente durch (z. B. „nur ohne
  Korrespondent“ oder „nur noch nie von der KI gesehen“). Vorher zeigt *Wie viele Dokumente?* die Zahl
  und die ungefähre Dauer; der Lauf ist absichtlich langsam, zeigt den Fortschritt und lässt sich
  anhalten und nach einem Neustart fortsetzen.

<a id="ai-review"></a>

### KI-Vorschläge prüfen

Mit eingeschalteter KI gibt es im Bereich **Automatik** den Reiter **KI-Vorschläge**; neben
„Automatik“ steht in der Navigation die Zahl der offenen Vorschläge (auch als Baustein auf der
Startseite). Die alte Adresse `…/ai/review` leitet dorthin. Je Dokument sehen Sie Vorschaubild, bisherigen und neuen Wert je
Feld und wie sicher die KI ist (*sicher*, *vermutlich*, *unsicher*). Je Feld: **Übernehmen**, **Ändern**
(eigenen Wert eintragen) oder **Verwerfen**; gesammelt: *Sichere übernehmen*, *Alle übernehmen*, *Alle
verwerfen*. **Rückgängig** stellt den alten Stand wieder her, solange niemand das Dokument inzwischen
geändert hat. Sie sehen nur Vorschläge zu Dokumenten, die Sie sehen dürfen; entscheiden können Sie nur bei
Dokumenten, die Sie ändern dürfen.


<a id="security"></a>

## Sicherheit

- Dokumentinhalte gelten als **nicht vertrauenswürdig** (Prompt-Injection in PDFs): Die
  automatische Verarbeitung darf nur vorschlagen und zuordnen – nicht löschen, keine Rechte ändern,
  keine Mails oder Webhooks auslösen.
- Rechte werden immer serverseitig geprüft; Daten anderer Benutzer kommen nie in einen Prompt.
- Der MCP-Server ruft für seine Werkzeuge die PaperRust-API auf und reicht dabei das Token des
  Agenten weiter. Die Adresse dafür ist `PAPERRUST_AI_API_BASE`; sie darf nur auf PaperRust selbst
  zeigen. Ohne Angabe nimmt PaperRust `http://127.0.0.1:<PAPERRUST_PORT>` (Standard Port 8000). Das
  passt zum Docker-Image, das auf diesem Port lauscht. Startet man `paperrust` direkt, lauscht es
  aber auf `PAPERRUST_BIND` (Standard `127.0.0.1:8080`) – dann `PAPERRUST_AI_API_BASE` passend
  setzen, z. B. `http://127.0.0.1:8080`.
- Adressen von Sprachmodell und Embedding-Dienst im eigenen Netz (z. B. ein lokales Ollama) sind
  standardmäßig erlaubt. Mit
  [`PAPERRUST_AI_LLM_ALLOW_INTERNAL_ENDPOINTS`](configuration.md#PAPERRUST_AI_LLM_ALLOW_INTERNAL_ENDPOINTS)`=false`
  lässt PaperRust nur öffentliche Adressen zu.
