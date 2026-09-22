//! Performance budgets (ADR 0001, D3): the checked-in ratchet that `perf
//! check` enforces. Raising a budget requires a visible diff and a
//! justification; lowering it is always allowed.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::record::Record;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Budgets {
    pub schema: u32,
    /// Last human review of these budgets (see ADR 0001, D9).
    pub reviewed: String,
    pub scope: Scope,
    pub artifact: BTreeMap<String, ArtifactBudget>,
    pub deps: Deps,
    /// iai-callgrind bench id -> instruction-count ceiling.
    #[serde(default)]
    pub bench: BTreeMap<String, BenchBudget>,
    #[serde(default)]
    pub advisory: Advisory,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchBudget {
    pub max_instructions: u64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scope {
    /// Workspace members whose dependency graphs count toward the dep
    /// budget. Tooling crates (xtask) stay out so their dependencies never
    /// count against the game's footprint.
    pub measured_members: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactBudget {
    pub budget_bytes: u64,
    /// Why this budget has its current value. Must be updated whenever the
    /// number is raised.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deps {
    pub max_external_normal: u64,
    #[serde(default)]
    pub allowed_duplicates: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Advisory {
    #[serde(default)]
    pub build: AdvisoryBuild,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvisoryBuild {
    /// Compile-time trend threshold; never a hard gate (ADR 0001, D2).
    pub check_clean_growth_pct: u32,
}

impl Default for AdvisoryBuild {
    fn default() -> Self {
        Self {
            check_clean_growth_pct: 10,
        }
    }
}

pub fn load(path: &Path) -> Result<Budgets> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read budgets file {}", path.display()))?;
    toml::from_str(&text)
        .with_context(|| format!("failed to parse budgets file {}", path.display()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Breach,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub level: Level,
    pub text: String,
}

impl Finding {
    fn ok(text: String) -> Self {
        Self {
            level: Level::Ok,
            text,
        }
    }

    fn warn(text: String) -> Self {
        Self {
            level: Level::Warn,
            text,
        }
    }

    fn breach(text: String) -> Self {
        Self {
            level: Level::Breach,
            text,
        }
    }
}

/// Compare a measurement record against the budgets. Pure, so it is unit
/// testable without running cargo.
pub fn compare(budgets: &Budgets, record: &Record) -> Vec<Finding> {
    let mut findings = Vec::new();

    for (name, budget) in &budgets.artifact {
        match record.artifacts.get(name) {
            Some(bytes) => {
                if *bytes > budget.budget_bytes {
                    let over = bytes - budget.budget_bytes;
                    let pct = 100.0 * over as f64 / budget.budget_bytes as f64;
                    findings.push(Finding::breach(format!(
                        "artifact {name}: {} / {} bytes (over by {}, {pct:.1}%) — note: {}",
                        fmt_num(*bytes),
                        fmt_num(budget.budget_bytes),
                        fmt_num(over),
                        budget.note,
                    )));
                } else {
                    let headroom = budget.budget_bytes - bytes;
                    let pct = 100.0 * headroom as f64 / budget.budget_bytes as f64;
                    findings.push(Finding::ok(format!(
                        "artifact {name}: {} / {} bytes (headroom {}, {pct:.1}%)",
                        fmt_num(*bytes),
                        fmt_num(budget.budget_bytes),
                        fmt_num(headroom),
                    )));
                }
            }
            None => findings.push(Finding::warn(format!(
                "artifact {name}: budgeted but not measured (was it built?)"
            ))),
        }
    }

    for name in record.artifacts.keys() {
        if !budgets.artifact.contains_key(name) {
            findings.push(Finding::warn(format!(
                "artifact {name}: measured but unbudgeted — add it to perf/budgets.toml"
            )));
        }
    }

    if record.deps.external_normal > budgets.deps.max_external_normal {
        findings.push(Finding::breach(format!(
            "deps: {} external (normal) > budget {} — new dependency requires a visible budget raise",
            record.deps.external_normal,
            budgets.deps.max_external_normal,
        )));
    } else {
        findings.push(Finding::ok(format!(
            "deps: {} / {} external (normal)",
            record.deps.external_normal, budgets.deps.max_external_normal,
        )));
    }

    let mut unexpected: Vec<String> = Vec::new();
    for (name, versions) in &record.deps.duplicates {
        if budgets.deps.allowed_duplicates.contains(name) {
            continue;
        }
        unexpected.push(format!("{name} ({})", versions.join(", ")));
    }
    if unexpected.is_empty() {
        let count = record.deps.duplicates.len();
        findings.push(Finding::ok(format!(
            "duplicates: {count} duplicated crate name(s), all allowlisted"
        )));
    } else {
        findings.push(Finding::breach(format!(
            "duplicates: unallowlisted duplicate versions: {}",
            unexpected.join("; ")
        )));
    }

    for name in &budgets.deps.allowed_duplicates {
        if !record.deps.duplicates.contains_key(name) {
            findings.push(Finding::warn(format!(
                "duplicates: {name} is allowlisted but no longer duplicated — remove the allowlist entry"
            )));
        }
    }

    if record.wasm.is_empty() {
        findings.push(Finding::warn(String::from(
            "wasm: not checked (see record skipped[] for the reason)",
        )));
    } else {
        let failed: Vec<&str> = record
            .wasm
            .iter()
            .filter(|(_, ok)| !**ok)
            .map(|(name, _)| name.as_str())
            .collect();
        if failed.is_empty() {
            findings.push(Finding::ok(format!(
                "wasm32 check: {} member(s) compile",
                record.wasm.len()
            )));
        } else {
            findings.push(Finding::breach(format!(
                "wasm32 check failed for: {}",
                failed.join(", ")
            )));
        }
    }

    for (id, budget) in &budgets.bench {
        match record.benches.get(id) {
            Some(measured) => {
                if measured.instructions > budget.max_instructions {
                    findings.push(Finding::breach(format!(
                        "bench {id}: {} instructions > budget {} — note: {}",
                        fmt_num(measured.instructions),
                        fmt_num(budget.max_instructions),
                        budget.note,
                    )));
                } else {
                    findings.push(Finding::ok(format!(
                        "bench {id}: {} / {} instructions",
                        fmt_num(measured.instructions),
                        fmt_num(budget.max_instructions),
                    )));
                }
            }
            None => findings.push(Finding::warn(format!(
                "bench {id}: budgeted but not measured (did the bench run?)"
            ))),
        }
    }
    for id in record.benches.keys() {
        if !budgets.bench.contains_key(id) {
            findings.push(Finding::warn(format!(
                "bench {id}: measured but unbudgeted — add it to perf/budgets.toml"
            )));
        }
    }

    // Trend-tier metrics: recorded and surfaced, never gated (ADR 0001 D2).
    for (scene, frame) in &record.runtime {
        findings.push(Finding::ok(format!(
            "trend runtime {scene}: p50 {:.3} ms, p99 {:.3} ms over {} ticks of {} entities",
            frame.frame_ms_p50, frame.frame_ms_p99, frame.ticks, frame.entities,
        )));
    }
    for (scene, mem) in &record.memory {
        findings.push(Finding::ok(format!(
            "trend memory {scene}: {} allocs, peak {} bytes",
            fmt_num(mem.allocs),
            fmt_num(mem.peak_bytes),
        )));
    }

    findings
}

/// 390104 -> "390,104"
pub fn fmt_num(n: u64) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record as rec;

    fn fixture(budget_bytes: u64, max_deps: u64) -> Budgets {
        Budgets {
            schema: 1,
            reviewed: "2026-09-22".to_string(),
            scope: Scope {
                measured_members: vec!["bw-core".to_string(), "bw-demo".to_string()],
            },
            artifact: BTreeMap::from([(
                "bw-demo".to_string(),
                ArtifactBudget {
                    budget_bytes,
                    note: "test".to_string(),
                },
            )]),
            deps: Deps {
                max_external_normal: max_deps,
                allowed_duplicates: vec!["old-dup".to_string()],
            },
            bench: BTreeMap::new(),
            advisory: Advisory::default(),
        }
    }

    fn record_fixture(bytes: u64, deps: u64, dups: &[(&str, &[&str])]) -> Record {
        Record {
            schema: 1,
            unix_time: 0,
            runner: "test".to_string(),
            git: rec::Git {
                sha: String::new(),
                branch: String::new(),
                dirty: false,
                commit_time: String::new(),
            },
            env: rec::Env {
                rustc: String::new(),
                host: String::new(),
                cpu: String::new(),
                cores: 0,
                kernel: String::new(),
            },
            config: rec::Config {
                profile: "size".to_string(),
                features: vec![],
                target: String::new(),
            },
            artifacts: BTreeMap::from([("bw-demo".to_string(), bytes)]),
            deps: rec::Deps {
                external_normal: deps,
                duplicates: dups
                    .iter()
                    .map(|(name, versions)| {
                        (
                            name.to_string(),
                            versions.iter().map(|v| v.to_string()).collect(),
                        )
                    })
                    .collect(),
            },
            wasm: BTreeMap::from([("bw-demo".to_string(), true)]),
            benches: BTreeMap::new(),
            runtime: BTreeMap::new(),
            memory: BTreeMap::new(),
            skipped: vec![],
        }
    }

    #[test]
    fn compare_passes_within_budgets() {
        let findings = compare(&fixture(100, 10), &record_fixture(99, 10, &[]));
        assert_eq!(
            findings.iter().filter(|f| f.level == Level::Breach).count(),
            0
        );
        assert!(findings.iter().any(|f| f.text.contains("headroom 1")));
    }

    #[test]
    fn compare_flags_size_and_dep_breaches() {
        let findings = compare(&fixture(100, 10), &record_fixture(101, 11, &[]));
        let breaches: Vec<&str> = findings
            .iter()
            .filter(|f| f.level == Level::Breach)
            .map(|f| f.text.as_str())
            .collect();
        assert!(breaches.iter().any(|t| t.contains("over by 1")));
        assert!(breaches.iter().any(|t| t.contains("> budget 10")));
    }

    #[test]
    fn compare_flags_missing_unbudgeted_and_stale_allowlist() {
        let mut budgets = fixture(100, 10);
        budgets.artifact.remove("bw-demo");
        let mut record = record_fixture(100, 10, &[]);
        record.artifacts.insert("bw-new".to_string(), 5);
        let findings = compare(&budgets, &record);
        assert!(
            findings
                .iter()
                .any(|f| f.level == Level::Warn && f.text.contains("unbudgeted"))
        );
        assert!(
            findings
                .iter()
                .any(|f| f.level == Level::Warn && f.text.contains("old-dup"))
        );
    }

    #[test]
    fn compare_treats_allowlisted_duplicates_as_ok() {
        let findings = compare(
            &fixture(100, 10),
            &record_fixture(99, 10, &[("old-dup", &["1.0", "1.1"])]),
        );
        assert_eq!(
            findings.iter().filter(|f| f.level == Level::Breach).count(),
            0
        );
    }

    #[test]
    fn compare_flags_unallowlisted_duplicates() {
        let findings = compare(
            &fixture(100, 10),
            &record_fixture(99, 10, &[("serde", &["1.0.200", "1.0.210"])]),
        );
        assert!(
            findings
                .iter()
                .any(|f| f.level == Level::Breach && f.text.contains("serde (1.0.200, 1.0.210)"))
        );
    }

    #[test]
    fn fmt_num_inserts_thousands_separators() {
        assert_eq!(fmt_num(0), "0");
        assert_eq!(fmt_num(999), "999");
        assert_eq!(fmt_num(390_104), "390,104");
        assert_eq!(fmt_num(1_000_000), "1,000,000");
    }

    #[test]
    fn compare_gates_on_instruction_budgets() {
        let mut budgets = fixture(100, 10);
        budgets.bench.insert(
            "sim::step".to_string(),
            BenchBudget {
                max_instructions: 1_000,
                note: "test".to_string(),
            },
        );
        let mut within = record_fixture(100, 10, &[]);
        within.benches.insert(
            "sim::step".to_string(),
            rec::BenchMeasurement { instructions: 999 },
        );
        let findings = compare(&budgets, &within);
        assert_eq!(
            findings.iter().filter(|f| f.level == Level::Breach).count(),
            0
        );
        assert!(findings.iter().any(|f| f.text.contains("999 / 1,000")));

        let mut over = record_fixture(100, 10, &[]);
        over.benches.insert(
            "sim::step".to_string(),
            rec::BenchMeasurement {
                instructions: 1_001,
            },
        );
        let findings = compare(&budgets, &over);
        assert!(
            findings.iter().any(|f| f.level == Level::Breach
                && f.text.contains("1,001 instructions > budget 1,000"))
        );

        // Budgeted but missing from the record: warn, not silent.
        let findings = compare(&budgets, &record_fixture(100, 10, &[]));
        assert!(
            findings
                .iter()
                .any(|f| f.level == Level::Warn && f.text.contains("budgeted but not measured"))
        );
    }
}
