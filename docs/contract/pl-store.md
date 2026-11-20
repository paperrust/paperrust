# pl-store – eigenes Datenmodell (L6)

Der übrige Vertrag von pl-store (v2-Lesen/Schreiben, Identität) steht in
[pl-api-v2.md](pl-api-v2.md), Abschnitt „pl-store“. Hier: die Umbaustufen des Datenmodells
(docs/DATAMODEL-V2.md §6), Register `crates/pl-store/src/datamodel.rs`, SQL unter
`crates/pl-store/migrations/dm<n>_*.sql` (+ `.down.sql`).

## Grundsätze

- Jede Stufe legt `paperrust_*`-Tabellen an, kopiert die Daten **mit denselben ids** und stellt unter
  dem Django-Namen eine **Kompatibilitäts-Sicht** mit exakt den v3.4.2-Spalten bereit (Name, Typ,
  Länge, Reihenfolge; `paperrust manage datamodel check`). Lesender Code bleibt unverändert; über die
  Sicht kostet Lesen nichts, solange die Sicht nur 1:1-Tabellen per `LEFT JOIN` auf eindeutigen
  Schlüsseln verbindet (der Planer entfernt ungenutzte Joins).
- Schreiben über die Sicht: `INSTEAD OF`-Trigger je Zeile. Nicht möglich auf der Sicht:
  `INSERT … ON CONFLICT`, `SELECT … FOR UPDATE` (äußere Joins), `COPY … FROM`, `TRUNCATE` – solche
  Stellen schreiben/sperren die native Tabelle (`pl_store::datamodel::document_table`).
- Sync- (`paperrust_sync_*`) und Suchindex-Trigger (`paperrust_search_*`) und alle eingehenden
  Fremdschlüssel hängen an der nativen Tabelle. `paperrust_native_table(text)` bildet für die
  Erfassungs-Trigger Django-Namen auf native Tabellen ab.
- Die Django-Tabelle bleibt als **leere** Strukturvorlage in `ngx_legacy` (für den Rückbau); die
  Datensicherung ist der `pg_dump` vor der Stufe (`pl_cli::datamodel::backup_before`).
- Umstieg von paperless-ngx 3.0–3.4.2 unverändert: Assistent/Import laden das v3.4.2-Schema, danach
  baut `manage migrate` mit denselben Stufen um. Rückweg: `document_exporter` liest über die Sichten
  und schreibt das v3.4.2-Manifest (`tests/datamodel/roundtrip.sh`).

## Stufe 1 – Dokument schlank (`pl_store.dm1_paperrust_document`)

| Tabelle | Spalten (Reihenfolge) | Schlüssel/Indizes |
|---|---|---|
| `paperrust_document` | id, deleted_at, restored_at, transaction_id, title varchar(128), mime_type varchar(256), page_count, created, modified, added, archive_serial_number, correspondent_id, document_type_id, storage_path_id, owner_id, content_length (NOT NULL, gepflegt), root_document_id, version_label varchar(64), version_index | PK id (IDENTITY, Sequenzstand von `documents_document_id_seq` übernommen), UNIQUE asn, CHECKs wie v3.4.2; Teilindizes „lebend“ `(created\|added\|modified DESC, id DESC)`, `(owner_id, created)`, `(created)`, `(added)`, `(modified)`, `(title, id)`, FK-Spalten `WHERE … IS NOT NULL`, `(root_document_id, version_index)` UNIQUE, Trigramm `upper(title)` (wenn vorher vorhanden) |
| `paperrust_document_content` | document_id, content text | PK, FK → Dokument `ON DELETE CASCADE`, Trigramm `upper(content)` (wenn vorher vorhanden) |
| `paperrust_document_file` | document_id, checksum, archive_checksum, filename, archive_filename, original_filename (varchar wie v3.4.2) | PK, FK `ON DELETE CASCADE`, UNIQUE filename/archive_filename, `(checksum)`, `(archive_checksum) WHERE NOT NULL` |

Sicht `documents_document` = `paperrust_document d LEFT JOIN … content c LEFT JOIN … file f`, Spalten
wie v3.4.2; Vorgabe `id` = `nextval('paperrust_document_id_seq')`. Trigger:
- `INSTEAD OF INSERT`: id aus der Sequenz (falls leer), drei INSERTs, `content_length = length(content)`,
  `RETURNING` liefert die berechneten Werte.
- `INSTEAD OF UPDATE`: schreibt `paperrust_document` **immer** (Anker der Sync-Erfassung: eine
  Anweisung je Dokument = `rev + 1`, wie zuvor), Text/Datei nur bei Änderung; id-Änderung → Fehler
  `feature_not_supported`.
- `INSTEAD OF DELETE`: löscht `paperrust_document` (Text/Datei per Kaskade).
- `paperrust_document_content` `AFTER INSERT OR UPDATE OF content`: `content_length` nachführen
  (native Schreiber).

Geänderter Code: `pl_documents_write::actions::merge_as_versions` sperrt `paperrust_document`
(`FOR UPDATE`), `pl_cli::export::importer` schreibt in Sichten mit UPDATE + INSERT statt
`ON CONFLICT` und setzt die Sequenz über die Spaltenvorgabe der Sicht, `pl_auth_api::users::destroy`
behandelt Sichten wie Tabellen (Besitzer auf NULL beim Löschen eines Nutzers).

### Known differences (Stufe 1)

- **Reihenfolge bei Gleichstand:** Sortierungen, deren Schlüssel nicht eindeutig ist (z. B.
  `ordering=-archive_serial_number` mit vielen leeren ASN, `modified`, `owner`, `page_count`,
  `*__name`, `num_notes`, Tag-Kinder bei `ordering=document_count`), reihen gleichwertige Zeilen in
  Planreihenfolge. upstream (Django) legt dafür keine Reihenfolge fest (kein eindeutiger Zusatzschlüssel
  in v3.4.2); nach dem Umbau ist der Plan ein anderer (Join mit Text-/Dateitabelle), daher kann die
  Reihenfolge innerhalb gleicher Werte von Django auf derselben Datenbank abweichen. Datensätze,
  Schlüsselfolge und Mengen je Wert sind gleich (Paritätslauf 2026-11-13, DATAMODEL-V2 §6).

- Fehlertexte von Constraint-Verletzungen nennen die nativen Tabellen/Constraints
  (`paperrust_document_file_filename_key` statt `documents_document_filename_key`); Fehlerklasse
  (SQLSTATE) gleich. Kein API-Text gibt diese Namen weiter.
- `documents_document` ist keine Tabelle mehr: paperless-ngx (Django) kann dieselbe Datenbank nicht
  mehr nutzen (seit `pl_objects.0001_paperrust_tag_adjacency` ohnehin nicht). Fremdwerkzeuge, die
  `COPY documents_document FROM …` oder `TRUNCATE` nutzen, gehen nicht mehr; Export/Import über
  `document_exporter`/`document_importer`.
- `uuid`/`rev` stehen noch nicht in `paperrust_document` (Plan §1.1), sondern weiter in
  `paperrust_sync_object`; sie wandern mit Stufe 5 für alle Arten gemeinsam.
- `paperrust-migrate-db` als **Quelle** liest PaperRust-Datenbanken nach Stufe 1 nicht (es erwartet
  Django-Tabellen); PaperRust → PaperRust umziehen mit `pg_dump`/`pg_restore` oder Export/Import.

## Stufe 2 – Tags + Zähler (`pl_store.dm2_paperrust_tag_count`)

| Objekt | Inhalt |
|---|---|
| `paperrust_document_tag` | die frühere `documents_document_tags`, **umbenannt** (keine Kopie): Spalten id, document_id, tag_id; PK `(document_id, tag_id)` (aus dem bisherigen UNIQUE), Index `(tag_id, document_id)`; `id` ohne Index, Vorgabe aus der Identitätssequenz (`paperrust_document_tag_id_seq`); Fremdschlüssel wie bisher |
| Sicht `documents_document_tags` | `SELECT id, document_id, tag_id` – einfach, **automatisch beschreibbar** (`INSERT … ON CONFLICT`, UPDATE, DELETE gehen; die Trigger der Tabelle feuern) |
| `paperrust_label_count` | `(kind, label_id, owner_bucket) → n`; kind 2 Tag, 3 Korrespondent, 4 Dokumenttyp, 5 Speicherpfad; zählt nicht gelöschte Dokumente inkl. Versionen; `owner_bucket` = owner der Wurzel (bei Versionen) bzw. des Dokuments, 0 = ohne Besitzer |
| `paperrust_label_count_delta` | nur INSERT (Differenzen je Anweisung), keine heißen Zeilen |
| Sicht `paperrust_label_count_live` | Summe aus beiden je Schlüssel |
| Funktionen | `paperrust_label_count_rebuild()`, `_fold()` (Advisory-Sperre, wartet nie), `_install()`, `paperrust_doc_bucket(owner, root)` |
| Trigger | `paperrust_count_{ins,upd,del}` je Anweisung (Transition-Tables) auf `paperrust_document` und `paperrust_document_tag`, `paperrust_count_trunc` (TRUNCATE → Neuaufbau) |

Pflege: Dokument-Trigger ziehen den Beitrag der alten Zeilen ab und addieren den der neuen
(Korrespondent/Typ/Pfad und vorhandene Tag-Zuordnungen); wechselt der Besitzer einer Wurzel (oder
entsteht/verschwindet sie), wandern ihre Versionen den Eimer mit. Zuordnungs-Trigger zählen nur, wenn
das Dokument (noch) existiert – so stimmt die Summe in beiden Löschreihenfolgen (verzögerte
Fremdschlüssel). Falten: `pl_objects::counters::available` stößt höchstens alle 10 s im Hintergrund
`paperrust_label_count_fold()` an, wenn > 1000 Differenzen anstehen; von Hand
`paperrust manage datamodel counts fold`. Prüfen/Neuaufbau: `datamodel counts check|rebuild`.

Lesen (`pl_objects::counters::count_source`, eingebunden über
`perms::document_count_source_ext(…, counters)`): Superuser = Summe aller Eimer; aktive eingeschränkte
Nutzer = Eimer `{uid, 0}` + live gezählte freigegebene fremde Dokumente (Wurzeln und Versionen getrennt,
`granted_key_cond` auf die Wurzel-id); inaktive Nutzer und `last_correspondence` (Max über `created`)
weiter über den bisherigen Weg. Ohne Zähler (DB vor Stufe 2) bisheriger Weg. Gleichheit mit dem
bisherigen Weg: Test `crates/pl-cli/tests/datamodel_db.rs` (alle Referenznutzer × 4 Arten, nach
Umbau, nach Schreiben inkl. Besitzerwechsel mit Versionen, Papierkorb, neuer Freigabe, nach Falten).

### Known differences (Stufe 2)

- keine in der API. Ohne id-Liste filtert der bisherige Weg eingeschränkter Nutzer die Zählung vorab
  auf sichtbare Objekte; der Zählerweg zählt alle, die Liste zeigt weiterhin nur sichtbare – gleiche
  Antworten.
- `documents_document_tags.id` hat keinen eindeutigen Index mehr (kein Leser nutzt ihn; Export
  schreibt Tags als Liste im Dokument).

## Stufe 3 – Feldwerte typisiert (`pl_store.dm3_paperrust_field_value`)

`paperrust_field_value`: id (IDENTITY, UNIQUE; Sequenzstand übernommen), deleted_at, restored_at,
transaction_id, created, document_id, field_id, **t** (Werttyp der Zeile: 0 string, 1 url, 2 longtext,
3 select, 4 monetary, 5 boolean, 6 date, 7 integer, 8 float, 9 documentlink – aus der gesetzten
Spalte, bei leeren Werten aus `data_type` des Feldes), v_text (string/url/longtext/select/monetary-
Rohtext), v_bool, v_date, v_int (int4), v_float (float8), v_num (`numeric(65,2)` GENERATED aus dem
Monetary-Rohtext, gleicher Ausdruck wie upstream `value_monetary_amount`), v_refs (jsonb). PK
`(document_id, field_id)` (= upstream UNIQUE), Index `(field_id, document_id)`, Teilindizes
`(field_id, v_date|v_int|v_float|v_num, document_id) WHERE … IS NOT NULL`, GIN `v_refs jsonb_path_ops`.
Fremdschlüssel auf `paperrust_document` und `documents_customfield` (deferred).

Sicht `documents_customfieldinstance` (v3.4.2-Spalten): `value_bool/date/int/float/document_ids`
und `value_monetary_amount` direkt (Indizes bleiben für bestehende Abfragen nutzbar), Textspalten
über `t` (`CASE`, Typ/Länge wie v3.4.2). INSTEAD-OF-Trigger (INSERT … RETURNING liefert den berechneten
Betrag); mehr als eine gesetzte Wertspalte → `check_violation` (upstream setzt nie zwei; Umbau bricht
mit Meldung ab, falls Altdaten zwei hätten). Zähler: Art 6 = nicht gelöschte Feldwerte sichtbarer
Dokumente (`pl_fields_views::cf_counters`, eingebunden über `custom_fields::count_select_for`).
Die Zähler-Funktionen stehen in `crates/pl-store/migrations/dm_label_count_fns.sql` (eine Fassung,
von Stufe 2 und 3 eingebunden; Art 6 aktiv, sobald `paperrust_field_value` existiert).

### Known differences (Stufe 3)

- Schreiben mit `INSERT … ON CONFLICT` auf `documents_customfieldinstance` geht nicht mehr (kein
  Schreiber in PaperRust nutzt es; Import nutzt UPDATE + INSERT).
- Fehlertexte bei Verletzung der Eindeutigkeit nennen `paperrust_field_value_pkey`.
- `v_refs` bleibt jsonb (Plan: `integer[]` + Rückwärtstabelle `paperrust_field_docref`): verlustfrei
  wie upstream; Rückwärtssuche über den GIN-Index.

## Stufe 4 – Rechte (`pl_store.dm4_paperrust_object_grant`)

| Objekt | Inhalt |
|---|---|
| `paperrust_object_grant` | **Wahrheit**: (object_kind, object_id, subject, perms, ids); PK `(object_kind, object_id, subject)`, Index `(subject, object_kind, object_id) INCLUDE (perms)`; perms-Bits 1 view, 2 change, 4 delete, 8 add; `ids[1..4]` = guardian-Zeilen-ids je Bit |
| `paperrust_grant_raw` | guardian-Zeilen außerhalb des Rasters (fremdes Modell, Recht eines anderen Modells, nicht-kanonische object_pk), unverändert |
| Sicht `paperrust_grant_perm` | Recht → (ct, kind, bit, pos) aus `auth_permission`/`django_content_type` |
| Sichten `guardian_userobjectpermission`, `guardian_groupobjectpermission` | v3.4.2-Spalten, nach id geordnet; INSTEAD OF INSERT/UPDATE/DELETE; id-Vorgabe aus `paperrust_grant_user_id_seq`/`_group_id_seq` (Sequenzstände übernommen) |
| Sicht `paperrust_grant` | dieselbe Form wie die L5-Tabelle (perm_id, subject, object_id, ct) – Lesewege von pl-authz unverändert |

- Einfügen einer schon vorhandenen Freigabe über die guardian-Sicht fügt nichts ein (wie
  `bulk_create(ignore_conflicts=True)`/`ON CONFLICT DO NOTHING`, den Schreibweisen der Schreiber).
- `pl_authz::grants::status` meldet `native` (immer bereit, keine Pflegetrigger); `manage grants
  rebuild/enable` sind dann wirkungslos, `disable --purge` wird abgelehnt (Rückweg: `datamodel rollback --to 3`).
- Erfassung: Sync (`paperrust_sync_install_triggers`, Modus `grant`, Art = object_kind) und Suchindex
  (`paperrust_search_grant*`) hängen an `paperrust_object_grant`.
- Export: guardian-Datensätze ausdrücklich nach id (`pl_cli::export::exporter::for_each_record`).

### Known differences (Stufe 4)

- Doppelte Freigabe über die guardian-Sicht → still ignoriert statt `unique_violation`.
- Keine Fremdschlüssel von `paperrust_object_grant` auf Nutzer/Gruppen (das Vorzeichen trennt sie);
  Löschen eines Nutzers/einer Gruppe entfernt die Freigaben wie bisher über die guardian-Sicht
  (`pl_auth_api` Kaskade).
- `TRUNCATE` der guardian-Tabellen geht nicht mehr (Sicht).

## Stufe 5 – Labels + Sync (`pl_store.dm5_paperrust_label_sync`)

Davor im selben Stufentext: `crates/pl-sync/migrations/0003_paperrust_sync_native.sql` (auch eigene
Migration `pl_sync.0003_paperrust_sync_native`) – Erfassung, Sequenzer, Neuaufbau und Aufbewahrung,
die beide Zustände kennen (vor/ab Stufe 5).

| Objekt | Inhalt |
|---|---|
| `paperrust_tag`, `paperrust_correspondent`, `paperrust_document_type`, `paperrust_storage_path`, `paperrust_custom_field` | die umbenannten Django-Tabellen (Fremdschlüssel, Indizes, Trigger folgen; Sequenzen `paperrust_*_id_seq`) + `uuid uuid NOT NULL UNIQUE DEFAULT gen_random_uuid()`, `rev bigint NOT NULL DEFAULT 1` |
| Sichten `documents_tag`, `documents_correspondent`, `documents_documenttype`, `documents_storagepath`, `documents_customfield` | einfache Sichten mit exakt den bisherigen Spalten (Tag: v3.4.2 ohne die von `pl_objects.0001` entfernten `tn_*`-Spalten außer `tn_parent_id`); automatisch beschreibbar (auch `INSERT … ON CONFLICT`, `FOR UPDATE`); `id`-Vorgabe `nextval('paperrust_*_id_seq')` (Import setzt die Sequenz darüber) |
| `paperrust_document.uuid`, `.rev` | Identität in der Zeile (übernommen aus `paperrust_sync_object`, Versionen bekommen eine eigene uuid; gemeldet wird weiter das Wurzeldokument); fillfactor 85 |
| `paperrust_sync_ident` | (kind, local_id, uuid, rev) nur für Arten 7–9 (Ansichten, Nutzer, Gruppen – Tabellen bleiben Django) |
| `paperrust_sync_tombstone` | (kind, local_id, uuid, rev, deleted_at) gelöschter Objekte; Index auf uuid |
| Sicht `paperrust_sync_object` | (kind, local_id **integer**, uuid, rev, deleted_at) über alle obigen Quellen – nur lesen; Leser casten `local_id::int8`; Papierkorb-Dokumente mit `deleted_at` = Zeitpunkt des Papierkorbs |

- Erfassung ab Stufe 5 (`paperrust_sync_install_triggers`): `rev + 1` je geänderter Zeile über den
  BEFORE-UPDATE-Trigger `paperrust_sync_rev` an den sechs nativen Tabellen; abhängige Tabellen
  (Tag-Zuordnung, Notiz, Feldwert, Filterregel, Freigabe) und Versionen erhöhen `rev` des Objekts
  über `paperrust_sync_bump(kind, ids)`; Löschen schreibt den Grabstein mit `rev + 1`, Wiederanlegen
  derselben id entfernt ihn. Ohne Sync-Erfassung (`sync disable`) bleibt `rev` stehen – wie vorher.
- Identität fehlender Objekte anlegen: `paperrust_sync_ensure(kind, ids)` (ab Stufe 5 nur Arten 7–9);
  `paperrust_sync_ensure_needed(kind)` als Torbedingung vor der teuren Unterabfrage in `sync/ids`.
- Sequenzer: je Objekt per Primärschlüssel auf der nativen Tabelle bzw. `paperrust_sync_ident`, sonst
  Grabstein → `'d'`; `paperrust_sync_live` entfällt. Aufbewahrung löscht mit dem Feed-Eintrag den
  Grabstein.
- `sync disable --purge` lässt Sicht, `paperrust_sync_ident` und `paperrust_sync_tombstone` stehen
  (gehören zum Datenmodell); `sync enable` spielt alle Sync-Migrationen (0001–0003) ein.

### Known differences (Stufe 5)

- `paperrust_sync_object` ist nicht mehr beschreibbar und hat keine Spalte `seq` mehr (Feed-Position
  steht in `paperrust_sync_log`). `deleted_at` eines Papierkorb-Dokuments ist der Papierkorb-Zeitpunkt
  (vorher: Zeitpunkt, zu dem der Sequenzer es bemerkte).
- `paperrust_note` (Plan §1.6) nicht angelegt: Notizen sind keine eigene Sync-Art, gezählt werden sie
  über den Dokument-`rev`; der Notiz-Zähler gehört zu Stufe 6.
- `TRUNCATE`/`COPY … FROM` auf die fünf Label-Namen geht nicht mehr (Sicht) – auf die nativen Tabellen.
