//! bw-demo: first gallery binary — a deterministic bouncing-particle scene
//! driven by Bevy ECS over the engine-agnostic `bw_core::sim`, measuring
//! itself headless per the ADR 0001 D4 contract:
//!
//!   bw-demo --perf-headless --scene <id> --ticks <N> --seed <S> --json
//!
//! Two measurement passes exist (ADR 0001, D8.8): the timing pass (default
//! build) reports frame-time percentiles; the allocation pass (build with
//! `--features perf-alloc`, run with `--perf-alloc`) reports allocation
//! count and peak bytes instead. A visual/renderer mode arrives with a
//! future gallery ADR.

mod alloc;

use std::hint::black_box;
use std::time::Instant;

use bevy::prelude::*;
use bw_core::sim::{Sim, SpatialHash};
use bw_core::stats::five_number_summary;
use serde::Serialize;

/// Scene presets: the id fixes the entity count and arena.
fn scene_preset(id: &str) -> Option<(usize, f32, f32)> {
    match id {
        "bounce" => Some((1_000, 200.0, 200.0)),
        "bounce-big" => Some((10_000, 600.0, 600.0)),
        _ => None,
    }
}

const DEFAULT_TICKS: u64 = 600;
const DEFAULT_SEED: u64 = 42;
const WARMUP_TICKS: u64 = 60;
/// Broadphase parameters: cell = query radius, so the 3x3 neighborhood
/// covers each query exactly.
const QUERY_RADIUS: f32 = 1.0;

struct Args {
    scene: String,
    ticks: u64,
    seed: u64,
    json: bool,
    alloc_pass: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        scene: String::new(),
        ticks: DEFAULT_TICKS,
        seed: DEFAULT_SEED,
        json: false,
        alloc_pass: false,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--perf-headless" => {}
            "--perf-scenes" => {
                println!("bounce bounce-big");
                std::process::exit(0);
            }
            "--scene" => args.scene = iter.next().ok_or("--scene needs a value")?,
            "--ticks" => {
                let v = iter.next().ok_or("--ticks needs a value")?;
                args.ticks = v.parse().map_err(|_| format!("bad --ticks value: {v}"))?;
            }
            "--seed" => {
                let v = iter.next().ok_or("--seed needs a value")?;
                args.seed = v.parse().map_err(|_| format!("bad --seed value: {v}"))?;
            }
            "--json" => args.json = true,
            "--perf-alloc" => args.alloc_pass = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if args.scene.is_empty() {
        args.scene = "bounce".to_string();
    }
    if scene_preset(&args.scene).is_none() {
        return Err(format!("unknown scene: {} (try --perf-scenes)", args.scene));
    }
    Ok(args)
}

#[derive(Resource)]
struct SimState {
    sim: Sim,
}

#[derive(Resource)]
struct TickConfig {
    dt: f32,
}

#[derive(Resource, Default)]
struct Stats {
    interactions: u64,
}

/// Mirror of the simulation position in ECS storage: what a renderer would
/// read. Synced from the SoA state every tick.
#[derive(Component)]
struct Position {
    x: f32,
    y: f32,
}

fn step_sim(mut state: ResMut<SimState>, cfg: Res<TickConfig>, mut stats: ResMut<Stats>) {
    state.sim.step(cfg.dt);
    let hash = SpatialHash::build(&state.sim.xs, &state.sim.ys, QUERY_RADIUS);
    stats.interactions += hash
        .query_pairs(&state.sim.xs, &state.sim.ys, QUERY_RADIUS)
        .len() as u64;
}

fn sync_positions(state: Res<SimState>, mut query: Query<&mut Position>) {
    // Query iteration order is not guaranteed by Bevy; it does not matter
    // here — each entity is written its own sim value, and the checksum is
    // computed from the sim, not the components.
    for (i, mut pos) in query.iter_mut().enumerate() {
        pos.x = state.sim.xs[i];
        pos.y = state.sim.ys[i];
    }
}

#[derive(Serialize)]
struct FrameStats {
    min: f64,
    p50: f64,
    p90: f64,
    p99: f64,
    max: f64,
}

#[derive(Serialize)]
struct AllocStats {
    count: u64,
    peak_bytes: u64,
}

#[derive(Serialize)]
struct PerfReport {
    scene: String,
    ticks: u64,
    warmup_ticks: u64,
    seed: u64,
    entities: usize,
    interactions: u64,
    state_checksum: String,
    frame_ms: Option<FrameStats>,
    allocs: Option<AllocStats>,
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("bw-demo: {err}");
            eprintln!(
                "usage: bw-demo --perf-headless [--scene <id>] [--ticks <n>] [--seed <n>] \
                 [--json] [--perf-alloc] | --perf-scenes"
            );
            std::process::exit(2);
        }
    };

    let Some((count, width, height)) = scene_preset(&args.scene) else {
        eprintln!("bw-demo: unknown scene {}", args.scene);
        std::process::exit(2);
    };

    let sim = Sim::seeded(count, args.seed, width, height);
    let entities = sim.len();

    let mut app = App::new();
    app.insert_resource(SimState { sim })
        .insert_resource(TickConfig { dt: 1.0 / 60.0 })
        .insert_resource(Stats::default())
        .add_systems(Update, (step_sim, sync_positions).chain());

    let initial_positions: Vec<(f32, f32)> = {
        let sim = &app.world().resource::<SimState>().sim;
        sim.xs.iter().zip(&sim.ys).map(|(x, y)| (*x, *y)).collect()
    };
    for (x, y) in initial_positions {
        app.world_mut().spawn(Position { x, y });
    }

    // Warmup: allocator, scheduler, and caches settle before timing starts.
    for _ in 0..WARMUP_TICKS {
        app.update();
    }

    let mut frame_ms = Vec::with_capacity(args.ticks as usize);
    for _ in 0..args.ticks {
        let start = Instant::now();
        app.update();
        frame_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    let stats = app.world().resource::<Stats>();
    let sim = &app.world().resource::<SimState>().sim;
    let mut report = PerfReport {
        scene: args.scene.clone(),
        ticks: args.ticks,
        warmup_ticks: WARMUP_TICKS,
        seed: args.seed,
        entities,
        interactions: stats.interactions,
        state_checksum: format!("{:016x}", sim.state_checksum()),
        frame_ms: None,
        allocs: None,
    };

    if args.alloc_pass {
        #[cfg(feature = "perf-alloc")]
        {
            let (count, peak) = alloc::snapshot();
            report.allocs = Some(AllocStats {
                count,
                peak_bytes: peak,
            });
        }
        #[cfg(not(feature = "perf-alloc"))]
        {
            let _ = &args;
            eprintln!("bw-demo: --perf-alloc requires building with --features perf-alloc");
            std::process::exit(2);
        }
    } else {
        let (min, p50, p90, p99, max) = five_number_summary(black_box(frame_ms));
        report.frame_ms = Some(FrameStats {
            min,
            p50,
            p90,
            p99,
            max,
        });
    }

    let json = serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string());
    if args.json {
        println!("{json}");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    }
}
