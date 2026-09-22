# ADR 0003: Headless gallery harness and benchmark contracts

- Status: Accepted
- Date: 2026-09-22

## Context

ADR 0001 D4 promised this ADR with the first gallery implementation: the
exact strategy for making every demo benchmarkable from birth. The first
gallery (`bw-demo`) is a deterministic bouncing-particle scene — simulation
logic in `bw-core` (engine-agnostic per ADR 0002), Bevy ECS orchestration in
the binary. No renderer exists yet (bevy minimal), so "frame time" here
means the cost of one full ECS update tick; renderer cost joins the same
contract when a headless-renderer ADR lands.

Bevy's default `App::run()` loop is wall-clock driven and
`Query::iter` ordering is unspecified — both are determinism hazards for
measurement.

## Decision

1. **Layering**: `bw-core::sim` owns the simulation (SoA particle state,
   seeded splitmix64 RNG, uniform-grid spatial hash, FNV state checksum) in
   plain Rust. The gallery binary owns Bevy glue: resources wrapping the
   sim, a `step_sim` system, and a `sync_positions` system materializing
   `Position` components for the renderer-to-be. Benches measure core
   functions directly; the gallery measures the full ECS tick around them.
2. **D4 contract flags** (every gallery binary, from its first version):
   `--perf-headless --scene <id> --ticks <N> --seed <S> --json`, plus
   `--perf-scenes` printing scene ids (how xtask discovers scenes without
   hardcoding) and `--perf-alloc` selecting the allocation pass. Defaults:
   600 ticks, seed 42, 60 warmup ticks discarded before timing.
3. **Two passes, two builds (D8.8)**: the timing pass runs the default
   build and reports frame-time percentiles (`five_number_summary` over
   per-tick `Instant` durations); the allocation pass builds with the
   `perf-alloc` feature, which installs a counting `GlobalAlloc`
   (allocations, peak bytes in use) and reports those instead — frame
   timings from an instrumented allocator are never recorded. The counting
   allocator is the workspace's only permitted `unsafe` outside audited
   code: a ~30-line forwarding wrapper to `System`, feature-gated and
   denied by lint everywhere else.
4. **Determinism**: simulation is pure float math over SoA arrays with a
   seeded RNG; spatial-hash queries iterate entity indices, never the
   `HashMap`, so output order is stable. Each report carries an
   `interactions` count and a `state_checksum` (FNV over the full state) —
   same `(scene, seed, ticks)` must always reproduce both, which
   unit-tests assert at the core level and which makes cross-machine
   frame-time comparisons meaningful (same work verified).
5. **ECS tick, not wall frames**: the harness loop calls `app.update()`
   N times at a fixed dt resource — no `App::run()`, no real-time
   accumulation. `Query` iteration order in `sync_positions` is allowed to
   be arbitrary since each entity is written its own sim value and the
   checksum comes from the sim, not the components.
6. **Reporting shape**: single JSON object per run with `scene`, `ticks`,
   `warmup_ticks`, `seed`, `entities`, `interactions`,
   `state_checksum`, and exactly one of `frame_ms` (timing pass) or
   `allocs` (allocation pass). `xtask perf measure` runs both passes for
   every scene of every scope member with a bin target: `runtime` and
   `memory` maps in the record (trend tier), never gated.
7. **Benches**: `bw-core` gains criterion (`sim`) and iai-callgrind
   (`iai_sim`) bench targets as dev-dependencies only — zero footprint
   cost. iai instruction counts are gate-tier with per-bench budgets
   (~2% headroom over baseline: counts vary ~0.1% across builds of
   identical code from address-dependent alignment); criterion is the
   wall-clock trend tier run manually/nightly. Both measure the same
   exercise functions over `Sim`/`SpatialHash`.
8. **Scene presets**: `"bounce"` (1k entities, 200x200) and
   `"bounce-big"` (10k entities, 600x600). Galleries grow by adding
   presets — each becomes a new trend series automatically.

## Alternatives considered

- **Run under `App::run()` with virtual clock injection** — couples the
  harness to Bevy's time internals across versions; rejected for the plain
  `app.update()` loop.
- **Measure allocation and timing in one pass** — rejected: the counting
  allocator perturbs timing (the reason ADR 0001 D8.8 exists).
- **`stats_alloc` crate instead of a hand-rolled allocator** — small
  dependency saved by ~30 lines we can audit entirely; revisit if the
  counter grows requirements (per-site tracking, etc.).
- **Gate on frame-time p99** — rejected now: trend tier per ADR 0001 D2;
  revisit only with a dedicated measurement machine.

## Consequences

- Every future gallery binary copies a small, explicit contract instead of
  inventing benchmark hooks; `xtask` discovers scenes via `--perf-scenes`,
  so no tooling changes per gallery.
- Frame-time trend data starts flowing from the first demo (baseline:
  bounce p50 ≈ 0.107 ms/tick, bounce-big p50 ≈ 1.19 ms/tick on the dev
  machine, 600 ticks).
- The sim checksum pins determinism: any nondeterminism introduced later
  (parallel systems, HashMap-order dependence, unseeded randomness) shows
  up as checksum mismatch across machines/runs, not as mysterious
  benchmark noise.
- Instruction budgets make ECS/system refactors gate-checked: the three
  baselines (step 13,005; hash build 646,330; hash query 1,491,261) now
  ratchet.
- The renderer eventually required by visual galleries will need its own
  headless-strategy decision (offscreen wgpu or headless feature of
  bevy_render) — deliberately out of scope here.
