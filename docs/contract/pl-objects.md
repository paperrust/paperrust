# Vertrag `pl-objects` (paperless-ngx v3.4.2)

Quellen (alle unter `upstream/src/`):
`documents/views.py` (`PassUserMixin`, `BulkPermissionMixin`, `PermissionsAwareDocumentCountMixin`,
`CorrespondentViewSet`, `TagViewSet`, `DocumentTypeViewSet`, `StoragePathViewSet`, `BulkEditObjectsView`),
`documents/serialisers.py` (`MatchingModelSerializer`, `SetPermissionsMixin`, `SerializerWithPerms`,
`SetPermissionsSerializer`, `OwnedObjectSerializer`, `CorrespondentSerializer`, `DocumentTypeSerializer`,
`TagSerializer`, `StoragePathSerializer`, `StoragePathTestSerializer`, `BulkEditObjectsSerializer`),
`documents/filters.py` (`CorrespondentFilterSet`, `TagFilterSet`, `DocumentTypeFilterSet`,
`StoragePathFilterSet`, `PermittedObjectsFilter`), `documents/permissions.py`, `documents/models.py`
(`MatchingModel`, `Tag.clean`), `documents/tasks.py::update_document_parent_tags`,
`documents/signals/handlers.py::before_task_publish_handler`, `documents/regex.py`,
`paperless/views.py::StandardPagination`.
Fremdbibliotheken (Version laut `upstream/uv.lock`): DRF 3.18.3, django-filter 26.2,
django-treenode 0.24.0 (nur noch für die Export-Projektion, siehe „Tag-Baum“), django-auditlog 3.4.1, django-guardian 3.5.2, Django 5.2.18.

## Routen-Übersicht

| id | Methode | Pfad | Status |
|---|---|---|---|
| `correspondents.list` | GET | `/api/correspondents/` | Rust |
| `correspondents.create` | POST | `/api/correspondents/` | Rust |
| `correspondents.retrieve` | GET | `/api/correspondents/{id}/` | Rust |
| `correspondents.update` | PUT | `/api/correspondents/{id}/` | Rust |
| `correspondents.partial_update` | PATCH | `/api/correspondents/{id}/` | Rust |
| `correspondents.destroy` | DELETE | `/api/correspondents/{id}/` | Rust |
| `document_types.*` | wie oben | `/api/document_types/…` | Rust |
| `tags.*` | wie oben | `/api/tags/…` | Rust |
| `storage_paths.list` / `.retrieve` / `.destroy` | GET/GET/DELETE | `/api/storage_paths/…` | Rust |
| `storage_paths.create` / `.update` / `.partial_update` | POST/PUT/PATCH | `/api/storage_paths/…` | Rust (2026-11-09, s. u.) |
| `storage_paths.test` | POST | `/api/storage_paths/test/` | Rust (W23, `src/sp_test.rs`, s. u.) |
| `bulk_edit_objects` | POST | `/api/bulk_edit_objects/` | Rust |

OPTIONS/HEAD bleiben Django (DRF-Metadaten). Eine `root`-Aktion gibt es bei Tags **nicht**
(nur `DocumentViewSet.root`).

## Gemeinsames Verhalten der vier ViewSets

### Rechte (`IsAuthenticated`, `PaperlessObjectPermissions`, `PermittedObjectsFilter`)

1. Nicht angemeldet → 401 `{"detail":"Authentication credentials were not provided."}`
   (Header `WWW-Authenticate: Token`, bzw. `Basic realm="api"` wenn der Request einen Basic-Header hatte) – Sache von `pl-authz`.
2. Modellrecht (Django `ModelBackend`, aktiv; aktiver Superuser hat alles):
   GET → `documents.view_<model>`, POST → `add_`, PUT/PATCH → `change_`, DELETE → `delete_`.
   Fehlt es → 403 `{"detail":"You do not have permission to perform this action."}`.
3. Sichtbarkeit (Liste und `get_object`): inaktiver angemeldeter Nutzer → leer; Superuser → alles;
   sonst `id IN permitted_object_ids(user, model, "view_<model>")` =
   `owner_id = user OR owner_id IS NULL OR CAST(id AS varchar) IN (UserObjectPermission.object_pk
   für user + view_<model> UNION ALL GroupObjectPermission.object_pk für Gruppen des Users)`.
   Nicht sichtbar / nicht vorhanden / pk nicht numerisch → 404 `{"detail":"No <Model> matches the given query."}`
   (`Correspondent`, `DocumentType`, `Tag`, `StoragePath`); pk nicht als int parsebar → 404 `{"detail":"Not found."}`.
4. Objektrecht (nur PUT/PATCH/DELETE): Besitzer oder `owner IS NULL` → erlaubt. Sonst guardian:
   aktiver Superuser → ja; sonst direktes oder Gruppen-Objektrecht `change_`/`delete_<model>`.
   Fehlt es: hat der Nutzer das Objektrecht `view_<model>` → 403 (Text wie oben), sonst 404 `{"detail":"Not found."}`.

### Query-Parameter

- `full_perms` (`get_boolean`: `yes,y,1,t,true`, case-insensitiv) → Feld `permissions` statt `user_can_change`.
- Paginierung (`StandardPagination`): `page` (Standard 1, `last` erlaubt, ungültig/außer Bereich →
  404 `{"detail":"Invalid page."}`), `page_size` (positive Zahl, Standard 25, max 100000, ungültig → 25).
- Filter (django-filter, ungültig → 400 `{"<param>": ["Enter a number."]}`):
  `id`, `id__in` (Komma-Liste), `name__istartswith|iendswith|icontains|iexact`;
  Tags zusätzlich `is_root` (NullBoolean: `true/True/1` → `tn_parent_id IS NULL`, `false/False/0` → NOT NULL, sonst kein Filter);
  Storage Paths zusätzlich `path__istartswith|iendswith|icontains|iexact`.
  Leere Werte filtern nicht. `i*`-Lookups wie Django/PostgreSQL: `UPPER(col::text) LIKE UPPER(pattern)` mit `\`-Escaping von `%`, `_`, `\`.
- `ordering` (DRF `OrderingFilter`, Komma-Liste, `-` für absteigend, unbekannte Terme werden verworfen):
  Korrespondenten `name, matching_algorithm, match, document_count, last_correspondence`;
  Tags `color, name, matching_algorithm, match, document_count`;
  Dokumenttypen `name, matching_algorithm, match, document_count`;
  Storage Paths `name, path, matching_algorithm, match, document_count`.
  Ohne gültigen Term: `ORDER BY LOWER(name)`. Keine pk-Tiebreaker (wie upstream).
- Korrespondenten-Liste: `last_correspondence=<nicht leer>` (jeder Wert, auch `false`) schaltet die Spalte ein.
  `ordering=last_correspondence` ohne diesen Parameter → Django-FieldError (500) – in Rust ebenso 500.
  Retrieve/Update/Destroy prüfen zusätzlich `has_object_permission` (GET: Objektrecht `view`, sonst 404).

### Antwort Liste

```
{"count": n, "next": url|null, "previous": url|null, ["all": [ids…] nur API-Version < 10], "results": [...]}
```
`next`/`previous`: absolute URL des Requests mit ersetztem `page` (bei Seite 1 entfernt), Query sortiert
und mit `quote_plus` kodiert (DRF `replace_query_param`). `all` = ids der gefilterten, sortierten Liste.
Tags zusätzlich `display_count` (hinter `results`) und Sonderfall `all` (s. Tags).

### document_count

- Korrespondent/Dokumenttyp/Storage Path: `COUNT(DISTINCT documents.id)` über `documents_document.<fk>`,
  Filter Superuser: `deleted_at IS NULL`; sonst `documents.id IN permitted_document_ids(user)`.
- Tags: Zählung über `documents_document_tags` (`COUNT(DISTINCT document_id)` mit
  `document_id IN permitted_document_ids(user)`), für **alle** Nutzer (auch Superuser).
- `permitted_document_ids(user)`: nicht gelöschte Dokumente; Superuser alle; sonst Autorisierung über das
  Wurzeldokument (`COALESCE(root_document_id, id)`, Besitzer des Wurzeldokuments): Besitzer = user, ohne Besitzer
  oder guardian-Recht `view_document` (User/Gruppe) auf `authorizing_id`.
- `last_correspondence`: `MAX(documents.created)` (Datum `YYYY-MM-DD`) mit demselben Filter; ohne Dokument `null`.

### Serializer-Felder (Reihenfolge)

Korrespondent: `id, slug, name, match, matching_algorithm, is_insensitive, document_count, last_correspondence, owner, user_can_change|permissions`
Dokumenttyp: `id, slug, name, match, matching_algorithm, is_insensitive, document_count, owner, user_can_change|permissions`
Storage Path: `id, slug, name, path, match, matching_algorithm, is_insensitive, document_count, owner, user_can_change|permissions`
Tag: `id, slug, name, color, text_color, match, matching_algorithm, is_insensitive, is_inbox_tag, document_count, owner, user_can_change|permissions, parent, children`

- `slug` = Django `slugify(name)` (NFKD → ASCII, lower, `[^\w\s-]` weg, `[-\s]+` → `-`, `-_` trimmen).
- `document_count` / `last_correspondence` fehlen ganz, wenn nicht annotiert (POST-Antwort; `last_correspondence`
  nur in Liste mit Parameter und in retrieve; nicht in PUT/PATCH-Antworten).
- `owner`: id oder `null`. `user_can_change`: Besitzer/kein Besitzer → true; aktiver Superuser → true;
  sonst direktes Recht `change_<model>` oder über Gruppe.
- `permissions`: `{"view":{"users":[…],"groups":[…]},"change":{…}}` – direkte guardian-Rechte, ids aufsteigend.
- `set_permissions` ist write-only.
- Tag `color` Zeichenkette, `text_color` aus Luminanz (`sqrt(.299 r² + .587 g² + .114 b²)` mit Kanal/256,
  `< 0.53` → `#ffffff`, sonst `#000000`, Parsefehler → `#000000`). `parent` = `tn_parent_id`.
  `children` = verschachtelte Tag-Objekte (gleiche Felder, rekursiv).

### Schreiben (POST/PUT/PATCH) – Validierung in DRF-Reihenfolge

1. Feldvalidierung (alle Felder, Fehler gesammelt, Reihenfolge der Felddefinition):
   - `name`: CharField(max 128), Pflicht bei POST/PUT. Texte: `This field is required.`, `This field may not be null.`,
     `Not a valid string.`, `This field may not be blank.`, `Ensure this field has no more than 128 characters.`,
     `Null characters are not allowed.`. Zahlen werden zu Strings, Whitespace getrimmt.
   - `match`: CharField(max 256, blank erlaubt); bei `matching_algorithm == 4` im Request:
     Regex-Kompilierung (Python `regex`) → `Invalid regular expression, see log for details.`
   - `matching_algorithm`: Choice 0–6 → `"<x>" is not a valid choice.`
   - `is_insensitive`, `is_inbox_tag`: Boolean → `Must be a valid boolean.`
   - `owner`: PK auf `auth_user`, null erlaubt, Default `null` (nur POST/PUT, nicht PATCH!) →
     `Invalid pk "<x>" - object does not exist.`, `Incorrect type. Expected pk value, received <type>.`
   - `color` (Tag): CharField(max 7) dann `#[0-9a-fA-F]{6}` am Anfang → `Invalid color.`
   - `parent` (Tag): PK auf beliebigen Tag (ohne Rechteprüfung), null erlaubt.
   - `set_permissions`: Objekt mit nur `view`/`change` (sonst `{"<key>":["Unknown permission action."]}`),
     darin `users`/`groups` Listen von Integern (null erlaubt). Danach Existenzprüfung:
     `Some users in don't exist or were specified twice.` / `Some groups in don't exist or were specified twice.`
2. Unique-Validatoren (DRF, `non_field_errors`): (a) `(name, owner)` bei owner ≠ null →
   `The fields name, owner must make a unique set.` (Update: nur wenn name/owner sich ändern); (b) der bedingte
   `UniqueTogetherValidator(name, condition owner IS NULL)` schlägt **nie** an: DRF prüft die Bedingung mit
   `Q(owner__isnull=True).check({"owner": None})`, `Value(None)` hat kein `output_field` →
   `OutputFieldIsNoneError` (FieldError) → `qs_exists_with_condition` liefert `False` (mit der Referenz-venv
   geprüft, W11). Ein Duplikat mit owner null meldet erst Schritt 4 (`{"error": [...]}`).
3. `TagSerializer.validate` (nur Tag): `{"parent":["Cannot set itself as parent."]}`,
   `{"parent":["Cannot set parent to a descendant."]}`, `{"parent":["Maximum nesting depth exceeded."]}`
   (Tiefe = Vorfahren des neuen Parents + 2 + eigene Teilbaumhöhe > 5).
4. `MatchingModelSerializer.validate`: bei `name` oder `owner` im Request: existiert anderes Objekt mit
   `(name, owner)` (owner = Request-Owner, sonst **aktueller Nutzer**) → `{"error":["Object violates owner / name unique constraint"]}`.
5. Update: owner-Änderung oder `set_permissions` nur für Superuser, Besitzer oder bei unbesessenem Objekt →
   403 `{"detail":"Insufficient permissions."}`.

Erstellen: owner = aktueller Nutzer, falls `owner` nicht im Request-Body. Antwort 201 mit Serializer
(ohne `document_count`). Update: Antwort 200 (mit `document_count` von vor dem Update).
Löschen: 204 ohne Body.

### Nebenwirkungen

- **Audit-Log** (`auditlog_logentry`, nur Korrespondent, Tag, Dokumenttyp; `PAPERLESS_AUDIT_LOG_ENABLED`, Standard an):
  create (action 0, nach dem Speichern; Tag-Felder seit L6 ohne treenode-Buchhaltung, siehe „Tag-Baum“), update (1, nur geänderte Felder, vor dem Speichern
  gegen DB-Stand), delete (2, je gelöschtem Objekt inkl. kaskadierter Kind-Tags). `changes` = JSON
  `{feld: [alt, neu]}` als Strings (`"None"` für leer; FK als id-String; Rückwärtsrelationen mit
  `related_name` als `"documents.Document.None"` bzw. `"documents.Tag.None"`). `actor_id`/`actor_email` = Nutzer nur,
  wenn Djangos Middlewares ihn **vor** DRF kennen (`AuditlogMiddleware._get_actor`: Session, Auto-Login,
  Remote-User-Middleware bei `PAPERLESS_ENABLE_HTTP_REMOTE_USER_API`); Token-, Basic- und DEBUG-Angular-Auth →
  beide `NULL` (`pl_core::client::audit_actor`, `drf::request_user_source`),
  `remote_addr` (erster Eintrag aus `X-Forwarded-For`, sonst Peer-IP), `remote_port` (`X-Forwarded-Port`),
  `cid` (`X-Correlation-ID`), `object_repr` = name, `content_type_id`, `object_pk`, `object_id`, `timestamp`.
- **Tag-Baum** (seit L6 Schritt 1): keine Neuberechnung mehr nach Speichern/Löschen – nur die Elternkante
  `tn_parent_id` wird geschrieben (v3.4.2: `update_tree()` schrieb alle `tn_*`-Spalten aller Tags neu und
  invalidierte den treenode-Cache). Details im Abschnitt „Tag-Baum (Adjazenzliste, L6 Schritt 1)“.
- Tag-Löschen kaskadiert auf alle Nachfahren (`tn_parent` CASCADE) und löscht m2m-Zeilen
  (`documents_document_tags`, Workflow-Trigger-/Action-, MailRule-Tabellen).
- **Abweichung von v3.4.2 (upstream-Fix fa83a4c5d, #14687):** `DELETE /api/tags/{pk}/` prüft zusätzlich, dass
  der Tag und **alle Nachfahren** in `permitted_object_ids(user, Tag, "delete_tag")` liegen, sonst 403
  `{"detail":"Insufficient permissions."}`; `bulk_edit_objects` `delete` für Tags nimmt alle Nachfahren in die
  Eigentümerprüfung auf (`_with_tag_descendants`). Test `crates/pl-objects/tests/tag_delete_descendants.rs`.
- Korrespondent/Dokumenttyp/Storage-Path löschen: `SET NULL` auf `documents_document`, `documents_workflowtrigger`,
  `documents_workflowaction`, `paperless_mail_mailrule` (letztere beiden nicht für Storage Path bei MailRule) und m2m-Zeilen löschen.
  Keine Dateiumbenennung (Django-`SET_NULL` sendet kein `post_save`), **außer** Storage Path:
  `bulk_update_documents(document_ids=[…])` per Celery (Header `trigger_source=system`) für alle nicht
  gelöschten Dokumente, die den Pfad nutzten. Rust: `sidefx::send_bulk_update_documents` →
  `pl_core::tasks::send_task` in die PaperRust-Warteschlange (seit 2026-11-14 der einzige Weg).
- Tag-Update mit neuem Parent (`perform_update`): allen Dokumenten mit dem Tag werden Parent + dessen Vorfahren
  per bulk insert hinzugefügt; betroffene Dokumente → `bulk_update_documents` (upstream Celery, Rust PaperRust-Warteschlange; `trigger_source=system`).
- Jede `bulk_update_documents`-Veröffentlichung legt (Django-Signal `before_task_publish`) einen
  `documents_paperlesstask`-Eintrag an (`task_type=bulk_update`, `trigger_source=system`, `status=PENDING`, `input_data={}`).
- Objektrechte (`set_permissions`): Ersetzen je Aktion (ohne merge), `change` impliziert `view`.

## Tags – Besonderheiten Liste

1. Gefilterte, sortierte Liste L (mit document_count).
2. D = alle Nachfahren von L (eine Momentaufnahme der Elternkanten, `tagtree::TagTree::descendants`). Wenn D nicht leer: Quelle S = Tags mit pk ∈ D ∪ L,
   beschränkt auf `view_tag`-sichtbar (Superuser alles), mit document_count, gleich sortiert; sonst S = L.
3. `children_map` = S gruppiert nach `tn_parent_id`; `children` jedes serialisierten Tags kommt daraus (rekursiv).
4. Antwort: Paginierung über L; zusätzlich `display_count = |S|`; bei D ≠ ∅ und API < 10: `all` = pks von S.
5. retrieve/update: `children` = Kinder über `tn_parent_id` (rekursiv nur erreichbare), sichtbar, mit document_count, sortiert nach
   `ordering`-Parameter (sonst `LOWER(name)`), rekursiv.

## Tag-Baum (Adjazenzliste, L6 Schritt 1)

Quelle: upstream 8aa99a381 (#14612, nach v3.4.2: `documents/tag_hierarchy.py`, `Tag.get_hierarchy()`,
Migration `0028_replace_tag_tree_fields`) – Idee nachgebaut, eigener Code. Plan: docs/DATAMODEL-V2.md §1.9.

- **Schema:** PaperRust-Migration `pl_objects.0001_paperrust_tag_adjacency`
  (`crates/pl-objects/migrations/`, eingehängt in `pl_cli::migrate::paperrust_migrations`) entfernt die 13
  abgeleiteten Spalten `tn_ancestors_pks … tn_siblings_count` inkl. `tn_priority`; `tn_parent_id` (FK, Index)
  bleibt mit gleichem Namen wie upstream nach #14612. Kein Eintrag in `django_migrations` (Umstiegs-Erkennung
  bleibt „v3.4.2“). Idempotent (`DROP COLUMN IF EXISTS`), reine Katalogänderung.
- **Umstieg von paperless-ngx 3.0–3.4.2:** Assistent bzw. Start mit vorhandener DB → v3.4.2-Schema (Schnappschuss
  oder Nachholen) → diese Migration. Exporte von 3.x mit `tn_*`-Feldern: `document_importer` überspringt genau
  diese 13 Felder, wenn die Spalten fehlen (`pl_cli::export::tagtree`), wie upstream.
- **Export (Rückweg):** Manifest bleibt v3.4.2-Format; die `tn_*`-Felder werden aus `tn_parent_id` berechnet
  (`pl_objects::treenode::compute`, `tn_priority` = 0), damit paperless-ngx 3.4.2 einen gültigen Baum importiert.
- **Code:** `tagtree::TagTree` (eine Abfrage `SELECT id, tn_parent_id`, je Vorgang): Nachfahren für Liste/
  Detail/`bulk_edit_objects` (`all` + Löschen), Vorfahren für `update_document_parent_tags`, Validierung
  `check_parent` (= `Tag.clean`, Texte unverändert: „Cannot set itself as parent.“, „Cannot set parent to a
  descendant.“, „Maximum nesting depth exceeded.“, Schlüssel `parent`). Andere Crates: `pl_consumer::db`
  (`tag_ancestors`, `tag(s)_descendants`, `with_ancestors` als rekursives SQL; genutzt von pl-consumer und
  pl-documents-write), `pl-ai` (`TagTree`).
- **API unverändert:** `parent`, `children` (nested, sichtbar, sortiert), `display_count`, `is_root`,
  Nachfahren-Rechteprüfung beim Löschen (#14687), Kaskade über `tn_parent`.

Known differences (gegenüber v3.4.2, gleich wie upstream nach #14612):
1. Audit-Log der Tags: `changes` enthält keine `tn_*`-Felder außer `tn_parent` (Feldliste
   `id, owner, name, match, matching_algorithm, is_insensitive, tn_parent, color, is_inbox_tag`).
   Paritätsregel `tag-adjacency-no-treenode-fields` (`tests/parity/pl-objects/accepted.json`).
2. `update_document_parent_tags` vergleicht Vorfahren als Zahlen (v3.4.2: Strings) → weniger Nachläufe.
3. `remove_inbox_tags` (pl-documents-write): Nachfahren eines Inbox-Tags, die gerade neu hinzugefügt werden,
   bleiben (`with_descendants(inbox) - keep`).
4. `tn_priority` (nur Django-Admin, nur für `tn_order`) geht bei der Migration verloren; keine API nutzt es.
5. Mischbetrieb mit Django v3.4.2 auf derselben DB geht nicht mehr (Django und Celery lesen Tags mit allen
   treenode-Spalten) – bewusst, Produktrichtung seit 2026-11-10 (docs/DATAMODEL-V2.md §0). Vor dem
   Umstieg sichern und paperless-ngx anhalten. Die frühere Sperre in `paperrust manage migrate`
   (`MIXED_MODE_ENDS`, solange `PAPERRUST_DJANGO_URL` nicht `off` war) ist seit 2026-11-14 entfernt: sie hing
   an der Proxy-Einstellung, die es nicht mehr gibt. Nur im Standalone-Betrieb zu migrieren und im Mischbetrieb die
   Spalten weiter zu pflegen wurde verworfen: das hieße `update_tree` behalten und zwei Schreiber mit
   verschiedenen Baummodellen auf denselben Zeilen.

## `/api/bulk_edit_objects/` (POST, JSON)

Request: `objects` (int-Liste), `all` (bool, Standard false), `filters` (Objekt, Filter wie Liste),
`object_type` (`tags|correspondents|document_types|storage_paths`), `operation` (`set_permissions|delete`),
`owner` (PK, null), `permissions` (wie `set_permissions`), `merge` (bool, Standard false).
Validierung: Choice-Fehler `"<x>" is not a valid choice.`; ohne `all`: `objects is required unless all is true.`,
`objects must not be empty`, `Some ids in objects don't exist or were specified twice.`;
`permissions` leer → `permissions must not be empty`. Fehler unter `non_field_errors`.
Recht: angemeldet. `all`: Objekte = permitted (`change_`/`delete_<model>`) ∩ Filter; bei Tags plus deren
Nachfahren, soweit selbst permitted. Nicht-Superuser: braucht Modellrecht `change_`/`delete_<model>` und alle
Objekte müssen eigene oder unbesessene sein, sonst 403 mit **Text-Body** `Insufficient permissions` (Django `HttpResponseForbidden`, `text/html`).
`set_permissions`: owner setzen (bei merge nur, wenn owner ≠ null, und nur bei Objekten ohne Besitzer),
Rechte setzen/mergen. Fehler → 400 Text `Error performing bulk permissions edit, check logs for more detail.`
`delete`: QuerySet-Delete (Kaskade/Audit wie oben). Antwort `{"result":"OK"}`.

## Storage Paths

`StoragePathSerializer.validate_path` ruft `validate_filepath_template_and_render` (sandboxed **Jinja2** mit
paperless-eigenen Filtern) auf – seit 2026-11-09 über `pl_templating` (Standalone,
docs/STANDALONE-GAPS.md): `path` = `CharField` (Pflicht, nicht leer, ohne Längengrenze), alte
`{var}`-Syntax umgewandelt (Warnung im Log), Rendern gegen das Platzhalter-Dokument; Fehler →
`{"path": ["Invalid variable detected."]}`. Sonst wie Korrespondenten (Matching, Owner, Rechte,
Eindeutigkeit). `update`: Dokumente des Pfads (ohne Papierkorb) bekommen `bulk_update_documents`
(trigger_source system) – **Known difference:** nach dem Commit gesendet statt vor `save()` (upstream
kann ein schneller Worker noch den alten Pfad sehen). Kein Auditlog (StoragePath nicht registriert).
Form-Bodys → Body-Gate (Django; ohne Django 404) wie alle Routen dieses Crates. `test` ist seit W23 in Rust (pl-templating hat inzwischen
`format_filename` und `storage_path_test_result`).

### `storage_paths.test` – POST `/api/storage_paths/test/` (W23, `src/sp_test.rs`)

Quelle: `StoragePathViewSet.test` + `get_permissions` (`action == "test"` →
`(IsAuthenticated, ViewDocumentsPermissions)`, **keine** Objektrechte am Speicherpfad),
`StoragePathTestSerializer`, `file_handling.format_filename`.

- Rechte: anonym 401/403 wie DRF; POST braucht `documents.view_document` (sonst 403
  `You do not have permission to perform this action.`).
- Körper: nur JSON (Form/Multipart → Body-Gate → 404). Kein Dict →
  `non_field_errors` („Invalid data. Expected a dictionary, but got list.“).
- Felder in Reihenfolge `path` (CharField, required, trim, nicht leer), `document`
  (PrimaryKeyRelatedField, required, nicht null, `""` = null; Queryset
  `Document.objects.filter(id__in=permitted_document_ids(user))` – nicht gelöscht, Versionen über
  ihr Wurzeldokument; unbekannt/nicht sichtbar → `Invalid pk "<v>" - object does not exist.`,
  falscher Typ → `Incorrect type. Expected pk value, received <type>.`). Fehler gesammelt → 400.
- Ergebnis: `format_filename(document, path)` (Vorlage **nicht** von `{}` umgewandelt,
  `FILENAME_FORMAT_REMOVE_NONE`, „-none-“ → „none“, Sicherheitsprüfung); leer/ungültig → `null`;
  sonst Pfad + Endung (`Path(filename).suffix` oder `file_type`) über
  `pl_templating::storage_path_test_result`. Antwort 200 JSON-String bzw. `null`.
- Dokumentkontext aus der DB wie `pl_consumer::templating::document_context` (eigene Kopie in
  `sp_test.rs`: pl-objects darf nicht von pl-consumer abhängen – pl-consumer → pl-search →
  (Feature `routes`) pl-objects wäre ein Zyklus).
- **Known difference:** Die Referenz läuft unter Windows: `str(Path(result))` liefert dort `\`
  als Trenner (z. B. `"Stadtwerke Zürich\Contract … 001.pdf"`). PaperRust liefert wie upstream
  unter Linux (dem einzigen offiziellen Ziel) `/`. Paritätsfälle mit Verzeichnissen weichen deshalb
  nur im Trenner ab (Regel `storage-path-test-windows-separator` in `tests/parity/pl-objects/accepted.json`).
- `None` → DRF `Response(None)`: 200, leerer Körper, **ohne** `Content-Type`; `""` bleibt `""`.
- Parität (W23, 2026-11-08, A = Referenz-Django :8000, B = PaperRust :8195 nur mit den W23-Routen, Rest Django; `tests/parity/pl-mail/w23b3env.sh`, Berichte `results/parity/w23/`): `tests/parity/pl-objects/cases.w23.json` 44 Fälle: 42 identisch, 2 akzeptiert
  (Trenner), 0 Abweichungen.

## Known differences

- **Review 2 M6 (behoben):** `MatchingModelSerializer.validate` prüft bei PATCH ohne `owner` gegen
  `request.user` (`data["owner"] if "owner" in data else self.user`); die von
  `UniqueTogetherValidator.filter_queryset` ergänzten Instanzwerte gelangen wie in DRF nicht in
  `validated_data` (DRF `run_validators` validiert eine Kopie).
- **Review 2 N21 (korrigiert, W11):** Der bedingte `UniqueTogetherValidator(name, owner IS NULL)` meldet nie
  etwas (siehe Schreiben, Schritt 2b); Duplikate mit owner null → `{"error": ["Object violates owner / name
  unique constraint"]}` aus `MatchingModelSerializer.validate` (W2-Paritätsfälle `*.create.duplicate` Schritt 3).
- **Audit-Akteur bei Basic/Token mit zusätzlich gültigem Session-Cookie:** Django schreibt dann den
  Session-Nutzer als actor (Middleware), DRF authentifiziert aber per Basic/Token (vor Session in der
  Klassenliste). Rust kennt in dem Fall nur die DRF-Methode → actor `NULL`. Seltener Mischfall; Behebung
  braucht die Session-Auswertung auch bei Header-Auth in pl-authz (CORE-REQUEST).
- **Review 2 N22:** `id__in` als JSON-Liste in `bulk_edit_objects.filters` wird akzeptiert; upstream 500
  (`value.split(",")` auf Liste). Bewusst toleranter.
- **Review 2 N25:** `fancy_regex` hat ein Kompilierlimit (10 MB); extrem verschachtelte, aber
  ≤ 256 Zeichen lange Muster wie `((a{255}){255}){255}` → „Invalid regular expression“, Python
  akzeptiert sie.

1. **Regex-Validierung** (`validate_match`): upstream kompiliert mit dem Python-Modul `regex`, Rust mit
   `fancy-regex` (Lookaround, Rückverweise, benannte Gruppen). Dialekt-Unterschiede (Python-spezifische
   Escapes/Inline-Flags mitten im Muster, `\p{..}`-Varianten) können zu anderem Ergebnis führen.
   Fehlertext identisch.
2. **Übersetzungen**: Fehlertexte nur Englisch (Django `LocaleMiddleware` übersetzt per `Accept-Language`).
3. **Audit-Log**: Schlüsselreihenfolge in `changes` ist upstream hash-abhängig (`set` von Feldern); in der DB
   liegt `jsonb`, die Reihenfolge ist dort ohnehin normalisiert → keine sichtbare Abweichung. `remote_addr`:
   upstream sieht hinter einem Reverse Proxy dessen Adresse; Rust nimmt `X-Forwarded-For[0]`, sonst die Peer-IP
   (nur wenn der Server `ConnectInfo` liefert, sonst `NULL`). `timestamp` = `clock_timestamp()`.
4. **treenode-Cache**: entfällt seit L6 Schritt 1 (keine treenode-Spalten mehr, kein Cache-Eintrag wird
   berührt; Django v3.4.2 kann Tags auf einer migrierten DB ohnehin nicht mehr lesen).
5. **Request-Körper**: nur JSON (`application/json`). Form-/Multipart-Körper (DRF-Standardparser) → Rust 415.
   Text bei JSON-Syntaxfehlern wie CPython (`pl_core::pyjson`, `ApiError::json_body_parse_error`); nur wenn
   Python den Body annähme, serde aber nicht (z. B. Zahl > 1e308), bleibt der serde-Text.
6. Float-Eingaben für Strings (`name: 1.5`) werden mit Python-`repr`-Nachbau zu Strings; sehr exotische
   Zahlen (> 1e308, NaN) sind in JSON ohnehin nicht darstellbar.
7. **Transaktion**: Rust schreibt jede Änderung in einer Transaktion (upstream ohne `ATOMIC_REQUESTS`
   → bei Fehler mitten im Request Teilzustände). Einzige nachgebildete Ausnahme: `set_permissions` bleibt
   gespeichert, wenn danach `validate_unique_together` scheitert (wie upstream).
8. **Einreihfehler** nach erfolgreichem Commit (z. B. Warteschlange nicht erreichbar) → Rust 500, die DB-Änderung
   bleibt (upstream: `apply_async` wirft bei Broker-Fehler ebenfalls → 500, DB-Änderung ebenfalls schon
   gespeichert). Seit 2026-11-14 PaperRust-Warteschlange statt Celery/Redis.

Upstream-Eigenheiten, die bewusst **nachgebaut** sind:
- `ordering=last_correspondence` ohne Parameter `last_correspondence` → 500 (FieldError).
- `user_can_change` in der Tag-Liste ist für Kind-Tags außerhalb der Seite `false` (Batch-Kontext ohne Eintrag),
  wenn der Nutzer weder Besitzer noch Superuser ist.
- ~~`update_document_parent_tags`: Vorfahren als Strings → Dokumente gelten immer als betroffen~~ – seit L6
  Schritt 1 wie upstream 8aa99a381: Ganzzahl-Vergleich, nur wirklich fehlende Paare/Dokumente.
- Audit-`create`/`delete`: FK-Felder ohne Wert erscheinen als `["None", "None"]`, Rückwärtsrelationen als
  `"documents.Document.None"` / `"documents.Tag.None"` (gegen die Referenz-DB geprüft).
- PUT ohne `owner` setzt den Besitzer auf `null` (DRF-Default `None` aus den Unique-Constraints).
- POST ohne `owner`: `owner` hat den DRF-Default `None` → `MatchingModelSerializer.validate` prüft gegen
  `owner IS NULL` (`{"error": ["…"]}` als Liste), danach `validate_unique_together` gegen den aktuellen Nutzer
  (`{"error": "Object violates owner / name unique constraint"}` als String).

## Paritätsstand

- Fall-Katalog: `tests/parity/pl-objects/cases.json` (erzeugt von `gen_cases.py`, 115 Fälle inkl. Schreibfälle,
  ergänzt `cases.base.json`), Lauf mit `python tests/parity/harness/run_parity.py --crate pl-objects`
  (braucht aktivierte Routen in `routes.toml` → Koordinator).
- In-Process-Vergleich nur lesend: `crates/pl-objects/tests/reference_parity.rs` (`#[ignore]`, alle GET-Routen,
  4 Nutzer × API 9/10 gegen Django `127.0.0.1:8000`). Lauf 2026-11-08: **0 Unterschiede in 1256 Requests**
  (Status + JSON). Schreibrouten und bulk_edit_objects: noch kein A/B-Lauf (Routen nicht aktiviert).
- treenode-Werte für den v3.4.2-Export gegen den Baum der Referenz-DB geprüft (Unit-Test
  `treenode::reference_tree`); Hierarchie nach der Migration gegen die treenode-Werte der Referenzkopie:
  `crates/pl-objects/tests/tag_adjacency.rs`.
- FK-Ziele für das Löschen gegen `pg_constraint` der Referenz-DB geprüft (identisch mit `Kind::relations()`).
