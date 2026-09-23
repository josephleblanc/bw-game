//! Criterion wall-clock benches for the simulation core (trend tier,
//! ADR 0001 D2). The same exercise functions are measured instruction-wise
//! by `iai_sim`; keep the two in sync. Measured regions allocate nothing
//! (ADR 0004): grids and pair scratches are pre-built in setup.

use std::hint::black_box;

use bw_core::sim::{Sim, SpatialGrid};
use criterion::{Criterion, criterion_group, criterion_main};

const DT: f32 = bw_core::time::SIM_DT;
const RADIUS: f32 = 1.0;

fn bench_sim(c: &mut Criterion) {
    let mut sim = Sim::seeded(1_000, 42, 200.0, 200.0);
    c.bench_function("sim_step_1000", |b| {
        b.iter(|| black_box(&mut sim).step(DT));
    });

    let mut grid = SpatialGrid::new(200.0, 200.0, RADIUS, sim.len());
    c.bench_function("grid_rebuild_1000", |b| {
        b.iter(|| black_box(&mut grid).rebuild(black_box(&sim.xs), black_box(&sim.ys)));
    });

    grid.rebuild(&sim.xs, &sim.ys);
    let mut pairs = Vec::with_capacity(sim.len());
    c.bench_function("grid_query_1000", |b| {
        b.iter(|| {
            black_box(&mut pairs).clear();
            black_box(&grid).query_pairs_into(
                black_box(&sim.xs),
                black_box(&sim.ys),
                RADIUS,
                black_box(&mut pairs),
            );
        });
    });
}

criterion_group!(benches, bench_sim);
criterion_main!(benches);
