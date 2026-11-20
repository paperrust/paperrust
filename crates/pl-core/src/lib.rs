//! # pl-core – shared core of PaperRust
//!
//! Everything every domain crate needs to behave **like Django/DRF in paperless-ngx v3.4.2**.
//! Contract, sources and known deviations: `docs/contract/pl-core.md`.
//!
//! | Module | Content | Upstream / model |
//! |---|---|---|
//! | [`brand`] | Name, version, repo URL, user agent, acknowledgment of PaperRust ([`brand::PAPERRUST_VERSION`]) | docs/BRANDING.md |
//! | [`config`] | [`Config::from_env`] – all backend-relevant `PAPERLESS_*` variables (primary name `PAPERRUST_*`, [`config::var`]) | `src/paperless/settings/` |
//! | [`state`] | [`AppState`] (pool, HTTP client, time zone, cache, plugged-in services such as queue/event bus) | – |
//! | [`route`] | [`ApiRoute`], [`route::drf_view`] (Allow/Vary like DRF) | `rest_framework.views.APIView` |
//! | [`error`] | [`ApiError`] in DRF format, [`ValidationErrors`] | `rest_framework.exceptions` |
//! | [`json`] | JSON like `JSONRenderer` ([`Drf`], [`json::to_drf_json`]) | `rest_framework.renderers.JSONRenderer` |
//! | [`datetime`] | Date/time output like DRF, input parsing like Django/django-filter | `rest_framework.fields`, `django.forms.fields` |
//! | [`version`] | Content negotiation + `AcceptHeaderVersioning` (406) | `rest_framework.negotiation`, `.versioning` |
//! | [`request`] | [`RequestContext`] (absolute URI like `build_absolute_uri`, language, version) | `django.http.request` |
//! | [`query`] | [`QueryParams`] like Django `QueryDict` | `django.http.QueryDict` |
//! | [`urlenc`] | `quote`/`parse_qsl`/`replace_query_param` like Python/DRF | `urllib.parse`, `rest_framework.utils.urls` |
//! | [`pagination`] | `StandardPagination` + page and COUNT in one round trip | `src/paperless/views.py::StandardPagination` |
//! | [`filters`] | django-filter lookups, `OrderingFilter` | `django_filters`, `rest_framework.filters.OrderingFilter` |
//! | [`markers`] | "Case not supported" (404 as without a route), `Authenticated`, request notes for pl-server | `ApiVersionMiddleware`, `SessionMiddleware` |
//! | [`locale`] | Language negotiation like `LocaleMiddleware` | `django.utils.translation.trans_real` |
//! | [`body`] | Body limit like `DATA_UPLOAD_MAX_MEMORY_SIZE`, [`body::read_body`] | `django.http.request` |
//! | [`dbconn`] | Detect dead pool connections after a DB restart, retry once | Django `CONN_MAX_AGE=0`/`CONN_HEALTH_CHECKS` |
//! | [`pyjson`] | CPython `json` error texts for invalid JSON bodies | `json.decoder`, `Modules/_json.c` |
//! | [`pyset`] | Iteration order of a CPython `set` (m2m insertion order like Django) | `Objects/setobject.c`, `related_descriptors._add_items` |
//! | [`client`] | Client address/port for the audit log (valid values only) | `auditlog.middleware` |
//! | [`cache`] | Django's `default` cache: Redis (shared with Django) or process-local like `LocMemCache` ([`AppState::cache`]) | `django.core.cache.backends.redis`/`.locmem` |
//! | [`celery`] | Celery producer (protocol v2, JSON) + `PaperlessTask` entry | `celery.app.amqp`, `kombu`, `documents/signals/handlers.py` |
//! | [`safepath`] | `safe_join` for stored file names (separators like `pathlib`, containment) | `Document.source_path` |
//! | [`sqlbind`] | `build_sqlx` with type signature in the SQL text (statement cache per connection, FINDINGS B3/N4) – use instead of `sea_query_binder::SqlxBinder` | sqlx-postgres `get_or_prepare` |
//! | [`tasks`] | Task dispatch: [`tasks::send_task`] via the plugged-in [`tasks::Dispatcher`] (PgQueue), [`tasks::TaskRequest`], `PaperlessTask` entry | `Task.apply_async`, `documents/signals/handlers.py` |
//!
//! ## Rules for handlers (short version)
//!
//! * Extractor order like DRF: first [`RequestContext`] (negotiation/version → 406),
//!   then authentication (`pl-authz`), then permissions.
//! * Respond with [`Drf`] / [`json::drf_response`], errors with [`ApiError`].
//! * Wrap the route with [`route::drf_view`] so that `Allow` and `Vary: Accept` are correct.
//! * No panics, no internals in error texts ([`ApiError::internal`] logs and responds
//!   like Django with the generic 500 page).

pub mod body;
pub mod brand;
pub mod cache;
pub mod client;
pub mod config;
pub mod datetime;
pub mod dbconn;
pub mod error;
pub mod filters;
pub mod json;
/// Continuously updated configuration (settings from the UI, docs/SETTINGS.md).
pub mod live;
pub mod locale;
// `LANGUAGES` there is generated; `locale::LANGUAGES` is authoritative (equality checked by a test).
#[allow(dead_code)]
mod locale_formats;
pub mod markers;
pub mod pagination;
pub mod pyjson;
pub mod pyrepr;
pub mod pyset;
pub mod query;
pub mod request;
pub mod requireauth;
pub mod resources;
pub mod route;
pub mod safepath;
pub mod sqlbind;
pub mod state;
pub mod tasks;
pub mod urlenc;
pub mod version;
pub mod vision;

pub use config::Config;
pub use error::{ApiError, ApiResult, ValidationErrors};
pub use json::Drf;
pub use markers::{Authenticated, DelegateToDjango, DjangoHandlerResponse, RequestNotes, delegate_to_django};
pub use query::QueryParams;
pub use request::RequestContext;
pub use route::{ApiRoute, RouteFlags};
pub use state::{AppState, AppStateInner, Services};
pub use version::{ApiVersion, Negotiation};

/// Re-exports, so that domain crates are guaranteed to use the same versions.
pub use chrono;
pub use chrono_tz;
pub use sea_query;
pub use sea_query_binder;
pub use serde_json;
pub use sqlx;
