# Vertrag `pl-core`

Gemeinsamer Kern für alle Fach-Crates. Ziel: Verhalten von Django 5.2.18 / DRF 3.18.3 /
django-filter 26.2 (Versionen laut `upstream/uv.lock`) in
paperless-ngx v3.4.2. Geprüft gegen die Original-Wheels dieser Versionen (Quelltext gelesen),
nicht aus dem Gedächtnis. Alle Punkte sind durch Unit-Tests in `crates/pl-core/src/*` belegt
(`cargo test -p pl-core`: 52 Unit-Tests + 5 Doc-Tests, ohne DB).

## Öffentliche API (Überblick)

| Bereich | API |
|---|---|
| Konfiguration | `Config::from_env()`, `Config::from_env_map(Env)`, `config::Env` (`get/bool/int/list/path/json`), `Config::host_allowed`, `Config::meta_key_to_header`, `config::{retired_warnings, split_domain_port, is_same_domain, str_to_bool, DbConfig, DbEngine, ConfigError}` |
| Zustand | `AppState::{from_env().await, new(Config), with_pool(Config, PgPool)}`, `state.db()/config()/tz()/http()/cache()/service()/set_service()`, `AppState.inner.db` (wie bisher), `state::{pg_connect_options, pool_options, StateError, PING_IF_IDLE_FOR}` |
| Routen | `ApiRoute {id, method, path, upstream, handler}` (unverändert) + `ApiRoute::new`, Markierungen `.delegatable()`, `.accepts_form()`, `.accepts_multipart()`, `.accepts_drf_parsers()`, `.flags()` → `RouteFlags`, `route::route_flags(id)`, `route::{drf_view, apply_drf_view_headers, patch_vary, allow_value, ALLOW_MODEL_LIST, ALLOW_MODEL_DETAIL, ALLOW_READ_ONLY, ALLOW_POST_ONLY}` |
| Request | `RequestContext` (Extraktor, `FromRequestParts<AppState>`): `path, raw_query, query, scheme, host, negotiation, language, headers`, `version()`, `build_absolute_uri()`, `build_absolute_uri_for(loc)`, `full_path()`, `scheme_host()`; `request::{ServerScheme, ServerAddr, request_scheme, request_host, language_from_request, supported_language_variant}` |
| Query | `QueryParams` (Extraktor): `parse, get (letzter Wert), get_list, contains, keys, pairs` |
| Fehler | `ApiError` (+ `IntoResponse`), `ApiResult<T>`, `ValidationErrors`, `error::NON_FIELD_ERRORS` |
| JSON | `Drf<T>` (`ok/created/with_status/indent`), `json::{drf_response, drf_response_indent, drf_no_content, to_drf_json, to_drf_json_indent, python_float_repr}` |
| Datum | `datetime::{format_datetime, format_datetime_opt, format_date, python_isoformat, parse_date_input, parse_iso_datetime_input, parse_datetime_input, drf_parse_datetime, drf_parse_date, django_parse_datetime, django_parse_date, python_datetime_fromisoformat, python_date_fromisoformat, from_current_timezone, strptime, date_input_formats, datetime_input_formats}` + Meldungskonstanten |
| Version | `ApiVersion` (`V9`, `V10`, `LATEST`, `as_u16`, `includes_all`), `Negotiation`, `version::{negotiate, prefers_browsable_api, Renderer, MediaType, parse_header_parameters, ALLOWED_VERSIONS, DEFAULT_VERSION}` |
| Paginierung | `pagination::{PageRequest::from_query/resolve, PageInfo (next_link/previous_link/offset), paginated_body, paginated_body_for_uri, fetch_page, count, count_statement, fetch_all_ids, python_int, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, TOTAL_COUNT_ALIAS}` |
| Filter | `filters::{FilterSet (field/declared/conditions/apply/params), FilterDef, FieldKind (INT, BIGINT, POSITIVE_INT, …, Char, Boolean, Date, DateTime), Lookup, FilterContext, CHAR_KWARGS, ID_KWARGS, INT_KWARGS, DATE_KWARGS, DATETIME_KWARGS, build_condition, char_condition, int_condition, parse_number, parse_char, parse_bool_widget, escape_like, IntValue, parse_ordering, apply_order_terms, apply_ordering, OrderTerm}` |
| Aufgaben | `tasks::{record_task(executor, req, id, now), TaskRequest (new/arg/kwarg/header/trigger_source), trigger_source::*, TRACKED_TASKS, DEFAULT_QUEUE ("celery", Name der Standard-Warteschlange wie upstream), tracked_task_type, determine_trigger_source, extract_input_data}` (bis 2026-11-14 im Modul `celery`) |
| Task-Versand (W14) | `tasks::{send_task(state, req) → Uuid, TaskDispatch (dispatch(TaskRequest) → task_id, records_task() – Standard true), Dispatcher(Arc<dyn TaskDispatch>) (Dienst), dispatcher(state), prepare_task(state, req) → PreparedTask (records_in_caller, record(executor), publish(state)), DispatchFuture}`. pl-server hängt beim Start immer den Dispatcher der PaperRust-Warteschlange ein (PgQueue). Ohne eingehängten `Dispatcher` (nur Crate-Tests/Werkzeuge ohne Warteschlange) liefern `send_task`/`prepare_task` einen Fehler (`ApiError::internal`, „no task queue attached“) – einen Celery-Ausweichweg gibt es seit 2026-11-14 nicht mehr. Die Task-ID wird vorab festgelegt; legt der Dispatcher den PaperlessTask nicht selbst an (`records_task() == false`), tut es `send_task` bzw. `PreparedTask::record` in der Transaktion des Aufrufers. |
| Server-Markierungen | `markers::{DelegateToDjango, delegate_to_django(), is_delegation, DELEGATE_STATUS, Authenticated, RequestNotes, Notes}`, `ApiError::delegate_to_django(reason)` / `is_delegation()` (Re-Exporte im Wurzelmodul). Name historisch: seit 2026-11-14 bedeutet die Markierung „Fall von PaperRust nicht unterstützt“, pl-server antwortet 404 (keine Weiterleitung an Django mehr) |
| Sprache | `locale::{language_from_request(cookie, accept), language_from_headers(headers, cookie_name), get_supported_language_variant, parse_accept_lang_header, cookie_value, static_language, LANGUAGES, LANGUAGE_CODE}` (gegen die Referenz geprüfter Port aus pl-server; `request::language_from_request` nutzt ihn) |
| Sonstiges | `urlenc::{quote, quote_plus, unquote, parse_qsl, parse_qs, urlencode_doseq, urlsplit, urlunsplit, replace_query_param, remove_query_param, escape_uri_path, iri_to_uri, decode_request_path}`, `pyrepr::{repr_value, repr_str, repr_tuple}`, `pyjson::decode_error` (CPython-JSON-Fehlertext), `pyset::{PySet, missing_targets_order}` (CPython-`set`-Reihenfolge), `client::{audit_remote_addr, audit_remote_port, audit_remote, audit_actor, RequestUserSource}`, `dbconn::{is_connection_error, note_error, needs_ping, retry_stale_once}`; Re-Exporte `chrono, chrono_tz, sea_query, sea_query_binder, serde_json, sqlx` |

### Typisches Handler-Muster

```rust
use pl_core::{ApiResult, AppState, Drf, RequestContext, route::{drf_view, ALLOW_MODEL_LIST}};
use pl_core::filters::{FilterSet, FilterContext, FieldKind, ID_KWARGS, CHAR_KWARGS, apply_ordering};
use pl_core::pagination::{PageRequest, fetch_page, fetch_all_ids, paginated_body};

async fn list(ctx: RequestContext, /* user: pl_authz::User, */ state: axum::extract::State<AppState>) -> ApiResult<Drf<serde_json::Value>> {
    let mut q = /* sea_query::Query::select() … inkl. Rechtefilter aus pl-authz */;
    filterset().apply(&mut q, &ctx.query, &FilterContext::new(&ctx, state.tz()))?;
    apply_ordering(&mut q, &ctx.query, &ORDERING_FIELDS, &["name"]);
    let (rows, info) = fetch_page(state.db(), &q, &PageRequest::from_query(&ctx.query)).await?;
    let all = if ctx.version().includes_all() { Some(fetch_all_ids(state.db(), &q, id_expr).await?.into_iter().map(Into::into).collect()) } else { None };
    Ok(Drf::ok(paginated_body(&ctx, &info, serialize(rows), all)).indent(ctx.negotiation.indent))
}
// ApiRoute { id: "tags.list", method: Method::GET, path: "/api/tags/", upstream: "…", handler: drf_view(get(list), ALLOW_MODEL_LIST) }
```

Reihenfolge der Extraktoren: `RequestContext` zuerst (DRF: Negotiation → Version → Authentifizierung
→ Rechte → Throttling, `APIView.initial`).

---

## 1. Konfiguration – `config.rs`

Quelle: `src/paperless/settings/__init__.py`, `settings/custom.py`, `settings/parsers.py`.

* Vor dem Lesen: erste existierende Datei aus `PAPERLESS_CONFIGURATION_PATH`, `../paperless.conf`,
  `/etc/paperless.conf`, `/usr/local/etc/paperless.conf` laden, gesetzte Variablen gewinnen
  (python-dotenv `override=False`).
* Bool: `str_to_bool` (`true/1/t/y/yes`, `false/0/f/n/no`, case-insensitive), sonst Startfehler.
* Liste: Komma, trimmen, Leere entfernen; leerer Wert → Standard.
* Pfade: absolut gemacht (`Path.resolve()`; ohne Symlink-Auflösung, s. Known differences).
* Felder (Auswahl, Standard in Klammern): `debug` (NO), `secret_key` (Pflicht, ≠ `change-me`),
  `time_zone`/`tz` (UTC), Verzeichnisse `static_root`, `media_root` (+`originals_dir`, `archive_dir`,
  `thumbnail_dir`, `share_link_bundle_dir`, `media_lock`), `data_dir` (+`index_dir`, `model_file`,
  `llm_index_dir`, `logging_dir`), `consumption_dir`, `scratch_dir` (`$TMP/paperless`),
  `empty_trash_dir` (`PAPERLESS_TRASH_DIR` oder veraltet `PAPERLESS_EMPTY_TRASH_DIR`);
  Hosting `force_script_name`, `base_url`, `login_url`, `login_redirect_url`, `logout_redirect_url`
  (`LOGIN_URL?loggedout=1`), `static_url`; `paperless_url`, `allowed_hosts` (`["*"]`; sonst
  `+localhost`; `+` Host aus `PAPERLESS_URL`), `csrf_trusted_origins` (+URL, +`http://localhost:4200`
  bei DEBUG), `cors_allowed_origins` (`["http://localhost:8000"]`, +URL, +DEBUG-Origin),
  `use_x_forwarded_host` (`PAPERLESS_USE_X_FORWARD_HOST`, false), `use_x_forwarded_port`,
  `secure_proxy_ssl_header` (`PAPERLESS_PROXY_SSL_HEADER`, JSON-Liste), `trusted_proxies`,
  `allauth_trusted_proxy_count`; Cookies `cookie_prefix`, `csrf_cookie_name`, `session_cookie_name`,
  `language_cookie_name`, `session_cookie_age` (1814400), `account_session_remember` (True),
  `session_expire_at_browser_close`; Auth `enable_http_remote_user(_api)`,
  `http_remote_user_header_name` (`HTTP_REMOTE_USER`, META-Format), `auto_login_username`,
  `disable_regular_login`, `redirect_login_to_sso`, Signup-/Social-Schalter,
  `account_email_verification` (`none` ohne E-Mail, sonst `optional`), `token_throttle_rate` (`5/min`);
  E-Mail `email_*`, `email_enabled`; DB `db` (s. u.);
  (die Redis-Felder `celery_redis_url`, `channels_redis_url`, `redis_prefix` sind seit 2026-11-14 entfernt);
  fachlich `filename_format`, `filename_format_remove_none`, `audit_log_enabled` (true),
  `empty_trash_delay` (30, min 1), `app_title`, `app_logo`, `enable_update_check` (None = "default"),
  `task_workers` (1; PaperRust: `auto` = `resources::task_workers_from_env`, Known difference 14), `worker_timeout` (1800), `tika_enabled`, `ai_enabled`, `enable_compression`
  (yes), `db_read_cache_enabled` (no). Alles Weitere: `config.env.get("PAPERLESS_…")` + Parser.
* DB (`parse_db_settings`): Engine `PAPERLESS_DBENGINE` ∈ {sqlite, postgresql, mariadb}, sonst
  `postgresql` wenn `PAPERLESS_DBHOST` gesetzt, sonst `sqlite`. `DBNAME/DBUSER/DBPASS` (paperless),
  `DBPORT`, `DBSSLMODE` (prefer), `DBSSLROOTCERT/CERT/KEY`, `DB_TIMEOUT`, `DB_POOLSIZE`,
  `PAPERLESS_DB_OPTIONS` (`k=v,…`, überschreibt). `AppState::new` lehnt Nicht-PostgreSQL ab.
* PaperRust: `PAPERLESS_RS_BIND` (`127.0.0.1:8080`, `config::DEFAULT_BIND`; pl-server validiert), `PAPERLESS_RS_BASE_DIR` (= Django `BASE_DIR.parent`,
  Standard: Arbeitsverzeichnis), `PAPERLESS_RS_DB_MAX_CONNECTIONS` (10, wenn `DB_POOLSIZE` fehlt).
* Suche: `ocr_language` (`PAPERLESS_OCR_LANGUAGE`, `eng`), `search_language` (`_get_search_language_setting`;
  ungültiges `PAPERLESS_SEARCH_LANGUAGE` → Startfehler wie upstream `ValueError`),
  `advanced_fuzzy_search_threshold`.
* Remote-OCR: `remote_ocr_engine/_api_key/_endpoint` (`os.getenv`, leer bleibt `Some("")`),
  `remote_ocr_mode` (`always`|`workflow_only`, sonst Startfehler), `remote_ocr_allow_internal_endpoints`.
* Entfernte Variablen (2026-11-14): `retired_warnings(&Env)` liefert je einen Hinweistext für
  `PAPERRUST_DJANGO_URL`, `PAPERRUST_QUEUE`/`PAPERRUST_QUEUE_BACKEND` (nur bei anderem Wert als
  `postgres`), `PAPERLESS_REDIS` („Redis can be shut down“), `PAPERRUST_REDIS_CONNECT_TIMEOUT`,
  `PAPERRUST_REDIS_RETRY_AFTER`, `PAPERRUST_PROXY_{CONNECT,RESPONSE}_TIMEOUT_SECS`,
  `PAPERRUST_PROXY_X_FORWARDED_FOR`. Nie ein Startfehler; Feld `django_url` gibt es nicht mehr.
* Mail: `email_certificate_file`, `email_allow_internal_hosts`, `oauth_callback_base_url`,
  `gmail_/outlook_oauth_client_id/_secret`, `gmail_/outlook_oauth_enabled` (wie upstream berechnet).

## 2. AppState – `state.rs`

* `Clone`, alles hinter `Arc`. `AppStateInner` ist `#[non_exhaustive]` (Felder lesbar, Bau nur
  über Konstruktoren).
* Pool: lazy (`connect_lazy_with`), `max_connections` s. o., `min_connections(0)`,
  `idle_timeout 60 s` (`POOL_IDLE_TIMEOUT`, bis 2026-11-18: 600 s), `max_lifetime 300 s`
  (`POOL_MAX_LIFETIME`, sqlx-Standard 30 min; Begründung unten „Speicher-Befund 2026-11-18“), `test_before_acquire(false)`, `before_acquire`: Ping nur wenn
  `idle_for >= 30 s` **oder** die Verbindung schon ruhte, als ein Verbindungsfehler beobachtet wurde
  (`dbconn::needs_ping`; Fehler werden in `From<sqlx::Error> for ApiError` gemeldet: I/O, TLS,
  Protokoll, SQLSTATE `08*`, `57P01..03`). Ein fehlgeschlagener Ping verwirft die Verbindung, sqlx
  nimmt die nächste/neue. `dbconn::retry_stale_once(|| …)` wiederholt eine Abfrage einmal nach einem
  Verbindungsfehler (für die erste Abfrage einer Anfrage, z. B. Nutzer-Lookup). Django öffnet pro
  Anfrage neu (`CONN_MAX_AGE=0`) und kennt das Problem nicht. Messung (Debug-Build, `SELECT 1`, 3×3000,
  Test `dbconn_measure_acquire_overhead`): bisher 233–258 µs, neu 242–265 µs (Rauschen),
  `test_before_acquire(true)` 320–349 µs (+~90 µs je Abfrage) – deshalb kein Ping pro `acquire`. `acquire_timeout = PAPERLESS_DB_TIMEOUT` falls gesetzt. Host mit `/` →
  Unix-Socket. `application_name = paperless-rs` (überschreibbar per `PAPERLESS_DB_OPTIONS`).
* Redis: entfernt (2026-11-14). Kein `RedisHandle`, kein `AppState::redis()`/`celery()`.
* Dienste: `set_service(T)`/`service::<T>()` (typisierte Ablage, z. B. `tasks::Dispatcher`).
* Djangos `default`-Cache: `AppState::cache()` → [`cache::DjangoCache`] (siehe §2a).

## 2a. Django-Cache – `cache.rs` (W23)

Quelle: `paperless/settings/__init__.py` (`_CACHE_BACKEND`, `KEY_PREFIX`), Django 5.2
`core/cache/backends/{base,locmem,db}.py`.

* `cache::backend(&Config)`: `PAPERRUST_CACHE` (`local` | `postgres`, auch `postgresql`/`pg`/`db`/
  `database`/`shared`; alles andere → `local`). Fehlt es: `PAPERLESS_CACHE_BACKEND` =
  `…redis.RedisCache` oder `…db.DatabaseCache` → `Backend::Postgres`, jeder andere Name →
  `Backend::Local`. Standard `Backend::Local`. Redis wird nie verbunden. Wird einmal beim Aufbau des
  Zustands bestimmt (`AppStateInner::cache_backend`).
* `Backend::Postgres`: UNLOGGED-Tabelle `paperrust_cache (cache_key text PK, value bytea, expires
  timestamptz NULL)`, Migration `pl_core.0001_paperrust_cache` (`cache::migrations::ALL`, angewendet
  von `manage migrate` und beim Serverstart, wenn das Backend `postgres` ist). Nötig nur im
  Mehrprozessbetrieb (`serve --no-worker` + `serve --worker-only`, mehrere Instanzen); sonst teilen die
  Prozesse weder Login-Drossel noch TOTP-Replay-Sperre noch Invalidierungen (pl-server warnt).
* Schlüssel ohne Präfix übergeben (`cache.get("treenode_list")`), voller Schlüssel
  `make_key(KEY_PREFIX, key)` = `:1:<key>` (`KEY_PREFIX` fest leer; `PAPERLESS_REDIS_PREFIX` wird
  ignoriert).
* Werte sind Rohbytes; die Aufrufer kodieren wie Djangos Serializer (`int` als Ziffern, sonst
  `pickle.dumps(v, 5)`).
* Methoden (alle `async`, `Result<_, ApiError>`): `get`, `get_many`, `set(key, v, timeout)`,
  `add`, `touch`, `delete`, `delete_many`, `incr`, `compare_and_set`. `timeout: Option<u64>`:
  `None` = nie ablaufen, `Some(0)` = sofort abgelaufen. `cache::DEFAULT_TIMEOUT` = 300 für Djangos
  Standard. `Backend::Postgres`: SQL auf `paperrust_cache` (abgelaufene Zeilen zählen als fehlend; `set`/`add`
  per `INSERT … ON CONFLICT`, `incr`/`compare_and_set` atomar mit Zeilensperre `FOR UPDATE`).
* `Backend::Local`: **ein** `LocalCache` pro `AppState` (alle Crates teilen ihn), Semantik von
  `LocMemCache`: LRU (`get`/`set`/`incr` schieben nach hinten), `MAX_ENTRIES` 300, beim Einfügen
  eines neuen Schlüssels über der Grenze `_cull` (die ältesten `len // 3`; `CULL_FREQUENCY` 0 =
  leeren), `_has_expired` (`exp <= now`), `delete` auch abgelaufener Einträge → `true`,
  `incr` auf fehlendem Schlüssel → `Ok(None)` (Django `ValueError`).
* Aufrufer (W23 umgestellt): pl-objects `invalidate_treenode_cache`, pl-auth-api
  (TOTP-Replay-Sperre `allauth.mfa.totp.used?…` per `add`, Login-Throttle `throttle_login_<ident>`
  per `compare_and_set`, Session-Cache `DEL`), pl-accounts `cache::Cache` (Rate-Limits, Sperren),
  pl-ai (LLM-Vorschläge, `clear_document_caches`), pl-server (Dokument-Caches des Workers).
* HTTP: `reqwest::Client` ohne Redirect-Folgen (Proxy-tauglich), keine Auto-Dekompression.

## 3. Routen und Request-Kontext – `route.rs`, `request.rs`

* `ApiRoute` unverändert (+ `ApiRoute::new`).
* `drf_view(handler, allow)`: DRF setzt auf **jede** Antwort (auch 4xx) `Allow` und `Vary: Accept`
  (`APIView.default_response_headers`, zwei Renderer). `Allow` = vorhandene Methoden in
  `http_method_names`-Reihenfolge; ViewSets: `HEAD` wenn `GET`, `OPTIONS` immer
  (`ModelViewSet`-Liste `GET, POST, HEAD, OPTIONS`, Detail `GET, PUT, PATCH, DELETE, HEAD, OPTIONS`).
* `RequestContext`:
  * Pfad: `OriginalUri` (falls genestet), prozentdekodiert wie Django (`repercent_broken_unicode`),
    mit `FORCE_SCRIPT_NAME`-Präfix falls nicht vorhanden.
  * Host (`get_host`): `X-Forwarded-Host` **nur** bei `PAPERLESS_USE_X_FORWARD_HOST=true`, sonst
    `Host`, sonst HTTP/2-Authority, sonst Server-Adresse (+`X-Forwarded-Port` nur bei
    `PAPERLESS_USE_X_FORWARD_PORT`). Validierung `split_domain_port` + `ALLOWED_HOSTS`
    (`*`, `.domain`, exakt); ungültig → 400 mit Djangos HTML-Seite `Bad Request (400)`.
  * Schema: `SECURE_PROXY_SSL_HEADER` (erster Wert vor Komma, getrimmt, exakter Vergleich) **nur**
    wenn `PAPERLESS_PROXY_SSL_HEADER` gesetzt; sonst Extension `ServerScheme` bzw. `http`.
  * `build_absolute_uri()` = `scheme://host` + `escape_uri_path(path)` + `?` + `iri_to_uri(QUERY_STRING)`.
  * Sprache (`LocaleMiddleware`/`get_language_from_request`): Cookie `{prefix}django_language`,
    dann `Accept-Language` (q-sortiert), Abbildung über `get_supported_language_variant` auf
    paperless `LANGUAGES` (`de` → `de-de`, `pt` → `pt-br`), sonst `en-us`.
  * Der Kontext wird in den Request-Extensions gecacht (zweiter Extraktor-Aufruf ist billig).

## 4. Fehler – `error.rs`

Quelle: `rest_framework/exceptions.py`, `views.py::exception_handler/handle_exception`,
`django/views/defaults.py`.

| Konstruktor | Status | Körper |
|---|---|---|
| `not_authenticated(www)` | 401 + `WWW-Authenticate: www`; ohne Header 403 | `{"detail":"Authentication credentials were not provided."}` |
| `authentication_failed(msg, www)` | 401/403 wie oben | `{"detail":"Incorrect authentication credentials."}` bzw. Text |
| `permission_denied()` / `_msg(t)` | 403 | `{"detail":"You do not have permission to perform this action."}` |
| `not_found()` | 404 | `{"detail":"Not found."}` (DRF `NotFound`, DRF-`get_object_or_404` bei ungültigem pk) |
| `not_found_model("Document")` | 404 | `{"detail":"No Document matches the given query."}` (Django `get_object_or_404`, DRF ≥3.15 reicht Text durch) |
| `invalid_page()` | 404 | `{"detail":"Invalid page."}` |
| `method_not_allowed("DELETE")` | 405 | `{"detail":"Method \"DELETE\" not allowed."}` |
| `not_acceptable()` | 406 | `{"detail":"Could not satisfy the request Accept header."}` |
| `invalid_version()` | 406 | `{"detail":"Invalid version in \"Accept\" header."}` |
| `unsupported_media_type(ct)` | 415 | `{"detail":"Unsupported media type \"ct\" in request."}` |
| `throttled(Some(s))` | 429 + `Retry-After` | `Request was throttled. Expected available in N second(s).` |
| `parse_error(None)` / `json_parse_error(m)` | 400 | `Malformed request.` / `JSON parse error - m` |
| `validation(v)`, `field_error`, `non_field_error`, `validation_list`, `ValidationErrors` | 400 | `{"f":["m"]}`, `{"non_field_errors":["m"]}`, `["m"]` |
| `json(status, v)` | beliebig | z. B. paperless `{"error": "..."}` |
| `django_text(status, t)` | beliebig | `HttpResponse*(t)`, `text/html; charset=utf-8` |
| `django_bad_request()` | 400 | Djangos `ERROR_PAGE_TEMPLATE` "Bad Request (400)", `text/html` |
| `internal(err)` | 500 | Djangos "Server Error (500)"-Seite; `err` nur im Log (`tracing::error!`) |

`ApiError` ist ein `Box` (klein in `Result`). `From` für `sqlx::Error`, `sea_query::error::Error`,
`serde_json::Error` → `internal`. **Kein** automatisches `RowNotFound → 404`.

## 5. JSON und Header – `json.rs`, `route.rs`

Quelle: `rest_framework/renderers.py::JSONRenderer`, `response.py::rendered_content`.

* `Content-Type: application/json` (kein charset). Leerer Körper (204) → **kein** Content-Type.
* Kompakt `(',', ':')`, UTF-8 roh (`UNICODE_JSON`), ` `/` ` escaped, Floats wie Python
  `repr` (`1e-05`, `1e+16`, `1.0`), NaN nicht möglich (`serde_json::Value`).
* `Accept: application/json; indent=N` → Einrückung 1–8 mit `(',', ': ')` (`Drf::indent(ctx.negotiation.indent)`).
* Schlüsselreihenfolge = Einfügereihenfolge (`preserve_order`).
* **Header-Parität** (wer setzt was):
  * pl-core (`drf_view`): `Allow`, `Vary: Accept`; Fehler: `WWW-Authenticate`, `Retry-After`.
  * pl-authz/pl-server: `X-Api-Version: 10` und `X-Version: <paperless-Version>` für angemeldete
    Nutzer (`ApiVersionMiddleware`).
  * pl-server (Django-Middlewares, Reihenfolge der `process_response`): `X-Frame-Options: SAMEORIGIN`,
    `Vary` ergänzt um `Accept-Language` (LocaleMiddleware, immer), `origin` (corsheaders, immer),
    `Cookie` (wenn Session gelesen), `Accept-Encoding` (Kompression); `Content-Language`,
    `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin`, `Cross-Origin-Opener-Policy:
    same-origin` (SecurityMiddleware), CORS-Header, `Content-Length`. Mit `route::patch_vary` ergänzen.

## 6. Datum/Zeit – `datetime.rs`

* Ausgabe `format_datetime`: in `PAPERLESS_TIME_ZONE`, `isoformat()`, Mikrosekunden nur ≠ 0
  (6-stellig), Offset mit Sekunden wenn nötig (LMT), `+00:00` → `Z`. `format_date`: `YYYY-MM-DD`.
* Filter-Eingabe:
  * `DateFilter` (Modell-`DateField`): `strptime` mit `DATE_INPUT_FORMATS` der **aktiven Sprache**
    (Tabelle aus `django/conf/locale/*/formats.py`, generiert: `crates/pl-core/tools/`), z. B. en-us
    `%Y-%m-%d`, `%m/%d/%Y`, …; de-de `%d.%m.%Y`, `%d.%m.%y`, `%Y-%m-%d`. Fehler `Enter a valid date.`
  * `IsoDateTimeFilter` (Modell-`DateTimeField` in `rest_framework.FilterSet`): Django
    `parse_datetime` (Python `fromisoformat`, dann Regex), naive Werte in `TIME_ZONE`; mehrdeutig/
    nicht existent → `"<dt> couldn’t be interpreted in time zone <tz>; it may be ambiguous or it may not exist."`;
    sonst `Enter a valid date/time.`
  * `parse_datetime_input` für `DateTimeFilter` (ISO, dann Locale-Formate).
* Serializer-Eingabe: `drf_parse_datetime` (`Datetime has wrong format. Use one of these formats instead: YYYY-MM-DDThh:mm[:ss[.uuuuuu]][+HH:MM|-HH:MM|Z].`,
  mehrdeutig `Invalid datetime for the timezone "<tz>".`), `drf_parse_date` (`Date has wrong format. Use one of these formats instead: YYYY-MM-DD.`).

## 7. Versionierung – `version.rs`

Quelle: `rest_framework/negotiation.py`, `utils/mediatypes.py`, `versioning.py`; Renderer =
DRF-Standard (`JSONRenderer`, `BrowsableAPIRenderer`), `DEFAULT_VERSION "10"`, `ALLOWED_VERSIONS ["9","10"]`.

* `?format=json|api` filtert Renderer; anderes → 404 `Not found.`.
* Accept-Liste: Header (fehlt → `*/*`; leer → keine Einträge → 406), max. 8192 Zeichen/64 Token,
  Token > 256 übersprungen. Buckets nach Spezifität (3: mit Param ≠ q, 2: typ/sub, 1: typ/*, 0: */*),
  innerhalb: Renderer-Reihenfolge, dann Header-Reihenfolge. q-Werte werden (wie DRF) ignoriert.
* Accepted media type: Client-Typ selbst bzw. bei Wildcard `application/json;k=v…` mit den
  Client-Parametern. `version`-Parameter (Namen klein, Werte entquotet) → `"9"`/`"10"` exakt, sonst
  406 `Invalid version in "Accept" header.`.
* Browser-Accept (`text/html,…`) → `Renderer::BrowsableApi`: Rust rendert keine HTML-Seite →
  `version::prefers_browsable_api` für `pl-server` (an Django geben).

## 8. Paginierung – `pagination.rs`

Quelle: `src/paperless/views.py::StandardPagination`, DRF `PageNumberPagination`, Django `Paginator`.

* `page`: letzter Wert; leer → 1; `last` → letzte Seite; Python-`int()` (Leerraum, Vorzeichen, `_`);
  sonst/`<1`/`>num_pages` → 404 `Invalid page.`; `num_pages = max(1, ceil(count/size))`.
* `page_size`: positiv, gekappt auf 100000, sonst 25.
* `next`/`previous`: `replace_query_param`/`remove_query_param` auf `build_absolute_uri()` –
  `parse_qs(keep_blank_values)`, Schlüssel **sortiert**, `urlencode(doseq)` mit `quote_plus`
  (Leerzeichen `+`, `/` → `%2F`); `previous` auf Seite 1 ohne `page`.
* Körper: `count`, `next`, `previous`, `all` (nur v9, Aufrufer liefert IDs), `results`.
* `fetch_page`: **ein** Roundtrip `SELECT …, (SELECT COUNT(*) FROM (<q ohne ORDER/LIMIT>) AS __pl_count) AS __pl_total_count … LIMIT n OFFSET m`
  (unkorreliert → InitPlan). Leere Seite oder `page=last`/ungültig → separater COUNT.
  Zeilen enthalten Spalte `__pl_total_count` zusätzlich.
* `fetch_all_ids`: IDs aller Treffer in Sortierreihenfolge (Ausdruck wird zu `bigint` gecastet).

## 9. Filter und Sortierung – `filters.rs`

Quelle: django-filter 26.2, Django 5.2 PG-Backend, DRF `OrderingFilter`, `src/documents/filters.py`.

* Parametername `feld` (exact) / `feld__lookup`; Wert = letzter Query-Wert; Filterreihenfolge =
  Meta-Felder in Reihenfolge, deklarierte Filter ersetzen gleichnamige an Ort und Stelle oder
  werden angehängt. **Alle** Filter werden validiert; Fehler 400 `{"param": ["msg"]}` in
  Filterreihenfolge (je Filter erste Meldung).
* Zahl (`NumberFilter` → `forms.DecimalField`): Python-`Decimal` inkl. Exponent/`_`; nicht endlich
  oder ungültig → `Enter a number.`; leer → kein Filter. Integer-Spalte: `int(Decimal)` (abschneiden);
  außerhalb des Spaltenbereichs (`FieldKind::INT` = int4 …): `exact`/`gt`/`gte` bei Überlauf leer,
  `gt`/`gte` bei Unterlauf alles, `lt`/`lte` umgekehrt (`IntegerFieldOverflow`).
* `__in` (`BaseInFilter`/`BaseCSVWidget`): leer → kein Filter; Split an `,`; leere Einträge
  ignoriert (`None` wird aus `IN` entfernt); nur leere → `1 = 0`.
* `__isnull` und Bool-Felder: `BooleanWidget` (`1/0/true/false`, case-insensitive), sonst kein Filter.
* Text (`CharFilter`): `strip`, leer → kein Filter, NUL → `Null characters are not allowed.`.
  `exact` → `col = v`; `iexact` → `UPPER(CAST(col AS text)) = UPPER($v)`; `icontains`/`istartswith`/
  `iendswith` → `UPPER(CAST(col AS text)) LIKE UPPER($pattern)` mit `\`-Maskierung von `\ % _`
  (PostgreSQL-Standard-Escape `\`). Case-Folding macht PostgreSQL (`UPPER`), identisch zu Django.
* Datum: `DateField` lt/gt…; `__year/__month/__day` → `EXTRACT(… FROM col [AT TIME ZONE tz])`;
  `DateTimeField` `__date__gt…` → `(col AT TIME ZONE tz)::date`.
* `ordering`: letzter Wert, an `,` geteilt, getrimmt, `-` = absteigend, ungültige Felder ignoriert;
  keine gültigen → Standardsortierung des Aufrufers. Doppelte Felder: erster gewinnt (Django-Compiler).

## 10. Aufgaben – `tasks.rs` (bis 2026-11-14: Celery-Producer `celery.rs`)

Seit 2026-11-14 gibt es keinen Celery-Producer mehr: Aufgaben gehen ausschließlich über den
eingehängten `Dispatcher` (PaperRust-Warteschlange `paperrust_task_queue`, pl-queue). Erhalten und
nach `tasks.rs` umgezogen: `TaskRequest`, `record_task` (PaperlessTask-Zeile wie unten),
`TRACKED_TASKS`, `DEFAULT_QUEUE`, `trigger_source`, `tracked_task_type`, `determine_trigger_source`,
`extract_input_data`. Entfernt: `CeleryProducer`, `PICKLE_ONLY_TASKS`, `AppState::celery()`.
Der Rest dieses Abschnitts beschreibt den früheren Celery-Weg (Geschichte).

Quelle: `paperless/settings/__init__.py` (Task queue), `paperless/celery.py`, `paperless/signed_pickle.py`,
`documents/signals/handlers.py`, Celery `amqp.as_task_v2`, kombu `Producer.publish`/Redis-Transport.

**Ergebnis der Prüfung „nimmt der Worker JSON an?“: Ja.** `CELERY_TASK_SERIALIZER = "signed-pickle"`
gilt nur für vom Django-Prozess gesendete Nachrichten; `CELERY_ACCEPT_CONTENT = ["application/json",
"application/x-signed-pickle"]` erlaubt dem Worker JSON. `signed-pickle` ist als eigener
Content-Type `application/x-signed-pickle` registriert; die HMAC-Prüfung (`signed_pickle_loads`) läuft
nur für diesen Content-Type. JSON-Nachrichten sind unsigniert, können aber keinen Code ausführen.
Ergebnisse (`CELERY_RESULT_SERIALIZER = signed-pickle`) liest Rust nicht.

* Queue: keine `task_routes` → `celery`; Redis-Schlüssel `PAPERLESS_REDIS_PREFIX + "celery"`
  (kombu `global_keyprefix` gilt für `LPUSH`), Priorität 0 → kein `\x06\x16`-Suffix. Rust macht
  direkt `LPUSH` (entspricht kombu `_put` nach Exchange-Lookup `celery`/`celery`).
* Nachricht (kombu-Envelope, JSON mit `ensure_ascii`): `body` (Base64 von
  `[args, kwargs, {"callbacks":null,"errbacks":null,"chain":null,"chord":null}]`),
  `content-encoding: utf-8`, `content-type: application/json`, `headers` (`lang, task, id, shadow,
  eta, expires, group, group_index, retries, timelimit, root_id, parent_id, argsrepr, kwargsrepr,
  origin, ignore_result, replaced_task_nesting, stamped_headers, stamps` + eigene wie
  `trigger_source`), `properties` (`correlation_id, reply_to "", delivery_mode 2, delivery_info
  {exchange: celery, routing_key: celery}, priority 0, body_encoding base64, delivery_tag`).
  `eta`/`expires` als ISO in `TIME_ZONE` (`CELERY_TIMEZONE`).
* `PaperlessTask`: für `TRACKED_TASKS` vor dem `LPUSH` `INSERT … ON CONFLICT (task_id) DO NOTHING`
  in `documents_paperlesstask` (`status=pending`, `trigger_source` aus Header oder `manual`,
  `input_data` wie `_extract_input_data`, `acknowledged=false`, `owner_id NULL`, `date_created=now`).
  Fehler → nur Log (wie upstream).
* `consume_file` (Python-Objekte) wird abgelehnt (`ApiError::internal`) → Route bleibt Django.
* Rückgabe: Task-UUID.

## 11. Tests

`cargo test -p pl-core -j 2`: Paginierungs-URLs und -Fehler, Query-Kodierung (Python-`quote_plus`,
`parse_qsl`), Datumsausgabe/-parsing (ISO, Regex-Fallback, strptime, Locale, DST), Fehler-JSON
aller Konstruktoren, Negotiation/Version (406/404/HTML), Filter-SQL und -Fehler, Ordering,
Konfiguration (Defaults, URL/Hosts, DB, dotenv, entfernte Variablen → Hinweis), Request-Kontext
(Host, Proxy-Header, Script-Name, Sprache), Cache (lokal; `postgres` als `#[ignore]`-Test gegen
`PL_TEST_DB`). DB-Pfade (`fetch_page`) sind nicht unit-getestet (keine Referenz-DB zum Zeitpunkt); SQL-Aufbau ist getestet.

## Known differences

1. **Browsable API**: Requests, die DRF als HTML rendern würde, beantwortet Rust mit JSON (bis
   2026-11-14 konnte `pl-server` sie per `prefers_browsable_api` an Django geben; ohne Django entfällt das).
2. **Negotiation-Bucket-Reihenfolge**: Python iteriert ein `set` (hash-zufällig pro Prozess);
   Rust nimmt Header-Reihenfolge. Nur relevant bei mehreren gleich spezifischen Typen, die
   verschiedene Renderer treffen.
3. **Übersetzte Fehlertexte**: Django übersetzt DRF/Django-Meldungen in die aktive Sprache
   (z. B. Deutsch bei `Accept-Language: de`). pl-core liefert die englischen Originale.
   Paritätstests mit `Accept-Language: en` bzw. ohne Header fahren. (Offen: Übersetzungstabellen.)
4. **Python-`int()`/`\d` mit Nicht-ASCII-Ziffern** (z. B. arabisch-indische Ziffern) werden nicht
   als Zahl erkannt.
5. **`__year` außerhalb 1..9999**: Django wirft 500 (`year is out of range`), Rust liefert leeres Ergebnis.
6. **Pfade**: `Path.resolve()` löst Symlinks auf, Rust macht Pfade nur absolut.
7. ~~**Celery**~~ (Geschichte, Celery-Weg seit 2026-11-14 entfernt): kein `task-sent`-Event (`CELERY_TASK_SEND_SENT_EVENT`, nur für Monitoring wie Flower);
   kein Abo des Ergebnis-Kanals; `argsrepr` nutzt vereinfachtes Python-`repr` (nicht druckbare
   Unicode-Zeichen werden nicht escaped); kombu-JSON nutzt `', '`-Separatoren, Rust kompakt
   (für den Worker gleichwertig).
8. **`application_name`** der DB-Verbindung ist `paperless-rs` statt `paperless-ngx` (zur
   Unterscheidung in `pg_stat_activity`).
9. ~~Accept-Language-Parsing vereinfacht~~ – jetzt Djangos Regex-Port (`locale.rs`, aus pl-server).
11. **Cache** (2026-11-14): Standard ist prozesslokal (`LocMemCache`-Semantik); upstream nimmt ohne
    `DEBUG` Redis. Gemeinsamer Cache für mehrere Prozesse über `PAPERRUST_CACHE=postgres` (Tabelle
    statt Redis); `PAPERLESS_CACHE_BACKEND=…RedisCache` wird als `postgres` gelesen. Schlüsselpräfix
    fest leer (`PAPERLESS_REDIS_PREFIX` wirkungslos). Mehrere Prozesse mit lokalem Cache → pl-server
    warnt beim Start.
12. **Kein Django, kein Celery, kein Redis** (2026-11-14): `PAPERRUST_DJANGO_URL`,
    `PAPERRUST_QUEUE(_BACKEND)`, `PAPERLESS_REDIS(_PREFIX)` und die Redis-/Proxy-Zeitlimits werden
    ignoriert (Hinweis über `retired_warnings`, nie Startfehler).
10. **`fromisoformat`**: Wochen-Datumsformen und Basisformat unterstützt; exotische Formen
    (z. B. Ordinaldaten) wie CPython abgelehnt, aber nicht alle CPython-Sonderfälle geprüft.
13. **`safepath` unter Windows strenger** (Sicherheitsprüfung 2026-11-19 F12): `safe_join` lehnt unter
    Windows zusätzlich Teile mit `:` (Laufwerk/Alternate Data Stream `name:strom`), Gerätenamen (`CON`,
    `PRN`, `AUX`, `NUL`, `COM0-9`, `LPT0-9`, `CONIN$`, `CONOUT$`, auch mit Endung, z. B. `nul.pdf`) und
    Teile mit Punkt/Leerzeichen am Ende ab (`None` wie bei `..`). upstream (`ORIGINALS_DIR / Path(fname)`)
    prüft nichts. Unter Linux unverändert (gewöhnliche Namen wie bei `PosixPath`). Neu:
    `sanitize_component` (Bereinigung eines Namensteils statt Fehler; genutzt von der Arbeitskopie im
    Consumer), `is_windows_device_name`, `is_windows_reserved_component`.
14. **`PAPERLESS_TASK_WORKERS=auto`** (`resources.rs`, 2026-11-23): unset = 1 as upstream
    (`DEFAULT_TASK_WORKERS`; threads per worker then `max(cores / 1, 1)` = all cores, memory cap of
    `apply_ocr_resource_cap` unchanged). The value `auto` (case-insensitive, `is_auto`) is a PaperRust
    opt-in: `default_task_workers` = `min(ceil(cores / t), 4, budget / (t × 100 MB))`, at least 1
    (`t` = explicit `THREADS_PER_WORKER` or 2). upstream `int("auto")` fails at startup. Every other
    value is parsed with `int()` and keeps the ngx error text. Shared entry point
    `task_workers_from_env` (pl-core `Config`, pl-queue); pl-ocr parses the same way with its own
    `py_int`. History: 2026-11-22 the automatic value was the default for an unset variable; withdrawn
    2026-11-23 because a single document got slower (Raspberry Pi 5, 3 cores: 255 documents 214 s →
    162 s, but one 8-page scan alone 14.8 s → 24 s). Startup log: `task workers: N (default|auto: from
    cores and memory|set explicitly)`.

## API-Änderungen

* 2026-11-14 (ohne Django/Celery/Redis): Modul `celery` entfernt; `TaskRequest`, `record_task`,
  `TRACKED_TASKS`, `DEFAULT_QUEUE`, `trigger_source`, `tracked_task_type`, `determine_trigger_source`,
  `extract_input_data` jetzt in `tasks`; `CeleryProducer`, `PICKLE_ONLY_TASKS` entfernt.
  `send_task`/`prepare_task` ohne Dispatcher → Fehler. `AppState::redis()`/`celery()`,
  `RedisHandle`, `state::{REDIS_CONNECT_TIMEOUT, REDIS_RETRY_AFTER}` entfernt. Config-Felder
  `django_url`, `celery_redis_url`, `channels_redis_url`, `redis_prefix` und
  `config::{parse_redis_url, redis_rs_url}` entfernt; neu `config::retired_warnings`.
  `cache::Backend::{Local, Postgres}` (vorher `Redis`), `cache::{DATABASE_CACHE_BACKEND, KEY_PREFIX,
  migrations}`; neue Variable `PAPERRUST_CACHE`. Abhängigkeit `redis` entfernt.
* W23 (2026-11-08): neues Modul `cache` (`Backend`, `backend`, `make_key`, `LocalCache`,
  `DjangoCache`, `REDIS_CACHE_BACKEND`, `LOCMEM_CACHE_BACKEND`, `DEFAULT_TIMEOUT`),
  `AppState::cache()`, `AppStateInner::{cache_backend, local_cache}`, `RedisHandle::with_timeouts`,
  `state::{REDIS_CONNECT_TIMEOUT, REDIS_RETRY_AFTER}`; neue Variablen `PAPERRUST_REDIS_CONNECT_TIMEOUT`,
  `PAPERRUST_REDIS_RETRY_AFTER`. `pl_auth_api::django_cache::{Backend, backend, make_key}` sind
  jetzt Re-Exporte aus `pl_core::cache`.
* W11 (2026-11-08): `pyjson` aus pl-fields-views übernommen (Goldvektoren
  `tests/parity/pl-core/pyjson_vectors.py` → `src/pyjson_vectors.in`), `ApiError::json_body_parse_error(body,
  fallback)`; `pyset` aus pl-cli übernommen; `client::audit_actor`/`RequestUserSource` (Audit-Akteur wie
  `AuditlogMiddleware`); `dbconn` (tote Pool-Verbindungen nach DB-Neustart).
* Erste Veröffentlichung (Welle 0). `ApiRoute` unverändert; `AppState.inner.db` bleibt erhalten.
  `AppStateInner` ist jetzt `#[non_exhaustive]` (vorher nur `db`): Struct-Literale außerhalb von
  pl-core sind nicht mehr möglich → `AppState::new`/`with_pool` verwenden.
* Workspace: `redis` mit Feature `connection-manager` ergänzt; pl-core nutzt zusätzlich `url`.
* Integration (Koordinator): `markers` (einheitliche Delegation an Django, `Authenticated`,
  `RequestNotes`), `locale` (aus pl-server übernommen), Routen-Markierungen (`RouteFlags`, in einer
  prozessweiten Tabelle nach id, damit das `ApiRoute`-Struct-Literal stabil bleibt),
  `Config::bind` Standard `127.0.0.1:8080`, neue Config-Felder (Suche, Remote-OCR, Mail/OAuth),
  `PICKLE_ONLY_TASKS` += `process_cf_select_update`, Celery `prepare`/`record_task`/`publish`.

## Upstream-Fixes nach v3.4.2 (paperless-ngx `dev`)

- **00096e025 (#14735) – `ALLOWED_HOSTS`-Standard.** Quelle: `paperless/settings/__init__.py`
  (`_get_allowed_hosts`, `_parse_paperless_url`). Rust: `config::initial_allowed_hosts` und
  `config::allowed_host_from_url_hostname`, benutzt von `Config::from_env_map`, `pl-authz::AuthConfig`
  und `pl-server::config` (alle drei Lesestellen gleich).
  - `["*"]` nur, wenn weder `PAPERLESS_ALLOWED_HOSTS` gesetzt ist noch `PAPERLESS_URL` (nicht leer);
    sonst die Liste aus `PAPERLESS_ALLOWED_HOSTS` (fehlend/leer → leer) + `localhost`, `127.0.0.1`, `[::1]` (upstream 40598db44, #14846, `dev` after 3.5.1), dann der Host
    aus `PAPERLESS_URL`. v3.4.2: nur `PAPERLESS_URL` gesetzt → `["*", host]` (jeder Host erlaubt).
  - Hostname aus `PAPERLESS_URL`: kleingeschrieben, ein Punkt am Ende entfernt, IPv6 in `[]` (vorher
    ohne Klammern, passte nie zu `split_domain_port`); leer, `*` oder mit `.` beginnend → Startfehler
    „PAPERLESS_URL '…' has no valid hostname, expected a URL like https://example.com“ (vorher „has no
    hostname“).
  - Test: `config::tests::allowed_hosts_at_startup_upstream_00096e025` (alle neun Fälle von
    `test_allowed_hosts_at_startup` samt Annahme/Ablehnung über `host_allowed`, plus
    `test_paperless_url_without_valid_hostname_is_rejected`).

## Last-Befund B2 (2026-11-18): Nebenpools, LISTEN außerhalb des Pools

Quelle: tests/stress/BEFUNDE.md B2. `dbconn.rs`:

- `side_pool(pool, max)`: eigener, kleiner Pool mit den Verbindungsoptionen des Hauptpools
  (lazy, gleiche `before_acquire`-Ping-Regel, gleiches Acquire-Zeitlimit). Genutzt für die
  Steuerung der Aufgaben-Warteschlange (Claim/Heartbeat/Lease/Abschluss), den Sync-Sequenzer und
  Long-Poll – Arbeit, die nicht am allgemeinen Pool verhungern bzw. ihn nicht leerlaufen lassen darf.
- `listener(pool)`: `PgListener` auf einem eigenen Ein-Verbindungs-Pool (wie
  `PgListener::connect(url)`, aber mit den Optionen des Hauptpools). sqlx' `connect_with(pool)`
  behielt dauerhaft eine Verbindung des Hauptpools.
- `state::pool_options`: neu `PAPERRUST_DB_ACQUIRE_TIMEOUT` (Sekunden, Vorrang vor
  `PAPERLESS_DB_TIMEOUT`) für das Warten auf eine freie Pool-Verbindung. Poolgröße unverändert
  (`PAPERLESS_DB_POOLSIZE` / `PAPERRUST_DB_MAX_CONNECTIONS`, Standard 10).

Known difference: keine (Django hat keinen Pool).

## Speicher-Befund 2026-11-18: Pool-Verbindungen schließen

Quelle: Jetson-Benchmark `results/benchmark/2026-11-18-jetson` (PostgreSQL nach dem Einlesen 147 MiB PSS,
davon 95,6 MiB anon, gegen 50 / 11,6 MiB bei paperless-ngx). Ursache: jede offene Verbindung ist ein
PostgreSQL-Prozess, der nach Arbeit Katalog-/Plan-Caches und vorbereitete Anweisungen behält (~8 MiB);
Django schließt nach jeder Anfrage (`CONN_MAX_AGE=0`). Lokal nach dem Einlesen 11–12 Backends, die nie
wegfielen: sqlx gibt freie Verbindungen FIFO aus (`crossbeam ArrayQueue`), und die Sekunden-Schleifen
(Warteschlangen-Claim auf dem Nebenpool, Suchindex-Abgleich auf dem Hauptpool) benutzten reihum jede
Verbindung, sodass `idle_timeout` nie griff.

- `state::POOL_IDLE_TIMEOUT` = 60 s (vorher 600 s) für Haupt- und Nebenpools (`dbconn::side_pool`).
- `state::POOL_MAX_LIFETIME` = 300 s (sqlx-Standard 1800 s): eine Verbindung wird bei der Rückgabe nach
  spätestens 5 min geschlossen; danach entstehen nur so viele neue, wie gleichzeitig gebraucht werden.
- `LISTEN`-Verbindungen (`dbconn::listener`, vier Stück) bleiben dauerhaft (ohne Abfragen klein);
  Zusammenlegen auf eine Verbindung wäre ein weiterer Schritt (nicht umgesetzt).
- Kosten: nach > 60 s Pause bzw. alle 5 min je Verbindung ein Neuaufbau (lokal einige ms). Zahlen:
  Bericht des Speicher-Auftrags 2026-11-18.

Known difference: keine (Django hat keinen Pool).

## Last-Befund N4 (2026-11-18): Bindungstypen dynamischer Abfragen – `sqlbind.rs`

- sqlx sucht vorbereitete Anweisungen je Verbindung nur nach dem SQL-Text. Gleicher sea-query-Text mit
  `i32` an einer Stelle und `i64` an einer anderen (z. B. `SELECT "id", "name" FROM "documents_correspondent"
  WHERE "id" IN ($1)`) → 500 `incorrect binary data format in bind parameter 1` bzw. `insufficient data left in
  message` (tests/stress/BEFUNDE.md N4).
- `pl_core::sqlbind::SqlxBinder::build_sqlx` (Ersatz für `sea_query_binder::SqlxBinder`, gleiche Signatur) hängt
  ` /*t:<hash>*/` an: Hash über Anzahl und Art der gebundenen Werte (keine Inhalte). Ohne Werte bleibt der Text
  unverändert. Gleiche Typen teilen weiter eine vorbereitete Anweisung; der Statement-Cache bleibt an
  (`statement_cache_capacity(0)` war nicht nötig).
- Wächter `tests/sqlbind_guard.rs`: kein `sea_query_binder::SqlxBinder`/`build_any_sqlx` in `crates/*/{src,tests}`
  außer `sqlbind.rs`. Beleg-Test `tests/sqlbind_db.rs` (`#[ignore]`, Wegwerf-DB `pl_sqlbind_test`).
- Nicht abgedeckt: mit `format!` gebaute Texte mit `.bind(…)` und statische Texte (dafür
  `pl-documents-write/tests/sql_cache_guard.rs`); Regel dort: `integer` → `i32`, `bigint` → `i64`.

## Ausfall-Befund C1 (tests/chaos, 2026-11-18): Datenbank nicht erreichbar

Vorher hing bei einem PostgreSQL-Ausfall **jede** Anfrage bis zur Pool-Zeitgrenze (30 s) und endete mit
„Server Error (500)“ bzw. `{"detail": "A server error occurred."}`. Jetzt (`dbconn.rs`):
- `is_unavailable(e)` (Verbindungsfehler, `PoolTimedOut`, `PoolClosed`) → `From<sqlx::Error> for ApiError`
  liefert `ApiError::db_unavailable()`: 503 `{"detail": "The database is temporarily not reachable. Please
  try again in a moment."}` + `Retry-After: 5` (pl-authz `AuthError::Unavailable` ebenso).
- `note_error` markiert die DB als „weg“ (`mark_down`/`is_down`), auch fehlgeschlagene Pings in
  `before_acquire`; `watch_health` (von pl-server gestartet) prüft dann jede Sekunde mit eigener Verbindung
  und hebt die Markierung auf. Solange „weg“: pl-server antwortet sofort (Middleware `dbcheck::unavailable_middleware`).
- `retry_stale_once` wartet beim zweiten Versuch höchstens `RETRY_LIMIT` (3 s) statt 30 s.
Bewusste Abweichung: upstream antwortet mit 500 (`OperationalError`).
