//! Trend report rendering (ADR 0001, D1/D5/D9): reads append-only JSONL
//! measurement records — the `perf-data` branch layout, monthly
//! `records-YYYY-MM.jsonl` shards — and emits the markdown that feeds the
//! rolling `perf-review` issue. Pure and unit-tested: CI only checks out,
//! appends records, and posts this output. Comparisons honor D8.1 (only
//! records sharing the latest record's runner/rustc/host are diffed;
//! mismatches are flagged, never silently merged into one trend line).

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::budgets::{self, Budgets, Level};
use crate::record::Record;

/// Deltas beyond these are flagged findings (D9). Instruction counts are
/// deterministic but vary ~0.1% between builds (address-dependent
/// alignment), so they get 1%; wall-clock frame times are the noisy tier
/// and only earn a finding beyond 10% (D2 — nothing stochastic gates).
const FLAG_INSTRUCTIONS_PCT: f64 = 1.0;
const FLAG_FRAME_MS_PCT: f64 = 10.0;
const FLAG_PEAK_BYTES_PCT: f64 = 2.0;

/// Open backlog items older than this (two weekly cycles) are flagged in
/// the report for a schedule/split/close decision (backlog.md contract).
const BACKLOG_FLAG_DAYS: i64 = 14;

/// One open `- [ ]` line from backlog.md.
struct BacklogItem {
    text: String,
    /// Days since the civil epoch, from a trailing `Added YYYY-MM-DD.`
    added_days: Option<i64>,
}

/// Entry point for `cargo xtask perf report`.
pub fn run(records_path: &Path, since: &str) -> Result<()> {
    let records = load_records(records_path)?;
    let Some(newest) = records.last().cloned() else {
        bail!(
            "no records found under {} (expected records-*.jsonl shards)",
            records_path.display()
        );
    };
    let days = parse_since(since)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cutoff = days.map(|d| now.saturating_sub(d * 86_400));
    let mut window: Vec<Record> = records
        .into_iter()
        .filter(|r| cutoff.is_none_or(|c| r.unix_time >= c))
        .collect();
    if window.is_empty() {
        // Nothing inside the window: fall back to the newest record so the
        // report still shows budget state, and say so.
        window = vec![newest];
        eprintln!("perf report: window {since} has no records; reporting latest only");
    }

    let root = crate::perf::workspace_root();
    let budgets_path = root.join("perf").join("budgets.toml");
    let loaded = budgets::load(&budgets_path).ok();
    // backlog.md is optional by design; a missing file shows as a skip,
    // never silently omits the section (D8.7).
    let backlog = std::fs::read_to_string(root.join("backlog.md"))
        .ok()
        .map(|text| parse_backlog(&text));
    let markdown = render(&window, loaded.as_ref(), backlog.as_deref(), now, since);
    println!("{markdown}");
    Ok(())
}

/// `7d`, `30d`, … -> days; `all` -> None. Anything else is an error, never
/// a silent default (D8.7).
fn parse_since(since: &str) -> Result<Option<u64>> {
    if since == "all" {
        return Ok(None);
    }
    let Some(days) = since.strip_suffix('d') else {
        bail!("--since must be <N>d or all, got {since:?}");
    };
    days.parse::<u64>()
        .map(Some)
        .with_context(|| format!("--since days not a number: {days}"))
}

/// Load every `*.jsonl` shard under `records_path` (or the single file
/// itself), sorted by time. Malformed lines fail loudly with file and line.
fn load_records(records_path: &Path) -> Result<Vec<Record>> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if records_path.is_dir() {
        let mut shards: Vec<_> = std::fs::read_dir(records_path)
            .with_context(|| format!("failed to list {}", records_path.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".jsonl"))
            })
            .collect();
        shards.sort();
        files = shards;
    } else if records_path.is_file() {
        files.push(records_path.to_path_buf());
    }
    let mut records = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        for (idx, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: Record = serde_json::from_str(line).with_context(|| {
                format!("{}:{}: malformed record line", file.display(), idx + 1)
            })?;
            records.push(record);
        }
    }
    records.sort_by_key(|r| r.unix_time);
    Ok(records)
}

struct Delta {
    metric: String,
    base: Option<u64>,
    last: Option<u64>,
    /// Flag threshold in percent; None = flag any change (deterministic).
    flag_pct: Option<f64>,
    action: &'static str,
}

/// Render the report: budget state (latest record vs budgets.toml), open
/// backlog items with age (backlog.md), the window's first→last deltas
/// within one environment, flagged findings each paired with a recommended
/// action (D9 invariant), and the latest record's skip log.
fn render(
    window: &[Record],
    budgets: Option<&Budgets>,
    backlog: Option<&[BacklogItem]>,
    now: u64,
    since: &str,
) -> String {
    let latest = &window[window.len() - 1];
    let env_key = |r: &Record| (r.runner.clone(), r.env.rustc.clone(), r.env.host.clone());
    let same_env: Vec<&Record> = window
        .iter()
        .filter(|r| env_key(r) == env_key(latest))
        .collect();
    let baseline = same_env.first().copied();

    let mut out = String::new();
    out.push_str("# perf report\n\n");
    out.push_str(&format!(
        "- window: last {since}; {} record(s) ({} same env as latest)\n",
        window.len(),
        same_env.len()
    ));
    out.push_str(&format!(
        "- env: runner {}, {}, {}\n",
        latest.runner, latest.env.rustc, latest.env.host
    ));
    if let Some(base) = baseline {
        out.push_str(&format!(
            "- range: {} → {} ({})\n",
            truncate_sha(&base.git.sha),
            truncate_sha(&latest.git.sha),
            latest.git.commit_time,
        ));
    } else {
        out.push_str("- range: single record in window\n");
    }
    if same_env.len() < window.len() {
        out.push_str(&format!(
            "- ⚠ {} record(s) excluded from deltas: different runner/rustc/host (D8.1)\n",
            window.len() - same_env.len()
        ));
    }
    out.push('\n');

    if let Some(loaded) = budgets {
        out.push_str("## budgets (latest record vs perf/budgets.toml)\n\n");
        for finding in budgets::compare(loaded, latest) {
            let tag = match finding.level {
                Level::Ok => "ok",
                Level::Warn => "warn",
                Level::Breach => "breach",
            };
            out.push_str(&format!("- [{tag}] {}\n", finding.text));
        }
        out.push('\n');
    } else {
        out.push_str("## budgets\n\n- skipped: perf/budgets.toml not loadable\n\n");
    }

    match backlog {
        Some(items) => {
            out.push_str(&format!(
                "## backlog ({} open, from backlog.md)\n\n",
                items.len()
            ));
            if items.is_empty() {
                out.push_str("- none — no open items\n");
            } else {
                let today = (now / 86_400) as i64;
                for item in items {
                    match item.added_days {
                        Some(added) => {
                            let age = (today - added).max(0);
                            if age > BACKLOG_FLAG_DAYS {
                                out.push_str(&format!(
                                    "- ⚠ {} — added {age}d ago — older than two weekly cycles; action: schedule, split, or close\n",
                                    item.text
                                ));
                            } else {
                                out.push_str(&format!("- {} — added {age}d ago\n", item.text));
                            }
                        }
                        None => out.push_str(&format!(
                            "- {} — no Added date (see backlog.md conventions)\n",
                            item.text
                        )),
                    }
                }
            }
            out.push('\n');
        }
        None => out.push_str("## backlog\n\n- skipped: backlog.md not found at workspace root\n\n"),
    }

    out.push_str("## deltas (first → last in window, same env)\n\n");
    if baseline.is_some_and(|b| b.unix_time != latest.unix_time) {
        let deltas = collect_deltas(baseline.unwrap_or(latest), latest);
        out.push_str("| metric | first | last | Δ |\n|---|---|---|---|\n");
        for d in &deltas {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                d.metric,
                d.base.map(budgets::fmt_num).unwrap_or_else(|| "—".into()),
                d.last.map(budgets::fmt_num).unwrap_or_else(|| "—".into()),
                delta_pct(d),
            ));
        }
        out.push('\n');
        let flagged: Vec<&Delta> = deltas.iter().filter(|d| is_flagged(d)).collect();
        if flagged.is_empty() {
            out.push_str("no flagged findings in window.\n");
        } else {
            out.push_str("### findings (each with a recommended action)\n\n");
            for d in flagged {
                out.push_str(&format!(
                    "- **{}**: {} — action: {}\n",
                    d.metric,
                    delta_pct(d),
                    d.action
                ));
            }
        }
        out.push('\n');
    } else {
        out.push_str("only one record in this window/env — nothing to diff yet.\n\n");
    }

    out.push_str("## skipped metrics (latest record)\n\n");
    if latest.skipped.is_empty() {
        out.push_str("- none\n");
    } else {
        for skip in &latest.skipped {
            out.push_str(&format!("- {}: {}\n", skip.metric, skip.reason));
        }
    }
    out
}

fn collect_deltas(base: &Record, last: &Record) -> Vec<Delta> {
    let mut deltas = Vec::new();
    let action_attr = "run `cargo xtask perf attribute` for size/monomorphization attribution";
    let action_bench = "re-run `cargo bench -p bw-core --bench iai_sim --profile runtime` locally to confirm, then `cargo xtask perf attribute` for pressure";
    let action_frame = "wall-clock tier: re-run `cargo xtask perf measure` on a quiet machine before trusting (D2)";
    let action_alloc = "run `cargo xtask perf dhat` to name the allocating callsite";

    for (name, v) in &base.artifacts {
        deltas.push(Delta {
            metric: format!("artifact {name} bytes"),
            base: Some(*v),
            last: last.artifacts.get(name).copied(),
            flag_pct: Some(0.0), // deterministic: any change is real
            action: action_attr,
        });
    }
    for (name, v) in &last.artifacts {
        if !base.artifacts.contains_key(name) {
            deltas.push(Delta {
                metric: format!("artifact {name} bytes"),
                base: None,
                last: Some(*v),
                flag_pct: Some(0.0),
                action: action_attr,
            });
        }
    }
    deltas.push(Delta {
        metric: "deps external (normal)".to_string(),
        base: Some(base.deps.external_normal),
        last: Some(last.deps.external_normal),
        flag_pct: Some(0.0),
        action: "inspect `cargo tree -d`; dedupe via workspace dependencies",
    });
    for (id, v) in &base.benches {
        deltas.push(Delta {
            metric: format!("bench {id} instructions"),
            base: Some(v.instructions),
            last: last.benches.get(id).map(|m| m.instructions),
            flag_pct: Some(FLAG_INSTRUCTIONS_PCT),
            action: action_bench,
        });
    }
    for (scene, v) in &base.runtime {
        for (label, value) in [("p50", v.frame_ms_p50), ("p99", v.frame_ms_p99)] {
            let last_val = last.runtime.get(scene).map(|m| {
                if label == "p50" {
                    m.frame_ms_p50
                } else {
                    m.frame_ms_p99
                }
            });
            deltas.push(Delta {
                metric: format!("runtime {scene} frame_ms {label} (×1000)"),
                base: Some((value * 1000.0) as u64),
                last: last_val.map(|v| (v * 1000.0) as u64),
                flag_pct: Some(FLAG_FRAME_MS_PCT),
                action: action_frame,
            });
        }
    }
    for (scene, v) in &base.memory {
        deltas.push(Delta {
            metric: format!("memory {scene} total allocs"),
            base: Some(v.allocs),
            last: last.memory.get(scene).map(|m| m.allocs),
            flag_pct: Some(0.0),
            action: action_alloc,
        });
        deltas.push(Delta {
            metric: format!("memory {scene} peak bytes"),
            base: Some(v.peak_bytes),
            last: last.memory.get(scene).map(|m| m.peak_bytes),
            flag_pct: Some(FLAG_PEAK_BYTES_PCT),
            action: action_alloc,
        });
    }
    deltas
}

fn delta_pct(d: &Delta) -> String {
    let (Some(base), Some(last)) = (d.base, d.last) else {
        return "new/removed".to_string();
    };
    if base == 0 {
        return if last == 0 {
            "0%".to_string()
        } else {
            "new".to_string()
        };
    }
    let pct = 100.0 * (last as f64 - base as f64) / base as f64;
    format!("{pct:+.1}%")
}

fn is_flagged(d: &Delta) -> bool {
    let (Some(base), Some(last)) = (d.base, d.last) else {
        return true; // appeared or disappeared: always worth eyes
    };
    if base == 0 {
        return last != 0;
    }
    let pct = 100.0 * (last as f64 - base as f64) / base as f64;
    match d.flag_pct {
        Some(threshold) => pct.abs() > threshold,
        None => last != base,
    }
}

fn truncate_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Parse the open `- [ ]` items out of backlog.md. Done items (`- [x]`),
/// headings, and prose are ignored; a missing `Added` date is tolerated
/// (the report says so) rather than dropping the item.
fn parse_backlog(text: &str) -> Vec<BacklogItem> {
    let mut items = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("- [ ] ") else {
            continue;
        };
        let added_days = find_added_days(rest);
        // The Added marker is metadata: keep it out of the display text
        // (the report renders the age itself).
        let text = match (added_days.is_some(), rest.rfind("Added ")) {
            (true, Some(idx)) => rest[..idx].trim_end().to_string(),
            _ => rest.trim().to_string(),
        };
        items.push(BacklogItem { text, added_days });
    }
    items
}

/// Extract a trailing `Added YYYY-MM-DD.` marker as days since the civil
/// epoch (1970-01-01 = 0).
fn find_added_days(text: &str) -> Option<i64> {
    const MARKER: &str = "Added ";
    let idx = text.rfind(MARKER)?;
    let date = text[idx + MARKER.len()..]
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .next()?;
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// Days since 1970-01-01 from a civil date (Howard Hinnant's algorithm;
/// chrono/time stay out of the tooling's dependency graph for this).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record as rec;
    use std::collections::BTreeMap;

    fn fixture_record(unix_time: u64, artifact_bytes: u64, instructions: u64) -> Record {
        Record {
            schema: 1,
            unix_time,
            runner: "ci".to_string(),
            git: rec::Git {
                sha: format!("{unix_time:040x}"),
                branch: "main".to_string(),
                dirty: false,
                commit_time: "2026-09-22T00:00:00+00:00".to_string(),
            },
            env: rec::Env {
                rustc: "rustc 1.95.0".to_string(),
                host: "x86_64-unknown-linux-gnu".to_string(),
                cpu: "t".to_string(),
                cores: 8,
                kernel: "t".to_string(),
            },
            config: rec::Config {
                profile: "size".to_string(),
                features: vec![],
                target: "x86_64-unknown-linux-gnu".to_string(),
            },
            artifacts: BTreeMap::from([("bw-demo".to_string(), artifact_bytes)]),
            deps: rec::Deps {
                external_normal: 78,
                duplicates: BTreeMap::new(),
            },
            wasm: BTreeMap::from([("bw-demo".to_string(), true)]),
            benches: BTreeMap::from([(
                "iai_sim::sim::step step_1000:s()".to_string(),
                rec::BenchMeasurement { instructions },
            )]),
            runtime: BTreeMap::new(),
            memory: BTreeMap::new(),
            skipped: vec![rec::Skip {
                metric: "clean compile time".to_string(),
                reason: "advisory nightly metric".to_string(),
            }],
        }
    }

    #[test]
    fn parse_since_accepts_days_and_all() {
        for (input, want) in [("7d", Some(7)), ("all", None)] {
            match parse_since(input) {
                Ok(v) => assert_eq!(v, want),
                Err(_) => panic!("parse_since({input}) failed"),
            }
        }
        assert!(parse_since("weekly").is_err());
    }

    #[test]
    fn loader_reads_shards_in_order_and_bails_on_garbage() {
        let dir = std::env::temp_dir().join(format!("bw-xtask-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|_| panic!("temp dir"));
        let shard_a = dir.join("records-2026-08.jsonl");
        let shard_b = dir.join("records-2026-09.jsonl");
        let a = serde_json::to_string(&fixture_record(1_000, 10, 10)).unwrap_or_default();
        let b = serde_json::to_string(&fixture_record(2_000, 20, 20)).unwrap_or_default();
        std::fs::write(&shard_a, format!("{a}\n")).unwrap_or_else(|_| panic!("write a"));
        std::fs::write(&shard_b, format!("{b}\n")).unwrap_or_default();

        let loaded = load_records(&dir).unwrap_or_else(|e| panic!("load: {e:#}"));
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].unix_time, 1_000);
        assert_eq!(loaded[1].artifacts["bw-demo"], 20);

        std::fs::write(&shard_b, "not json\n").unwrap_or_else(|_| panic!("write bad"));
        assert!(load_records(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_flags_double_digit_growth_with_action() {
        let base = fixture_record(1_000, 1_000_000, 13_005);
        let last = fixture_record(2_000, 1_200_000, 13_100);
        let markdown = render(&[base, last], None, None, 0, "7d");
        assert!(markdown.contains("2 record(s) (2 same env"));
        assert!(markdown.contains("artifact bw-demo bytes"));
        assert!(markdown.contains("+20.0%"));
        assert!(markdown.contains("action: run `cargo xtask perf attribute`"));
        // bench +0.7% stays under the 1% instruction threshold: listed in
        // the delta table, but not escalated to a finding.
        assert!(markdown.contains("bench iai_sim::sim::step"));
        assert!(!markdown.contains("- **bench"));
    }

    #[test]
    fn render_flags_env_mismatch_instead_of_diffing() {
        let mut other_env = fixture_record(1_000, 500_000, 10_000);
        other_env.runner = "elsewhere".to_string();
        let last = fixture_record(2_000, 1_000_000, 13_005);
        let markdown = render(&[other_env, last], None, None, 0, "30d");
        assert!(markdown.contains("1 record(s) excluded from deltas"));
        assert!(markdown.contains("only one record in this window/env"));
    }

    #[test]
    fn render_lists_skips_from_latest_record() {
        let markdown = render(&[fixture_record(1_000, 1, 1)], None, None, 0, "7d");
        assert!(markdown.contains("clean compile time: advisory nightly metric"));
    }

    #[test]
    fn backlog_parser_extracts_open_items_with_dates() {
        let text = "\
# Backlog

## Open

- [ ] **tree-tune** — Refine the tree visuals. Added 2026-09-22.
- [ ] **no-date** — Legacy line without a marker.
- [x] **done-thing** — Finished. Added 2026-09-01.

Some prose line that mentions Added 2026-01-01 but is not an item.
";
        let items = parse_backlog(text);
        assert_eq!(items.len(), 2, "done items and prose are not open items");
        assert!(items[0].text.starts_with("**tree-tune**"));
        assert!(
            !items[0].text.contains("Added"),
            "marker stays out of display text"
        );
        assert_eq!(items[0].added_days, Some(days_from_civil(2026, 9, 22)));
        assert_eq!(items[1].added_days, None);
    }

    #[test]
    fn civil_days_are_calendar_accurate() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        // Leap year: Feb 29 exists in 2024.
        assert_eq!(
            days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 28),
            2
        );
        assert_eq!(
            days_from_civil(2026, 1, 1) - days_from_civil(2025, 12, 31),
            1
        );
        assert_eq!(
            days_from_civil(2026, 9, 22) - days_from_civil(2026, 9, 15),
            7
        );
    }

    #[test]
    fn render_lists_backlog_with_age_and_flags_stale_items() {
        let items = vec![
            BacklogItem {
                text: "**tree-tune** — Refine the tree visuals.".to_string(),
                added_days: Some(days_from_civil(2026, 9, 20)),
            },
            BacklogItem {
                text: "**old-thing** — From a while back.".to_string(),
                added_days: Some(days_from_civil(2026, 8, 1)),
            },
            BacklogItem {
                text: "**no-date**".to_string(),
                added_days: None,
            },
        ];
        // "Now" = 2026-09-22.
        let now = (days_from_civil(2026, 9, 22) * 86_400) as u64;
        let markdown = render(
            &[fixture_record(1_000, 1, 1)],
            None,
            Some(&items),
            now,
            "7d",
        );
        assert!(markdown.contains("## backlog (3 open, from backlog.md)"));
        assert!(markdown.contains("**tree-tune** — Refine the tree visuals. — added 2d ago"));
        // 2026-08-01 is 52 days old: flagged with the canned action.
        assert!(markdown
            .contains("⚠ **old-thing** — From a while back. — added 52d ago — older than two weekly cycles; action: schedule, split, or close"));
        assert!(markdown.contains("**no-date** — no Added date"));
    }

    #[test]
    fn render_skips_backlog_section_when_file_is_missing() {
        let markdown = render(&[fixture_record(1_000, 1, 1)], None, None, 0, "7d");
        assert!(markdown.contains("## backlog"));
        assert!(markdown.contains("backlog.md not found at workspace root"));
    }
}
