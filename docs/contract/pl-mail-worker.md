# Vertrag `pl-mail` – Mail-Worker (`fetch/`) und Workflow-Effekte (`effects/`)

Quelle: paperless-ngx v3.4.2 (`upstream/`, unverändert), imap_tools 1.15.1, CPython 3.12 (`imaplib`,
`email` compat32, `smtplib`), Django 5.2 (`core/mail`), httpx 0.28.1, Celery 5.6.3. Kein HTTP-Endpunkt
(keine neuen Routen). Ersetzt die Celery-Aufgaben des Mail-Abrufs und die E-Mail-/Webhook-Aktionen
der Workflows.

| Rust | upstream |
|---|---|
| `fetch::process_mail_accounts`, `fetch::tasks::*_task` | `paperless_mail/tasks.py::process_mail_accounts` |
| `fetch::handler::AccountRun` | `paperless_mail/mail.py::MailAccountHandler` (`handle_mail_account`, `_handle_mail_rule`, `_handle_message`, `_process_attachments`, `_process_eml`, `_get_title`, `_get_correspondent`, `_get_uid_validity`, `_record_processed_without_consumption`, `queue_consumption_tasks`) |
| `fetch::apply_mail_action`, `fetch::error_callback` | `mail.py::apply_mail_action`, `error_callback` |
| `fetch::criteria` | `mail.py::BaseMailAction`+Unterklassen, `get_rule_action`, `make_criterias`, `APPLE_MAIL_TAG_COLORS`; imap_tools `query.py` |
| `fetch::client::MailBox` | `mail.py::get_mailbox`, `Pinned*MailBox`, `mailbox_login`; imap_tools `mailbox.py`/`folder.py`/`utils.py`/`imap_utf7.py`; `imaplib` |
| `fetch::message` | imap_tools `message.py` (`MailMessage`, `MailAttachment`) über `pl_parsers::mail` |
| `fetch::generator` | `_process_eml` (From nach vorn) + `email.generator.BytesGenerator` (compat32) + `email.header.Header.encode` |
| `fetch::db` | Modelle `MailAccount`, `MailRule`, `ProcessedMail`; `Correspondent.get_or_create` + django-auditlog |
| `effects::SharedEffects` (`WorkflowEffects`) | `documents/workflows/actions.py::execute_email_action`, `execute_webhook_action` |
| `effects::context` | `actions.py::build_workflow_action_context` |
| `effects::smtp` | `documents/mail.py::send_email`, Django `EmailMessage` + SMTP-Backend |
| `effects::webhook` | `documents/workflows/webhooks.py::send_webhook`, `paperless/network.py` |

## Einbindung (für W1)

```rust
let worker = Arc::new(pl_mail::fetch::MailWorker::new(pool.clone(), queue.clone(),
    pl_mail::fetch::MailWorkerConfig::from_core(&core)));
let webhooks = Arc::new(pl_mail::effects::webhook::WebhookSettings::from_env(&Env::from_process())?);
handlers = pl_mail::fetch::register_mail_handlers(handlers, worker, webhooks);
// Workflow-Effekte im Consumer:
consumer.effects = Arc::new(pl_mail::effects::SharedEffects(Arc::new(
    pl_mail::effects::MailWorkflowEffects::new(pool.clone(), Some(queue.clone()), &core)?)));
```

`register_mail_handlers(pl_queue::Handlers, Arc<MailWorker>, Arc<WebhookSettings>) -> Handlers`
registriert `paperless_mail.tasks.process_mail_accounts`, `paperless_mail.mail.apply_mail_action`,
`paperless_mail.mail.error_callback` und `documents.workflows.webhooks.send_webhook` (Neuversuch-Regel
aus `pl_queue::registry`: 3 Neuversuche, Backoff 1·2ⁿ s mit Jitter, max. 600 s, nur bei
`HTTPStatusError`). Zeitplan: Beat-Eintrag „Check all e-mail accounts“ (`PAPERLESS_EMAIL_TASK_CRON`,
Standard `*/10` mit SECRET_KEY-Offset, `expires` 540 s) steht in `pl_queue::beat::DEFAULT_ENTRIES`.
`consume_file` muss im selben Worker registriert sein (pl-consumer `task::handler`).

## `process_mail_accounts(account_ids=None)`

1. Sperre: `pg_try_advisory_lock(0x7061706572 6d6c66)` auf einer eigenen (abgekoppelten) Verbindung.
   Belegt → Ergebnis `"Skipped: mail account processing already in progress."`. Freigabe am Ende
   bzw. automatisch beim Verbindungsabbruch.
2. Konten `id = ANY(account_ids)` (leer/None → alle), nach `id`. Ohne aktivierte Regel:
   Log `No rules enabled for account …. Skipping.`
3. Je Konto `handle_mail_account`; `MailError` → Log, weiter.
4. Ergebnis `"Added N document(s)."` bzw. `"No new documents were added."` (N = eingereihte Dateien).
   Abweichung (pl-mail.md Known difference 26): Konto-Fehler (MailError, Verbindungs-/Postfachfehler,
   OAuth-Erneuerung gescheitert, alle aktiven Regeln gescheitert) werden mitgezählt. Alle versuchten
   Konten gescheitert → Task FAILURE `MailError: All N mail account(s) failed: <konto>: <fehler>; …`;
   nur einige → upstream-Text + ` K of N mail account(s) failed: …`. Ohne Fehler Texte unverändert.

## `handle_mail_account(account)`

`get_mailbox` (DNS + Sperre nicht-öffentlicher IPs bei `PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS=false`,
Verbindung zu den gepinnten IPs, TLS mit SNI auf den Hostnamen, STARTTLS mit Fähigkeitsprüfung,
Port 993 + STARTTLS → Fehler) → Begrüßung → `CAPABILITY`. MailError beim Verbinden → an den Aufrufer;
andere Fehler → Log `Error while retrieving mailbox`, 0.
Danach: abgelaufenes OAuth-Token (`is_token` und `expiration < now`) → Refresh
(`oauth::refresh_account_oauth_token`, speichert Passwort/Ablauf/Refresh-Token; Misserfolg → 0 ohne
Login). `X-GM-EXT-1` in den Fähigkeiten → Gmail-Labels. Login wie `mailbox_login` (XOAUTH2 /
`LOGIN "u" "p"` / `AUTHENTICATE PLAIN` für Nicht-ASCII-Passwörter) + `SELECT "INBOX"`; Fehler →
MailError. Regeln nach `order` (dann `id`), deaktivierte übersprungen, Fehler je Regel protokolliert,
`stop_processing` nach der ersten Regel mit > 0 Dateien (kumuliert). Am Ende immer `LOGOUT`.

## `_handle_mail_rule`

- `SELECT` des Ziel-Ordners (bei MOVE zuerst `action_parameter`), dann des Regel-Ordners;
  `NO` → Log, `LIST "" "*"`, MailError `Rule <konto>.<regel>: Folder <f> does not exist in account <konto>`.
  `READ-ONLY` → imaplib-`readonly`-Fehler.
- `STATUS "<ordner>" (UIDVALIDITY)` → `uid_validity` (Fehler → Warnung, `None`).
- Kriterien (imap_tools, Schlüssel alphabetisch): `SINCE d-Mon-yyyy` (heute in TIME_ZONE − maximum_age,
  nur > 0), `FROM`/`TO`/`SUBJECT`/`BODY "…"` (nicht leer), Aktion: MARK_READ `UNSEEN`, FLAG und
  Apple-Farbe `UNFLAGGED`, TAG `UNKEYWORD kw` bzw. mit Gmail `(NOT (X-GM-LABELS "kw") UNKEYWORD kw)`;
  DELETE/MOVE ohne Filter → `ALL`. `UID SEARCH CHARSET <character_set> <kriterien>` (Kriterien im
  Zeichensatz des Kontos kodiert); Fehler → MailError.
- Bereits verarbeitet: `ProcessedMail(rule, folder, uid ∈ …[, uid_validity = v OR NULL])` in Stapeln
  zu 10 000. Neue UIDs numerisch sortiert, `UID FETCH <500er-Stapel> (BODY.PEEK[] UID FLAGS RFC822.SIZE)`,
  Antwortelemente paarweise wie imap_tools.
- Je Nachricht: doppelt im Lauf / schon von früherer Regel eingereiht (`(ordner, uid)`) / schon
  verarbeitet → überspringen. Fehler einer Nachricht → Log, weiter.

## `_handle_message`

- Nur Anhänge (`consumption_scope=1`) ohne Anhänge → `ProcessedMail` `PROCESSED_WO_CONSUMPTION`
  (`get_or_create` auf rule/uid/folder/uid_validity).
- `tag_ids` = Regel-Tags (Ordnung nach Tag-Name), Dokumenttyp der Regel.
- Scope 2/3: `.eml`: `mkstemp(prefix="paperless-mail-", suffix=".eml")` in `SCRATCH_DIR`, Bytes =
  `_process_eml` (letzte `From`-Kopfzeile nach vorn, `as_bytes()` compat32: Zeilenenden `\n`,
  ASCII-Kopfzeilen neu gefaltet (78), 8-Bit-Kopfzeilen roh, multipart/signed ungefaltet, Präambel/
  Epilog wie feedparser). Overrides: `title=subject` (ohne NFC), `filename=sanitize(NFC(subject+".eml"))`,
  Korrespondent, Dokumenttyp, Tags, `owner_id = rule.owner` (**ohne** `assign_owner_from_rule`, wie
  upstream).
- Scope 1/3: Anhänge (imap_tools-Definition: Teile mit Content-ID, Dateiname oder `message/rfc822`).
  `attachment_type=1` → nur `Content-Disposition: attachment`. Ein-/Ausschluss-Muster (kommagetrennt,
  `fnmatch` auf Kleinbuchstaben). Korrespondent je Anhang (FROM_EMAIL/FROM_NAME legen per
  `get_or_create(name)` an: `match=name`, `matching_algorithm=3`, `is_insensitive=true`, Audit-Eintrag
  `create`; DB-Fehler → Log, kein Korrespondent; FROM_CUSTOM = Regel-Korrespondent). Titel: Betreff /
  `NFC(stem(dateiname))` / keiner. MIME per libmagic-Nachbau (`pl_parsers::mime::detect_bytes`);
  unterstützt (`ParserRegistry::is_mime_type_supported`) → `mkdtemp(prefix="paperless-mail-")`,
  Datei `sanitize(NFC(filename))` bzw. `no-name-attachment`. Overrides: title, filename,
  correspondent, document_type, tag_ids, `owner_id` nur mit `assign_owner_from_rule`.
  Keine Datei → `PROCESSED_WO_CONSUMPTION`.
- Einreihen: **ein** `chord([consume_file…] (trigger_source=email_consume), apply_mail_action.s(rule_id,
  message_uid, message_subject, message_date, uid_validity, folder, account_id)).on_error(error_callback.s(…))`
  je Mail – bei Scope 3 (EVERYTHING) eml zuerst, dann die Anhänge in Mail-Reihenfolge im selben Chord
  (upstream: zwei Chords mit derselben Aktion; pl-mail.md Known difference 25).
  `PROCESSED_WO_CONSUMPTION` nur, wenn gar nichts eingereiht wurde. `folder`/`account_id` sind
  PaperRust-kwargs (Known difference 24). `message_date` als ISO-Text, aware in TIME_ZONE.
  `consume_file`-kwargs wie pl-queue B5.

## `apply_mail_action` / `error_callback`

`apply_mail_action`: Regel laden; zeigt sie inzwischen auf ein anderes Konto oder einen anderen Ordner
als in den kwargs `account_id`/`folder` → überspringen ohne Verbindung (pl-mail.md Known difference 24).
Sonst neue Verbindung, Login (`SELECT "INBOX"`), `SELECT <regel-ordner>`; UIDVALIDITY aus
`OK [UIDVALIDITY n]` (sonst `STATUS`) ≠ gespeicherter Wert → überspringen. Überspringen = Warnung im
Log, `ProcessedMail` `FAILED` mit Grund in `error`, Task-Ergebnis SUCCESS mit dem Grund als Text.
Unbekannte UIDVALIDITY oder Tasks ohne die neuen kwargs → keine Prüfung, wie upstream.
Aktionen (Known difference 23 – **kein** ordnerweites `EXPUNGE` nach Markier-Aktionen):
DELETE `UID STORE u +FLAGS (\Deleted)` + `UID EXPUNGE u` (mit `UIDPLUS`, auch nach der Anmeldung
gemeldet); MARK_READ/FLAG `UID STORE u +FLAGS (\Seen|\Flagged)` ohne `EXPUNGE`; MOVE `UID MOVE u "<ziel>"`
bzw. ohne `MOVE`-Fähigkeit `UID COPY` + DELETE wie eben; TAG Gmail `UID STORE u +X-GM-LABELS kw`
(Antwortstatus geprüft: `NO` → `MailboxTaggingError: "NO" received` → FAILED, Known difference 27;
Apple-Farbe mit Gmail → ohne Argument, wie upstream), Apple `-FLAGS ($MailFlagBit0 $MailFlagBit1
$MailFlagBit2)`, `+FLAGS (<bits>)`, `+FLAGS (\Flagged)`, sonst `+FLAGS (kw)` – jeweils ohne `EXPUNGE`.
Rückfall ohne `UIDPLUS` (nur DELETE und MOVE ohne `MOVE`): ordnerweites `EXPUNGE` wie upstream,
Warnung im Log. `LOGOUT` ohne `BYE` → Fehler. Ergebnis `ProcessedMail` `SUCCESS` bzw. `FAILED` mit
Fehlertext, danach Aufgabenfehler.
`error_callback`: `ProcessedMail` `FAILED` per `get_or_create` (idempotent; pl-queue ruft je
fehlgeschlagenem Kopf + einmal `ChordError`).

## Workflow-Effekte (`WorkflowEffects`)

- `email`: `EMAIL_ENABLED` aus → Log `Email backend has not been configured…`. Sonst Aktion laden
  (`documents_workflowactionemail`), Kontext (`build_workflow_action_context`: Dokument: Titel,
  `doc_url = PAPERLESS_URL + BASE_URL + documents/<id>/`, Korrespondent, Typ, Besitzer, `filename`
  = original_filename, `current_filename` = filename, `added` lokal, `created`, `id`, Custom Fields;
  Einlesen: Titel aus Overrides bzw. Pfad der Datei), Betreff/Text über
  `pl_templating::parse_w_workflow_placeholders`. Anhang bei `include_document`: Trigger
  Updated/Scheduled → `source_path` + MIME des Dokuments; sonst die Einlese-Datei (zum Aufrufzeitpunkt
  gesichert). Name `Path(current_filename).name`, doppelte Namen `_01`, `_02`. Empfänger
  `to.split(",")`. Versand per SMTP: `EMAIL_HOST/PORT`, `EMAIL_USE_SSL` (TLS direkt), `EMAIL_USE_TLS`
  (STARTTLS, Fähigkeit nötig), Anmeldung nur mit Benutzer **und** Passwort (PLAIN, sonst LOGIN),
  `MAIL FROM:<DEFAULT_FROM_EMAIL> [size=N]`, `RCPT TO` je Empfänger, `DATA` mit Punkt-Verdopplung,
  `QUIT`; Timeout 30 s. Jeder Fehler → Log `Error occurred sending notification email: …`.
- `webhook`: Aktion laden; `use_params` → je Param Vorlage rendern (Fehler → Log, Rest verworfen),
  sonst `body` rendern (Text); Header `str(k): str(v)`; `include_document` → Datei
  `(filename|Dateiname, Inhalt, MIME)`. Einreihen von `send_webhook` (ohne Warteschlange: direkt mit
  gleicher Neuversuch-Regel). `send_webhook`:
  `validate_outbound_http_url` (Schema ∈ `PAPERLESS_WEBHOOKS_ALLOWED_SCHEMES` [http, https], Host
  vorhanden, Port (Standard 443/80) ∈ `PAPERLESS_WEBHOOKS_ALLOWED_PORTS` falls gesetzt; sonst
  `ValueError` „Invalid URL scheme or hostname.“ / „Destination port not permitted.“, kein
  Neuversuch), dann Auflösen, bei `PAPERLESS_WEBHOOKS_ALLOW_INTERNAL_REQUESTS=false` jede
  nicht-öffentliche Adresse sperren (`Connection blocked: <host> resolves to a non-public address`,
  `ConnectError`, kein Neuversuch), Verbindung **genau zur geprüften IP** (Host/SNI = Hostname),
  `Host`-Header aus der Aktion verworfen, 5 s Timeout, keine Weiterleitungen, Körper wie httpx
  (`content` > `files` (multipart) > `data` (urlencoded) > `json` (kompakt, UTF-8)); Status außer 2xx
  → `HTTPStatusError` → Neuversuch.
- `password_removal`: nicht Teil dieses Moduls (pl-pdfops, W5); optionaler Rückruf, Standard Log.

## Konfiguration

`MailWorkerConfig::from_core`: `PAPERLESS_TIME_ZONE`, `PAPERLESS_SCRATCH_DIR`,
`PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS`, `PAPERLESS_EMAIL_CERTIFICATE_LOCATION`, OAuth-Client-Daten
(`MailConfig`, seit 2026-11-14 ohne `redis_url`/`redis_prefix`), `PAPERLESS_AUDIT_LOG_ENABLED`; IMAP-Timeout 60 s je Schritt. `EmailSettings::from_core`:
`PAPERLESS_EMAIL_HOST/PORT/HOST_USER/HOST_PASSWORD/FROM/USE_TLS/USE_SSL`. `WebhookSettings::from_env`:
`PAPERLESS_WEBHOOKS_ALLOWED_SCHEMES/ALLOWED_PORTS/ALLOW_INTERNAL_REQUESTS`. `EffectsConfig::from_core`:
`PAPERLESS_URL`, `BASE_URL`, `MEDIA_ROOT`, `SCRATCH_DIR`.

## Known differences

1. **Abrufsperre** per PostgreSQL-Advisory-Lock statt Django-Cache-Schlüssel (kein TTL nötig, löst sich
   beim Verbindungsabbruch). Nicht mit einem gleichzeitig laufenden paperless-ngx/Celery-Worker geteilt (Mischbetrieb
   gibt es seit 2026-11-14 nicht mehr; dasselbe gilt für die Warteschlange – deshalb dürfen die
   Mail-Aufgaben PaperRust-kwargs tragen, pl-mail.md Known difference 28).
2. **GPG-Entschlüsselung** (M1, 2026-11-18, `fetch/gpg.rs`): `PAPERRUST_/PAPERLESS_ENABLE_GPG_DECRYPTOR`
   und `PAPERRUST_/PAPERLESS_EMAIL_GNUPG_HOME` wie upstream; Nachrichten mit `multipart/encrypted`
   werden mit dem Programm `gpg` (`--batch --decrypt`, wie python-gnupg) entschlüsselt, fehlende
   Kopfzeilen der Originalnachricht werden angehängt (je Name das erste Vorkommen, wie
   `_build_decrypted_message`). Fehlschlag → `Decryption failed: <status>, <stderr>`, Mail wird nicht
   verarbeitet (Fehler-Log je Regel, wie upstream). Unterschiede: (a) ist die Entschlüsselung an, aber
   `gpg` fehlt oder `EMAIL_GNUPG_HOME` ist kein Ordner, lässt upstream den Vorverarbeiter still weg und
   liest verschlüsselte Mails als Geheimtext ein – hier wird eine verschlüsselte Mail mit klarer
   Fehlermeldung übersprungen (bleibt im Postfach, nächster Lauf versucht erneut); (b) Kopfzeilen
   werden als Rohzeilen übernommen statt von `email.policy.default` neu gefaltet; (c) ungültiger
   Schalterwert → Warnung und „aus“ statt Startabbruch; (d) Zeitlimit 120 s je `gpg`-Aufruf.
3. **`ProcessedMail.error`**: Python-Traceback (`traceback.format_exc()`) nicht nachbildbar; Text
   `Traceback (most recent call last):\n  File "paperrust", …\n<Typ>: <Meldung>\n`. Im Errback steht
   der Traceback der fehlgeschlagenen Kopfaufgabe (falls pl-queue einen liefert).
4. **Zeitlimit**: upstream unbegrenzt, hier 60 s je IMAP-Schritt (wie Verbindungstest).
5. **Apple-Farbmarken entfernen**: upstream iteriert ein Python-`set` (Reihenfolge je Prozess zufällig);
   hier sortiert `$MailFlagBit0 $MailFlagBit1 $MailFlagBit2`.
6. **Unterstützte MIME-Typen** wie der Rust-Consumer (`ParserRegistry` Standard: Office ohne Tika
   lesbar); upstream ohne Tika ignoriert Office-Anhänge.
7. **`message/delivery-status`** im `.eml`: Körper unverändert (zeilenweise) statt Neu-Erzeugung der
   Kopfblöcke.
8. **Workflow-Effekte beim Einlesen** (Consumer, `WorkflowEffects::email_in`/`webhook_in`): laufen mit
   der Verbindung der offenen Einlese-Transaktion; Kontext, Vorlagen, Anhang bzw. Webhook-Datei werden
   **sofort** gebaut wie upstream (Stand zum Zeitpunkt der Aktion). Abweichend nur: das SMTP-Senden
   und ein Webhook ohne Warteschlange laufen danach im Hintergrund (upstream synchron im Signal); wird
   die Transaktion zurückgerollt, ist die Mail wie upstream schon unterwegs. Der synchrone Weg
   (`email`/`webhook`, z. B. pl-documents-write „Dokument geändert“) wartet weiter im Hintergrund auf
   das Festschreiben (bis 120 s) und nimmt dann den festgeschriebenen Stand.
9. **Einlese-Trigger**: voller Kontext aus `ActionInput.overrides` (Titel `overrides.title or
   str(original_file)`, Korrespondent/Typ/Besitzer per `filter(pk=…).first()`, `created`, Custom
   Fields der Overrides, `doc_id = ""` über `WorkflowDocId::Empty`), `mime_type` des
   `ConsumableDocument`. „Dokument hinzugefügt“: Instanz im Speicher (`ActionInput.document`).
   `original_filename`/`filename`-Platzhalter: unter Windows wird `\` vor `Path.stem` zu `/`
   (wie `WindowsPath`). Test: `effects::tests::in_transaction_context_with_db` (`#[ignore]`,
   `PL_W9_TEST_DB=1`, DB-Kopie `pl_w9_test`, Transaktion wird zurückgerollt).
10. **E-Mail-Format**: MIME sinngleich, nicht byte-gleich mit Django (Boundary, Message-ID-Domäne,
    Kopf-Kodierung RFC 2047 base64, `Date` in UTC); `CRAM-MD5` nicht angeboten (kein MD5 im Workspace).
11. **Webhook-Ziele** werden mit dem WHATWG-URL-Parser (`url`) statt `urllib.parse` gelesen
    (Randfälle bei ungültigen URLs können anders abgelehnt werden); nur `http`/`https` sendbar.
    Datei in der Warteschlange base64-kodiert (`{"__type__": "bytes", "__value__": …}`).
12. **`fnmatch`** wie POSIX (`normcase` unverändert); die Referenz unter Windows ersetzt zusätzlich `/`.
13. **Mehrdeutige Korrespondenten** (`get` liefert mehrere): Fehler der Nachricht wie upstream
    (`MultipleObjectsReturned`), Text anders.
14. **Mail-Abruf-Review 2026-11-27**: kein ordnerweites `EXPUNGE` nach Markier-Aktionen, Prüfung von
    Konto/Ordner/UIDVALIDITY vor der verzögerten Aktion, ein Chord je Mail, Konto-Fehler im Ergebnis,
    geprüfter Gmail-Label-Status – Einzelheiten und Tests in `pl-mail.md`, Known differences 23–27
    (maßgeblich; die Abschnitte oben fassen sie nur zusammen). Neue kwargs ohne Rücksicht auf
    Python-Worker: Known difference 28.

## Paritätsstand / Tests

- `cargo test -p pl-mail -j 1` (ohne DB): 45 Unit-Tests (u. a. Kriterien wie imap_tools inkl.
  Gmail/Apple, `fnmatch`, UTF-7-Ordner, imaplib-Antwortparser/Literale, UID-Muster, Header-Faltung,
  ISO-Datum mit LMT-Offset, Zeichensätze, URL-Prüfung/SSRF, httpx-Körper, Dateinamen `_01`,
  Punkt-Verdopplung).
- `tests/worker_messages.rs` (ohne DB): 11 Testmails (`tests/parity/pl-mail-worker/mails`, erzeugt
  von `make_corpus.py`) gegen Referenzwerte aus **unverändertem** imap_tools/CPython
  (`upstream_messages.py` → `expected/`): subject, from_, from_values.name, date, Anhänge (Name,
  Disposition, Typ, SHA-256, libmagic-MIME) und die `.eml`-Bytes von `_process_eml` –
  **77 Vergleiche, 0 Abweichungen** (inkl. Faltung langer Kopfzeilen, 8-Bit-Kopfzeilen, LF-only,
  fehlende Schlussgrenze, multipart/signed, eingebettete message/rfc822, RFC 2047/2231).
- `tests/effects_local.rs`: SMTP-Senke (PLAIN/LOGIN, Empfänger, RFC-2231-Namen, Punkt-Verdopplung,
  doppelte Anhangnamen) und HTTP-Empfänger (JSON/Formular/multipart, Host-Header verworfen, keine
  Weiterleitung, 3 Neuversuche bei 500, SSRF-Sperre ohne Verbindungsaufbau, Port-/Schema-Liste) –
  5 Tests lokal; `workflow_effects_with_db` (`#[ignore]`, `PL_W6_TEST_DB=1`) mit den Seed-Aktionen
  (E-Mail-Aktion 3, Webhook-Aktion 4) auf Dokument 1 inkl. Platzhaltern und Warten auf Festschreiben.
- `tests/worker_parity.rs` (`#[ignore]`, `PL_W6_TEST_DB=1`, Referenz-PostgreSQL + reference/venv):
  zwei Fake-IMAP-Server (Standard mit `MOVE`, Gmail mit `X-GM-EXT-1`), 9 Regeln (Anhänge+MARK_READ
  mit Tags/Typ/Besitzer/Ausschluss, EML+FLAG mit Betreff-Filter, EVERYTHING+MOVE mit Einschluss und
  Inline-Teilen, DELETE mit Absender/Alter, deaktiviert, fehlender Ordner, TAG-Schlüsselwort mit
  stop_processing, Gmail-Label, Apple-Farbe auf Gmail → FAILED), je zwei Läufe.
  - `mail_worker_parity`: Rust (Chords sofort wie Celery eager) gegen upstream
    `process_mail_accounts` (eigene DB `pl_w6_test_up`, Overlay `pl_w6_settings`, `consume_file`
    aufzeichnend ersetzt, da die Referenz kein OCR hat): Ergebnistexte, eingereihte Dateien
    (Name, SHA-256, MIME, Overrides), ProcessedMail, angelegte Korrespondenten + Audit-Einträge,
    IMAP-Befehlsfolge des Abrufs, IMAP-Aktionen, Postfachzustand danach.
    Upstream läuft live (`run_upstream.sh`, keine Aufzeichnung). Die gewollten Abweichungen werden
    in `apply_known_differences` auf die upstream-Beobachtung angewandt und **gezählt**; die Zähler
    sind eine eigene Vergleichsgruppe (exakt): Known difference 23 – 6× `EXPUNGE` nach
    `UID STORE ±FLAGS` entfernt (das `EXPUNGE` nach `+FLAGS (\Deleted)` bleibt: der Fake-Server meldet
    kein `UIDPLUS` → Rückfall wie upstream); Known difference 25 – Regel 203 (EVERYTHING), Archiv
    UID 3: 1 doppelte `ProcessedMail`-SUCCESS-Zeile und 1 doppelte `UID MOVE 3 "Done"`-Verbindung
    entfernt, 0 zusätzliche `PROCESSED_WO_CONSUMPTION`. Known difference 24 wird auf der Rust-Seite
    geprüft (jede eingereihte `apply_mail_action` trägt `folder` und `account_id`); 26 und 27 zeigen
    sich im Szenario nicht (kein Konto scheitert ganz; Gmail-Label ohne Argument scheitert auf beiden
    Seiten), werden daher nicht umgeschrieben und müssen gleich sein. Jede andere Abweichung
    schlägt fehl. Stand 2026-11-28: **9 Vergleichsgruppen, 0 Abweichungen** (15 Dateien,
    27 ProcessedMail, 13 Aktionsverbindungen).
    Weitere Normalisierung: die Referenz unter Windows liefert für Pfade mit Umlauten statt des
    MIME-Typs eine libmagic-Fehlermeldung (Referenz-Artefakt).
  - `mail_worker_queue_e2e`: derselbe Ablauf über `PgQueue` + `Worker` (echte Chords, Errbacks,
    FAILED-Jobs) und die Abrufsperre (zweiter Lauf → „Skipped …“). Mit asynchronen Aktionen laufen
    Verschieben/Löschen erst nach dem Abruf (z. B. Löschungen im zweiten Lauf); geprüft werden nur
    Invarianten (fehlgeschlagene Aktionen = FAILED-Zeilen, Gmail-Label gesetzt, ≥ 3 Mails in
    `Done`, gelöschte Mail weg), kein Vergleich mit upstream.

## Upstream-Fixes nach v3.4.2 (paperless-ngx `dev`)

- **a1d76f3a1 (#14745) – `.eml`-Datei mit CRLF.** Quelle: `paperless_mail/mail.py::_process_eml`
  (`message.obj.as_bytes(policy=message.obj.policy.clone(linesep="\r\n"))`). Rust:
  `fetch::generator::eml_bytes` schreibt mit `as_bytes_linesep(.., b"\r\n")`: jede vom Generator
  erzeugte Zeilenendung (Kopfzeilen, gefaltete Kopfzeilen, Leerzeile, MIME-Grenzen, normalisierte
  Textzeilen) ist CRLF; unverändert kopierte Rohdaten (multipart ohne Teile, 8-Bit-Kopfwerte) bleiben
  wie upstream. `as_bytes` bleibt compat32 (`\n`). Grund upstream: RFC 5322; Outlook las sonst
  weiche quoted-printable-Umbrüche als `=`.
  Tests: `generator::tests::eml_bytes_use_crlf`, `write_lines_normalizes` (CRLF-Fall);
  `tests/worker_messages.rs` vergleicht weiter die LF-Fassung (`eml_bytes_linesep(.., b"\n")`) mit der
  v3.4.2-Referenz und prüft, dass die CRLF-Fassung genau dieselbe Nachricht mit CRLF ist.
