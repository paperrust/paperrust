//! Restart from the GUI (docs/SETTINGS.md E9, §2.5 no. 7).
//!
//! [`request`] triggers the orderly shutdown (pl-server `shutdown::signal` waits for
//! [`requested_wait`]); the process then ends with [`EXIT_CODE`] = 75 (`EX_TEMPFAIL`).
//! Docker entrypoint, desktop supervisor and the systemd template (`RestartForceExitStatus=75`)
//! restart it. Supervisors announce themselves with `PAPERRUST_RESTART_SUPERVISED=1`; without this switch
//! the GUI only offers the hint "please restart the service" ([`supported`]).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

/// Exit code "restart requested".
pub const EXIT_CODE: u8 = 75;
/// Set by supervisors that treat exit 75 as a restart.
pub const SUPERVISED_VAR: &str = "PAPERRUST_RESTART_SUPERVISED";

struct Signal {
    flag: AtomicBool,
    notify: Notify,
}

fn global() -> &'static Signal {
    static S: OnceLock<Signal> = OnceLock::new();
    S.get_or_init(|| Signal {
        flag: AtomicBool::new(false),
        notify: Notify::new(),
    })
}

/// Does a supervisor restart the process on exit 75?
pub fn supported(env: &pl_core::config::Env) -> bool {
    env.get(SUPERVISED_VAR)
        .and_then(pl_core::config::str_to_bool)
        .unwrap_or(false)
}

/// Request a restart (ends the process in an orderly way with exit 75).
pub fn request(reason: &str) {
    tracing::info!(reason, "restart requested (exit code {EXIT_CODE})");
    let s = global();
    s.flag.store(true, Ordering::SeqCst);
    s.notify.notify_waiters();
}

/// Was a restart requested?
pub fn requested() -> bool {
    global().flag.load(Ordering::SeqCst)
}

/// Waits until a restart is requested (for the server's shutdown signal).
pub async fn requested_wait() {
    let s = global();
    loop {
        let n = s.notify.notified();
        if s.flag.load(Ordering::SeqCst) {
            return;
        }
        n.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_wakes_waiter() {
        let w = tokio::spawn(requested_wait());
        tokio::task::yield_now().await;
        request("test");
        tokio::time::timeout(std::time::Duration::from_secs(2), w)
            .await
            .unwrap()
            .unwrap();
        assert!(requested());
        // then done immediately
        requested_wait().await;
        assert!(supported(&pl_core::config::Env::from_pairs([(
            SUPERVISED_VAR,
            "1"
        )])));
        assert!(!supported(&pl_core::config::Env::from_pairs([("X", "1")])));
    }
}
