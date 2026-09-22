# ADR 0005: Storage-class policy — the stack is the default

- Status: Accepted
- Date: 2026-09-22

## Context

ADR 0004 made steady-state heap allocation a gated zero. The measurement
work behind it also surfaced *why* in a form worth making explicit,
because it generalizes beyond allocation counts to layout choices:

A heap allocation charges four times, and only the first is visible at
the call site:

1. **Entry** — allocator bookkeeping: size-class lookup, free-list pop,
   metadata write, lock in threaded programs. The Rust Performance Book:
   "a global lock, non-trivial data-structure work, and possibly a
   syscall."
2. **Rent** — first touch of the fresh memory (page faults or cold cache
   lines), then pointer-chasing for the data's lifetime. Stack memory is
   hot by construction: the top of the stack is in L1 because execution
   was just there.
3. **Exit** — the matching free, which lands later and typically in a
   different function's frame.
4. **Externalities** — fragmentation taxing future allocations, and cache
   pollution evicting *unrelated* data.

Because costs 3–4 are deferred and distributed, profilers under-attribute
them and teams systematically underestimate allocation. The evidence:
rustc historically gained ~1% wall-clock per ~10 allocations removed per
million instructions (Rust Performance Book); and our own spatial-grid
refactor ([case study 0001](../case-studies/0001-spatial-grid-zero-alloc.md),
commit 887cfc3) saved ~67 ns per allocation eliminated — above the raw
cost of a malloc/free pair — with the remainder being locality and cache
recovery.

## Decision

1. **Stack is the default storage class** for hot-path data whose
   lifetime is bounded by a frame or tick. Registers and inline storage
   count as stack here.
2. **Cross-tick data** lives in pre-sized persistent contiguous (SoA)
   buffers under ADR 0004's allocate-once discipline — persistent buffers
   are the stack's sibling: entered once, hot forever.
3. **Hot-path layouts avoid per-element heap indirection** (`Vec<Vec<T>>`,
   per-node `Box`, string keys in maps): flat storage plus indices;
   iteration should walk arrays in order, not chase pointers.
4. **Heap by deliberation**: dynamic size or lifetime, asset loading,
   cold paths — legitimate. The steady-state gate catches accidental
   per-frame cases mechanically; exceptions are visible budget raises.
5. **Fixed-small-N inline types** (`SmallVec`, `ArrayVec`) are an
   acceptable bridge when the maximum is known small; benchmark when
   adopting (they trade per-operation overhead for heap avoidance).

## Alternatives considered

- **"Modern allocators are fast enough"** — rejected on the evidence
  above: the call-site cost is the minority of the bill.
- **Ban the heap entirely** — rejected: assets, dynamic scenes, and setup
  windows are legitimate (ADR 0004); the policy targets per-frame paths.
- **Leave it as an unwritten norm** — rejected: this cost model is easy
  to under-weight in review precisely because its evidence is scattered;
  writing it down with our own numbers anchors future design debates.

## Consequences

- Layout choices on hot paths (flat vs nested, inline vs boxed) get
  explicit review attention, with this ADR as the reference.
- The steady gate and probes enforce the mechanical part; this ADR covers
  the design-time part (a layout can be allocation-free and still
  pointer-hostile).
- Evidence for future storage decisions accumulates as case studies under
  `docs/case-studies/`.
