//! Schema registry of all system settings (docs/SETTINGS.md section 1, inventory).
//!
//! Every key appears exactly once in [`SCHEMA`] with class (A/B/C/D/X), effect, type, default
//! and – for fields of `paperless_applicationconfiguration` – the AC column name. The canonical
//! key is the full variable name (`PAPERLESS_EMAIL_HOST`, `PAPERRUST_LOG`) or, for
//! desktop files, the path (`desktop.server.port`, `app.mode`). The count per class is checked by the
//! test `inventory_complete` against docs/SETTINGS.md §1.19.

use serde::Serialize;

/// Class (E1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
pub enum Class {
    /// GUI, effective live.
    A,
    /// GUI, effective after restart (subsystem or server).
    B,
    /// Bootstrap (needed before the DB) – environment/file/`desktop.toml` only.
    C,
    /// Deliberately environment only (trust boundary to the host, E5).
    D,
    /// No effect in PaperRust (mixed operation/upstream Docker) or internal; display only.
    X,
}

impl Class {
    /// Writable in the GUI (rule N or rule AC)?
    pub fn gui_writable(self) -> bool {
        matches!(self, Class::A | Class::B)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Class::A => "A",
            Class::B => "B",
            Class::C => "C",
            Class::D => "D",
            Class::X => "X",
        }
    }
}

/// Effectiveness (column "Effective").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// `L` – next request/task.
    Live,
    /// `W` – subsystem in the process restarts (worker pool, scheduler, folder watching).
    Worker,
    /// `S` – restart of the server process.
    Server,
    /// `1` – first start only.
    Once,
    /// `–` – no effect in PaperRust.
    None,
}

/// Where the value comes from today (column "Source", simplified to the primary source).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Store {
    /// Environment/`paperrust.conf` or `paperless.conf` (`E`), possibly set by the desktop supervisor (`DE`).
    Env,
    /// Docker-Entrypoint (`DI`).
    Entrypoint,
    /// `desktop.toml` of the supervisor (`DT`).
    DesktopToml,
    /// `app.toml` of the desktop shell (`AT`).
    AppToml,
    /// Currently fixed in code (`K`), now configurable.
    Code,
}

/// Category = subsection of docs/SETTINGS.md §1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Database,
    Paths,
    Network,
    Auth,
    Email,
    MailFetch,
    Consumer,
    Barcodes,
    Ocr,
    Documents,
    Tasks,
    Integrity,
    Logging,
    Branding,
    Ai,
    Webhooks,
    Desktop,
}

impl Category {
    pub const ALL: [Category; 17] = [
        Category::Database,
        Category::Paths,
        Category::Network,
        Category::Auth,
        Category::Email,
        Category::MailFetch,
        Category::Consumer,
        Category::Barcodes,
        Category::Ocr,
        Category::Documents,
        Category::Tasks,
        Category::Integrity,
        Category::Logging,
        Category::Branding,
        Category::Ai,
        Category::Webhooks,
        Category::Desktop,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Database => "database",
            Category::Paths => "paths",
            Category::Network => "network",
            Category::Auth => "auth",
            Category::Email => "email",
            Category::MailFetch => "mail_fetch",
            Category::Consumer => "consumer",
            Category::Barcodes => "barcodes",
            Category::Ocr => "ocr",
            Category::Documents => "documents",
            Category::Tasks => "tasks",
            Category::Integrity => "integrity",
            Category::Logging => "logging",
            Category::Branding => "branding",
            Category::Ai => "ai",
            Category::Webhooks => "webhooks",
            Category::Desktop => "desktop",
        }
    }
}

/// Shape of a JSON value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonShape {
    Any,
    Object,
    Array,
}

/// Type of a value (§2.4). What is stored is always the canonical env string (E7).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ty {
    /// `str_to_bool` (`true/1/t/y/yes`, `false/0/f/n/no`).
    Bool,
    /// Switch `on`/`off` (plus the `str_to_bool` values), PaperRust's own switches.
    OnOff,
    /// Python `int()`; bounds inclusive.
    Int {
        min: Option<i64>,
        max: Option<i64>,
    },
    /// Python `float()`; lower bound inclusive.
    Float {
        min: Option<f64>,
    },
    Str,
    /// Comma-separated list (`get_list_from_env`).
    List,
    Json {
        shape: JsonShape,
    },
    /// Fixed choice.
    Choice {
        choices: &'static [&'static str],
    },
    Path,
    Url,
    /// Celery crontab `m h dom mon dow` or `disable`.
    Cron,
    /// IANA time zone.
    TimeZone,
}

const fn int(min: Option<i64>, max: Option<i64>) -> Ty {
    Ty::Int { min, max }
}
const INT0: Ty = int(Some(0), None);
const INT1: Ty = int(Some(1), None);
const PORT: Ty = int(Some(1), Some(65535));
const FLOAT: Ty = Ty::Float { min: None };
const FLOAT0: Ty = Ty::Float { min: Some(0.0) };
const STR: Ty = Ty::Str;
const BOOL: Ty = Ty::Bool;
const LIST: Ty = Ty::List;
const PATH: Ty = Ty::Path;
const URL: Ty = Ty::Url;
const CRON: Ty = Ty::Cron;
const JSON_OBJ: Ty = Ty::Json { shape: JsonShape::Object };
const JSON_ARR: Ty = Ty::Json { shape: JsonShape::Array };
const fn choice(choices: &'static [&'static str]) -> Ty {
    Ty::Choice { choices }
}

/// A setting.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SettingDef {
    /// Canonical name (full variable or desktop path).
    pub key: &'static str,
    /// Key whose older/alternative name this entry is (separate entry because it is a
    /// separate variable; e.g. `PAPERRUST_QUEUE_BACKEND` → `PAPERRUST_QUEUE`).
    pub alias_of: Option<&'static str>,
    pub category: Category,
    pub class: Class,
    pub effect: Effect,
    /// `+I`: after a change rebuild index/filenames.
    pub reindex: bool,
    /// Secret (write-only; encrypted in `paperrust_setting`).
    pub secret: bool,
    pub ty: Ty,
    /// Default as canonical env string (`None` = not set or derived, see `help`).
    pub default: Option<&'static str>,
    /// Column in `paperless_applicationconfiguration` (rule AC, E3).
    pub ac_field: Option<&'static str>,
    pub store: Store,
    /// Internal, not shown in the GUI (⁴).
    pub internal: bool,
    /// Short description (detail: docs/user/configuration.md).
    pub help: &'static str,
}

impl SettingDef {
    const fn new(
        category: Category,
        key: &'static str,
        class: Class,
        effect: Effect,
        ty: Ty,
        default: Option<&'static str>,
        help: &'static str,
    ) -> Self {
        Self {
            key,
            alias_of: None,
            category,
            class,
            effect,
            reindex: false,
            secret: false,
            ty,
            default,
            ac_field: None,
            store: Store::Env,
            internal: false,
            help,
        }
    }
    const fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
    const fn reindex(mut self) -> Self {
        self.reindex = true;
        self
    }
    const fn ac(mut self, field: &'static str) -> Self {
        self.ac_field = Some(field);
        self
    }
    const fn store(mut self, s: Store) -> Self {
        self.store = s;
        self
    }
    const fn internal(mut self) -> Self {
        self.internal = true;
        self
    }
    const fn alias_of(mut self, k: &'static str) -> Self {
        self.alias_of = Some(k);
        self
    }

    /// Is the value also read by Django (mixed-operation hint "does not affect
    /// Django paths", §2.5 no. 8)?
    pub fn django_consumer(&self) -> bool {
        self.key.starts_with(pl_core::config::UPSTREAM_PREFIX) && self.class.gui_writable()
    }

    /// Is the key an environment variable (otherwise desktop file)?
    pub fn is_env(&self) -> bool {
        !matches!(self.store, Store::DesktopToml | Store::AppToml)
    }

    /// English short description ([`crate::help_en`]); if it is missing, the German `help`.
    pub fn help_en(&self) -> &'static str {
        crate::help_en::help_en(self.key).unwrap_or(self.help)
    }

    /// Displayed name: `PAPERRUST_X` for the canonical key `PAPERLESS_X`, otherwise the
    /// key itself. The canonical key stays `PAPERLESS_X` because stored values
    /// (`paperrust_setting.key`, encryption of the secrets) depend on it; `PAPERLESS_X`
    /// remains valid as an alias ([`Self::aliases`]).
    pub fn name(&self) -> std::borrow::Cow<'static, str> {
        display_name(self.key)
    }

    /// Further valid names besides [`Self::name`] (e.g. `PAPERLESS_X` as the former name).
    pub fn aliases(&self) -> Vec<String> {
        let name = self.name();
        self.env_names().into_iter().filter(|n| *n != name).collect()
    }

    /// Documentation anchor (`/rs/docs/configuration#NAME`).
    pub fn doc_anchor(&self) -> String {
        format!("/rs/docs/configuration#{}", self.name())
    }

    /// Other valid variable names (PAPERRUST_X for PAPERLESS_X, PAPERLESS_RS_X for own ones).
    pub fn env_names(&self) -> Vec<String> {
        if self.is_env() { pl_core::config::candidates(self.key) } else { Vec::new() }
    }
}

use Category as K;
use Class::{A, B, C, D, X};
use Effect::{Live as L, None as N, Once as O, Server as S, Worker as W};

const fn s(
    cat: Category,
    key: &'static str,
    class: Class,
    eff: Effect,
    ty: Ty,
    def: Option<&'static str>,
    help: &'static str,
) -> SettingDef {
    SettingDef::new(cat, key, class, eff, ty, def, help)
}

const COLOR: &[&str] = &["LeaveColorUnchanged", "RGB", "UseDeviceIndependentColor", "Gray", "CMYK"];

/// Complete inventory (docs/SETTINGS.md §1.1–1.16a). Order = documentation.
pub static SCHEMA: &[SettingDef] = &[
    // ---- 1.1 Database, secret key, startup ----
    s(K::Database, "PAPERLESS_CONFIGURATION_PATH", C, S, PATH, None, "Pfad der Konfigurationsdatei"),
    s(K::Database, "PAPERLESS_DBENGINE", C, S, STR, None, "DB-Art (nur postgresql)"),
    s(K::Database, "PAPERLESS_DBHOST", C, S, STR, None, "DB-Host"),
    s(K::Database, "PAPERLESS_DBPORT", C, S, PORT, Some("5432"), "DB-Port"),
    s(K::Database, "PAPERLESS_DBNAME", C, S, STR, Some("paperless"), "DB-Name"),
    s(K::Database, "PAPERLESS_DBUSER", C, S, STR, Some("paperless"), "DB-Nutzer"),
    s(K::Database, "PAPERLESS_DBPASS", C, S, STR, Some("paperless"), "DB-Passwort").secret(),
    s(
        K::Database,
        "PAPERLESS_DBSSLMODE",
        C,
        S,
        choice(&["disable", "allow", "prefer", "require", "verify-ca", "verify-full"]),
        Some("prefer"),
        "TLS-Modus der DB-Verbindung",
    ),
    s(K::Database, "PAPERLESS_DBSSLROOTCERT", C, S, PATH, None, "CA-Datei"),
    s(K::Database, "PAPERLESS_DBSSLCERT", C, S, PATH, None, "Client-Zertifikat"),
    s(K::Database, "PAPERLESS_DBSSLKEY", C, S, PATH, None, "Client-Schlüssel"),
    s(K::Database, "PAPERLESS_DB_OPTIONS", C, S, STR, None, "zusätzliche Verbindungsoptionen k=v,k=v"),
    s(K::Database, "PAPERLESS_DB_POOLSIZE", C, S, INT1, None, "Poolgröße"),
    s(K::Database, "PAPERLESS_DB_TIMEOUT", C, S, INT0, None, "Verbindungs-Timeout (s)"),
    s(K::Database, "PAPERRUST_DB_MAX_CONNECTIONS", C, S, INT1, Some("10"), "Poolgröße ohne DB_POOLSIZE"),
    // Load findings B2 (2026-11-18): wait time for a pool connection, side pool sync long poll
    s(
        K::Database,
        "PAPERRUST_DB_ACQUIRE_TIMEOUT",
        C,
        S,
        INT1,
        Some("30"),
        "Wartezeit auf eine freie DB-Verbindung (s), Vorrang vor DB_TIMEOUT",
    ),
    s(
        K::Database,
        "PAPERRUST_DB_LONGPOLL_CONNECTIONS",
        C,
        S,
        INT1,
        Some("8"),
        "eigene DB-Verbindungen für wartende Sync-Clients (Long-Poll)",
    ),
    s(K::Database, "PAPERRUST_DB_REQUIRE_AUTH", D, S, STR, None, "erlaubte DB-Anmeldeverfahren"),
    s(K::Database, "PAPERLESS_SECRET_KEY", C, S, STR, None, "Sitzungs-Schlüssel, Basis der Settings-Verschlüsselung").secret(),
    // Redis (Celery broker/cache in paperless-ngx): PaperRust no longer uses it (2026-11-14).
    s(K::Database, "PAPERLESS_REDIS", X, N, URL, None, "Redis-URL von paperless-ngx (wirkungslos, Redis wird nicht mehr gebraucht)")
        .secret(),
    s(K::Database, "PAPERLESS_REDIS_PREFIX", X, N, STR, None, "Schlüsselpräfix Redis (wirkungslos)"),
    s(
        K::Database,
        "PAPERRUST_CACHE",
        B,
        S,
        choice(&["local", "postgres"]),
        Some("local"),
        "Cache: lokal oder gemeinsam in PostgreSQL (Mehrprozessbetrieb)",
    ),
    s(K::Database, "PAPERRUST_INSTANCE_ID", B, S, STR, None, "Instanzkennung in /__rs/health"),
    s(K::Database, "PAPERRUST_INIT", C, O, BOOL, Some("yes"), "Entrypoint-Init (Migration, Superuser) ausführen").store(Store::Entrypoint),
    s(K::Database, "PAPERRUST_INIT_DB_TIMEOUT", C, O, INT0, Some("300"), "Warten auf die DB im Entrypoint (s)").store(Store::Entrypoint),
    s(K::Database, "PAPERLESS_ADMIN_USER", C, O, STR, None, "Superuser beim ersten Start anlegen"),
    s(K::Database, "PAPERLESS_ADMIN_PASSWORD", C, O, STR, None, "Passwort des ersten Superusers").secret(),
    s(K::Database, "PAPERLESS_ADMIN_MAIL", C, O, STR, Some("root@localhost"), "E-Mail des ersten Superusers"),
    s(K::Database, "PAPERLESS_DEBUG", D, S, BOOL, Some("no"), "Debug-Modus"),
    s(K::Database, "PAPERLESS_DB_READ_CACHE_ENABLED", X, N, BOOL, Some("no"), "Django-Lesecache (cachalot)"),
    s(K::Database, "PAPERLESS_READ_CACHE_TTL", X, N, INT0, Some("3600"), "TTL des Django-Lesecaches"),
    s(K::Database, "PAPERLESS_READ_CACHE_REDIS_URL", X, N, URL, None, "Redis des Django-Lesecaches"),
    s(
        K::Database,
        "PAPERLESS_CACHE_BACKEND",
        X,
        N,
        STR,
        None,
        "Django-Cache-Klasse (RedisCache/DatabaseCache wirken wie PAPERRUST_CACHE=postgres)",
    ),
    s(K::Database, "PAPERLESS_CHANNELS_BACKEND", X, N, STR, None, "Django-Channels-Klasse"),
    s(K::Database, "PAPERLESS_APPS", X, N, LIST, None, "zusätzliche Django-Apps"),
    // ---- 1.2 Paths ----
    s(K::Paths, "PAPERRUST_BASE_DIR", C, S, PATH, None, "Installationswurzel (Standard: Arbeitsverzeichnis)"),
    s(K::Paths, "PAPERLESS_DATA_DIR", C, S, PATH, None, "Daten (Index, Modell, Logs; Standard BASE/data)"),
    s(K::Paths, "PAPERLESS_MEDIA_ROOT", C, S, PATH, None, "Originale/Archiv/Vorschauen (Standard BASE/media)"),
    s(K::Paths, "PAPERLESS_CONSUMPTION_DIR", C, S, PATH, None, "Eingangsordner (Standard BASE/consume)"),
    s(K::Paths, "PAPERLESS_SCRATCH_DIR", C, S, PATH, None, "Arbeitsordner (Standard /tmp/paperrust)"),
    s(K::Paths, "PAPERLESS_STATICDIR", C, S, PATH, None, "statische Dateien (Standard BASE/static)"),
    s(K::Paths, "PAPERLESS_LOGGING_DIR", C, S, PATH, None, "Logordner (Standard DATA/log)"),
    s(K::Paths, "PAPERLESS_TRASH_DIR", C, S, PATH, None, "Ordner für gelöschte Dateien (leer = löschen)"),
    s(K::Paths, "PAPERLESS_EMPTY_TRASH_DIR", C, S, PATH, None, "alter Name von TRASH_DIR").alias_of("PAPERLESS_TRASH_DIR"),
    s(K::Paths, "PAPERLESS_MODEL_FILE", C, S, PATH, None, "Pfad des upstream-Klassifikators"),
    s(K::Paths, "PAPERRUST_CLASSIFIER_MODEL_FILE", C, S, PATH, None, "Pfad des Rust-Klassifikators"),
    s(K::Paths, "PAPERRUST_ROUTES", C, S, PATH, Some("./routes.toml"), "routes.toml"),
    s(K::Paths, "PAPERRUST_TESSERACT_PATH", C, S, PATH, None, "tesseract-Programm (Standard aus PATH)"),
    s(K::Paths, "PAPERRUST_TESSERACT_LIB", C, S, STR, None, "libtesseract (Datei/Ordner oder off)"),
    s(K::Paths, "PAPERRUST_TESSDATA_DIR", C, S, PATH, None, "tessdata-Verzeichnis"),
    s(K::Paths, "PAPERRUST_PDFIUM_PATH", C, S, PATH, None, "PDFium-Bibliothek"),
    s(K::Paths, "PAPERRUST_LIGHT_UI_DIR", C, S, PATH, None, "Dateien der Light-UI (Standard STATICDIR/../light)"),
    s(
        K::Paths,
        "PAPERLESS_THUMBNAIL_FONT_NAME",
        B,
        L,
        PATH,
        Some("/usr/share/fonts/liberation/LiberationSerif-Regular.ttf"),
        "Schrift für Text-Vorschaubilder",
    ),
    s(K::Paths, "PAPERLESS_EMAIL_CERTIFICATE_LOCATION", B, W, PATH, None, "CA-Datei für IMAP"),
    // ---- 1.3 Network, web server, proxy, CORS/CSRF ----
    s(K::Network, "PAPERRUST_BIND", C, S, STR, Some(pl_core::config::DEFAULT_BIND), "Adresse:Port des Servers"),
    s(K::Network, "PAPERLESS_BIND_ADDR", C, S, STR, Some("::"), "upstream-Bindeadresse (Entrypoint baut PAPERRUST_BIND)")
        .store(Store::Entrypoint),
    s(K::Network, "PAPERLESS_PORT", C, S, PORT, Some("8000"), "upstream-Port (Entrypoint baut PAPERRUST_BIND)").store(Store::Entrypoint),
    s(K::Network, "PAPERLESS_WEBSERVER_WORKERS", X, N, INT1, Some("1"), "granian-Prozesse (upstream-Image)").store(Store::Entrypoint),
    s(K::Network, "PAPERRUST_DJANGO_URL", X, N, STR, None, "entfernt: Weiterleitung an Django gibt es nicht mehr (gesetzt → Warnung)"),
    s(K::Network, "PAPERRUST_STOP_ON_STDIN_EOF", C, S, BOOL, Some("no"), "Ende bei stdin-EOF (Supervisor)"),
    s(K::Network, "PAPERLESS_FORCE_SCRIPT_NAME", C, S, STR, None, "Unterpfad hinter einem Proxy"),
    s(K::Network, "PAPERLESS_URL", B, S, URL, None, "öffentliche Adresse (CSRF, Links, OAuth)"),
    s(K::Network, "PAPERLESS_ALLOWED_HOSTS", B, L, LIST, Some("*"), "erlaubte Host-Köpfe"),
    s(K::Network, "PAPERLESS_CSRF_TRUSTED_ORIGINS", B, L, LIST, None, "vertrauenswürdige Origins"),
    s(K::Network, "PAPERLESS_CORS_ALLOWED_HOSTS", B, L, LIST, Some("http://localhost:8000"), "CORS-Origins"),
    s(K::Network, "PAPERLESS_COOKIE_PREFIX", B, S, STR, Some(""), "Präfix von Sitzungs-/CSRF-Cookie"),
    s(K::Network, "PAPERLESS_SESSION_COOKIE_AGE", A, L, INT1, Some("1814400"), "Sitzungsdauer (s)"),
    s(K::Network, "PAPERLESS_LOGOUT_REDIRECT_URL", A, L, STR, None, "Ziel nach Abmeldung"),
    s(K::Network, "PAPERLESS_STATIC_URL", X, N, STR, None, "URL statischer Dateien"),
    s(K::Network, "PAPERLESS_ENABLE_COMPRESSION", A, L, BOOL, Some("yes"), "Antwortkompression"),
    s(K::Network, "PAPERLESS_TOKEN_THROTTLE_RATE", A, L, STR, Some("5/min"), "Login-/Token-Drosselung (n/min)"),
    s(K::Network, "PAPERLESS_TRUSTED_PROXIES", D, S, LIST, None, "vertrauenswürdige Proxys"),
    s(K::Network, "PAPERLESS_ALLAUTH_TRUSTED_PROXY_COUNT", D, S, INT0, None, "Proxy-Anzahl für die Client-IP"),
    s(K::Network, "PAPERLESS_ALLAUTH_TRUSTED_CLIENT_IP_HEADER", D, S, STR, None, "Kopf mit der Client-IP"),
    s(K::Network, "PAPERLESS_USE_X_FORWARD_HOST", D, S, BOOL, Some("false"), "X-Forwarded-Host vertrauen"),
    s(K::Network, "PAPERLESS_USE_X_FORWARD_PORT", D, S, BOOL, Some("false"), "X-Forwarded-Port vertrauen"),
    s(K::Network, "PAPERLESS_PROXY_SSL_HEADER", D, S, JSON_ARR, None, "Kopf für „ist HTTPS“"),
    s(K::Network, "PAPERRUST_DOCS_URL", A, L, URL, None, "Basis der Hilfe-Links (statt /rs/docs/)"),
    s(K::Network, "PAPERRUST_SHARE_MODE", C, S, choice(&["local", "lan"]), None, "Freigabe-Hinweis im Desktop"),
    s(K::Network, "PAPERRUST_SHARE_URL", B, S, URL, None, "Adresse in Freigabelinks (Desktop-LAN)"),
    s(K::Network, "PAPERRUST_API_V2", B, S, Ty::OnOff, Some("on"), "v2-API abschalten"),
    s(K::Network, "PAPERRUST_LIGHT_UI", B, S, Ty::OnOff, Some("on"), "Light-UI unter /rs/app/ abschalten"),
    // ---- 1.4 Login, accounts, SSO ----
    s(K::Auth, "PAPERLESS_ACCOUNT_ALLOW_SIGNUPS", A, L, BOOL, Some("no"), "Selbstregistrierung"),
    s(K::Auth, "PAPERLESS_ACCOUNT_DEFAULT_GROUPS", A, L, LIST, None, "Gruppen neuer Nutzer"),
    s(K::Auth, "PAPERLESS_ACCOUNT_DEFAULT_HTTP_PROTOCOL", A, L, choice(&["http", "https"]), Some("https"), "Protokoll in Mail-Links"),
    s(
        K::Auth,
        "PAPERLESS_ACCOUNT_EMAIL_VERIFICATION",
        A,
        L,
        choice(&["none", "optional", "mandatory"]),
        Some("optional"),
        "E-Mail-Bestätigung",
    ),
    s(K::Auth, "PAPERLESS_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS", A, L, BOOL, Some("true"), "Mails an unbekannte Adressen (Passwort-Reset)"),
    s(K::Auth, "PAPERLESS_ACCOUNT_SESSION_REMEMBER", A, L, BOOL, Some("true"), "„angemeldet bleiben“"),
    s(K::Auth, "PAPERLESS_SOCIALACCOUNT_PROVIDERS", B, S, JSON_OBJ, Some("{}"), "OIDC/OAuth-Anbieter inkl. Client-Secrets").secret(),
    s(K::Auth, "PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS", A, L, BOOL, Some("yes"), "Registrierung über SSO"),
    s(K::Auth, "PAPERLESS_SOCIAL_AUTO_SIGNUP", A, L, BOOL, Some("no"), "SSO-Konto ohne Formular anlegen"),
    s(K::Auth, "PAPERLESS_SOCIAL_ACCOUNT_DEFAULT_GROUPS", A, L, LIST, None, "Gruppen neuer SSO-Nutzer"),
    s(K::Auth, "PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS", A, L, BOOL, Some("no"), "Gruppen aus Claim abgleichen"),
    s(K::Auth, "PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM", A, L, STR, Some("groups"), "Name des Gruppen-Claims"),
    s(K::Auth, "PAPERLESS_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP", A, L, STR, None, "Gruppe → Superuser"),
    s(K::Auth, "PAPERLESS_SOCIAL_ACCOUNT_SYNC_STAFF_GROUP", A, L, STR, None, "Gruppe → Staff"),
    s(K::Auth, "PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR", A, L, LIST, Some("mfa"), "AMR-Werte, die als MFA gelten"),
    s(K::Auth, "PAPERLESS_DISABLE_REGULAR_LOGIN", A, L, BOOL, Some("no"), "Passwort-Login abschalten (nur mit SSO-Anbieter)"),
    s(K::Auth, "PAPERLESS_REDIRECT_LOGIN_TO_SSO", A, L, BOOL, Some("no"), "Login-Seite direkt zum SSO (nur mit SSO-Anbieter)"),
    s(K::Auth, "PAPERLESS_AUTO_LOGIN_USERNAME", D, S, STR, None, "ohne Anmeldung als dieser Nutzer"),
    s(K::Auth, "PAPERLESS_ENABLE_HTTP_REMOTE_USER", D, S, BOOL, Some("no"), "Remote-User-Kopf für die UI"),
    s(K::Auth, "PAPERLESS_ENABLE_HTTP_REMOTE_USER_API", D, S, BOOL, Some("no"), "Remote-User-Kopf für die API"),
    s(K::Auth, "PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME", D, S, STR, Some("HTTP_REMOTE_USER"), "Name des Remote-User-Kopfs"),
    s(K::Auth, "PAPERRUST_PASSWORD_MIN_LENGTH", A, L, INT1, Some("8"), "Mindestlänge von Passwörtern (nur Rust-Pfade)").store(Store::Code),
    s(K::Auth, "PAPERRUST_MFA_TOTP_ISSUER", A, L, STR, Some("PaperRust"), "Aussteller im TOTP").store(Store::Code),
    // ---- 1.5 E-mail sending ----
    s(K::Email, "PAPERLESS_EMAIL_HOST", A, L, STR, Some("localhost"), "SMTP-Server"),
    s(K::Email, "PAPERLESS_EMAIL_PORT", A, L, PORT, Some("25"), "SMTP-Port"),
    s(K::Email, "PAPERLESS_EMAIL_HOST_USER", A, L, STR, Some(""), "SMTP-Nutzer"),
    s(K::Email, "PAPERLESS_EMAIL_HOST_PASSWORD", A, L, STR, Some(""), "SMTP-Passwort").secret(),
    s(K::Email, "PAPERLESS_EMAIL_FROM", A, L, STR, None, "Absender (Standard = EMAIL_HOST_USER)"),
    s(K::Email, "PAPERLESS_EMAIL_USE_TLS", A, L, BOOL, Some("no"), "STARTTLS"),
    s(K::Email, "PAPERLESS_EMAIL_USE_SSL", A, L, BOOL, Some("no"), "implizites TLS"),
    s(K::Email, "PAPERRUST_EMAIL_SUBJECT_PREFIX", A, L, STR, Some("[PaperRust] "), "Betreffpräfix").store(Store::Code),
    // ---- 1.6 Mail fetching ----
    s(K::MailFetch, "PAPERLESS_EMAIL_TASK_CRON", A, W, CRON, None, "Abrufzeitplan (Standard */10 mit Versatz aus SECRET_KEY)"),
    s(K::MailFetch, "PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS", D, L, BOOL, Some("true"), "IMAP zu internen Adressen"),
    s(K::MailFetch, "PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT", A, L, int(Some(1), Some(4)), Some("1"), "Standard-Layout Mail→PDF"),
    s(K::MailFetch, "PAPERLESS_ENABLE_GPG_DECRYPTOR", B, S, BOOL, Some("no"), "GPG-Mails entschlüsseln"),
    s(K::MailFetch, "PAPERLESS_EMAIL_GNUPG_HOME", D, S, PATH, None, "GnuPG-Ordner"),
    s(K::MailFetch, "PAPERLESS_OAUTH_CALLBACK_BASE_URL", A, L, URL, None, "Rückruf-Basis Gmail/Outlook (Standard = URL)"),
    s(K::MailFetch, "PAPERLESS_GMAIL_OAUTH_CLIENT_ID", A, L, STR, None, "Gmail-OAuth-Client"),
    s(K::MailFetch, "PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET", A, L, STR, None, "Gmail-OAuth-Secret").secret(),
    s(K::MailFetch, "PAPERLESS_OUTLOOK_OAUTH_CLIENT_ID", A, L, STR, None, "Outlook-OAuth-Client"),
    s(K::MailFetch, "PAPERLESS_OUTLOOK_OAUTH_CLIENT_SECRET", A, L, STR, None, "Outlook-OAuth-Secret").secret(),
    // ---- 1.7 Ingestion ----
    s(K::Consumer, "PAPERLESS_CONSUMER_POLLING_INTERVAL", A, W, FLOAT0, Some("0"), "Abfrageintervall Eingangsordner (0 = Ereignisse)"),
    s(K::Consumer, "PAPERLESS_CONSUMER_STABILITY_DELAY", A, L, FLOAT0, Some("5"), "Wartezeit, bis eine Datei stabil ist (s)"),
    s(K::Consumer, "PAPERLESS_CONSUMER_RECURSIVE", A, W, BOOL, Some("no"), "Unterordner einlesen"),
    s(K::Consumer, "PAPERLESS_CONSUMER_SUBDIRS_AS_TAGS", A, L, BOOL, Some("no"), "Unterordner als Tags"),
    s(K::Consumer, "PAPERLESS_CONSUMER_IGNORE_PATTERNS", A, L, JSON_ARR, Some("[]"), "Dateinamen-Muster ignorieren (Regex)"),
    s(K::Consumer, "PAPERLESS_CONSUMER_IGNORE_DIRS", A, L, JSON_ARR, Some("[]"), "Ordner ignorieren"),
    s(K::Consumer, "PAPERLESS_CONSUMER_DELETE_DUPLICATES", A, L, BOOL, Some("no"), "Duplikate löschen"),
    s(K::Consumer, "PAPERLESS_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED", A, L, BOOL, Some("no"), "Doppelseiten zusammenführen"),
    s(K::Consumer, "PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME", A, L, STR, Some("double-sided"), "Unterordner für Doppelseiten"),
    s(K::Consumer, "PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT", A, L, BOOL, Some("no"), "TIFF beim Zusammenführen"),
    s(K::Consumer, "PAPERLESS_CONSUMER_DISABLE", B, W, STR, None, "Eingangsordner-Überwachung aus (jeder nicht leere Wert)"),
    s(K::Consumer, "PAPERLESS_PRE_CONSUME_SCRIPT", D, L, PATH, None, "Skript vor dem Einlesen"),
    s(K::Consumer, "PAPERLESS_POST_CONSUME_SCRIPT", D, L, PATH, None, "Skript nach dem Einlesen"),
    s(K::Consumer, "PAPERLESS_MAX_IMAGE_PIXELS", A, L, INT0, None, "Pixelgrenze für Bilder"),
    // ---- 1.8 Barcodes (ApplicationConfiguration) ----
    s(K::Barcodes, "PAPERLESS_CONSUMER_ENABLE_BARCODES", A, L, BOOL, Some("no"), "Trennblatt-Barcodes").ac("barcodes_enabled"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_BARCODE_TIFF_SUPPORT", A, L, BOOL, Some("no"), "Barcodes auch in TIFF")
        .ac("barcode_enable_tiff_support"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_BARCODE_STRING", A, L, STR, Some("PATCHT"), "Text des Trennblatts").ac("barcode_string"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES", A, L, BOOL, Some("no"), "Trennseite behalten")
        .ac("barcode_retain_split_pages"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_ENABLE_ASN_BARCODE", A, L, BOOL, Some("no"), "ASN aus Barcode").ac("barcode_enable_asn"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_ASN_BARCODE_PREFIX", A, L, STR, Some("ASN"), "ASN-Präfix").ac("barcode_asn_prefix"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_BARCODE_UPSCALE", A, L, FLOAT0, Some("0.0"), "Hochskalieren").ac("barcode_upscale"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_BARCODE_DPI", A, L, INT1, Some("300"), "Render-DPI").ac("barcode_dpi"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_BARCODE_MAX_PAGES", A, L, INT0, Some("0"), "max. gescannte Seiten (0 = alle)")
        .ac("barcode_max_pages"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_ENABLE_TAG_BARCODE", A, L, BOOL, Some("no"), "Tags aus Barcode").ac("barcode_enable_tag"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_TAG_BARCODE_MAPPING", A, L, JSON_OBJ, Some(r#"{"TAG:(.*)": "\\g<1>"}"#), "Regex → Tag")
        .ac("barcode_tag_mapping"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_TAG_BARCODE_SPLIT", A, L, BOOL, Some("no"), "an Tag-Barcodes trennen").ac("barcode_tag_split"),
    s(K::Barcodes, "PAPERLESS_CONSUMER_STORE_BARCODE_VALUES", A, L, BOOL, Some("no"), "Barcodewerte speichern").ac("barcode_store_values"),
    // ---- 1.9 OCR ----
    s(K::Ocr, "PAPERLESS_OCR_LANGUAGE", A, L, STR, Some("deu+eng"), "Tesseract-Sprachen (deu+eng)").ac("language"),
    s(K::Ocr, "PAPERLESS_OCR_MODE", A, L, choice(&["auto", "force", "redo", "off"]), Some("auto"), "OCR-Modus").ac("mode"),
    s(K::Ocr, "PAPERLESS_ARCHIVE_FILE_GENERATION", A, L, choice(&["auto", "always", "never"]), Some("auto"), "Archiv-PDF erzeugen")
        .ac("archive_file_generation"),
    s(K::Ocr, "PAPERLESS_OCR_OUTPUT_TYPE", A, L, choice(&["pdf", "pdfa", "pdfa-1", "pdfa-2", "pdfa-3"]), Some("pdfa"), "Ausgabeformat")
        .ac("output_type"),
    s(K::Ocr, "PAPERLESS_OCR_PAGES", A, L, INT0, Some("0"), "nur die ersten n Seiten (0 = alle)").ac("pages"),
    s(K::Ocr, "PAPERLESS_OCR_IMAGE_DPI", A, L, INT0, None, "DPI für Bilder ohne Angabe").ac("image_dpi"),
    s(K::Ocr, "PAPERLESS_OCR_CLEAN", A, L, choice(&["clean", "clean-final", "none"]), Some("clean"), "unpaper-Bereinigung")
        .ac("unpaper_clean"),
    s(K::Ocr, "PAPERLESS_OCR_DESKEW", A, L, BOOL, Some("true"), "Geraderichten").ac("deskew"),
    s(K::Ocr, "PAPERLESS_OCR_ROTATE_PAGES", A, L, BOOL, Some("true"), "Seiten drehen").ac("rotate_pages"),
    s(K::Ocr, "PAPERLESS_OCR_ROTATE_PAGES_THRESHOLD", A, L, FLOAT0, Some("12.0"), "Drehschwelle").ac("rotate_pages_threshold"),
    s(K::Ocr, "PAPERLESS_OCR_MAX_IMAGE_PIXELS", A, L, FLOAT0, None, "Pixelgrenze OCR").ac("max_image_pixels"),
    s(K::Ocr, "PAPERLESS_OCR_COLOR_CONVERSION_STRATEGY", A, L, choice(COLOR), Some("RGB"), "Farbumwandlung PDF/A")
        .ac("color_conversion_strategy"),
    s(K::Ocr, "PAPERLESS_OCR_USER_ARGS", A, L, JSON_OBJ, None, "weitere OCR-Argumente").ac("user_args"),
    s(K::Ocr, "PAPERLESS_OCR_SKIP_ARCHIVE_FILE", X, N, STR, None, "alt, ohne Wirkung"),
    s(K::Ocr, "PAPERLESS_OCR_LANGUAGES", X, N, STR, None, "Sprachpakete im upstream-Image installieren").store(Store::Entrypoint),
    s(K::Ocr, "PAPERRUST_OCR_LANGUAGE_AUTO", A, L, BOOL, Some("false"), "OCR-Sprache automatisch erkennen und ggf. neu erkennen"),
    s(K::Ocr, "PAPERRUST_OCR_ENGINE", A, L, choice(&["tesseract", "windows", "macos", "auto"]), Some("tesseract"), "OCR-Engine"),
    s(
        K::Ocr,
        "PAPERRUST_OCR_ENGINE_IDLE_SECS",
        A,
        L,
        INT0,
        Some("180"),
        "Tesseract-Engines nach so vielen Sekunden Leerlauf entladen (0 = nie)",
    ),
    s(
        K::Ocr,
        "PAPERRUST_PDFA_FONT_DIR",
        C,
        L,
        PATH,
        None,
        "Ordner mit Ersatzschriften (Liberation) für fehlende Schriften im PDF/A-Archiv",
    ),
    s(K::Ocr, "PAPERLESS_REMOTE_OCR_ENGINE", A, L, choice(&["azureai"]), None, "Remote-OCR").ac("remote_ocr_engine"),
    s(K::Ocr, "PAPERLESS_REMOTE_OCR_API_KEY", A, L, STR, None, "API-Schlüssel Remote-OCR (Klartext wie upstream)")
        .secret()
        .ac("remote_ocr_api_key"),
    s(K::Ocr, "PAPERLESS_REMOTE_OCR_ENDPOINT", A, L, URL, None, "Endpunkt Remote-OCR").ac("remote_ocr_endpoint"),
    s(
        K::Ocr,
        "PAPERLESS_REMOTE_OCR_MODE",
        A,
        L,
        choice(&["always", "workflow_only"]),
        Some("always"),
        "Remote-OCR für alle oder nur per Workflow",
    )
    .ac("remote_ocr_mode"),
    s(K::Ocr, "PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS", D, L, BOOL, Some("true"), "interne Adressen für Remote-OCR"),
    s(K::Ocr, "PAPERLESS_TIKA_ENABLED", X, N, BOOL, Some("no"), "Tika/Gotenberg nutzen"),
    s(K::Ocr, "PAPERLESS_TIKA_ENDPOINT", X, N, URL, Some("http://localhost:9998"), "Tika-URL"),
    s(K::Ocr, "PAPERLESS_TIKA_GOTENBERG_ENDPOINT", X, N, URL, Some("http://localhost:3000"), "Gotenberg-URL"),
    s(K::Ocr, "PAPERLESS_CONVERT_BINARY", X, N, PATH, None, "ImageMagick (nicht mehr im Code)"),
    s(K::Ocr, "PAPERLESS_CONVERT_TMPDIR", X, N, PATH, None, "ImageMagick (nicht mehr im Code)"),
    s(K::Ocr, "PAPERLESS_CONVERT_MEMORY_LIMIT", X, N, STR, None, "ImageMagick (nicht mehr im Code)"),
    s(K::Ocr, "PAPERLESS_GS_BINARY", X, N, PATH, None, "Ghostscript (nicht mehr im Code)"),
    s(K::Ocr, "PAPERLESS_ENABLE_NLTK", X, N, BOOL, None, "entfernt"),
    s(K::Ocr, "PAPERLESS_NLTK_DIR", X, N, PATH, None, "entfernt"),
    // ---- 1.10 Documents ----
    s(K::Documents, "PAPERLESS_DATE_ORDER", A, L, STR, Some("DMY"), "Datumsreihenfolge im Inhalt"),
    s(K::Documents, "PAPERLESS_FILENAME_DATE_ORDER", A, L, STR, None, "Datum aus Dateiname"),
    s(K::Documents, "PAPERLESS_DATE_PARSER_LANGUAGES", A, L, STR, None, "Sprachen der Datumserkennung (Standard aus OCR-Sprache)"),
    s(K::Documents, "PAPERLESS_IGNORE_DATES", A, L, LIST, None, "ignorierte Daten"),
    s(K::Documents, "PAPERLESS_NUMBER_OF_SUGGESTED_DATES", A, L, INT0, Some("3"), "Anzahl Datumsvorschläge"),
    s(K::Documents, "PAPERLESS_FILENAME_FORMAT", A, L, STR, None, "Speicherpfad-Vorlage (global)").reindex(),
    s(K::Documents, "PAPERLESS_FILENAME_FORMAT_REMOVE_NONE", A, L, BOOL, Some("no"), "leere Platzhalter entfernen").reindex(),
    s(
        K::Documents,
        "PAPERLESS_SEARCH_LANGUAGE",
        B,
        S,
        choice(pl_core::config::SEARCH_SUPPORTED_LANGUAGES),
        None,
        "Sprache des Suchindex (Standard aus OCR-Sprache)",
    )
    .reindex(),
    s(K::Documents, "PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD", A, L, FLOAT, None, "Schwelle unscharfe Suche"),
    s(K::Documents, "PAPERLESS_CLASSIFIER_MATCH_THRESHOLD", A, L, FLOAT, Some("0.3"), "Mindestsicherheit Klassifikator"),
    s(K::Documents, "PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS", A, L, FLOAT0, Some("0.1"), "Regex-Zeitlimit für Zuordnungsmuster"),
    s(K::Documents, "PAPERLESS_TIME_ZONE", B, S, Ty::TimeZone, Some("UTC"), "Zeitzone"),
    s(K::Documents, "PAPERLESS_EMPTY_TRASH_DELAY", A, L, INT1, Some("30"), "Tage im Papierkorb"),
    s(K::Documents, "PAPERLESS_AUDIT_LOG_ENABLED", B, S, BOOL, Some("true"), "Änderungsprotokoll"),
    s(K::Documents, "PAPERLESS_ENABLE_UPDATE_CHECK", A, L, STR, Some("default"), "Versionsprüfung (default oder bool)"),
    s(K::Documents, "PAPERRUST_REMOTE_VERSION_URL", B, L, URL, None, "Quelle der Versionsprüfung"),
    s(K::Documents, "PAPERLESS_APP_TITLE", A, L, STR, None, "Anwendungstitel").ac("app_title"),
    s(K::Documents, "PAPERLESS_APP_LOGO", A, L, STR, None, "Logo (Upload)").ac("app_logo"),
    s(
        K::Documents,
        "PAPERRUST_TIMELINE_EXTRACT_DATES",
        A,
        L,
        BOOL,
        Some("no"),
        "Zeitstrahl: Fristen/Laufzeiten im Dokumentinhalt erkennen",
    ),
    s(K::Documents, "PAPERRUST_TIMELINE_DATE_FIELDS", A, L, JSON_OBJ, None, "Zeitstrahl: Custom Fields je Datumsart (JSON)"),
    s(
        K::Documents,
        "PAPERRUST_TEXT_HIT_CACHE",
        A,
        L,
        choice(&["on", "off"]),
        Some("on"),
        "Trefferspeicher für Titel/Inhalt-Suche mit häufigen Wörtern (exakt; off = jedes Mal den ganzen Text lesen)",
    ),
    s(
        K::Documents,
        "PAPERRUST_TEXT_HIT_CACHE_WORKERS",
        A,
        L,
        int(Some(1), Some(64)),
        None,
        "Verbindungen für den Aufbau des Trefferspeichers (leer = Prozessorkerne, höchstens 8; 1 = eine Abfrage)",
    ),
    s(
        K::Documents,
        "PAPERRUST_SEARCH_ENGINE",
        A,
        L,
        choice(&["legacy", "native"]),
        Some("native"),
        "Volltextsuche der Dokumentliste (L3: legacy = alle Treffer ranken und mit SQL schneiden, native = Rechte im Index, nur Top-N, Trefferzahl aus tantivy)",
    ),
    // ---- 1.11 Tasks, workers, schedules ----
    s(K::Tasks, "PAPERLESS_TASK_WORKERS", B, W, INT1, Some("1"), "parallele Aufgaben"),
    s(K::Tasks, "PAPERLESS_THREADS_PER_WORKER", B, W, INT1, None, "OCR-Threads je Aufgabe (Standard automatisch)"),
    s(K::Tasks, "PAPERLESS_WORKER_TIMEOUT", A, L, INT1, Some("1800"), "Zeitlimit je Aufgabe (s)"),
    s(K::Tasks, "PAPERLESS_TRAIN_TASK_CRON", A, W, CRON, Some("5 */1 * * *"), "Klassifikator trainieren"),
    s(K::Tasks, "PAPERLESS_INDEX_TASK_CRON", A, W, CRON, Some("0 0 * * *"), "Index optimieren"),
    s(K::Tasks, "PAPERLESS_SANITY_TASK_CRON", A, W, CRON, Some("30 0 * * sun"), "Prüfung"),
    s(K::Tasks, "PAPERLESS_EMPTY_TRASH_TASK_CRON", A, W, CRON, Some("0 1 * * *"), "Papierkorb leeren"),
    s(K::Tasks, "PAPERLESS_WORKFLOW_SCHEDULED_TASK_CRON", A, W, CRON, Some("5 */1 * * *"), "geplante Workflows"),
    // History of the workflows (docs/contract/automation.md §2.3)
    s(
        K::Tasks,
        "PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS",
        A,
        L,
        INT0,
        Some("90"),
        "Verlauf der Workflows aufbewahren (Tage, 0 = unbegrenzt)",
    ),
    s(K::Tasks, "PAPERLESS_LLM_INDEX_TASK_CRON", A, W, CRON, Some("10 2 * * *"), "KI-Index"),
    s(K::Tasks, "PAPERLESS_SHARE_LINK_BUNDLE_CLEANUP_CRON", A, W, CRON, Some("0 2 * * *"), "abgelaufene Freigabe-Bündel"),
    // upstream fix a139bdff4 (#14722)
    s(K::Tasks, "PAPERLESS_SESSION_CLEANUP_TASK_CRON", A, W, CRON, Some("0 3 * * *"), "abgelaufene Sitzungen löschen"),
    s(K::Tasks, "PAPERRUST_AI_TRIGGER_TAG_CRON", A, W, CRON, None, "KI-Auslöser-Tag"),
    s(K::Tasks, "PAPERRUST_QUEUE", X, N, STR, None, "Warteschlange (wirkungslos: immer PostgreSQL; `celery` → Warnung)"),
    s(K::Tasks, "PAPERRUST_QUEUE_BACKEND", X, N, STR, None, "älterer Name von PAPERRUST_QUEUE (wirkungslos)").alias_of("PAPERRUST_QUEUE"),
    s(K::Tasks, "PAPERRUST_QUEUE_NAMES", B, W, LIST, Some("celery"), "abgearbeitete Warteschlangen"),
    s(K::Tasks, "PAPERRUST_QUEUE_VISIBILITY_TIMEOUT", A, W, FLOAT0, Some("60"), "Lease-Dauer (s)"),
    s(K::Tasks, "PAPERRUST_QUEUE_MAX_DELIVERIES", A, L, INT1, Some("3"), "Zustellungen je Versuch"),
    s(K::Tasks, "PAPERRUST_QUEUE_POLL_INTERVAL", A, W, FLOAT0, Some("1"), "Abfrage ohne NOTIFY (s)"),
    s(K::Tasks, "PAPERRUST_QUEUE_SHUTDOWN_GRACE", A, L, FLOAT0, Some("30"), "Wartezeit beim Beenden (s)"),
    s(K::Tasks, "PAPERRUST_QUEUE_RESULT_EXPIRES", A, L, FLOAT0, Some("3600"), "Aufbewahrung erledigter Aufgaben (s)"),
    s(K::Tasks, "PAPERRUST_WORKER_NAME", B, S, STR, None, "Worker-Name (Standard paperrust@host:pid)"),
    s(
        K::Tasks,
        "PAPERRUST_INDEX_WRITER",
        B,
        S,
        choice(&["queue", "rust", "off", "celery"]),
        Some("queue"),
        "Suchindex über die Warteschlange, direkt oder aus (`celery` = alter Name für `queue`)",
    ),
    s(K::Tasks, "PAPERLESS_ENABLE_FLOWER", X, N, STR, None, "Flower im upstream-Image").store(Store::Entrypoint),
    s(K::Tasks, "PAPERLESS_SUPERVISORD_WORKING_DIR", X, N, PATH, None, "veraltet").store(Store::Entrypoint),
    // ---- 1.12 Integrity, sync, import ----
    s(K::Integrity, "PAPERRUST_INTEGRITY_INCREMENTAL", A, L, BOOL, Some("no"), "unveränderte Dateien nicht neu hashen"),
    s(K::Integrity, "PAPERRUST_INTEGRITY_FULL_HASH_DAYS", A, L, INT0, Some("28"), "Höchstalter einer Bestätigung (Tage)"),
    s(K::Integrity, "PAPERRUST_INTEGRITY_THREADS", A, L, INT0, Some("0"), "parallele Leser (0 = automatisch)"),
    s(K::Integrity, "PAPERRUST_INTEGRITY_REPORT_KEEP", A, L, INT0, Some("10"), "aufbewahrte Berichte"),
    // Protection against ransomware (pl-guard, 2026-11-14): guard, read-only mode, encrypted backups
    s(K::Integrity, "PAPERRUST_GUARD", A, W, Ty::OnOff, Some("on"), "Ransomware-Wächter für den Medienordner"),
    s(K::Integrity, "PAPERRUST_GUARD_THRESHOLD", A, W, INT1, Some("20"), "Wächter: so viele verdächtige Dateien schalten auf „nur lesen“"),
    s(K::Integrity, "PAPERRUST_GUARD_WINDOW_MINUTES", A, W, INT1, Some("10"), "Wächter: Zeitfenster für die Schwelle (Minuten)"),
    s(
        K::Integrity,
        "PAPERRUST_GUARD_SCAN_INTERVAL",
        A,
        W,
        INT0,
        Some("900"),
        "Wächter: Abstand der Ordnerdurchsicht in Sekunden (0 = aus)",
    ),
    s(K::Integrity, "PAPERRUST_GUARD_SAMPLE_FILES", A, W, INT0, Some("20"), "Wächter: je Durchsicht vollständig gehashte Dokumente"),
    s(K::Integrity, "PAPERRUST_GUARD_WATCH", A, W, Ty::OnOff, Some("on"), "Wächter: Dateisystem-Ereignisse nutzen (inotify u. a.)"),
    s(K::Integrity, "PAPERRUST_GUARD_EXTRA_EXTENSIONS", A, W, LIST, None, "Wächter: zusätzliche verdächtige Dateiendungen"),
    s(K::Integrity, "PAPERRUST_GUARD_MAIL", A, W, LIST, None, "Wächter: Empfänger der Alarm-Mail (leer = alle Superuser)"),
    s(K::Integrity, "PAPERRUST_BACKUP_TARGET", A, L, STR, None, "Sicherung: Ziel (Ordner oder s3://bucket/präfix)"),
    s(K::Integrity, "PAPERRUST_BACKUP_CRON", A, W, CRON, Some("15 2 * * *"), "Sicherung: Zeitplan (Crontab, disable = aus)"),
    s(K::Integrity, "PAPERRUST_BACKUP_DATABASE", A, L, Ty::OnOff, Some("on"), "Sicherung: Datenbank mitsichern"),
    s(K::Integrity, "PAPERRUST_BACKUP_KEEP_DAILY", A, L, INT0, Some("7"), "Sicherung: tägliche Stände behalten"),
    s(K::Integrity, "PAPERRUST_BACKUP_KEEP_WEEKLY", A, L, INT0, Some("4"), "Sicherung: wöchentliche Stände behalten"),
    s(K::Integrity, "PAPERRUST_BACKUP_KEEP_MONTHLY", A, L, INT0, Some("12"), "Sicherung: monatliche Stände behalten"),
    s(K::Integrity, "PAPERRUST_BACKUP_S3_ENDPOINT", A, L, URL, None, "Sicherung: S3-Endpunkt (https://host:port)"),
    s(K::Integrity, "PAPERRUST_BACKUP_S3_REGION", A, L, STR, Some("us-east-1"), "Sicherung: S3-Region"),
    s(K::Integrity, "PAPERRUST_BACKUP_S3_ACCESS_KEY", A, L, STR, None, "Sicherung: S3-Zugangsschlüssel (Access Key ID)"),
    s(K::Integrity, "PAPERRUST_BACKUP_S3_SECRET_KEY", A, L, STR, None, "Sicherung: S3-Geheimschlüssel").secret(),
    s(
        K::Integrity,
        "PAPERRUST_BACKUP_S3_OBJECT_LOCK",
        A,
        L,
        choice(&["off", "governance", "compliance"]),
        Some("off"),
        "Sicherung: S3 Object Lock (Sperrfrist je Objekt)",
    ),
    s(K::Integrity, "PAPERRUST_BACKUP_S3_LOCK_DAYS", A, L, INT1, Some("30"), "Sicherung: Sperrfrist in Tagen (Object Lock)"),
    s(
        K::Integrity,
        "PAPERRUST_BACKUP_PASSPHRASE",
        D,
        N,
        STR,
        None,
        "Sicherung: Passphrase für init/restore (nur Umgebung, nie gespeichert)",
    )
    .secret(),
    s(K::Integrity, "PAPERRUST_SYNC_SEQUENCER", B, W, Ty::OnOff, Some("on"), "Sync-Sequenzer im Worker"),
    s(K::Integrity, "PAPERRUST_SYNC_TOMBSTONE_DAYS", A, L, INT1, Some("180"), "Aufbewahrung von Löschmarken (Tage)"),
    s(K::Integrity, "PAPERRUST_SYNC_INTERVAL", A, W, FLOAT0, Some("1"), "Sequenzer-Abfrage (s)"),
    s(K::Integrity, "PAPERRUST_SYNC_RETENTION_INTERVAL", A, W, FLOAT0, Some("3600"), "Aufräumlauf (s)"),
    s(K::Integrity, "PAPERRUST_SYNC_BATCH", A, L, INT1, Some("50000"), "Zeilen je Transaktion"),
    s(K::Integrity, "PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES", D, N, INT0, None, "Entpackgrenze beim Import (CLI)"),
    s(K::Integrity, "PAPERRUST_MIGRATE_SOURCE", C, O, STR, None, "Umzugswerkzeug: Quelle"),
    s(K::Integrity, "PAPERRUST_MIGRATE_TARGET", C, O, STR, None, "Umzugswerkzeug: Ziel"),
    s(K::Integrity, "PAPERRUST_MIGRATE_API_TOKEN", C, O, STR, None, "Umzugswerkzeug: API-Token").secret(),
    // ---- 1.13 Logs ----
    s(K::Logging, "PAPERRUST_LOG", A, L, STR, Some("info"), "Log-Filter (tracing EnvFilter)"),
    s(K::Logging, "PAPERRUST_LOG_FILES", B, S, BOOL, Some("yes"), "Logdateien schreiben"),
    s(K::Logging, "PAPERLESS_LOGROTATE_MAX_SIZE", B, S, INT1, Some("1048576"), "Größe je Logdatei (Bytes)"),
    s(K::Logging, "PAPERLESS_LOGROTATE_MAX_BACKUPS", B, S, INT0, Some("20"), "Anzahl Logdateien"),
    // ---- 1.14 Branding ----
    s(K::Branding, "PAPERRUST_BRANDING", B, S, Ty::OnOff, Some("on"), "PaperRust-Logo/Name/Farbe"),
    s(K::Branding, "PAPERRUST_ACCENT", B, S, STR, Some("#B7410E"), "Akzentfarbe (Hex)"),
    s(K::Branding, "PAPERRUST_DEFAULT_LANGUAGE", A, L, STR, None, "Sprache für Nutzer ohne Wahl / Login-Seite").store(Store::Code),
    s(K::Branding, "PAPERRUST_UI_DEFAULTS", A, L, JSON_OBJ, None, "Vorgaben für ui_settings neuer Nutzer").store(Store::Code),
    // ---- 1.15 KI ----
    s(K::Ai, "PAPERLESS_AI_ENABLED", A, L, BOOL, Some("no"), "KI an").ac("ai_enabled"),
    s(K::Ai, "PAPERLESS_AI_LLM_BACKEND", A, L, choice(&["ollama", "openai-like"]), None, "LLM-Backend").ac("llm_backend"),
    s(K::Ai, "PAPERLESS_AI_LLM_MODEL", A, L, STR, None, "LLM-Modell").ac("llm_model"),
    s(K::Ai, "PAPERLESS_AI_LLM_API_KEY", A, L, STR, None, "LLM-API-Schlüssel (Klartext wie upstream)").secret().ac("llm_api_key"),
    s(K::Ai, "PAPERLESS_AI_LLM_ENDPOINT", A, L, URL, None, "LLM-Endpunkt").ac("llm_endpoint"),
    s(K::Ai, "PAPERLESS_AI_LLM_OUTPUT_LANGUAGE", A, L, STR, None, "Ausgabesprache").ac("llm_output_language"),
    s(K::Ai, "PAPERLESS_AI_LLM_CONTEXT_SIZE", A, L, INT1, Some("8192"), "Kontextgröße").ac("llm_context_size"),
    s(K::Ai, "PAPERLESS_AI_LLM_REQUEST_TIMEOUT", A, L, INT1, Some("120"), "Zeitlimit (s)").ac("llm_request_timeout"),
    s(K::Ai, "PAPERLESS_AI_LLM_EMBEDDING_BACKEND", A, L, choice(&["huggingface", "openai-like", "ollama"]), None, "Embedding-Backend")
        .ac("llm_embedding_backend")
        .reindex(),
    s(K::Ai, "PAPERLESS_AI_LLM_EMBEDDING_MODEL", A, L, STR, None, "Embedding-Modell").ac("llm_embedding_model").reindex(),
    s(K::Ai, "PAPERLESS_AI_LLM_EMBEDDING_API_KEY", A, L, STR, None, "Embedding-API-Schlüssel (Klartext wie upstream)")
        .secret()
        .ac("llm_embedding_api_key"),
    s(K::Ai, "PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT", A, L, URL, None, "Embedding-Endpunkt").ac("llm_embedding_endpoint"),
    s(K::Ai, "PAPERLESS_AI_LLM_EMBEDDING_CHUNK_SIZE", A, L, INT1, Some("1024"), "Chunk-Größe").ac("llm_embedding_chunk_size").reindex(),
    s(K::Ai, "PAPERLESS_AI_LLM_EXTRA_PARAMS", A, L, JSON_OBJ, Some("{}"), "zusätzliche Anfrage-Parameter"),
    s(K::Ai, "PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS", D, L, BOOL, Some("true"), "interne Adressen für LLM"),
    s(K::Ai, "PAPERRUST_AI_API_BASE", B, S, URL, None, "interne API-Basis des MCP-Servers"),
    s(K::Ai, "PAPERRUST_AI_MCP_ENABLED", B, S, BOOL, Some("no"), "/mcp ausliefern"),
    s(K::Ai, "PAPERRUST_AI_MCP_RATE_LIMIT", A, L, INT0, Some("120"), "MCP-Anfragen je Minute und Token"),
    s(K::Ai, "PAPERRUST_AI_MCP_MAX_CREATES_PER_HOUR", A, L, INT0, Some("30"), "MCP-Neuanlagen je Stunde und Token"),
    s(K::Ai, "PAPERRUST_AI_MCP_ALLOWED_ORIGINS", A, L, LIST, None, "erlaubte Origins für /mcp"),
    s(K::Ai, "PAPERRUST_AI_MCP_TOOL_GROUPS", A, L, LIST, Some("read,write"), "freigeschaltete MCP-Werkzeuggruppen (read, write, ingest)"),
    s(K::Ai, "PAPERRUST_AI_MCP_ALLOW_URL_FETCH", D, L, BOOL, Some("no"), "add_document darf http(s)-URLs abrufen (SSRF-Schalter)"),
    s(K::Ai, "PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES", A, L, INT1, Some("1572864"), "größte Datei für add_document (Bytes)"),
    s(K::Ai, "PAPERRUST_AI_MCP_MAX_INGESTS_PER_HOUR", A, L, INT0, Some("30"), "add_document je Token und Stunde"),
    s(K::Ai, "PAPERRUST_AI_MATCH_THRESHOLD", A, L, FLOAT0, Some("0.8"), "Namensabgleich"),
    s(K::Ai, "PAPERRUST_AI_AUTO_MODE", A, L, choice(&["off", "suggest", "apply"]), Some("off"), "KI-Automatik"),
    s(K::Ai, "PAPERRUST_AI_AUTO_MIN_CONFIDENCE", A, L, FLOAT0, Some("0.9"), "Übernahmeschwelle"),
    s(K::Ai, "PAPERRUST_AI_AUTO_TRIGGER_TAG", A, L, STR, None, "Auslöser-Tag"),
    s(K::Ai, "PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS", A, L, INT0, Some("3"), "neue Objekte je Lauf"),
    s(K::Ai, "PAPERRUST_AI_AUTO_MAX_DOCUMENTS_PER_RUN", A, L, INT0, Some("50"), "Dokumente je Lauf"),
    s(K::Ai, "PAPERRUST_AI_AUTO_CREATE_MISSING", A, L, BOOL, Some("no"), "fehlende Objekte anlegen"),
    s(K::Ai, "PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR", A, L, INT0, Some("100"), "manuelle Läufe: Dokumente je Stunde"),
    s(K::Ai, "PAPERRUST_AI_VISION_ENABLED", A, L, BOOL, Some("no"), "KI-Texterkennung verfügbar"),
    s(K::Ai, "PAPERRUST_AI_VISION_MODEL", A, L, STR, None, "Bildmodell (leer = Textmodell)"),
    s(K::Ai, "PAPERRUST_AI_VISION_WHEN", A, L, choice(&["off", "difficult", "always"]), Some("difficult"), "wann KI-Texterkennung"),
    s(K::Ai, "PAPERRUST_AI_VISION_MAX_PAGES", A, L, INT1, Some("10"), "Seiten je Dokument"),
    s(K::Ai, "PAPERRUST_AI_VISION_MAX_PAGES_PER_DAY", A, L, INT0, Some("500"), "Seiten je Tag (0 = unbegrenzt)"),
    s(K::Ai, "PAPERRUST_AI_CUSTOM_FIELDS_ENABLED", A, L, BOOL, Some("no"), "Benutzerfelder per KI ausfüllen"),
    // ---- 1.16 Webhooks ----
    s(K::Webhooks, "PAPERLESS_WEBHOOKS_ALLOWED_SCHEMES", D, L, LIST, Some("http,https"), "erlaubte Schemata"),
    s(K::Webhooks, "PAPERLESS_WEBHOOKS_ALLOWED_PORTS", D, L, LIST, None, "erlaubte Ports (Standard alle)"),
    s(K::Webhooks, "PAPERLESS_WEBHOOKS_ALLOW_INTERNAL_REQUESTS", D, L, BOOL, Some("true"), "interne Ziele"),
    // ---- 1.16a Desktop ----
    s(K::Desktop, "desktop.database.port", C, S, PORT, None, "Wunschport eingebettetes PostgreSQL").store(Store::DesktopToml),
    s(K::Desktop, "desktop.database.user", C, O, STR, None, "Cluster-Superuser").store(Store::DesktopToml),
    s(K::Desktop, "desktop.database.password", C, O, STR, None, "Passwort des Cluster-Superusers").secret().store(Store::DesktopToml),
    s(K::Desktop, "desktop.database.app_user", C, O, STR, Some("paperrust"), "Anwendungsrolle").store(Store::DesktopToml),
    s(K::Desktop, "desktop.database.app_password", C, O, STR, None, "Passwort der Anwendungsrolle").secret().store(Store::DesktopToml),
    s(K::Desktop, "desktop.database.name", C, O, STR, Some("paperless"), "DB-Name").store(Store::DesktopToml),
    s(K::Desktop, "desktop.database.locale_provider", C, O, choice(&["builtin", "icu"]), Some("icu"), "Locale-Provider (nur initdb)")
        .store(Store::DesktopToml),
    s(K::Desktop, "desktop.server.port", C, S, PORT, None, "App-Port").store(Store::DesktopToml),
    s(K::Desktop, "desktop.server.secret_key", C, S, STR, None, "SECRET_KEY").secret().store(Store::DesktopToml),
    s(K::Desktop, "desktop.server.time_zone", B, S, Ty::TimeZone, Some("UTC"), "→ TIME_ZONE (wird nach DB umgezogen)")
        .store(Store::DesktopToml),
    s(K::Desktop, "desktop.server.ocr_language", A, L, STR, Some("deu+eng"), "→ OCR_LANGUAGE (wird nach AC umgezogen)")
        .store(Store::DesktopToml),
    s(K::Desktop, "desktop.server.open_browser", C, O, BOOL, Some("yes"), "Browser beim Start öffnen").store(Store::DesktopToml),
    s(K::Desktop, "desktop.server.lan_sharing", C, S, BOOL, Some("no"), "im LAN freigeben").store(Store::DesktopToml),
    s(K::Desktop, "desktop.server.lan_address", C, S, STR, None, "LAN-Adresse für Links").store(Store::DesktopToml),
    s(K::Desktop, "desktop.setup.cluster_initialized", X, O, BOOL, None, "Erst-Start: Cluster angelegt")
        .store(Store::DesktopToml)
        .internal(),
    s(K::Desktop, "desktop.setup.database_created", X, O, BOOL, None, "Erst-Start: DB angelegt").store(Store::DesktopToml).internal(),
    s(K::Desktop, "desktop.setup.superuser_created", X, O, BOOL, None, "Erst-Start: Superuser angelegt")
        .store(Store::DesktopToml)
        .internal(),
    s(K::Desktop, "desktop.setup.home_protected", X, O, BOOL, None, "Erst-Start: Datenordner geschützt")
        .store(Store::DesktopToml)
        .internal(),
    s(K::Desktop, "desktop.setup.settings_migrated", X, O, BOOL, None, "Zeitzone/OCR-Sprache in die Datenbank umgezogen")
        .store(Store::DesktopToml)
        .internal(),
    s(K::Desktop, "desktop.backup.schedule", C, L, STR, Some("off"), "automatische Sicherung (off/daily/weekly + Uhrzeit)")
        .store(Store::DesktopToml),
    s(K::Desktop, "desktop.backup.keep", C, L, INT1, Some("7"), "aufbewahrte Sicherungen").store(Store::DesktopToml),
    s(K::Desktop, "desktop.updates.check", C, L, choice(&["off", "notify", "install"]), Some("notify"), "App-Update prüfen")
        .store(Store::DesktopToml),
    s(K::Desktop, "app.mode", C, S, choice(&["local", "remote"]), None, "Betriebsart der Hülle").store(Store::AppToml),
    s(K::Desktop, "app.active_profile", C, L, STR, None, "aktives Fern-Profil").store(Store::AppToml),
    s(K::Desktop, "app.profile", C, L, STR, None, "Fern-Profile (name, url, flavor, ui, tls_pin, allow_http, extra_origins, sync)")
        .store(Store::AppToml),
    s(K::Desktop, "PAPERRUST_DESKTOP_HOME", C, S, PATH, None, "Datenordner der Desktop-App"),
    s(K::Desktop, "PAPERRUST_DESKTOP_MODE", C, O, STR, None, "Erst-Start ohne Formular: Betriebsart"),
    s(K::Desktop, "PAPERRUST_DESKTOP_SERVER_URL", C, O, URL, None, "Erst-Start ohne Formular: Server-URL"),
    s(K::Desktop, "PAPERRUST_DESKTOP_ALLOW_HTTP", C, O, BOOL, None, "Erst-Start ohne Formular: HTTP erlauben"),
    s(K::Desktop, "PAPERRUST_DESKTOP_ADMIN_USER", C, O, STR, None, "Erst-Start-Superuser"),
    s(K::Desktop, "PAPERRUST_DESKTOP_ADMIN_EMAIL", C, O, STR, None, "Erst-Start-Superuser: E-Mail"),
    s(K::Desktop, "PAPERRUST_DESKTOP_ADMIN_PASSWORD", C, O, STR, None, "Erst-Start-Superuser: Passwort").secret(),
    s(K::Desktop, "PAPERRUST_DESKTOP_IMPORT", C, O, PATH, None, "Erst-Start mit Import"),
    s(K::Desktop, "PAPERRUST_DESKTOP_IMPORT_DB", C, O, STR, None, "Erst-Start mit Import: DB"),
    s(K::Desktop, "PAPERRUST_DESKTOP_IMPORT_MEDIA", C, O, PATH, None, "Erst-Start mit Import: Medien"),
    s(K::Desktop, "PAPERRUST_DESKTOP_LOG", B, S, STR, Some("info"), "Log-Stufe Supervisor"),
    s(K::Desktop, "PAPERRUST_DESKTOP_SERVER_LOG", B, S, STR, Some("info"), "Log-Stufe Server"),
    s(K::Desktop, "PAPERRUST_DESKTOP_LOG_MAX_MB", B, S, INT1, Some("10"), "Loggrenze (MB)"),
    s(K::Desktop, "PAPERRUST_DESKTOP_PG_BIN", C, S, PATH, None, "Installationslayout: PostgreSQL"),
    s(K::Desktop, "PAPERRUST_DESKTOP_SERVER_BIN", C, S, PATH, None, "Installationslayout: Server"),
    s(K::Desktop, "PAPERRUST_DESKTOP_MIGRATE_DB_BIN", C, S, PATH, None, "Installationslayout: Umzugswerkzeug"),
    s(K::Desktop, "PAPERRUST_DESKTOP_STATIC", C, S, PATH, None, "Installationslayout: statische Dateien"),
    s(K::Desktop, "PAPERRUST_DESKTOP_ROUTES", C, S, PATH, None, "Installationslayout: routes.toml"),
    s(K::Desktop, "PAPERRUST_DESKTOP_UPDATES", C, L, Ty::OnOff, Some("on"), "App-Updates der Desktop-Hülle (off/aus schaltet ab)"),
    s(K::Desktop, "PAPERRUST_DESKTOP_UPDATE_URL", C, L, URL, None, "Update-Adresse der Desktop-Hülle (Platzhalter {{channel}})"),
    s(K::Desktop, "PAPERRUST_DESKTOP_UPDATE_CHANNEL", C, L, choice(&["stable", "beta"]), Some("stable"), "Update-Kanal der Desktop-Hülle"),
    s(K::Desktop, "PAPERRUST_DESKTOP_NO_JOB", D, S, BOOL, Some("no"), "Windows-Job-Objekt aus (Fehlersuche)"),
    s(K::Desktop, "PAPERRUST_DESKTOP_RESTART_AFTER", X, N, STR, None, "intern (Neustart der Hülle)").store(Store::Code).internal(),
    // own switches (2026-11-13): bulk changes, backup on switchover
    s(
        K::Documents,
        "PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS",
        C,
        S,
        INT0,
        Some("500"),
        "Massenänderung ab so vielen Dokumenten im Hintergrund (0 = immer sofort)",
    ),
    s(K::Tasks, "PAPERRUST_BULK_UPDATE", C, W, STR, None, "Massen-Neuspeichern: `single` = alter Einzelweg (Fehlersuche)"),
    // Load findings B1/B6 (2026-11-18)
    s(K::Tasks, "PAPERRUST_BULK_UPDATE_COALESCE", C, S, Ty::OnOff, Some("on"), "wartende Nacharbeiten von Massenänderungen zusammenfassen"),
    s(
        K::Documents,
        "PAPERRUST_MEDIA_LOCK_TIMEOUT",
        C,
        S,
        FLOAT0,
        Some("120"),
        "höchste Wartezeit auf die Dateisperre MEDIA_LOCK (s, 0 = ohne Limit)",
    ),
    s(K::Database, "PAPERRUST_PG_BIN", C, S, PATH, None, "Ordner mit pg_dump für die Sicherung beim Umstieg"),
    // Scanner connection (pl-scan, 2026-11-13): SMTP reception for devices only (scan-to-email)
    s(K::Network, "PAPERRUST_SMTP_INTAKE", B, S, Ty::OnOff, Some("off"), "SMTP-Empfang für Scanner/Drucker (Scan-to-Email) einschalten"),
    s(K::Network, "PAPERRUST_SMTP_INTAKE_LISTEN", B, S, STR, Some("0.0.0.0:2525"), "SMTP-Empfang: Adresse und Port (ip:port)"),
    s(K::Network, "PAPERRUST_SMTP_INTAKE_MAX_MB", B, S, int(Some(1), Some(1024)), Some("50"), "SMTP-Empfang: größte Nachricht in MB"),
    s(K::Network, "PAPERRUST_SMTP_INTAKE_ALLOWED_IPS", B, S, STR, None, "SMTP-Empfang: erlaubte Absender-IPs/Netze (leer = alle)"),
    s(
        K::Network,
        "PAPERRUST_SMTP_INTAKE_RATE_PER_MINUTE",
        B,
        S,
        int(Some(1), Some(10000)),
        Some("30"),
        "SMTP-Empfang: Nachrichten je Minute und Gerät",
    ),
    s(K::Network, "PAPERRUST_SMTP_INTAKE_REQUIRE_TLS", B, S, Ty::OnOff, Some("off"), "SMTP-Empfang: Anmeldung nur nach STARTTLS"),
    s(
        K::Network,
        "PAPERRUST_SMTP_INTAKE_IMPLICIT_TLS",
        B,
        S,
        Ty::OnOff,
        Some("off"),
        "SMTP-Empfang: direktes TLS (SMTPS, Port 465) statt STARTTLS",
    ),
    s(K::Network, "PAPERRUST_SMTP_INTAKE_TLS_CERT", D, S, PATH, None, "SMTP-Empfang: Zertifikat (PEM) für STARTTLS"),
    s(K::Network, "PAPERRUST_SMTP_INTAKE_TLS_KEY", D, S, PATH, None, "SMTP-Empfang: privater Schlüssel (PEM) für STARTTLS"),
    // PDF tools (2026-11-13, pl-pdftools): shrinking on ingestion, child process limits
    s(K::Consumer, "PAPERRUST_SHRINK_ON_INGEST_MB", A, L, INT0, Some("0"), "Scans über so vielen MB beim Einlesen verkleinern (0 = aus)"),
    s(
        K::Consumer,
        "PAPERRUST_SHRINK_PRESET",
        A,
        L,
        choice(&["lossless", "standard", "strong"]),
        Some("standard"),
        "Stufe beim Verkleinern: verlustfrei, Standard (150 dpi), stark (96 dpi)",
    ),
    s(
        K::Consumer,
        "PAPERRUST_PDFTOOL_TIMEOUT",
        A,
        L,
        INT1,
        Some("120"),
        "Zeitgrenze je PDF-Werkzeug-Lauf (Schwärzen, Verkleinern, Reparieren) in Sekunden",
    ),
    s(K::Consumer, "PAPERRUST_PDFTOOL_MEMORY_MB", A, L, INT0, Some("1536"), "Speichergrenze des PDF-Werkzeugs in MB (0 = ohne Grenze)"),
    s(
        K::Consumer,
        "PAPERRUST_OFFICE_TO_PDF",
        A,
        L,
        BOOL,
        Some("true"),
        "Office-Dateien (docx, odt, rtf) beim Einlesen mit WordCraft in ein Archiv-PDF umwandeln",
    ),
    s(
        K::Consumer,
        "PAPERRUST_OFFICE_TO_PDF_TIMEOUT",
        A,
        L,
        INT0,
        Some("0"),
        "Zeitgrenze je Office-Umwandlung in Sekunden (0 = wie PDF-Werkzeug)",
    ),
    s(K::Paths, "PAPERRUST_PDFTOOL_BIN", C, S, PATH, None, "Pfad zu paperrust-pdftool (Standard: neben dem Server-Programm)"),
    // Own data model (pl-cli datamodel): backup before the conversion stages
    s(K::Database, "PAPERRUST_DATAMODEL_BACKUP", C, O, Ty::OnOff, Some("on"), "Sicherung (pg_dump) vor den Datenmodell-Umbaustufen"),
    s(
        K::Database,
        "PAPERRUST_MIGRATE_WITHOUT_BACKUP",
        C,
        O,
        BOOL,
        Some("false"),
        "Datenmodell-Umbau ohne pg_dump trotzdem ausführen (ohne Sicherung)",
    ),
];

/// Entry for a canonical key. Also accepts the name variants
/// (`PAPERRUST_EMAIL_HOST` → `PAPERLESS_EMAIL_HOST`, `PAPERLESS_RS_LOG` → `PAPERRUST_LOG`).
pub fn find(key: &str) -> Option<&'static SettingDef> {
    if let Some(d) = SCHEMA.iter().find(|d| d.key == key) {
        return Some(d);
    }
    let cands = pl_core::config::candidates(key);
    let rest = key.strip_prefix(pl_core::config::PREFIX).or_else(|| key.strip_prefix(pl_core::config::LEGACY_PREFIX));
    SCHEMA.iter().find(|d| {
        d.is_env()
            && (cands.iter().any(|c| c == d.key) || rest.is_some_and(|r| d.key.strip_prefix(pl_core::config::UPSTREAM_PREFIX) == Some(r)))
    })
}

/// Displayed name for a key (see [`SettingDef::name`]): `PAPERLESS_X` →
/// `PAPERRUST_X` ([`pl_core::config::primary_name`]); `PAPERRUST_X` and desktop paths stay.
pub fn display_name(key: &str) -> std::borrow::Cow<'static, str> {
    pl_core::config::primary_name(key).into()
}

/// Entry for an AC column.
pub fn find_ac(field: &str) -> Option<&'static SettingDef> {
    SCHEMA.iter().find(|d| d.ac_field == Some(field))
}

/// Count per class (`[A, B, C, D, X]`) and number of AC fields.
pub fn counts() -> ([usize; 5], usize) {
    let mut c = [0usize; 5];
    for d in SCHEMA {
        c[d.class as usize] += 1;
    }
    (c, SCHEMA.iter().filter(|d| d.ac_field.is_some()).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_and_aliases() {
        assert_eq!(display_name("PAPERLESS_EMAIL_HOST"), "PAPERRUST_EMAIL_HOST");
        assert_eq!(display_name("PAPERRUST_LOG"), "PAPERRUST_LOG");
        assert_eq!(display_name("PAPERLESS_RS_LOG"), "PAPERRUST_LOG");
        assert_eq!(display_name("desktop.server.port"), "desktop.server.port");
        let d = find("PAPERLESS_EMAIL_HOST").unwrap();
        assert_eq!(d.name(), "PAPERRUST_EMAIL_HOST");
        assert_eq!(d.aliases(), vec!["PAPERLESS_EMAIL_HOST".to_string()]);
        assert_eq!(d.doc_anchor(), "/rs/docs/configuration#PAPERRUST_EMAIL_HOST");
        // the displayed name finds the same entry
        assert!(std::ptr::eq(find(&d.name()).unwrap(), d));
        for d in SCHEMA {
            assert!(!d.name().starts_with("PAPERLESS_"), "{}", d.key);
        }
    }
}
