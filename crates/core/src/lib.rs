//! bw-core: engine-agnostic game logic.
//!
//! Simulation and domain code lives here in plain Rust (ADR 0002) so it is
//! benchmarkable without the engine and insulated from engine churn; Bevy
//! glue lives in the gallery binaries.

pub mod gallery;
pub mod sim;
pub mod stats;
pub mod tree;

/// Probe runtime for `#[alloc_probe]` (ADR 0004, D4). Feature-gated: the
/// dhat machinery exists only in allocation-measurement builds.
#[cfg(feature = "perf-alloc")]
pub mod alloc_probe;
