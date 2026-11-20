//! Memory limit of the child process: a counting allocator on top of the system allocator.
//!
//! Only `paperrust-pdftool` installs it (`#[global_allocator]`). If the heap exceeds the
//! limit, it returns `null`; Rust then aborts the process with "memory allocation failed". The
//! server only sees a failed child process. Platform independent (no `setrlimit`,
//! no Windows job object needed).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static USED: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(0);

/// Set the limit in bytes (0 = no limit).
pub fn set_limit_bytes(bytes: usize) {
    LIMIT.store(bytes, Ordering::SeqCst);
}

/// Set the limit in MiB (0 = no limit).
pub fn set_limit_mb(mb: u64) {
    let bytes = usize::try_from(mb.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX);
    set_limit_bytes(bytes);
}

/// Heap in use, in bytes.
pub fn used() -> usize {
    USED.load(Ordering::Relaxed)
}

fn reserve(n: usize) -> bool {
    let limit = LIMIT.load(Ordering::Relaxed);
    let prev = USED.fetch_add(n, Ordering::Relaxed);
    if limit != 0 && prev.saturating_add(n) > limit {
        USED.fetch_sub(n, Ordering::Relaxed);
        return false;
    }
    true
}

fn release(n: usize) {
    USED.fetch_sub(n, Ordering::Relaxed);
}

/// The counting allocator.
pub struct Guard;

// SAFETY: passes every request on to `System` unchanged and only keeps books on the size;
// a refused reservation returns `null`, as `GlobalAlloc` provides for failures.
unsafe impl GlobalAlloc for Guard {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: contract of GlobalAlloc::alloc passed on to the caller.
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            release(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: as `alloc`.
        let p = unsafe { System.alloc_zeroed(layout) };
        if p.is_null() {
            release(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` comes from this allocator (i.e. from `System`) with `layout`.
        unsafe { System.dealloc(ptr, layout) };
        release(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old = layout.size();
        if new_size > old && !reserve(new_size - old) {
            return std::ptr::null_mut();
        }
        // SAFETY: contract of GlobalAlloc::realloc passed on to the caller.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if p.is_null() {
            if new_size > old {
                release(new_size - old);
            }
        } else if new_size < old {
            release(old - new_size);
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_respects_limit() {
        // The test process uses the system allocator; only the bookkeeping here.
        set_limit_bytes(0);
        let base = used();
        assert!(reserve(10));
        release(10);
        assert_eq!(used(), base);
        set_limit_bytes(base + 100);
        assert!(reserve(50));
        assert!(!reserve(100));
        release(50);
        set_limit_bytes(0);
    }
}
