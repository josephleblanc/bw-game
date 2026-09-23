//! Allocation-probe runtime (ADR 0004, D4/D5): the machinery behind
//! `#[alloc_probe]` (`crates/alloc-probe`). Compiled only under this
//! crate's `perf-alloc` feature, which the gallery binaries chain from
//! their own `perf-alloc` feature — the counting allocator never enters
//! timing or shipped builds (ADR 0001, D8.8).
//!
//! dhat-rs is the global allocator in a measurement binary (each binary
//! declares its own `#[global_allocator]` — see `bw-demo/src/main.rs` and
//! `tests/steady_alloc.rs`; keeping the static out of this library leaves
//! that decision with the binary). dhat's monotonic `HeapStats` totals
//! power the setup/warmup/measured windows and the per-tick series;
//! `Probe` guards attribute allocations per function by totals-delta.
//! dhat 0.3 has no region type, so per-callsite detail comes from the JSON
//! the Profiler writes on drop.

pub use dhat;

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// Running per-function totals: name -> (blocks, bytes, calls).
pub type Registry = BTreeMap<&'static str, (u64, u64, u64)>;

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Attribution guard emitted by `#[alloc_probe]`: snapshots the monotonic
/// totals at entry; Drop accumulates the delta into the registry. Deltas
/// use totals (never current bytes) so allocations made-and-freed inside
/// the function — the churn we hunt — are counted. The macro places the
/// guard first in the function so it closes last over the whole body.
pub struct Probe {
    name: &'static str,
    start_blocks: u64,
    start_bytes: u64,
}

impl Probe {
    pub fn new(name: &'static str) -> Self {
        let stats = dhat::HeapStats::get();
        Self {
            name,
            start_blocks: stats.total_blocks,
            start_bytes: stats.total_bytes,
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let stats = dhat::HeapStats::get();
        let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
        let entry = registry.entry(self.name).or_default();
        entry.0 += stats.total_blocks - self.start_blocks;
        entry.1 += stats.total_bytes - self.start_bytes;
        entry.2 += 1;
    }
}

/// Monotonic allocation snapshot: (total_blocks, total_bytes).
pub fn totals() -> (u64, u64) {
    let stats = dhat::HeapStats::get();
    (stats.total_blocks, stats.total_bytes)
}

/// Currently-live bytes: flat across ticks is healthy; monotonic
/// growth is the leak signal.
pub fn live_bytes() -> u64 {
    dhat::HeapStats::get().curr_bytes as u64
}

/// High-water mark of live bytes.
pub fn peak_bytes() -> u64 {
    dhat::HeapStats::get().max_bytes as u64
}

pub fn registry_snapshot() -> Registry {
    registry().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Create the run profiler. Call before any allocation you want
/// counted; the JSON report (per-callsite blocks/bytes/lifetimes) is
/// written when the returned Profiler drops.
pub fn init_profiler(path: &str) -> dhat::Profiler {
    if let Some(parent) = std::path::Path::new(path).parent() {
        // Best-effort: dhat errors out on drop if the directory is
        // missing, but does not fail the run.
        let _ = std::fs::create_dir_all(parent);
    }
    dhat::Profiler::builder().file_name(path).build()
}
