//! `--processes` and `process_parallel` (B7).
//!
//! upstream: src/documents/management/commands/base.py@v3.4.2 (`PaperlessCommand.add_arguments`:
//! default `max(1, os.cpu_count() // 4)`; `execute`: "--processes must be at least 1";
//! `process_parallel`: `ProcessPoolExecutor(max_workers=process_count)` + `as_completed`, with 1
//! sequentially in the main process). Here: tokio tasks instead of processes, at most `n` at a time,
//! results in completion order like `as_completed`.

use std::future::Future;

use crate::args::Parsed;
use crate::{CliError, CliResult};

/// `max(1, (os.cpu_count() or 1) // 4)`.
pub fn default_processes() -> usize {
    let cpus = std::thread::available_parallelism().map(usize::from).unwrap_or(1);
    (cpus / 4).max(1)
}

/// Value of `--processes` (missing → default).
pub fn processes(a: &Parsed) -> CliResult<usize> {
    match a.int("processes")? {
        None => Ok(default_processes()),
        Some(n) if n < 1 => Err(CliError::Command("--processes must be at least 1".into())),
        Some(n) => Ok(usize::try_from(n).unwrap_or(usize::MAX)),
    }
}

/// `process_parallel(fn, items)`: `f` for each element, at most `n` at a time; `on_result`
/// receives `(item, result)` in completion order (for `n == 1` in input order).
/// A crashed task (panic) counts as an error of the element.
pub async fn process_parallel<T, R, F, Fut>(items: Vec<T>, n: usize, f: F, mut on_result: impl FnMut(T, Result<R, String>))
where
    T: Clone + Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: Future<Output = Result<R, String>> + Send + 'static,
{
    let n = n.max(1);
    let mut pending = items.into_iter();
    let mut set: tokio::task::JoinSet<(T, Result<R, String>)> = tokio::task::JoinSet::new();
    let mut in_flight: std::collections::HashMap<tokio::task::Id, T> = std::collections::HashMap::new();
    loop {
        while set.len() < n {
            let Some(item) = pending.next() else { break };
            let fut = f(item.clone());
            let it = item.clone();
            let h = set.spawn(async move { (it, fut.await) });
            in_flight.insert(h.id(), item);
        }
        match set.join_next_with_id().await {
            None => break,
            Some(Ok((id, (item, r)))) => {
                in_flight.remove(&id);
                on_result(item, r);
            }
            Some(Err(e)) => {
                if let Some(item) = in_flight.remove(&e.id()) {
                    on_result(item, Err(format!("task failed: {e}")));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn processes_option() {
        let p =
            |v: &[&str]| crate::args::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &[crate::args::PROCESSES], 0).unwrap();
        assert_eq!(processes(&p(&[])).unwrap(), default_processes());
        assert_eq!(processes(&p(&["--processes", "3"])).unwrap(), 3);
        assert!(matches!(processes(&p(&["--processes", "0"])), Err(CliError::Command(m)) if m == "--processes must be at least 1"));
        assert!(default_processes() >= 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runs_concurrently_but_bounded() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut seen = Vec::new();
        let (a, p) = (active.clone(), peak.clone());
        process_parallel(
            (0..12).collect(),
            3,
            move |i: i32| {
                let (a, p) = (a.clone(), p.clone());
                async move {
                    let now = a.fetch_add(1, Ordering::SeqCst) + 1;
                    p.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    a.fetch_sub(1, Ordering::SeqCst);
                    if i == 5 { Err("boom".to_string()) } else { Ok(i * 2) }
                }
            },
            |i, r| seen.push((i, r)),
        )
        .await;
        assert_eq!(seen.len(), 12);
        let peak = peak.load(Ordering::SeqCst);
        assert!((2..=3).contains(&peak), "bounded parallelism, peak {peak}");
        assert!(seen.iter().any(|(i, r)| *i == 5 && r.is_err()));
        assert!(seen.iter().any(|(i, r)| *i == 4 && r.as_ref().ok() == Some(&8)));
    }
}
