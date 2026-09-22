# ADR 0004: Allocation policy — allocate once, steady-state zero

- Status: Accepted
- Date: 2026-09-22

## Context

ADR 0001 D8.8 split timing and allocation measurement into separate passes;
the dhat-rs validation pass then produced the data this policy rests on. On
the first gallery: steady state was ~994 blocks/tick (bounce) and ~9,874
(bounce-big), ~100% from `step_sim` — per-cell `Vec` growth in the spatial
hash rebuild plus the pairs `Vec`. The Bevy schedule itself allocated ~4
blocks per *run* (scene-independent), and `sync_positions` allocated
nothing. In other words: the engine inherits us ~zero per-tick allocation
debt; churn is ours, structural, and avoidable.

Stated goal: **allocate once per scene and avoid per-frame churn** —
including transitive allocations like repeated `iter()`/`collect()` chains
that build fresh `Vec`s every frame where `filter()`/`map()` over
references or reused buffers would do. This is not a ban on allocation:
scene setup, asset loading, and growth-capacity establishment are
legitimate one-off costs; deliberate copies (the render mirror) are
measured, not prohibited. The line is fuzzy at the edges — this ADR makes
the core of it machine-checked and leaves the edges to trend data.

## Decision

1. **Steady-state zero-alloc rule (gate).** Every gallery scene must show
   **0 blocks/tick** in the harness's measured window — gated on the worse
   of p50 and max, so a single allocating tick fails. Budgets live in
   `[steady.<scene>]` in `perf/budgets.toml`; raises follow ADR 0001 D3
   mechanics (visible diff, note, justification). Warmup and setup are
   excluded by window design: capacities and dhat's callsite bookkeeping
   settle there.
2. **Reuse over reallocate (API shape).** Hot-path data structures are
   persistent and rebuilt in place (`SpatialGrid::rebuild`); per-frame
   outputs go through caller-owned scratch buffers
   (`query_pairs_into(&mut Vec)`, cleared not reallocated). Allocating
   convenience wrappers (`query_pairs`) are allowed off the per-frame path
   only. Scratch buffers are sized at setup with explicit headroom; a scene
   that outgrows its scratch fails the gate visibly rather than growing
   silently mid-frame.
3. **Setup window is amortized (trend).** App/schedule construction, scene
   seeding, scratch sizing, and future asset loads are recorded
   (setup/warmup totals, live-bytes drift) but not gated. Live-bytes drift
   per tick is watched as the leak signal (currently a fixed ~24 B/tick
   from the engine schedule machinery, scene-independent).
4. **Enforcement ladder.** Unit tier: dhat heap-usage tests in `bw-core`
   (`tests/steady_alloc.rs`, own test binary so the global allocator sees
   no other tests' allocations) assert zero blocks for `Sim::step`,
   `SpatialGrid::rebuild`, and `query_pairs_into`. Harness tier: the
   per-tick series and `[steady.*]` budgets gate whole scenes. Attribution
   tier: `Probe` totals-delta guards per function (hand-written today;
   extraction into an `alloc_probe` proc macro once probe sites multiply),
   dhat's JSON for callsite naming, valgrind DHAT for opt-in deep dives.
5. **Probe semantics.** Probes snapshot monotonic `HeapStats` totals
   (blocks and bytes) at entry and accumulate the delta at drop — totals,
   never current-bytes deltas, so made-and-freed transients are counted.
   Probes assume main-thread execution (bevy minimal runs schedules on the
   calling thread); revisit when the task pool arrives with assets.
6. **Zero-copy stance.** Copies are measured, not banned: `sync_positions`
   is a deliberate per-tick copy (render mirror) whose *volume* is
   trend-tier; revisiting it (components-as-storage) trades against SoA
   locality and is a data decision for the renderer era. `clone()` off the
   hot path is fine; on the hot path it shows up in the probes and earns
   justification.

## Alternatives considered

- **Gate on cumulative allocation totals** — rejected: conflates setup
  with steady state; the pass would fail on every asset load.
- **p50-only gate** — rejected: a once-per-frame allocation (growing
  buffer) hides behind a good median; max closes that hole.
- **Keep the HashMap spatial hash, add capacity retention** — rejected:
  per-cell `Vec`s remain; the dense grid removes per-cell heap storage
  entirely and (measured) costs ~9% more instructions to rebuild but 3.8×
  less to query, with zero allocation.
- **`SmallVec`/`ArrayVec` per-cell storage** — rejected: still per-cell
  storage to copy and size; the dense counting-sort layout needs none.
- **Accept churn until frame time suffers** — rejected: accretion is the
  default game-project failure mode ADR 0001 exists to prevent.

## Consequences

- Measured on the first gallery: steady state 994/9,874 blocks per tick →
  **0/0** (p50 and max), total tick instructions roughly halved, and the
  interactions checksum is bit-identical — the refactor changed the memory
  behavior, not the simulation semantics.
- The dense grid's rebuild is O(cells): trivial at 200×200, a ~1.4 MB
  memset at 600×600 (still measured faster overall). Huge sparse arenas
  may eventually want an open-addressing variant — a data-driven decision
  the benches will surface.
- The zero-alloc rule may need conscious exceptions later (streaming,
  dynamic scenes with growing entity counts); those go through visible
  budget raises with notes, exactly like size and instruction budgets.
- The `alloc_probe` proc macro remains future work: two probe sites don't
  yet justify two crates; the hand-proven guard shape is the spec.
