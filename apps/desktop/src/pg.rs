//! Embedded PostgreSQL via the bundled tools (`initdb`, `pg_ctl`, `pg_isready`,
//! `createdb`, `pg_dump`). No library, no download at runtime (docs/DESKTOP.md).
//!
//! - `initdb` once: superuser = `database.user`, password via `--pwfile` (file readable only by the
//!   user, deleted again immediately), `scram-sha-256` for all connections, UTF-8.
//! - `postgresql.conf` gets `include_if_exists = 'paperrust.conf'` once; the supervisor rewrites this file
//!   on **every** start (port, only `127.0.0.1`, no Unix socket, small
//!   memory values for laptops).
//! - Start: `pg_ctl start -w`; a cluster that is still running (supervisor crashed) is first stopped with
//!   `pg_ctl stop -m fast`, so that port/settings are certainly correct.
//! - Stop: `pg_ctl stop -m fast -w` (aborts sessions, checkpoint, clean shutdown);
//!   only if that fails `-m immediate` (recovery at the next start, no loss of
//!   committed transactions).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};

use tokio::process::Command;

use crate::layout::EXE;
use crate::{DesktopError, Result, io_err};

/// Tools that must be in the package.
pub const REQUIRED_TOOLS: &[&str] = &["initdb", "pg_ctl", "postgres", "pg_isready", "createdb", "psql", "pg_dump"];

/// Password file for `initdb` in `run/`; if it stays behind after a crash, the
/// supervisor deletes it the next time it opens (D-N2).
pub const INITDB_PWFILE: &str = "initdb.pw";

/// SQL for the application role (D-N3), runs on **every** start as cluster superuser (idempotent):
/// create the role without superuser/CREATEDB/CREATEROLE rights or align its password, hand over the database
/// and schema `public` to it, and re-own all tables/sequences/views/functions in `public`
/// that still belong to someone else (database from before D-N3, `pg_restore` as superuser).
/// Identifiers are restricted to `[A-Za-z0-9_]` by `DesktopConfig::validate`, the password to `[A-Za-z0-9]`;
/// the script goes to `psql` via stdin (password never on a command line).
pub fn app_role_sql(db: &str, role: &str, password: &str) -> String {
    format!(
        r#"\set ON_ERROR_STOP on
DO $$BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}') THEN
    CREATE ROLE "{role}" LOGIN;
  END IF;
END$$;
ALTER ROLE "{role}" WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD '{password}';
ALTER DATABASE "{db}" OWNER TO "{role}";
ALTER SCHEMA public OWNER TO "{role}";
DO $$DECLARE r record; BEGIN
  FOR r IN
    SELECT c.oid::regclass AS obj, c.relkind FROM pg_class c
      JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE n.nspname = 'public' AND c.relkind IN ('r','p','v','m','S','f')
       AND pg_get_userbyid(c.relowner) <> '{role}'
       AND NOT (c.relkind = 'S' AND EXISTS (
             SELECT 1 FROM pg_depend d WHERE d.objid = c.oid AND d.deptype IN ('a','i')))
  LOOP
    EXECUTE format('ALTER %s %s OWNER TO %I',
      CASE r.relkind WHEN 'S' THEN 'SEQUENCE' WHEN 'v' THEN 'VIEW'
                     WHEN 'm' THEN 'MATERIALIZED VIEW' WHEN 'f' THEN 'FOREIGN TABLE'
                     ELSE 'TABLE' END, r.obj, '{role}');
  END LOOP;
  FOR r IN
    SELECT p.oid::regprocedure AS obj FROM pg_proc p
      JOIN pg_namespace n ON n.oid = p.pronamespace
     WHERE n.nspname = 'public' AND pg_get_userbyid(p.proowner) <> '{role}'
  LOOP
    EXECUTE format('ALTER ROUTINE %s OWNER TO %I', r.obj, '{role}');
  END LOOP;
END$$;
"#
    )
}

/// Major version the package is built for (as with the user: 17.x).
pub const SUPPORTED_MAJOR: u32 = 17;

pub const INCLUDE_LINE: &str = "include_if_exists = 'paperrust.conf'";

/// `pg_ctl status` result (exit codes according to the pg_ctl docs: 0 running, 3 not running, 4 no data directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgStatus {
    Running,
    Stopped,
    NoDataDir,
}

/// `pg_ctl (PostgreSQL) 17.4` / `initdb (PostgreSQL) 18beta1` → major version.
pub fn parse_major(version_output: &str) -> Option<u32> {
    let v = version_output.split_whitespace().last()?;
    let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Major version of a data directory (`PG_VERSION`).
pub fn data_major(pgdata: &Path) -> Option<u32> {
    std::fs::read_to_string(pgdata.join("PG_VERSION")).ok()?.trim().parse().ok()
}

/// Data directory and tools must have the same major version.
pub fn check_compat(tools_major: u32, data_major: Option<u32>) -> Result<()> {
    match data_major {
        None => Ok(()),
        Some(d) if d == tools_major => Ok(()),
        Some(d) if d < tools_major => Err(DesktopError::Postgres(format!(
            "Datenbank ist von PostgreSQL {d}, mitgeliefert ist {tools_major}: Upgrade nötig (pg_upgrade bzw. Sicherung + Wiederherstellung, siehe docs/DESKTOP.md „Updates“)"
        ))),
        Some(d) => Err(DesktopError::Postgres(format!(
            "Datenbank ist von PostgreSQL {d}, mitgeliefert ist nur {tools_major}: bitte eine neuere PaperRust-Version installieren"
        ))),
    }
}

/// Arguments for `initdb`.
pub fn initdb_args(pgdata: &Path, user: &str, pwfile: &Path, locale_provider: &str) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec![
        "-D".into(),
        pgdata.into(),
        "-U".into(),
        user.into(),
        {
            let mut s = OsString::from("--pwfile=");
            s.push(pwfile);
            s
        },
        "--auth=scram-sha-256".into(),
        "--encoding=UTF8".into(),
    ];
    match locale_provider {
        // ICU: language-correct sorting (roughly similar to Docker Postgres with en_US.utf8).
        "icu" => a.extend(["--locale-provider=icu".into(), "--icu-locale=und".into(), "--locale=C".into()]),
        // PG ≥ 17: platform independent, stable sorting (code points), no OS locale needed.
        _ => a.extend(["--locale-provider=builtin".into(), "--builtin-locale=C.UTF-8".into()]),
    }
    a
}

/// Content of `paperrust.conf` (new on every start).
pub fn runtime_conf(port: u16) -> String {
    format!(
        "# Von PaperRust Desktop bei jedem Start geschrieben – Änderungen gehen verloren.\n\
         # Eigene Einstellungen in postgresql.conf vor der include-Zeile oder per ALTER SYSTEM.\n\
         listen_addresses = '127.0.0.1'\n\
         port = {port}\n\
         unix_socket_directories = ''\n\
         max_connections = 40\n\
         shared_buffers = '64MB'\n\
         work_mem = '8MB'\n\
         maintenance_work_mem = '64MB'\n\
         logging_collector = off\n\
         log_min_messages = warning\n\
         log_line_prefix = '%m [%p] '\n\
         log_timezone = 'UTC'\n\
         # JIT aus: bringt für PaperRust-Abfragen nichts und erlaubt, LLVM aus dem Paket zu lassen.\n\
         jit = off\n"
    )
}

/// Appends the include line to `postgresql.conf` once (idempotent).
pub fn ensure_include(postgresql_conf: &str) -> Option<String> {
    if postgresql_conf.lines().any(|l| l.trim() == INCLUDE_LINE) {
        None
    } else {
        let mut s = postgresql_conf.to_string();
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str("\n# PaperRust Desktop\n");
        s.push_str(INCLUDE_LINE);
        s.push('\n');
        Some(s)
    }
}

/// A cluster under `pgdata`.
#[derive(Clone)]
pub struct PgCluster {
    pub bin: PathBuf,
    pub pgdata: PathBuf,
    pub log_dir: PathBuf,
    pub run_dir: PathBuf,
    pub port: u16,
    pub user: String,
    password: String,
    pub locale_provider: String,
}

impl std::fmt::Debug for PgCluster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgCluster")
            .field("pgdata", &self.pgdata)
            .field("port", &self.port)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// Windows: no console window, own process group (Ctrl+C in the supervisor's console
/// does not reach the children; the supervisor decides the order on shutdown).
pub(crate) fn detach_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        // Own process group: SIGINT from the terminal only hits the supervisor.
        cmd.process_group(0);
    }
}

/// Removes inherited libpq/Paperless variables (`PGDATA`, `PGHOST`, `PAPERLESS_DBHOST` …), so that
/// a user's environment never points to a different database.
pub(crate) fn scrub_env(cmd: &mut Command) {
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("PG") || k.starts_with("PAPERLESS_") || k.starts_with("PAPERRUST_") || k.starts_with("DJANGO_SUPERUSER_") {
            cmd.env_remove(k.as_ref());
        }
    }
}

async fn output_with_timeout(mut cmd: Command, what: &str, timeout: Duration) -> Result<Output> {
    cmd.kill_on_drop(true);
    let fut = cmd.output();
    match tokio::time::timeout(timeout, fut).await {
        Ok(r) => r.map_err(io_err(format!("{what} starten"))),
        Err(_) => Err(DesktopError::Postgres(format!("{what}: keine Antwort nach {} s", timeout.as_secs()))),
    }
}

fn fail(what: &str, out: &Output) -> DesktopError {
    let err = String::from_utf8_lossy(&out.stderr);
    let msg = err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim();
    DesktopError::Postgres(format!("{what} fehlgeschlagen ({}): {msg}", out.status))
}

impl PgCluster {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bin: PathBuf,
        pgdata: PathBuf,
        log_dir: PathBuf,
        run_dir: PathBuf,
        port: u16,
        user: String,
        password: String,
        locale_provider: String,
    ) -> Self {
        Self { bin, pgdata, log_dir, run_dir, port, user, password, locale_provider }
    }

    pub fn tool(&self, name: &str) -> PathBuf {
        self.bin.join(format!("{name}{EXE}"))
    }

    fn cmd(&self, name: &str) -> Command {
        let mut c = Command::new(self.tool(name));
        scrub_env(&mut c);
        detach_console(&mut c);
        c.stdin(Stdio::null());
        c
    }

    /// Command that connects to the cluster (password only via the environment, never as an argument).
    fn client(&self, name: &str) -> Command {
        let mut c = self.cmd(name);
        c.env("PGHOST", "127.0.0.1")
            .env("PGPORT", self.port.to_string())
            .env("PGUSER", &self.user)
            .env("PGPASSWORD", &self.password)
            .env("PGCONNECT_TIMEOUT", "5")
            // D-N3: accept only SCRAM – a foreign service on the port (TOCTOU) gets no
            // plaintext/MD5 password (libpq ≥ 16).
            .env("PGREQUIREAUTH", "scram-sha-256");
        c
    }

    pub async fn tools_major(&self) -> Result<u32> {
        let mut c = self.cmd("pg_ctl");
        c.arg("--version");
        let out = output_with_timeout(c, "pg_ctl --version", Duration::from_secs(20)).await?;
        let text = String::from_utf8_lossy(&out.stdout);
        parse_major(&text).ok_or_else(|| DesktopError::Postgres(format!("Version nicht lesbar: {}", text.trim())))
    }

    pub fn is_initialized(&self) -> bool {
        self.pgdata.join("PG_VERSION").is_file()
    }

    /// `initdb` (only if no cluster exists yet).
    pub async fn initdb(&self) -> Result<()> {
        if self.is_initialized() {
            return Ok(());
        }
        if self.pgdata.exists() && std::fs::read_dir(&self.pgdata).map(|mut d| d.next().is_some()).unwrap_or(true) {
            return Err(DesktopError::Postgres(format!(
                "{} ist nicht leer, enthält aber keinen Cluster (abgebrochener Erst-Start?). Verzeichnis prüfen und ggf. löschen.",
                self.pgdata.display()
            )));
        }
        let pwfile = self.run_dir.join(INITDB_PWFILE);
        crate::config::write_private(&pwfile, self.password.as_bytes()).map_err(io_err("Passwortdatei für initdb schreiben"))?;
        let mut c = self.cmd("initdb");
        c.args(initdb_args(&self.pgdata, &self.user, &pwfile, &self.locale_provider));
        let res = output_with_timeout(c, "initdb", Duration::from_secs(300)).await;
        let _ = std::fs::remove_file(&pwfile);
        let out = res?;
        if !out.status.success() {
            return Err(fail("initdb", &out));
        }
        let conf_path = self.pgdata.join("postgresql.conf");
        let conf = std::fs::read_to_string(&conf_path).map_err(io_err("postgresql.conf lesen"))?;
        if let Some(new) = ensure_include(&conf) {
            std::fs::write(&conf_path, new).map_err(io_err("postgresql.conf schreiben"))?;
        }
        Ok(())
    }

    pub fn write_runtime_conf(&self) -> Result<()> {
        let p = self.pgdata.join("paperrust.conf");
        std::fs::write(&p, runtime_conf(self.port)).map_err(io_err(format!("{} schreiben", p.display())))
    }

    pub async fn status(&self) -> Result<PgStatus> {
        let mut c = self.cmd("pg_ctl");
        c.arg("status").arg("-D").arg(&self.pgdata);
        let out = output_with_timeout(c, "pg_ctl status", Duration::from_secs(20)).await?;
        Ok(match out.status.code() {
            Some(0) => PgStatus::Running,
            Some(4) => PgStatus::NoDataDir,
            _ => PgStatus::Stopped,
        })
    }

    /// Starts the cluster and waits until it accepts connections.
    pub async fn start(&self) -> Result<()> {
        check_compat(self.tools_major().await?, data_major(&self.pgdata))?;
        if self.status().await? == PgStatus::Running {
            tracing::warn!("PostgreSQL lief noch (vorheriger Absturz?) – wird neu gestartet");
            self.stop().await?;
        }
        self.write_runtime_conf()?;
        // Size limit: PostgreSQL keeps its log open, so rotate only before the start (logs.rs).
        for f in ["postgresql.log", "pg_ctl.log"] {
            let _ = crate::logs::rotate_if_larger(&self.log_dir.join(f), crate::logs::max_bytes(), crate::logs::KEEP);
        }
        let ctl_log = crate::logs::open_append(&self.log_dir.join("pg_ctl.log")).map_err(io_err("pg_ctl.log öffnen"))?;
        let mut c = self.cmd("pg_ctl");
        c.arg("start")
            .arg("-D")
            .arg(&self.pgdata)
            .arg("-l")
            .arg(self.log_dir.join("postgresql.log"))
            .args(["-w", "-t", "120", "-s"])
            // No pipes: the detached postgres process would inherit them and `output()` would wait forever.
            .stdout(Stdio::null())
            .stderr(Stdio::from(ctl_log));
        let status = tokio::time::timeout(Duration::from_secs(150), async { c.status().await })
            .await
            .map_err(|_| DesktopError::Postgres("pg_ctl start: keine Antwort nach 150 s".into()))?
            .map_err(io_err("pg_ctl start"))?;
        if !status.success() {
            return Err(DesktopError::Postgres(format!(
                "Start fehlgeschlagen ({status}); Details in {}",
                self.log_dir.join("postgresql.log").display()
            )));
        }
        self.wait_ready(Duration::from_secs(60)).await
    }

    /// Health check: `pg_isready` until success or timeout.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        loop {
            if self.is_ready().await {
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(DesktopError::Postgres(format!("nimmt nach {} s keine Verbindungen an", timeout.as_secs())));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    pub async fn is_ready(&self) -> bool {
        let mut c = self.client("pg_isready");
        c.args(["-q", "-t", "3", "-d", "postgres"]);
        matches!(output_with_timeout(c, "pg_isready", Duration::from_secs(10)).await, Ok(o) if o.status.success())
    }

    /// Shut down cleanly (fast), if necessary immediate. If it is not running, that is not an error.
    pub async fn stop(&self) -> Result<()> {
        if self.status().await? != PgStatus::Running {
            return Ok(());
        }
        for mode in ["fast", "immediate"] {
            let mut c = self.cmd("pg_ctl");
            c.arg("stop").arg("-D").arg(&self.pgdata).args(["-m", mode, "-w", "-t", "60", "-s"]);
            match output_with_timeout(c, "pg_ctl stop", Duration::from_secs(80)).await {
                Ok(o) if o.status.success() => {
                    tracing::info!(mode, "PostgreSQL gestoppt");
                    return Ok(());
                }
                Ok(o) => {
                    tracing::warn!(mode, error = %fail("pg_ctl stop", &o), "Stopp fehlgeschlagen")
                }
                Err(e) => tracing::warn!(mode, error = %e, "Stopp fehlgeschlagen"),
            }
        }
        Err(DesktopError::Postgres("ließ sich nicht stoppen".into()))
    }

    /// `createdb` (idempotent: "already exists" counts as success).
    pub async fn ensure_database(&self, name: &str) -> Result<()> {
        let mut c = self.client("createdb");
        c.args(["-E", "UTF8", "-T", "template0", "--maintenance-db=postgres", name]);
        let out = output_with_timeout(c, "createdb", Duration::from_secs(60)).await?;
        if out.status.success() || String::from_utf8_lossy(&out.stderr).contains("already exists") {
            Ok(())
        } else {
            Err(fail("createdb", &out))
        }
    }

    /// A value via `psql -tAc` (only fixed, internally formulated queries – no user input).
    pub async fn query_scalar(&self, db: &str, sql: &str) -> Result<String> {
        let mut c = self.client("psql");
        c.args(["-X", "-q", "-t", "-A", "-v", "ON_ERROR_STOP=1", "-d", db, "-c", sql]);
        let out = output_with_timeout(c, "psql", Duration::from_secs(60)).await?;
        if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()) } else { Err(fail("psql", &out)) }
    }

    /// SQL script via stdin to `psql` (for scripts with secrets: none of it on the command line).
    pub async fn run_script(&self, db: &str, sql: &str) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut c = self.client("psql");
        c.args(["-X", "-q", "-v", "ON_ERROR_STOP=1", "-d", db, "-f", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = c.spawn().map_err(io_err("psql starten"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(sql.as_bytes()).await.map_err(io_err("psql: Skript übergeben"))?;
            drop(stdin);
        }
        let out = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output())
            .await
            .map_err(|_| DesktopError::Postgres("psql: keine Antwort nach 120 s".into()))?
            .map_err(io_err("psql"))?;
        if out.status.success() { Ok(()) } else { Err(fail("psql", &out)) }
    }

    /// Create/align the application role and hand over ownership ([`app_role_sql`], D-N3).
    pub async fn ensure_app_role(&self, db: &str, role: &str, password: &str) -> Result<()> {
        if role == self.user {
            return Err(DesktopError::Postgres("Anwendungsrolle darf nicht der Cluster-Superuser sein".into()));
        }
        self.run_script(db, &app_role_sql(db, role, password)).await
    }

    /// `pg_dump -Fc` of the database `name` to `out_file`.
    pub async fn dump(&self, name: &str, out_file: &Path) -> Result<()> {
        let mut c = self.client("pg_dump");
        c.args(["-Fc", "--no-owner", "-d", name, "-f"]).arg(out_file);
        let out = output_with_timeout(c, "pg_dump", Duration::from_secs(3600)).await?;
        if out.status.success() { Ok(()) } else { Err(fail("pg_dump", &out)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_major("pg_ctl (PostgreSQL) 17.4\n"), Some(17));
        assert_eq!(parse_major("initdb (PostgreSQL) 18beta1"), Some(18));
        assert_eq!(parse_major("pg_ctl (PostgreSQL) 9.6.24"), Some(9));
        assert_eq!(parse_major(""), None);
        assert!(check_compat(17, None).is_ok());
        assert!(check_compat(17, Some(17)).is_ok());
        let up = check_compat(18, Some(17)).unwrap_err().to_string();
        assert!(up.contains("Upgrade"), "{up}");
        assert!(check_compat(17, Some(18)).is_err());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(data_major(dir.path()), None);
        std::fs::write(dir.path().join("PG_VERSION"), "17\n").unwrap();
        assert_eq!(data_major(dir.path()), Some(17));
    }

    #[test]
    fn initdb_never_gets_password_on_command_line() {
        let a = initdb_args(Path::new("/d"), "paperrust", Path::new("/r/initdb.pw"), "builtin");
        let s: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert!(s.contains(&"--auth=scram-sha-256".to_string()));
        assert!(s.contains(&"--builtin-locale=C.UTF-8".to_string()));
        assert!(s.iter().any(|x| x.starts_with("--pwfile=") && x.ends_with("initdb.pw")));
        let icu: Vec<String> =
            initdb_args(Path::new("/d"), "u", Path::new("/p"), "icu").iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert!(icu.contains(&"--locale-provider=icu".to_string()));
    }

    #[test]
    fn app_role_is_not_superuser() {
        let sql = app_role_sql("paperless", "paperless", "Abc123");
        assert!(sql.contains("NOSUPERUSER NOCREATEDB NOCREATEROLE"));
        assert!(sql.contains(r#"ALTER DATABASE "paperless" OWNER TO "paperless""#));
        assert!(sql.contains("ALTER SCHEMA public OWNER"));
        assert!(sql.starts_with("\\set ON_ERROR_STOP on"));
    }

    #[test]
    fn runtime_conf_is_loopback_only() {
        let c = runtime_conf(54_417);
        assert!(c.contains("listen_addresses = '127.0.0.1'"));
        assert!(c.contains("port = 54417"));
        assert!(c.contains("unix_socket_directories = ''"));
        assert!(!c.contains("0.0.0.0") && !c.contains("'*'"));
    }

    #[test]
    fn include_is_idempotent() {
        let once = ensure_include("max_connections = 100").unwrap();
        assert!(once.ends_with(&format!("{INCLUDE_LINE}\n")));
        assert_eq!(ensure_include(&once), None);
    }
}
