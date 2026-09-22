//! Criterion wall-clock benches for the simulation core (trend tier,
//! ADR 0001 D2). The same exercise functions are measured instruction-wise
//! by `iai_sim`; keep the two in sync.

use std::hint::black_box;

use bw_core::sim::{Sim, SpatialHash};
use criterion::{Criterion, criterion_group, criterion_main};

const DT: f32 = 1.0 / 60.0;
const RADIUS: f32 = 1.0;

fn bench_sim(c: &mut Criterion) {
    let mut sim = Sim::seeded(1_000, 42, 200.0, 200.0);
    c.bench_function("sim_step_1000", |b| {
        b.iter(|| black_box(&mut sim).step(DT));
    });

    c.bench_function("hash_build_1000", |b| {
        b.iter(|| SpatialHash::build(black_box(&sim.xs), black_box(&sim.ys), RADIUS));
    });

    let hash = SpatialHash::build(&sim.xs, &sim.ys, RADIUS);
    c.bench_function("hash_query_1000", |b| {
        b.iter(|| black_box(&hash).query_pairs(black_box(&sim.xs), black_box(&sim.ys), RADIUS));
    });
}

criterion_group!(benches, bench_sim);
criterion_main!(benches);
