//! `cargo xtask perf` — measurement and budget enforcement (ADR 0001).
//!
//! Phase 0 covers artifact sizes and dependency hygiene. Benches, frame
//! timing, memory, and compile-time trends arrive in later phases and are
//! recorded as explicit skips until then (D8.7).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::budgets::{self, Budgets, Level};
use crate::record::{Config, Deps, Env, Git, Record, Skip};

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
         cargo xtask perf check [--budgets <file>] [--profile <name>]"
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

/// Build the measured members under `profile` and gather every Phase-0
/// metric. Human-readable progress goes to stderr; data comes back in the
/// returned record.
fn gather(profile: &str, runner: String) -> Result<Record> {
    let root = workspace_root();

    let budgets_path = root.join("perf").join("budgets.toml");
    let members = if budgets_path.exists() {
        budgets::load(&budgets_path)?.scope.measured_members
    } else {
        // Seeding mode: measure every member except tooling.
        metadata_members(&root)?
            .into_iter()
            .filter(|name| name != "xtask")
            .collect::<Vec<_>>()
    };

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
        skipped: phase0_skips(),
    })
}

/// Metrics deliberately out of Phase-0 scope, each with its reason so
/// nothing is silently missing from the record (ADR 0001, D8.7).
fn phase0_skips() -> Vec<Skip> {
    vec![
        Skip {
            metric: "wasm32 check".to_string(),
            reason: "no wasm target in scope until the first Bevy dependency (ADR 0001 Phase 1)"
                .to_string(),
        },
        Skip {
            metric: "benches (iai/criterion)".to_string(),
            reason: "no benches defined yet (ADR 0001 Phase 2)".to_string(),
        },
        Skip {
            metric: "frame-time/runtime".to_string(),
            reason: "no headless scenes yet (ADR 0001 Phase 2)".to_string(),
        },
        Skip {
            metric: "memory".to_string(),
            reason: "no headless scenes yet (ADR 0001 Phase 2)".to_string(),
        },
        Skip {
            metric: "clean compile time".to_string(),
            reason: "advisory nightly metric (ADR 0001 Phase 3); not measured on PRs".to_string(),
        },
    ]
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
    fn tree_parser_ignores_non_package_lines() {
        assert!(parse_tree_packages("").is_empty());
        assert!(parse_tree_packages("garbage line\nv1.0\n").is_empty());
    }
}
