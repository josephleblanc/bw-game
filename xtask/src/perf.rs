//! `cargo xtask perf` — measurement and budget enforcement (ADR 0001).
//!
//! Gate tier: artifact sizes, dependency hygiene, wasm32 checks, iai
//! instruction counts. Trend tier: headless gallery frame times and
//! allocation counts, criterion wall-clock (run manually). Compile-time
//! trends are Phase 3; everything out of scope is recorded as an explicit
//! skip (D8.7).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::budgets::{self, Budgets, Level};
use crate::record::{
    AllocMeasurement, BenchMeasurement, Config, Deps, Env, FrameMeasurement, Git, Record, Skip,
};

/// Headless-contract defaults (ADR 0001, D4; ADR 0003).
const PERF_TICKS: &str = "600";
const PERF_SEED: &str = "42";

pub fn dispatch(args: &[String]) -> Result<u8> {
    let Some((sub, rest)) = args.split_first() else {
        print_usage();
        return Ok(2);
    };
    if sub != "perf" {
        print_usage();
        return Ok(2);
    }
    let Some((cmd, rest)) = rest.split_first() else {
        print_usage();
        return Ok(2);
    };
    match cmd.as_str() {
        "measure" => {
            let flags = parse_flags(rest, &["--profile", "--out", "--runner"])?;
            let record = gather(
                flags.get("--profile").map(String::as_str).unwrap_or("size"),
                flags
                    .get("--runner")
                    .map(String::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(default_runner),
            )?;
            emit(&record, flags.get("--out").map(PathBuf::from).as_deref())?;
            Ok(0)
        }
        "check" => {
            let flags = parse_flags(rest, &["--budgets", "--profile"])?;
            let budgets_path = flags
                .get("--budgets")
                .map(PathBuf::from)
                .unwrap_or_else(|| workspace_root().join("perf").join("budgets.toml"));
            let loaded = budgets::load(&budgets_path)?;
            let record = gather(
                flags.get("--profile").map(String::as_str).unwrap_or("size"),
                default_runner(),
            )?;
            Ok(report_findings(&loaded, &record, &budgets_path))
        }
        "attribute" => {
            let flags = parse_flags(rest, &["--top", "--profile"])?;
            let top: usize = flags
                .get("--top")
                .and_then(|v| v.parse().ok())
                .unwrap_or(15);
            attribute(
                top,
                flags.get("--profile").map(String::as_str).unwrap_or("size"),
            )?;
            Ok(0)
        }
        _ => {
            print_usage();
            Ok(2)
        }
    }
}

fn print_usage() {
    eprintln!(
        "usage:\n  \
         cargo xtask perf measure [--profile <name>] [--out <file>] [--runner <name>]\n  \
         cargo xtask perf check [--budgets <file>] [--profile <name>]\n  \
         cargo xtask perf attribute [--top <n>] [--profile <name>]"
    );
}

fn parse_flags(args: &[String], allowed: &[&str]) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        if !allowed.contains(&flag.as_str()) {
            bail!(
                "unknown flag {flag}; expected one of: {}",
                allowed.join(", ")
            );
        }
        let Some(value) = iter.next() else {
            bail!("flag {flag} requires a value");
        };
        map.insert(flag.clone(), value.clone());
    }
    Ok(map)
}

fn default_runner() -> String {
    std::env::var("PERF_RUNNER").unwrap_or_else(|_| "local".to_string())
}

/// Workspace root, resolved from the compiled-in manifest dir so the
/// commands work from any cwd under the repo.
fn workspace_root() -> PathBuf {
    match Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        Some(root) => root.to_path_buf(),
        None => PathBuf::from("."),
    }
}

fn emit(record: &Record, out: Option<&Path>) -> Result<()> {
    let json = serde_json::to_string_pretty(record).context("failed to serialize record")?;
    match out {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            std::fs::write(path, json + "\n")
                .with_context(|| format!("failed to write {}", path.display()))?;
            eprintln!("record written to {}", path.display());
        }
        None => println!("{json}"),
    }
    Ok(())
}

/// Members whose measurements count: from budgets `[scope]` when present,
/// else every workspace member except tooling.
fn scope_members(root: &Path) -> Result<Vec<String>> {
    let budgets_path = root.join("perf").join("budgets.toml");
    if budgets_path.exists() {
        Ok(budgets::load(&budgets_path)?.scope.measured_members)
    } else {
        // Seeding mode: measure every member except tooling.
        Ok(metadata_members(root)?
            .into_iter()
            .filter(|name| name != "xtask")
            .collect())
    }
}

/// Build the measured members under `profile` and gather every metric in
/// scope. Human-readable progress goes to stderr; data comes back in the
/// returned record.
fn gather(profile: &str, runner: String) -> Result<Record> {
    let root = workspace_root();
    let members = scope_members(&root)?;

    eprintln!("building {} under profile {profile}…", members.join(", "));
    let mut build = vec!["build".to_string(), format!("--profile={profile}")];
    for member in &members {
        build.push("-p".to_string());
        build.push(member.clone());
    }
    let build_strs: Vec<&str> = build.iter().map(String::as_str).collect();
    run_capture(&root, "cargo", &build_strs)?;

    let meta = cargo_metadata(&root)?;
    let artifacts = measure_artifacts(&meta, &members, profile);
    for (name, bytes) in &artifacts {
        eprintln!("artifact {name}: {} bytes", budgets::fmt_num(*bytes));
    }

    let mut pairs = BTreeSet::new();
    for member in &members {
        let tree = run_capture(
            &root,
            "cargo",
            &[
                "tree",
                &format!("-p{member}"),
                "-e",
                "normal",
                "--prefix",
                "none",
            ],
        )?;
        pairs.extend(parse_tree_packages(&tree));
    }
    let member_names: BTreeSet<&str> = meta.packages.iter().map(|p| p.name.as_str()).collect();
    let external: BTreeSet<_> = pairs
        .iter()
        .filter(|(name, _)| !member_names.contains(name.as_str()))
        .cloned()
        .collect();
    let mut duplicates: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, version) in &external {
        duplicates
            .entry(name.clone())
            .or_default()
            .push(version.clone());
    }
    duplicates.retain(|_, versions| versions.len() > 1);
    let dup_count = duplicates.len();
    eprintln!(
        "deps: {} external (normal), {dup_count} duplicated name(s)",
        external.len()
    );

    let mut wasm = BTreeMap::new();
    let mut wasm_skip: Option<String> = None;
    for member in &members {
        match run_check_wasm(&root, member) {
            Ok(passed) => {
                wasm.insert(member.clone(), passed);
            }
            Err(reason) => {
                // Environment-level failure (e.g. target not installed):
                // record the skip once, don't pretend members failed.
                wasm_skip = Some(reason);
                wasm.clear();
                break;
            }
        }
    }
    if wasm.is_empty() {
        eprintln!(
            "wasm32 check: skipped ({})",
            wasm_skip.clone().unwrap_or_default()
        );
    } else {
        eprintln!("wasm32 check: {} member(s) checked", wasm.len());
    }

    // iai-callgrind benches (gate tier): instruction counts.
    let mut benches = BTreeMap::new();
    let mut bench_skip: Option<String> = None;
    let iai_targets: Vec<(String, String)> = meta
        .packages
        .iter()
        .filter(|p| members.contains(&p.name))
        .flat_map(|p| {
            p.targets
                .iter()
                .filter(|t| t.kind.iter().any(|k| k == "bench") && t.name.starts_with("iai"))
                .map(move |t| (p.name.clone(), t.name.clone()))
        })
        .collect();
    if iai_targets.is_empty() {
        bench_skip = Some(String::from("no iai bench targets defined"));
    }
    for (member, bench_name) in &iai_targets {
        eprintln!("iai bench {bench_name} ({member}) under valgrind…");
        match run_capture(
            &root,
            "cargo",
            &[
                "bench",
                "--profile",
                "runtime",
                "-p",
                member,
                "--bench",
                bench_name,
            ],
        ) {
            Ok(out) => {
                for (id, count) in parse_iai(&out) {
                    benches.insert(
                        id,
                        BenchMeasurement {
                            instructions: count,
                        },
                    );
                }
            }
            Err(err) => {
                bench_skip = Some(format!(
                    "iai bench run failed (is valgrind installed?): {err:#}"
                ));
                break;
            }
        }
    }
    eprintln!("benches: {} iai bench(es) measured", benches.len());

    // Headless gallery passes (trend tier): timing pass on the default
    // build, allocation pass on the perf-alloc build (ADR 0003, D8.8).
    let mut runtime = BTreeMap::new();
    let mut memory = BTreeMap::new();
    let mut runtime_skip: Option<String> = Some(String::from(
        "no gallery binaries with a headless contract found",
    ));
    let mut memory_skip: Option<String> = Some(String::from("no gallery binaries discovered"));
    let bins = member_bins(&meta, &members);
    for (member, bin) in &bins {
        run_capture(
            &root,
            "cargo",
            &["build", "--profile", "runtime", "-p", member],
        )?;
        let bin_path = meta.target_directory.join("runtime").join(bin);
        let Some(bin_str) = bin_path.to_str() else {
            continue;
        };
        let Ok(scenes) = run_capture(&root, bin_str, &["--perf-scenes"]) else {
            continue; // not a gallery binary
        };
        let scenes: Vec<&str> = scenes.split_whitespace().collect();
        if scenes.is_empty() {
            continue;
        }
        runtime_skip = None;
        for scene in &scenes {
            let out = run_capture(
                &root,
                bin_str,
                &[
                    "--perf-headless",
                    "--scene",
                    scene,
                    "--ticks",
                    PERF_TICKS,
                    "--seed",
                    PERF_SEED,
                    "--json",
                ],
            )?;
            if let Some(measured) = parse_perf_json(&out).and_then(|v| frame_measurement(&v)) {
                runtime.insert(format!("{member}::{scene}"), measured);
            }
        }
        eprintln!("runtime: {} scene(s) timed", runtime.len());

        // Allocation pass: separate build with the perf-alloc feature so
        // the counting allocator never perturbs the timing numbers above.
        match run_capture(
            &root,
            "cargo",
            &[
                "build",
                "--profile",
                "runtime",
                "-p",
                member,
                "--features",
                "perf-alloc",
            ],
        ) {
            Ok(_) => {
                memory_skip = None;
                for scene in &scenes {
                    let out = run_capture(
                        &root,
                        bin_str,
                        &[
                            "--perf-headless",
                            "--scene",
                            scene,
                            "--ticks",
                            PERF_TICKS,
                            "--seed",
                            PERF_SEED,
                            "--perf-alloc",
                            "--json",
                        ],
                    )?;
                    if let Some(measured) =
                        parse_perf_json(&out).and_then(|v| alloc_measurement(&v))
                    {
                        memory.insert(format!("{member}::{scene}"), measured);
                    }
                }
                eprintln!("memory: {} scene(s) counted", memory.len());
            }
            Err(err) => {
                memory_skip = Some(format!("perf-alloc build failed: {err:#}"));
            }
        }
    }

    if runtime.is_empty() && runtime_skip.is_none() {
        runtime_skip = Some(String::from(
            "gallery scenes ran but no timing reports parsed",
        ));
    }
    if memory.is_empty() && memory_skip.is_none() {
        memory_skip = Some(String::from("gallery alloc pass ran but no reports parsed"));
    }
    let skipped = build_skips(
        wasm_skip.as_deref(),
        bench_skip.as_deref().filter(|_| benches.is_empty()),
        runtime_skip.as_deref().filter(|_| runtime.is_empty()),
        memory_skip.as_deref().filter(|_| memory.is_empty()),
    );

    Ok(Record {
        schema: 1,
        unix_time: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        runner,
        git: git_info(&root),
        env: env_info(&root),
        config: Config {
            profile: profile.to_string(),
            features: vec![],
            target: host_triple(&root).unwrap_or_else(|| "unknown".to_string()),
        },
        artifacts,
        deps: Deps {
            external_normal: external.len() as u64,
            duplicates,
        },
        wasm,
        benches,
        runtime,
        memory,
        skipped,
    })
}

/// `cargo check --target wasm32-unknown-unknown -p <member>`: Ok(passed)
/// distinguishes compile results; Err carries an environment-level reason
/// (recorded as a skip, not a failure).
fn run_check_wasm(root: &Path, member: &str) -> Result<bool, String> {
    let output = std::process::Command::new("cargo")
        .args(["check", "--target", "wasm32-unknown-unknown", "-p", member])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to spawn cargo: {e}"))?;
    if output.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("not installed") || stderr.contains("may not be installed") {
        return Err(String::from(
            "wasm32-unknown-unknown std target not installed (rustup target add)",
        ));
    }
    Ok(false)
}

/// Metrics not measured by this run, each with its reason so nothing is
/// silently missing from the record (ADR 0001, D8.7).
fn build_skips(
    wasm: Option<&str>,
    benches: Option<&str>,
    runtime: Option<&str>,
    memory: Option<&str>,
) -> Vec<Skip> {
    let mut skips = Vec::new();
    if let Some(reason) = wasm {
        skips.push(Skip {
            metric: "wasm32 check".to_string(),
            reason: reason.to_string(),
        });
    }
    if let Some(reason) = benches {
        skips.push(Skip {
            metric: "benches (iai/criterion)".to_string(),
            reason: reason.to_string(),
        });
    }
    if let Some(reason) = runtime {
        skips.push(Skip {
            metric: "frame-time/runtime".to_string(),
            reason: reason.to_string(),
        });
    }
    if let Some(reason) = memory {
        skips.push(Skip {
            metric: "memory".to_string(),
            reason: reason.to_string(),
        });
    }
    skips.push(Skip {
        metric: "clean compile time".to_string(),
        reason: "advisory nightly metric (ADR 0001 Phase 3); not measured on PRs".to_string(),
    });
    skips
}

/// Attribution report (ADR 0001, D6): size and monomorphization contributors
/// per member, archived under target/perf/attribution/. Informational only —
/// regressions arrive with named causes here, but nothing gates on it.
fn attribute(top: usize, profile: &str) -> Result<()> {
    let root = workspace_root();
    let members = scope_members(&root)?;
    let meta = cargo_metadata(&root)?;
    let out_dir = root.join("target").join("perf").join("attribution");
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    for member in &members {
        let has_bin = meta.packages.iter().any(|p| {
            p.name == *member && p.targets.iter().any(|t| t.kind.iter().any(|k| k == "bin"))
        });
        if has_bin && tool_available("cargo-bloat") {
            let log = out_dir.join(format!("{member}.bloat.log"));
            eprintln!("cargo bloat --crates for {member} (profile {profile})…");
            match run_capture(
                &root,
                "cargo",
                &[
                    "bloat",
                    &format!("--profile={profile}"),
                    "--crates",
                    "-n",
                    &top.to_string(),
                    &format!("-p{member}"),
                ],
            ) {
                Ok(out) => {
                    std::fs::write(&log, &out)
                        .with_context(|| format!("failed to write {}", log.display()))?;
                    println!("== bloat (crates) [{member}] -> {}", log.display());
                    println!("{out}");
                }
                Err(err) => println!("FAIL  bloat [{member}]: {err:#}"),
            }
        } else if has_bin {
            println!(
                "SKIP  bloat [{member}]: cargo-bloat not installed (cargo install cargo-bloat)"
            );
        }

        if tool_available("cargo-llvm-lines") {
            let log = out_dir.join(format!("{member}.llvm-lines.log"));
            eprintln!("cargo llvm-lines for {member}…");
            match run_capture(
                &root,
                "cargo",
                &["llvm-lines", "--profile", profile, &format!("-p{member}")],
            ) {
                Ok(out) => {
                    std::fs::write(&log, &out)
                        .with_context(|| format!("failed to write {}", log.display()))?;
                    println!("== llvm-lines [{member}] -> {} (top {top})", log.display());
                    for line in out.lines().take(top + 1) {
                        println!("{line}");
                    }
                }
                Err(err) => println!("FAIL  llvm-lines [{member}]: {err:#}"),
            }
        } else {
            println!(
                "SKIP  llvm-lines [{member}]: cargo-llvm-lines not installed (cargo install cargo-llvm-lines)"
            );
        }
    }
    Ok(())
}

fn tool_available(name: &str) -> bool {
    let Ok(path_var) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join(name).exists())
}

fn report_findings(loaded: &Budgets, record: &Record, budgets_path: &Path) -> u8 {
    println!(
        "perf budgets: {} (reviewed {})",
        budgets_path.display(),
        loaded.reviewed
    );
    let findings = budgets::compare(loaded, record);
    for finding in &findings {
        let tag = match finding.level {
            Level::Ok => "OK   ",
            Level::Warn => "WARN ",
            Level::Breach => "BREACH",
        };
        println!("{tag} {}", finding.text);
    }
    println!(
        "advisory: clean-check growth threshold {}% (trend-only, never a gate)",
        loaded.advisory.build.check_clean_growth_pct
    );
    let breaches = findings.iter().filter(|f| f.level == Level::Breach).count();
    if breaches == 0 {
        let warns = findings.iter().filter(|f| f.level == Level::Warn).count();
        println!("PASS ({} findings, {warns} warnings)", findings.len());
        0
    } else {
        println!("FAIL ({} breaches)", breaches);
        1
    }
}

// ---- cargo metadata -------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Metadata {
    target_directory: PathBuf,
    packages: Vec<Package>,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    targets: Vec<Target>,
}

#[derive(Debug, Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
}

fn cargo_metadata(root: &Path) -> Result<Metadata> {
    let out = run_capture(
        root,
        "cargo",
        &["metadata", "--no-deps", "--format-version", "1"],
    )?;
    serde_json::from_str(&out).context("failed to parse `cargo metadata` output")
}

fn metadata_members(root: &Path) -> Result<Vec<String>> {
    Ok(cargo_metadata(root)?
        .packages
        .into_iter()
        .map(|p| p.name)
        .collect())
}

fn measure_artifacts(meta: &Metadata, members: &[String], profile: &str) -> BTreeMap<String, u64> {
    let profile_dir = meta.target_directory.join(profile);
    let mut artifacts = BTreeMap::new();
    for package in &meta.packages {
        if !members.contains(&package.name) {
            continue;
        }
        for target in &package.targets {
            if target.kind.iter().any(|k| k == "bin") {
                let path = profile_dir.join(&target.name);
                if let Ok(file) = std::fs::metadata(&path) {
                    artifacts.insert(target.name.clone(), file.len());
                }
            }
        }
    }
    artifacts
}

/// Parse `cargo tree --prefix none` output: one "name vX.Y.Z" per line,
/// possibly followed by "(proc-macro)", "(/path)", or "(*)" markers.
fn parse_tree_packages(tree: &str) -> BTreeSet<(String, String)> {
    tree.lines()
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            let name = tokens.next()?;
            let version = tokens.next()?.strip_prefix('v')?;
            Some((name.to_string(), version.to_string()))
        })
        .collect()
}

/// Parse iai-callgrind output: a non-indented bench id line followed by
/// indented stat lines; we keep the "Instructions:" count per bench.
fn parse_iai(out: &str) -> BTreeMap<String, u64> {
    let mut map = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !line.starts_with(' ') {
            current = Some(trimmed.to_string());
        } else if let Some(id) = &current
            && let Some(rest) = trimmed.strip_prefix("Instructions:")
        {
            // Comparison runs print "current|baseline (pct) [factor]":
            // keep only the current value (left of the pipe).
            let value_part = rest.split('|').next().unwrap_or(rest);
            let digits: String = value_part.chars().filter(|c| c.is_ascii_digit()).collect();
            if let Ok(count) = digits.parse() {
                map.insert(id.clone(), count);
            }
        }
    }
    map
}

/// Gallery binaries report JSON on stdout, possibly after other output;
/// parse from the first '{'.
fn parse_perf_json(out: &str) -> Option<serde_json::Value> {
    let start = out.find('{')?;
    serde_json::from_str(&out[start..]).ok()
}

fn frame_measurement(v: &serde_json::Value) -> Option<FrameMeasurement> {
    let f = v.get("frame_ms")?;
    Some(FrameMeasurement {
        frame_ms_min: f.get("min")?.as_f64()?,
        frame_ms_p50: f.get("p50")?.as_f64()?,
        frame_ms_p90: f.get("p90")?.as_f64()?,
        frame_ms_p99: f.get("p99")?.as_f64()?,
        frame_ms_max: f.get("max")?.as_f64()?,
        entities: v.get("entities")?.as_u64()?,
        ticks: v.get("ticks")?.as_u64()?,
        interactions: v.get("interactions")?.as_u64()?,
    })
}

fn alloc_measurement(v: &serde_json::Value) -> Option<AllocMeasurement> {
    let a = v.get("allocs")?;
    let steady = v
        .pointer("/alloc_detail/measured/blocks_per_tick")
        .and_then(|b| {
            Some(crate::record::SteadyTicks {
                p50: b.get("p50")?.as_f64()?,
                max: b.get("max")?.as_f64()?,
            })
        });
    Some(AllocMeasurement {
        allocs: a.get("count")?.as_u64()?,
        peak_bytes: a.get("peak_bytes")?.as_u64()?,
        steady_blocks_per_tick: steady,
    })
}

/// (member, bin-name) for every scope member with a binary target.
fn member_bins(meta: &Metadata, members: &[String]) -> Vec<(String, String)> {
    let mut bins = Vec::new();
    for package in &meta.packages {
        if !members.contains(&package.name) {
            continue;
        }
        for target in &package.targets {
            if target.kind.iter().any(|k| k == "bin") {
                bins.push((package.name.clone(), target.name.clone()));
            }
        }
    }
    bins
}

// ---- environment ----------------------------------------------------------

fn run_capture(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let display = format!("{program} {}", args.join(" "));
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to spawn `{display}`"))?;
    if !output.status.success() {
        bail!(
            "`{display}` failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim_end()
        );
    }
    Ok(String::from_utf8(output.stdout)
        .context("command output was not valid UTF-8")?
        .trim_end()
        .to_string())
}

fn git_info(root: &Path) -> Git {
    let sha = run_capture(root, "git", &["rev-parse", "HEAD"]).unwrap_or_else(|_| "unknown".into());
    let branch = run_capture(root, "git", &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_else(|_| "unknown".into());
    let dirty = run_capture(root, "git", &["status", "--porcelain"])
        .map(|out| !out.is_empty())
        .unwrap_or(true);
    let commit_time = run_capture(root, "git", &["log", "-1", "--format=%cI"])
        .unwrap_or_else(|_| "unknown".into());
    Git {
        sha,
        branch,
        dirty,
        commit_time,
    }
}

fn host_triple(root: &Path) -> Option<String> {
    let vv = run_capture(root, "rustc", &["-vV"]).ok()?;
    vv.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "host").then(|| value.trim().to_string())
    })
}

fn env_info(root: &Path) -> Env {
    Env {
        rustc: run_capture(root, "rustc", &["-V"]).unwrap_or_else(|_| "unknown".into()),
        host: host_triple(root).unwrap_or_else(|| "unknown".into()),
        cpu: cpu_model(),
        cores: std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(0),
        kernel: kernel_version(),
    }
}

fn cpu_model() -> String {
    let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") else {
        return "unknown".to_string();
    };
    for line in info.lines() {
        if line.starts_with("model name")
            && let Some((_, value)) = line.split_once(':')
        {
            return value.trim().to_string();
        }
    }
    "unknown".to_string()
}

fn kernel_version() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_parser_extracts_name_version_pairs() {
        let tree = "\
bw-demo v0.1.0 (/home/u/bw-game/crates/demo)
anyhow v1.0.104
bw-core v0.1.0 (/home/u/bw-game/crates/core)
serde v1.0.229
serde_derive v1.0.229 (proc-macro)
proc-macro2 v1.0.107
proc-macro2 v1.0.107 (*)
serde v1.0.200
";
        let pairs = parse_tree_packages(tree);
        assert_eq!(pairs.len(), 7);
        assert!(pairs.contains(&("serde".to_string(), "1.0.229".to_string())));
        assert!(pairs.contains(&("serde".to_string(), "1.0.200".to_string())));
        assert!(pairs.contains(&("serde_derive".to_string(), "1.0.229".to_string())));
        assert!(pairs.contains(&("bw-demo".to_string(), "0.1.0".to_string())));
    }

    #[test]
    fn iai_parser_extracts_instruction_counts() {
        let out = "iai_sim::sim::step step_1000:setup_sim_1000()
  Instructions:                      13005|13005                (No change)
  L1 Hits:                           15282|15282                (No change)
iai_sim::sim::hash_build build_1000:setup_sim_1000()
  Instructions:                     646330|646409               (-0.01222%) [-1.00012x]
iai_sim::sim::hash_query query_1000:setup_hash_1000()
  Instructions:                     1491128|1490286              (+0.05650%) [+1.00056x]
Iai-Callgrind result: Ok. 3 without regressions
";
        let map = parse_iai(out);
        assert_eq!(map.len(), 3);
        assert_eq!(
            map.get("iai_sim::sim::step step_1000:setup_sim_1000()"),
            Some(&13_005)
        );
        assert_eq!(
            map.get("iai_sim::sim::hash_build build_1000:setup_sim_1000()"),
            Some(&646_330)
        );
        assert_eq!(
            map.get("iai_sim::sim::hash_query query_1000:setup_hash_1000()"),
            Some(&1_491_128)
        );
    }

    #[test]
    fn tree_parser_ignores_non_package_lines() {
        assert!(parse_tree_packages("").is_empty());
        assert!(parse_tree_packages("garbage line\nv1.0\n").is_empty());
    }
}
