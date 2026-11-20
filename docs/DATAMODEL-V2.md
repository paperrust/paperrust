# PaperRust – eigenes Datenmodell (Altlast L6), Umsetzungsplan

Stand 2026-11-10 (Plan), Umsetzung ab 2026-11-13: siehe §6. Rahmen: docs/ARCHITECTURE-V2.md §5 (L6), Messbasis:
docs/PERF-SCALE.md (1 Mio. Dokumente, `pl_scale_1m`), Sync: docs/SYNC.md, Rechte: L5
(`crates/pl-authz/migrations/0002_paperrust_grant.sql`), Umstieg: `crates/pl-dbmigrate`, `pl-cli export`.

## Kurzfassung (einfach gesagt)

- **Was es bringt:** Heute liegt in der Hauptliste jedes Dokument mitsamt seinem ganzen Text. Das
  macht jede Liste schwer (1 GB statt ca. 0,25 GB). Neu: Text in eine eigene Tabelle, fertige Zähler
  statt jedes Mal neu zählen, Feldwerte sauber nach Typ. Erwartung: Tag- und Feldlisten, Statistik und
  Sortieren nach eigenem Feld von **Sekunden auf unter 0,1 s**, alle anderen Listen deutlich leichter.
- **Was es kostet:** Etwa 6–9 Wochen Arbeit in 5 Stufen. Jede Stufe läuft für sich; die alte
  Tabellenform bleibt dabei als „Ersatzansicht“ stehen, damit nichts auf einmal kaputtgeht.
- **Umstieg von paperless-ngx** bleibt ein Knopfdruck (gleicher Assistent, danach automatische
  Umwandlung). **Rückweg** nur noch als Export im paperless-ngx-Format, nicht mehr „gleiche Datenbank“.
- **Empfehlung:** Machen, aber erst wenn die laufenden Arbeiten (Tempo, Massenänderungen,
  Migration) fertig sind und der Export-Rückweg geprüft ist. Mit Stufe 1 (Dokument schlank) beginnen.

## 0. Ausgangslage und Grundentscheidungen

**Neu seit 2026-11-10:** paperless-ngx muss nicht mehr auf derselben Datenbank laufen. Damit fällt
die Regel „Schema expand-only, Django-Tabellen gehören upstream“ (ARCHITECTURE-V2 §4.1) **für das
Datenmodell** weg. Bleiben muss: Umstieg von paperless-ngx sicher (Migrationsassistent) und ein Weg
zurück (Export im paperless-ngx-Format).

Gemessen (lesend, `pl_scale_1m`, 2026-11-10, diese Sitzung; Ergänzung zu PERF-SCALE E1–E8):

| # | Messung | Ergebnis |
|---|---|---|
| M1 | Größe `documents_document` | Heap **1 039 MB**, gesamt 2 811 MB, 24 Indizes |
| M2 | Ø Zeilengröße mit / ohne `content` (Stichprobe 0,5 %) | **2 340 B / 227 B** → schmaler Heap ≈ 0,25 GB (−75 %) |
| M3 | `COUNT(*)` lebende Wurzeldokumente (ohne Q1-Index auf dieser DB) | 1 057 ms, Parallel Seq Scan über 132 928 Blöcke (= ganzer Heap) |
| M4 | Sortierung Custom Field (Datum) als `LEFT JOIN` + `ORDER BY value_date DESC NULLS LAST LIMIT 25` | **4 077 ms**: Seq Scan über den Dokument-Heap; vorhandener Index `(field_id, value_date)` passt nicht zu `DESC NULLS LAST` |
| M5 | dieselbe Sortierung von der Feldseite aus (Join zum Dokument per PK) | 401 ms: Bitmap-Scan über den breiten Feldwert-Heap (233 MB, 13 Wertspalten) |
| M6 | `SELECT tag_id, count(*) FROM documents_document_tags GROUP BY tag_id` (ohne Rechte, ohne Papierkorb) | 1 153 ms über 5,05 Mio. Zeilen – Untergrenze jeder Zählung „live“ |
| M7 | Indizes, die nur Django-Artefakte sind (`*_like`, `varchar_pattern_ops`) | 5 Stück, ≈ 375 MB auf `documents_document` |

**Kein Gratis-Trick über TOAST:** `ALTER TABLE … SET (toast_tuple_target = 128)` hilft nach dem
PostgreSQL-Quellcode nur Zeilen, die ohnehin über ≈ 2 KB liegen (das Auslagern startet erst ab der
festen Schwelle `TOAST_TUPLE_THRESHOLD`); Inhalte von 0,5–2 KB bleiben im Heap. Auf einer Kopie
nachmessen, aber nicht als Ersatz für das Auslagern einplanen.

Grundentscheidungen:

1. **Ganzzahl-`id` bleibt Primärschlüssel**, `uuid` + `rev` werden echte Spalten (UNIQUE). Abweichung
   von ARCHITECTURE-V2 §5 („UUID-PK“): v1-API, Angular, Audit-Log, Freigaben, Export und alle
   Fremdschlüssel arbeiten mit Ganzzahlen; 4-Byte-Schlüssel halten Zwischentabellen und Indizes klein.
   Sync/v2 nutzen `uuid`, wie heute.
2. **Gleiche ids beim Umbau.** Jede neue Tabelle übernimmt die ids der Django-Zeile. Damit bleiben
   Links, Audit-Log (`auditlog_logentry.object_pk`), Suchindex, Dateinamen, Sync-Clients gültig.
3. **Übergang über Kompatibilitäts-Sichten** (Abschnitt 2): Nach jeder Stufe gibt es unter dem alten
   Django-Namen eine Sicht mit **exakt** den alten Spalten. Alter Code läuft weiter; Crates ziehen
   einzeln auf die neuen Tabellen um. Harter Schnitt nur am Ende, je Crate.
4. **Django-Schema = Importformat.** Der Migrationsassistent lädt paperless-ngx-Daten weiter in das
   v3.4.2-Schema (vorhandener, geprüfter Code), danach wandeln dieselben Migrationen um, die auch
   bestehende PaperRust-Installationen umstellen. Ein Weg, ein Test.
5. **Namen:** Präfix `paperrust_` (AGENTS.md). Auth-Tabellen (`auth_user`, `auth_group`,
   `django_session`, `authtoken_token`, `auth_permission`) bleiben vorerst unverändert (Anmeldung,
   Sitzungs-Cookie, Angular), ebenso Mail, Workflows, Saved Views, Audit-Log – geringer Leidensdruck.

## 1. Zieltabellen

Spalten in Reihenfolge; `→` = Fremdschlüssel. Indizes ohne PK/UNIQUE separat. „Ersetzt“ = welche
Django-Tabelle als Sicht daraus entsteht.

### 1.1 `paperrust_document` – schmal (ersetzt `documents_document` mit 1.2 und 1.3)

| Spalte | Typ | Bemerkung |
|---|---|---|
| `id` | `integer` PK | gleiche id wie Django |
| `uuid` | `uuid NOT NULL UNIQUE` | aus `paperrust_sync_object` übernommen |
| `rev` | `bigint NOT NULL DEFAULT 1` | +1 je Änderung (Trigger, wie heute je Anweisung) |
| `title` | `text NOT NULL` + `CHECK (length(title) <= 128)` | |
| `created` | `date NOT NULL` | |
| `added`, `modified` | `timestamptz NOT NULL` | |
| `correspondent_id`, `document_type_id`, `storage_path_id` | `integer NULL` → Labels | |
| `owner_id` | `integer NULL` → `auth_user` | |
| `archive_serial_number` | `integer NULL UNIQUE` | |
| `mime_type` | `text NOT NULL` | |
| `page_count`, `content_length` | `integer NULL` / `integer NOT NULL DEFAULT 0` | |
| `note_count` | `smallint NOT NULL DEFAULT 0` | **Zähler**, Trigger auf 1.6 |
| `tag_count` | `smallint NOT NULL DEFAULT 0` | **Zähler**, Trigger auf 1.4 |
| `root_document_id`, `version_index`, `version_label` | wie v3.4.2 | Versionen |
| `deleted_at`, `restored_at`, `transaction_id` | wie django-softdelete | Papierkorb |

Indizes (alle auf der schmalen Tabelle, daher klein):
- `paperrust_doc_live_created (created DESC, id DESC) INCLUDE (owner_id) WHERE root_document_id IS NULL AND deleted_at IS NULL` (Q1, schon vorhanden – wandert mit)
- `(added DESC, id DESC)`, `(modified DESC, id DESC)` je mit derselben `WHERE`-Bedingung
- `(correspondent_id)`, `(document_type_id)`, `(storage_path_id)`, `(owner_id)` – je Teilindex `WHERE … IS NOT NULL`
- `(title, id)` für die Titelsortierung, `(note_count DESC, id DESC)` für „Notizanzahl“
- **entfallen:** alle fünf `*_like`-Indizes (M7), `checksum`-/`filename`-Indizes (wandern nach 1.3)

Fällt weg: **#4** (Liste/COUNT/Seite): was nicht Index-Only läuft, liest ≈ ¼ der Blöcke (M2/M3:
1 057 ms → ≈ 0,25–0,3 s erwartet ohne Q1, Titelsortierung 1,1 s → ≈ 0,3 s); **#7** Notizanzahl
2,2 s → < 50 ms (Index auf Zähler); **#6** Summe `content_length` ohne Text im Heap; Seq Scans
(z. B. M4) werden 4× billiger, auch dort, wo noch kein passender Index existiert.

### 1.2 `paperrust_document_content` – Inhalt ausgelagert

| Spalte | Typ |
|---|---|
| `document_id` | `integer` PK → `paperrust_document ON DELETE CASCADE` |
| `content` | `text NOT NULL` (Speicher `EXTENDED`, komprimiert) |

Indizes: Trigramm-GIN `paperrust_doc_content_trgm` auf `upper(content)` (heute
`crates/pl-store/migrations/0002_paperrust_doc_trgm.sql` auf der Dokumenttabelle – zieht mit um).
Gewinn: indirekt (1.1). `title_content` (#2) bleibt Sache von Trigramm/tantivy (L3), wird hier nicht
schneller, aber auch nicht langsamer.

### 1.3 `paperrust_document_file` – Dateien (1:1)

`document_id` PK → Dokument, `filename` UNIQUE, `archive_filename` UNIQUE, `original_filename`,
`checksum`, `archive_checksum`, `storage_type` (falls später nötig). Indizes: `(checksum)`,
`(archive_checksum)` (Duplikatprüfung beim Einlesen). Nur Einlesen, Download, Umbenennen,
Integritätsprüfung lesen hier. Gewinn: ≈ 250 MB Indizes und ≈ 110 B je Zeile weg aus 1.1.

### 1.4 `paperrust_document_tag` – Zuordnung ohne Ersatzschlüssel (ersetzt `documents_document_tags`)

`document_id integer`, `tag_id integer`, **PK `(document_id, tag_id)`**, Index `(tag_id, document_id)`.
Kein `id` (wird von keinem Export/Audit referenziert; Django-M2M exportiert als Liste im Dokument).
Beide Richtungen laufen Index-Only (Filter „Tag any/all/none“, `selection_data` über die Auswahl).
Gewinn: −1 Spalte, −1 Index (≈ 110 MB), Filter Tag all/none (1,6–1,7 s) profitieren von Index-Only.

### 1.5 `paperrust_label_count` – Zähler je Label (neu)

| Spalte | Typ | Bemerkung |
|---|---|---|
| `kind` | `smallint` | 2 tag, 3 correspondent, 4 document_type, 5 storage_path, 6 custom_field (Arten wie SYNC.md) |
| `label_id` | `integer` | |
| `owner_bucket` | `integer` | `owner_id` des Dokuments, `0` = ohne Besitzer |
| `n` | `integer NOT NULL` | lebende Wurzeldokumente |

PK `(kind, label_id, owner_bucket)`. Gepflegt von Statement-Triggern auf 1.1/1.4/1.7 (Differenzen
aus Transition-Tables, aggregiert je Anweisung – kein Zeilen-Trigger). Gegen heiße Zeilen (Inbox-Tag
bei parallelen Schreibern): Deltas zuerst in `paperrust_label_count_delta` (nur INSERT), ein
Worker-Lauf faltet sie zusammen; Lesen = Summe aus beiden.
Dazu `paperrust_doc_totals (owner_bucket PK, docs, chars, inbox)` für die Statistik.

Abfrage: Superuser = Summe über alle Buckets. Eingeschränkter Nutzer = Buckets `{uid, 0}` + Zählung
nur über die **freigegebenen fremden** Dokumente (Grant-Join, typisch wenige 10 000 statt 1 Mio.).
Fällt weg: **#3** Label-Listen 10–14 s (nach Q2 2,5 s; Untergrenze live M6 1,15 s) → **< 5 ms**
admin, **< 100 ms** eingeschränkt (Schätzung); **#6** Statistik 9,5 s → < 50 ms.
Paritätsregel: Zahl muss gleich `document_count` von upstream sein (Wurzeldokumente, Papierkorb
ausgeschlossen, Sichtbarkeit wie `get_objects_for_user_owner_aware`) – Differenztest wie bei L5.

### 1.6 `paperrust_note` (ersetzt `documents_note`)

Wie heute (`id`, `document_id`, `user_id`, `note`, `created`, `deleted_at`…), plus `uuid`. Index
`(document_id) WHERE deleted_at IS NULL`. Hauptänderung ist der Zähler `note_count` in 1.1.
`id` bleibt (Audit-Log registriert `Note`, upstream `documents/models.py: auditlog.register(Note)`).

### 1.7 `paperrust_field_value` – typisierte Custom-Field-Werte (ersetzt `documents_customfieldinstance`)

| Spalte | Typ | Bemerkung |
|---|---|---|
| `id` | `integer UNIQUE` | bleibt: Audit-Log registriert `CustomFieldInstance`, Export hat `pk` |
| `document_id` | `integer` → Dokument | |
| `field_id` | `integer` → `paperrust_custom_field` | |
| `v_int` | `bigint` | integer |
| `v_num` | `numeric(20,4)` | float, monetary-Betrag |
| `v_date` | `date` | date |
| `v_bool` | `boolean` | boolean |
| `v_text` | `text` | string, url, longtext, select (Options-id), **monetary-Rohtext** |
| `v_currency` | `char(3)` | monetary (aus Rohtext) |
| `v_refs` | `integer[]` | documentlink |
| `created` | `timestamptz` | |

PK `(document_id, field_id)`. **Genau eine** `v_*`-Spalte je Typ befüllt (`CHECK` über den Typ des
Feldes ist nicht möglich ohne Join → Prüfung im Schreibcode + Konsistenztest). Monetary bleibt
zusätzlich als Rohtext erhalten, damit Sicht und Export Zeichen für Zeichen gleich sind
(upstream speichert `"EUR12.50"`/`"12.50"`). Rückwärtssuche für Dokumentlinks:
`paperrust_field_docref (target_id, field_id, document_id)`, PK in dieser Reihenfolge.

Indizes – **pro Typ als Teilindex in Sortierrichtung der UI**, damit Sortieren und Bereichsfilter
Index-Only laufen:
- `(field_id, v_date DESC NULLS LAST, document_id) WHERE v_date IS NOT NULL` – analog `v_int`, `v_num`
- `(field_id, upper(v_text), document_id) WHERE v_text IS NOT NULL` (string/select/url)
- `(field_id, document_id)` für EXISTS/`has field`

Fällt weg: **#7** Custom-Field-Sortierung 9 s (M4: 4,1 s / M5: 0,4 s in günstigster Form) →
**< 50 ms** (oberste 25 aus dem Index, dann 25 PK-Zugriffe); `custom_field_query` Bereich/exact
(0,6–2 s) → Indexzugriff; Custom-Field-Liste mit Zählung (#3, 4–11 s) über 1.5.
Speicher: 13 Wertspalten + 7 Django-Indizes (697 MB) → ≈ 8 schmale Spalten, Teilindizes.

### 1.8 `paperrust_grant` – Freigaben als Wahrheit (ersetzt guardian-Tabellen)

Heute (L5) ist `paperrust_grant (perm_id, subject, object_id, ct)` eine von Triggern gepflegte
**Ableitung**. Neu wird sie die Wahrheit, mit eigener, verlustfreier Form:

| Spalte | Typ | Bemerkung |
|---|---|---|
| `object_kind` | `smallint` | Arten wie SYNC.md |
| `object_id` | `integer` | |
| `subject` | `integer` | > 0 Nutzer, < 0 Gruppe (wie L5) |
| `perms` | `smallint` | Bitmaske `1 view, 2 change, 4 delete, 8 add` (= guardian-Codenamen je Modell) |

PK `(object_kind, object_id, subject)`, Index `(subject, object_kind, object_id)`. Sonstige
guardian-Zeilen, die nicht in dieses Raster passen (fremde Modelle, nicht-kanonische `object_pk`),
landen unverändert in `paperrust_grant_raw` (nur für Export, Rückweg verlustfrei).
Sichtbarkeit Dokument: `owner_id = $u OR owner_id IS NULL OR EXISTS (SELECT 1 FROM paperrust_grant g
WHERE g.object_kind = 1 AND g.object_id = d.id AND g.subject = ANY($subjects) AND g.perms & 1 <> 0)`.
Fällt weg: **#9** Rest nach L5 (L5 brachte schon COUNT −35 %, Grantees je Seite −99 %): kein Join
auf `auth_permission`/`django_content_type` mehr je Abfrage, eine Zeile je (Objekt, Subjekt) statt je
Recht (≈ −40 % Zeilen), keine Pflege-Trigger. Erwartung zusätzlich −20–40 % bei eingeschränkten
Filtern; der große Gewinn für eingeschränkte Nutzer kommt mit 1.5 (Zählungen).

### 1.9 Labels mit `uuid`/`rev` (ersetzt `documents_tag`, `_correspondent`, `_documenttype`, `_storagepath`, `_customfield`)

`paperrust_tag`, `paperrust_correspondent`, `paperrust_document_type`, `paperrust_storage_path`,
`paperrust_custom_field`: Spalten wie upstream v3.4.2 (inkl. Matching, Tag-Baum `tn_parent_id`/
`tn_ancestors_pks`…), zusätzlich `uuid`, `rev`. Kein Leistungsgewinn, aber `paperrust_sync_object`
wird damit für alle Arten überflüssig.

**Stand 2026-11-13 – vorgezogener Teilschritt umgesetzt: Tag-Baum als Adjazenzliste** (Idee von upstream
8aa99a381/#14612, eigener Code; Vertrag docs/contract/pl-objects.md „Tag-Baum“). Migration
`pl_objects.0001_paperrust_tag_adjacency` entfernt die 13 abgeleiteten django-treenode-Spalten aus
`documents_tag`; Wahrheit ist nur `tn_parent_id`. Noch ohne Umbenennung nach `paperrust_tag` (das bleibt
Stufe 5); Spaltenname wie upstream nach #14612, damit neuere paperless-ngx-Exporte direkt passen. Umstieg
3.0–3.4.2 unverändert (erst v3.4.2-Schema, dann diese Migration); Export berechnet die Felder für 3.4.2.

Messung (`pl_scale_1m`, 800 Tags, 4 Ebenen, nur lesend; Schreibkosten auf einer Wegwerf-Kopie der Tag-Tabelle):

| Vorgang | vorher (treenode) | nachher (Adjazenzliste) |
|---|---|---|
| Tags lesen (alle Spalten / nur Kerne) | 1,4 ms | 1,6 ms (Rauschen; Tabelle 400 kB) |
| Hierarchie laden (`id, tn_parent_id`) | – (Spalten gelesen) | 1,0 ms |
| Nachfahren der größten Wurzel / Vorfahren eines Blatts | Textspalte lesen | 1,7 ms / 0,3 ms (rekursives SQL) |
| Tag-Seite, Tag-Filter der Dokumentliste | 10–14 s bzw. 0,4–3,9 s (PERF-SCALE #3, Filter) | **unverändert** – Kosten liegen in `document_count` bzw. der Dokumentabfrage, nicht im Baum |
| Neuer Wurzel-Tag | 475–801 von 801 Tag-Zeilen neu geschrieben | 1 Zeile |
| Neues Kind / Blatt löschen / Teilbaum verschieben | 117 / 107 / 137 Zeilen | 1 / 1 / 1 Zeile |
| Tag umbenennen | 8 Zeilen | 1 Zeile |

Jede neu geschriebene Zeile war eine eigene `UPDATE`-Anweisung (800 Anweisungen ≈ 0,12–0,15 s ohne Trigger,
gemessen) **und** ein Sync-Ereignis (`rev`+1, Feed-Eintrag) – ein einziger neuer Tag ließ Sync-Clients bis zu
alle Tags neu laden. Das entfällt. Für Lesen, Tag-Seite und Filter bringt der Schritt nichts Messbares; er ist
Aufräumen und Schreib-/Sync-Entlastung, kein Lese-Tempo.

### 1.10 Sync ohne Seitentabelle

`paperrust_sync_object` (301 MB) entfällt nach 1.1/1.6/1.9: `uuid`/`rev` stehen in der Zeile,
Tombstones wandern in `paperrust_sync_tombstone (kind, uuid, local_id, seq, deleted_at)`.
`paperrust_sync_log`/`_pending`/`_meta` bleiben. Der Sequenzer prüft die Existenz per PK auf der
nativen Tabelle (keine `UNION`-Sicht `paperrust_sync_live` mehr).
Fällt weg: **#8** `sync/ids` 13–25 s → Index-Only über `(id) INCLUDE (uuid)`, kein `INSERT … ON
CONFLICT` je Aufruf (≈ 1,5 s für 1 Mio. inkl. JSON, mit Paginierung ≪ 1 s); **#10d** Sequenzer-
Existenzprüfung ohne Seq Scan über 931 MB (sofern „Sync-Sequenzer v2“ aus fe4a63f das nicht schon
löst – vor Stufe 5 nachmessen).

### 1.11 Was das Modell nicht löst

- **#1** `selection_data`: lösen Q3/L1 (Aggregat über die Auswahl). 1.4 macht es nur Index-Only.
- **#2** `title_content`, **#5** Volltext: L3 (Trigramm/tantivy).
- **#10a–c** Massenänderungen 122–260 ms/Dokument: Worker-Bündelung/L2 (Sitzung „Massenänderungen“).
- Klassifikator (L4).

### 1.12 Größe (Schätzung für 1 Mio. Dokumente)

| Teil | heute | neu |
|---|---:|---:|
| Dokument-Heap | 1 039 MB | ≈ 260 MB (+ Inhalt ≈ Text komprimiert in 1.2) |
| Dokument-Indizes | ≈ 800 MB | ≈ 250 MB (1.1) + ≈ 250 MB (1.3) |
| Tag-Zuordnung | 521 MB | ≈ 300 MB |
| Feldwerte | 697 MB | ≈ 350 MB |
| `paperrust_sync_object` | 301 MB | 0 |

## 2. Übergangsweg

### 2.1 Sichten statt hartem Schnitt – warum

Heute greifen **25 Crates** direkt auf `documents_document` zu (`grep`, Spitze: pl-documents-write 23
Dateien, pl-docfilter 11, pl-consumer 11, pl-cli 10). Ein harter Schnitt hieße: alles auf einmal
umschreiben, wochenlang nichts lauffähig, Paritätslauf erst am Ende. Darum:

1. Je Stufe: neue Tabellen anlegen, Daten in **einer Transaktion** kopieren (`INSERT … SELECT`, ids
   gleich), Django-Tabelle nach Schema `ngx_legacy` verschieben (Sicherung bis Stufe 6),
   **Sicht mit altem Namen und exakt den alten Spalten/Typen** in `public` anlegen.
2. Lesen über die Sicht kostet nichts Zusätzliches: Die Sicht verbindet 1:1-Tabellen per
   `LEFT JOIN` auf eindeutigen Schlüsseln, PostgreSQL entfernt ungenutzte Joins (Join Removal). Wer
   `content` nicht liest, liest 1.2 nicht.
3. Schreiben über die Sicht: `INSTEAD OF`-Trigger je Zeile verteilen auf die neuen Tabellen;
   `INSERT … RETURNING id` funktioniert. Sync-, Grant- und Zähler-Trigger hängen an den **neuen**
   Tabellen.
4. Fremdschlüssel anderer Django-Tabellen (z. B. Notizen, Freigabelinks, Workflow-Läufe) werden
   generisch per `pg_constraint` auf die neue Tabelle umgehängt (gleiche ids → gültig).
5. Danach ziehen Crates **einzeln** von der Sicht auf die neuen Tabellen (über `pl-store`-Funktionen).
   Wenn kein Code mehr den alten Namen nutzt (CI-Prüfung per `grep`), fällt die Sicht.

Bekannte Stolpersteine der Sicht (vorher prüfen, Fundstellen heute):
- `INSERT … ON CONFLICT` geht nicht auf Sichten mit `INSTEAD OF`-Trigger → 1 Stelle
  (`crates/pl-documents-write/src/docs.rs`), vorher auf die neue Tabelle umstellen.
- `SELECT … FOR UPDATE` über eine Sicht mit Outer Join scheitert → `pl-documents-write/src/actions.rs`
  prüfen (übrige Fundstellen betreffen Einstellungen/Accounts, nicht das Datenmodell).
- `COPY` in Sichten geht nicht → betrifft nur `pl-dbmigrate` (schreibt künftig in das Importschema,
  Abschnitt 3) und `pl-mail` (kein Datenmodell).
- Trigger je Zeile bei Massen-Updates über die Sicht: `bulk_edit` 10 000 Dokumente → messen; Ziel
  < 10 % Mehrkosten, sonst den Schreiber zuerst umstellen.

### 2.2 Reihenfolge

| Stufe | Inhalt | Was danach lauffähig ist | Aufwand |
|---|---|---|---|
| **0 Vorbereitung** | Migrationsrahmen für nicht-expand-only-Schritte (Version, Sicherung vorher, Rückbau-Skript je Stufe), Paritätslauf + Messlauf auf Kopie von `pl_scale_1m`, Export-Rundreise-Test (3.3) grün, CI-Prüfung „keine neuen Zugriffe auf Django-Namen“ | alles wie heute | M |
| **1 Dokument schlank** | 1.1 + 1.2 + 1.3, `uuid`/`rev` in die Zeile, Sicht `documents_document`, Q1-/Trigramm-Indizes wandern mit; danach `pl_store::docsql` (zentraler SQL-Bauer der Liste, L1) und Serialisierung (`pl-documents`) auf native Tabellen | alle Crates über die Sicht; Liste/Filter nativ | **L** |
| **2 Tags + Zähler** | 1.4 + 1.5 + `paperrust_doc_totals` + `note_count`/`tag_count`, Sicht `documents_document_tags`; `pl-objects` (Label-Listen), `pl-system` (Statistik) lesen Zähler | Zählungen nativ, Schreiber über Sicht | M |
| **3 Feldwerte typisiert** | 1.7 + `paperrust_field_docref`, Sicht `documents_customfieldinstance` (baut `value_*` zurück, `value_monetary` aus Rohtext); `docsql` Custom-Field-Filter/-Sortierung nativ | Felder nativ lesen, Schreiber über Sicht | L |
| **4 Rechte** | 1.8 als Wahrheit, guardian-Tabellen werden Sichten (Bitmaske → `permission_id`-Zeilen per Join auf `auth_permission`), L5-Pflegetrigger entfallen; `pl-authz` liest nativ | Rechte nativ; `set_permissions` schreibt über Sicht, dann nativ | M |
| **5 Labels + Sync** | 1.9 + 1.10, `paperrust_sync_object` auflösen, Sequenzer per PK | Sync nativ | M |
| **6 Schreiber umstellen** | je Crate: `pl-documents-write`, `pl-consumer`, `pl-workflows`, `pl-mail`, `pl-ai`, `pl-fields-views`, `pl-search`/`pl-indexer`, `pl-cli` (Export/Import), `pl-dbmigrate`-Prüfung; je Crate Paritätslauf | je Crate einzeln | je S–M, gesamt L |
| **7 Aufräumen** | Sichten löschen, `ngx_legacy` löschen (nach ≥ 1 Release), alte Legacy-Wege (`PAPERRUST_PERMISSION_ENGINE=legacy`, `QUERY_ENGINE=legacy`) entfernen, Altcode nach `pl-compat-…` | nur noch natives Modell | S |

Regel je Stufe (ersetzt ARCHITECTURE-V2 §4.1 für das Datenmodell): Paritätslauf lesend **und**
schreibend unverändert grün, Messlauf `tests/bench/scale/run.py` ohne Regression > 10 %,
Rückbau-Skript der Stufe getestet (Sicht weg, `ngx_legacy`-Tabelle mit `INSERT … SELECT FROM
<Sicht>` auf aktuellen Stand bringen, zurückverschieben). Bis Stufe 7 ist jede Stufe einzeln umkehrbar.

`pl-api-v2` bleibt unberührt (liest über `pl-domain`-Traits aus `pl-store`), ebenso Client-Replikat
(SQLite, `pl-sync-client`) – der Sync-Vertrag ändert sich nicht.

## 3. Migration von paperless-ngx und Rückweg

### 3.1 Umstieg (Migrationsassistent)

Ablauf bleibt für den Nutzer gleich (`paperrust-migrate-db switch`, docs/MIGRATION.md):

1. Quelle erkennen, lesen, prüfen – unverändert (`detect`, `source`, `value`).
2. Laden in das **v3.4.2-Django-Schema** wie heute (COPY, eine Transaktion, Zeilen-Hashes) – aber in
   ein eigenes Schema `ngx_import` statt `public`.
3. **Prüfbericht gegen `ngx_import`** – unverändert (`verify.rs` vergleicht Quelle ↔ Ziel).
4. `paperrust manage migrate` führt die Stufen 1–5 aus: `INSERT … SELECT` aus `ngx_import` in die
   nativen Tabellen (Umwandlungen: Inhalt/Datei auslagern, Tag-Zuordnung ohne id, Feldwerte nach
   Typ, guardian → Bitmaske, uuid erzeugen, Zähler berechnen). Dieselben SQL-Schritte wie für
   bestehende Installationen.
5. **Zweiter Prüfschritt:** die Kompatibilitäts-Sichten (gleiche Spalten wie Django) werden mit
   `ngx_import` verglichen (Zeilenzahl + Zeilen-Hash je Tabelle, vorhandener Hash-Code aus
   `pl-dbmigrate/src/hash.rs`). Nach Stufe 7 übernimmt diesen Vergleich die „ngx-Projektion“ (3.2).
6. `ngx_import` löschen (oder mit `--keep-import` behalten).

Dauer bei 1 Mio. Dokumenten (Schätzung, auf Kopie messen): Umwandlung 3–8 min zusätzlich zum heutigen
Laden; Zähler/uuid sind mengenbasiert. Der Export-Weg (`document_importer`) lädt ebenso erst ins
Importschema.

### 3.2 Rückweg: Export im paperless-ngx-Format

- Kein Rückweg mehr „gleiche Datenbank unter paperless-ngx starten“ (bewusst, neue Produktrichtung).
- `paperrust document_exporter` (heute 1:1 upstream, `crates/pl-cli/src/export/exporter.rs`) liest
  bis Stufe 7 einfach über die Sichten → funktioniert ohne Änderung.
- Ab Stufe 6: Modul **ngx-Projektion** (`pl-compat-ngx`, eine Datei je Django-Modell) erzeugt die
  Django-Zeilen aus den nativen Tabellen – dasselbe SQL wie die Sichten, nur als Bibliothek. Export,
  Migrationsprüfung (3.1 Schritt 5) und Rückbau-Skripte nutzen dieselbe Projektion (eine Wahrheit).
- Was im Export verloren geht (Known differences, im Vertrag festhalten): `uuid`/`rev`, Sync-Stand,
  PaperRust-eigene Tabellen (KI, Lebensbereiche, Geräte-Token, Einstellungen), Zähler (werden in
  paperless-ngx neu berechnet). Optional `paperrust-extra.json` im Export, damit ein Re-Import in
  PaperRust verlustfrei ist (uuid bleibt → Sync-Clients müssen nicht neu laden).
- Ausgabeversion bleibt `3.4.2`; neuere paperless-ngx-Versionen importieren ältere Exporte über
  ihre eigenen Migrationen.

### 3.3 Abnahmetest Rundreise (vor Stufe 1 Pflicht, danach je Stufe)

paperless-ngx-Export (Referenzumgebung) → Umstieg → PaperRust-Export → Vergleich der Manifeste
(Datensätze, Felder, Dateien, Prüfsummen; nur Reihenfolge/erlaubte Felder ausgenommen) →
Import des PaperRust-Exports in ein frisches paperless-ngx 3.4.2 (Referenz-Agent) → Stichprobe über
die API. Dazu `pl_scale_1m` als Größenprobe (nur Dauer und Speicher).

## 4. Risiken, Aufwand, Voraussetzungen

### 4.1 Voraussetzungen (vorher erledigt)

1. Laufende Sitzungen gemergt: Tempo (perf-finish: `docsql`, Indizes), Massenänderungen (bulk-ops:
   `pl-documents-write`, Worker), Migration (`pl-dbmigrate`). Sie berühren genau die Dateien von
   Stufe 1/2/6.
2. Entscheidung im Repo festhalten: Schema gehört PaperRust; upstream-Migrationen werden nicht mehr
   übernommen (docs/UPSTREAM-SYNC.md anpassen) – nur noch der Import älterer/neuerer
   paperless-ngx-Stände über den Assistenten.
3. Export-Rundreise (3.3) automatisiert und grün **auf dem heutigen Modell** – sonst fehlt der
   Vergleichsmaßstab.
4. Messkopie `pl_scale_1m` exklusiv für diese Arbeit (eigene DB, damit andere Messungen nicht leiden).

### 4.2 Risiken

| Risiko | Wirkung | Gegenmittel |
|---|---|---|
| Sicht ist nicht 100 % gleich (Typ, NULL, Reihenfolge ohne `ORDER BY`) | stille Abweichung in v1 | Paritätslauf lesend+schreibend je Stufe; Spaltentypen aus `pl-dbmigrate/schema/v3.4.2-columns.tsv` automatisch vergleichen |
| `INSTEAD OF`-Trigger: `ON CONFLICT`, `FOR UPDATE`, `COPY` (2.1) | Laufzeitfehler in einzelnen Handlern | Fundstellen vor der Stufe umstellen; Schreibparitätslauf |
| Zähler driften (vergessener Schreibweg, `TRUNCATE`, Trigger aus) | falsche Zahlen in der UI | Zähler nur aus Triggern; `paperrust manage counts check|rebuild` wie bei L5; nächtliche Stichprobe |
| Heiße Zählerzeilen (Inbox) bei vielen Schreibern | Sperrwartezeiten | Delta-Tabelle + Zusammenfalten (1.5) |
| Monetary/Select/Dokumentlink verlustbehaftet umgewandelt | Export ≠ Original | Rohtext behalten, Rundreisetest mit allen 10 Feldtypen |
| Lange Umbau-Transaktion sperrt große Tabellen (1 Mio.: Minuten) | Ausfallzeit beim Update | Stufen einzeln, Wartungsmodus mit Fortschrittsanzeige, Dauer vorab auf Kopie messen und in den Release-Notes nennen |
| Doppelte Wahrheit während des Übergangs (alter Code über Sicht, neuer nativ) | Verwirrung bei Fehlern | eine Stufe zur Zeit, CI-Liste „wer nutzt noch Django-Namen“ |
| Rückweg nur noch per Export | Nutzer kann nicht „mal eben“ zurück | klar dokumentieren; Export-Rundreise ist Release-Kriterium |
| Andere Sitzungen ändern parallel dieselben Crates | Konflikte | Voraussetzung 4.1.1; Stufen nicht parallelisieren |

### 4.3 Aufwand (S < 1 Tag, M 1–3 Tage, L > 3 Tage)

| Schritt | Aufwand | Größter Einzelposten |
|---|---|---|
| 0 Vorbereitung | M | Rundreisetest, Migrationsrahmen mit Rückbau |
| 1 Dokument schlank | L (≈ 6–8 Tage) | Sicht + Trigger, `docsql`/Serialisierung nativ, Paritätsläufe |
| 2 Tags + Zähler | M | Zähler-Trigger + Differenztest |
| 3 Feldwerte | L (≈ 4–6 Tage) | 10 Typen, Sicht rückwärts, Indizes, `custom_field_query` |
| 4 Rechte | M | Bitmaske ↔ guardian-Sicht, `set_permissions` |
| 5 Labels + Sync | M | Sequenzer/Tombstones ohne Seitentabelle |
| 6 Schreiber umstellen | L (≈ 10–15 Tage, 9 Crates je S–M) | `pl-documents-write`, `pl-consumer` |
| 7 Aufräumen | S | |
| **Gesamt** | **≈ 30–45 Arbeitstage** | |

Nutzen tritt früh ein: nach Stufe 1 + 2 (≈ 2 Wochen) sind #3, #4, #6, #7 (Notizen) erledigt; Stufe 3
bringt #7 (Custom Field). Stufen 4–7 sind vor allem Aufräumen und Vereinfachung.

## 5. Offene Punkte

- TOAST-Verhalten (0.) und die Größen in 1.12 auf einer Kopie bestätigen.
- Prüfen, ob der „Sync-Sequenzer v2“ (fe4a63f) #10d schon löst – dann entfällt das Ziel in 1.10.
- Ob `tag_ids integer[]` (GIN) im Dokument die Tag-Filter weiter beschleunigt, erst nach Stufe 2
  messen (doppelte Wahrheit, darum nicht im Grundplan).
- Auth-Tabellen (Rollen statt Django-Modellrechte, ARCHITECTURE-V2 L5 Teil 2) bewusst nicht in
  diesem Plan.

## 6. Umsetzungsstand (Stufen 0–7)

Register der Stufen: `crates/pl-store/src/datamodel.rs` (`STAGES`), SQL je Stufe unter
`crates/pl-store/migrations/dm<n>_*.sql` (+ `.down.sql`). Eingespielt von `paperrust manage migrate`
nach allen übrigen eigenen Migrationen, protokolliert in `paperrust_migrations` (kein Eintrag in
`django_migrations` – v3.4.2 bleibt die Erkennung für den Umstieg). Fortschritt: `docs/progress.toml`,
Gruppe „Eigenes Datenmodell“.

### Stufe 0 – Migrationsrahmen, Sicherung, Rückbau (fertig, 2026-11-13)

- **Rahmen:** `pl_store::datamodel::Stage { n, name, title, up, down, views }`; `pl_cli::datamodel`:
  vor jeder Stufe auf einer DB mit Dokumenten `pg_dump -Fc` nach `<DATA_DIR>/backups/` (ohne
  `pg_dump` Warnung und weiter; `--no-backup`/`PAPERRUST_DATAMODEL_BACKUP=off`),
  `paperrust manage datamodel status | check | rollback --to <n>`. `check` vergleicht die
  Kompatibilitäts-Sichten mit `pl-dbmigrate/schema/v3.4.2-columns.tsv` (Risiko §4.2 „Sicht nicht
  100 % gleich“, automatisch).
- **Erfassungs-Trigger folgen der nativen Tabelle:** `paperrust_native_table(text)` (pl_sync 0001,
  gleiche Fassung in jeder Stufe) bildet Django-Namen auf die geplanten `paperrust_*`-Tabellen ab,
  sobald es sie gibt; `paperrust_sync_tables()` nutzt sie (`sync enable` nach einer Stufe legt die
  Trigger an der richtigen Tabelle an).
- **CI-Prüfung** `tools/check_django_names.py` (+ `tools/django-names-baseline.json`, Schritt in
  `.github/workflows/ci.yml`): Zeilen je Crate, die einen der ersetzten Django-Namen enthalten, dürfen
  nicht steigen (Stand 2026-11-13: 629 Zeilen; `pl-dbmigrate`/`pl-sync-client` ausgenommen).
- **Rundreise** `tests/datamodel/roundtrip.sh` (§3.3): (1) paperless-ngx 3.4.2 exportiert die
  Referenzdaten = Maßstab Y; PaperRust baut eine Kopie um und exportiert X: **X = Y byte-gleich**;
  (2) Umstieg mit `paperrust-migrate-db switch` + `migrate`: Export **inhaltlich gleich** Y (Reihenfolge
  der Modelle ohne `Meta.ordering` – guardian, CustomFieldInstance – folgt nach dem Umzug der
  COPY-Reihenfolge, bei upstream genauso); (3) paperless-ngx importiert X in eine frische DB:
  **DB-Inhalt gleich** dem Import von Y (75 Tabellen); (4) PaperRust importiert X in ein frisch
  migriertes Modell und exportiert wieder: gleich bis auf `modified` (auto_now, upstream ebenso);
  (5) `datamodel rollback --to 0` → Export = Y, erneuter Umbau. Auf dem heutigen Modell (ohne Stufen)
  alle grün (2026-11-13).
- Offen aus §2.2 „Stufe 0“: Paritätslauf/Messlauf laufen je Stufe (siehe dort), nicht als eigener
  Schritt.

### Stufe 1 – Dokument schlank (fertig, 2026-11-13)

Migration `pl_store.dm1_paperrust_document` (`crates/pl-store/migrations/dm1_paperrust_document.sql`,
Rückbau `.down.sql`), Vertrag `docs/contract/pl-store.md`. `paperrust_document` (schmal),
`paperrust_document_content`, `paperrust_document_file`; Sicht `documents_document` mit
`INSTEAD OF`-Triggern; Sync-/Suchtrigger und 9 eingehende Fremdschlüssel an `paperrust_document`;
Django-Tabelle als leere Vorlage in `ngx_legacy`. Kopie in Heap-Reihenfolge (ohne `ORDER BY`), damit
Sortierungen mit Gleichstand möglichst wie vorher bleiben.

**Abweichungen vom Plan (bewusst):**
- `uuid`/`rev` nicht schon in Stufe 1 in die Zeile, sondern mit Stufe 5 für alle Arten gemeinsam
  (bis dahin unverändert `paperrust_sync_object`; vermeidet doppelte Wahrheit).
- Spaltentypen wie Django (`varchar(n)` statt `text` + CHECK), damit die Sicht typgleich ist.
- `ngx_legacy.documents_document` bleibt **leer** (Strukturvorlage für den Rückbau); Datensicherung
  ist der `pg_dump` vor der Stufe. Eine zweite Datenkopie in der DB kostete bei 1 Mio. Dokumenten 2,8 GB.
- `docsql`/Serialisierung lesen weiter über die Sicht: Join Removal macht das kostenlos (EXPLAIN im
  Test `datamodel_db`), Umstellung auf native Namen gehört zu Stufe 6 (dort mit Test-DBs, die die
  Stufen eingespielt haben). Keine Änderung an den Abfragepfaden von perf-rest.
- Schreibende Stellen, die über die Sicht nicht gehen, wurden umgestellt: `merge_as_versions`
  (`FOR UPDATE` auf `paperrust_document`), `document_importer` (UPDATE + INSERT statt
  `ON CONFLICT`, Sequenz über die Spaltenvorgabe der Sicht), Nutzer löschen (Sichten zählen als
  Tabellen).

**Messung** (Kopie `pl_dm_scale` von `pl_scale_1m`, 1 Mio. Dokumente, PostgreSQL 18, Debug-Build
nicht beteiligt – reine SQL-Zeiten laut `EXPLAIN ANALYZE`, bestes von 3, warm;
`tests/datamodel/measure.py`, Rohdaten `results/datamodel/dm1-*.json`):

| Messung | vorher (Django-Tabelle) | nachher (über die Sicht) |
|---|---:|---:|
| Heap Dokumenttabelle | 1 038 MB | **130 MB** (+ Text 812 MB, Datei 132 MB je eigene Tabelle) |
| Indizes Dokumenttabelle | 2 103 MB | 331 MB (+ Text 1 019 MB Trigramm, Datei 181 MB) |
| gesamt (Heap + Indizes + TOAST) | 4 166 MB | 3 517 MB (462 + 2 742 + 313) |
| `count(*)` lebende Dokumente (Index-Only/Seq) | 204 ms | 193–237 ms (gleich) |
| Seite 25 nach `created`/`title`/`added` | 0,0–4,8 ms | 0,0–0,1 ms |
| Seite 25 nach `created`, Offset 500 000 | 115 ms | 107 ms |
| 25 vollständige Zeilen (`SELECT *`) | 0,1 ms | 0,2 ms |
| `count`/`sum(content_length)` ohne Index | 831 ms | **221 ms** |
| `count` Filter ohne Index (`mime_type`) | 911 ms | **228 ms** |
| `count` je Korrespondent | 863 ms | **242 ms** |
| Prüfsumme nachschlagen | 0,1 ms | 0,0 ms |
| `content icontains` (Trigramm) | 4 273 ms | 3 634–3 905 ms (gleich, Rauschen) |
| **10 000 Dokumente Korrespondent setzen** (über den heutigen Schreibweg) | **52 920 ms** | **4 797 ms** |
| ein Dokument: Text ändern / anlegen | 3,7 / 4,0 ms | 2,0 / 0,8 ms |
| Dauer des Umbaus (Kopie, Indizes inkl. Trigramm, Fremdschlüssel) | – | 12 min 48 s |

Lesend über Indizes unverändert (keine Regression > 10 %); alles, was den Dokument-Heap liest, wird
≈ 4× schneller; Massenänderungen ≈ 11× schneller, weil eine Zeilenänderung nicht mehr alle 24 Indizes
inkl. des Trigramm-Index über den Text neu schreibt (Text unverändert → `paperrust_document_content`
unberührt). Die Mehrkosten der Sicht-Trigger je Zeile (Plan §2.1, Ziel < 10 %) sind damit mehr als
ausgeglichen. Der Plan schätzte für Zählungen ohne Index 0,25–0,3 s – getroffen.

**Tests:**
- `crates/pl-cli/tests/datamodel_db.rs` (`#[ignore]`, Referenz-DB): Sicht = Original (Zeilen-Hash),
  `datamodel check`, Fremdschlüssel/Trigger, Join Removal, INSERT … RETURNING, Text/Datei/Besitzer
  ändern (content_length, Sync-`rev` + 1, Suchmarkierung), UPDATE … FROM, eindeutige Dateinamen,
  Löschen mit/ohne Notiz, Rückbau (Hash, Sequenz, Trigger), erneuter Umbau – grün.
- `tests/datamodel/roundtrip.sh` mit Stufe 1: alle 5 Fälle grün (Export = ngx-Export byte-gleich,
  Umstieg per Assistent, Rückweg nach paperless-ngx 3.4.2, Import in das neue Modell, Rückbau).
- Paritätslauf (alle Lesefälle aller Crates, 2 024 Vergleiche): PaperRust ohne Stufe gegen PaperRust
  mit Stufe 1 auf je einer Kopie der Referenzdaten. Unterschiede nur (a) Umgebung (DB-Name in
  `/api/status/`, Sekunden im Drosseltext, von A beim Basic-Login neu berechnete Passwort-Hashes)
  und (b) 29 Sortierfälle (28 Dokumentlisten, 1 Tag-Liste), in denen nur die **Reihenfolge bei gleichem Sortierwert** abweicht
  (gleiche Datensätze, gleiche Schlüsselfolge, gleiche Mengen je Wert – geprüft für alle 16
  Dokument-Sortierungen); upstream legt diese Reihenfolge nicht fest (Known difference in
  `docs/contract/pl-store.md`).

### Stufe 2 – Tags + Zähler (fertig, 2026-11-13)

Migration `pl_store.dm2_paperrust_tag_count` (+ `.down.sql`), Vertrag `docs/contract/pl-store.md`
„Stufe 2“, Lesen `crates/pl-objects/src/counters.rs`.

- `documents_document_tags` → **umbenannt** in `paperrust_document_tag` (kein Kopieren von 5 Mio.
  Zeilen), PK `(document_id, tag_id)`, Index `(tag_id, document_id)`; zwei Django-Indizes entfallen.
  Sicht `documents_document_tags` ist eine einfache, automatisch beschreibbare Sicht.
- `paperrust_label_count` (+ `_delta`, Sicht `_live`, Index `(kind, owner_bucket, label_id)`) für
  Tags, Korrespondenten, Dokumenttypen, Speicherpfade; Pflege per Anweisungs-Trigger, Falten im
  Hintergrund; `paperrust manage datamodel counts check|rebuild|fold`.
- `pl-objects` liest `document_count` aus den Zählern (Liste, Einzelobjekt, Tag-Kinder, Sortierung nach
  `document_count`); `last_correspondence` und inaktive Nutzer weiter live.

**Abweichungen vom Plan (bewusst):**
- `id` bleibt in der Zuordnung (ohne Index): nur so ist die Sicht automatisch beschreibbar (alle
  Schreiber inkl. `INSERT … ON CONFLICT` laufen unverändert, keine Trigger-Kosten je Zeile).
- `note_count`/`tag_count` in `paperrust_document` nicht angelegt: einziger Leser wäre die Sortierung
  nach Notizanzahl in `pl_store::docsql` (Abfragepfad von perf-rest); ein Zähler-Update in der
  Dokumentzeile erhöhte zudem bei jeder Notiz/Tag-Änderung den Sync-`rev` ein zweites Mal. Gehört mit
  der Umstellung von `docsql` zu Stufe 6.
- `paperrust_doc_totals` nicht angelegt: die Statistik braucht je MIME-Typ auch das ASN-Maximum
  (nicht inkrementell führbar), und seit Stufe 1 liest ihre Abfrage nur noch die schmale
  Dokumenttabelle (gemessen unten, kein Engpass mehr wie 9,5 s in PERF-SCALE).
- `custom_field` (Art 6) zählt mit Stufe 3 (Feldwerte), nicht hier.

**Messung** (Kopie von `pl_scale_1m`, 1 Mio. Dokumente, 800 Tags, 5 000 Korrespondenten, Nutzer
`admin` und `restricted` (2 Gruppen, ~33 000 Freigaben); API über `tests/bench/scale/run.py`, c=1 p50
in ms, Debug-Build; vorher = Stufe 1, nachher = Stufe 2; Rohdaten `results/datamodel/dm2-*`):

| Endpunkt | admin vorher | admin nachher | restricted vorher | restricted nachher |
|---|---:|---:|---:|---:|
| `/api/tags/` Seite 25 nach Name | 336 | **40** | 2 616 | **241** |
| `/api/tags/` alle | 1 423 | **217** | 1 186 | **353** |
| `/api/correspondents/` Seite 25 | 8 | 6 | 16 | 14 |
| `/api/correspondents/` alle (5 000) | 348 | 284 | 360 | 337 |
| `/api/correspondents/` nach `-document_count` | 181 | 104 | 227 | 201 |
| `/api/document_types/` alle | 247 | **13** | 232 | 193 |
| `/api/storage_paths/` alle | 3 001 | **33** | 2 566 | 2 464 (Kosten nicht in der Zählung) |
| `/api/statistics/` | 585 | 493 | 494 | 389 |

Umbau auf 1 Mio. Dokumenten: **12,4 s** (Umbenennen + Zähler aufbauen, 286 833 Zählerzeilen).
Zuordnung: 436 MB gesamt (vorher 521 MB, PERF-SCALE). Der Rest bei eingeschränkten Nutzern ist die
Live-Zählung über die freigegebenen fremden Dokumente (~14 000 Zufallszugriffe, ~150 ms kalt) und
bei Speicherpfaden die Liste selbst (nicht die Zählung; offen für perf-rest).

**Tests:** Differenztest Zähler = bisheriger Weg (`crates/pl-cli/tests/datamodel_db.rs`: 5 Nutzer ×
4 Arten, mit/ohne id-Liste, nach Umbau, nach Schreiben, nach Falten), Rundreise
(`tests/datamodel/roundtrip.sh`) alle 5 Fälle grün, Paritätslauf Stufe 1 gegen Stufe 2: 2 021 von
2 024 gleich, Rest Umgebung bzw. Reihenfolge bei gleichem `document_count` (bekannte Abweichung).

### Stufe 3 – Feldwerte typisiert (fertig, 2026-11-14)

Migration `pl_store.dm3_paperrust_field_value` (+ `.down.sql`; beide mit
`dm_label_count_fns.sql` davor), Vertrag `docs/contract/pl-store.md` „Stufe 3“, Zählung
`crates/pl-fields-views/src/cf_counters.rs`.

- `paperrust_field_value`: 13 Wertspalten → Werttyp `t` + `v_text`/`v_bool`/`v_date`/`v_int`/
  `v_float`/`v_num` (GENERATED aus dem Monetary-Rohtext)/`v_refs`; PK `(document_id, field_id)`,
  `id` UNIQUE; Teilindizes je Typ `(field_id, v_*, document_id)`, GIN auf `v_refs`.
- Sicht `documents_customfieldinstance` bildet `value_date/int/float/monetary_amount/
  document_ids/bool` 1:1 ab (bestehende Abfragen nutzen die neuen Indizes), Textspalten über `t`.
- Zähler Art 6: `document_count` der Custom-Field-Liste aus den Zählern.
- Kopie in der Reihenfolge, in der Django die Feldwerte liefert (über den Index auf `created`),
  damit der Export byte-gleich bleibt (Gleichstände bei `created`, `Meta.ordering`).

**Abweichungen vom Plan (bewusst):** `v_int` bleibt `integer`, Float in eigener Spalte
`v_float` (statt `v_num numeric(20,4)`: Float → numeric ist nicht verlustfrei), `v_num` nur für
Monetary (`numeric(65,2)` wie upstream), `v_refs jsonb` statt `integer[]` + `paperrust_field_docref`
(verlustfrei, Rückwärtssuche über GIN). Spalte `t` zusätzlich, weil fünf Textarten sich `v_text`
teilen und leere Werte ihren Typ behalten müssen. Teilindizes aufsteigend (ohne Nullwerte ist die
Leserichtung egal).

**Messung** (Kopie von `pl_scale_1m`: 3 Mio. Feldwerte, 20 Felder; API über run.py, c=1 p50 in ms,
vorher = Stufe 2, nachher = Stufe 3; Rohdaten `results/datamodel/dm3-*`; drei Werte mit 12 statt 5
Läufen nachgemessen, weil die ersten Läufe unter Last anderer Sitzungen streuten):

| Endpunkt | admin vorher | admin nachher | restricted vorher | restricted nachher |
|---|---:|---:|---:|---:|
| `/api/custom_fields/` alle (mit `document_count`) | 2 325 | **17** | 2 444 | 1 458 |
| Dokumente `custom_field_query` AND (Bereich/exact) | 799 | **106** | 1 354 | 1 158 |
| Dokumente `custom_field_query` select | 2 032 | 910 | 1 224 | 1 262 |
| Dokumente `custom_field_query` OR + icontains | 2 837 | 2 082 | 2 169 | 1 544 |
| Dokumente `custom_fields__icontains` | 3 388 | 3 335 (12 Läufe) | 2 984 | 3 286 |
| Dokumente nach eigenem Feld sortiert | 1 083 | 1 076 (12 Läufe) | 1 102 | 1 163 |
| Dokumentliste Standard | 419 | 306 (12 Läufe) | 407 | 477 |
| Einzeldokument | 18,8 | 9,3 (12 Läufe) | 18,5 | 21,2 |

Feldwerte: 476 MB gesamt (Heap 239 MB + Indizes 237 MB; vorher 697 MB laut Plan). Umbau: **2 min 37 s**.
Die Sortierung nach eigenem Feld bleibt bei ~1,1 s: `pl_store::docsql` sortiert über einen
`CASE`-Ausdruck über alle Dokumente; der passende Teilindex existiert jetzt, die Abfrageform zu ändern
gehört zu Stufe 6 (Abfragepfad von perf-rest). Eingeschränkte Nutzer bei der Custom-Field-Liste:
Rest ist die Live-Zählung über ~33 000 freigegebene Dokumente mit je mehreren Feldwerten.

**Tests:** `crates/pl-cli/tests/datamodel_db.rs` (Feldwerte identisch zur Django-Tabelle, Schreiben
über die Sicht, Art-6-Zähler = bisherige Zählung für alle Nutzer, Rückbau aller Stufen, erneuter
Umbau), Rundreise 5/5 grün (Export byte-gleich auch nach Rückbau), Paritätslauf Stufe 2 gegen 3:
2 023 von 2 024 gleich (Rest: Datenbankname in `/api/status/`).

### Stufe 4 – Rechte (fertig, 2026-11-14)

Migration `pl_store.dm4_paperrust_object_grant` (+ `.down.sql`), Vertrag `docs/contract/pl-store.md`
„Stufe 4“. `paperrust_object_grant` (Bitmaske je Objekt und Subjekt, guardian-ids je Bit) ist die
Wahrheit, `paperrust_grant_raw` nimmt Zeilen außerhalb des Rasters auf; `guardian_*objectpermission`
und `paperrust_grant` (L5-Form) sind Sichten, die L5-Pflegetrigger entfallen. pl-authz erkennt den
nativen Zustand (`grants::status().native`, Hintergrund-Refresher setzt `grants::native()`) und
erzeugt die Freigabe-Bausteine (`granted_ids_subquery/_sql`, `checker_exists_sql`,
`any_grant_exists`) dann direkt auf `paperrust_object_grant` – über die Vereinigungs-Sicht fand der
Planer die Indexwege nicht (erste Messung: Einzeldokument 21 → 204 ms, Korrespondenten eingeschränkt
0,5 → 41 s; behoben durch `bigint`-Spalten ohne Cast in der Sicht, Ausdrucksindex für `object_pk`, die
Rechte-Abbildung als kleine Tabelle mit Statistik statt Sicht und nativ erzeugtes SQL).

**Abweichungen vom Plan:** `object_id`/`subject` `bigint` (wie L5, damit die L5-Sicht ohne Cast
indexfähig bleibt); zusätzlich `ids integer[]` (guardian-ids für Export, Audit, Rückweg); Rechte-
Abbildung `paperrust_grant_perm` als von Triggern nachgeführte Tabelle. Export ordnet guardian-
Datensätze nach id (upstream: Planreihenfolge eines Joins, nicht reproduzierbar). Doppelte Freigabe
über die guardian-Sicht wird still ignoriert (Schreiber nutzen `ON CONFLICT DO NOTHING`, das auf einer
Sicht nicht selbst wirkt). `pl_store.0003_paperrust_doc_live_fk` (perf-rest) legt seinen Index nach
Stufe 1 auf `paperrust_document`; Stufe 1 nimmt ihn beim Umbau mit.

**Messung** (Kopie von `pl_scale_1m`, 596 180 guardian-Zeilen → 397 227 Bitmasken-Zeilen, 58 MB;
API c=1 p50 in ms, vorher = Stufe 3, nachher = Stufe 4; Rohdaten `results/datamodel/dm4-*`):

| Endpunkt | admin vorher | admin nachher | restricted vorher | restricted nachher |
|---|---:|---:|---:|---:|
| Dokumentliste Standard | 186 | 206 | 339 | **222** |
| Filter Tag „any“ | 496 | 312 | 440 | 310 |
| Filter Besitzer | 44 | 40 | 52 | 40 |
| Filter „von mir geteilt“ | 349 | **91** | 186 | **68** |
| `selection_data` 10 000 | 344 | 258 | 463 | 296 |
| `/api/statistics/` | 826 | 325 | 598 | 404 |
| `/api/tags/` Seite | 45 | 31 | 338 | **168** |
| `/api/correspondents/` alle | 483 | 320 | 538 | **262** |
| Einzeldokument | 21 | 9 | 19 | 10 |

Umbau auf 1 Mio. Dokumenten: **16 s**. (Werte unter der Last anderer Sitzungen; Größenordnung belastbar.)

**Tests:** `crates/pl-cli/tests/datamodel_db.rs` (guardian-Zeilen identisch, nativ bereit,
Zählungen über Altweg, L5-Sicht und nativen Weg gleich, Entzug über die Sicht, Rückbau aller vier
Stufen), Rundreise 5/5 (Export gleich bis auf die Reihenfolge der guardian-Datensätze), Paritätslauf
Stufe 3 gegen 4: keine inhaltliche Abweichung; Rechte setzen per API (`set_permissions`) auf beiden
Ständen gleich.

### Stufe 5 – Labels + Sync (fertig, 2026-11-14)

Migration `pl_store.dm5_paperrust_label_sync` (+ `.down.sql`; davor im selben Stufentext
`crates/pl-sync/migrations/0003_paperrust_sync_native.sql` und `dm_label_count_fns.sql`), Vertrag
`docs/contract/pl-store.md` „Stufe 5“.

- Die fünf Label-Tabellen heißen `paperrust_tag`, `paperrust_correspondent`, `paperrust_document_type`,
  `paperrust_storage_path`, `paperrust_custom_field` (umbenannt, nicht kopiert) und tragen `uuid`/`rev`;
  unter den Django-Namen bleiben einfache, beschreibbare Sichten.
- `paperrust_document` trägt `uuid`/`rev` in der Zeile (Neuschreiben in Heap-Reihenfolge, fillfactor 85).
- `paperrust_sync_object` ist nur noch eine Lesesicht; Ansichten/Nutzer/Gruppen stehen in
  `paperrust_sync_ident`, Grabsteine in `paperrust_sync_tombstone`. `paperrust_sync_live` entfällt, der
  Sequenzer liest je Objekt per Primärschlüssel. Erfassung: `rev + 1` über einen Zeilen-Trigger an der
  nativen Tabelle, abhängige Tabellen und Freigaben über `paperrust_sync_bump`.
- Leser (`pl_store::ident`, `pl_store::sync`, `pl-timeline`) lesen weiter `paperrust_sync_object`
  (`local_id::int8`), fehlende Identitäten legt `paperrust_sync_ensure` an – beide Zustände ohne Weiche im
  Rust-Code.

**Abweichungen vom Plan (bewusst):** Arten 7–9 behalten eine (kleine) Identitätstabelle, weil ihre
Tabellen laut §0.5 Django bleiben. `paperrust_note` nicht angelegt (keine eigene Sync-Art). Die
Lesesicht bleibt als Übergang (fällt mit dem Umzug der Leser auf die nativen Tabellen).

**Beim Messen gefunden und behoben (betraf schon Stufe 2):** Der Zähler-Trigger an
`paperrust_document` las bei jeder Anweisung seine Übergangstabellen je Zeile ganz (10 000 Dokumente
≈ 18 s, auch wenn sich nichts Gezähltes änderte) und wählte bei überschätzter Zeilenzahl Hash-Joins über
alle Tag-Zuordnungen und Feldwerte; nach vielen Ein-Zeilen-Aufrufen hielt plpgsql zudem deren Plan vor
(10 000 Dokumente in einer Anweisung ≈ 50 s, der Besitzer-Vergleich der Sync-Erfassung ≈ 13 s). Jetzt:
nur Zeilen mit gezählter Änderung, Joins statt Unterabfragen, Indexzugriff je Dokument, große
Anweisungen (> 50 Zeilen) über `EXECUTE`. 10 000 Dokumente in einer Anweisung: **2,5 s** (davon Zähler
0,5 s, Erfassung 0,05 s).

**Messung** (Kopie `pl_dm5_scale` von `pl_scale_1m`, vorher = Stufe 4, nachher = Stufe 5; API über
run.py c=1 p50 in ms, Debug-Build; SQL über `tests/datamodel/measure_sync.py`; Rohdaten
`results/datamodel/dm5-*`; andere Sitzungen bauten zeitweise mit):

| Messung | vorher | nachher |
|---|---:|---:|
| `sync/ids?kind=document` admin / eingeschränkt | 11 992 / 4 609 | **6 846 / 1 357** |
| `sync/changes` erste Seite admin / eingeschränkt | 400 / 139 | 221 / 36 |
| v2 `documents/query` (Tag + Datum) admin / eingeschränkt | 211 / 604 | 101 / 96 |
| Dokumentliste Standard admin / eingeschränkt | 296 / 289 | 149 / 176 |
| Einzeldokument | 21,8 | 8,4 |
| `/api/tags/` alle admin / eingeschränkt | 270 / 2 235 | 198 / 351 |
| `/api/custom_fields/` alle eingeschränkt | 1 141 | 610 |
| SQL: Tag an 10 000 Dokumente / wieder weg (mit Erfassung) | 1 635 / 2 435 | 1 804 / 1 992 |
| SQL: Korrespondent an 10 000 Dokumenten über die Sicht / zurück | 8 791 / 5 574 | 6 809 / 7 469 |
| SQL: Sequenzer danach (10 000 bzw. 20 000 Wartelisten-Zeilen) | 423 | 442 (ein Ausreißer 43 s unter Last, nicht reproduzierbar: 135 ms je 5 000) |
| Größe Dokumenttabelle + Sync-Identität | 501 + 301 MB | **564 MB** + 0,1 MB |

Umbau auf 1 Mio. Dokumenten: **58 s**. Die API-Werte außerhalb des Sync streuen mit der Rechnerlast
(vorher lief ein fremder Build); belastbar ist: keine Regression, `sync/ids` 2–3× schneller, 237 MB
weniger. `sync/ids` für Admin bleibt bei ~7 s: der Rest ist das Serialisieren von 1 Mio. Paaren im
Debug-Build und die Sichtbarkeits-Unterabfrage, nicht mehr die Seitentabelle.

**Tests:** `crates/pl-cli/tests/datamodel_db.rs` (Label-Sichten = Original, Identität aller Arten,
Grabstein mit uuid und rev + 1, Sequenzer meldet `d`, rev bei Tag-Zuordnung/Label-Änderung, neuer Tag
über uuid auflösbar, Zähler nach Schreiben, Rückbau aller fünf Stufen mit gleicher uuid/rev, erneuter
Umbau), `crates/pl-sync/tests/db.rs` 10/10 (Zustand vor Stufe 5, inkl. `disable --purge`/`enable`),
Rundreise 5/5, Paritätslauf Stufe 4 gegen 5 (`tests/datamodel/parity_stage.sh 5 --no-write`): 2 023 von
2 024 gleich (Rest: Datenbankname in `/api/status/`; in einem Lauf zusätzlich die Reihenfolge zweier
Feldwerte mit gleichem `created` – upstream legt sie nicht fest). Schreibfälle des Paritätskatalogs
liefen **nicht** (verändern die Referenzkopie; Schreiben prüft der DB-Test).

### Stufe 6 – Schreiber umstellen, Einlesen bündeln (fertig im gewählten Umfang, 2026-11-16)

Keine neue Migration; reine Code-Umstellung (pl-authz, pl-consumer, pl-documents-write).

- **Massen-Schreiber direkt auf die native Tabelle:** `pl_authz::grants::document_row_table()` liefert
  `paperrust_document`, sobald das eigene Datenmodell erkannt ist, sonst den Django-Namen. Darüber
  schreiben `bulk_edit` (Korrespondent/Typ/Speicherpfad, Besitzer), „geändert am“ setzen
  (pl-documents-write `docs.rs`, `cfi.rs`), der gebündelte Nachlauf (pl-consumer `bulk.rs`) und
  `pl_consumer::db::update_document_fields`, solange keine Spalte außerhalb der schmalen Zeile
  (Text, Dateinamen, Prüfsumme) betroffen ist. Grund: Über die Sicht `documents_document` kostet jede
  Zeile einen INSTEAD-OF-Trigger mit eigener Anweisung – 10 000 Dokumente 6,8 s statt 2,5 s (SQL, 1 Mio.
  Dokumente, gemessen in Stufe 5).
- **Einlesen gebündelt:** `pl_consumer::lookup` – ein Nachschlage-Speicher je Einlesevorgang für
  Label-/Nutzernamen, ContentType-ids, Tag-Vorfahren, „Objekt existiert“, die Dokumentzeile und die Tags
  des Dokuments. Jede Schreibstelle in pl-consumer verwirft den Dokument-/Tag-Eintrag, danach wird wieder
  gelesen. Tags eines Dokuments kommen in einer Anweisung in die Datenbank (`db::add_tags`, gleiche
  Zeilenfolge wie upstream `bulk_create`).

**Bewusst nicht umgestellt (Abweichung vom Plan §2.2):** Schreiber, die je Aufruf ein Dokument ändern
(pl-workflows, pl-mail, pl-ai, pl-fields-views, Einzel-PUT/PATCH), bleiben auf den Sichten – ein
INSTEAD-OF-Trigger je Zeile kostet dort unter 1 ms und hält Audit und Zähler an einer Stelle. Export und
Import lesen/schreiben weiter über die Sichten (byte-gleicher Export, Rundreise grün); die
„ngx-Projektion“ als eigene Bibliothek ist damit nicht nötig, solange es die Sichten gibt. Offen bleiben
die Sortierung nach eigenem Feld über die Teilindizes in `pl_store::docsql` (≈ 1,1 s) und der
Notiz-Zähler – beides Lesepfade.

**Messung Einlesen** (Kopie `pl_dm6_scale` von `pl_scale_1m` mit allen fünf Stufen, 30 Textdateien
über die Upload-Route, Worker im selben Prozess, Debug-Build; vorher = main 21ac8ec, nachher = dieser
Stand; `tests/datamodel/measure_ingest.py`, Rohdaten `results/datamodel/dm6-*/ingest.json`):

| | vorher | nachher |
|---|---:|---:|
| SQL-Anweisungen je Dokument (ohne Anfragen des Messskripts, inkl. Aufräumen) | 277,8 | **200,1 (−28 %)** |
| davon Korrespondent-/Dokumenttyp-Name | 44 | 0 |
| Dokument neu laden | 23 | 12 |
| Tags des Dokuments lesen / einzeln einfügen | 20 / 11 | 10 / 7 |
| ContentType nachschlagen | 11 | 0 |
| Zeit je Dokument (4 Läufe abwechselnd, ohne Debug-Protokoll) | 1 038–1 381 ms | 1 063–1 323 ms |

Die Zeit je Dokument ändert sich nicht messbar: Sie wird hier von Warteschlange, Textverarbeitung und
Vorschau bestimmt, nicht von den Rundreisen zur Datenbank (lokale DB, < 0,3 ms je Anweisung). Der Gewinn
zeigt sich bei entfernter Datenbank und unter Last. Größte Reste: zwei vollständige Workflow-Ladevorgänge
je Dokument (je ≈ 30 Abfragen, zwei verschiedene Auslöser), Audit-Einträge (11), Inhalt der neuesten
Version für Dateinamen (9).

**Tests:** Einlese-Parität gegen upstream (`tests/parity/pl-consumer`): alle Fälle gleich bis auf
`barcode_tags_app_config` – derselbe Unterschied entsteht mit main (Tags aus Barcodes werden nicht
angelegt; bestehender Fehler, nicht Teil dieser Stufe). Gleichheitstest gebündelter Nachlauf
(`bulk_equal.rs`): 204 Dokumente, 403 Dateien, 62 Audit-Einträge, 33 Workflow-Läufe gleich.
Rundreise 5/5, `datamodel_db`, Clippy und Unit-Tests von pl-authz, pl-consumer, pl-documents-write,
pl-store, pl-sync, pl-cli. Paritätslauf Stufe 4 gegen 5 mit diesem Code (lesend): keine inhaltliche
Abweichung (Rest: Sekunden im Drosseltext, Datenbankname, eine nur auf Seite B gesetzte
Umgebungsvariable in `/api/config/`).

**Nicht geprüft:** die nativen Schreibwege selbst mit einem Gleichheitstest – `bulk_equal.rs` und die
Einlese-Parität laufen auf Datenbanken ohne Stufen (dort schreibt der Code über den Django-Namen). Die
Schreibfälle des Paritätskatalogs sind für den Vergleich zweier PaperRust-Stände ungeeignet: Der Harness
setzt vor jedem Schreibfall die gemeinsame Referenz-DB zurück, nicht die Kopien
(`tests/datamodel/parity_stage.sh` erzwingt darum `--no-write`; ein versehentlicher Lauf am 2026-11-16
hat die Referenz-DB `paperless` einmal aus `paperless_snapshot` neu erzeugt – Inhalt danach wie Seed).

### Stufe 7 – Aufräumen (Teil 1 fertig, 2026-11-16; Teil 2 offen)

**Teil 1 (umgesetzt): doppelte Abfrage- und Rechte-Schalter entfernt.**
- `PAPERRUST_QUERY_ENGINE` entfernt: Dokumentliste, `all=true`-Auswahl, v2-`documents/query` und
  `sync/ids` laufen nur noch über den AST-Weg (`pl_store::docsql`). Weg sind
  `pl_documents::listing::filtered_documents_legacy`, der Altzweig in `selection_filter`/`selection_data`,
  `pl_store::documents::apply_params`/`has_any_or_not` und die v1-Parameter-Brücke für v2-Abfragen ohne
  `any`/`not`. Der bisherige Weg über `pl_docfilter::apply` bleibt nur als Vergleichsmaßstab im
  Differenztest `crates/pl-store/tests/docquery_diff.rs` (eigene Kopie im Test). `pl-docfilter` selbst
  (Bereich der Sitzung perf-last) ist **nicht** geändert.
- `PAPERRUST_PERMISSION_ENGINE` entfernt: Der Rechteweg wird nicht mehr eingestellt. Mit bereitem
  Freigabe-Index bzw. nativem Datenmodell (Stufe 4) gilt immer `grants`. Der guardian-Weg bleibt **nur**
  als automatischer Rückfall für Datenbanken ohne PaperRust-Migrationen und als Maßstab im Differenztest
  (`set_override`, nur Tests).
- Einstellungs-Inventar (pl-settings, docs/SETTINGS.md, Benutzerdoku): beide Schalter gestrichen
  (Zählung A 178, gesamt 370).

**Teil 2 (offen, mit Begründung):**
- *guardian-Rückfall ganz entfernen:* 20 der 25 DB-Testdateien legen ihre Datenbank als reine Kopie von
  `paperless_snapshot` an – ohne PaperRust-Migrationen, also ohne Freigabe-Index. Erst wenn diese Tests
  (und die Paritäts-Skripte) über `manage migrate` laufen oder eine umgebaute Vorlage benutzen, kann der
  Rückfall weg. Vorschlag: Vorlage `paperless_snapshot_dm` (Snapshot + alle Migrationen) einmal je
  Seed-Stand erzeugen, Tests per Umgebungsvariable darauf umstellen.
- *Sichten und `ngx_legacy` löschen:* Export und Import lesen/schreiben über die Kompatibilitäts-Sichten
  (byte-gleicher Export), und `datamodel rollback` braucht `ngx_legacy`. Laut Plan erst nach mindestens
  einem Release mit den Stufen; dann zuerst die Leser auf die nativen Tabellen umziehen (CI-Prüfung
  `tools/check_django_names.py` zeigt, wer noch Django-Namen benutzt: Stand 671 Zeilen).

**Tests:** Abfrage-Differenztest (alter gegen AST-Weg) 2 855 Fälle (fest 1 655, zufällig 1 000,
Auswahl 200) – 0 Unterschiede; Rechte-Differenztest (`grants_diff`, 60 Mutationsrunden) grün;
pl-settings-Inventar grün; Clippy und Unit-Tests der geänderten Crates; Rundreise 5/5;
`datamodel_db`; Paritätslauf Stufe-6-Programm gegen Stufe-7-Programm (beide mit allen Stufen, lesend,
`PAPERRUST_BIN_A=… tests/datamodel/parity_stage.sh 9`): 2 023 von 2 024 gleich (Rest: Datenbankname in
`/api/status/`). `grants_bench` (Messhilfe in `grants_diff.rs`, seit L5 unverändert) scheitert an einem
Bit-Literal im erzeugten SQL – nicht Teil dieser Stufe, nicht untersucht.

### Stand 2026-11-16 (nach der Pause)

- **Fertig und geprüft:** Stufe 5 (5261dba, auf main), Stufe 6 (84ecea3), Stufe 7 Teil 1 (dieser Commit).
- **Offen:** Stufe 7 Teil 2 (guardian-Rückfall entfernen nach Umstellung der Test-Datenbanken; Sichten,
  `ngx_legacy` und Rückbau-Skripte nach einem Release), Sortierung nach eigenem Feld über die Teilindizes
  und Notiz-Zähler (`pl_store::docsql`, Lesepfad), Einlesen weiter bündeln (Workflow-Laden je Auslöser,
  Audit).
- **Bekannt, nicht Teil dieser Arbeit:** Einlese-Parität `barcode_tags_app_config` (auf main identisch),
  Messhilfe `grants_bench` (SQL mit ungültigem Bit-Literal).
- **Weitermachen mit Teil 2:** Vorlage `paperless_snapshot_dm` (Snapshot + `manage migrate`) bauen,
  DB-Tests der Crates darauf umstellen (ca. 20 Dateien), danach in pl-authz `Engine::Legacy` und die
  guardian-SQL-Bausteine entfernen; Paritäts-Skripte prüfen.
