//! `paperrust-pdftool` – child process for the PdfCraft tools (docs/contract/pl-pdftools.md).
//!
//! Reads a JSON job from stdin, writes a JSON reply to stdout. Runs with a
//! counting allocator (memory limit from the job), the work in its own thread with a
//! large stack and under `catch_unwind`. Whatever still crashes (stack overflow, memory limit,
//! `abort`) only terminates this process.

use std::io::{Read, Write};
use std::process::ExitCode;

use pl_pdftools::job::{Job, Op, Reply, SelfTest};

#[global_allocator]
static ALLOC: pl_pdftools::guard::Guard = pl_pdftools::guard::Guard;

/// Maximum size of the job on stdin.
const MAX_JOB: u64 = 16 * 1024 * 1024;
/// Stack of the worker thread (deep nesting in broken PDFs).
const STACK: usize = 64 * 1024 * 1024;

fn reply(r: &Reply) -> ExitCode {
    let mut out = std::io::stdout().lock();
    match serde_json::to_writer(&mut out, r).and_then(|_| out.flush().map_err(serde_json::Error::io)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(3),
    }
}

#[cfg(debug_assertions)]
fn self_test(mode: SelfTest) -> Reply {
    match mode {
        SelfTest::Panic => panic!("self test: panic"),
        SelfTest::Hang => loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
        },
        SelfTest::Oom => {
            let mut keep: Vec<Vec<u8>> = Vec::new();
            loop {
                keep.push(vec![1u8; 16 * 1024 * 1024]);
            }
        }
        SelfTest::Abort => std::process::abort(),
        SelfTest::StackOverflow => {
            #[allow(unconditional_recursion)]
            fn deep(n: u64) -> u64 {
                let pad = std::hint::black_box([n; 64]);
                deep(pad[3] + 1) + pad[0]
            }
            Reply::Failed { error: deep(0).to_string() }
        }
    }
}

#[cfg(not(debug_assertions))]
fn self_test(_mode: SelfTest) -> Reply {
    Reply::Failed { error: "self tests are only available in debug builds".into() }
}

fn main() -> ExitCode {
    let mut raw = String::new();
    if std::io::stdin().take(MAX_JOB).read_to_string(&mut raw).is_err() {
        return reply(&Reply::Failed { error: "invalid job".into() });
    }
    let job: Job = match serde_json::from_str(&raw) {
        Ok(j) => j,
        Err(_) => return reply(&Reply::Failed { error: "invalid job".into() }),
    };
    drop(raw);
    pl_pdftools::guard::set_limit_mb(job.memory_mb);
    let worker = std::thread::Builder::new().name("pdftool".into()).stack_size(STACK).spawn(move || {
        std::panic::catch_unwind(move || match job.op {
            Op::SelfTest { mode } => self_test(mode),
            op => pl_pdftools::engine::run(op),
        })
    });
    let r = match worker {
        Ok(h) => match h.join() {
            Ok(Ok(r)) => r,
            // Panic in PdfCraft code: report as a domain error, process ends cleanly
            Ok(Err(_)) | Err(_) => Reply::Failed { error: "The PDF could not be processed (internal error in the PDF tool).".into() },
        },
        Err(_) => Reply::Failed { error: "The PDF tool could not start.".into() },
    };
    reply(&r)
}
