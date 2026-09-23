//! bw-demo: first gallery binary — a deterministic bouncing-particle scene
//! driven by Bevy ECS over the engine-agnostic `bw_core::sim`, measuring
//! itself headless per the ADR 0001 D4 contract:
//!
//!   bw-demo --perf-headless --scene <id> --ticks <N> --seed <S>
//!           [--dhat-out <path>] --json
//!
//! Two measurement passes exist (ADR 0001, D8.8): the timing pass (default
//! build) reports frame-time percentiles; the allocation pass (build with
//! `--features perf-alloc`, run with `--perf-alloc`) reports windowed and
//! per-tick allocation series with per-function attribution via
//! `#[alloc_probe]` (ADR 0004). A visual/renderer mode arrives with a
//! future gallery ADR.

// The attribute macro from the alloc-probe crate; the runtime module is
// aliased because a plain `use bw_core::alloc_probe` would shadow the
// extern-crate name in `use` paths under this feature.
use alloc_probe::alloc_probe;

#[cfg(feature = "perf-alloc")]
use bw_core::alloc_probe as alloc_probe_rt;

#[cfg(feature = "perf-alloc")]
use bw_core::gallery::alloc_detail;

/// Counting allocator for the perf-alloc build. Declared by each
/// measurement binary — not by bw-core — so the allocator stays a
/// per-binary decision (see also `tests/steady_alloc.rs` in bw-core).
#[cfg(feature = "perf-alloc")]
#[global_allocator]
static ALLOC: alloc_probe_rt::dhat::Alloc = alloc_probe_rt::dhat::Alloc;

use std::hint::black_box;
use std::time::Instant;

use bevy::prelude::*;
#[cfg(feature = "perf-alloc")]
use bw_core::gallery::AllocStats;
use bw_core::gallery::{FrameStats, PerfReport};
use bw_core::sim::{Sim, SpatialGrid};
use bw_core::time::SIM_DT;

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
    dhat_out: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        scene: String::new(),
        ticks: DEFAULT_TICKS,
        seed: DEFAULT_SEED,
        json: false,
        alloc_pass: false,
        dhat_out: None,
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
            "--dhat-out" => {
                let v = iter.next().ok_or("--dhat-out needs a value")?;
                args.dhat_out = Some(v);
            }
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

/// Profiler output path, scanned from the raw args before parsing so the
/// profiler can start before parse-time allocations. Flag errors surface
/// from `parse_args` as usual; a missing flag falls back to the shared
/// default (per-scene callers pass `--dhat-out` to avoid clobbering it).
#[cfg(feature = "perf-alloc")]
fn dhat_out_path() -> String {
    const DEFAULT: &str = "target/perf/dhat-heap.json";
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--dhat-out"
            && let Some(value) = args.next()
        {
            return value;
        }
    }
    DEFAULT.to_string()
}

#[derive(Resource)]
struct SimState {
    sim: Sim,
    /// Persistent broadphase storage, rebuilt in place every tick (ADR 0004).
    grid: SpatialGrid,
    /// Reused pairs scratch: cleared and refilled by `query_pairs_into`.
    pairs: Vec<(u32, u32)>,
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

#[alloc_probe]
fn step_sim(mut state: ResMut<SimState>, cfg: Res<TickConfig>, mut stats: ResMut<Stats>) {
    let SimState { sim, grid, pairs } = &mut *state;
    sim.step(cfg.dt);
    grid.rebuild(&sim.xs, &sim.ys);
    pairs.clear();
    grid.query_pairs_into(&sim.xs, &sim.ys, QUERY_RADIUS, pairs);
    stats.interactions += pairs.len() as u64;
}

#[alloc_probe]
fn sync_positions(state: Res<SimState>, mut query: Query<&mut Position>) {
    // Query iteration order is not guaranteed by Bevy; it does not matter
    // here — each entity is written its own sim value, and the checksum is
    // computed from the sim, not the components.
    for (i, mut pos) in query.iter_mut().enumerate() {
        pos.x = state.sim.xs[i];
        pos.y = state.sim.ys[i];
    }
}

fn main() {
    // First statement so the setup window covers everything after main
    // entry. Pre-main and parse-time allocations are passed through dhat
    // uncounted. The output path is scanned from the raw args (before
    // parsing) so per-scene reports don't clobber the shared default; the
    // same flag is validated again by `parse_args`.
    #[cfg(feature = "perf-alloc")]
    let _profiler = alloc_probe_rt::init_profiler(&dhat_out_path());

    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("bw-demo: {err}");
            eprintln!(
                "usage: bw-demo --perf-headless [--scene <id>] [--ticks <n>] [--seed <n>] \
                 [--dhat-out <path>] [--json] [--perf-alloc] | --perf-scenes"
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
    app.insert_resource(SimState {
        sim,
        grid: SpatialGrid::new(width, height, QUERY_RADIUS, count),
        // Sized to the entity count: generous headroom at these densities.
        // A scene that outgrows it fails the steady-state budget visibly
        // instead of allocating silently mid-frame (ADR 0004).
        pairs: Vec::with_capacity(count),
    })
    .insert_resource(TickConfig { dt: SIM_DT })
    .insert_resource(Stats::default())
    .add_systems(Update, (step_sim, sync_positions).chain());

    let initial_positions: Vec<(f32, f32)> = {
        let sim = &app.world().resource::<SimState>().sim;
        sim.xs.iter().zip(&sim.ys).map(|(x, y)| (*x, *y)).collect()
    };
    for (x, y) in initial_positions {
        app.world_mut().spawn(Position { x, y });
    }

    // Setup window ends here: everything above is one-off scene cost.
    #[cfg(feature = "perf-alloc")]
    let setup_totals = alloc_probe_rt::totals();

    // Warmup: allocator, scheduler, and caches settle before timing starts.
    for _ in 0..WARMUP_TICKS {
        app.update();
    }
    #[cfg(feature = "perf-alloc")]
    let warmup_totals = alloc_probe_rt::totals();
    #[cfg(feature = "perf-alloc")]
    let probes_before_window = alloc_probe_rt::registry_snapshot();
    #[cfg(feature = "perf-alloc")]
    let live_start = alloc_probe_rt::live_bytes();

    #[cfg(feature = "perf-alloc")]
    let mut tick_blocks: Vec<f64> = Vec::with_capacity(args.ticks as usize);
    #[cfg(feature = "perf-alloc")]
    let mut tick_bytes: Vec<f64> = Vec::with_capacity(args.ticks as usize);
    let mut frame_ms = Vec::with_capacity(args.ticks as usize);
    for _ in 0..args.ticks {
        #[cfg(feature = "perf-alloc")]
        let before = alloc_probe_rt::totals();
        let start = Instant::now();
        app.update();
        let elapsed = start.elapsed();
        #[cfg(feature = "perf-alloc")]
        {
            let after = alloc_probe_rt::totals();
            tick_blocks.push((after.0 - before.0) as f64);
            tick_bytes.push((after.1 - before.1) as f64);
        }
        frame_ms.push(elapsed.as_secs_f64() * 1000.0);
    }

    #[cfg(feature = "perf-alloc")]
    let measured_totals = alloc_probe_rt::totals();
    #[cfg(feature = "perf-alloc")]
    let live_end = alloc_probe_rt::live_bytes();
    #[cfg(feature = "perf-alloc")]
    let probes_final = alloc_probe_rt::registry_snapshot();

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
        alloc_detail: None,
    };

    if args.alloc_pass {
        #[cfg(feature = "perf-alloc")]
        {
            let (total_blocks, _) = alloc_probe_rt::totals();
            report.allocs = Some(AllocStats {
                count: total_blocks,
                peak_bytes: alloc_probe_rt::peak_bytes(),
            });
            report.alloc_detail = Some(alloc_detail(
                &probes_before_window,
                &probes_final,
                setup_totals,
                warmup_totals,
                measured_totals,
                live_start,
                live_end,
                tick_blocks,
                tick_bytes,
                args.ticks,
            ));
        }
        #[cfg(not(feature = "perf-alloc"))]
        {
            let _ = &args;
            eprintln!("bw-demo: --perf-alloc requires building with --features perf-alloc");
            std::process::exit(2);
        }
    } else {
        report.frame_ms = Some(FrameStats::of(black_box(frame_ms)));
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
