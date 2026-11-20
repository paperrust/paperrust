//! Ordered, bounded page pipeline (OCRmyPDF `--jobs`, `use_threads=True`).
//!
//! OCRmyPDF processes pages in parallel with `jobs` workers and assembles the result in
//! page order. Here: `produce` (rasterizing with PDFium – not thread-safe and
//! `!Send`) always runs on the calling thread, `work` (Tesseract subprocess) in up to
//! `jobs` worker threads. The handover channel has capacity 0, i.e. at most `jobs` items are
//! in progress and one is being produced (memory: ≤ `jobs + 1` rasters at the same time).
//!
//! Error semantics like the serial loop: the error with the smallest page index wins
//! (no matter whether from `produce` or `work`); all pages before it are fully processed. After the
//! first error no further pages are produced.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};

use crate::Result;

/// Processes `0..n`: `produce(i)` serially (caller thread), `None` = nothing to do; `work(i, t)`
/// in parallel with `jobs` workers. Result ordered by index (`None` = skipped).
///
/// `jobs <= 1` is exactly the previous serial loop (`produce(i)`, then `work(i)`, then `i+1`).
pub fn ordered<T, R, P, W>(jobs: usize, n: usize, mut produce: P, work: W) -> Result<Vec<Option<R>>>
where
    T: Send,
    R: Send,
    P: FnMut(usize) -> Result<Option<T>>,
    W: Fn(usize, T) -> Result<R> + Sync,
{
    let mut out: Vec<Option<R>> = Vec::with_capacity(n);
    if jobs <= 1 || n <= 1 {
        for i in 0..n {
            out.push(match produce(i)? {
                Some(t) => Some(work(i, t)?),
                None => None,
            });
        }
        return Ok(out);
    }
    out.resize_with(n, || None);
    let workers = jobs.min(n);
    let abort = AtomicBool::new(false);
    let (tx, rx) = mpsc::sync_channel::<(usize, T)>(0);
    let rx = Mutex::new(rx);
    let (rtx, rrx) = mpsc::channel::<(usize, Result<R>)>();
    let mut produce_err: Option<(usize, crate::OcrError)> = None;
    std::thread::scope(|s| {
        for _ in 0..workers {
            let rtx = rtx.clone();
            let (rx, work, abort) = (&rx, &work, &abort);
            s.spawn(move || {
                loop {
                    // Hold the lock only for fetching.
                    let next = match rx.lock() {
                        Ok(g) => g.recv(),
                        Err(_) => break,
                    };
                    let Ok((i, t)) = next else { break };
                    let r = work(i, t);
                    if r.is_err() {
                        abort.store(true, Ordering::SeqCst);
                    }
                    if rtx.send((i, r)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(rtx);
        for i in 0..n {
            if abort.load(Ordering::SeqCst) {
                break;
            }
            match produce(i) {
                Ok(Some(t)) => {
                    if tx.send((i, t)).is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    produce_err = Some((i, e));
                    break;
                }
            }
        }
        drop(tx);
    });
    // All workers have finished (scope); collect results.
    let mut first_err = produce_err;
    for (i, r) in rrx.try_iter() {
        match r {
            Ok(v) => out[i] = Some(v),
            Err(e) => {
                if first_err.as_ref().is_none_or(|(j, _)| i < *j) {
                    first_err = Some((i, e));
                }
            }
        }
    }
    match first_err {
        Some((_, e)) => Err(e),
        None => Ok(out),
    }
}

/// `jobs` from the OCRmyPDF arguments (≥ 1; if the value is missing: number of cores like OCRmyPDF).
pub fn effective_jobs(jobs: Option<i64>) -> usize {
    match jobs {
        Some(j) if j >= 1 => usize::try_from(j).unwrap_or(1),
        Some(_) => 1,
        None => std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OcrError;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    fn delay(i: usize) -> Duration {
        // deterministically "random" latency 5..45 ms
        Duration::from_millis(5 + ((i as u64 * 7919) % 41))
    }

    #[test]
    fn ordered_results_and_skips() {
        for jobs in [1, 2, 3, 8] {
            let r = ordered(jobs, 20, |i| Ok((i % 5 != 3).then_some(i * 10)), |i, t| {
                std::thread::sleep(delay(i));
                Ok(format!("{i}:{t}"))
            })
            .unwrap();
            let want: Vec<Option<String>> = (0..20).map(|i| (i % 5 != 3).then(|| format!("{i}:{}", i * 10))).collect();
            assert_eq!(r, want, "jobs={jobs}");
        }
    }

    #[test]
    fn first_error_in_page_order_wins() {
        // Page 4 (work) and page 9 (produce) fail; page 4 must win, even if page 9
        // fails first in time (page 4 is slow).
        for jobs in [1, 4] {
            let e = ordered(
                jobs,
                12,
                |i| if i == 9 { Err(OcrError::Other("produce 9".into())) } else { Ok(Some(i)) },
                |i, _| {
                    if i == 4 {
                        std::thread::sleep(Duration::from_millis(80));
                        return Err(OcrError::Other("work 4".into()));
                    }
                    Ok(i)
                },
            )
            .unwrap_err();
            assert_eq!(e.message(), "work 4", "jobs={jobs}");
        }
        // Only a produce error: that one is reported.
        let e = ordered(4, 6, |i| if i == 2 { Err(OcrError::Other("p2".into())) } else { Ok(Some(i)) }, |i, _| Ok(i)).unwrap_err();
        assert_eq!(e.message(), "p2");
        // Two work errors: smaller index wins, even if it finishes later.
        let e = ordered(
            4,
            8,
            |i| Ok(Some(i)),
            |i, _| match i {
                1 => {
                    std::thread::sleep(Duration::from_millis(60));
                    Err(OcrError::Other("w1".into()))
                }
                2 => Err(OcrError::Other("w2".into())),
                _ => Ok(i),
            },
        )
        .unwrap_err();
        assert_eq!(e.message(), "w1");
    }

    #[test]
    fn serial_stops_at_first_error_and_produces_nothing_after() {
        let produced = AtomicUsize::new(0);
        let e = ordered(
            1,
            10,
            |i| {
                produced.fetch_add(1, Ordering::SeqCst);
                Ok(Some(i))
            },
            |i, _| if i == 3 { Err(OcrError::Other("x".into())) } else { Ok(()) },
        )
        .unwrap_err();
        assert_eq!(e.message(), "x");
        assert_eq!(produced.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn bounded_in_flight_and_concurrent() {
        let jobs = 4;
        let live = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let start = Instant::now();
        let n = 16;
        ordered(
            jobs,
            n,
            |i| {
                // "raster" lives from creation until the end of work
                let l = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(l, Ordering::SeqCst);
                Ok(Some(i))
            },
            |_, _| {
                std::thread::sleep(Duration::from_millis(50));
                live.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
        let wall = start.elapsed();
        assert!(peak.load(Ordering::SeqCst) <= jobs + 1, "peak {}", peak.load(Ordering::SeqCst));
        // serial would be 800 ms, ideal 200 ms
        assert!(wall < Duration::from_millis(600), "wall {wall:?}");
    }

    #[test]
    fn jobs_from_args() {
        assert_eq!(effective_jobs(Some(3)), 3);
        assert_eq!(effective_jobs(Some(0)), 1);
        assert_eq!(effective_jobs(Some(-2)), 1);
        assert!(effective_jobs(None) >= 1);
    }
}
