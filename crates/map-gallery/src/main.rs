//! bw-map-gallery: the fourth gallery — generated tile maps
//! (docs/maps/map-design.md, tier 2). Scenes build a `bw_core::map`
//! map from `(scene, seed)`; the visual surface is the tactical SVG
//! ortho render under the shared ADR 0006 orientation.
//!
//! Headless contract (ADR 0001 D4, ADR 0003 — same flags as the other
//! galleries):
//!
//!   bw-map-gallery --perf-headless --scene <id> [--seed <S>] [--json]
//!                   [--perf-alloc] [--dhat-out <path>] | --perf-scenes
//!
//! A map is static: the pass measures generation — the one real op —
//! and reports the map checksum, tile counts, and terrain mix.
//! `--ticks` is accepted for contract shape and reported as the
//! single generation pass it is. Per-tick work arrives with
//! pathfinding (tier 3); the live viewer with walkers on maps is
//! tier 4.

#[cfg(feature = "perf-alloc")]
use bw_core::alloc_probe as alloc_probe_rt;

/// Counting allocator for the perf-alloc build (same declaration as
/// the other galleries: each measurement binary owns its allocator
/// choice).
#[cfg(feature = "perf-alloc")]
#[global_allocator]
static ALLOC: alloc_probe_rt::dhat::Alloc = alloc_probe_rt::dhat::Alloc;

use std::hint::black_box;
use std::time::Instant;

#[cfg(feature = "perf-alloc")]
use bw_core::gallery::AllocStats;
use bw_core::gallery::{FrameStats, PerfReport};

use bw_map_gallery::{SCENES, build_scene, scene_preset, summarize, svg};

const DEFAULT_SEED: u64 = 42;

struct Args {
    scene: String,
    ticks: u64,
    seed: u64,
    json: bool,
    alloc_pass: bool,
    dhat_out: Option<String>,
    render: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        scene: String::new(),
        ticks: 1,
        seed: DEFAULT_SEED,
        json: false,
        alloc_pass: false,
        dhat_out: None,
        render: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--perf-headless" => {}
            "--perf-scenes" => {
                let ids: Vec<&str> = SCENES.iter().map(|(id, _)| *id).collect();
                println!("{}", ids.join(" "));
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
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if args.scene.is_empty() {
        args.scene = "meadow".to_string();
    }
    if scene_preset(&args.scene).is_none() {
        return Err(format!("unknown scene: {} (try --perf-scenes)", args.scene));
    }
    Ok(args)
}

/// Profiler output path, scanned from the raw args before parsing so
/// the profiler can start before parse-time allocations (same
/// rationale as the other galleries).
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
            eprintln!("bw-map-gallery: {err}");
            eprintln!(
                "usage: bw-map-gallery --perf-headless [--scene <id>] [--seed <n>] \
                 [--dhat-out <path>] [--json] [--perf-alloc] | --perf-scenes\n\
                 \x20        visual output: --render <map.svg>"
            );
            std::process::exit(2);
        }
    };

    let Some(params) = scene_preset(&args.scene) else {
        eprintln!("bw-map-gallery: unknown scene {}", args.scene);
        std::process::exit(2);
    };

    // Visual output path: render and exit; it is not a measurement
    // pass.
    if let Some(path) = &args.render {
        let map = build_scene(&params, args.seed);
        let meta = svg::RenderMeta {
            scene: &args.scene,
            seed: args.seed,
        };
        match svg::render(&map, &meta, path) {
            Ok(()) => eprintln!("bw-map-gallery: map written to {path}"),
            Err(err) => {
                eprintln!("bw-map-gallery: {err}");
                std::process::exit(2);
            }
        }
        return;
    }

    // The measured op: generation. One pass, timed.
    let start = Instant::now();
    let map = build_scene(&params, args.seed);
    let gen_ms = black_box(start.elapsed().as_secs_f64() * 1000.0);
    let summary = summarize(&map);

    let mut report = PerfReport {
        scene: args.scene.clone(),
        ticks: 1, // a map is static: one generation pass, whatever --ticks asked
        warmup_ticks: 0,
        seed: args.seed,
        entities: summary.tiles,
        interactions: summary.walkable as u64,
        state_checksum: format!("{:016x}", summary.checksum),
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
        }
        #[cfg(not(feature = "perf-alloc"))]
        {
            eprintln!("bw-map-gallery: --perf-alloc requires building with --features perf-alloc");
            std::process::exit(2);
        }
    } else {
        report.frame_ms = Some(FrameStats::of(black_box(vec![gen_ms])));
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
