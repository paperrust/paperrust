//! Parallel tasks and OCR threads from cores and free memory.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (`TASK_WORKERS =
//! get_int_from_env("PAPERLESS_TASK_WORKERS", 1)`, `THREADS_PER_WORKER =
//! os.getenv("PAPERLESS_THREADS_PER_WORKER", default_threads_per_worker(TASK_WORKERS))`,
//! `default_threads_per_worker = max(floor(cpu_count / task_workers), 1)`).
//!
//! PaperRust differs from upstream in three ways:
//!
//! 1. **`PAPERLESS_TASK_WORKERS` (also `PAPERRUST_TASK_WORKERS`) unset means `auto`** since
//!    2026-11-23 ([`DEFAULT_TASK_WORKERS`]; upstream: 1). The number of parallel tasks is derived
//!    from the machine ([`default_task_workers`]): `ceil(cores / threads)` where `threads` is the
//!    explicit `PAPERLESS_THREADS_PER_WORKER` or [`AUTO_THREADS_PER_TASK`], at most
//!    [`MAX_AUTO_TASK_WORKERS`], and no more than fit into the memory budget at
//!    `threads × OCR_THREAD_MB` each (at least 1). `auto` (case-insensitive, [`is_auto`]) stays
//!    accepted explicitly; a number is always used unchanged (`1` = one document at a time, like
//!    upstream). History: an automatic default existed on 2026-11-22, was withdrawn on 2026-11-23
//!    (a lone 8-page scan took 24 s instead of 14.8 s on 3 cores because it only got
//!    `cores / tasks` threads) and came back the same day together with the shared page pool
//!    (results/benchmark/2026-11-23-nas-adaptive: lone scan 15.5 s as serial, OCR batch 55 s
//!    instead of 95 s, 0 errors).
//! 2. **OCR threads come from one shared page pool** (pl-ocr `permits`): the whole process runs at
//!    most `P` page tasks at once and a single document may use up to `P` of them, so a lone
//!    document gets every core and concurrent documents share them. `P` = cores, lowered so that
//!    `P × OCR_THREAD_MB` fits into [`RAM_SHARE_PERCENT`] of the memory available at startup
//!    ([`pool_threads`]); with an explicit `PAPERLESS_THREADS_PER_WORKER` the pool has `cores`
//!    permits (no memory cap, like before) and the value caps each document.
//! 3. **Memory cap written into the environment:** when `P < cores` and
//!    `PAPERLESS_THREADS_PER_WORKER` is unset, [`apply_ocr_resource_cap`] writes
//!    `PAPERLESS_THREADS_PER_WORKER = P` (= [`memory_threads_cap`]`(1, mem)`, the pool total) into
//!    the process environment so that pl-ocr and pl-queue, which read the environment themselves,
//!    see the same per-document cap. pl-ocr recognises the written value
//!    ([`threads_written_by_cap`]) so that it does not count as "set by the user" for the pool size.
//!
//! The machine (cores, available memory) is measured once per process ([`machine`]) so that
//! pl-queue (slots) and pl-ocr (threads, pool) always derive the same numbers. Cores come from
//! `std::thread::available_parallelism`, which on Linux already honours the affinity mask / cpuset
//! (`sched_getaffinity`, e.g. `docker --cpuset-cpus`) and cgroup v1/v2 CPU quotas (`cpu.max`,
//! `cpu.cfs_quota_us` / `cpu.cfs_period_us`, e.g. `docker --cpus`); memory respects cgroup v1/v2
//! limits ([`available_memory_mb`]).

use std::sync::OnceLock;

use crate::config::{ConfigError, Env};

/// Value of `PAPERLESS_TASK_WORKERS` that selects the automatic task count.
pub const TASK_WORKERS_AUTO: &str = "auto";
/// Meaning of an unset `PAPERLESS_TASK_WORKERS` (PaperRust: `auto`; upstream: `1`).
pub const DEFAULT_TASK_WORKERS: &str = TASK_WORKERS_AUTO;
/// upstream default of `PAPERLESS_TASK_WORKERS` (one task at a time); also the fallback for an
/// invalid value in [`plan_from_env`] (the configuration reports the error itself).
pub const UPSTREAM_TASK_WORKERS: i64 = 1;

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
/// two tasks per four cores. Batches use all cores even when documents need no OCR (text PDFs:
/// ~1 core each); the page pool gives a lone document all cores anyway.
pub const AUTO_THREADS_PER_TASK: i64 = 2;
/// Upper bound of the automatic task count (more parallel documents mostly add database
/// connections, memory and contention on the media lock / search index without more speed).
pub const MAX_AUTO_TASK_WORKERS: i64 = 4;

/// Where `task_workers` comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkersSource {
    /// Not set – PaperRust default `auto` ([`DEFAULT_TASK_WORKERS`], [`default_task_workers`]).
    Default,
    /// `auto` set explicitly – derived from cores and memory ([`default_task_workers`]).
    Auto,
    /// A number set by the user (invalid values count as set; the configuration reports them).
    Explicit,
}

/// Where the OCR page pool / per-document threads come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadsSource {
    /// `PAPERLESS_THREADS_PER_WORKER` set by the user – per-document cap unchanged, pool = cores.
    Explicit,
    /// Unset – pool = all cores (fits into memory).
    Default,
    /// Unset – pool lowered below the cores according to free memory.
    MemoryCapped,
    /// Unset, available memory unknown (platform) – pool = all cores.
    DefaultUnknownMemory,
}

/// Result of [`plan_ocr_resources`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePlan {
    pub task_workers: i64,
    pub workers_source: WorkersSource,
    /// Page tasks one document may run at the same time (`PAPERLESS_THREADS_PER_WORKER` or the
    /// pool size).
    pub threads_per_worker: i64,
    pub threads_source: ThreadsSource,
    /// Size of the process-wide OCR page pool (page tasks of all documents together).
    pub pool_threads: i64,
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
        let workers = match self.workers_source {
            WorkersSource::Default => "default: auto from cores and memory",
            WorkersSource::Auto => "auto: from cores and memory",
            WorkersSource::Explicit => "set explicitly",
        };
        let per_doc = match self.threads_source {
            ThreadsSource::Explicit => format!("per document up to {}, set explicitly", self.threads_per_worker),
            ThreadsSource::Default => format!("per document up to {}", self.threads_per_worker),
            ThreadsSource::MemoryCapped => format!(
                "per document up to {}; capped from {} cores to fit {}% of available memory at ~{OCR_THREAD_MB} MB per OCR thread",
                self.threads_per_worker, self.cores, RAM_SHARE_PERCENT
            ),
            ThreadsSource::DefaultUnknownMemory => format!("per document up to {}; available memory unknown", self.threads_per_worker),
        };
        format!(
            "task workers: {} ({workers}); OCR: shared page pool of {} threads ({per_doc}); cores: {}, available memory: {mem}",
            self.task_workers, self.pool_threads, self.cores,
        )
    }
}

/// `max(floor(cores / task_workers), 1)` (Python: `task_workers == 0` → ZeroDivisionError; here 1).
/// Kept for comparison; PaperRust shares the cores through the page pool ([`pool_threads`]).
pub fn upstream_threads_per_worker(task_workers: i64, cores: i64) -> i64 {
    if task_workers <= 0 {
        return 1;
    }
    cores.max(1).div_euclid(task_workers).max(1)
}

/// Automatic task count (unset `PAPERLESS_TASK_WORKERS` or `auto`).
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

/// Maximum number of OCR threads per worker that fits into memory (at least 1). With
/// `task_workers = 1` this is the memory cap of the shared page pool.
pub fn memory_threads_cap(task_workers: i64, available_mb: u64) -> i64 {
    let budget = available_mb.saturating_mul(RAM_SHARE_PERCENT) / 100;
    let per_worker = budget / (task_workers.max(1) as u64 * OCR_THREAD_MB);
    i64::try_from(per_worker).unwrap_or(i64::MAX).max(1)
}

/// Size of the shared OCR page pool: logical cores, lowered to [`memory_threads_cap`]`(1, mem)`
/// unless `PAPERLESS_THREADS_PER_WORKER` was set by the user (then no memory cap, like before –
/// but still never more page tasks than cores). At least 1.
pub fn pool_threads(cores: i64, available_mb: Option<u64>, threads_explicit: bool) -> i64 {
    let cores = cores.max(1);
    match (threads_explicit, available_mb) {
        (false, Some(mb)) => cores.min(memory_threads_cap(1, mb)),
        _ => cores,
    }
}

/// Pure calculation (tests). `task_workers` = number to use (None = automatic), `threads` = set
/// value (None = not set). [`plan_from_env`] maps an unset `PAPERLESS_TASK_WORKERS` to None.
pub fn plan_ocr_resources(task_workers: Option<i64>, threads: Option<i64>, cores: i64, available_mb: Option<u64>) -> ResourcePlan {
    let workers = task_workers.unwrap_or_else(|| default_task_workers(threads, cores, available_mb));
    let cores = cores.max(1);
    let pool = pool_threads(cores, available_mb, threads.is_some());
    let (threads_per_worker, threads_source) = match (threads, available_mb) {
        (Some(t), _) => (t, ThreadsSource::Explicit),
        (None, None) => (pool, ThreadsSource::DefaultUnknownMemory),
        (None, Some(_)) if pool < cores => (pool, ThreadsSource::MemoryCapped),
        (None, Some(_)) => (pool, ThreadsSource::Default),
    };
    ResourcePlan {
        task_workers: workers,
        workers_source: if task_workers.is_some() { WorkersSource::Explicit } else { WorkersSource::Auto },
        threads_per_worker,
        threads_source,
        pool_threads: pool,
        cores,
        available_mb,
    }
}

/// Plan for an environment (incl. `paperless.conf`). Invalid numbers count as set (the
/// error message comes later from the configuration itself; nothing is limited here then).
pub fn plan_from_env(env: &Env, cores: i64, available_mb: Option<u64>) -> ResourcePlan {
    let parse = |k: &str| env.get(k).map(|v| v.trim().parse::<i64>().unwrap_or(i64::MIN));
    let (workers, source) = match env.get("PAPERLESS_TASK_WORKERS") {
        None => (None, WorkersSource::Default),
        Some(v) if is_auto(v) => (None, WorkersSource::Auto),
        Some(v) => (Some(v.trim().parse::<i64>().unwrap_or(UPSTREAM_TASK_WORKERS)), WorkersSource::Explicit),
    };
    let threads = parse("PAPERLESS_THREADS_PER_WORKER");
    let mut plan = plan_ocr_resources(workers, threads, cores, available_mb);
    plan.workers_source = source;
    plan
}

/// Effective `PAPERLESS_TASK_WORKERS` from an [`Env`]: unset or `auto` → [`task_workers_or_auto`],
/// otherwise `int()` (ngx error text for invalid values). Range checks (`>= 1`) are the caller's
/// job.
pub fn task_workers_from_env(env: &Env) -> Result<i64, ConfigError> {
    match env.get("PAPERLESS_TASK_WORKERS") {
        None => Ok(task_workers_or_auto(None, env.int("PAPERLESS_THREADS_PER_WORKER")?)),
        Some(v) if is_auto(v) => Ok(task_workers_or_auto(None, env.int("PAPERLESS_THREADS_PER_WORKER")?)),
        Some(_) => env.int_or("PAPERLESS_TASK_WORKERS", UPSTREAM_TASK_WORKERS),
    }
}

/// Logical cores (`os.cpu_count()`), honouring affinity/cpuset and cgroup CPU quotas on Linux
/// (see the module docs).
pub fn available_cores() -> i64 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as i64)
}

/// Available memory in MB (Windows: `ullAvailPhys`; Linux: `MemAvailable`, limited by the cgroup
/// v1/v2 memory limits of this process and its ancestors – container or systemd `MemoryMax=`).
/// Other platforms: None.
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
    fn read(path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    pub fn available_memory_bytes() -> Option<u64> {
        let host = super::parse_meminfo_available(&read("/proc/meminfo")?)?;
        Some(match super::cgroup_memory_available(&read) {
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
    kb.checked_mul(1024)
}

/// Limits at or above this count as "no limit" (cgroup v1 reports unlimited as
/// `9223372036854771712`, i.e. `i64::MAX` rounded down to the page size).
pub const CGROUP_UNLIMITED_BYTES: u64 = 1 << 60;

/// A cgroup memory limit (`memory.max` / `memory.limit_in_bytes`): bytes, or None for `max`,
/// huge "unlimited" values ([`CGROUP_UNLIMITED_BYTES`]) and unparsable text.
pub fn parse_cgroup_limit(text: &str) -> Option<u64> {
    let t = text.trim();
    if t == "max" {
        return None;
    }
    t.parse::<u64>().ok().filter(|v| *v < CGROUP_UNLIMITED_BYTES)
}

/// Value of `key` in a `memory.stat` file (`inactive_file 12345` lines).
pub fn parse_memory_stat(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next() == Some(key)).then(|| it.next().and_then(|v| v.parse().ok())).flatten()
    })
}

/// Memory still available below a cgroup limit: `limit − (usage − inactive_file)` (reclaimable
/// page cache does not count as used, like `docker stats`). None if there is no limit; a missing
/// usage counts as 0.
pub fn cgroup_available(limit: Option<&str>, usage: Option<&str>, inactive_file: Option<u64>) -> Option<u64> {
    let limit = parse_cgroup_limit(limit?)?;
    let used: u64 = usage.and_then(|c| c.trim().parse().ok()).unwrap_or(0);
    Some(limit.saturating_sub(used.saturating_sub(inactive_file.unwrap_or(0))))
}

/// cgroup hierarchy version for [`parse_proc_self_cgroup`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgroupVersion {
    /// Unified hierarchy (`0::/path`).
    V2,
    /// Legacy hierarchy, memory controller (`N:memory:/path`, also `N:foo,memory:/path`).
    V1,
}

/// Own cgroup path from `/proc/self/cgroup` (e.g. `/system.slice/paperrust.service`).
pub fn parse_proc_self_cgroup(text: &str, version: CgroupVersion) -> Option<&str> {
    text.lines().find_map(|l| {
        let mut parts = l.splitn(3, ':');
        let (id, controllers, path) = (parts.next()?, parts.next()?, parts.next()?);
        let hit = match version {
            CgroupVersion::V2 => id == "0" && controllers.is_empty(),
            CgroupVersion::V1 => controllers.split(',').any(|c| c == "memory"),
        };
        (hit && path.starts_with('/')).then_some(path.trim_end())
    })
}

/// `/a/b` → `["/a/b", "/a", "/"]` (deepest first, always ending with the root).
pub fn cgroup_ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = path.trim_end_matches('/');
    while !cur.is_empty() {
        out.push(cur.to_string());
        cur = match cur.rfind('/') {
            Some(i) => &cur[..i],
            None => "",
        };
    }
    out.push("/".to_string());
    out
}

/// Smallest `limit − (usage − inactive_file)` over this process's cgroup and its ancestors, for
/// cgroup v2 (`/sys/fs/cgroup/<path>/memory.max`, `memory.current`) and v1
/// (`/sys/fs/cgroup/memory/<path>/memory.limit_in_bytes`, `memory.usage_in_bytes`); levels without
/// a limit file or without a limit are skipped. The mount root is always checked as well: in a
/// container the own cgroup is usually mounted there although `/proc/self/cgroup` names a host
/// path. `read` returns a file's content (the real file system on Linux, a map in tests). None if
/// no limit applies.
pub fn cgroup_memory_available(read: &dyn Fn(&str) -> Option<String>) -> Option<u64> {
    let own = read("/proc/self/cgroup").unwrap_or_default();
    let mut best: Option<u64> = None;
    for (version, mount, limit, usage, stat_key) in [
        (CgroupVersion::V2, "/sys/fs/cgroup", "memory.max", "memory.current", "inactive_file"),
        (CgroupVersion::V1, "/sys/fs/cgroup/memory", "memory.limit_in_bytes", "memory.usage_in_bytes", "total_inactive_file"),
    ] {
        let rel = parse_proc_self_cgroup(&own, version).unwrap_or("/");
        for dir in cgroup_ancestors(rel) {
            let base = if dir == "/" { mount.to_string() } else { format!("{mount}{dir}") };
            let Some(lim) = read(&format!("{base}/{limit}")) else { continue };
            let inactive = read(&format!("{base}/memory.stat")).and_then(|s| parse_memory_stat(&s, stat_key));
            let used = read(&format!("{base}/{usage}"));
            if let Some(avail) = cgroup_available(Some(&lim), used.as_deref(), inactive) {
                best = Some(best.map_or(avail, |b| b.min(avail)));
            }
        }
    }
    best
}

static PLAN: OnceLock<ResourcePlan> = OnceLock::new();
static MACHINE: OnceLock<(i64, Option<u64>)> = OnceLock::new();

/// (logical cores, available memory in MB), measured once per process. Every automatic default
/// derives from this snapshot so that values computed at different times (worker pool start,
/// consumer rebuild after a settings change) agree.
pub fn machine() -> (i64, Option<u64>) {
    *MACHINE.get_or_init(|| (available_cores(), available_memory_mb()))
}

/// `true` if `threads` is the `PAPERLESS_THREADS_PER_WORKER` value that the startup memory cap
/// ([`apply_ocr_resource_cap`]) wrote into the environment itself (the pool total `P`), not one
/// set by the user.
pub fn threads_written_by_cap(threads: i64) -> bool {
    PLAN.get().is_some_and(|p| p.threads_source == ThreadsSource::MemoryCapped && p.threads_per_worker == threads)
}

/// Page tasks one document may run at once for a parsed `PAPERLESS_THREADS_PER_WORKER` (None =
/// unset): an explicit value (at least 1), otherwise the pool size on [`machine`]. A value written
/// by the startup cap is the pool size already, so both paths agree.
pub fn ocr_threads_per_document(threads: Option<i64>) -> i64 {
    match threads {
        Some(t) => t.max(1),
        None => {
            let (cores, mem) = machine();
            pool_threads(cores, mem, false)
        }
    }
}

/// Task count for raw values read from the environment (`workers` = parsed number, None = unset
/// or `auto`; `threads` = parsed `PAPERLESS_THREADS_PER_WORKER`, None = unset). An explicit worker
/// count is returned unchanged (validation is the caller's job). Automatic: the worker count of
/// the startup plan when the startup cap wrote `threads` itself (otherwise that written value
/// would look explicit and change the derivation), else [`default_task_workers`] on [`machine`].
pub fn task_workers_or_auto(workers: Option<i64>, threads: Option<i64>) -> i64 {
    if let Some(w) = workers {
        return w;
    }
    if let Some(p) = PLAN.get()
        && p.workers_source != WorkersSource::Explicit
        && threads.is_some_and(threads_written_by_cap)
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
    fn pool_never_oversubscribes() {
        for cores in 1..=64 {
            for mem in [None, Some(0), Some(256), Some(1024), Some(4096), Some(65536)] {
                let p = plan_ocr_resources(None, None, cores, mem);
                assert!(p.task_workers >= 1 && p.threads_per_worker >= 1 && p.pool_threads >= 1);
                assert!(p.task_workers <= MAX_AUTO_TASK_WORKERS);
                // the pool never has more permits than cores; one document never more than the pool
                assert!(p.pool_threads <= cores, "cores {cores} mem {mem:?}: {p:?}");
                assert_eq!(p.threads_per_worker, p.pool_threads);
                if let Some(mb) = mem
                    && p.pool_threads > 1
                {
                    let budget = (mb * RAM_SHARE_PERCENT / 100) as i64;
                    assert!(p.pool_threads * OCR_THREAD_MB as i64 <= budget, "{p:?}");
                }
                // the written cap is the pool total
                if let Some((_, v)) = p.env_override() {
                    assert_eq!(v, memory_threads_cap(1, mem.unwrap_or(0)).to_string());
                }
            }
        }
    }

    #[test]
    fn cap_by_memory() {
        // 12 cores, automatic tasks: 4 GB free -> 4 tasks, pool of all 12 cores
        let p = plan_ocr_resources(None, None, 12, Some(4096));
        assert_eq!((p.task_workers, p.pool_threads, p.threads_per_worker, p.threads_source), (4, 12, 12, ThreadsSource::Default));
        assert_eq!(p.env_override(), None);
        // 1.5 GB free -> 768 MB budget -> 3 tasks; pool 768 / 100 = 7 instead of 12
        let p = plan_ocr_resources(None, None, 12, Some(1536));
        assert_eq!((p.task_workers, p.pool_threads, p.threads_source), (3, 7, ThreadsSource::MemoryCapped));
        assert_eq!(p.env_override(), Some(("PAPERLESS_THREADS_PER_WORKER", "7".to_string())));
        // 1 worker set (upstream setup): same pool
        let p = plan_ocr_resources(Some(1), None, 12, Some(1536));
        assert_eq!((p.task_workers, p.threads_per_worker, p.threads_source), (1, 7, ThreadsSource::MemoryCapped));
        assert_eq!(p.env_override(), Some(("PAPERLESS_THREADS_PER_WORKER", "7".to_string())));
        // 3 workers set, 2 GB free -> pool 10 (shared), not 1024 / 300 = 3 each
        let p = plan_ocr_resources(Some(3), None, 12, Some(2048));
        assert_eq!((p.task_workers, p.pool_threads, p.threads_source), (3, 10, ThreadsSource::MemoryCapped));
        // almost no memory -> at least 1
        let p = plan_ocr_resources(Some(4), None, 12, Some(50));
        assert_eq!((p.pool_threads, p.threads_per_worker), (1, 1));
        assert_eq!(memory_threads_cap(1, 0), 1);
    }

    #[test]
    fn pool_threads_values() {
        assert_eq!(pool_threads(12, None, false), 12);
        assert_eq!(pool_threads(3, Some(8000), false), 3);
        assert_eq!(pool_threads(12, Some(1024), false), 5);
        assert_eq!(pool_threads(12, Some(0), false), 1);
        // explicit threads: no memory cap, still never more than cores
        assert_eq!(pool_threads(12, Some(1024), true), 12);
        assert_eq!(pool_threads(0, None, false), 1);
        assert_eq!(ocr_threads_per_document(Some(3)), 3);
        assert_eq!(ocr_threads_per_document(Some(0)), 1);
        let (cores, mem) = machine();
        assert_eq!(ocr_threads_per_document(None), pool_threads(cores, mem, false));
    }

    #[test]
    fn explicit_values_stay() {
        let p = plan_ocr_resources(Some(2), Some(6), 12, Some(100));
        assert_eq!((p.task_workers, p.threads_per_worker, p.pool_threads, p.threads_source), (2, 6, 12, ThreadsSource::Explicit));
        assert_eq!(p.env_override(), None);
        assert!(p.describe().contains("per document up to 6, set explicitly"), "{}", p.describe());
        // an explicit task_workers is never changed automatically
        let p = plan_ocr_resources(Some(8), None, 12, Some(100));
        assert_eq!(p.task_workers, 8);
        // memory unknown -> automatic tasks, pool of all cores
        let p = plan_ocr_resources(None, None, 12, None);
        assert_eq!((p.task_workers, p.threads_per_worker, p.threads_source), (4, 12, ThreadsSource::DefaultUnknownMemory));
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
        assert_eq!((p.task_workers, p.workers_source, p.pool_threads), (2, WorkersSource::Explicit, 5));
        // `1` keeps one task at a time (upstream behaviour), all cores for it
        let p = plan_from_env(&Env::from_pairs([("PAPERRUST_TASK_WORKERS", "1")]), 3, Some(8000));
        assert_eq!((p.task_workers, p.workers_source, p.threads_per_worker), (1, WorkersSource::Explicit, 3));
        // invalid value: do not limit anything (the configuration reports the error itself)
        let env = Env::from_pairs([("PAPERLESS_THREADS_PER_WORKER", "x")]);
        assert_eq!(plan_from_env(&env, 12, Some(100)).env_override(), None);
        let env = Env::from_pairs([("PAPERLESS_TASK_WORKERS", "x")]);
        assert_eq!(plan_from_env(&env, 12, Some(100_000)).workers_source, WorkersSource::Explicit);
        // unset: automatic (PaperRust default); 800 MB free -> 400 MB budget -> 2 tasks, pool of
        // 4 threads instead of 12
        let p = plan_from_env(&Env::default(), 12, Some(800));
        assert_eq!(p.workers_source, WorkersSource::Default);
        assert_eq!((p.task_workers, p.pool_threads, p.threads_per_worker), (2, 4, 4));
        let d = p.describe();
        assert!(
            d.starts_with(
                "task workers: 2 (default: auto from cores and memory); OCR: shared page pool of 4 threads (per document up to 4;"
            ),
            "{d}"
        );
        assert!(d.contains("capped from 12 cores"), "{d}");
        // NAS / Pi 5: 3 cores, plenty of memory -> 2 tasks sharing a pool of 3
        let p = plan_from_env(&Env::default(), 3, Some(8000));
        assert_eq!((p.task_workers, p.pool_threads, p.threads_per_worker, p.threads_source), (2, 3, 3, ThreadsSource::Default));
        assert_eq!(
            p.describe(),
            "task workers: 2 (default: auto from cores and memory); OCR: shared page pool of 3 threads (per document up to 3); cores: 3, available memory: 8000 MB"
        );
        // explicit `auto` (any case, also via PAPERRUST_): same numbers, other label
        for v in ["auto", "AUTO", " Auto "] {
            let p = plan_from_env(&Env::from_pairs([("PAPERRUST_TASK_WORKERS", v)]), 12, Some(800));
            assert_eq!(p.workers_source, WorkersSource::Auto);
            assert_eq!((p.task_workers, p.pool_threads), (2, 4));
            assert!(p.describe().contains("(auto: from cores and memory)"));
        }
    }

    #[test]
    fn task_workers_from_env_values() {
        let (cores, mem) = machine();
        // unset = auto
        assert_eq!(task_workers_from_env(&Env::default()).unwrap(), default_task_workers(None, cores, mem));
        assert_eq!(task_workers_from_env(&Env::from_pairs([("PAPERRUST_TASK_WORKERS", "1")])).unwrap(), 1);
        assert_eq!(task_workers_from_env(&Env::from_pairs([("PAPERRUST_TASK_WORKERS", "3")])).unwrap(), 3);
        assert_eq!(
            task_workers_from_env(&Env::from_pairs([("PAPERRUST_TASK_WORKERS", "Auto")])).unwrap(),
            default_task_workers(None, cores, mem)
        );
        assert_eq!(
            task_workers_from_env(&Env::from_pairs([("PAPERLESS_TASK_WORKERS", "auto"), ("PAPERLESS_THREADS_PER_WORKER", "1")])).unwrap(),
            default_task_workers(Some(1), cores, mem)
        );
        assert_eq!(
            task_workers_from_env(&Env::from_pairs([("PAPERLESS_THREADS_PER_WORKER", "1")])).unwrap(),
            default_task_workers(Some(1), cores, mem)
        );
        let e = task_workers_from_env(&Env::from_pairs([("PAPERLESS_TASK_WORKERS", "autox")])).unwrap_err().to_string();
        assert!(e.contains("invalid literal for int() with base 10: 'autox'"), "{e}");
        assert!(task_workers_from_env(&Env::from_pairs([("PAPERLESS_THREADS_PER_WORKER", "x")])).is_err());
        assert!(is_auto("auto") && is_auto(" AUTO\n") && !is_auto("") && !is_auto("automatic"));
    }

    #[test]
    fn meminfo_and_cgroup() {
        let t = "MemTotal:       16000000 kB\nMemFree:  1000 kB\nMemAvailable:    2048000 kB\n";
        assert_eq!(parse_meminfo_available(t), Some(2_048_000 * 1024));
        assert_eq!(parse_meminfo_available("MemTotal: 1 kB"), None);
        assert_eq!(parse_meminfo_available("MemAvailable: x kB"), None);
        assert_eq!(parse_meminfo_available("MemAvailable: 99999999999999999999 kB"), None);
        // limits
        assert_eq!(parse_cgroup_limit("max\n"), None);
        assert_eq!(parse_cgroup_limit("3221225472\n"), Some(3 << 30));
        assert_eq!(parse_cgroup_limit("9223372036854771712\n"), None, "cgroup v1 unlimited");
        assert_eq!(parse_cgroup_limit("18446744073709551615"), None);
        assert_eq!(parse_cgroup_limit("-1"), None);
        assert_eq!(parse_cgroup_limit(""), None);
        // v2: memory.max / memory.current
        assert_eq!(cgroup_available(Some("max\n"), Some("100"), None), None);
        assert_eq!(cgroup_available(Some("1073741824\n"), Some("73741824\n"), None), Some(1_000_000_000));
        assert_eq!(cgroup_available(None, None, None), None);
        assert_eq!(cgroup_available(Some("1000"), None, None), Some(1000));
        assert_eq!(cgroup_available(Some("1000"), Some("garbage"), None), Some(1000));
        // usage above the limit (page cache) never underflows; inactive file cache is reclaimable
        assert_eq!(cgroup_available(Some("1000"), Some("1500"), None), Some(0));
        assert_eq!(cgroup_available(Some("1000"), Some("900"), Some(600)), Some(700));
        assert_eq!(cgroup_available(Some("1000"), Some("900"), Some(5000)), Some(1000));
        // v1: memory.limit_in_bytes unlimited
        assert_eq!(cgroup_available(Some("9223372036854771712"), Some("5000"), None), None);
        // memory.stat
        let stat = "anon 100\nfile 900\ninactive_file 600\nactive_file 300\n";
        assert_eq!(parse_memory_stat(stat, "inactive_file"), Some(600));
        assert_eq!(parse_memory_stat(stat, "total_inactive_file"), None);
        assert_eq!(parse_memory_stat("cache 1\ntotal_inactive_file 42\n", "total_inactive_file"), Some(42));
        assert_eq!(parse_memory_stat("inactive_file x\n", "inactive_file"), None);
    }

    #[test]
    fn proc_self_cgroup_paths() {
        let v2 = "0::/system.slice/paperrust.service\n";
        assert_eq!(parse_proc_self_cgroup(v2, CgroupVersion::V2), Some("/system.slice/paperrust.service"));
        assert_eq!(parse_proc_self_cgroup(v2, CgroupVersion::V1), None);
        assert_eq!(parse_proc_self_cgroup("0::/\n", CgroupVersion::V2), Some("/"));
        let v1 = "12:pids:/docker/abc\n4:memory:/docker/abc\n3:cpu,cpuacct:/docker/abc\n1:name=systemd:/docker/abc\n";
        assert_eq!(parse_proc_self_cgroup(v1, CgroupVersion::V1), Some("/docker/abc"));
        assert_eq!(parse_proc_self_cgroup(v1, CgroupVersion::V2), None);
        assert_eq!(parse_proc_self_cgroup("5:blkio,memory:/x\n", CgroupVersion::V1), Some("/x"));
        assert_eq!(parse_proc_self_cgroup("garbage\n\n", CgroupVersion::V2), None);
        assert_eq!(parse_proc_self_cgroup("", CgroupVersion::V1), None);
        assert_eq!(cgroup_ancestors("/a/b"), vec!["/a/b", "/a", "/"]);
        assert_eq!(cgroup_ancestors("/a/b/"), vec!["/a/b", "/a", "/"]);
        assert_eq!(cgroup_ancestors("/"), vec!["/"]);
        assert_eq!(cgroup_ancestors(""), vec!["/"]);
    }

    #[test]
    fn cgroup_memory_from_files() {
        use std::collections::HashMap;
        let fs = |pairs: &[(&str, &str)]| {
            let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            move |p: &str| m.get(p).cloned()
        };
        // no cgroup files at all (other OS, sandbox): no limit
        assert_eq!(cgroup_memory_available(&fs(&[])), None);
        // Docker, cgroup v2 with its own namespace: limit at the mount root
        let docker = fs(&[
            ("/proc/self/cgroup", "0::/\n"),
            ("/sys/fs/cgroup/memory.max", "3221225472\n"),
            ("/sys/fs/cgroup/memory.current", "1073741824\n"),
            ("/sys/fs/cgroup/memory.stat", "anon 1\ninactive_file 536870912\n"),
        ]);
        assert_eq!(cgroup_memory_available(&docker), Some((3 << 30) - (1 << 30) + (1 << 29)));
        // v2 without a limit
        let host = fs(&[("/proc/self/cgroup", "0::/user.slice\n"), ("/sys/fs/cgroup/user.slice/memory.max", "max\n")]);
        assert_eq!(cgroup_memory_available(&host), None);
        // systemd service with MemoryMax= on the slice above it: the tighter ancestor wins
        let svc = fs(&[
            ("/proc/self/cgroup", "0::/paperrust.slice/paperrust.service\n"),
            ("/sys/fs/cgroup/paperrust.slice/paperrust.service/memory.max", "max\n"),
            ("/sys/fs/cgroup/paperrust.slice/paperrust.service/memory.current", "100\n"),
            ("/sys/fs/cgroup/paperrust.slice/memory.max", "1000\n"),
            ("/sys/fs/cgroup/paperrust.slice/memory.current", "400\n"),
        ]);
        assert_eq!(cgroup_memory_available(&svc), Some(600));
        // Docker, cgroup v1 (host path in /proc/self/cgroup, own cgroup mounted at the root)
        let v1 = fs(&[
            ("/proc/self/cgroup", "4:memory:/docker/abc\n0::/\n"),
            ("/sys/fs/cgroup/memory/memory.limit_in_bytes", "2147483648\n"),
            ("/sys/fs/cgroup/memory/memory.usage_in_bytes", "147483648\n"),
            ("/sys/fs/cgroup/memory/memory.stat", "cache 5\ntotal_inactive_file 0\n"),
        ]);
        assert_eq!(cgroup_memory_available(&v1), Some(2_000_000_000));
        // v1 host: unlimited root
        let v1host = fs(&[
            ("/proc/self/cgroup", "4:memory:/user.slice\n"),
            ("/sys/fs/cgroup/memory/memory.limit_in_bytes", "9223372036854771712\n"),
        ]);
        assert_eq!(cgroup_memory_available(&v1host), None);
    }

    #[test]
    fn reads_memory_here() {
        if cfg!(any(windows, target_os = "linux")) {
            assert!(available_memory_mb().is_some_and(|m| m > 0));
        }
    }
}
