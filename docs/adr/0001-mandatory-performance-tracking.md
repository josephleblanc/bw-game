# ADR 0001: Mandatory performance tracking for time and space

- Status: Accepted
- Date: 2026-09-22

## Context

bw-game is a new Bevy/Rust game being built demo-first: isolated demos and
gallery binaries come before the integrated game. Stated project goals:

1. High quality, modular, performant code.
2. A contained dependency footprint — Bevy features and dependencies are
   added deliberately, as measured changes, not casually.
3. Performance tracked **continuously** during development, in both **time**
   (compile time, runtime/frame performance) and **space** (binary size,
   memory, dependency graph), with measurements that are **mandatory**,
   **accurate**, and **periodically reviewed with actionable recommendations**.

Relevant facts about our situation:

- The workspace is a day old: `bw-core` and `bw-demo`, no Bevy dependency yet.
  Installing the measurement process now is nearly free; retrofitting it after
  the game exists is expensive and, in practice, rarely happens. In
  particular, benchmarkability of visual demos must be designed in (headless,
  deterministic) — it cannot be bolted on later.
- Games regress by accretion: one more dependency, one more default feature,
  one more monomorphized generic, one more per-frame allocation. None of these
  individually "looks wrong" in review; only measurement catches them.
- Shared CI machines exhibit ±10–20% wall-clock noise. A process that gates on
  raw wall-clock times in CI produces false positives, which leads to gates
  being disabled, which ends the mandate. Accuracy policy must therefore be
  noise-aware from day one.
- Binary sizes, dependency-graph properties, and CPU instruction counts are
  deterministic for a given (toolchain, lockfile, profile, features) tuple.
  These can be gated hard in CI without false positives.
- The repository currently has no git remote or CI host. Any process we adopt
  must be fully runnable locally now and portable to a CI host later.

## Decision

We adopt a **budgets-and-ratchets** performance system: repo-owned measurement
tooling produces versioned measurement records, blocking CI gates compare
deterministic metrics against checked-in budgets, and scheduled jobs generate
trend reports that are reviewed on a fixed cadence with mandated, actionable
findings. The system comprises nine sub-decisions.

### D1. Scripts-first: all logic in `cargo xtask`, CI is a thin wrapper

Everything a CI job does — measure, compare, gate, report — is a subcommand of
a workspace `xtask` crate, invocable identically on a developer machine:

```
cargo xtask perf measure [--profile <p>] [--out <record.json>]
cargo xtask perf check  [--baseline <ref>]        # exit 1 on budget breach
cargo xtask perf report --since <7d|30d|…>       # markdown report + findings
cargo xtask perf baseline update                 # dedicated, reviewable refresh
```

CI YAML only checks out, installs tools, and calls these. Rationale: the
process must work before we have a CI host; developers must be able to
reproduce any CI failure exactly; logic in YAML is untestable and
provider-locked. `xtask` is a workspace member excluded from `default-members`
so it never slows day-to-day builds.

### D2. Two-tier accuracy policy: gate the deterministic, trend the noisy

| Metric | Tool | Tier | Cadence |
|---|---|---|---|
| Native binary size (per artifact) | `stat` on `size`-profile build | **Gate** (blocking) | every PR |
| Benchmark instruction counts | `iai-callgrind` (valgrind) | **Gate** (blocking) | every PR |
| Dependency hygiene: duplicate versions, unexpected default features | `cargo tree -d`, `cargo tree -e features` | **Gate** (blocking) | every PR |
| WASM target compiles | `cargo check --target wasm32-unknown-unknown` | **Gate** (blocking) | every PR, once Bevy lands |
| Wall-clock micro/system benchmarks | `criterion` | Trend (statistical) | nightly + on demand |
| Frame-time of gallery scenes | headless harness (D4) | Trend (statistical) | nightly |
| Clean/warm compile time | `cargo build --timings`, `hyperfine` | Trend (advisory) | nightly |
| Peak RSS / allocations | `/usr/bin/time -v`, `dhat` | Trend → Gate later | nightly |

Rules of the policy:

1. Nothing stochastic is ever gated on a single run in shared CI. Noisy
   metrics enter through distributions (criterion's median/CI, percentile
   frame times over fixed tick counts) and act as findings, not failures.
2. Instruction counts are the CI-safe proxy for "time": valgrind counts are
   immune to CPU contention. They miss cache/memory-hierarchy regressions, so
   nightly criterion trends complement them (see Consequences).
3. Sizes are measured on `--profile size` builds; benches are built with
   `--profile runtime`; the `quick` profile is never a measurement target.

### D3. Budgets and ratchets in `perf/budgets.toml`

Every gated metric has a checked-in budget with a mandatory human note:

```toml
schema = 1
reviewed = "2026-09-22"

[artifact.bw-demo]                 # profile = "size", target = host
budget_bytes = 400_000             # set from Phase-0 baseline; ratchet down only
note = "hello-world scaffold; first Bevy addition gets its own explicit step-up"

[bench.core-sanity]
max_instructions = 0               # set from first iai baseline
note = "placeholder until real benches exist"

[advisory.build]
check_clean_growth_pct = 10        # trend-only; never blocks
```

Semantics that make tracking *mandatory*:

- A PR that exceeds a budget fails CI. There are exactly two remedies:
  make the code smaller/faster, or raise the budget in the same PR — a
  visible diff to `perf/budgets.toml` with an updated `note` explaining why.
  Silent regressions are structurally impossible; conscious ones are
  reviewable.
- Budget increases above 5% (size) or any increase to a bench budget must be
  justified in the PR description and, for significant ones, reference an
  ADR. The first big test of this process is deliberately the addition of
  Bevy itself.
- Ratcheting down (tightening) is always allowed and encouraged by reviews.

### D4. Headless benchmark contract for every gallery binary

Every gallery/demo binary must accept, from its first merged version:

```
<binary> --perf-headless --scene <id> --ticks <N> --seed <S> --json
```

and emit machine-readable JSON: frame-time percentiles over the fixed tick
count, entity and draw-call counts, and (in a separate allocation-counting
pass — see rule D8.8) allocation count and peak bytes, exiting 0. Runs are
deterministic: fixed seed, fixed timestep, no vsync/display, no wall-clock
dependence in simulation. The timing pass runs without the allocation-tracking
feature so counters never perturb timings. A gallery demo without this mode
does not merge — this is the single most important retrofit-prevention rule
in this ADR. (Exact renderer/headless strategy gets its own ADR with the
first gallery implementation.)

### D5. Versioned measurement records and append-only history

Every measurement run produces a record:

```json
{
  "schema": 1,
  "timestamp": "2026-09-22T14:00:00Z",
  "git": {"sha": "…", "branch": "main", "dirty": false},
  "env": {"rustc": "…", "cpu": "…", "cores": 8, "kernel": "…", "runner": "local|ci-name"},
  "config": {"profile": "size", "features": [], "target": "x86_64-unknown-linux-gnu"},
  "artifacts": {"bw-demo": {"bytes": 390_104}},
  "benches": {"core-sanity": {"instructions": 41}},
  "build": {"check_clean_s": 4.2},
  "runtime": {"bw-demo::intro": {"frame_ms_p50": 4.1, "frame_ms_p99": 9.9, "entities": 1000}},
  "memory": {"bw-demo::intro": {"peak_rss_kib": 210_000, "allocs": 85_311}},
  "skipped": [{"metric": "iai.benches", "reason": "valgrind not installed"}]
}
```

Records are append-only JSONL on a dedicated orphan branch (`perf-data`), so
history never pollutes `main`'s blame. Comparisons between records with
mismatched `env`/`config` are flagged in reports rather than silently diffed.
When a CI host exists, its scheduled jobs append records automatically.

### D6. Attribution is mandatory: regressions arrive with named causes

Size or compile-time regressions are reported together with attribution from
`cargo bloat` (crates and functions), `cargo llvm-lines` (monomorphization
pressure), `cargo build --timings` (crate compile times), `cargo tree -d`
(duplicates), and — once WASM ships — `twiggy`. "Binary grew 800 KB" is not
an acceptable finding; "grew 800 KB, 610 KB from `bevy_render` via default
features X, Y" is. Tools being unavailable is acceptable only as an explicitly
recorded skip in the report, never a silent omission.

### D7. CI/CD structure (reference implementation: GitHub Actions)

No CI host exists yet; all of the above is local-first via `xtask`. When the
repository gets a host, wire exactly three workflows:

- **`ci.yml`** (PR + push): fmt, clippy (workspace `deny` lints), tests, WASM
  check (once applicable), then `xtask perf measure` + `xtask perf check` —
  **required status checks**; this is what makes the mandate real.
- **`perf-trend.yml`** (weekly cron + manual): clean-build timings, full
  criterion run against the `main` baseline, attribution dumps (bloat,
  llvm-lines, tree, timings HTML) attached as artifacts, record appended to
  `perf-data`, and a rendered report posted to a rolling `perf-review` issue.
- **`release.yml`** (tag/manual): builds `runtime` and `size` artifacts,
  refuses to publish if any budget is red, attaches the measurement record as
  the release's perf manifest.

### D8. Measurement accuracy rules

1. Every record carries environment metadata; cross-environment comparisons
   are flagged, not merged into one trend line.
2. Toolchain and lockfile are pinned (`rust-toolchain.toml`, committed
   `Cargo.lock`) — measurements are only comparable within the same tuple of
   (rustc, lockfile, profile, features).
3. Stochastic metrics are reported as distributions (median, CI, p99), never
   as single means.
4. Benchmark inputs are fixed and recorded: seeds, tick counts, scenes.
5. Compile-time numbers always state cache state (clean vs incremental) and
   are kept in separate buckets.
6. A regression finding without attribution is a bug in the report (D6).
7. Skips are recorded with reasons; no metric silently disappears.
8. Measurement instrumentation must not perturb what it measures:
   allocation counting runs in a separate pass from timing.
9. Baselines are refreshed only by the dedicated, reviewable
   `xtask perf baseline update` change — never as a side effect of a feature
   PR.

### D9. Review cadence and actionable findings

- **Weekly**: `perf-trend.yml` posts/updates the `perf-review` issue
  containing the rendered report: 7/30-day deltas per metric, budget headroom
  table, top size/compile-time contributors with attribution, benches with
  significant drift, and the skipped-metrics log.
- **Report invariant**: every flagged finding is paired with a recommended
  action ("feature-gate X", "dedupe Y via workspace dependency", "move Z off
  the per-frame path") and a suggested owner area. A finding without a
  recommended action is a report bug.
- **Biweekly** (or per milestone): a human review walks open findings,
  assigns owners, and prunes/retires budgets that no longer reflect intent
  (each budget carries `reviewed`/`note` to make staleness visible).
- **Milestone gate**: no release cuts with red budgets or unreviewed
  `perf-review` findings.

## Alternatives considered

- **A benchmarking SaaS (Bencher, Codspeed, etc.)** — excellent noise
  handling and trend UI, but adds an external dependency for a project whose
  stated goal is a contained footprint, and would strand the process if the
  repo never gets a public host. Revisit if maintaining `perf-data` history
  and reports becomes a measurable burden.
- **Gating wall-clock benchmarks in CI** — rejected: shared-runner noise
  produces false positives, which leads to disabled gates and a dead mandate.
  Instruction counts (D2) give us hard CI gating without the noise.
- **Measurement logic in CI YAML (or `just`/shell scripts)** — rejected in
  favor of `xtask` (D1): untestable, provider-locked, and impossible to run
  identically before we have a host.
- **Ad-hoc measurement "when something feels slow"** — rejected: this is the
  default game-project failure mode this ADR exists to prevent; visual demos
  are systematically unbenchmarkable after the fact without the D4 contract.

## Consequences

**Positive**

- Regressions in size, dependency hygiene, and instruction counts are caught
  at PR time by blocking checks; nothing ships by accretion.
- Every gallery demo is benchmarkable from birth, giving frame-time trend
  data for the life of the project.
- Perf work is data-driven: reviews and planning start from the report's
  ranked contributors, not vibes.
- The contained-footprint goal is structurally enforced (budgets + feature
  audits) rather than aspirational.

**Costs and accepted risks**

- CI minutes: valgrind benches and nightly clean builds add real time;
  mitigated by gating cheap deterministic metrics on PRs and running
  expensive attribution/trends on the weekly schedule only.
- Maintenance: `xtask`, budgets, and report tooling are real code with tests
  and a schema to evolve. We accept this as the price of the mandate.
- Instruction counts don't model cache behavior — a change can be flat in
  instructions but slower in wall time. Nightly criterion trends are the
  complement; if they prove too noisy on the eventual CI host to trust, we
  move frame-time trending to a designated local machine recorded as such in
  `env.runner`.
- Budget gaming (inflated baselines) is possible; attribution requirements
  (D6) and reviewable budget notes (D3) are the counterweights.
- Discipline tax: every demo implements the headless contract. We accept this
  as non-negotiable because retrofit cost is far higher.

**Signals to revisit this ADR**

- The repo moves to a host whose runners are too weak for valgrind benches.
- `perf-data` history/report maintenance exceeds its perceived value
  (→ evaluate SaaS again).
- The game's frame-time needs outgrow fixed-tick headless measurement
  (→ new ADR on the runtime profiling harness).

## Implementation notes

Phased so each step lands with the work that needs it:

- **Phase 0 — now, before any Bevy dependency**: `rust-toolchain.toml` pin;
  `perf/budgets.toml` seeded from measured scaffold baselines; `xtask` crate
  with `perf measure/check` covering artifact sizes and dependency hygiene;
  `docs/build-configuration.md` (profiles/commands); test `cargo xtask perf`
  locally as the "CI" until a host exists.
- **Phase 1 — with the first Bevy dependency**: WASM check gate; `cargo
  bloat`/`llvm-lines` attribution wired into `perf report`; explicit budget
  step-up PR demonstrating the D3 process; CI host wiring (`ci.yml`) if a
  remote exists by then.
- **Phase 2 — with the first gallery demo**: headless harness + `--perf-*`
  contract in `bw-demo`; first `criterion`/`iai-callgrind` benches sharing
  exercise functions in `bw-core` (bench-only dev-dependencies, so zero
  footprint cost); per-demo budgets; PR perf summary comment.
- **Phase 3 — multiple demos/assets**: `perf-trend.yml` weekly job, `perf-data`
  branch history, dhat/tracking-allocator memory pass, `release.yml` with the
  budget-green gate, biweekly review ritual.

Status 2026-09-22: Phases 0–2 landed (32a1214, 2eee22f, 9f39aa6). The
Phase 3 dhat memory pass landed with the allocation-policy work
(05cbf79…8b959b5) — the alloc pass runs inside `perf measure`/`check`,
gates via `[steady.*]` budgets, and `cargo xtask perf dhat` inspects it
per scene; the `alloc_probe` macro landed with it. The remaining Phase 3
items — `perf-trend.yml`, the `perf-data` branch, `release.yml`, CI
wiring, the review ritual — all presuppose a hosted remote; until one
exists, `cargo xtask perf check` remains the CI.

Remote landed later the same day (`josephleblanc/bw-game`, private):
`ci.yml` (fmt/clippy/tests/wasm + measure/check), `perf-trend.yml`
(weekly + manual; appends records to the seeded `perf-data` orphan
branch, posts the rendered report to the rolling `perf-review` issue via
the new `cargo xtask perf report`), `release.yml` (budget-green gate +
perf manifest), and `docs/perf-review.md` (the D9 ritual). Branch
protection — required status checks — is not available for private
repos on a free plan; until the repo goes public or the account upgrades,
the required-check half of D7 is unwired: CI fails loudly, but nothing
mechanically blocks a red merge.
