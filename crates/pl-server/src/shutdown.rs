//! Orderly stop: the future returned by [`signal`] resolves on the first of
//!
//! * Ctrl+C (all platforms),
//! * Unix `SIGTERM` (docker stop, systemd, s6),
//! * Windows `CTRL_BREAK_EVENT` (sent by a supervisor via `GenerateConsoleCtrlEvent` to a child in
//!   its own process group), `CTRL_CLOSE_EVENT` (console window closed; Windows allows ~5 s),
//!   `CTRL_LOGOFF_EVENT`, `CTRL_SHUTDOWN_EVENT`,
//! * a restart request (`pl_settings::restart::request`, exit code 75 – docs/SETTINGS.md E9),
//! * stdin EOF when `PAPERRUST_STOP_ON_STDIN_EOF=1` (the supervisor holds the write end of a
//!   pipe; it closes it – or dies – and the server stops). stdin is read on a plain thread so a
//!   pending read never blocks runtime shutdown.
//!
//! Afterwards the HTTP server stops accepting, gives open connections 10 s
//! ([`crate::serve::serve`]) and the worker/beat/consumer get their stop signal
//! (`worker::start_background`).

use std::future::pending;

/// Resolves once a stop was requested; returns what triggered it (for the log).
pub async fn signal(stop_on_stdin_eof: bool) -> &'static str {
    let stdin = async {
        if stop_on_stdin_eof {
            match stdin_eof() {
                Some(rx) => {
                    let _ = rx.await;
                    "stdin EOF"
                }
                None => pending().await,
            }
        } else {
            pending().await
        }
    };
    let why = tokio::select! {
        w = ctrl_c() => w,
        w = platform() => w,
        w = stdin => w,
        // Restart from the GUI (docs/SETTINGS.md E9): stop in an orderly way, exit code 75 (cli::main).
        _ = pl_settings::restart::requested_wait() => "restart requested",
    };
    tracing::info!(reason = why, "shutting down");
    why
}

async fn ctrl_c() -> &'static str {
    if tokio::signal::ctrl_c().await.is_err() {
        // No signal handler available: run until killed (or another trigger fires).
        pending::<()>().await;
    }
    "ctrl-c"
}

#[cfg(unix)]
async fn platform() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut s) => {
            s.recv().await;
            "SIGTERM"
        }
        Err(e) => {
            tracing::warn!(error = %e, "cannot install SIGTERM handler");
            pending().await
        }
    }
}

#[cfg(windows)]
async fn platform() -> &'static str {
    use tokio::signal::windows;
    macro_rules! wait {
        ($ctor:expr, $name:literal) => {
            async {
                match $ctor {
                    Ok(mut s) => {
                        s.recv().await;
                        $name
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, event = $name, "cannot install console handler");
                        pending().await
                    }
                }
            }
        };
    }
    tokio::select! {
        w = wait!(windows::ctrl_break(), "ctrl-break") => w,
        w = wait!(windows::ctrl_close(), "ctrl-close") => w,
        w = wait!(windows::ctrl_logoff(), "ctrl-logoff") => w,
        w = wait!(windows::ctrl_shutdown(), "ctrl-shutdown") => w,
    }
}

#[cfg(not(any(unix, windows)))]
async fn platform() -> &'static str {
    pending().await
}

/// A thread reads (and discards) stdin; the receiver completes at EOF or on a read error
/// (a closed pipe reads as EOF on all platforms; a missing stdin as well).
fn stdin_eof() -> Option<tokio::sync::oneshot::Receiver<()>> {
    use std::io::Read;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new().name("stdin-eof".into()).spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 512];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    tracing::warn!(error = %e, "reading stdin failed; treating as EOF (PAPERRUST_STOP_ON_STDIN_EOF)");
                    break;
                }
            }
        }
        let _ = tx.send(());
    });
    match spawned {
        Ok(_) => Some(rx),
        Err(e) => {
            tracing::warn!(error = %e, "cannot start stdin watcher; PAPERRUST_STOP_ON_STDIN_EOF has no effect");
            None
        }
    }
}
