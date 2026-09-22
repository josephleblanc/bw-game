//! bw-demo: first gallery binary — a deterministic bouncing-particle scene
//! driven by Bevy ECS over the engine-agnostic `bw_core::sim`, measuring
//! itself headless per the ADR 0001 D4 contract:
//!
//!   bw-demo --perf-headless --scene <id> --ticks <N> --seed <S> --json
//!
//! Two measurement passes exist (ADR 0001, D8.8): the timing pass (default
//! build) reports frame-time percentiles; the allocation pass (build with
//! `--features perf-alloc`, run with `--perf-alloc`) reports windowed and
//! per-tick allocation series with per-function probe attribution. A
//! visual/renderer mode arrives with a future gallery ADR.

mod alloc;

use std::hint::black_box;
use std::time::Instant;

use bevy::prelude::*;
use bw_core::sim::{Sim, SpatialGrid};
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

fn step_sim(mut state: ResMut<SimState>, cfg: Res<TickConfig>, mut stats: ResMut<Stats>) {
    #[cfg(feature = "perf-alloc")]
    let _probe = alloc::Probe::new("step_sim");
    let SimState { sim, grid, pairs } = &mut *state;
    sim.step(cfg.dt);
    grid.rebuild(&sim.xs, &sim.ys);
    pairs.clear();
    grid.query_pairs_into(&sim.xs, &sim.ys, QUERY_RADIUS, pairs);
    stats.interactions += pairs.len() as u64;
}

fn sync_positions(state: Res<SimState>, mut query: Query<&mut Position>) {
    #[cfg(feature = "perf-alloc")]
    let _probe = alloc::Probe::new("sync_positions");
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

impl FrameStats {
    fn of(sample: Vec<f64>) -> Self {
        let (min, p50, p90, p99, max) = five_number_summary(sample);
        Self {
            min,
            p50,
            p90,
            p99,
            max,
        }
    }
}

#[derive(Serialize)]
struct AllocStats {
    count: u64,
    peak_bytes: u64,
}

#[derive(Serialize)]
struct WindowTotals {
    blocks: u64,
    bytes: u64,
}

#[derive(Serialize)]
struct ProbeTotals {
    blocks: u64,
    bytes: u64,
    calls: u64,
    blocks_per_call: f64,
    bytes_per_call: f64,
}

#[derive(Serialize)]
struct MeasuredWindow {
    blocks: u64,
    bytes: u64,
    ticks: u64,
    blocks_per_tick: FrameStats,
    bytes_per_tick: FrameStats,
}

#[derive(Serialize)]
struct AllocDetail {
    /// One-off cost: profiler creation through entity spawn (app + schedule
    /// construction, scene seeding, archetype table growth).
    setup: WindowTotals,
    /// Warmup window: schedule-system initialization and first-run growth;
    /// shares steady-state code paths.
    warmup: WindowTotals,
    /// The measured window: the steady-state churn the budget will gate.
    measured: MeasuredWindow,
    /// Per-function attribution, whole run (warmup + measured).
    probes: std::collections::BTreeMap<String, ProbeTotals>,
    /// Per-function attribution within the measured window only.
    measured_probes: std::collections::BTreeMap<String, ProbeTotals>,
    /// Everything inside a tick not attributed to a probed function: the
    /// allocation debt inherited from the engine's schedule machinery.
    schedule_residual: WindowTotals,
    /// Live bytes at window boundaries: flat means no leak; drift is the
    /// leak signal (per tick).
    live: LiveBytes,
}

#[derive(Serialize)]
struct LiveBytes {
    start: u64,
    end: u64,
    drift_bytes_per_tick: f64,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    alloc_detail: Option<AllocDetail>,
}

fn main() {
    // First statement so the setup window covers everything after main
    // entry. Pre-main and parse-time allocations are passed through dhat
    // uncounted.
    #[cfg(feature = "perf-alloc")]
    let _profiler = alloc::init_profiler("target/perf/dhat-heap.json");

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
    app.insert_resource(SimState {
        sim,
        grid: SpatialGrid::new(width, height, QUERY_RADIUS, count),
        // Sized to the entity count: generous headroom at these densities.
        // A scene that outgrows it fails the steady-state budget visibly
        // instead of allocating silently mid-frame (ADR 0004).
        pairs: Vec::with_capacity(count),
    })
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

    // Setup window ends here: everything above is one-off scene cost.
    #[cfg(feature = "perf-alloc")]
    let setup_totals = alloc::totals();

    // Warmup: allocator, scheduler, and caches settle before timing starts.
    for _ in 0..WARMUP_TICKS {
        app.update();
    }
    #[cfg(feature = "perf-alloc")]
    let warmup_totals = alloc::totals();
    #[cfg(feature = "perf-alloc")]
    let probes_before_window = alloc::registry_snapshot();
    #[cfg(feature = "perf-alloc")]
    let live_start = alloc::live_bytes();

    #[cfg(feature = "perf-alloc")]
    let mut tick_blocks: Vec<f64> = Vec::with_capacity(args.ticks as usize);
    #[cfg(feature = "perf-alloc")]
    let mut tick_bytes: Vec<f64> = Vec::with_capacity(args.ticks as usize);
    let mut frame_ms = Vec::with_capacity(args.ticks as usize);
    for _ in 0..args.ticks {
        #[cfg(feature = "perf-alloc")]
        let before = alloc::totals();
        let start = Instant::now();
        app.update();
        let elapsed = start.elapsed();
        #[cfg(feature = "perf-alloc")]
        {
            let after = alloc::totals();
            tick_blocks.push((after.0 - before.0) as f64);
            tick_bytes.push((after.1 - before.1) as f64);
        }
        frame_ms.push(elapsed.as_secs_f64() * 1000.0);
    }

    #[cfg(feature = "perf-alloc")]
    let measured_totals = alloc::totals();
    #[cfg(feature = "perf-alloc")]
    let live_end = alloc::live_bytes();
    #[cfg(feature = "perf-alloc")]
    let probes_final = alloc::registry_snapshot();

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
            let (total_blocks, _) = alloc::totals();
            report.allocs = Some(AllocStats {
                count: total_blocks,
                peak_bytes: alloc::peak_bytes(),
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

/// Assemble the windowed/probed allocation report from the harness's
/// boundary snapshots. Series are already per-tick samples in order.
#[cfg(feature = "perf-alloc")]
#[allow(clippy::too_many_arguments)]
fn alloc_detail(
    probes_before_window: &alloc::Registry,
    probes_final: &alloc::Registry,
    setup_totals: (u64, u64),
    warmup_totals: (u64, u64),
    measured_totals: (u64, u64),
    live_start: u64,
    live_end: u64,
    tick_blocks: Vec<f64>,
    tick_bytes: Vec<f64>,
    ticks: u64,
) -> AllocDetail {
    fn probe_map(
        source: &alloc::Registry,
        calls: &alloc::Registry,
    ) -> std::collections::BTreeMap<String, ProbeTotals> {
        let mut map = std::collections::BTreeMap::new();
        // `calls` carries the per-name call counts (identical in both
        // snapshots for run-long probes; window entries use whole-run
        // counts as the denominator).
        for (name, (blocks, bytes, _)) in source {
            let n = calls.get(name).map(|(_, _, c)| *c).unwrap_or(1) as f64;
            map.insert(
                (*name).to_string(),
                ProbeTotals {
                    blocks: *blocks,
                    bytes: *bytes,
                    calls: n as u64,
                    blocks_per_call: *blocks as f64 / n,
                    bytes_per_call: *bytes as f64 / n,
                },
            );
        }
        map
    }

    // Window-only probe attribution = final snapshot - pre-window snapshot.
    let measured_probes_map = {
        let mut window = alloc::Registry::new();
        for (name, (fb, fby, fc)) in probes_final {
            let before = probes_before_window.get(name).copied().unwrap_or((0, 0, 0));
            window.insert(
                name,
                (
                    fb.saturating_sub(before.0),
                    fby.saturating_sub(before.1),
                    fc.saturating_sub(before.2),
                ),
            );
        }
        window
    };

    let probe_blocks_in_window: u64 = measured_probes_map.values().map(|(b, _, _)| *b).sum();
    let probe_bytes_in_window: u64 = measured_probes_map.values().map(|(_, by, _)| *by).sum();

    let ticks_f = ticks.max(1) as f64;
    AllocDetail {
        setup: WindowTotals {
            blocks: setup_totals.0,
            bytes: setup_totals.1,
        },
        warmup: WindowTotals {
            blocks: warmup_totals.0 - setup_totals.0,
            bytes: warmup_totals.1 - setup_totals.1,
        },
        measured: MeasuredWindow {
            blocks: measured_totals.0 - warmup_totals.0,
            bytes: measured_totals.1 - warmup_totals.1,
            ticks,
            blocks_per_tick: FrameStats::of(tick_blocks),
            bytes_per_tick: FrameStats::of(tick_bytes),
        },
        probes: probe_map(probes_final, probes_final),
        measured_probes: probe_map(&measured_probes_map, probes_final),
        schedule_residual: WindowTotals {
            blocks: measured_totals.0 - warmup_totals.0 - probe_blocks_in_window,
            bytes: measured_totals.1 - warmup_totals.1 - probe_bytes_in_window,
        },
        live: LiveBytes {
            start: live_start,
            end: live_end,
            drift_bytes_per_tick: (live_end as f64 - live_start as f64) / ticks_f,
        },
    }
}
