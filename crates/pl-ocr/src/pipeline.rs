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
//!
//! PaperRust: with a [`PagePool`] ([`ordered_shared`]) each page task additionally holds one
//! process-wide permit, so the cores are shared adaptively between concurrent documents (a lone
//! document uses all of them). Memory with the pool: at most `jobs` rasters per document (the
//! page being produced holds a permit too), and at most the pool size across all documents.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};

use crate::Result;
use crate::permits::{DocShare, PagePermit, PagePool};

/// Processes `0..n`: `produce(i)` serially (caller thread), `None` = nothing to do; `work(i, t)`
/// in parallel with `jobs` workers. Result ordered by index (`None` = skipped).
///
/// `jobs <= 1` is exactly the previous serial loop (`produce(i)`, then `work(i)`, then `i+1`).
/// No permit pool: every document gets its fixed `jobs` (tests, tools); see [`ordered_shared`].
pub fn ordered<T, R, P, W>(jobs: usize, n: usize, produce: P, work: W) -> Result<Vec<Option<R>>>
where
    T: Send,
    R: Send,
    P: FnMut(usize) -> Result<Option<T>>,
    W: Fn(usize, T) -> Result<R> + Sync,
{
    ordered_shared(None, jobs, n, |_| true, produce, work)
}

/// Like [`ordered`], but with a [`PagePool`]: every page task (`produce(i)` + `work(i, t)`) runs
/// under one permit of the pool, so concurrent documents share the cores adaptively (see
/// [`crate::permits`]); `jobs` is this document's upper bound of permits held at the same time.
///
/// The permit for page `i` is acquired on the caller thread **before** `produce(i)` (rasterizing
/// counts as page work) and travels with the page to the worker, which drops it when `work(i, t)`
/// is done. The caller thread never holds a permit while waiting for another one, and workers
/// never wait for permits (lock order, see `permits`). Invariant: the document holds at most
/// `jobs` permits and there are `min(jobs, n)` workers, so while the caller holds the permit of
/// the page it produces, at least one worker is free and the handover cannot block on a busy
/// worker.
///
/// `wants(i)`: page `i` needs work (`false`: `produce(i)` is expected to return `None` quickly, so
/// no permit is taken for it – text pages of a mixed PDF do not wait for a busy pool). If
/// `produce(i)` returns `Some` anyway, a permit is acquired after producing.
///
/// `pool = None` is exactly the previous behaviour (fixed `jobs` workers, no permits).
pub fn ordered_shared<T, R, Q, P, W>(
    pool: Option<&PagePool>,
    jobs: usize,
    n: usize,
    wants: Q,
    mut produce: P,
    work: W,
) -> Result<Vec<Option<R>>>
where
    T: Send,
    R: Send,
    Q: Fn(usize) -> bool,
    P: FnMut(usize) -> Result<Option<T>>,
    W: Fn(usize, T) -> Result<R> + Sync,
{
    let share = pool.map(|p| p.share(jobs.max(1)));
    let share = share.as_ref();
    let mut out: Vec<Option<R>> = Vec::with_capacity(n);
    if jobs <= 1 || n <= 1 {
        for i in 0..n {
            let permit = share.filter(|_| wants(i)).map(DocShare::acquire);
            out.push(match produce(i)? {
                Some(t) => {
                    let _permit = permit.or_else(|| share.map(DocShare::acquire));
                    Some(work(i, t)?)
                }
                None => None,
            });
        }
        return Ok(out);
    }
    out.resize_with(n, || None);
    let workers = jobs.min(n);
    let abort = AtomicBool::new(false);
    let (tx, rx) = mpsc::sync_channel::<(usize, T, Option<PagePermit<'_>>)>(0);
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
                    let Ok((i, t, permit)) = next else { break };
                    let r = work(i, t);
                    // Page done: release its permit before reporting (another document may wait).
                    drop(permit);
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
            let permit = share.filter(|_| wants(i)).map(DocShare::acquire);
            if abort.load(Ordering::SeqCst) {
                break;
            }
            match produce(i) {
                Ok(Some(t)) => {
                    let permit = permit.or_else(|| share.map(DocShare::acquire));
                    if tx.send((i, t, permit)).is_err() {
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
            let r = ordered(
                jobs,
                20,
                |i| Ok((i % 5 != 3).then_some(i * 10)),
                |i, t| {
                    std::thread::sleep(delay(i));
                    Ok(format!("{i}:{t}"))
                },
            )
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

    /// One document through the shared pipeline: every page task (produce + work) counts as busy
    /// in `busy`/`peak`; returns the results and when the first page task started.
    fn shared_doc(
        pool: &PagePool,
        jobs: usize,
        n: usize,
        ms: u64,
        busy: &AtomicUsize,
        peak: &AtomicUsize,
    ) -> (Vec<Option<String>>, Instant) {
        let first = Mutex::new(None::<Instant>);
        let r = ordered_shared(
            Some(pool),
            jobs,
            n,
            |_| true,
            |i| {
                first.lock().unwrap().get_or_insert_with(Instant::now);
                let b = busy.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(b, Ordering::SeqCst);
                Ok(Some(i))
            },
            |i, t| {
                std::thread::sleep(Duration::from_millis(ms));
                busy.fetch_sub(1, Ordering::SeqCst);
                Ok(format!("{i}:{t}"))
            },
        )
        .unwrap();
        let started = first.lock().unwrap().unwrap_or_else(Instant::now);
        (r, started)
    }

    #[test]
    fn shared_lone_document_uses_all_permits() {
        let pool = PagePool::new(3);
        let (busy, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let t = Instant::now();
        let (r, _) = shared_doc(&pool, 8, 12, 40, &busy, &peak);
        let want: Vec<Option<String>> = (0..12).map(|i| Some(format!("{i}:{i}"))).collect();
        assert_eq!(r, want);
        assert_eq!(peak.load(Ordering::SeqCst), 3, "lone document must use the whole pool");
        // 12 × 40 ms / 3 = 160 ms ideal; serial would be 480 ms
        assert!(t.elapsed() < Duration::from_millis(400), "{:?}", t.elapsed());
        assert_eq!(pool.busy(), 0);
    }

    #[test]
    fn shared_documents_never_exceed_pool_and_late_one_starts_within_a_page() {
        for size in [1usize, 2, 3] {
            let pool = PagePool::new(size);
            let (busy, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
            let page_ms = 60;
            std::thread::scope(|s| {
                let (pool, busy, peak) = (&pool, &busy, &peak);
                let a = s.spawn(move || shared_doc(pool, 8, 10, page_ms, busy, peak));
                // B arrives while A holds the whole pool.
                std::thread::sleep(Duration::from_millis(page_ms / 2));
                let arrive = Instant::now();
                let b = s.spawn(move || shared_doc(pool, 8, 4, page_ms, busy, peak));
                let (ra, _) = a.join().unwrap();
                let (rb, b_started) = b.join().unwrap();
                assert_eq!(ra, (0..10).map(|i| Some(format!("{i}:{i}"))).collect::<Vec<_>>());
                assert_eq!(rb, (0..4).map(|i| Some(format!("{i}:{i}"))).collect::<Vec<_>>());
                // within about one page time (+ scheduling slack), not after A has finished
                let wait = b_started.saturating_duration_since(arrive);
                assert!(wait < Duration::from_millis(page_ms + 100), "size {size}: B waited {wait:?}");
            });
            assert!(peak.load(Ordering::SeqCst) <= size, "size {size}: peak {}", peak.load(Ordering::SeqCst));
            assert_eq!(pool.busy(), 0);
        }
    }

    #[test]
    fn shared_skipped_pages_need_no_permit() {
        // Pool exhausted by someone else: a document whose pages all need no work still finishes.
        let pool = PagePool::new(1);
        let other = pool.share(1);
        let held = other.acquire();
        let produced = AtomicUsize::new(0);
        for jobs in [1, 4] {
            let r = ordered_shared(
                Some(&pool),
                jobs,
                5,
                |_| false,
                |_| {
                    produced.fetch_add(1, Ordering::SeqCst);
                    Ok(None::<usize>)
                },
                |i, _| Ok(i),
            )
            .unwrap();
            assert!(r.iter().all(Option::is_none));
        }
        assert_eq!(produced.load(Ordering::SeqCst), 10);
        drop(held);
    }

    #[test]
    fn shared_results_and_errors_match_unshared() {
        let pool = PagePool::new(2);
        for jobs in [1, 3, 8] {
            let r = ordered_shared(
                Some(&pool),
                jobs,
                20,
                |i| i % 5 != 3,
                |i| Ok((i % 5 != 3).then_some(i * 10)),
                |i, t| {
                    std::thread::sleep(delay(i));
                    Ok(format!("{i}:{t}"))
                },
            )
            .unwrap();
            let want: Vec<Option<String>> = (0..20).map(|i| (i % 5 != 3).then(|| format!("{i}:{}", i * 10))).collect();
            assert_eq!(r, want, "jobs={jobs}");
            let e = ordered_shared(
                Some(&pool),
                jobs,
                12,
                |_| true,
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
            assert_eq!(pool.busy(), 0, "permits leaked after error (jobs={jobs})");
        }
    }

    #[test]
    fn jobs_from_args() {
        assert_eq!(effective_jobs(Some(3)), 3);
        assert_eq!(effective_jobs(Some(0)), 1);
        assert_eq!(effective_jobs(Some(-2)), 1);
        assert!(effective_jobs(None) >= 1);
    }
}
