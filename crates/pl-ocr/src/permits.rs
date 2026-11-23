//! Process-wide page-work permits: adaptive sharing of the cores between concurrent documents.
//!
//! PaperRust addition (no upstream counterpart; OCRmyPDF gives every document a fixed `--jobs`).
//! Before, each document ran up to `jobs = PAPERLESS_THREADS_PER_WORKER` page tasks, and that
//! number was fixed per document: with `PAPERLESS_TASK_WORKERS=auto` a document got
//! `cores / tasks` threads even when it was alone (Pi 5, 3 cores: a lone 8-page scan 24 s instead
//! of 14.8 s), and without `auto` concurrent documents could not share at all.
//!
//! Now every page task (rasterizing one page and recognizing it) runs under a **permit** from one
//! process-wide [`PagePool`] (size = cores, lowered to the memory budget, see [`pool_size`]):
//!
//! - a document alone takes as many permits as are free (up to its own cap `jobs`), i.e. all
//!   cores;
//! - several documents share: when a permit is released, it goes to the waiting document that
//!   currently holds the **fewest** permits (ties: who waited first). A newly started document
//!   (0 permits) therefore gets the next released permit – within one page time – and no
//!   document starves (FIFO among equals);
//! - never more than `size` permits are out at the same time (no oversubscription);
//! - permits are released per page, so shares re-balance continuously.
//!
//! **Lock order / deadlock freedom:** the pool has one mutex and one condition variable; nothing
//! else is locked while it is held. A thread waits for a permit only while it holds **no** permit
//! (the page pipeline acquires one permit per page on the producer thread, hands it with the page
//! to a worker, and the worker drops it when the page is done). Holders never wait for other
//! permits, so every held permit is eventually released.
//!
//! Only scheduling changes: which page runs when. Page order, output bytes and text are the same
//! (see `pipeline::ordered_shared`).

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

/// Pool of page-work permits shared by all documents of the process (or a test).
#[derive(Debug)]
pub struct PagePool {
    size: usize,
    state: Mutex<State>,
    cv: Condvar,
}

#[derive(Debug, Default)]
struct State {
    free: usize,
    /// Registered documents: (held permits, per-document cap).
    docs: HashMap<u64, (usize, usize)>,
    /// Waiting acquirers: (document, ticket). Tickets grow monotonically (FIFO among equals).
    waiters: Vec<(u64, u64)>,
    next_doc: u64,
    next_ticket: u64,
    /// Highest number of permits out at the same time (tests/diagnostics).
    peak: usize,
}

impl State {
    fn eligible(&self, doc: u64) -> Option<usize> {
        self.docs.get(&doc).and_then(|&(held, cap)| (held < cap).then_some(held))
    }

    /// May waiter `(doc, ticket)` take a permit now? Only if one is free, its document is below
    /// its cap, and fewer eligible waiters rank before it than permits are free (rank: fewer held
    /// permits first, then lower ticket).
    fn can_take(&self, doc: u64, ticket: u64) -> bool {
        if self.free == 0 {
            return false;
        }
        let Some(mine) = self.eligible(doc) else { return false };
        let before =
            self.waiters.iter().filter(|&&(d, t)| t != ticket && self.eligible(d).is_some_and(|h| (h, t) < (mine, ticket))).count();
        before < self.free
    }
}

impl PagePool {
    /// Pool with `size` permits (at least 1).
    pub fn new(size: usize) -> Self {
        let size = size.max(1);
        Self { size, state: Mutex::new(State { free: size, ..State::default() }), cv: Condvar::new() }
    }

    /// Number of permits.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Permits currently out.
    pub fn busy(&self) -> usize {
        let st = self.lock();
        self.size - st.free
    }

    /// Highest number of permits out at the same time since creation.
    pub fn peak(&self) -> usize {
        self.lock().peak
    }

    /// Registers a document that may hold at most `cap` permits at the same time (at least 1).
    pub fn share(&self, cap: usize) -> DocShare<'_> {
        let mut st = self.lock();
        let id = st.next_doc;
        st.next_doc += 1;
        st.docs.insert(id, (0, cap.max(1)));
        DocShare { pool: self, id }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock cannot leave the counters half-updated (every critical
        // section updates them together), so a poisoned lock is still usable.
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One document's registration in a [`PagePool`]; permits are acquired through it.
#[derive(Debug)]
pub struct DocShare<'a> {
    pool: &'a PagePool,
    id: u64,
}

impl<'a> DocShare<'a> {
    /// Waits until this document may take a permit (see module docs) and returns it. Must not be
    /// called while the calling thread holds a permit of the same pool (lock order).
    pub fn acquire(&self) -> PagePermit<'_> {
        let pool = self.pool;
        let mut st = pool.lock();
        let ticket = st.next_ticket;
        st.next_ticket += 1;
        st.waiters.push((self.id, ticket));
        while !st.can_take(self.id, ticket) {
            st = pool.cv.wait(st).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        st.waiters.retain(|&(_, t)| t != ticket);
        st.free -= 1;
        let busy = pool.size - st.free;
        st.peak = st.peak.max(busy);
        if let Some(d) = st.docs.get_mut(&self.id) {
            d.0 += 1;
        }
        let more = st.free > 0 && !st.waiters.is_empty();
        drop(st);
        if more {
            // Another waiter may now rank high enough for a remaining free permit.
            pool.cv.notify_all();
        }
        PagePermit { share: self }
    }

    /// Permits this document holds right now.
    pub fn held(&self) -> usize {
        self.pool.lock().docs.get(&self.id).map_or(0, |d| d.0)
    }
}

impl Drop for DocShare<'_> {
    fn drop(&mut self) {
        // Permits borrow the share, so none can be out any more.
        self.pool.lock().docs.remove(&self.id);
        self.pool.cv.notify_all();
    }
}

/// Permission to run one page task; released on drop. `Send`: moves with the page from the
/// producer thread to a worker thread.
#[derive(Debug)]
pub struct PagePermit<'a> {
    share: &'a DocShare<'a>,
}

impl Drop for PagePermit<'_> {
    fn drop(&mut self) {
        let pool = self.share.pool;
        let mut st = pool.lock();
        st.free = (st.free + 1).min(pool.size);
        if let Some(d) = st.docs.get_mut(&self.share.id) {
            d.0 = d.0.saturating_sub(1);
        }
        drop(st);
        pool.cv.notify_all();
    }
}

/// Pool size: logical cores, lowered so that `size × OCR_THREAD_MB` fits into the memory budget
/// of `pl_core::resources` (at least 1) unless `PAPERLESS_THREADS_PER_WORKER` was set by the user
/// (then, like before, no memory cap – but still never more page tasks than cores). Same rule as
/// the startup plan (`pl_core::resources::pool_threads`).
pub fn pool_size(cores: i64, available_mb: Option<u64>, threads_explicit: bool) -> usize {
    usize::try_from(pl_core::resources::pool_threads(cores, available_mb, threads_explicit).max(1)).unwrap_or(1)
}

/// `true` if `PAPERLESS_THREADS_PER_WORKER` was set by the user – not written by the startup
/// memory cap `pl_core::resources::apply_ocr_resource_cap` (that value is the memory-capped pool
/// size itself, see `pl_core::resources::threads_written_by_cap`).
pub fn threads_set_by_user() -> bool {
    match pl_core::config::Env::from_process().get("PAPERLESS_THREADS_PER_WORKER") {
        Some(v) => v.trim().parse::<i64>().map_or(true, |t| !pl_core::resources::threads_written_by_cap(t)),
        None => false,
    }
}

static GLOBAL: OnceLock<Arc<PagePool>> = OnceLock::new();

/// The process-wide pool (created on first use from [`pl_core::resources::machine`]).
pub fn global() -> Arc<PagePool> {
    GLOBAL
        .get_or_init(|| {
            let (cores, mem) = pl_core::resources::machine();
            let size = pool_size(cores, mem, threads_set_by_user());
            tracing::info!("OCR page pool: {size} page tasks at the same time, shared by all documents (cores: {cores})");
            Arc::new(PagePool::new(size))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(5), "timeout waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn size_from_cores_and_memory() {
        assert_eq!(pool_size(12, None, false), 12);
        assert_eq!(pool_size(3, Some(8000), false), 3);
        // 1 GB free -> 512 MB budget -> 5 page tasks at 100 MB
        assert_eq!(pool_size(12, Some(1024), false), 5);
        assert_eq!(pool_size(12, Some(0), false), 1);
        // explicit threads: no memory cap, still never more than cores
        assert_eq!(pool_size(12, Some(1024), true), 12);
        assert_eq!(pool_size(0, None, false), 1);
        assert_eq!(PagePool::new(0).size(), 1);
    }

    #[test]
    fn lone_document_takes_all_permits_up_to_its_cap() {
        let pool = PagePool::new(3);
        let a = pool.share(8);
        let p: Vec<_> = (0..3).map(|_| a.acquire()).collect();
        assert_eq!((pool.busy(), a.held()), (3, 3));
        drop(p);
        assert_eq!(pool.busy(), 0);
        let b = pool.share(2);
        let _p1 = b.acquire();
        let _p2 = b.acquire();
        // cap 2: a third acquire would wait although a permit is free
        let (tx, rx) = mpsc::channel();
        std::thread::scope(|s| {
            s.spawn(|| {
                let _p = b.acquire();
                let _ = tx.send(());
            });
            assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "cap ignored");
            drop(_p1);
            assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        });
    }

    #[test]
    fn single_permit_works_and_serializes() {
        let pool = PagePool::new(1);
        let busy = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    let d = pool.share(4);
                    for _ in 0..10 {
                        let _p = d.acquire();
                        let b = busy.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(b, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_micros(200));
                        busy.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!((pool.peak(), pool.busy()), (1, 0));
    }

    #[test]
    fn new_document_gets_the_next_released_permit() {
        // Document A holds all 3 permits and keeps asking for more (waiter ahead in time); B
        // arrives later with 0 permits. The next release must go to B, not to A.
        let pool = PagePool::new(3);
        let a = pool.share(3);
        let b = pool.share(3);
        let mut held: Vec<_> = (0..3).map(|_| a.acquire()).collect();
        let (tx, rx) = mpsc::channel::<&str>();
        std::thread::scope(|s| {
            let txa = tx.clone();
            let a = &a;
            s.spawn(move || {
                let _p = a.acquire();
                let _ = txa.send("a");
            });
            // A's waiter is registered before B's (lower ticket); A is at its cap though, so
            // release one of A's permits first to make A eligible again – then both compete.
            wait_until("a waiting", || pool.lock().waiters.len() == 1);
            let txb = tx.clone();
            let b = &b;
            s.spawn(move || {
                let _p = b.acquire();
                let _ = txb.send("b");
                std::thread::sleep(Duration::from_millis(50));
            });
            wait_until("b waiting", || pool.lock().waiters.len() == 2);
            held.pop(); // A now holds 2 (< cap), B holds 0 -> B ranks first
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).ok(), Some("b"));
            assert!(rx.recv_timeout(Duration::from_millis(20)).is_err(), "A must still wait");
            held.pop();
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).ok(), Some("a"));
        });
    }

    #[test]
    fn never_more_than_size_and_no_starvation() {
        for size in [1, 2, 3, 5] {
            let pool = PagePool::new(size);
            let busy = AtomicUsize::new(0);
            let peak = AtomicUsize::new(0);
            let done = AtomicUsize::new(0);
            std::thread::scope(|s| {
                // 6 documents with several page threads each (like workers holding handed-over
                // permits), each acquiring 20 pages.
                for doc in 0..6 {
                    let (pool, busy, peak, done) = (&pool, &busy, &peak, &done);
                    s.spawn(move || {
                        let share = pool.share(1 + doc % 4);
                        let (tx, rx) = mpsc::sync_channel::<PagePermit<'_>>(0);
                        let rx = Mutex::new(rx);
                        std::thread::scope(|s2| {
                            for _ in 0..3 {
                                let rx = &rx;
                                s2.spawn(move || {
                                    while let Ok(p) = rx.lock().map_err(|_| ()).and_then(|g| g.recv().map_err(|_| ())) {
                                        let b = busy.fetch_add(1, Ordering::SeqCst) + 1;
                                        peak.fetch_max(b, Ordering::SeqCst);
                                        std::thread::sleep(Duration::from_micros(300));
                                        busy.fetch_sub(1, Ordering::SeqCst);
                                        drop(p);
                                        done.fetch_add(1, Ordering::SeqCst);
                                    }
                                });
                            }
                            for _ in 0..20 {
                                let p = share.acquire();
                                if tx.send(p).is_err() {
                                    break;
                                }
                            }
                            drop(tx);
                        });
                    });
                }
            });
            assert!(peak.load(Ordering::SeqCst) <= size, "size {size}: peak {}", peak.load(Ordering::SeqCst));
            assert!(pool.peak() <= size);
            assert_eq!(done.load(Ordering::SeqCst), 6 * 20, "size {size}: some pages never ran");
            assert_eq!(pool.busy(), 0);
            assert!(pool.lock().docs.is_empty() && pool.lock().waiters.is_empty());
        }
    }

    #[test]
    fn shares_balance_between_two_documents() {
        // Two documents with cap 3 on a pool of 4 both ask for more: the released permit goes to
        // the one holding fewer, so the shares even out at 2 + 2.
        let pool = PagePool::new(4);
        let a = pool.share(3);
        let b = pool.share(3);
        let pa: Vec<_> = (0..3).map(|_| a.acquire()).collect();
        let pb1 = b.acquire();
        assert_eq!((a.held(), b.held()), (3, 1));
        std::thread::scope(|s| {
            let (a, b) = (&a, &b);
            let ta = s.spawn(move || a.acquire());
            wait_until("a waiting", || pool.lock().waiters.len() == 1);
            let tb = s.spawn(move || b.acquire());
            wait_until("both waiting", || pool.lock().waiters.len() == 2);
            // A releases one: A holds 2, B 1 -> B (fewer) gets it although A waited first.
            let mut pa = pa;
            pa.pop();
            wait_until("b served", || tb.is_finished());
            std::thread::sleep(Duration::from_millis(20));
            assert!(!ta.is_finished(), "A must wait while B has fewer permits");
            assert_eq!((a.held(), b.held()), (2, 2));
            pa.pop();
            wait_until("a served", || ta.is_finished());
            let (qa, qb) = (ta.join(), tb.join());
            assert!(qa.is_ok() && qb.is_ok());
        });
        drop(pb1);
        assert_eq!(pool.busy(), 0);
    }
}
