//! Queue configuration.
//!
//! upstream: src/paperless/settings/__init__.py (`CELERY_WORKER_CONCURRENCY =
//! get_int_from_env("PAPERLESS_TASK_WORKERS", 1)`, `CELERY_TASK_TIME_LIMIT =
//! get_int_from_env("PAPERLESS_WORKER_TIMEOUT", 1800)`, `THREADS_PER_WORKER`,
//! `default_threads_per_worker`, `CELERY_RESULT_EXPIRES = 3600`, `CELERY_TIMEZONE = TIME_ZONE`).

use chrono_tz::Tz;
use pl_core::config::{ConfigError, Env};

use crate::DEFAULT_QUEUE;

/// Settings (all from the environment, see the contract).
#[derive(Debug, Clone)]
pub struct QueueConfig {
    /// `PAPERLESS_TASK_WORKERS` (1): concurrently running tasks per worker process.
    pub concurrency: usize,
    /// `PAPERLESS_THREADS_PER_WORKER` (max(floor(cpus / workers), 1)): passed on to handlers
    /// (OCR threads); the queue itself does not use it.
    pub threads_per_worker: usize,
    /// `PAPERLESS_WORKER_TIMEOUT` (1800 s): hard time limit per task.
    pub time_limit_secs: f64,
    /// `PAPERRUST_QUEUE_NAMES` (`celery`): queues to process.
    pub queues: Vec<String>,
    /// `PAPERRUST_QUEUE_VISIBILITY_TIMEOUT` (60 s): lease duration; extended by heartbeat.
    /// A crashed worker releases its tasks at the latest after this.
    pub visibility_timeout_secs: f64,
    /// `PAPERRUST_QUEUE_MAX_DELIVERIES` (3): deliveries per attempt before a task ends as FAILURE
    /// (`WorkerLostError`) after repeated worker loss.
    pub max_deliveries: i32,
    /// `PAPERRUST_QUEUE_POLL_INTERVAL` (1 s): polling interval if no NOTIFY arrives.
    pub poll_interval_secs: f64,
    /// `PAPERRUST_QUEUE_SHUTDOWN_GRACE` (30 s): time to wait for running tasks on shutdown.
    pub shutdown_grace_secs: f64,
    /// `PAPERRUST_QUEUE_RESULT_EXPIRES` (3600 s = `CELERY_RESULT_EXPIRES`): finished rows
    /// are deleted after that (documents_paperlesstask stays untouched).
    pub result_expires_secs: f64,
    /// `PAPERLESS_TIME_ZONE` (= `CELERY_TIMEZONE`).
    pub tz: Tz,
    /// Worker name (`PAPERRUST_WORKER_NAME`, otherwise `paperrust@<host>:<pid>`).
    pub worker_name: String,
}

fn positive_f64(env: &Env, key: &str, default: f64) -> Result<f64, ConfigError> {
    let v = env.float(key)?.unwrap_or(default);
    if !(v.is_finite() && v > 0.0) {
        return Err(ConfigError::Invalid(format!("{key} must be a positive number")));
    }
    Ok(v)
}

/// `default_threads_per_worker(task_workers)`.
pub fn default_threads_per_worker(task_workers: i64) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()).max(1) as i64;
    if task_workers <= 0 {
        // Python: floor(cores / 0) → ZeroDivisionError (not caught) – here 1.
        return 1;
    }
    ((cores / task_workers).max(1)) as usize
}

impl QueueConfig {
    /// From the environment (`Env::from_process()` or test values).
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        // PAPERRUST_QUEUE / PAPERRUST_QUEUE_BACKEND have no effect (PostgreSQL only); an old
        // value such as `celery` leads to a notice at startup (pl_core::config::retired_warnings),
        // never to an abort.
        let workers = env.int_or("PAPERLESS_TASK_WORKERS", 1)?;
        if workers < 1 {
            return Err(ConfigError::Invalid("PAPERRUST_TASK_WORKERS must be >= 1".into()));
        }
        let threads = match env.int("PAPERLESS_THREADS_PER_WORKER")? {
            Some(t) => t.max(1) as usize,
            None => default_threads_per_worker(workers),
        };
        let timeout = env.int_or("PAPERLESS_WORKER_TIMEOUT", 1800)?;
        if timeout <= 0 {
            return Err(ConfigError::Invalid("PAPERRUST_WORKER_TIMEOUT must be > 0".into()));
        }
        let tz_name = env.get("PAPERLESS_TIME_ZONE").unwrap_or("UTC");
        let tz: Tz = tz_name.parse().map_err(|_| ConfigError::Invalid(format!("unknown time zone '{tz_name}'")))?;
        let max_deliveries = env.int_or("PAPERRUST_QUEUE_MAX_DELIVERIES", 3)?;
        if !(1..=1000).contains(&max_deliveries) {
            return Err(ConfigError::Invalid("PAPERRUST_QUEUE_MAX_DELIVERIES must be 1..=1000".into()));
        }
        let host = env.get("HOSTNAME").or_else(|| env.get("COMPUTERNAME")).unwrap_or("localhost");
        Ok(Self {
            concurrency: workers as usize,
            threads_per_worker: threads,
            time_limit_secs: timeout as f64,
            queues: env.list("PAPERRUST_QUEUE_NAMES", &[DEFAULT_QUEUE]),
            visibility_timeout_secs: positive_f64(env, "PAPERRUST_QUEUE_VISIBILITY_TIMEOUT", 60.0)?,
            max_deliveries: max_deliveries as i32,
            poll_interval_secs: positive_f64(env, "PAPERRUST_QUEUE_POLL_INTERVAL", 1.0)?,
            shutdown_grace_secs: positive_f64(env, "PAPERRUST_QUEUE_SHUTDOWN_GRACE", 30.0)?,
            result_expires_secs: positive_f64(env, "PAPERRUST_QUEUE_RESULT_EXPIRES", 3600.0)?,
            tz,
            worker_name: env
                .get("PAPERRUST_WORKER_NAME")
                .map(String::from)
                .unwrap_or_else(|| format!("paperrust@{host}:{}", std::process::id())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let c = QueueConfig::from_env(&Env::from_pairs([("HOSTNAME", "box")])).unwrap();
        assert_eq!(c.concurrency, 1);
        assert_eq!(c.time_limit_secs, 1800.0);
        assert_eq!(c.queues, vec!["celery".to_string()]);
        assert!(c.threads_per_worker >= 1);
        assert!(c.worker_name.starts_with("paperrust@box:"));
        let c = QueueConfig::from_env(&Env::from_pairs([
            ("PAPERRUST_QUEUE_BACKEND", "postgres"),
            ("PAPERLESS_TASK_WORKERS", "4"),
            ("PAPERLESS_THREADS_PER_WORKER", "2"),
            ("PAPERLESS_WORKER_TIMEOUT", "60"),
            ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
            ("PAPERRUST_QUEUE_NAMES", "celery, ocr"),
        ]))
        .unwrap();
        assert_eq!((c.concurrency, c.threads_per_worker, c.time_limit_secs), (4, 2, 60.0));
        assert_eq!(c.queues, vec!["celery".to_string(), "ocr".to_string()]);
        assert!(QueueConfig::from_env(&Env::from_pairs([("PAPERLESS_TASK_WORKERS", "x")])).is_err());
        // old values do not abort startup
        assert!(QueueConfig::from_env(&Env::from_pairs([("PAPERRUST_QUEUE_BACKEND", "celery")])).is_ok());
        assert!(QueueConfig::from_env(&Env::from_pairs([("PAPERRUST_QUEUE", "redis")])).is_ok());
    }
}
