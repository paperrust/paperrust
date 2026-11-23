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
//! `PAPERLESS_TASK_WORKERS` (never changed by the memory cap).
//!
//! `PAPERLESS_TASK_WORKERS` (also `PAPERRUST_TASK_WORKERS`): unset means **1** like upstream
//! ([`DEFAULT_TASK_WORKERS`]), so a single document gets all cores (`threads_per_worker = cores`).
//! The automatic parallel default from 2026-11-22 was withdrawn on 2026-11-23 (Pi 5, 3 cores: a
//! batch of 255 documents got faster, 214 s -> 162 s, but a single 8-page scan alone took 24 s
//! instead of 14.8 s). It stays available as opt-in with the value `auto` (case-insensitive,
//! [`is_auto`]): the number of parallel tasks is then derived from the machine
//! ([`default_task_workers`]): `ceil(cores / threads)` where `threads` is the explicit
//! `PAPERLESS_THREADS_PER_WORKER` or [`AUTO_THREADS_PER_TASK`], at most [`MAX_AUTO_TASK_WORKERS`],
//! and no more than fit into the memory budget at `threads × OCR_THREAD_MB` each (at least 1). An
//! explicit number is always used unchanged. The machine (cores, available memory) is measured
//! once per process ([`machine`]) so that pl-queue (slots) and pl-ocr (threads per task) always
//! derive the same numbers ([`task_workers_or_auto`]).
//!
//! The limited value is written into the process environment at program start as
//! `PAPERLESS_THREADS_PER_WORKER` ([`apply_ocr_resource_cap`]), so that pl-ocr and pl-queue, which read
//! the environment themselves, see the same value. If the limit does not apply, nothing is set.

use std::sync::OnceLock;

use crate::config::{ConfigError, Env};

/// Default of an unset `PAPERLESS_TASK_WORKERS` (upstream `get_int_from_env("PAPERLESS_TASK_WORKERS", 1)`).
pub const DEFAULT_TASK_WORKERS: i64 = 1;
/// Value of `PAPERLESS_TASK_WORKERS` that selects the automatic task count (PaperRust opt-in).
pub const TASK_WORKERS_AUTO: &str = "auto";

/// `true` if a raw `PAPERLESS_TASK_WORKERS` value is `auto` (case-insensitive, surrounding
/// whitespace ignored).
pub fn is_auto(raw: &str) -> bool {
    raw.trim().eq_ignore_ascii_case(TASK_WORKERS_AUTO)
}

/// Assumed memory requirement per parallel OCR instance (Tesseract 300 dpi ≈ 70 MB + raster).
pub const OCR_THREAD_MB: u64 = 100;
/// Share of the available memory that parallel OCR may occupy at most (rest:
/// server, PostgreSQL, page raster, other programs).
pub const RAM_SHARE_PERCENT: u64 = 50;
/// OCR threads per task assumed when deriving the automatic task count (`auto`, threads unset):
/// two tasks with two threads each on four cores. Page-parallel OCR of one big scan keeps two
/// threads; batches use all cores even when documents need no OCR (text PDFs: ~1 core each).
pub const AUTO_THREADS_PER_TASK: i64 = 2;
/// Upper bound of the automatic task count (more parallel documents mostly add database
/// connections, memory and contention on the media lock / search index without more speed).
pub const MAX_AUTO_TASK_WORKERS: i64 = 4;

/// Where `task_workers` comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkersSource {
    /// Not set – upstream default [`DEFAULT_TASK_WORKERS`].
    Default,
    /// `auto` – derived from cores and memory ([`default_task_workers`]).
    Auto,
    /// A number set by the user (invalid values count as set; the configuration reports them).
    Explicit,
}

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
    pub workers_source: WorkersSource,
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
            match self.workers_source {
                WorkersSource::Default => " (default)",
                WorkersSource::Auto => " (auto: from cores and memory)",
                WorkersSource::Explicit => " (set explicitly)",
            },
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

/// Automatic task count for `PAPERLESS_TASK_WORKERS=auto` (PaperRust opt-in; unset = 1).
///
/// `threads` = explicit `PAPERLESS_THREADS_PER_WORKER` (None = unset). Result:
/// `min(ceil(cores / t), MAX_AUTO_TASK_WORKERS, budget / (t × OCR_THREAD_MB))`, at least 1, with
/// `t = threads` or [`AUTO_THREADS_PER_TASK`] and `budget` = [`RAM_SHARE_PERCENT`] of the available
/// memory (no memory limit when unknown).
pub fn default_task_workers(threads: Option<i64>, cores: i64, available_mb: Option<u64>) -> i64 {
    let t = threads.filter(|t| *t >= 1).unwrap_or(AUTO_THREADS_PER_TASK);
    let cores = cores.max(1);
    let by_cores = ((cores + t - 1) / t).clamp(1, MAX_AUTO_TASK_WORKERS);
    match available_mb {
        Some(mb) => {
            let budget = mb.saturating_mul(RAM_SHARE_PERCENT) / 100;
            let by_mem = i64::try_from(budget / (t as u64).saturating_mul(OCR_THREAD_MB)).unwrap_or(i64::MAX);
            by_cores.min(by_mem).max(1)
        }
        None => by_cores,
    }
}

/// Maximum number of OCR threads per worker that fits into memory (at least 1).
pub fn memory_threads_cap(task_workers: i64, available_mb: u64) -> i64 {
    let budget = available_mb.saturating_mul(RAM_SHARE_PERCENT) / 100;
    let per_worker = budget / (task_workers.max(1) as u64 * OCR_THREAD_MB);
    i64::try_from(per_worker).unwrap_or(i64::MAX).max(1)
}

/// Pure calculation (tests). `task_workers` = number to use (None = `auto`), `threads` = set
/// value (None = not set). An unset `PAPERLESS_TASK_WORKERS` is `Some(DEFAULT_TASK_WORKERS)` here
/// ([`plan_from_env`] maps it).
pub fn plan_ocr_resources(task_workers: Option<i64>, threads: Option<i64>, cores: i64, available_mb: Option<u64>) -> ResourcePlan {
    let workers = task_workers.unwrap_or_else(|| default_task_workers(threads, cores, available_mb));
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
        workers_source: if task_workers.is_some() { WorkersSource::Explicit } else { WorkersSource::Auto },
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
    let raw_workers = env.get("PAPERLESS_TASK_WORKERS");
    let (workers, source) = match raw_workers {
        None => (Some(DEFAULT_TASK_WORKERS), WorkersSource::Default),
        Some(v) if is_auto(v) => (None, WorkersSource::Auto),
        Some(v) => (Some(v.trim().parse::<i64>().unwrap_or(DEFAULT_TASK_WORKERS)), WorkersSource::Explicit),
    };
    let threads = parse("PAPERLESS_THREADS_PER_WORKER");
    let mut plan = plan_ocr_resources(workers, threads, cores, available_mb);
    plan.workers_source = source;
    plan
}

/// Effective `PAPERLESS_TASK_WORKERS` from an [`Env`]: unset → [`DEFAULT_TASK_WORKERS`], `auto` →
/// [`task_workers_or_auto`], otherwise `int()` (ngx error text for invalid values). Range checks
/// (`>= 1`) are the caller's job.
pub fn task_workers_from_env(env: &Env) -> Result<i64, ConfigError> {
    match env.get("PAPERLESS_TASK_WORKERS") {
        Some(v) if is_auto(v) => Ok(task_workers_or_auto(None, env.int("PAPERLESS_THREADS_PER_WORKER")?)),
        _ => env.int_or("PAPERLESS_TASK_WORKERS", DEFAULT_TASK_WORKERS),
    }
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
static MACHINE: OnceLock<(i64, Option<u64>)> = OnceLock::new();

/// (logical cores, available memory in MB), measured once per process. Every automatic default
/// derives from this snapshot so that values computed at different times (worker pool start,
/// consumer rebuild after a settings change) agree.
pub fn machine() -> (i64, Option<u64>) {
    *MACHINE.get_or_init(|| (available_cores(), available_memory_mb()))
}

/// Task count for raw values read from the environment (`workers` = parsed number, None = the
/// value is `auto`; `threads` = parsed `PAPERLESS_THREADS_PER_WORKER`, None = unset). Callers map
/// an unset `PAPERLESS_TASK_WORKERS` to [`DEFAULT_TASK_WORKERS`] themselves (see
/// [`task_workers_from_env`]). An explicit worker count is returned unchanged (validation is the
/// caller's job). `auto`: the worker count of the startup plan when the startup cap wrote
/// `threads` itself (otherwise that written value would look explicit), else
/// [`default_task_workers`] on [`machine`].
pub fn task_workers_or_auto(workers: Option<i64>, threads: Option<i64>) -> i64 {
    if let Some(w) = workers {
        return w;
    }
    if let Some(p) = PLAN.get()
        && p.workers_source == WorkersSource::Auto
        && p.threads_source == ThreadsSource::MemoryCapped
        && threads == Some(p.threads_per_worker)
    {
        return p.task_workers;
    }
    let (cores, mem) = machine();
    default_task_workers(threads, cores, mem)
}

/// Computes the plan from process environment + `paperless.conf` and, if limited, writes
/// `PAPERLESS_THREADS_PER_WORKER` into the process environment. Only the first call has an effect.
///
/// # Safety
/// Changes the process environment (`std::env::set_var`). Call only at the very start of `main`
/// while no other thread is running (like [`crate::config::apply_env_aliases`], after it).
pub unsafe fn apply_ocr_resource_cap() -> &'static ResourcePlan {
    PLAN.get_or_init(|| {
        let (cores, mem) = machine();
        let plan = plan_from_env(&Env::from_process(), cores, mem);
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
    fn auto_task_workers() {
        // neither value set: ceil(cores / 2), at most 4
        assert_eq!(default_task_workers(None, 1, None), 1);
        assert_eq!(default_task_workers(None, 2, None), 1);
        assert_eq!(default_task_workers(None, 3, None), 2);
        assert_eq!(default_task_workers(None, 4, None), 2);
        assert_eq!(default_task_workers(None, 6, None), 3);
        assert_eq!(default_task_workers(None, 12, None), 4);
        assert_eq!(default_task_workers(None, 0, None), 1);
        // explicit threads: Pi 5 benchmark (3 cores, THREADS_PER_WORKER=2) -> 2 tasks
        assert_eq!(default_task_workers(Some(2), 3, Some(6000)), 2);
        assert_eq!(default_task_workers(Some(1), 3, Some(6000)), 3);
        assert_eq!(default_task_workers(Some(4), 4, Some(6000)), 1);
        // invalid thread values fall back to the assumption
        assert_eq!(default_task_workers(Some(0), 4, None), 2);
        assert_eq!(default_task_workers(Some(i64::MIN), 4, None), 2);
        // memory: 1 GB free -> 512 MB budget -> 2 tasks with 2 threads at 100 MB
        assert_eq!(default_task_workers(None, 12, Some(1024)), 2);
        // almost no memory -> 1
        assert_eq!(default_task_workers(None, 12, Some(50)), 1);
        assert_eq!(default_task_workers(None, 12, Some(0)), 1);
    }

    #[test]
    fn auto_workers_and_threads_never_oversubscribe() {
        for cores in 1..=64 {
            for mem in [None, Some(256), Some(1024), Some(4096), Some(65536)] {
                let p = plan_ocr_resources(None, None, cores, mem);
                assert!(p.task_workers >= 1 && p.threads_per_worker >= 1);
                assert!(p.task_workers <= MAX_AUTO_TASK_WORKERS);
                // floor(cores / workers) threads each: never more OCR threads than cores (once
                // there are at least as many cores as tasks)
                if cores >= p.task_workers {
                    assert!(p.task_workers * p.threads_per_worker <= cores, "cores {cores} mem {mem:?}: {p:?}");
                }
                if let Some(mb) = mem {
                    let budget = (mb * RAM_SHARE_PERCENT / 100) as i64;
                    if p.task_workers > 1 || p.threads_per_worker > 1 {
                        assert!(p.task_workers * p.threads_per_worker * OCR_THREAD_MB as i64 <= budget, "{p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn cap_by_memory() {
        // 12 cores, automatic tasks: 4 GB free -> 4 tasks, 3 threads each (upstream formula) fit
        let p = plan_ocr_resources(None, None, 12, Some(4096));
        assert_eq!((p.task_workers, p.threads_per_worker, p.threads_source), (4, 3, ThreadsSource::Default));
        assert_eq!(p.env_override(), None);
        // 1.5 GB free -> 768 MB budget -> 3 tasks; 768 / 300 = 2 threads instead of 4
        let p = plan_ocr_resources(None, None, 12, Some(1536));
        assert_eq!((p.task_workers, p.threads_per_worker, p.threads_source, p.upstream_threads), (3, 2, ThreadsSource::MemoryCapped, 4));
        assert_eq!(p.env_override(), Some(("PAPERLESS_THREADS_PER_WORKER", "2".to_string())));
        // 1 worker set (upstream setup): 1.5 GB free -> 768 MB -> 7 threads instead of 12
        let p = plan_ocr_resources(Some(1), None, 12, Some(1536));
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
        // an explicit task_workers is never changed automatically
        let p = plan_ocr_resources(Some(8), None, 12, Some(100));
        assert_eq!(p.task_workers, 8);
        // memory unknown -> automatic tasks, upstream thread formula
        let p = plan_ocr_resources(None, None, 12, None);
        assert_eq!((p.task_workers, p.threads_per_worker, p.threads_source), (4, 3, ThreadsSource::DefaultUnknownMemory));
        // explicit threads only: tasks follow them
        let p = plan_ocr_resources(None, Some(2), 3, Some(6000));
        assert_eq!((p.task_workers, p.threads_per_worker, p.threads_source), (2, 2, ThreadsSource::Explicit));
    }

    #[test]
    fn explicit_workers_pass_through() {
        assert_eq!(task_workers_or_auto(Some(1), None), 1);
        assert_eq!(task_workers_or_auto(Some(7), Some(1)), 7);
        let (cores, mem) = machine();
        assert_eq!(task_workers_or_auto(None, None), default_task_workers(None, cores, mem));
        assert_eq!(task_workers_or_auto(None, Some(1)), default_task_workers(Some(1), cores, mem));
    }

    #[test]
    fn from_env_aliases_and_conf() {
        let env = Env::from_pairs([("PAPERRUST_THREADS_PER_WORKER", "3")]);
        let p = plan_from_env(&env, 12, Some(100));
        assert_eq!((p.threads_per_worker, p.threads_source), (3, ThreadsSource::Explicit));
        let env = Env::from_pairs([("PAPERLESS_TASK_WORKERS", "2")]);
        let p = plan_from_env(&env, 12, Some(1000));
        assert_eq!((p.task_workers, p.workers_source, p.threads_per_worker), (2, WorkersSource::Explicit, 2));
        // invalid value: do not limit anything (the configuration reports the error itself)
        let env = Env::from_pairs([("PAPERLESS_THREADS_PER_WORKER", "x")]);
        assert_eq!(plan_from_env(&env, 12, Some(100)).env_override(), None);
        let env = Env::from_pairs([("PAPERLESS_TASK_WORKERS", "x")]);
        assert_eq!(plan_from_env(&env, 12, Some(100_000)).workers_source, WorkersSource::Explicit);
        // unset: 1 task (upstream), all cores for it; 800 MB free -> 400 MB budget -> 4 threads
        // instead of 12
        let p = plan_from_env(&Env::default(), 12, Some(800));
        assert_eq!(p.workers_source, WorkersSource::Default);
        assert_eq!((p.task_workers, p.threads_per_worker), (1, 4));
        assert!(p.describe().contains("capped from 12"));
        assert!(p.describe().contains("(default)"));
        let p = plan_from_env(&Env::default(), 3, Some(8000));
        assert_eq!((p.task_workers, p.threads_per_worker, p.threads_source), (1, 3, ThreadsSource::Default));
        // `auto` (any case, also via PAPERRUST_): automatic from cores and memory
        for v in ["auto", "AUTO", " Auto "] {
            let p = plan_from_env(&Env::from_pairs([("PAPERRUST_TASK_WORKERS", v)]), 12, Some(800));
            assert_eq!(p.workers_source, WorkersSource::Auto);
            assert_eq!((p.task_workers, p.threads_per_worker), (2, 2));
            assert!(p.describe().contains("capped from 6"));
            assert!(p.describe().contains("auto: from cores and memory"));
        }
        let p = plan_from_env(&Env::from_pairs([("PAPERLESS_TASK_WORKERS", "auto")]), 3, Some(8000));
        assert_eq!((p.task_workers, p.threads_per_worker), (2, 1));
    }

    #[test]
    fn task_workers_from_env_values() {
        assert_eq!(task_workers_from_env(&Env::default()).unwrap(), 1);
        assert_eq!(task_workers_from_env(&Env::from_pairs([("PAPERRUST_TASK_WORKERS", "3")])).unwrap(), 3);
        let (cores, mem) = machine();
        assert_eq!(
            task_workers_from_env(&Env::from_pairs([("PAPERRUST_TASK_WORKERS", "Auto")])).unwrap(),
            default_task_workers(None, cores, mem)
        );
        assert_eq!(
            task_workers_from_env(&Env::from_pairs([("PAPERLESS_TASK_WORKERS", "auto"), ("PAPERLESS_THREADS_PER_WORKER", "1")])).unwrap(),
            default_task_workers(Some(1), cores, mem)
        );
        let e = task_workers_from_env(&Env::from_pairs([("PAPERLESS_TASK_WORKERS", "autox")])).unwrap_err().to_string();
        assert!(e.contains("invalid literal for int() with base 10: 'autox'"), "{e}");
        assert!(
            is_auto("auto")
                && is_auto(
                    " AUTO
"
                )
                && !is_auto("")
                && !is_auto("automatic")
        );
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
