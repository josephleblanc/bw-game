//! bw-walker-gallery: the third gallery — walking characters, the first
//! built on a real 3D environment: world-space poses from
//! `bw_core::character` (engine-agnostic, allocate-once), the
//! [`CharacterMovementPlugin`] from this crate's library driving the ECS
//! side, and two visual surfaces reading identical data — the offline
//! SVG render through a perspective 3D camera (default build, ADR 0002
//! keeps the renderer out) and the live 3D viewer behind the dev-only
//! `viewer` cargo feature.
//!
//! Headless contract (ADR 0001 D4, ADR 0003 — same flags as `bw-demo`
//! and `bw-tree-gallery`):
//!
//!   bw-walker-gallery --perf-headless --scene <id> --ticks <N> --seed <S>
//!                     [--dhat-out <path>] --json | --perf-scenes
//!
//! The harness drives the plugin itself: one measured tick is one
//! `FixedUpdate` run (the sim's fixed `SIM_DT` step, run directly — the
//! harness *is* the tick contract's interactive boundary) plus one
//! `app.update()` (the render mirror). The wall clock never enters the
//! sim, so `(scene, seed, ticks)` reproduces the checksum exactly.

#[cfg(feature = "perf-alloc")]
use bw_core::alloc_probe as alloc_probe_rt;

#[cfg(feature = "perf-alloc")]
use bw_core::gallery::alloc_detail;

/// Counting allocator for the perf-alloc build (same declaration as
/// bw-demo / bw-tree-gallery: each measurement binary owns its
/// allocator choice).
#[cfg(feature = "perf-alloc")]
#[global_allocator]
static ALLOC: alloc_probe_rt::dhat::Alloc = alloc_probe_rt::dhat::Alloc;

use std::hint::black_box;
use std::time::Instant;

use bevy::app::FixedUpdate;
use bevy::prelude::*;
#[cfg(feature = "perf-alloc")]
use bw_core::gallery::AllocStats;
use bw_core::gallery::{FrameStats, PerfReport};
use bw_core::sim::Rng;

use bw_walker_gallery::{
    CharacterMovementPlugin, CirclePath, WalkerSkeleton, spawn_walker, state_checksum,
    total_footfalls,
};

mod mandala;
mod svg;
// walker-playground: the live 3D environment exists only behind the
// dev-only `viewer` cargo feature (bevy windowing/3D are otherwise
// deferred per ADR 0002). mandala-view: the stress-test live scene,
// same gate.
#[cfg(feature = "viewer")]
mod mandala_view;
#[cfg(feature = "viewer")]
mod viewer;

const DEFAULT_TICKS: u64 = 600;
const DEFAULT_SEED: u64 = 42;
const WARMUP_TICKS: u64 = 60;
/// Preview sampling rate for `--render` (SMIL interpolates between
/// frames; the walk cycle is smooth enough at a modest rate).
const RENDER_FPS: u32 = 24;

/// Cruise speeds of the two gaits (m/s) — also the viewer's keyboard
/// targets.
pub const WALK_SPEED: f32 = 1.35;
pub const RUN_SPEED: f32 = 3.6;

/// Scene presets. Every scene is scripted circles: deterministic from
/// `(scene, seed)` alone, shared by the harness, the SVG render, and
/// the viewer's ambient walkers.
pub struct ScenePreset {
    /// Walkers in the scene.
    pub count: usize,
    /// Circle radius and speed ranges (crowd draws seeded values).
    pub radius: (f32, f32),
    pub speed: (f32, f32),
    /// Fixed-speed presets use the same value for both bounds.
    pub fixed: Option<f32>,
    /// Assign carries round-robin (chest / side / none) — the carry
    /// channel's scene.
    pub carry: bool,
}

fn scene_preset(id: &str) -> Option<ScenePreset> {
    match id {
        // One figure walking a 5 m ring at walk cruise speed.
        "walk" => Some(ScenePreset {
            count: 1,
            radius: (5.0, 5.0),
            speed: (WALK_SPEED, WALK_SPEED),
            fixed: Some(WALK_SPEED),
            carry: false,
        }),
        // The same ring at run speed: the gait blend's upper band.
        "walk-run" => Some(ScenePreset {
            count: 1,
            radius: (7.0, 7.0),
            speed: (RUN_SPEED, RUN_SPEED),
            fixed: Some(RUN_SPEED),
            carry: false,
        }),
        // Porters on nested rings, carrying round-robin: chest holds,
        // side holds, and empty-handed — the loaded-walk read, with
        // the shortened stride quickening the cadence.
        "carry" => Some(ScenePreset {
            count: 12,
            radius: (3.0, 9.0),
            speed: (1.1, 1.6),
            fixed: None,
            carry: true,
        }),
        // A crowd of 96 seeded walkers on nested rings — the
        // perf-heavy scene (96 × 13 = 1248 bone segments).
        "crowd" => Some(ScenePreset {
            count: 96,
            radius: (2.5, 12.0),
            speed: (0.9, 2.4),
            fixed: None,
            carry: false,
        }),
        _ => None,
    }
}

/// One walker to spawn: character state plus its scripted circle.
pub struct SpawnSpec {
    pub character: bw_core::character::Character,
    pub path: CirclePath,
}

/// Deterministic per-walker seed (same hop shape as the tree gallery's
/// per-anchor seeds): walker `k` of a scene seeded with `seed` always
/// sees the same rng.
fn walker_seed(seed: u64, k: usize) -> u64 {
    seed.wrapping_add((k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// Build the scene's walkers: positions on seeded rings, headings
/// tangent to them, phases and scales drawn per walker. Shared by the
/// headless harness, the SVG render, and the viewer.
pub fn build_scene(preset: &ScenePreset, seed: u64) -> Vec<SpawnSpec> {
    let mut specs = Vec::with_capacity(preset.count);
    for k in 0..preset.count {
        let mut rng = Rng::new(walker_seed(seed, k));
        let radius = rng.range_f32(preset.radius.0, preset.radius.1);
        let speed = match preset.fixed {
            Some(v) => v,
            None => rng.range_f32(preset.speed.0, preset.speed.1),
        };
        let dir = if rng.next_f32() < 0.5 { 1.0 } else { -1.0 };
        let angle = rng.range_f32(0.0, std::f32::consts::TAU);
        let mut character = bw_core::character::Character::new(
            bw_core::math::Vec3::new(radius * angle.sin(), 0.0, radius * angle.cos()),
            angle + dir * std::f32::consts::FRAC_PI_2,
        );
        character.speed = 0.0; // eased in by the first ticks, like a real start
        character.phase = rng.range_f32(0.0, std::f32::consts::TAU);
        character.scale = rng.range_f32(0.92, 1.08);
        if preset.carry {
            character.carry = match k % 3 {
                0 => bw_core::character::Carry::Chest,
                1 => bw_core::character::Carry::Side,
                _ => bw_core::character::Carry::None,
            };
        }
        specs.push(SpawnSpec {
            character,
            path: CirclePath { radius, speed, dir },
        });
    }
    specs
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
    viewer: bool,
    /// Dev smoke: with `--viewer`, capture two screenshots and exit.
    #[cfg_attr(not(feature = "viewer"), allow(dead_code))]
    viewer_shot: Option<String>,
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
        viewer: false,
        viewer_shot: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--perf-headless" => {}
            "--perf-scenes" => {
                println!("walk walk-run crowd mandala-100 mandala-1k mandala-10k");
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
            "--viewer" => args.viewer = true,
            "--viewer-shot" => {
                let v = iter.next().ok_or("--viewer-shot needs a value")?;
                args.viewer_shot = Some(v);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if args.scene.is_empty() {
        args.scene = "walk".to_string();
    }
    if scene_preset(&args.scene).is_none() && mandala::preset(&args.scene).is_none() {
        return Err(format!("unknown scene: {} (try --perf-scenes)", args.scene));
    }
    Ok(args)
}

/// The scene's spawn specs, dispatching circle scenes and mandala
/// stress scenes (both seeded). One choke point so the harness, the SVG
/// path, and the viewer all lay out identical worlds.
fn scene_specs(scene: &str, seed: u64) -> Vec<SpawnSpec> {
    if let Some(count) = mandala::preset(scene) {
        return mandala::spawn_specs(&mandala::build(count, seed));
    }
    // parse_args already validated the id; "walk" is the fallback preset.
    match scene_preset(scene).or_else(|| scene_preset("walk")) {
        Some(preset) => build_scene(&preset, seed),
        None => Vec::new(),
    }
}

/// Profiler output path, scanned from the raw args before parsing so the
/// profiler can start before parse-time allocations (same rationale as
/// bw-demo / bw-tree-gallery).
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

fn main() {
    #[cfg(feature = "perf-alloc")]
    let _profiler = alloc_probe_rt::init_profiler(&dhat_out_path());

    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("bw-walker-gallery: {err}");
            eprintln!(
                "usage: bw-walker-gallery --perf-headless [--scene <id>] [--ticks <n>] \
                 [--seed <n>] [--dhat-out <path>] [--json] [--perf-alloc] | --perf-scenes\n\
                 \x20        visual output: --render <anim.svg> --render-frame <static.svg>\n\
                 \x20        live 3D environment (dev build): --viewer"
            );
            std::process::exit(2);
        }
    };

    // Mandala stress scenes dispatch on the id everywhere; circle scenes
    // keep their preset (consumed by the viewer's ambient builders).
    if mandala::preset(&args.scene).is_some() {
        if args.viewer {
            #[cfg(feature = "viewer")]
            {
                let count = mandala::preset(&args.scene).unwrap_or(100);
                mandala_view::run(&args.scene, count, args.seed, args.viewer_shot.as_deref());
                return;
            }
            #[cfg(not(feature = "viewer"))]
            {
                eprintln!(
                    "bw-walker-gallery: --viewer needs a build with --features viewer (dev-only)"
                );
                std::process::exit(2);
            }
        }
        if args.render.is_some() || args.render_frame.is_some() {
            render_visuals(&args.scene, args.seed, &args.render, &args.render_frame);
            return;
        }
        run_headless(
            &args.scene,
            args.seed,
            args.ticks,
            args.json,
            args.alloc_pass,
        );
        return;
    }

    // Live 3D environment: hand off to the viewer (feature-gated; the
    // default build has no windowing and answers with a build hint).
    #[cfg(feature = "viewer")]
    if args.viewer {
        viewer::run(&args.scene, args.seed, args.viewer_shot.as_deref());
        return;
    }
    #[cfg(not(feature = "viewer"))]
    if args.viewer {
        eprintln!("bw-walker-gallery: --viewer needs a build with --features viewer (dev-only)");
        std::process::exit(2);
    }

    // Visual output path: render and exit; it is not a measurement pass.
    if args.render.is_some() || args.render_frame.is_some() {
        render_visuals(&args.scene, args.seed, &args.render, &args.render_frame);
        return;
    }

    run_headless(
        &args.scene,
        args.seed,
        args.ticks,
        args.json,
        args.alloc_pass,
    );
}

/// The `--render` / `--render-frame` path (not a measurement pass).
fn render_visuals(scene: &str, seed: u64, render: &Option<String>, render_frame: &Option<String>) {
    if let Some(count) = mandala::preset(scene)
        && count > 400
    {
        eprintln!(
            "bw-walker-gallery: {scene} is too large for the SVG path \
             (self-contained animation files scale with walkers × frames); \
             use the viewer: --viewer"
        );
        std::process::exit(2);
    }
    let skeleton = bw_core::character::Skeleton::humanoid();
    let mut walkers: Vec<svg::RenderWalker> = scene_specs(scene, seed)
        .into_iter()
        .map(|spec| {
            svg::RenderWalker::new(
                spec.character,
                bw_core::character::circle_input(spec.path.radius, spec.path.speed, spec.path.dir),
                &skeleton,
            )
        })
        .collect();
    let meta = svg::RenderMeta {
        scene,
        seed,
        fps: RENDER_FPS,
    };
    if let Some(path) = render {
        match svg::render_animation(&mut walkers, &skeleton, &meta, 2.0, path) {
            Ok(()) => eprintln!("bw-walker-gallery: animation written to {path}"),
            Err(err) => {
                eprintln!("bw-walker-gallery: {err}");
                std::process::exit(2);
            }
        }
    }
    if let Some(path) = render_frame {
        match svg::render_frame(&mut walkers, &skeleton, &meta, 1.25, path) {
            Ok(()) => eprintln!("bw-walker-gallery: frame written to {path}"),
            Err(err) => {
                eprintln!("bw-walker-gallery: {err}");
                std::process::exit(2);
            }
        }
    }
}

/// The headless measurement harness: the plugin, driven directly.
fn run_headless(scene: &str, seed: u64, ticks: u64, json: bool, alloc_pass: bool) {
    let specs = scene_specs(scene, seed);
    let entities = specs.len() * bw_core::character::BONE_COUNT;

    let mut app = App::new();
    app.add_plugins(CharacterMovementPlugin)
        .insert_resource(WalkerSkeleton(bw_core::character::Skeleton::humanoid()));
    {
        let world = app.world_mut();
        let skeleton = world.resource::<WalkerSkeleton>().0.clone();
        for spec in specs {
            let input =
                bw_core::character::circle_input(spec.path.radius, spec.path.speed, spec.path.dir);
            let entity = spawn_walker(world, &skeleton, spec.character, input);
            world.entity_mut(entity).insert(spec.path);
        }
    }

    // Setup window ends here: everything above is one-off scene cost.
    #[cfg(feature = "perf-alloc")]
    let setup_totals = alloc_probe_rt::totals();

    for _ in 0..WARMUP_TICKS {
        tick_once(&mut app);
    }
    #[cfg(feature = "perf-alloc")]
    let warmup_totals = alloc_probe_rt::totals();
    #[cfg(feature = "perf-alloc")]
    let probes_before_window = alloc_probe_rt::registry_snapshot();
    #[cfg(feature = "perf-alloc")]
    let live_start = alloc_probe_rt::live_bytes();

    #[cfg(feature = "perf-alloc")]
    let mut tick_blocks: Vec<f64> = Vec::with_capacity(ticks as usize);
    #[cfg(feature = "perf-alloc")]
    let mut tick_bytes: Vec<f64> = Vec::with_capacity(ticks as usize);
    let mut frame_ms = Vec::with_capacity(ticks as usize);
    for _ in 0..ticks {
        #[cfg(feature = "perf-alloc")]
        let before = alloc_probe_rt::totals();
        let start = Instant::now();
        tick_once(&mut app);
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

    let interactions = total_footfalls(app.world_mut());
    let checksum = state_checksum(app.world_mut());
    let mut report = PerfReport {
        scene: scene.to_string(),
        ticks,
        warmup_ticks: WARMUP_TICKS,
        seed,
        entities,
        interactions,
        state_checksum: format!("{checksum:016x}"),
        frame_ms: None,
        allocs: None,
        alloc_detail: None,
    };

    if alloc_pass {
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
                ticks,
            ));
        }
        #[cfg(not(feature = "perf-alloc"))]
        {
            eprintln!(
                "bw-walker-gallery: --perf-alloc requires building with --features perf-alloc"
            );
            std::process::exit(2);
        }
    } else {
        report.frame_ms = Some(FrameStats::of(black_box(frame_ms)));
    }

    let payload = serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string());
    if json {
        println!("{payload}");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    }
}

/// One measured tick: one fixed sim step (run directly — the harness is
/// the accumulator boundary) plus one render-mirror update. This is the
/// same pair the live viewer paces with `Time<Fixed>`.
fn tick_once(app: &mut App) {
    if app.world_mut().try_run_schedule(FixedUpdate).is_err() {
        eprintln!("bw-walker-gallery: FixedUpdate schedule missing (plugin not added?)");
        std::process::exit(2);
    }
    app.update();
}
