//! bw-tree-gallery: the second gallery — procedurally generated
//! golden-ratio trees ("semi-fractal": golden-angle forks with seeded
//! jitter) growing and swaying in the wind, viewed through the fixed
//! tilted camera of PS2-era tactical RPGs (Final Fantasy Tactics,
//! Disgaea).
//!
//! Headless contract (ADR 0001 D4, ADR 0003 — same flags as `bw-demo`):
//!
//!   bw-tree-gallery --perf-headless --scene <id> --ticks <N> --seed <S>
//!                    [--dhat-out <path>] --json | --perf-scenes
//!
//! Simulation lives in `bw_core::tree` (engine-agnostic, allocate-once);
//! this binary owns the Bevy ECS glue: a `step_trees` system advancing the
//! clock and posing every tree, and a `sync_segments` system materializing
//! per-segment components for the renderer-to-be. The visual surface today
//! is `--render <file>` / `--render-frame <file>`: a self-contained
//! animated SVG in the tactical-camera projection (see `proj` and `svg`)
//! — the real renderer stays deferred per ADR 0002, so no Bevy render
//! features are enabled for this.

use alloc_probe::alloc_probe;

#[cfg(feature = "perf-alloc")]
use bw_core::alloc_probe as alloc_probe_rt;

#[cfg(feature = "perf-alloc")]
use bw_core::gallery::alloc_detail;

/// Counting allocator for the perf-alloc build (same declaration as
/// bw-demo: each measurement binary owns its allocator choice).
#[cfg(feature = "perf-alloc")]
#[global_allocator]
static ALLOC: alloc_probe_rt::dhat::Alloc = alloc_probe_rt::dhat::Alloc;

use std::hint::black_box;
use std::time::Instant;

use bevy::prelude::*;
#[cfg(feature = "perf-alloc")]
use bw_core::gallery::AllocStats;
use bw_core::gallery::{FrameStats, PerfReport};
use bw_core::time::SIM_DT;
use bw_core::tree::{CALM_WIND, Tree, TreeParams, TreePose, WindParams};

mod proj;
mod svg;

const DEFAULT_TICKS: u64 = 600;
const DEFAULT_SEED: u64 = 42;
const WARMUP_TICKS: u64 = 60;
/// Preview sampling rate for `--render` (SMIL interpolates between frames,
/// so a modest rate stays smooth for slow growth and gentle sway).
const RENDER_FPS: u32 = 15;

/// Scene presets: parameters, wind, and tile anchors for the trees.
/// `tile_radius` is the checkerboard ring drawn around each anchor.
struct ScenePreset {
    params: TreeParams,
    wind: WindParams,
    anchors: &'static [(i32, i32)],
    tile_radius: i32,
}

const SINGLE_ANCHOR: [(i32, i32); 1] = [(0, 0)];
const GROVE_ANCHORS: [(i32, i32); 9] = [
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, -1),
    (0, 0),
    (0, 1),
    (1, -1),
    (1, 0),
    (1, 1),
];

fn scene_preset(id: &str) -> Option<ScenePreset> {
    match id {
        // A single oak: straight trunk, golden-angle forks, gentle breeze.
        // Radius-2 tile ring so the cast shadow stays on the checkerboard.
        "tree" => Some(ScenePreset {
            params: TreeParams::oak(),
            wind: CALM_WIND,
            anchors: &SINGLE_ANCHOR,
            tile_radius: 2,
        }),
        // Same tree in a storm: wide, fast sway.
        "tree-storm" => Some(ScenePreset {
            params: TreeParams::oak(),
            wind: WindParams {
                amp_rad: 0.34,
                freq_scale: 1.8,
            },
            anchors: &SINGLE_ANCHOR,
            tile_radius: 2,
        }),
        // A 3x3 grove of younger/smaller trees across the checkerboard.
        "tree-grove" => Some(ScenePreset {
            params: TreeParams {
                trunk_len: 70.0,
                trunk_width: 7.0,
                max_depth: 6,
                ..TreeParams::oak()
            },
            wind: WindParams {
                amp_rad: 0.12,
                freq_scale: 0.9,
            },
            anchors: &GROVE_ANCHORS,
            tile_radius: 1,
        }),
        _ => None,
    }
}

/// A built scene: generated trees with their (reused) pose buffers. The
/// seed sequence is deterministic per anchor index, so the same
/// `(scene, seed)` always builds the same grove.
struct BuiltScene {
    trees: Vec<Tree>,
    poses: Vec<TreePose>,
    anchors: Vec<(i32, i32)>,
    segments: usize,
    leaves: u64,
    growth_end: f32,
}

fn build_scene(preset: &ScenePreset, seed: u64) -> BuiltScene {
    let mut trees = Vec::with_capacity(preset.anchors.len());
    let mut poses = Vec::with_capacity(preset.anchors.len());
    let mut anchors = Vec::with_capacity(preset.anchors.len());
    let mut segments = 0;
    let mut leaves = 0;
    let mut growth_end = 0.0f32;
    for (k, &(ti, tj)) in preset.anchors.iter().enumerate() {
        let tree_seed = seed.wrapping_add((k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        // Trees live in their own local plane (base 0,0): the ground anchor
        // enters only through the billboard projection at render time, so
        // ECS segments stay tree-plane local.
        let tree = Tree::generate(&preset.params, tree_seed);
        segments += tree.node_count();
        leaves += tree.leaf_count();
        growth_end = growth_end.max(tree.growth_end());
        poses.push(TreePose::new(&tree));
        trees.push(tree);
        anchors.push((ti, tj));
    }
    BuiltScene {
        trees,
        poses,
        anchors,
        segments,
        leaves,
        growth_end,
    }
}

struct Args {
    scene: String,
    ticks: u64,
    seed: u64,
    json: bool,
    alloc_pass: bool,
    dhat_out: Option<String>,
    render: Option<String>,
    render_frame: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        scene: String::new(),
        ticks: DEFAULT_TICKS,
        seed: DEFAULT_SEED,
        json: false,
        alloc_pass: false,
        dhat_out: None,
        render: None,
        render_frame: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--perf-headless" => {}
            "--perf-scenes" => {
                println!("tree tree-storm tree-grove");
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
            "--render" => {
                let v = iter.next().ok_or("--render needs a value")?;
                args.render = Some(v);
            }
            "--render-frame" => {
                let v = iter.next().ok_or("--render-frame needs a value")?;
                args.render_frame = Some(v);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if args.scene.is_empty() {
        args.scene = "tree".to_string();
    }
    if scene_preset(&args.scene).is_none() {
        return Err(format!("unknown scene: {} (try --perf-scenes)", args.scene));
    }
    Ok(args)
}

/// Profiler output path, scanned from the raw args before parsing so the
/// profiler can start before parse-time allocations (same rationale as
/// bw-demo).
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

/// Clock advanced by the sim itself — never Bevy's wall-clock `Time` — so
/// runs are reproducible from `(scene, seed, ticks)` alone.
#[derive(Resource)]
struct SimClock {
    t: f32,
    dt: f32,
}

#[derive(Resource)]
struct GroveState {
    trees: Vec<Tree>,
    /// Reused pose buffers, one per tree (ADR 0004: no per-tick allocation).
    poses: Vec<TreePose>,
    wind: WindParams,
}

/// Mirror of one tree segment in ECS storage: what a renderer would read.
/// Carries its `(tree, node)` indices so sync is order-independent.
#[derive(Component)]
struct Segment {
    tree: u32,
    node: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    width: f32,
}

#[alloc_probe]
fn step_trees(mut clock: ResMut<SimClock>, mut state: ResMut<GroveState>) {
    clock.t += clock.dt;
    let t = clock.t;
    let GroveState { trees, poses, wind } = &mut *state;
    for (tree, pose) in trees.iter().zip(poses.iter_mut()) {
        tree.pose_into(pose, t, wind);
    }
}

#[alloc_probe]
fn sync_segments(state: Res<GroveState>, mut query: Query<&mut Segment>) {
    // Query iteration order is arbitrary in Bevy and does not matter here:
    // every entity reads its own pose slot via its (tree, node) indices,
    // and the state checksum comes from the pose buffers, not components.
    for mut seg in query.iter_mut() {
        let pose = &state.poses[seg.tree as usize];
        let i = seg.node as usize;
        seg.x0 = pose.start_x[i];
        seg.y0 = pose.start_y[i];
        seg.x1 = pose.tip_x[i];
        seg.y1 = pose.tip_y[i];
        seg.width = pose.width[i];
    }
}

fn main() {
    #[cfg(feature = "perf-alloc")]
    let _profiler = alloc_probe_rt::init_profiler(&dhat_out_path());

    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("bw-tree-gallery: {err}");
            eprintln!(
                "usage: bw-tree-gallery --perf-headless [--scene <id>] [--ticks <n>] \
                 [--seed <n>] [--dhat-out <path>] [--json] [--perf-alloc] | --perf-scenes\n\
                 \x20        visual output: --render <anim.svg> --render-frame <static.svg>"
            );
            std::process::exit(2);
        }
    };

    let Some(preset) = scene_preset(&args.scene) else {
        eprintln!("bw-tree-gallery: unknown scene {}", args.scene);
        std::process::exit(2);
    };

    // Visual output path: render and exit; it is not a measurement pass.
    if args.render.is_some() || args.render_frame.is_some() {
        let mut scene = build_scene(&preset, args.seed);
        let meta = svg::RenderMeta {
            scene: &args.scene,
            seed: args.seed,
            fps: RENDER_FPS,
        };
        if let Some(path) = &args.render {
            match svg::render_animation(
                &scene.trees,
                &mut scene.poses,
                &scene.anchors,
                &preset.wind,
                preset.tile_radius,
                &meta,
                path,
            ) {
                Ok(()) => eprintln!("bw-tree-gallery: animation written to {path}"),
                Err(err) => {
                    eprintln!("bw-tree-gallery: {err}");
                    std::process::exit(2);
                }
            }
        }
        if let Some(path) = &args.render_frame {
            let t = scene.growth_end + 0.75; // fully grown, mid-sway
            match svg::render_frame(
                &scene.trees,
                &mut scene.poses,
                &scene.anchors,
                &preset.wind,
                preset.tile_radius,
                &meta,
                t,
                path,
            ) {
                Ok(()) => eprintln!("bw-tree-gallery: frame written to {path}"),
                Err(err) => {
                    eprintln!("bw-tree-gallery: {err}");
                    std::process::exit(2);
                }
            }
        }
        return;
    }

    let scene = build_scene(&preset, args.seed);
    let entities = scene.segments;
    let leaves = scene.leaves;

    let mut app = App::new();
    app.insert_resource(GroveState {
        trees: scene.trees,
        poses: scene.poses,
        wind: preset.wind,
    })
    .insert_resource(SimClock { t: 0.0, dt: SIM_DT })
    .add_systems(Update, (step_trees, sync_segments).chain());

    // Spawn one Segment entity per tree node, carrying its (tree, node)
    // indices so `sync_segments` stays query-order independent.
    let node_counts: Vec<usize> = app
        .world()
        .resource::<GroveState>()
        .trees
        .iter()
        .map(Tree::node_count)
        .collect();
    {
        let world = app.world_mut();
        for (ti, n) in node_counts.iter().enumerate() {
            for node in 0..*n {
                world.spawn(Segment {
                    tree: ti as u32,
                    node: node as u32,
                    x0: 0.0,
                    y0: 0.0,
                    x1: 0.0,
                    y1: 0.0,
                    width: 0.0,
                });
            }
        }
    }

    // Setup window ends here: everything above is one-off scene cost.
    #[cfg(feature = "perf-alloc")]
    let setup_totals = alloc_probe_rt::totals();

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

    let state = app.world().resource::<GroveState>();
    let mut checksum = 0xCBF2_9CE4_8422_2325u64;
    for pose in &state.poses {
        checksum = checksum.wrapping_mul(0x1000_0000_01B3) ^ pose.checksum();
    }
    let mut report = PerfReport {
        scene: args.scene.clone(),
        ticks: args.ticks,
        warmup_ticks: WARMUP_TICKS,
        seed: args.seed,
        entities,
        interactions: leaves,
        state_checksum: format!("{checksum:016x}"),
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
            eprintln!("bw-tree-gallery: --perf-alloc requires building with --features perf-alloc");
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
