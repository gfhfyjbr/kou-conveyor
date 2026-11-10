use serde::Serialize;

#[cfg(feature = "jemalloc")]
type Base = tikv_jemallocator::Jemalloc;
#[cfg(not(feature = "jemalloc"))]
type Base = std::alloc::System;

#[cfg(feature = "jemalloc")]
const BASE: Base = tikv_jemallocator::Jemalloc;
#[cfg(not(feature = "jemalloc"))]
const BASE: Base = std::alloc::System;

#[cfg(not(feature = "count-alloc"))]
#[global_allocator]
static ALLOCATOR: Base = BASE;

#[derive(Clone, Copy, Default, Serialize)]
pub struct Stats {
    pub allocated_bytes: u64,
    pub allocations: u64,
    pub live_requested_bytes: u64,
    pub peak_requested_bytes: u64,
    pub process_cpu_ns: u64,
}

#[cfg(feature = "count-alloc")]
mod counted {
    use super::{BASE, Stats};
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    pub struct Counted;
    static LIVE: AtomicU64 = AtomicU64::new(0);
    static PEAK: AtomicU64 = AtomicU64::new(0);
    static TOTAL: AtomicU64 = AtomicU64::new(0);
    static COUNT: AtomicU64 = AtomicU64::new(0);

    fn allocated(n: usize) {
        TOTAL.fetch_add(n as u64, Relaxed);
        COUNT.fetch_add(1, Relaxed);
        let live = LIVE.fetch_add(n as u64, Relaxed) + n as u64;
        PEAK.fetch_max(live, Relaxed);
    }

    // All pointer/layout operations are delegated to the selected allocator.
    unsafe impl GlobalAlloc for Counted {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { BASE.alloc(layout) };
            if !ptr.is_null() {
                allocated(layout.size());
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size() as u64, Relaxed);
            unsafe { BASE.dealloc(ptr, layout) };
        }

        unsafe fn realloc(&self, ptr: *mut u8, old: Layout, size: usize) -> *mut u8 {
            let new_ptr = unsafe { BASE.realloc(ptr, old, size) };
            if !new_ptr.is_null() {
                LIVE.fetch_sub(old.size() as u64, Relaxed);
                allocated(size);
            }
            new_ptr
        }
    }

    pub fn stats() -> Stats {
        Stats {
            allocated_bytes: TOTAL.load(Relaxed),
            allocations: COUNT.load(Relaxed),
            live_requested_bytes: LIVE.load(Relaxed),
            peak_requested_bytes: PEAK.load(Relaxed),
            ..Stats::default()
        }
    }

    pub fn reset_peak() {
        PEAK.store(LIVE.load(Relaxed), Relaxed);
    }
}

#[cfg(feature = "count-alloc")]
#[global_allocator]
static ALLOCATOR: counted::Counted = counted::Counted;

pub fn stats() -> Stats {
    #[cfg(feature = "count-alloc")]
    let mut snapshot = counted::stats();
    #[cfg(not(feature = "count-alloc"))]
    let mut snapshot = Stats::default();
    snapshot.process_cpu_ns = crate::cpu::process_ns();
    snapshot
}

pub fn reset_peak() {
    #[cfg(feature = "count-alloc")]
    counted::reset_peak();
}

pub fn name() -> &'static str {
    if cfg!(feature = "jemalloc") {
        "jemalloc"
    } else {
        "system"
    }
}
