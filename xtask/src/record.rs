//! Measurement record schema (ADR 0001, D5). Records are append-only
//! artifacts: never edited after being written, only superseded.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub schema: u32,
    /// Unix epoch seconds at measurement time.
    pub unix_time: u64,
    /// Who produced this record: "local" or a CI runner name (D5).
    pub runner: String,
    pub git: Git,
    pub env: Env,
    pub config: Config,
    /// Artifact name -> size in bytes.
    pub artifacts: BTreeMap<String, u64>,
    pub deps: Deps,
    /// Scope member -> did `cargo check --target wasm32-unknown-unknown`
    /// pass. Empty when the check could not run at all (see `skipped`).
    #[serde(default)]
    pub wasm: BTreeMap<String, bool>,
    /// iai-callgrind bench id -> instruction count (gate tier, ADR 0001 D2).
    #[serde(default)]
    pub benches: BTreeMap<String, BenchMeasurement>,
    /// Headless gallery scene -> frame-time stats (trend tier). Timing pass
    /// only; never measured with the counting allocator active (D8.8).
    #[serde(default)]
    pub runtime: BTreeMap<String, FrameMeasurement>,
    /// Headless gallery scene -> allocation stats from the perf-alloc pass.
    #[serde(default)]
    pub memory: BTreeMap<String, AllocMeasurement>,
    /// Metrics that were not measured, with reasons. Skips are always
    /// recorded, never silent (ADR 0001, D8.7).
    pub skipped: Vec<Skip>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Git {
    pub sha: String,
    pub branch: String,
    pub dirty: bool,
    /// ISO 8601 commit timestamp (`git log -1 --format=%cI`).
    pub commit_time: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Env {
    /// Full `rustc -V` line.
    pub rustc: String,
    /// Host triple, which is also the build target (no --target passed).
    pub host: String,
    pub cpu: String,
    pub cores: u32,
    pub kernel: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Cargo profile measured (sizes: "size"; benches later: "runtime").
    pub profile: String,
    /// Feature flags active during the build ([] = workspace defaults).
    pub features: Vec<String>,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deps {
    /// Unique (crate, version) pairs in the union of measured members'
    /// normal-dependency graphs, excluding workspace members.
    pub external_normal: u64,
    /// Crate name -> the multiple versions present in the graph.
    pub duplicates: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skip {
    pub metric: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchMeasurement {
    /// Total instructions reported by iai-callgrind.
    pub instructions: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameMeasurement {
    pub frame_ms_min: f64,
    pub frame_ms_p50: f64,
    pub frame_ms_p90: f64,
    pub frame_ms_p99: f64,
    pub frame_ms_max: f64,
    pub entities: u64,
    pub ticks: u64,
    pub interactions: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllocMeasurement {
    pub allocs: u64,
    pub peak_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Record {
        Record {
            schema: 1,
            unix_time: 1_758_556_800,
            runner: "local".to_string(),
            git: Git {
                sha: "026c703".to_string(),
                branch: "main".to_string(),
                dirty: false,
                commit_time: "2026-09-22T21:24:49+00:00".to_string(),
            },
            env: Env {
                rustc: "rustc 1.95.0".to_string(),
                host: "x86_64-unknown-linux-gnu".to_string(),
                cpu: "Test CPU".to_string(),
                cores: 8,
                kernel: "7.2.6".to_string(),
            },
            config: Config {
                profile: "size".to_string(),
                features: vec![],
                target: "x86_64-unknown-linux-gnu".to_string(),
            },
            artifacts: BTreeMap::from([("bw-demo".to_string(), 390_104)]),
            deps: Deps {
                external_normal: 12,
                duplicates: BTreeMap::new(),
            },
            wasm: BTreeMap::from([("bw-demo".to_string(), true)]),
            benches: BTreeMap::from([(
                "iai_sim::sim::step step_1000".to_string(),
                BenchMeasurement {
                    instructions: 41_233,
                },
            )]),
            runtime: BTreeMap::from([(
                "bw-demo::bounce".to_string(),
                FrameMeasurement {
                    frame_ms_min: 0.1,
                    frame_ms_p50: 0.2,
                    frame_ms_p90: 0.3,
                    frame_ms_p99: 0.4,
                    frame_ms_max: 0.5,
                    entities: 1_000,
                    ticks: 600,
                    interactions: 30_000,
                },
            )]),
            memory: BTreeMap::from([(
                "bw-demo::bounce".to_string(),
                AllocMeasurement {
                    allocs: 259_981,
                    peak_bytes: 316_999,
                },
            )]),
            skipped: vec![Skip {
                metric: "benches".to_string(),
                reason: "no benches yet (ADR 0001 Phase 2)".to_string(),
            }],
        }
    }

    #[test]
    fn record_round_trips_through_json() -> serde_json::Result<()> {
        let record = sample();
        let json = serde_json::to_string(&record)?;
        let back: Record = serde_json::from_str(&json)?;
        assert_eq!(record, back);
        Ok(())
    }
}
