# Case study 0001: Dense SpatialGrid — the measured cost of leaving the stack

- Date: 2026-09-22
- Data-producing commits: `05cbf79` (dhat allocation harness), `887cfc3`
  (SpatialGrid refactor), `8b959b5` (steady-state gate, ADR 0004)
- Policy references: [ADR 0004](../adr/0004-allocation-policy.md),
  [ADR 0005](../adr/0005-storage-policy-stack-by-default.md)

**The quotable claim:** replacing a per-frame-rebuilt `HashMap` of
per-cell `Vec`s with a persistent dense grid eliminated 99.4% of all
allocations (657,475 → 1,670 per run), made every steady-state tick
allocation-free (p50 and max = 0), and cut frame time 2.7× — while the
sim's observable behavior stayed bit-identical. Each eliminated
allocation was worth ~67 ns, *more* than a malloc/free pair costs,
because the allocator call was only part of the bill; the rest was cache
locality recovered by staying on contiguous storage
([ADR 0005](../adr/0005-storage-policy-stack-by-default.md)).

## Situation

The first gallery (`bw-demo`, scenes: `bounce` = 1k entities,
`bounce-big` = 10k) rebuilt its broadphase every tick as a
`HashMap<(i32,i32), Vec<usize>>` and queried it into a freshly grown
`Vec`. The dhat allocation pass (setup/warmup/measured windows, per-tick
series, per-function probes) attributed essentially all churn to
`step_sim`, and the callsite report named the line: `Vec::push →
grow_one → grow_amortized`, 6.5M blocks / 208 MB on bounce-big — one
small `Vec` per occupied cell per tick.

The Bevy schedule itself was measured nearly allocation-free
(4 blocks per run, scene-independent), so the debt was ours.

## Change

`SpatialHash` → `SpatialGrid` (crates/core/src/sim.rs): dense counting
sort over the fixed arena — per-cell counts, prefix-summed cell starts,
entity indices in one contiguous `entries` array — allocated once at
scene setup and rebuilt in place. Pair queries write into a
caller-owned scratch (`query_pairs_into`, cleared not reallocated),
sized to the entity count at setup. Cross-checked: interaction counts
(25,331 / 286,399) and sim checksums identical before and after.

## Evidence

| Metric (600 ticks)                | before        | after         |
|-----------------------------------|---------------|---------------|
| steady blocks/tick, p50 / max     | 994 / 1,002   | **0 / 0**     |
| … bounce-big                      | 9,874 / 9,890 | **0 / 0**     |
| total allocations per run         | 657,475       | **1,670**     |
| … bounce-big                      | 6,517,028     | **1,693**     |
| frame p50, runtime profile        | 0.107 ms      | **0.040 ms**  |
| … bounce-big                      | 1.187 ms      | **0.431 ms**  |
| broadphase query instructions     | 1,490,567     | **391,502**   |
| warmup-window blocks              | 60,114        | **559**       |

Regressions, accepted deliberately:

| Metric                     | before    | after          |
|----------------------------|-----------|----------------|
| rebuild instructions       | 646,330   | 705,832 (+9%)  |
| peak live bytes (bounce)   | 327 KB    | 728 KB         |
| … bounce-big               | 2.2 MB    | 5.9 MB         |

## Reading the numbers

On `bounce`, per tick: 107 µs → 40 µs saves 67 µs for 994 eliminated
allocations ≈ **67 ns per removal**. A malloc/free fast-path pair costs
roughly 20–50 ns, so the allocator call alone explains only about half
the win. The rest is the invisible part of [ADR
0005](../adr/0005-storage-policy-stack-by-default.md)'s cost model: no
per-allocation headers or metadata, contiguous `entries` scans instead
of chasing per-cell `Vec` pointers, and the entity arrays no longer
evicted from cache by allocation traffic. (The query's 3.8× instruction
drop conflates locality with the algorithm change; the bounded claim is
the *total* effect.)

The two regressions are the known trade: O(arena/cell²) storage held
permanently instead of O(occupied cells) churned, and an O(cells)
prefix-sum scan in the rebuild. Both are watched by the trend tier
(peak bytes) and instruction budgets (rebuild 720,000 / query 400,000).

## Enforcement that came with it

- `[steady.<scene>]` budgets gate the worse of p50/max blocks per tick
  at zero (`cargo xtask perf check`).
- `crates/core/tests/steady_alloc.rs` asserts zero blocks over 100 warm
  ticks of `step` + `rebuild` + `query_pairs_into` in a dedicated dhat
  test binary.
- The dhat callsite report (`target/perf/dhat-heap.json`) remains the
  tool for naming any future culprit.

## Reproduce

```sh
cargo xtask perf check                       # gates incl. steady zero-alloc
cargo xtask perf measure --out record.json   # full record
cargo run -p bw-demo --features perf-alloc -- \
  --perf-headless --scene bounce-big --ticks 600 --seed 42 --perf-alloc --json
cargo bench -p bw-core --bench iai_sim --profile runtime
```
