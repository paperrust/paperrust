//! Automatic limiting of parallel OCR according to free memory.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (`TASK_WORKERS =
//! get_int_from_env("PAPERLESS_TASK_WORKERS", 1)`, `THREADS_PER_WORKER =
//! os.getenv("PAPERLESS_THREADS_PER_WORKER", default_threads_per_worker(TASK_WORKERS))`,
//! `default_threads_per_worker = max(floor(cpu_count / task_workers), 1)`).
//!
//! PaperRust addition: each parallel Tesseract instance needs about 70 MB at 300 dpi
//! (results/bench/ingest). Upstream only counts cores; on machines with many cores and
//! little free RAM (laptop 8 GB, 12 logical cores) `task_workers × threads_per_worker`
//! Tesseract processes can exhaust memory. If **`PAPERLESS_THREADS_PER_WORKER` is not
//! set** (environment or `paperless.conf`, also as `PAPERRUST_THREADS_PER_WORKER`), the
//! upstream default is lowered so far that
//! `task_workers × threads_per_worker × OCR_THREAD_MB` fits into half of the memory
//! available at startup (at least 1). If the value is set, it stays unchanged; likewise
//! `PAPERLESS_TASK_WORKERS` (default 1 like upstream, never changed automatically).
//!
//! The limited value is written into the process environment at program start as
//! `PAPERLESS_THREADS_PER_WORKER` ([`apply_ocr_resource_cap`]), so that pl-ocr and pl-queue, which read
//! the environment themselves, see the same value. If the limit does not apply, nothing is set.

use std::sync::OnceLock;

use crate::config::Env;

/// Assumed memory requirement per parallel OCR instance (Tesseract 300 dpi ≈ 70 MB + raster).
pub const OCR_THREAD_MB: u64 = 100;
/// Share of the available memory that parallel OCR may occupy at most (rest:
/// server, PostgreSQL, page raster, other programs).
pub const RAM_SHARE_PERCENT: u64 = 50;

/// Where `threads_per_worker` comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadsSource {
    /// Set by the user – unchanged.
    Explicit,
    /// upstream default `max(cores / task_workers, 1)` – fits into memory.
    Default,
    /// upstream default lowered according to free memory.
    MemoryCapped,
    /// Available memory unknown (platform) – upstream default.
    DefaultUnknownMemory,
}

/// Result of [`plan_ocr_resources`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePlan {
    pub task_workers: i64,
    pub task_workers_explicit: bool,
    pub threads_per_worker: i64,
    pub threads_source: ThreadsSource,
    /// upstream default (for comparison in the log).
    pub upstream_threads: i64,
    pub cores: i64,
    pub available_mb: Option<u64>,
}

impl ResourcePlan {
    /// `PAPERLESS_THREADS_PER_WORKER` that must be written into the process environment.
    pub fn env_override(&self) -> Option<(&'static str, String)> {
        (self.threads_source == ThreadsSource::MemoryCapped).then(|| ("PAPERLESS_THREADS_PER_WORKER", self.threads_per_worker.to_string()))
    }

    /// One line for the startup log.
    pub fn describe(&self) -> String {
        let mem = self.available_mb.map_or_else(|| "unknown".to_string(), |m| format!("{m} MB"));
        let src = match self.threads_source {
            ThreadsSource::Explicit => "set explicitly".to_string(),
            ThreadsSource::Default => "upstream default (cores / task workers)".to_string(),
            ThreadsSource::MemoryCapped => format!(
                "capped from {} to fit {}% of available memory at ~{OCR_THREAD_MB} MB per OCR thread",
                self.upstream_threads, RAM_SHARE_PERCENT
            ),
            ThreadsSource::DefaultUnknownMemory => "upstream default (available memory unknown)".to_string(),
        };
        format!(
            "task workers: {}{}, OCR threads per worker: {} ({src}); cores: {}, available memory: {mem}",
            self.task_workers,
            if self.task_workers_explicit { " (set explicitly)" } else { " (default)" },
            self.threads_per_worker,
            self.cores,
        )
    }
}

/// `max(floor(cores / task_workers), 1)` (Python: `task_workers == 0` → ZeroDivisionError; here 1).
pub fn upstream_threads_per_worker(task_workers: i64, cores: i64) -> i64 {
    if task_workers <= 0 {
        return 1;
    }
    cores.max(1).div_euclid(task_workers).max(1)
}

/// Maximum number of OCR threads per worker that fits into memory (at least 1).
pub fn memory_threads_cap(task_workers: i64, available_mb: u64) -> i64 {
    let budget = available_mb.saturating_mul(RAM_SHARE_PERCENT) / 100;
    let per_worker = budget / (task_workers.max(1) as u64 * OCR_THREAD_MB);
    i64::try_from(per_worker).unwrap_or(i64::MAX).max(1)
}

/// Pure calculation (tests). `task_workers`/`threads` = set values (None = not set).
pub fn plan_ocr_resources(task_workers: Option<i64>, threads: Option<i64>, cores: i64, available_mb: Option<u64>) -> ResourcePlan {
    let workers = task_workers.unwrap_or(1);
    let upstream = upstream_threads_per_worker(workers, cores);
    let (threads_per_worker, threads_source) = match (threads, available_mb) {
        (Some(t), _) => (t, ThreadsSource::Explicit),
        (None, None) => (upstream, ThreadsSource::DefaultUnknownMemory),
        (None, Some(mb)) => {
            let cap = memory_threads_cap(workers, mb);
            if cap < upstream { (cap, ThreadsSource::MemoryCapped) } else { (upstream, ThreadsSource::Default) }
        }
    };
    ResourcePlan {
        task_workers: workers,
        task_workers_explicit: task_workers.is_some(),
        threads_per_worker,
        threads_source,
        upstream_threads: upstream,
        cores: cores.max(1),
        available_mb,
    }
}

/// Plan for an environment (incl. `paperless.conf`). Invalid numbers count as set (the
/// error message comes later from the configuration itself; nothing is limited here then).
pub fn plan_from_env(env: &Env, cores: i64, available_mb: Option<u64>) -> ResourcePlan {
    let parse = |k: &str| env.get(k).map(|v| v.trim().parse::<i64>().unwrap_or(i64::MIN));
    let workers = parse("PAPERLESS_TASK_WORKERS").filter(|w| *w != i64::MIN);
    let explicit_workers = env.get("PAPERLESS_TASK_WORKERS").is_some();
    let threads = parse("PAPERLESS_THREADS_PER_WORKER");
    let mut plan = plan_ocr_resources(workers, threads, cores, available_mb);
    plan.task_workers_explicit = explicit_workers;
    plan
}

/// Logical cores (`os.cpu_count()`).
pub fn available_cores() -> i64 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as i64)
}

/// Available memory in MB (Windows: `ullAvailPhys`; Linux: `MemAvailable`, in a
/// container additionally limited by the cgroup limit). Other platforms: None.
pub fn available_memory_mb() -> Option<u64> {
    imp::available_memory_bytes().map(|b| b / (1024 * 1024))
}

#[cfg(windows)]
mod imp {
    #[repr(C)]
    #[allow(non_snake_case)]
    struct MemoryStatusEx {
        dwLength: u32,
        dwMemoryLoad: u32,
        ullTotalPhys: u64,
        ullAvailPhys: u64,
        ullTotalPageFile: u64,
        ullAvailPageFile: u64,
        ullTotalVirtual: u64,
        ullAvailVirtual: u64,
        ullAvailExtendedVirtual: u64,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    pub fn available_memory_bytes() -> Option<u64> {
        let mut s = MemoryStatusEx {
            dwLength: std::mem::size_of::<MemoryStatusEx>() as u32,
            dwMemoryLoad: 0,
            ullTotalPhys: 0,
            ullAvailPhys: 0,
            ullTotalPageFile: 0,
            ullAvailPageFile: 0,
            ullTotalVirtual: 0,
            ullAvailVirtual: 0,
            ullAvailExtendedVirtual: 0,
        };
        // SAFETY: valid, correctly initialized buffer with dwLength set.
        let ok = unsafe { GlobalMemoryStatusEx(&mut s) };
        (ok != 0).then_some(s.ullAvailPhys)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub fn available_memory_bytes() -> Option<u64> {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let host = super::parse_meminfo_available(&meminfo)?;
        let max = std::fs::read_to_string("/sys/fs/cgroup/memory.max").ok();
        let cur = std::fs::read_to_string("/sys/fs/cgroup/memory.current").ok();
        Some(match super::cgroup_available(max.as_deref(), cur.as_deref()) {
            Some(c) => host.min(c),
            None => host,
        })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    pub fn available_memory_bytes() -> Option<u64> {
        None
    }
}

/// `MemAvailable:  123456 kB` → Bytes.
pub fn parse_meminfo_available(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// cgroup v2: `memory.max` (`max` = unlimited) minus `memory.current`.
pub fn cgroup_available(max: Option<&str>, current: Option<&str>) -> Option<u64> {
    let max: u64 = max?.trim().parse().ok()?;
    let cur: u64 = current.and_then(|c| c.trim().parse().ok()).unwrap_or(0);
    Some(max.saturating_sub(cur))
}

static PLAN: OnceLock<ResourcePlan> = OnceLock::new();

/// Computes the plan from process environment + `paperless.conf` and, if limited, writes
/// `PAPERLESS_THREADS_PER_WORKER` into the process environment. Only the first call has an effect.
///
/// # Safety
/// Changes the process environment (`std::env::set_var`). Call only at the very start of `main`
/// while no other thread is running (like [`crate::config::apply_env_aliases`], after it).
pub unsafe fn apply_ocr_resource_cap() -> &'static ResourcePlan {
    PLAN.get_or_init(|| {
        let plan = plan_from_env(&Env::from_process(), available_cores(), available_memory_mb());
        if let Some((k, v)) = plan.env_override() {
            // SAFETY: per this function's contract no other thread is running yet.
            unsafe { std::env::set_var(k, v) };
        }
        plan
    })
}

/// Plan from [`apply_ocr_resource_cap`] (None if not called).
pub fn applied_plan() -> Option<&'static ResourcePlan> {
    PLAN.get()
}

/// Startup log (after the logger is set up).
pub fn log_resource_plan() {
    if let Some(p) = PLAN.get() {
        if p.threads_source == ThreadsSource::MemoryCapped {
            tracing::warn!("parallel OCR limited by available memory: {}", p.describe());
        } else {
            tracing::info!("parallel OCR: {}", p.describe());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_default() {
        assert_eq!(upstream_threads_per_worker(1, 12), 12);
        assert_eq!(upstream_threads_per_worker(5, 12), 2);
        assert_eq!(upstream_threads_per_worker(20, 12), 1);
        assert_eq!(upstream_threads_per_worker(0, 12), 1);
    }

    #[test]
    fn cap_by_memory() {
        // 12 cores, 1 worker: 4 GB free -> 2 GB budget -> 20 threads > 12 -> upstream stays
        let p = plan_ocr_resources(None, None, 12, Some(4096));
        assert_eq!((p.threads_per_worker, p.threads_source), (12, ThreadsSource::Default));
        assert_eq!(p.env_override(), None);
        // 1,5 GB frei -> 768 MB -> 7 Threads
        let p = plan_ocr_resources(None, None, 12, Some(1536));
        assert_eq!((p.threads_per_worker, p.threads_source, p.upstream_threads), (7, ThreadsSource::MemoryCapped, 12));
        assert_eq!(p.env_override(), Some(("PAPERLESS_THREADS_PER_WORKER", "7".to_string())));
        // 3 workers set, 2 GB free -> 1024 / 300 = 3 instead of 4
        let p = plan_ocr_resources(Some(3), None, 12, Some(2048));
        assert_eq!((p.task_workers, p.threads_per_worker, p.threads_source), (3, 3, ThreadsSource::MemoryCapped));
        assert!(p.task_workers * p.threads_per_worker * OCR_THREAD_MB as i64 <= 2048 * RAM_SHARE_PERCENT as i64 / 100);
        // almost no memory -> at least 1
        let p = plan_ocr_resources(Some(4), None, 12, Some(50));
        assert_eq!(p.threads_per_worker, 1);
        assert_eq!(memory_threads_cap(1, 0), 1);
    }

    #[test]
    fn explicit_values_stay() {
        let p = plan_ocr_resources(Some(2), Some(6), 12, Some(100));
        assert_eq!((p.task_workers, p.threads_per_worker, p.threads_source), (2, 6, ThreadsSource::Explicit));
        assert_eq!(p.env_override(), None);
        // task_workers is never changed automatically
        let p = plan_ocr_resources(Some(8), None, 12, Some(100));
        assert_eq!(p.task_workers, 8);
        // Memory unknown -> upstream
        let p = plan_ocr_resources(None, None, 12, None);
        assert_eq!((p.threads_per_worker, p.threads_source), (12, ThreadsSource::DefaultUnknownMemory));
    }

    #[test]
    fn from_env_aliases_and_conf() {
        let env = Env::from_pairs([("PAPERRUST_THREADS_PER_WORKER", "3")]);
        let p = plan_from_env(&env, 12, Some(100));
        assert_eq!((p.threads_per_worker, p.threads_source), (3, ThreadsSource::Explicit));
        let env = Env::from_pairs([("PAPERLESS_TASK_WORKERS", "2")]);
        let p = plan_from_env(&env, 12, Some(1000));
        assert_eq!((p.task_workers, p.task_workers_explicit, p.threads_per_worker), (2, true, 2));
        // invalid value: do not limit anything (the configuration reports the error itself)
        let env = Env::from_pairs([("PAPERLESS_THREADS_PER_WORKER", "x")]);
        assert_eq!(plan_from_env(&env, 12, Some(100)).env_override(), None);
        let p = plan_from_env(&Env::default(), 12, Some(800));
        assert!(!p.task_workers_explicit);
        assert_eq!(p.threads_per_worker, 4);
        assert!(p.describe().contains("capped from 12"));
    }

    #[test]
    fn meminfo_and_cgroup() {
        let t = "MemTotal:       16000000 kB\nMemFree:  1000 kB\nMemAvailable:    2048000 kB\n";
        assert_eq!(parse_meminfo_available(t), Some(2_048_000 * 1024));
        assert_eq!(parse_meminfo_available("MemTotal: 1 kB"), None);
        assert_eq!(cgroup_available(Some("max\n"), Some("100")), None);
        assert_eq!(cgroup_available(Some("1073741824\n"), Some("73741824\n")), Some(1_000_000_000));
        assert_eq!(cgroup_available(None, None), None);
    }

    #[test]
    fn reads_memory_here() {
        if cfg!(any(windows, target_os = "linux")) {
            assert!(available_memory_mb().is_some_and(|m| m > 0));
        }
    }
}
