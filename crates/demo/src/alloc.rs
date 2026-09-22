//! Allocation-counting global allocator, compiled only behind the
//! `perf-alloc` feature (ADR 0003). The counting pass is a separate build
//! and run so instrumentation never perturbs timing measurements
//! (ADR 0001, D8.8).

#[cfg(feature = "perf-alloc")]
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static ALLOCS: AtomicU64 = AtomicU64::new(0);
    pub static BYTES_IN_USE: AtomicU64 = AtomicU64::new(0);
    pub static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);

    // SAFETY: forwards every operation to the system allocator, tracking
    // only sizes and counts; returns exactly what `System` returns. This
    // is the one place in the workspace that must use `unsafe`, and it is
    // deliberately tiny and feature-gated (the workspace denies unsafe
    // elsewhere).
    pub struct CountingAlloc;

    #[allow(unsafe_code)]
    unsafe impl GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                ALLOCS.fetch_add(1, Ordering::Relaxed);
                let in_use = BYTES_IN_USE.fetch_add(layout.size() as u64, Ordering::Relaxed)
                    + layout.size() as u64;
                PEAK_BYTES.fetch_max(in_use, Ordering::Relaxed);
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            BYTES_IN_USE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
            unsafe { System.dealloc(ptr, layout) };
        }
    }
}

#[cfg(feature = "perf-alloc")]
#[global_allocator]
static ALLOCATOR: counting::CountingAlloc = counting::CountingAlloc;

/// (alloc_count, peak_bytes_in_use), valid only in `perf-alloc` builds.
#[cfg(feature = "perf-alloc")]
pub fn snapshot() -> (u64, u64) {
    (
        counting::ALLOCS.load(std::sync::atomic::Ordering::Relaxed),
        counting::PEAK_BYTES.load(std::sync::atomic::Ordering::Relaxed),
    )
}
