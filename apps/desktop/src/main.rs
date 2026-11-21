//! `paperrust-desktop` – minimal shell (system browser + console). The later Tauri shell calls
//! the same library (`Supervisor`).
//!
//! ```text
//! paperrust-desktop [run] [--no-browser]   start the app (default)
//! paperrust-desktop backup [TARGET]        backup (pg_dump + document_exporter); with the app running
//!                                          the instance backs up itself (only without TARGET)
//! paperrust-desktop manage <cmd> [args…]   paperrust manage … with the desktop database, app closed
//! paperrust-desktop stop                   stop the running instance in an orderly way (installer, scripts)
//! paperrust-desktop paths                  show data/installation paths
//! ```

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use paperrust_desktop::layout::InstallLayout;
use paperrust_desktop::paths::AppPaths;
use paperrust_desktop::supervisor::{AdminPrompt, Event, Opened, SetupChoice, SetupOptions, Supervisor, detect_setup, setup_from_env};
use paperrust_desktop::{DesktopError, Result};

#[derive(Debug, PartialEq, Eq)]
enum Cmd {
    Run { browser: bool },
    Backup(Option<PathBuf>),
    Manage(Vec<String>),
    Stop,
    Paths,
    Help,
    Version,
}

fn usage() -> &'static str {
    "usage: paperrust-desktop [run] [--no-browser]
       paperrust-desktop backup [ZIEL]
       paperrust-desktop manage <command> [args...]
       paperrust-desktop stop
       paperrust-desktop paths"
}

fn parse(args: &[String]) -> std::result::Result<Cmd, String> {
    match args.first().map(String::as_str) {
        None => Ok(Cmd::Run { browser: true }),
        Some("run") | Some("--no-browser") => {
            let rest = if args[0] == "run" { &args[1..] } else { args };
            match rest {
                [] => Ok(Cmd::Run { browser: true }),
                [f] if f == "--no-browser" => Ok(Cmd::Run { browser: false }),
                _ => Err(usage().into()),
            }
        }
        Some("backup") => match &args[1..] {
            [] => Ok(Cmd::Backup(None)),
            [d] => Ok(Cmd::Backup(Some(PathBuf::from(d)))),
            _ => Err(usage().into()),
        },
        Some("manage") if args.len() > 1 => Ok(Cmd::Manage(args[1..].to_vec())),
        Some("paths") if args.len() == 1 => Ok(Cmd::Paths),
        Some("stop") if args.len() == 1 => Ok(Cmd::Stop),
        // Overview/version on stdout with exit 0; wrong invocations stay exit 2
        Some("--help" | "-h" | "help") if args.len() == 1 => Ok(Cmd::Help),
        Some("--version" | "-V") if args.len() == 1 => Ok(Cmd::Version),
        _ => Err(usage().into()),
    }
}

/// Console dialog (only the first-start choice; "start fresh" creates the local owner without
/// user name/password, docs/DESKTOP.md "Anmeldung im Desktop-Betrieb"); without a terminal via
/// `PAPERRUST_DESKTOP_ADMIN_{USER,EMAIL,PASSWORD}` or `PAPERRUST_DESKTOP_IMPORT`/
/// `PAPERRUST_DESKTOP_IMPORT_DB` (see `supervisor::setup_from_env`).
struct ConsolePrompt;

fn console_line(prompt: &str) -> Option<String> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut s = String::new();
    (std::io::stdin().lock().read_line(&mut s).ok()? > 0).then(|| s.trim_end_matches(['\r', '\n']).to_string())
}

impl AdminPrompt for ConsolePrompt {
    fn choose_setup(&self, o: &SetupOptions) -> Option<SetupChoice> {
        if let Some(c) = setup_from_env(|k| std::env::var(k).ok()) {
            return Some(c);
        }
        eprintln!(
            "\nErst-Start: 1 = neu beginnen, 2 = von paperless-ngx umsteigen (Export-Ordner/ZIP, \
             Datenordner mit SQLite oder Datenbank-URL – wird erkannt)"
        );
        if o.can_migrate_db {
            eprintln!("            3 = bestehende paperless-ngx-Datenbank übernehmen (URL von Hand)");
        }
        match console_line("Auswahl [1]: ")?.trim() {
            "2" => loop {
                let input = console_line("Quelle (Export, paperless-Ordner, data-Ordner oder sqlite:///…/postgres://…): ")?;
                match detect_setup(&input, None) {
                    Ok(SetupChoice::MigrateDb { source_url, media_dir: None }) => {
                        let media = console_line("Alter Medienordner (MEDIA_ROOT, leer = Dateien nicht übernehmen): ")?;
                        break Some(SetupChoice::MigrateDb {
                            source_url,
                            media_dir: (!media.trim().is_empty()).then(|| PathBuf::from(media.trim())),
                        });
                    }
                    Ok(c) => {
                        eprintln!(
                            "Erkannt: {}",
                            match &c {
                                SetupChoice::ImportExport { .. } => "paperless-ngx-Export → Import",
                                _ => "paperless-ngx-Datenbank → Übernahme (Medienordner gefunden)",
                            }
                        );
                        break Some(c);
                    }
                    Err(e) => eprintln!("{e}"),
                }
            },
            "3" if o.can_migrate_db => {
                let source_url = console_line("Quelle (z. B. sqlite:///C:/paperless/data/db.sqlite3): ")?;
                let media = console_line("Alter Medienordner (MEDIA_ROOT, leer = später kopieren): ")?;
                Some(SetupChoice::MigrateDb {
                    source_url: source_url.trim().to_string(),
                    media_dir: (!media.trim().is_empty()).then(|| PathBuf::from(media.trim())),
                })
            }
            _ => Some(SetupChoice::Fresh(None)),
        }
    }
}

/// With login key (`run/desktop-login.token`): `/rs/desktop/login?token=…&next=/` – the
/// browser is then logged in without user name/password. Without a key the normal address.
/// After logout or session expiry, a new start (start menu/shortcut) logs in
/// automatically again (a second instance opens this URL).
fn login_or_plain(run_dir: &std::path::Path, url: &str) -> String {
    paperrust_desktop::autologin::login_url_for(run_dir, url, "/").unwrap_or_else(|| url.to_string())
}

/// Open the URL in the default browser (without shell interpretation of the URL, outside the job object).
fn open_browser(url: &str) {
    paperrust_desktop::job::open_external(url);
}

/// Quit request: Ctrl+C; Windows additionally closing the console/logging off; Unix SIGTERM/SIGHUP;
/// everywhere `paperrust-desktop stop` (file `run/stop.request`).
async fn shutdown_requested(stop_file: PathBuf) {
    tokio::select! {
        () = signals() => {}
        () = paperrust_desktop::lock::stop_requested(stop_file) => {}
    }
    tracing::info!("Beenden angefordert");
}

async fn signals() {
    #[cfg(windows)]
    {
        use tokio::signal::windows;
        let (Ok(mut close), Ok(mut shut), Ok(mut brk)) = (windows::ctrl_close(), windows::ctrl_shutdown(), windows::ctrl_break()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = close.recv() => {}
            _ = shut.recv() => {}
            _ = brk.recv() => {}
        }
    }
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut hup)) = (signal(SignalKind::terminate()), signal(SignalKind::hangup())) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
    }
}

async fn real_main(cmd: Cmd) -> Result<i32> {
    let paths = AppPaths::from_env()?;
    paths.ensure_dirs()?;
    // Open the log only after the single-instance lock (and rotate): `paths`, `stop` or a
    // second start would otherwise rotate away the desktop.log of the running instance (smoke test D1).
    let layout = InstallLayout::from_current_exe().map_err(|e| DesktopError::Other(format!("Programmpfad: {e}")))?;
    if cmd == Cmd::Paths {
        println!("Daten:        {}", paths.home.display());
        println!("Konfiguration {}", paths.config_file.display());
        println!("Logs:         {}", paths.logs.display());
        println!("Installation: {}", layout.root.display());
        println!("PostgreSQL:   {}", layout.pg_bin.display());
        println!("Server:       {}", layout.server_bin.display());
        let opt = |p: &Option<PathBuf>| p.as_ref().map_or_else(|| "– (nicht gefunden)".to_string(), |p| p.display().to_string());
        println!("routes.toml:  {}", opt(&layout.routes_file));
        println!("Oberfläche:   {}", opt(&layout.static_dir));
        println!("Tesseract:    {}", opt(&layout.tesseract));
        println!("tessdata:     {}", opt(&layout.tessdata));
        println!("PDFium:       {}", opt(&layout.pdfium));
        for m in layout.missing(|p| p.exists()) {
            println!("FEHLT:        {m}");
        }
        return Ok(0);
    }
    if cmd == Cmd::Stop {
        let r = paperrust_desktop::lock::request_stop(
            &paths.lock_file(),
            &paths.instance_file(),
            &paths.stop_request_file(),
            std::time::Duration::from_secs(120),
        )
        .await?;
        use paperrust_desktop::lock::StopOutcome;
        return match r {
            StopOutcome::NotRunning => {
                eprintln!("PaperRust läuft nicht.");
                Ok(0)
            }
            StopOutcome::Stopped => {
                eprintln!("PaperRust beendet.");
                Ok(0)
            }
            StopOutcome::TimedOut => {
                Err(DesktopError::Other("PaperRust hat sich nach 120 s nicht beendet (logs/desktop.log prüfen)".into()))
            }
        };
    }
    // Backup with the app running: the instance backs up itself (like tray "Sicherung jetzt").
    if let Cmd::Backup(None) = &cmd {
        // `None` = app is not running: back up here below.
        if let Some(dir) = paperrust_desktop::supervisor::request_live_backup(&paths, std::time::Duration::from_secs(6 * 3600)).await? {
            eprintln!("Sicherung (laufende Instanz) fertig: {}", dir.display());
            return Ok(0);
        }
    }
    // Login key of the (running) instance is located here (automatic login in the browser).
    let run_dir = paths.run.clone();
    let sup = match Supervisor::open(paths, layout)? {
        Opened::Ready(s) => {
            let log = paperrust_desktop::logs::init(&s.paths.logs).map_err(|e| DesktopError::Other(format!("Log öffnen: {e}")))?;
            tracing::info!(log = %log.display(), home = %s.paths.home.display(), "PaperRust Desktop {}", env!("CARGO_PKG_VERSION"));
            if s.config_created {
                tracing::info!(file = %s.paths.config_file.display(), "neue Desktop-Konfiguration erzeugt");
            }
            for n in &s.notices {
                tracing::warn!("{n}");
            }
            *s
        }
        Opened::AlreadyRunning(url) => {
            return match (&cmd, url) {
                (Cmd::Run { browser: true }, Some(u)) => {
                    open_browser(&login_or_plain(&run_dir, &u));
                    Ok(0)
                }
                (_, u) => Err(DesktopError::AlreadyRunning(u.unwrap_or_else(|| "startet gerade".into()))),
            };
        }
    };
    match cmd {
        Cmd::Run { browser } => {
            let browser = browser && sup.cfg.server.open_browser;
            let on_event = move |e: Event| match e {
                Event::Ready { url } => {
                    tracing::info!(%url, "PaperRust bereit");
                    eprintln!("\nPaperRust läuft: {url}  (Beenden: Strg+C)");
                    if browser {
                        open_browser(&login_or_plain(&run_dir, &url));
                    }
                }
                other => tracing::info!(event = ?other, "Status"),
            };
            let stop_file = sup.paths.stop_request_file();
            sup.run(&ConsolePrompt, &on_event, shutdown_requested(stop_file)).await?;
            Ok(0)
        }
        Cmd::Backup(target) => {
            let dir = sup.backup(target).await?;
            eprintln!("Sicherung fertig: {}", dir.display());
            Ok(0)
        }
        Cmd::Manage(args) => sup.manage(&args).await,
        Cmd::Paths | Cmd::Stop | Cmd::Help | Cmd::Version => Ok(0),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse(&args) {
        Ok(c) => c,
        Err(u) => {
            eprintln!("{u}");
            return ExitCode::from(2);
        }
    };
    match cmd {
        Cmd::Help => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Cmd::Version => {
            println!("paperrust-desktop {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    // Child processes (PostgreSQL, server) die with this process (Windows job object).
    if matches!(cmd, Cmd::Run { .. } | Cmd::Backup(_) | Cmd::Manage(_))
        && let Err(e) = paperrust_desktop::job::kill_children_with_app()
    {
        eprintln!("paperrust-desktop: Job-Objekt: {e} (Kindprozesse überleben einen Absturz)");
    }
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("paperrust-desktop: Laufzeit: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(real_main(cmd)) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            tracing::error!("{e}");
            eprintln!("paperrust-desktop: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> std::result::Result<Cmd, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn cli() {
        assert_eq!(p(&[]).unwrap(), Cmd::Run { browser: true });
        assert_eq!(p(&["run"]).unwrap(), Cmd::Run { browser: true });
        assert_eq!(p(&["run", "--no-browser"]).unwrap(), Cmd::Run { browser: false });
        assert_eq!(p(&["--no-browser"]).unwrap(), Cmd::Run { browser: false });
        assert_eq!(p(&["backup"]).unwrap(), Cmd::Backup(None));
        assert_eq!(p(&["backup", "D:/b"]).unwrap(), Cmd::Backup(Some(PathBuf::from("D:/b"))));
        assert_eq!(p(&["manage", "document_importer", "x"]).unwrap(), Cmd::Manage(vec!["document_importer".into(), "x".into()]));
        assert_eq!(p(&["paths"]).unwrap(), Cmd::Paths);
        assert_eq!(p(&["stop"]).unwrap(), Cmd::Stop);
        assert!(p(&["stop", "x"]).is_err());
        assert!(p(&["manage"]).is_err());
        assert!(p(&["run", "x"]).is_err());
        assert!(p(&["bogus"]).is_err());
        assert_eq!(p(&["--help"]).unwrap(), Cmd::Help);
        assert_eq!(p(&["-h"]).unwrap(), Cmd::Help);
        assert_eq!(p(&["--version"]).unwrap(), Cmd::Version);
        assert_eq!(p(&["-V"]).unwrap(), Cmd::Version);
        assert!(p(&["--version", "x"]).is_err());
    }
}
