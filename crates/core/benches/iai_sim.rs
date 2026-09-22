//! iai-callgrind instruction-count benches: the deterministic, CI-safe
//! time proxy that perf budgets gate on (ADR 0001 D2). Setup functions run
//! before measurement, so counts cover only the exercised code.

use bw_core::sim::{Sim, SpatialHash};

const DT: f32 = 1.0 / 60.0;
const RADIUS: f32 = 1.0;

fn setup_sim_1000() -> Sim {
    Sim::seeded(1_000, 42, 200.0, 200.0)
}

fn setup_hash_1000() -> (SpatialHash, Sim) {
    let sim = Sim::seeded(1_000, 42, 200.0, 200.0);
    let hash = SpatialHash::build(&sim.xs, &sim.ys, RADIUS);
    (hash, sim)
}

#[iai_callgrind::library_benchmark]
#[benches::step_1000(setup = setup_sim_1000)]
fn step(mut sim: Sim) {
    sim.step(DT);
}

#[iai_callgrind::library_benchmark]
#[benches::build_1000(setup = setup_sim_1000)]
fn hash_build(sim: Sim) -> SpatialHash {
    SpatialHash::build(&sim.xs, &sim.ys, RADIUS)
}

#[iai_callgrind::library_benchmark]
#[benches::query_1000(setup = setup_hash_1000)]
fn hash_query(input: (SpatialHash, Sim)) -> usize {
    let (hash, sim) = input;
    hash.query_pairs(&sim.xs, &sim.ys, RADIUS).len()
}

iai_callgrind::library_benchmark_group!(name = sim; benchmarks = step, hash_build, hash_query);
iai_callgrind::main!(library_benchmark_groups = sim);
