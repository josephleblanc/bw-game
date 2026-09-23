//! Shared headless-gallery measurement report (ADR 0001 D4, ADR 0003):
//! the JSON shape every gallery binary emits for the timing and allocation
//! passes, plus the windowed/probed assembly of the allocation detail.
//! Lives in bw-core so every gallery reports the identical structure and
//! `xtask` can parse them uniformly; the flag contract itself stays with
//! each binary (see the gallery crates' `main.rs`).

use serde::Serialize;

use crate::stats::five_number_summary;

#[cfg(feature = "perf-alloc")]
use crate::alloc_probe as alloc_probe_rt;

#[derive(Serialize)]
pub struct FrameStats {
    pub min: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
}

impl FrameStats {
    pub fn of(sample: Vec<f64>) -> Self {
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
pub struct AllocStats {
    pub count: u64,
    pub peak_bytes: u64,
}

#[derive(Serialize)]
pub struct WindowTotals {
    pub blocks: u64,
    pub bytes: u64,
}

#[derive(Serialize)]
pub struct ProbeTotals {
    pub blocks: u64,
    pub bytes: u64,
    pub calls: u64,
    pub blocks_per_call: f64,
    pub bytes_per_call: f64,
}

#[derive(Serialize)]
pub struct MeasuredWindow {
    pub blocks: u64,
    pub bytes: u64,
    pub ticks: u64,
    pub blocks_per_tick: FrameStats,
    pub bytes_per_tick: FrameStats,
}

#[derive(Serialize)]
pub struct AllocDetail {
    /// One-off cost: profiler creation through entity spawn (app + schedule
    /// construction, scene seeding, archetype table growth).
    pub setup: WindowTotals,
    /// Warmup window: schedule-system initialization and first-run growth;
    /// shares steady-state code paths.
    pub warmup: WindowTotals,
    /// The measured window: the steady-state churn the budget will gate.
    pub measured: MeasuredWindow,
    /// Per-function attribution, whole run (warmup + measured).
    pub probes: std::collections::BTreeMap<String, ProbeTotals>,
    /// Per-function attribution within the measured window only.
    pub measured_probes: std::collections::BTreeMap<String, ProbeTotals>,
    /// Everything inside a tick not attributed to a probed function: the
    /// allocation debt inherited from the engine's schedule machinery.
    pub schedule_residual: WindowTotals,
    /// Live bytes at window boundaries: flat means no leak; drift is the
    /// leak signal (per tick).
    pub live: LiveBytes,
}

#[derive(Serialize)]
pub struct LiveBytes {
    pub start: u64,
    pub end: u64,
    pub drift_bytes_per_tick: f64,
}

#[derive(Serialize)]
pub struct PerfReport {
    pub scene: String,
    pub ticks: u64,
    pub warmup_ticks: u64,
    pub seed: u64,
    pub entities: usize,
    pub interactions: u64,
    pub state_checksum: String,
    pub frame_ms: Option<FrameStats>,
    pub allocs: Option<AllocStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alloc_detail: Option<AllocDetail>,
}

/// Assemble the windowed/probed allocation report from the harness's
/// boundary snapshots. Series are already per-tick samples in order.
#[cfg(feature = "perf-alloc")]
#[allow(clippy::too_many_arguments)]
pub fn alloc_detail(
    probes_before_window: &alloc_probe_rt::Registry,
    probes_final: &alloc_probe_rt::Registry,
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
        source: &alloc_probe_rt::Registry,
        calls: &alloc_probe_rt::Registry,
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
        let mut window = alloc_probe_rt::Registry::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The timing-pass JSON shape `xtask perf` parses (`frame_measurement`)
    /// must stay intact: scene + tick config + counters + frame_ms.
    #[test]
    fn timing_report_serializes_the_contract_shape() {
        let report = PerfReport {
            scene: "tree".to_string(),
            ticks: 600,
            warmup_ticks: 60,
            seed: 42,
            entities: 512,
            interactions: 128,
            state_checksum: "0123456789abcdef".to_string(),
            frame_ms: Some(FrameStats {
                min: 0.0,
                p50: 0.1,
                p90: 0.2,
                p99: 0.3,
                max: 0.4,
            }),
            allocs: None,
            alloc_detail: None,
        };
        let json = serde_json::to_value(&report).unwrap_or(serde_json::Value::Null);
        for key in [
            "scene",
            "ticks",
            "warmup_ticks",
            "seed",
            "entities",
            "interactions",
            "state_checksum",
            "frame_ms",
        ] {
            assert!(json.get(key).is_some(), "report missing {key}");
        }
        assert!(json.get("allocs").is_some(), "allocs stays present-null");
        assert!(
            json.get("alloc_detail").is_none(),
            "alloc_detail is skipped when absent"
        );
    }
}
