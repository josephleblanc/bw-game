//! Steady-state zero-alloc contract (ADR 0004): hot-path functions must
//! allocate nothing once warm. This lives in its own integration-test
//! binary on purpose — the dhat global allocator is process-wide, so a
//! dedicated binary keeps other tests' allocations out of the measured
//! window (and only one Profiler may run at a time).

use bw_core::sim::{Sim, SpatialGrid};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const TICKS: usize = 100;
const DT: f32 = bw_core::time::SIM_DT;

#[test]
fn steady_state_sim_grid_and_query_allocate_nothing() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let mut sim = Sim::seeded(1_000, 42, 200.0, 200.0);
    let mut grid = SpatialGrid::new(200.0, 200.0, 1.0, sim.len());
    let mut pairs = Vec::new();

    // Warm: establish capacities and dhat's callsite bookkeeping before
    // the measured window opens.
    grid.rebuild(&sim.xs, &sim.ys);
    grid.query_pairs_into(&sim.xs, &sim.ys, 1.0, &mut pairs);

    let before = dhat::HeapStats::get();
    for _ in 0..TICKS {
        sim.step(DT);
        grid.rebuild(&sim.xs, &sim.ys);
        grid.query_pairs_into(&sim.xs, &sim.ys, 1.0, &mut pairs);
    }
    let after = dhat::HeapStats::get();

    assert_eq!(
        after.total_blocks,
        before.total_blocks,
        "steady state allocated {} blocks ({} bytes) over {TICKS} ticks",
        after.total_blocks - before.total_blocks,
        after.total_bytes - before.total_bytes,
    );
}
