//! Child processes die with the app (docs/DESKTOP.md section 14 no. 7).
//!
//! **Windows:** At startup the supervisor process joins its own job object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. All children started afterwards (server, `manage`,
//! `pg_ctl` and the `postgres` started by `pg_ctl`, WebView2) inherit the membership. If
//! the app dies – even hard via Task Manager – Windows closes the last handle of the job and terminates
//! all members. PostgreSQL is then terminated as with `-m immediate`; the next start does
//! crash recovery from the WAL (committed transactions stay). On normal exit the
//! supervisor has already stopped in an orderly way beforehand – the job then does nothing.
//!
//! On Windows `pg_ctl` itself creates a job object for the postmaster, but **only** if the
//! new process is not yet in any job (`pg_ctl.c`, `CreateRestrictedProcess`); inside our job
//! it nests nothing and does not break away either (no `CREATE_BREAKAWAY_FROM_JOB`).
//!
//! `JOB_OBJECT_LIMIT_BREAKAWAY_OK` is set so that programs that are **not** supposed to end with the app
//! (browser, Explorer for "open logs") can be started with `CREATE_BREAKAWAY_FROM_JOB`
//! ([`open_external`]).
//!
//! **Unix:** no counterpart needed: the server ends via stdin EOF, PostgreSQL is recognized as an orphan
//! at the next start and stopped (`Supervisor::start_database`).
//!
//! Can be disabled with `PAPERRUST_DESKTOP_NO_JOB=1` (debugging).

/// Result of [`kill_children_with_app`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Process is now in a job with "kill on close".
    Bound,
    /// Disabled (`PAPERRUST_DESKTOP_NO_JOB`) or not Windows.
    Skipped,
}

/// Binds all future child processes to the lifetime of this process (Windows).
/// Call once early in `main`, before the first child process.
pub fn kill_children_with_app() -> std::io::Result<JobState> {
    if std::env::var_os("PAPERRUST_DESKTOP_NO_JOB").is_some_and(|v| !v.is_empty() && v != "0") {
        return Ok(JobState::Skipped);
    }
    #[cfg(windows)]
    {
        win::bind_current_process()?;
        Ok(JobState::Bound)
    }
    #[cfg(not(windows))]
    {
        Ok(JobState::Skipped)
    }
}

/// Open a URL or folder with the default program (without shell interpretation). On Windows
/// outside the job object, so a freshly started browser/Explorer survives the app.
pub fn open_external(target: &str) {
    #[cfg(windows)]
    let r = {
        use std::os::windows::process::CommandExt;
        let cmd = || {
            let mut c = std::process::Command::new("rundll32");
            c.args(["url.dll,FileProtocolHandler", target]);
            c
        };
        // Job without BREAKAWAY_OK (app itself started in a foreign job): start normally.
        cmd()
            .creation_flags(win::CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .or_else(|_| cmd().spawn())
    };
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(target).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(target).spawn();
    if let Err(e) = r {
        // Never log the query: the auto-login URL carries the desktop login key (found 2026-11-20 on Linux
        // without xdg-open, the key ended up in desktop.log).
        tracing::warn!(error = %e, "{} ließ sich nicht öffnen", redact_query(target));
    }
}

/// `target` without query string and fragment (for logs).
fn redact_query(target: &str) -> &str {
    target.split(['?', '#']).next().unwrap_or(target)
}

#[cfg(test)]
mod redact_tests {
    #[test]
    fn query_is_stripped() {
        assert_eq!(super::redact_query("http://127.0.0.1:58080/rs/desktop/login?token=abc&next=%2F"), "http://127.0.0.1:58080/rs/desktop/login");
        assert_eq!(super::redact_query("/home/x/folder"), "/home/x/folder");
    }
}

#[cfg(windows)]
pub(crate) mod win {
    //! Minimal FFI to kernel32 (no new dependency; windows-sys in the workspace lacks the
    //! feature `Win32_System_JobObjects`).
    use std::ffi::c_void;
    use std::io;

    pub type Handle = *mut c_void;

    pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const JOB_OBJECT_LIMIT_BREAKAWAY_OK: u32 = 0x0000_0800;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;

    #[repr(C)]
    #[derive(Default)]
    pub struct BasicLimitInformation {
        pub per_process_user_time_limit: i64,
        pub per_job_user_time_limit: i64,
        pub limit_flags: u32,
        pub minimum_working_set_size: usize,
        pub maximum_working_set_size: usize,
        pub active_process_limit: u32,
        pub affinity: usize,
        pub priority_class: u32,
        pub scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct IoCounters {
        pub read_operation_count: u64,
        pub write_operation_count: u64,
        pub other_operation_count: u64,
        pub read_transfer_count: u64,
        pub write_transfer_count: u64,
        pub other_transfer_count: u64,
    }

    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`
    #[repr(C)]
    #[derive(Default)]
    pub struct ExtendedLimitInformation {
        pub basic: BasicLimitInformation,
        pub io: IoCounters,
        pub process_memory_limit: usize,
        pub job_memory_limit: usize,
        pub peak_process_memory_used: usize,
        pub peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(h: Handle) -> i32;
    }

    /// New anonymous job object with "kill on close" + "breakaway ok".
    pub fn create_kill_on_close_job() -> io::Result<Handle> {
        // SAFETY: calls according to the Win32 contract; `info` lives until after the call, size is correct.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut info = ExtendedLimitInformation::default();
            info.basic.limit_flags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            let ok = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                (&raw const info).cast(),
                u32::try_from(std::mem::size_of::<ExtendedLimitInformation>()).unwrap_or(0),
            );
            if ok == 0 {
                let e = io::Error::last_os_error();
                CloseHandle(job);
                return Err(e);
            }
            Ok(job)
        }
    }

    /// Assign a process (`process` = handle with PROCESS_SET_QUOTA|PROCESS_TERMINATE) to the job.
    pub fn assign(job: Handle, process: Handle) -> io::Result<()> {
        // SAFETY: valid handles per the caller's contract.
        if unsafe { AssignProcessToJobObject(job, process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn close(job: Handle) {
        // SAFETY: handle comes from `create_kill_on_close_job` and is closed exactly once.
        unsafe {
            CloseHandle(job);
        }
    }

    /// Current process into a new job; the handle deliberately stays open until the process ends
    /// (then Windows closes it and terminates all children).
    pub fn bind_current_process() -> io::Result<()> {
        let job = create_kill_on_close_job()?;
        // SAFETY: pseudo-handle of the own process, does not need to be closed.
        let me = unsafe { GetCurrentProcess() };
        if let Err(e) = assign(job, me) {
            close(job);
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn job_kills_member_when_closed() {
        use std::os::windows::io::AsRawHandle;
        use std::time::{Duration, Instant};
        assert_eq!(
            std::mem::size_of::<super::win::ExtendedLimitInformation>(),
            144
        );
        let job = super::win::create_kill_on_close_job().unwrap();
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "ping -n 60 127.0.0.1 >NUL"])
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        super::win::assign(job, child.as_raw_handle().cast()).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        super::win::close(job);
        let start = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "Kind lebt nach Schließen des Jobs weiter"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
