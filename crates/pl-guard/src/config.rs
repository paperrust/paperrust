//! Settings `PAPERRUST_GUARD_*` and `PAPERRUST_BACKUP_*` (registered in pl-settings, docs/SETTINGS.md).
//! Read via a lookup function (server: `pl_core::config::var`, tests: fixed values).

use std::path::PathBuf;

/// All keys of the guard (a change in the UI restarts it).
pub const GUARD_KEYS: &[&str] = &[
    "PAPERRUST_GUARD",
    "PAPERRUST_GUARD_THRESHOLD",
    "PAPERRUST_GUARD_WINDOW_MINUTES",
    "PAPERRUST_GUARD_SCAN_INTERVAL",
    "PAPERRUST_GUARD_SAMPLE_FILES",
    "PAPERRUST_GUARD_WATCH",
    "PAPERRUST_GUARD_EXTRA_EXTENSIONS",
    "PAPERRUST_GUARD_MAIL",
];

fn on_off(v: Option<String>, default: bool) -> bool {
    match v.as_deref().map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("on") => true,
        Some("off") => false,
        Some(s) if !s.is_empty() => pl_core::config::str_to_bool(s).unwrap_or(default),
        _ => default,
    }
}

fn int(v: Option<String>, default: i64, min: i64) -> i64 {
    match v.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => match s.parse::<i64>() {
            Ok(n) if n >= min => n,
            _ => {
                tracing::warn!(target: "paperless.guard", value = s, default, "invalid number in guard/backup settings, using default");
                default
            }
        },
        _ => default,
    }
}

fn list(v: Option<String>) -> Vec<String> {
    v.unwrap_or_default().split(',').map(|s| s.trim().trim_start_matches('.').to_string()).filter(|s| !s.is_empty()).collect()
}

/// Guard settings.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardConfig {
    pub enabled: bool,
    /// This many suspicious files …
    pub threshold: i64,
    /// … within this many minutes switch to "read-only".
    pub window_minutes: i64,
    /// Interval of the sampling (seconds, 0 = no folder review).
    pub scan_interval_secs: u64,
    /// Fully hashed documents per sampling run (rotating).
    pub sample_files: i64,
    pub watch: bool,
    pub extra_extensions: Vec<String>,
    /// Recipient of the alarm mail (empty = all active superusers with an email address).
    pub mail_to: Vec<String>,
    /// Waiting time before a change is evaluated (let ongoing PaperRust operations finish).
    pub grace_secs: u64,
}

impl GuardConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            enabled: on_off(get("PAPERRUST_GUARD"), true),
            threshold: int(get("PAPERRUST_GUARD_THRESHOLD"), 20, 1),
            window_minutes: int(get("PAPERRUST_GUARD_WINDOW_MINUTES"), 10, 1),
            scan_interval_secs: int(get("PAPERRUST_GUARD_SCAN_INTERVAL"), 900, 0) as u64,
            sample_files: int(get("PAPERRUST_GUARD_SAMPLE_FILES"), 20, 0),
            watch: on_off(get("PAPERRUST_GUARD_WATCH"), true),
            extra_extensions: list(get("PAPERRUST_GUARD_EXTRA_EXTENSIONS")),
            mail_to: list(get("PAPERRUST_GUARD_MAIL")),
            grace_secs: 60,
        }
    }

    pub fn from_env() -> Self {
        Self::from_lookup(pl_core::config::var)
    }
}

/// Object Lock mode of S3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Off,
    Governance,
    Compliance,
}

impl LockMode {
    pub fn header(self) -> Option<&'static str> {
        match self {
            LockMode::Off => None,
            LockMode::Governance => Some("GOVERNANCE"),
            LockMode::Compliance => Some("COMPLIANCE"),
        }
    }
}

/// Access to S3-compatible storage.
#[derive(Clone, PartialEq)]
pub struct S3Config {
    /// `http(s)://host:port` (without bucket; path-style addressing).
    pub endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
    /// Prefix without leading/trailing `/` (empty = root).
    pub prefix: String,
    pub lock: LockMode,
    pub lock_days: i64,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("lock", &self.lock)
            .field("lock_days", &self.lock_days)
            .finish_non_exhaustive()
    }
}

/// Where backups go.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Local(PathBuf),
    S3(S3Config),
}

impl Target {
    /// Display without secrets (for log, run list, UI).
    pub fn display(&self) -> String {
        match self {
            Target::Local(p) => p.display().to_string(),
            Target::S3(s) => {
                if s.prefix.is_empty() {
                    format!("s3://{}", s.bucket)
                } else {
                    format!("s3://{}/{}", s.bucket, s.prefix)
                }
            }
        }
    }
}

/// Backup settings.
#[derive(Debug, Clone, PartialEq)]
pub struct BackupConfig {
    pub target: Option<Target>,
    pub database: bool,
    /// always keep this many newest states (internal, default 1; tests set more)
    pub keep_last: usize,
    pub keep_daily: usize,
    pub keep_weekly: usize,
    pub keep_monthly: usize,
}

impl BackupConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let raw = get("PAPERRUST_BACKUP_TARGET").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let target = match raw {
            None => None,
            Some(t) if t.starts_with("s3://") => {
                let rest = &t["s3://".len()..];
                let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
                if bucket.is_empty() {
                    return Err("PAPERRUST_BACKUP_TARGET: s3:// needs a bucket name".into());
                }
                let endpoint = get("PAPERRUST_BACKUP_S3_ENDPOINT").map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty());
                let Some(endpoint) = endpoint else {
                    return Err("PAPERRUST_BACKUP_S3_ENDPOINT is required for an s3:// target".into());
                };
                if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
                    return Err("PAPERRUST_BACKUP_S3_ENDPOINT must start with http:// or https://".into());
                }
                let lock = match get("PAPERRUST_BACKUP_S3_OBJECT_LOCK").unwrap_or_default().trim().to_ascii_lowercase().as_str() {
                    "" | "off" | "no" | "false" => LockMode::Off,
                    "governance" => LockMode::Governance,
                    "compliance" => LockMode::Compliance,
                    other => return Err(format!("PAPERRUST_BACKUP_S3_OBJECT_LOCK: unknown mode {other:?} (off, governance, compliance)")),
                };
                Some(Target::S3(S3Config {
                    endpoint,
                    region: get("PAPERRUST_BACKUP_S3_REGION").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "us-east-1".into()),
                    access_key: get("PAPERRUST_BACKUP_S3_ACCESS_KEY").unwrap_or_default().trim().to_string(),
                    secret_key: get("PAPERRUST_BACKUP_S3_SECRET_KEY").unwrap_or_default().trim().to_string(),
                    bucket: bucket.to_string(),
                    prefix: prefix.trim_matches('/').to_string(),
                    lock,
                    lock_days: int(get("PAPERRUST_BACKUP_S3_LOCK_DAYS"), 30, 1),
                }))
            }
            Some(t) => Some(Target::Local(PathBuf::from(t.strip_prefix("file://").unwrap_or(&t)))),
        };
        Ok(Self {
            target,
            database: on_off(get("PAPERRUST_BACKUP_DATABASE"), true),
            keep_last: 1,
            keep_daily: int(get("PAPERRUST_BACKUP_KEEP_DAILY"), 7, 0) as usize,
            keep_weekly: int(get("PAPERRUST_BACKUP_KEEP_WEEKLY"), 4, 0) as usize,
            keep_monthly: int(get("PAPERRUST_BACKUP_KEEP_MONTHLY"), 12, 0) as usize,
        })
    }

    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(pl_core::config::var)
    }
}

/// Passphrase for `backup init`/`restore` from the environment (never in responses or logs).
pub fn passphrase_from_env() -> Option<String> {
    pl_core::config::var("PAPERRUST_BACKUP_PASSPHRASE").filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(a, _)| *a == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn guard_defaults_and_values() {
        let d = GuardConfig::from_lookup(|_| None);
        assert!(d.enabled && d.watch);
        assert_eq!((d.threshold, d.window_minutes, d.scan_interval_secs, d.sample_files), (20, 10, 900, 20));
        let c = GuardConfig::from_lookup(look(&[
            ("PAPERRUST_GUARD", "off"),
            ("PAPERRUST_GUARD_THRESHOLD", "5"),
            ("PAPERRUST_GUARD_WINDOW_MINUTES", "0"),
            ("PAPERRUST_GUARD_EXTRA_EXTENSIONS", ".foo, bar"),
        ]));
        assert!(!c.enabled);
        assert_eq!(c.threshold, 5);
        assert_eq!(c.window_minutes, 10, "ungültig → Standard");
        assert_eq!(c.extra_extensions, vec!["foo", "bar"]);
    }

    #[test]
    fn backup_targets() {
        let c = BackupConfig::from_lookup(|_| None).unwrap();
        assert_eq!(c.target, None);
        assert_eq!((c.keep_daily, c.keep_weekly, c.keep_monthly), (7, 4, 12));
        let c = BackupConfig::from_lookup(look(&[("PAPERRUST_BACKUP_TARGET", "/volume2/backup/paperrust")])).unwrap();
        assert_eq!(c.target, Some(Target::Local("/volume2/backup/paperrust".into())));
        assert!(BackupConfig::from_lookup(look(&[("PAPERRUST_BACKUP_TARGET", "s3://b")])).is_err(), "Endpunkt fehlt");
        let c = BackupConfig::from_lookup(look(&[
            ("PAPERRUST_BACKUP_TARGET", "s3://bucket/pre/fix/"),
            ("PAPERRUST_BACKUP_S3_ENDPOINT", "https://s3.example.test/"),
            ("PAPERRUST_BACKUP_S3_SECRET_KEY", "geheim"),
            ("PAPERRUST_BACKUP_S3_OBJECT_LOCK", "Compliance"),
        ]))
        .unwrap();
        let Some(Target::S3(s)) = &c.target else { panic!() };
        assert_eq!((s.bucket.as_str(), s.prefix.as_str(), s.endpoint.as_str()), ("bucket", "pre/fix", "https://s3.example.test"));
        assert_eq!(s.lock, LockMode::Compliance);
        assert_eq!(s.lock_days, 30);
        assert!(!format!("{s:?}").contains("geheim"), "Debug ohne Geheimnis");
        assert_eq!(c.target.as_ref().unwrap().display(), "s3://bucket/pre/fix");
    }
}
