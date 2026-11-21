//! `paperrust` as a child process: environment (only `PAPERLESS_*`/`PAPERRUST_*` from the desktop
//! configuration, inherited ones are removed), start, health check (`GET /__rs/health`), stop.
//!
//! Why a child process instead of in-process (docs/DESKTOP.md "Server: Kindprozess"): crash isolation,
//! the same binary as server operation, the supervisor builds without pl-server (small, fast), and
//! `pl_server::cli::run` reads its configuration from the process environment anyway.
//!
//! **Clean stop:** the child's stdin is a pipe; the supervisor closes it to terminate.
//! Unix additionally `SIGINT` (which `paperrust` already listens to today, `tokio::signal::ctrl_c`).
//! Windows: Ctrl+C cannot be delivered to a process without a console → `paperrust` listens for
//! stdin EOF (`PAPERRUST_STOP_ON_STDIN_EOF=1`, pl-server `shutdown.rs`, W22) and shuts down server and
//! worker in an orderly way. Only after the grace period `TerminateProcess` (PostgreSQL transactions
//! stay consistent; running tasks are handed out again by the queue after
//! `PAPERRUST_QUEUE_VISIBILITY_TIMEOUT`).
//! Side effect of the pipe: if the supervisor dies, the child sees EOF and ends as well.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};

use crate::config::DesktopConfig;
use crate::layout::InstallLayout;
use crate::paths::AppPaths;
use crate::pg::{detach_console, scrub_env};
use crate::{DesktopError, Result, io_err};

pub fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Address for share links on the LAN when `server.lan_sharing` is on: `server.lan_address` or
/// the IPv4 of the network card with the default route ([`detect_lan_ipv4`]). `None` = local only
/// (sharing off, or no LAN address found – then with a warning).
pub fn lan_host(cfg: &DesktopConfig) -> Option<String> {
    if !cfg.server.lan_sharing {
        return None;
    }
    let fixed = cfg.server.lan_address.trim();
    if !fixed.is_empty() {
        return Some(fixed.to_string());
    }
    match detect_lan_ipv4() {
        Some(ip) => Some(ip.to_string()),
        None => {
            tracing::warn!(
                "lan_sharing: keine LAN-Adresse gefunden (kein Netzwerk?) – App bleibt nur auf 127.0.0.1; server.lan_address setzen"
            );
            None
        }
    }
}

/// IPv4 of the interface through which the system routes to the network: "connect" a UDP socket (sends
/// nothing) and read the local address. `None` without network or for loopback/link-local.
pub fn detect_lan_ipv4() -> Option<std::net::Ipv4Addr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // TEST-NET-1 (RFC 5737), only for route selection
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local() => Some(ip),
        _ => None,
    }
}

/// Complete environment for `paperrust serve --worker` and `paperrust manage …`.
pub fn server_env(cfg: &DesktopConfig, paths: &AppPaths, layout: &InstallLayout, db_port: u16, app_port: u16) -> BTreeMap<String, String> {
    let p = |x: &Path| x.display().to_string();
    let mut e = BTreeMap::new();
    let mut set = |k: &str, v: String| {
        e.insert(k.to_string(), v);
    };
    // Datenbank (upstream-kompatible Namen)
    set("PAPERLESS_DBENGINE", "postgresql".into());
    set("PAPERLESS_DBHOST", "127.0.0.1".into());
    set("PAPERLESS_DBPORT", db_port.to_string());
    set("PAPERLESS_DBNAME", cfg.database.name.clone());
    // D-N3: application role without superuser rights (owner of the DB), not the cluster superuser.
    set("PAPERLESS_DBUSER", cfg.database.app_user.clone());
    set("PAPERLESS_DBPASS", cfg.database.app_password.clone());
    set("PAPERLESS_DBSSLMODE", "disable".into());
    // D-N3/CORE-REQUEST 7: accept only SCRAM – a foreign service on the DB port gets no
    // plaintext/MD5 password (pl-core `requireauth`, probe at startup).
    set("PAPERRUST_DB_REQUIRE_AUTH", "scram-sha-256".into());
    // Pfade
    set("PAPERLESS_DATA_DIR", p(&paths.data));
    set("PAPERLESS_MEDIA_ROOT", p(&paths.media));
    set("PAPERLESS_CONSUMPTION_DIR", p(&paths.consume));
    set("PAPERLESS_SCRATCH_DIR", p(&paths.scratch));
    // UI logs (paperrust.log, mail.log; older paperless.log) next to desktop.log/server.log
    set("PAPERLESS_LOGGING_DIR", p(&paths.logs));
    if let Some(s) = &layout.static_dir {
        set("PAPERLESS_STATICDIR", p(s));
    }
    // Web. Default: only 127.0.0.1. With `lan_sharing` (opt-in, section 21 of docs/DESKTOP.md)
    // additionally on the LAN; share links then point to the LAN address.
    set("PAPERLESS_SECRET_KEY", cfg.server.secret_key.clone());
    match lan_host(cfg) {
        Some(host) => {
            set("PAPERLESS_URL", format!("http://{host}:{app_port}"));
            set("PAPERLESS_ALLOWED_HOSTS", format!("127.0.0.1,localhost,{host}"));
            set("PAPERRUST_BIND", format!("0.0.0.0:{app_port}"));
            set("PAPERRUST_SHARE_MODE", "lan".into());
        }
        None => {
            set("PAPERLESS_URL", base_url(app_port));
            set("PAPERLESS_ALLOWED_HOSTS", "127.0.0.1,localhost".into());
            set("PAPERRUST_BIND", format!("127.0.0.1:{app_port}"));
            set("PAPERRUST_SHARE_MODE", "local".into());
        }
    }
    if !cfg.setup.settings_migrated {
        // before the one-time move into the database (supervisor::migrate_settings)
        set("PAPERLESS_TIME_ZONE", cfg.server.time_zone.clone());
        set("PAPERLESS_OCR_LANGUAGE", cfg.server.ocr_language.clone());
    }
    // Desktop settings from the UI via the run folder (docs/SETTINGS.md §2.6).
    set("PAPERRUST_SUPERVISOR_CONTROL", p(&paths.run));
    if let Some(r) = &layout.routes_file {
        set("PAPERRUST_ROUTES", p(r));
    }
    // Parallelism: PAPERLESS_TASK_WORKERS / PAPERLESS_THREADS_PER_WORKER are deliberately NOT
    // set here (and removed by `scrub_env`). `paperrust` then limits the OCR threads at startup
    // itself according to free memory (pl-core `resources`, log line "parallel OCR: …").
    // Whoever wants fixed values puts them in `<home>/paperless.conf` – those apply unchanged.
    // Queue in PostgreSQL and in-process cache are the only ways (no Redis/Celery).
    set("PAPERRUST_WORKER_NAME", "desktop".into());
    // Restart from the UI (docs/SETTINGS.md E9): the supervisor restarts on exit code 75.
    set("PAPERRUST_RESTART_SUPERVISED", "1".into());
    // Mitgelieferte OCR-/PDF-Bausteine (optional)
    if let Some(t) = &layout.tesseract {
        set("PAPERRUST_TESSERACT_PATH", p(t));
    }
    if let Some(t) = &layout.tessdata {
        set("PAPERRUST_TESSDATA_DIR", p(t));
    }
    if let Some(t) = &layout.pdfium {
        set("PAPERRUST_PDFIUM_PATH", p(t));
    }
    // Text thumbnails: bundled font instead of the Linux path from the upstream default
    if let Some(f) = &layout.thumbnail_font {
        set("PAPERLESS_THUMBNAIL_FONT_NAME", p(f));
    }
    // Logs go to files: no ANSI color codes (tracing-subscriber honors NO_COLOR).
    set("NO_COLOR", "1".into());
    set("PAPERRUST_LOG", std::env::var("PAPERRUST_DESKTOP_SERVER_LOG").unwrap_or_else(|_| "info".into()));
    // Origin "Desktop app" instead of "environment" in the UI (docs/SETTINGS.md §2.6).
    let managed: Vec<String> = e
        .keys()
        .filter(|k| k.starts_with("PAPERLESS_") || k.starts_with("PAPERRUST_"))
        .filter(|k| !matches!(k.as_str(), "PAPERRUST_SUPERVISOR_CONTROL" | "PAPERRUST_RESTART_SUPERVISED"))
        .cloned()
        .collect();
    e.insert("PAPERRUST_SUPERVISOR_MANAGED_KEYS".into(), managed.join(","));
    e
}

/// Variables whose value is never logged.
pub fn is_secret(key: &str) -> bool {
    matches!(key, "PAPERLESS_DBPASS" | "PAPERLESS_SECRET_KEY" | "DJANGO_SUPERUSER_PASSWORD" | "PGPASSWORD" | crate::autologin::TOKEN_ENV)
}

#[derive(Clone)]
pub struct ServerSpec {
    pub program: PathBuf,
    pub env: BTreeMap<String, String>,
    /// Working directory: `<home>/run` (pl-core otherwise looks for `../paperless.conf` relative to it).
    pub cwd: PathBuf,
    pub log_dir: PathBuf,
}

/// D-N1: `Debug` without secrets (DB password, `SECRET_KEY` …).
impl std::fmt::Debug for ServerSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env: BTreeMap<&str, &str> = self.env.iter().map(|(k, v)| (k.as_str(), if is_secret(k) { "***" } else { v.as_str() })).collect();
        f.debug_struct("ServerSpec")
            .field("program", &self.program)
            .field("env", &env)
            .field("cwd", &self.cwd)
            .field("log_dir", &self.log_dir)
            .finish()
    }
}

impl ServerSpec {
    fn command(&self, args: &[&str], extra_env: &[(&str, &str)]) -> Command {
        let mut c = Command::new(&self.program);
        scrub_env(&mut c);
        detach_console(&mut c);
        c.args(args).envs(&self.env).current_dir(&self.cwd);
        for (k, v) in extra_env {
            c.env(k, v);
        }
        c
    }

    /// `paperrust manage <args…>`; output to `manage.log`. Returns: exit status.
    pub async fn manage(&self, args: &[&str], extra_env: &[(&str, &str)], timeout: Duration) -> Result<ExitStatus> {
        let log_path = self.log_dir.join("manage.log");
        let _ = crate::logs::rotate_if_larger(&log_path, crate::logs::max_bytes(), crate::logs::KEEP);
        let mut log = crate::logs::open_append(&log_path).map_err(io_err("manage.log öffnen"))?;
        {
            use std::io::Write;
            let _ = writeln!(log, "=== {} manage {}", unix_now(), args.join(" "));
        }
        let err = log.try_clone().map_err(io_err("manage.log"))?;
        let mut full = vec!["manage"];
        full.extend_from_slice(args);
        let mut c = self.command(&full, extra_env);
        c.stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(err)).kill_on_drop(true);
        let mut child = c.spawn().map_err(io_err(format!("{} starten", self.program.display())))?;
        match tokio::time::timeout(timeout, child.wait()).await {
            Ok(r) => r.map_err(io_err("paperrust manage")),
            Err(_) => {
                let _ = child.kill().await;
                Err(DesktopError::Server(format!("manage {}: Zeitüberschreitung ({} s)", args.join(" "), timeout.as_secs())))
            }
        }
    }

    /// Starts `paperrust serve --worker`. `PAPERRUST_INSTANCE_ID` is a random ID of this
    /// start; the health check only accepts a response that returns it as `instance`
    /// (D-N5, CORE-REQUEST 6, implemented since R1).
    pub fn spawn_server(&self) -> Result<ServerProcess> {
        let log_path = self.log_dir.join("server.log");
        crate::logs::rotate(&log_path, crate::logs::KEEP).map_err(io_err("server.log rotieren"))?;
        // Output via pipes through the supervisor: size rotation during operation (logs.rs). If the
        // supervisor dies, the child ends anyway (stdin EOF or job object).
        let log = std::sync::Arc::new(std::sync::Mutex::new(
            crate::logs::RotatingFile::open(&log_path, crate::logs::max_bytes(), crate::logs::KEEP).map_err(io_err("server.log öffnen"))?,
        ));
        let instance_id = crate::config::random_secret(24);
        let mut c = self.command(&["serve", "--worker"], &[("PAPERRUST_STOP_ON_STDIN_EOF", "1"), ("PAPERRUST_INSTANCE_ID", &instance_id)]);
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = c.spawn().map_err(io_err(format!("{} starten", self.program.display())))?;
        if let Some(o) = child.stdout.take() {
            tokio::spawn(crate::logs::pump(o, log.clone()));
        }
        if let Some(e) = child.stderr.take() {
            tokio::spawn(crate::logs::pump(e, log));
        }
        let stdin = child.stdin.take();
        Ok(ServerProcess { child, stdin, instance_id })
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct ServerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    instance_id: String,
}

impl ServerProcess {
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Has the child already ended? (`Some(status)`)
    pub fn try_exited(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    pub async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Waits for `GET /__rs/health` → 200 **from this child**; aborts if the child ends
    /// beforehand. D-N5: a foreign process that took the port between port selection and start
    /// does not count as "healthy": the response must carry `"service":"paperrust"` and (if
    /// output) our instance ID, the child must still be alive afterwards, and on Windows
    /// the listening socket must belong to our child. On [`DesktopError::PortTaken`] the
    /// supervisor tries once with a new port.
    pub async fn wait_healthy(&mut self, port: u16, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        loop {
            if let Some(st) = self.try_exited() {
                let msg = format!("beendet beim Start ({st}); Details in logs/server.log");
                return Err(if crate::ports::is_listening(port) { DesktopError::PortTaken(msg) } else { DesktopError::Server(msg) });
            }
            if let Some(h) = health_probe(port).await
                && h.ok
            {
                if !h.is_ours(&self.instance_id) {
                    return Err(DesktopError::PortTaken(format!("auf 127.0.0.1:{port} antwortet ein anderer Dienst")));
                }
                if let Some(st) = self.try_exited() {
                    return Err(DesktopError::PortTaken(format!("beendet beim Start ({st}), Port {port} von anderem Prozess belegt")));
                }
                if let (Some(pid), Some(owner)) = (self.pid(), listener_pid(port).await)
                    && owner != pid
                {
                    return Err(DesktopError::PortTaken(format!(
                        "127.0.0.1:{port} gehört Prozess {owner}, nicht dem PaperRust-Server ({pid})"
                    )));
                }
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(DesktopError::Server(format!("nach {} s nicht bereit", timeout.as_secs())));
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// Stop cleanly: close stdin (+ SIGINT on Unix), wait `grace`, then terminate hard.
    pub async fn stop(mut self, grace: Duration) -> Result<()> {
        if self.try_exited().is_some() {
            return Ok(());
        }
        if let Some(mut s) = self.stdin.take() {
            let _ = s.shutdown().await;
            drop(s);
        }
        #[cfg(unix)]
        if let Some(pid) = self.child.id().and_then(|p| i32::try_from(p).ok()) {
            // SAFETY: kill(2) with the PID of our own child that has not been reaped yet.
            unsafe {
                libc::kill(pid, libc::SIGINT);
            }
        }
        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(Ok(st)) => {
                tracing::info!(status = %st, "PaperRust-Server beendet");
                Ok(())
            }
            _ => {
                tracing::warn!(grace_s = grace.as_secs(), "PaperRust-Server reagiert nicht – wird hart beendet");
                self.child.kill().await.map_err(io_err("PaperRust-Server beenden"))
            }
        }
    }
}

/// Response of `GET /__rs/health` (status line + body, without an HTTP library).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub ok: bool,
    pub body: String,
}

const SERVICE_FIELD: &str = r#""service":"paperrust""#;
const INSTANCE_FIELD: &str = r#""instance":""#;

impl Health {
    /// Does the response come from exactly this `paperrust` start? Since CORE-REQUEST 6 (R1, d847e3b)
    /// pl-server outputs `PAPERRUST_INSTANCE_ID` as field `instance` – the check is now
    /// enforced: without a matching `instance` the response counts as foreign (D-N5).
    pub fn is_ours(&self, instance_id: &str) -> bool {
        let compact: String = self.body.chars().filter(|c| !c.is_whitespace()).collect();
        if !compact.contains(SERVICE_FIELD) || instance_id.is_empty() {
            return false;
        }
        compact.find(INSTANCE_FIELD).is_some_and(|i| compact[i + INSTANCE_FIELD.len()..].split('"').next() == Some(instance_id))
    }
}

/// `GET /__rs/health` → 200?
pub async fn health(port: u16) -> bool {
    health_probe(port).await.is_some_and(|h| h.ok)
}

/// `GET /__rs/health` on 127.0.0.1; `None` = no connection/response.
pub async fn health_probe(port: u16) -> Option<Health> {
    let probe = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        let req = format!("GET /__rs/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.ok()?;
        // Response is small (JSON < 1 KiB); `Connection: close` → the server closes afterwards.
        let mut buf = Vec::with_capacity(1024);
        let _ = (&mut s).take(16 * 1024).read_to_end(&mut buf).await;
        let text = String::from_utf8_lossy(&buf);
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        Some(Health { ok: is_ok_status_line(&buf), body: body.to_string() })
    };
    tokio::time::timeout(Duration::from_secs(3), probe).await.ok().flatten()
}

/// Windows: PID of the process listening on `127.0.0.1:port` (`netstat -ano -p TCP`).
/// Other systems or on errors: `None` (the check is then skipped).
pub async fn listener_pid(port: u16) -> Option<u32> {
    if !cfg!(windows) {
        return None;
    }
    let mut c = Command::new("netstat");
    c.args(["-ano", "-p", "TCP"]).stdin(Stdio::null());
    detach_console(&mut c);
    let out = tokio::time::timeout(Duration::from_secs(10), c.output()).await.ok()?.ok()?;
    parse_netstat_listener(&String::from_utf8_lossy(&out.stdout), port)
}

/// Line `TCP 127.0.0.1:<port> 0.0.0.0:0 <status> <PID>`; the status is localized
/// ("LISTENING"/"ABHÖREN"), so a listener is recognized by the foreign address `0.0.0.0:0`.
pub fn parse_netstat_listener(out: &str, port: u16) -> Option<u32> {
    let local = format!("127.0.0.1:{port}");
    out.lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() >= 5 && f[0] == "TCP" && f[1] == local && f[2] == "0.0.0.0:0" { f[f.len() - 1].parse().ok() } else { None }
    })
}

pub fn is_ok_status_line(head: &[u8]) -> bool {
    head.starts_with(b"HTTP/1.1 200") || head.starts_with(b"HTTP/1.0 200")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (DesktopConfig, AppPaths, InstallLayout) {
        let cfg = DesktopConfig::generate();
        let paths = AppPaths::under("/home/u/paperrust");
        let layout = InstallLayout::resolve(Path::new("/opt/pr"), |_| None, |_| true);
        (cfg, paths, layout)
    }

    /// docs/SETTINGS.md §2.6: control folder, managed keys, move of time zone/OCR language.
    #[test]
    fn env_settings_control_and_migration() {
        let (mut cfg, paths, layout) = fixture();
        let e = server_env(&cfg, &paths, &layout, 54_417, 58_080);
        assert_eq!(e["PAPERRUST_SUPERVISOR_CONTROL"], paths.run.display().to_string());
        assert!(e.contains_key("PAPERLESS_TIME_ZONE") && e.contains_key("PAPERLESS_OCR_LANGUAGE"));
        let managed: Vec<&str> = e["PAPERRUST_SUPERVISOR_MANAGED_KEYS"].split(',').collect();
        assert!(managed.contains(&"PAPERLESS_DBHOST") && managed.contains(&"PAPERLESS_TIME_ZONE"));
        assert!(!managed.contains(&"PAPERRUST_SUPERVISOR_CONTROL"));
        cfg.setup.settings_migrated = true;
        let e = server_env(&cfg, &paths, &layout, 54_417, 58_080);
        assert!(!e.contains_key("PAPERLESS_TIME_ZONE") && !e.contains_key("PAPERLESS_OCR_LANGUAGE"));
        assert!(!e["PAPERRUST_SUPERVISOR_MANAGED_KEYS"].contains("PAPERLESS_TIME_ZONE"));
    }

    #[test]
    fn env_is_standalone_and_loopback() {
        let (cfg, paths, layout) = fixture();
        let e = server_env(&cfg, &paths, &layout, 54_417, 58_080);
        // no switches for Django/Celery/Redis any more (they would only give warnings at startup)
        for k in ["PAPERRUST_QUEUE", "PAPERRUST_DJANGO_URL", "PAPERLESS_CACHE_BACKEND", "PAPERLESS_REDIS"] {
            assert!(!e.contains_key(k), "{k}");
        }
        // supervisor restarts on exit code 75 (docs/SETTINGS.md E9)
        assert_eq!(e["PAPERRUST_RESTART_SUPERVISED"], "1");
        assert_eq!(e["PAPERLESS_DBHOST"], "127.0.0.1");
        assert_eq!(e["PAPERLESS_DBPORT"], "54417");
        assert_eq!(e["PAPERRUST_BIND"], "127.0.0.1:58080");
        // automatic OCR limiting in the server (pl-core resources) stays active
        for k in ["PAPERLESS_TASK_WORKERS", "PAPERLESS_THREADS_PER_WORKER", "PAPERRUST_TASK_WORKERS", "PAPERRUST_THREADS_PER_WORKER"] {
            assert!(!e.contains_key(k), "{k} must be left to the server's automatic cap");
        }
        assert_eq!(e["PAPERLESS_URL"], "http://127.0.0.1:58080");
        assert_eq!(e["PAPERLESS_DBPASS"], cfg.database.app_password);
        assert_eq!(e["PAPERLESS_MEDIA_ROOT"], paths.media.display().to_string());
        assert_eq!(e["PAPERLESS_SCRATCH_DIR"], paths.scratch.display().to_string());
        assert_eq!(e["NO_COLOR"], "1");
        assert_eq!(e["PAPERRUST_DB_REQUIRE_AUTH"], "scram-sha-256");
        assert!(!e.keys().any(|k| k.starts_with("PAPERLESS_RS_")), "nur noch PAPERRUST_*");
        assert!(e.contains_key("PAPERRUST_PDFIUM_PATH"));
        assert!(e.contains_key("PAPERRUST_ROUTES"));
        assert_eq!(
            e["PAPERLESS_THUMBNAIL_FONT_NAME"],
            Path::new("/opt/pr").join("fonts").join("LiberationSerif-Regular.ttf").display().to_string()
        );
        assert!(e.keys().all(|k| k.starts_with("PAPERLESS_") || k.starts_with("PAPERRUST_") || k == "NO_COLOR"));
        assert!(is_secret("PAPERLESS_DBPASS") && is_secret("PAPERLESS_SECRET_KEY") && !is_secret("PAPERLESS_DBHOST"));
    }

    #[test]
    fn local_only_by_default_lan_sharing_opt_in() {
        let (mut cfg, paths, layout) = fixture();
        let e = server_env(&cfg, &paths, &layout, 54_417, 58_080);
        assert_eq!(e["PAPERRUST_BIND"], "127.0.0.1:58080");
        assert_eq!(e["PAPERRUST_SHARE_MODE"], "local");
        assert_eq!(e["PAPERLESS_ALLOWED_HOSTS"], "127.0.0.1,localhost");
        assert_eq!(e["PAPERLESS_LOGGING_DIR"], paths.logs.display().to_string());
        // lan_address without lan_sharing: no effect
        cfg.server.lan_address = "192.168.1.20".into();
        assert_eq!(server_env(&cfg, &paths, &layout, 1, 58_080)["PAPERRUST_BIND"], "127.0.0.1:58080");
        cfg.server.lan_sharing = true;
        let e = server_env(&cfg, &paths, &layout, 54_417, 58_080);
        assert_eq!(e["PAPERRUST_BIND"], "0.0.0.0:58080");
        assert_eq!(e["PAPERLESS_URL"], "http://192.168.1.20:58080");
        assert_eq!(e["PAPERLESS_ALLOWED_HOSTS"], "127.0.0.1,localhost,192.168.1.20");
        assert_eq!(e["PAPERRUST_SHARE_MODE"], "lan");
        // database stays local
        assert_eq!(e["PAPERLESS_DBHOST"], "127.0.0.1");
        // automatic address: a real IPv4 or (without network) back to local
        cfg.server.lan_address.clear();
        let e = server_env(&cfg, &paths, &layout, 54_417, 58_080);
        match detect_lan_ipv4() {
            Some(ip) => assert_eq!(e["PAPERLESS_URL"], format!("http://{ip}:58080")),
            None => assert_eq!(e["PAPERRUST_BIND"], "127.0.0.1:58080"),
        }
    }

    #[test]
    fn optional_parts_absent() {
        let (cfg, paths, _) = fixture();
        let layout = InstallLayout::resolve(Path::new("/opt/pr"), |_| None, |_| false);
        let e = server_env(&cfg, &paths, &layout, 1000, 1001);
        assert!(!e.contains_key("PAPERRUST_TESSERACT_PATH"));
        assert!(!e.contains_key("PAPERLESS_STATICDIR"));
        // without bundled font: server default (upstream path), no empty value
        assert!(!e.contains_key("PAPERLESS_THUMBNAIL_FONT_NAME"));
    }

    #[test]
    fn status_line() {
        assert!(is_ok_status_line(b"HTTP/1.1 200 OK\r\n"));
        assert!(!is_ok_status_line(b"HTTP/1.1 502 Bad Gateway\r\n"));
        assert!(!is_ok_status_line(b""));
    }

    #[test]
    fn debug_hides_secrets() {
        let (cfg, paths, layout) = fixture();
        let spec = ServerSpec {
            program: PathBuf::from("paperrust"),
            env: server_env(&cfg, &paths, &layout, 1, 2),
            cwd: PathBuf::from("."),
            log_dir: PathBuf::from("."),
        };
        let d = format!("{spec:?}");
        assert!(!d.contains(&cfg.database.app_password), "{d}");
        assert!(!d.contains(&cfg.server.secret_key), "{d}");
        assert!(d.contains("PAPERLESS_DBPASS") && d.contains("***"));
        // DB access of the server is the application role, not the cluster superuser (D-N3)
        assert_eq!(spec.env["PAPERLESS_DBUSER"], cfg.database.app_user);
        assert_ne!(spec.env["PAPERLESS_DBUSER"], cfg.database.user);
    }

    #[test]
    fn health_must_be_ours() {
        let h = |body: &str| Health { ok: true, body: body.into() };
        let today = r#"{"status":"ok","service":"paperrust","version":"0.1.0","instance":"abc","rust_routes":5}"#;
        assert!(h(today).is_ours("abc"));
        assert!(h(r#"{"service": "paperrust", "instance": "abc"}"#).is_ours("abc"));
        assert!(!h(r#"{"service":"paperrust","instance":"xyz"}"#).is_ours("abc"));
        assert!(!h(r#"{"service":"paperrust","instance":"abcd"}"#).is_ours("abc"));
        // without instance ID (older build or foreign service) no longer accepted
        assert!(!h(r#"{"status":"ok","service":"paperrust","version":"0.1.0"}"#).is_ours("abc"));
        assert!(!h(r#"{"status":"ok"}"#).is_ours("abc"));
        assert!(!h(r#"{"status":"ok","rust_routes":397}"#).is_ours("abc"));
        assert!(!h(r#"{"service":"paperrust","instance":""}"#).is_ours(""));
        assert!(!h("<html>fremder Dienst</html>").is_ours("abc"));
    }

    #[test]
    fn netstat_listener() {
        let out = "\r\nAktive Verbindungen\r\n\r\n  Proto  Lokale Adresse  Remoteadresse  Status  PID\r\n  TCP    0.0.0.0:135   0.0.0.0:0   ABHÖREN   1648\r\n  TCP    127.0.0.1:58080   0.0.0.0:0   ABHÖREN   4242\r\n  TCP    127.0.0.1:58080   127.0.0.1:50000   HERGESTELLT   4242\r\n  TCP    127.0.0.1:5808   0.0.0.0:0   LISTENING   1\r\n";
        assert_eq!(parse_netstat_listener(out, 58_080), Some(4242));
        assert_eq!(parse_netstat_listener(out, 5808), Some(1));
        assert_eq!(parse_netstat_listener(out, 9), None);
    }

    /// A foreign service on the port is not accepted as healthy (D-N5).
    #[tokio::test]
    async fn foreign_listener_is_not_healthy() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut b = [0u8; 256];
                let _ = s.read(&mut b).await;
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n{\"status\":\"ok\"}").await;
            }
        });
        let h = health_probe(port).await.unwrap();
        assert!(h.ok && !h.is_ours("x"));
        // Child process that never listens itself: wait_healthy must reject the foreign service
        let child = if cfg!(windows) {
            Command::new("cmd").args(["/C", "ping -n 30 127.0.0.1 >NUL"]).stdin(Stdio::piped()).kill_on_drop(true).spawn()
        } else {
            Command::new("sleep").arg("30").stdin(Stdio::piped()).kill_on_drop(true).spawn()
        };
        let mut child = child.unwrap();
        let stdin = child.stdin.take();
        let mut proc = ServerProcess { child, stdin, instance_id: "x".into() };
        let err = proc.wait_healthy(port, Duration::from_secs(5)).await.unwrap_err();
        assert!(matches!(err, DesktopError::PortTaken(_)), "{err}");
        let _ = proc.stop(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn health_against_fake_server() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = l.accept().await {
                let mut b = [0u8; 256];
                let _ = s.read(&mut b).await;
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}").await;
            }
        });
        assert!(health(port).await);
        let free = crate::ports::pick_port(None, &[]).unwrap();
        assert!(!health(free).await);
    }
}
