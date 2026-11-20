//! Command line of the `paperrust` binary.
//!
//! ```text
//! paperrust [serve] [--worker | --worker-only | --no-worker]
//! paperrust manage <command> [args…]
//! paperrust migrate [--from <paperless-ngx>] …   (switch assistant → paperrust-migrate-db)
//! ```
//!
//! * `serve` (default, may be omitted): HTTP server.
//! * `--worker`: additionally run the task worker, the scheduler (beat) and the consumption
//!   folder watcher in the same process.
//! * `--worker-only`: only worker + beat + folder watcher, no HTTP server.
//! * `--no-worker` (default): HTTP server only.
//! * `manage …`: management commands of pl-cli (`pl_cli::run_async`, replaces `manage.py`);
//!   `mail_fetcher` runs pl-mail's `process_mail_accounts` directly (like upstream).
//!
//! Replaces `celery worker`, `celery beat` and `manage.py document_consumer` of upstream
//! (upstream: docker/rootfs/etc/s6-overlay/s6-rc.d/svc-{worker,scheduler,consumer}@v3.4.2).

use std::process::ExitCode;

use pl_core::AppState;
use pl_core::state::StateError;
use tracing_subscriber::EnvFilter;

use crate::config::{DjangoSettings, ServerConfig};
use crate::routing;
use crate::worker::{self, WorkerMode};

/// Parsed command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cli {
    pub mode: WorkerMode,
}

/// What the command line asks for (D-067: `--help`/`--version` answer with exit code 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Run(Cli),
    Help,
    Version,
}

/// Like [`parse_args`], but `-h`/`--help`/`help` and `-V`/`--version`/`version` as the only
/// argument are answers, not errors.
pub fn parse_action<I: IntoIterator<Item = String>>(args: I) -> Result<Action, String> {
    let args: Vec<String> = args.into_iter().collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["-h" | "--help" | "help"] => Ok(Action::Help),
        ["-V" | "--version" | "version"] => Ok(Action::Version),
        _ => parse_args(args).map(Action::Run),
    }
}

/// `paperrust --version`: name and version from [`pl_core::brand`].
pub fn version_text() -> String {
    format!(
        "{} {} (compatible with paperless-ngx {})",
        pl_core::brand::BIN_NAME,
        pl_core::brand::PAPERRUST_VERSION,
        pl_core::brand::UPSTREAM_VERSION
    )
}

/// `paperrust --help`.
pub fn help_text() -> String {
    format!(
        "{} – {}\n\n{}\n\n\
         serve            HTTP server (default; may be omitted)\n  \
         --worker         also run worker, scheduler and consumption folder watcher\n  \
         --worker-only    only worker, scheduler and folder watcher (no HTTP server)\n  \
         --no-worker      HTTP server only (default)\n\
         manage <cmd>     management commands, e.g. `manage migrate`\n\
         migrate --from   switch assistant from paperless-ngx (docs/user/umstieg.md)\n\
         -h, --help       this help\n\
         -V, --version    print the version\n\n\
         Before the first start and after every update: `paperrust manage migrate`.\n\
         Configuration: environment variables PAPERRUST_* / paperrust.conf (docs/user/configuration.md).",
        version_text(),
        pl_core::brand::PRODUCT_NAME,
        usage()
    )
}

/// Parse `args` (without the program name).
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Cli, String> {
    let mut mode: Option<WorkerMode> = None;
    let mut seen_command = false;
    for a in args {
        let set = |m: WorkerMode, mode: &mut Option<WorkerMode>| -> Result<(), String> {
            match mode {
                Some(prev) if *prev != m => Err("--worker, --worker-only and --no-worker are mutually exclusive".into()),
                _ => {
                    *mode = Some(m);
                    Ok(())
                }
            }
        };
        match a.as_str() {
            "serve" if !seen_command => seen_command = true,
            "--worker" => set(WorkerMode::WithWorker, &mut mode)?,
            "--worker-only" => set(WorkerMode::WorkerOnly, &mut mode)?,
            "--no-worker" => set(WorkerMode::NoWorker, &mut mode)?,
            "-h" | "--help" => return Err(usage()),
            other => return Err(format!("unknown argument '{other}'\n{}", usage())),
        }
    }
    Ok(Cli { mode: mode.unwrap_or(WorkerMode::NoWorker) })
}

fn usage() -> String {
    "usage: paperrust [serve] [--worker | --worker-only | --no-worker]
       paperrust manage <command> [args...]
       paperrust migrate --from <paperless-ngx data> [options]   (switch from paperless-ngx)".into()
}

/// `paperrust migrate [switch|verify|detect] <args…>` → arguments for `paperrust-migrate-db`
/// (the switch assistant from paperless-ngx, docs/user/umstieg.md). Without a sub-command:
/// `switch`. Not to be confused with `paperrust manage migrate` (database schema).
pub fn migrate_args(args: &[String]) -> Option<Vec<String>> {
    if args.first().map(String::as_str) != Some("migrate") {
        return None;
    }
    let rest = &args[1..];
    Some(match rest.first().map(String::as_str) {
        Some("switch" | "verify" | "detect") => rest.to_vec(),
        _ => std::iter::once("switch".to_string()).chain(rest.iter().cloned()).collect(),
    })
}

/// Runs `paperrust-migrate-db` (next to this binary, else from PATH) and returns its exit code.
fn run_migrate_tool(args: Vec<String>) -> ExitCode {
    let name = format!("paperrust-migrate-db{}", std::env::consts::EXE_SUFFIX);
    let bin = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(&name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from(&name));
    match std::process::Command::new(&bin).args(&args).status() {
        Ok(st) => ExitCode::from(st.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(1)),
        Err(e) => {
            eprintln!("paperrust: {} not found or not startable ({e}); it is part of the PaperRust package", bin.display());
            ExitCode::FAILURE
        }
    }
}

/// `paperrust manage <args…>` → the arguments for pl-cli (without `manage`).
pub fn manage_args(args: &[String]) -> Option<Vec<String>> {
    (args.first().map(String::as_str) == Some("manage")).then(|| args[1..].to_vec())
}

/// `manage.py mail_fetcher` = `tasks.process_mail_accounts()` (synchronous, no Celery): pl-mail's
/// `process_mail_accounts` with this process's configuration and task queue (attachments go to
/// `consume_file` in the queue).
fn register_mail_fetcher() {
    let _ = pl_cli::mail::set_mail_fetcher(Box::new(|| {
        Box::pin(async {
            let core = pl_core::Config::from_env().map_err(|e| e.to_string())?;
            let queue_cfg = worker::queue_config(&core.env).map_err(|e| e.to_string())?;
            let state = AppState::new(core).map_err(|e| e.to_string())?;
            let setup = worker::setup_queue(&state, queue_cfg).await?;
            let mail = pl_mail::fetch::MailWorker::new(
                state.db().clone(),
                setup.queue.clone(),
                pl_mail::fetch::MailWorkerConfig::from_core(state.config()),
            );
            pl_mail::fetch::tasks::process_mail_accounts(&mail, None).await
        })
    }));
}

/// Console log (`PAPERRUST_LOG`, default `info`) plus the log files `paperrust.log` /
/// `mail.log` in `PAPERRUST_LOGGING_DIR` for the Logs view ([`crate::logfile`]). Logger names
/// show as `paperrust.*` ([`crate::logfile::display_logger`]).
fn init_tracing() {
    use tracing_subscriber::Layer as _;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    let filter = pl_core::config::var("PAPERRUST_LOG")
        .and_then(|f| EnvFilter::try_new(f).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    let files = crate::logfile::from_env().map(|l| l.with_filter(crate::logfile::filter()));
    // PAPERRUST_LOG live from the web UI (docs/SETTINGS.md §1.13, WP4c).
    let (filter, reload) = tracing_subscriber::reload::Layer::new(filter);
    crate::settings_live::set_log_reload(Box::new(move |f: &str| {
        let new = EnvFilter::try_new(f).map_err(|e| e.to_string())?;
        reload.reload(new).map_err(|e| e.to_string())
    }));
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(crate::logfile::ConsoleFormat)
                .with_filter(filter),
        )
        .with(files)
        .init();
}

/// Startup warning when the process-local cache (`LocMemCache` semantics) is used although more
/// than one process serves this installation (security review final H1): throttling, TOTP replay
/// protection, allauth rate limits and cache invalidation are then per process. Indicators:
/// `--worker-only` (the HTTP side runs elsewhere); `--no-worker` (a separate `--worker-only`
/// process runs the tasks). Remedy: the shared cache in PostgreSQL (`PAPERRUST_CACHE=postgres`).
pub fn local_cache_warning(local: bool, mode: WorkerMode) -> Option<String> {
    if !local {
        return None;
    }
    const FIX: &str = "set PAPERRUST_CACHE=postgres in every process for a shared cache";
    let why = match mode {
        WorkerMode::WorkerOnly => "worker-only process with a local cache backend: this worker and the HTTP process(es)",
        WorkerMode::NoWorker => "HTTP-only process with a local cache backend: this process and the separate worker process",
        WorkerMode::WithWorker => return None,
    };
    Some(format!("{why} do not share throttling, TOTP replay protection, login rate limits and cache invalidation; {FIX}"))
}

/// Shared state from pl-core (lazy pool). PaperRust needs PostgreSQL.
fn build_state(core: pl_core::Config) -> Result<AppState, String> {
    AppState::new(core).map_err(|e| match e {
        StateError::UnsupportedEngine(engine) => format!("database engine {engine:?} is not supported: PaperRust needs PostgreSQL (PAPERRUST_DBENGINE=postgresql)"),
        e => e.to_string(),
    })
}

/// Startup message: upstream URL patterns PaperRust does not serve (they answer 404) and whether
/// the web UI routes are on.
fn log_unported(plan: &routing::MountPlan) {
    let active: Vec<&str> = plan.active.iter().map(|r| r.path).collect();
    let mut missing = crate::frontend::urlconf::unported(active.iter().copied());
    // `/ws/status/` is served natively (frontline, not a route).
    missing.retain(|u| u.kind != crate::frontend::urlconf::Kind::Websocket);
    tracing::info!(missing = missing.len(), "upstream URL patterns without a PaperRust route answer 404 (HTML page, DRF JSON below /api/)");
    for u in &missing {
        tracing::debug!(pattern = u.pattern, view = u.view, "upstream URL pattern not served");
    }
    let ui = crate::frontend::UiFlags::from_ids(plan.active.iter().map(|r| r.id));
    if !ui.index || !ui.static_files {
        tracing::warn!(
            ui_index = ui.index,
            static_files = ui.static_files,
            "the web UI needs the route ids ui.index and static.files (switched off in routes.toml)"
        );
    }
}

/// Open one pool connection and run the per-request user query once on it
/// ([`pl_authz::user::warm_up`]): the first request then finds a ready connection with the
/// statement prepared. Only one – the pool hands out idle connections in turn, and several
/// half-warm connections made the following requests slower (measured, O1).
/// Errors only log: the pool opens connections on demand anyway.
async fn warm_pool(db: sqlx::PgPool) {
    let t0 = std::time::Instant::now();
    let r = async {
        let mut c = db.acquire().await?;
        pl_authz::user::warm_up(&mut c).await
    }
    .await;
    match r {
        Ok(()) => tracing::debug!(ms = t0.elapsed().as_millis() as u64, "database pool warmed up"),
        Err(e) => tracing::debug!(error = %e, "database pool warm-up failed"),
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let mut cfg = ServerConfig::from_env().map_err(|e| e.to_string())?;
    // Same configuration source as Django (environment + paperless.conf).
    let mut core = pl_core::Config::from_env().map_err(|e| e.to_string())?;
    // Regularly return free heap (HTTP, worker and both; see memtrim).
    crate::memtrim::start();
    // PAPERRUST_DB_REQUIRE_AUTH (unset: no check, like before) – before any connection sends the
    // password; then: database reachable (D-087) and migrated (A-181/B-312), else abort with a
    // clear message ([`crate::dbcheck`]). The database settings are bootstrap-only (class C), so
    // the overlay below cannot change them.
    pl_core::requireauth::check(&core.db).await?;
    crate::dbcheck::check(&core.db).await?;
    // Values saved in the web UI (paperrust_setting, docs/SETTINGS.md §2.1) count from the start,
    // including restart-only keys; re-read both configurations with them.
    if pl_settings::reload::startup_overlay(&core).await {
        cfg = ServerConfig::from_env().map_err(|e| e.to_string())?;
        core = pl_core::Config::from_env().map_err(|e| e.to_string())?;
    }
    pl_core::config::log_alias_warnings(&core.env);
    tracing::info!(
        version = pl_core::brand::PAPERRUST_VERSION,
        compat = pl_core::brand::UPSTREAM_VERSION,
        "{} {} (compatible with paperless-ngx {}; {})",
        pl_core::brand::PRODUCT_NAME,
        pl_core::brand::PAPERRUST_VERSION,
        pl_core::brand::UPSTREAM_VERSION,
        pl_core::brand::ATTRIBUTION
    );
    // PaperRust name/logo for the Rust `ui_settings.get` route when PAPERLESS_APP_* are unset.
    cfg.branding.apply_ui_defaults(&mut core);
    tracing::info!(
        enabled = cfg.branding.enabled,
        accent = %format!("hsl({},{}%,{}%)", cfg.branding.accent.h, cfg.branding.accent.s, cfg.branding.accent.l),
        "branding (PAPERRUST_BRANDING)"
    );
    cfg.settings = DjangoSettings::from_core(&core);
    let queue_cfg = worker::queue_config(&core.env).map_err(|e| e.to_string())?;
    let routes_cfg = routing::load_routes_file(&cfg.routes_file, cfg.routes_file_required).map_err(|e| e.to_string())?;
    let plan = routing::plan(crate::all_routes(), &routes_cfg).map_err(|e| e.to_string())?;
    let state = build_state(core)?;
    // Django's `default` cache: process-local, or shared in PostgreSQL (PAPERRUST_CACHE=postgres).
    let cache_backend = state.cache().backend();
    match cache_backend {
        pl_core::cache::Backend::Postgres => tracing::info!("cache: shared in PostgreSQL (paperrust_cache)"),
        pl_core::cache::Backend::Local => tracing::info!("cache: process-local (LocMemCache semantics)"),
    }
    // Security review final H1: warn whenever more than one process uses the process-local cache.
    if let Some(msg) = local_cache_warning(cache_backend == pl_core::cache::Backend::Local, cli.mode) {
        tracing::warn!("{msg}");
    }
    // O1: the idempotent migrations (different tables) at the same time, each on its own pooled
    // connection – the start waits for the slowest one instead of the sum.
    // The table is also needed with the process-local cache: login throttles and TOTP replay
    // locks always live there (S4, pl_core::cache); there a failure only falls back to per-process.
    let cache_migration = async {
        for (name, sql) in pl_core::cache::migrations::ALL {
            if let Err(e) = sqlx::raw_sql(sql).execute(state.db()).await {
                if cache_backend == pl_core::cache::Backend::Postgres {
                    return Err(format!("applying the cache migration {name} failed: {e}"));
                }
                tracing::warn!(migration = %name, error = %e, "cache: the shared table paperrust_cache could not be created; login throttling stays per process");
                state.keep_guarded_keys_local();
                break;
            }
        }
        Ok::<(), String>(())
    };
    let (setup, ai, cache) = tokio::join!(
        worker::setup_queue(&state, queue_cfg),
        worker::apply_ai_migration(&state, &plan),
        cache_migration
    );
    let setup = setup?;
    ai?;
    cache?;
    worker::install_hooks(&state, &setup)?;

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    // System settings (docs/SETTINGS.md WP3): snapshot as a service + LISTEN/NOTIFY.
    let settings = pl_settings::reload::install(&state, true, stop_rx.clone()).await;
    crate::settings_live::reload_log_filter(&settings, stop_rx.clone());
    let background = worker::start_background(&state, &setup, cli.mode, stop_rx).await?;
    // tests/chaos C1: detect DB outage and recovery without manual work (API immediately 503, then 200 again).
    tokio::spawn(pl_core::dbconn::watch_health((*state.db().connect_options()).clone()));

    let result = if cli.mode.serves_http() {
        for r in plan.active_info() {
            tracing::info!(id = r.id, method = %r.method, path = r.path, "rust route active");
        }
        tracing::info!(active = plan.active.len(), total = plan.total, "routes mounted");
        log_unported(&plan);
        let db = state.db().clone();
        let app = crate::build_app(&cfg, plan, state).map_err(|e| e.to_string())?;
        // O1: open a pool connection in the background (on Windows a new PostgreSQL backend
        // costs tens of ms), so the first request does not pay for it.
        tokio::spawn(warm_pool(db));
        if !cfg.bind.ip().is_loopback() {
            tracing::warn!(bind = %cfg.bind, "listening on a non-loopback address (PAPERRUST_BIND)");
        }
        let listener = tokio::net::TcpListener::bind(cfg.bind)
            .await
            .map_err(|e| format!("cannot bind {}: {e}", cfg.bind))?;
        tracing::info!(bind = %cfg.bind, stop_on_stdin_eof = cfg.stop_on_stdin_eof, "PaperRust listening");
        let stop = cfg.stop_on_stdin_eof;
        crate::serve::serve(listener, app, async move {
            crate::shutdown::signal(stop).await;
        })
        .await
        .map_err(|e| e.to_string())
    } else {
        tracing::info!("worker only (no HTTP server)");
        crate::shutdown::signal(cfg.stop_on_stdin_eof).await;
        Ok(())
    };
    tracing::info!("stopping worker, scheduler and consumer");
    let _ = stop_tx.send(true);
    background.join().await;
    result
}

/// Entry point of the `paperrust` binary. The binary calls
/// [`pl_core::config::apply_env_aliases`] first (single-threaded); the alias warnings are logged
/// here once the logger runs.
/// Stack of the `manage` commands (main thread and runtime threads), see [`main`].
const MANAGE_STACK: usize = 16 * 1024 * 1024;

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(m) = migrate_args(&args) {
        return run_migrate_tool(m);
    }
    if let Some(manage) = manage_args(&args) {
        // Django's manage.py logs to the console; pl-cli prints its own messages.
        init_tracing();
        pl_core::config::log_alias_warnings(&pl_core::config::Env::from_process());
        register_mail_fetcher();
        let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().thread_stack_size(MANAGE_STACK).build() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("paperrust: cannot start runtime: {e}");
                return ExitCode::FAILURE;
            }
        };
        // tests/chaos C4: `manage mail_fetcher` ran on the main thread (Windows: 1 MiB stack) into a
        // stack overflow ("thread 'main' has overflowed its stack", process gone, exit 127). The command
        // therefore runs on its own thread with a large stack.
        let code = match std::thread::Builder::new()
            .name("manage".into())
            .stack_size(MANAGE_STACK)
            .spawn(move || rt.block_on(pl_cli::run_async(manage)))
        {
            Ok(h) => h.join().unwrap_or(1),
            Err(e) => {
                eprintln!("paperrust: cannot start thread: {e}");
                return ExitCode::FAILURE;
            }
        };
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    let cli = match parse_action(args) {
        Ok(Action::Run(c)) => c,
        Ok(Action::Help) => {
            println!("{}", help_text());
            return ExitCode::SUCCESS;
        }
        Ok(Action::Version) => {
            println!("{}", version_text());
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("paperrust: {e}");
            return ExitCode::from(2);
        }
    };
    init_tracing();
    if cli.mode.runs_worker() {
        pl_core::resources::log_resource_plan();
    }
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("paperrust: cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = rt.block_on(run(cli));
    // Blocking tasks still running (e.g. a hung OCR call) must not keep the process alive.
    rt.shutdown_timeout(std::time::Duration::from_secs(5));
    match result {
        Ok(()) if pl_settings::restart::requested() => {
            tracing::info!("stopped for restart (exit code {})", pl_settings::restart::EXIT_CODE);
            ExitCode::from(pl_settings::restart::EXIT_CODE)
        }
        Ok(()) => {
            tracing::info!("stopped");
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!("{e}");
            eprintln!("paperrust: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Cli, String> {
        parse_args(a.iter().map(|s| s.to_string()))
    }

    /// H1: warn about the local cache whenever several processes are evident.
    #[test]
    fn local_cache_warning_cases() {
        use super::local_cache_warning as w;
        assert!(w(false, WorkerMode::WorkerOnly).is_none());
        assert!(w(true, WorkerMode::WithWorker).is_none());
        assert!(w(true, WorkerMode::WorkerOnly).unwrap().contains("worker-only"));
        let m = w(true, WorkerMode::NoWorker).unwrap();
        assert!(m.contains("separate worker") && m.contains("PAPERRUST_CACHE=postgres"));
    }

    /// D-067: `--help` and `--version` answer (exit 0 in `main`), name/version from pl_core::brand.
    #[test]
    fn help_and_version() {
        let a = |v: &[&str]| parse_action(v.iter().map(|s| s.to_string()));
        for h in ["--help", "-h", "help"] {
            assert_eq!(a(&[h]), Ok(Action::Help));
        }
        for v in ["--version", "-V", "version"] {
            assert_eq!(a(&[v]), Ok(Action::Version));
        }
        assert_eq!(a(&["serve", "--worker"]), Ok(Action::Run(Cli { mode: WorkerMode::WithWorker })));
        assert!(a(&["foo"]).is_err());
        assert!(a(&["serve", "--help"]).is_err());
        let v = version_text();
        assert!(v.starts_with(pl_core::brand::BIN_NAME) && v.contains(pl_core::brand::PAPERRUST_VERSION));
        let h = help_text();
        assert!(h.contains("--worker-only") && h.contains("manage migrate") && h.contains("--version"));
    }

    #[test]
    fn args() {
        assert_eq!(p(&[]).unwrap().mode, WorkerMode::NoWorker);
        assert_eq!(p(&["serve"]).unwrap().mode, WorkerMode::NoWorker);
        assert_eq!(p(&["serve", "--worker"]).unwrap().mode, WorkerMode::WithWorker);
        assert_eq!(p(&["--worker-only"]).unwrap().mode, WorkerMode::WorkerOnly);
        assert_eq!(p(&["serve", "--no-worker"]).unwrap().mode, WorkerMode::NoWorker);
        assert!(p(&["--worker", "--no-worker"]).is_err());
        assert!(p(&["--worker", "--worker"]).is_ok());
        assert!(p(&["serve", "serve"]).is_err());
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["manage", "migrate"]).is_err());
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(manage_args(&a(&["manage", "migrate", "--plan"])), Some(a(&["migrate", "--plan"])));
        assert_eq!(manage_args(&a(&["manage"])), Some(vec![]));
        assert_eq!(manage_args(&a(&["serve", "manage"])), None);
        assert_eq!(manage_args(&[]), None);
        assert_eq!(migrate_args(&a(&["migrate", "--from", "/x"])), Some(a(&["switch", "--from", "/x"])));
        assert_eq!(migrate_args(&a(&["migrate", "verify", "--from", "/x"])), Some(a(&["verify", "--from", "/x"])));
        assert_eq!(migrate_args(&a(&["manage", "migrate"])), None);
    }
}
