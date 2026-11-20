//! Regularly return free heap to the operating system (base memory after ingestion).
//!
//! Measurement 2026-11-18 (docs/contract/pl-server.md "Grundspeicher"): after ingestion, preview or
//! metadata, free memory remains in the process although nothing is running anymore:
//!
//! - **mimalloc** (Rust heap) returns free pages only with a delay, namely only at the next
//!   allocator call of the same thread or the arena. The worker threads of `spawn_blocking`
//!   end after 10 s of idle time; their free segments then wait for an arena cleanup that
//!   never comes in a quiet process. `mi_collect(true)` performs the due returns.
//! - **glibc** (`malloc` of the C libraries PDFium/Tesseract/Leptonica): large image buffers end up
//!   in the arenas after the first free of a large block (dynamic mmap threshold) and
//!   are only returned from the top; `malloc_trim(0)` returns free pages of all arenas.
//!
//! A dedicated, almost always sleeping thread does both every [`EVERY`] (fixed, no setting:
//! there is nothing to weigh). Cost: a few milliseconds per round on this thread;
//! only pages that are already free are returned (under load the allocator would have freed them
//! after its deadline as well) – no measurable effect on speed.

use std::sync::OnceLock;
use std::time::Duration;

/// Interval of the return.
pub const EVERY: Duration = Duration::from_secs(30);

/// One round: due returns of the Rust allocator and (Linux/glibc) of the C runtime.
pub fn trim_now() {
    #[cfg(feature = "mimalloc")]
    {
        unsafe extern "C" {
            fn mi_collect(force: bool);
        }
        // SAFETY: mimalloc function without preconditions, thread-safe; the static library
        // is linked via `mimalloc`/`libmimalloc-sys` (global allocator of this program).
        unsafe { mi_collect(true) };
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: glibc function without preconditions, thread-safe.
        unsafe { malloc_trim(0) };
    }
}

/// Starts the return thread once per process.
pub fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let r = std::thread::Builder::new().name("memory-trim".into()).spawn(|| {
            loop {
                std::thread::sleep(EVERY);
                trim_now();
            }
        });
        if let Err(e) = r {
            tracing::warn!("could not start the memory trim thread: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_runs() {
        let v: Vec<u8> = vec![1; 4 << 20];
        drop(v);
        trim_now();
    }
}
