//! bw-core: engine-agnostic game logic.
//!
//! Simulation and domain code lives here in plain Rust (ADR 0002) so it is
//! benchmarkable without the engine and insulated from engine churn; Bevy
//! glue lives in the gallery binaries.

pub mod sim;
pub mod stats;
