//! iai-callgrind instruction-count benches: the deterministic, CI-safe
//! time proxy that perf budgets gate on (ADR 0001 D2). Setup functions run
//! before measurement, so counts cover only the exercised code — and the
//! exercised code allocates nothing (ADR 0004).

use bw_core::sim::{Sim, SpatialGrid};

const DT: f32 = 1.0 / 60.0;
const RADIUS: f32 = 1.0;

fn setup_sim_1000() -> Sim {
    Sim::seeded(1_000, 42, 200.0, 200.0)
}

fn setup_grid() -> (SpatialGrid, Sim) {
    let sim = setup_sim_1000();
    let mut grid = SpatialGrid::new(200.0, 200.0, RADIUS, sim.len());
    grid.rebuild(&sim.xs, &sim.ys);
    (grid, sim)
}

fn setup_query() -> (SpatialGrid, Sim, Vec<(u32, u32)>) {
    let (grid, sim) = setup_grid();
    let mut pairs = Vec::with_capacity(sim.len());
    grid.query_pairs_into(&sim.xs, &sim.ys, RADIUS, &mut pairs);
    (grid, sim, pairs)
}

#[iai_callgrind::library_benchmark]
#[benches::step_1000(setup = setup_sim_1000)]
fn step(mut sim: Sim) {
    sim.step(DT);
}

#[iai_callgrind::library_benchmark]
#[benches::rebuild_1000(setup = setup_grid)]
fn grid_rebuild(mut input: (SpatialGrid, Sim)) {
    let (grid, sim) = &mut input;
    grid.rebuild(&sim.xs, &sim.ys);
}

#[iai_callgrind::library_benchmark]
#[benches::query_1000(setup = setup_query)]
fn grid_query(input: (SpatialGrid, Sim, Vec<(u32, u32)>)) -> usize {
    let (grid, sim, mut pairs) = input;
    pairs.clear();
    grid.query_pairs_into(&sim.xs, &sim.ys, RADIUS, &mut pairs);
    pairs.len()
}

iai_callgrind::library_benchmark_group!(name = sim; benchmarks = step, grid_rebuild, grid_query);
iai_callgrind::main!(library_benchmark_groups = sim);
