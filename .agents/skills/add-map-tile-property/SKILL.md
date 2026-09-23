---
name: add-map-tile-property
description: Add a new per-tile property (channel) to the bw-game map — temperature, fertility, qi, wetness, pH — following the repo's SoA/checksummed pattern. Use whenever adding, changing, or querying per-tile map state in bw_core::map, or when a gameplay system (farming, climate, feng shui, buildings) needs to read or write a tile property. Also covers repinning the golden hashes and running the gate ritual.
---

# Adding a map tile property (bw-game)

Per-tile properties are **channels**: one flat `Vec<T>` per property on
`Map`, over the same row-major index space (`z * width + x`), SoA,
allocated once at generation (ADR 0004). The law lives in
`docs/maps/map-design.md` → "Tile channels (the property pattern)";
**temperature and fertility are the reference implementations** — read
them in `crates/core/src/map.rs` before starting. Do not build a
generic channel registry: every channel is explicit, and that
explicitness is the point.

## Step 0 — decide a channel is the right shape

A channel fits when the data is **per-tile**, lives as long as the
map, and is read by systems rather than rendered per-frame. If the
state belongs to an entity (a walker's carry), it is a component; if
there is one per world (the clock), it is a resource.

Pick the storage type by semantics, and write the units into the
field's doc comment:

| semantics                    | type                          | precedent     |
| ---------------------------- | ----------------------------- | ------------- |
| scalar with units            | `f32`                         | temperature   |
| fraction / bounded ratio     | `f32`, asserted into `[0, 1]` | fertility     |
| categorical state            | small `#[repr(u8)]` enum      | terrain       |
| orthogonal booleans          | hand-rolled bitflags          | occupancy     |

Answer these before writing code — they decide every step after:

1. What are the units and the legal range?
2. Is it **generated** (a property of the place: noise) or **event-
   written** (gameplay state: starts neutral), or both?
3. Who reads it, and does anything **derive** from it (a cache that
   must rebuild when it changes, like cost does from occupancy)?

## The recipe

Work in `crates/core/src/map.rs` unless a step says otherwise. The
compiler enforces several steps for you (every `GenParams` literal and
`Map` construction site must be updated) — let it.

1. **`GenParams` += tuning knobs** — but only real knobs. If the
   semantics pin the range (fertility is a fraction), add no knob.
   Every new field must join `checksum()` (fold `f32` via
   `to_bits()`) *and* the `every_generation_parameter_moves_the_checksum`
   test. This is the tuning-churn tripwire: retuning must trip a
   golden hash somewhere.
2. **Take the next `CH_*` noise stream id** if generation fills the
   channel (stone 0, water 1, temp 2, fert 3 — next is 4). Channels
   share `octaves`/`scale`; amplitude and shape belong to the new
   knobs. Update the const's doc comment listing.
3. **`Map` struct += the `Vec<T>` field**, doc comment carrying units
   and who writes it. Initialize with capacity in `generate()`
   (and only there — `blank()` routes through `generate`).
4. **Fill it in `generate()`'s row-major loop**, sampling noise at
   tile centers (`x + 0.5`, `z + 0.5`) like the terrain does. If
   terrain should damp or gate it (fertility ×0.60 on stone), say so
   in a comment. Assert new params are finite/ranged at the top of
   `generate()`.
5. **Accessor**: `pub fn <name>_at(&self, tile: Tile) -> Option<T>` —
   `None` out of bounds, fail closed like every other query.
6. **Mutator**: `pub fn set_<name>(&mut self, tile: Tile, value: T)
   -> bool` — false = out-of-bounds no-op. `assert!` value invariants
   (finite, range): a NaN reaching an `f32` channel poisons the
   checksum silently. If the channel feeds a derived cache, rebuild
   it here — the `set_occupancy` → `rebuild_cost` precedent. Events
   call this from `Update`-side systems; **FixedUpdate reads
   channels, never writes them** (no per-tick allocation, ever).
7. **`checksum()`**: fold the channel behind a fresh 8-byte tag
   (`*b"TEMPERAT"` style) after the existing layers. Tags keep byte
   streams from aliasing.
8. **Tests** (in `map.rs`'s `mod tests`, fixed seeds):
   - character bands: gradient/envelope/damping laws (see
     `temperature_gradients_southward_within_its_envelope` and
     `fertility_is_a_fraction_and_respects_terrain`);
   - write round-trip is **checksum-exact** (`channel_writes_round_trip_checksum_exactly`):
     set → checksum moves, restore → checksum returns, OOB → no-op;
   - extend the parameter-sensitivity test with each new knob.
9. **Repin the goldens that were meant to move — same commit, with a
   comment naming why.** A channel changes two pins:
   - `default_generation_checksum_is_pinned` in `map.rs` (the channel
     joined the checksum), and
   - `golden_render_hash_is_pinned` in
     `crates/map-gallery/src/svg.rs` — the render caption carries the
     map checksum as provenance, so it moves too. This is correct
     behavior, not collateral damage.
   Run the tests, read the new values from the failure output,
   `printf '0x%016X\n' <decimal>` to hex them, pin, and note the
   repin date in the test's doc comment.
10. **Consumers, if the tier wants them** (reads are free): gallery
    scene summaries (`SceneSummary` in
    `crates/map-gallery/src/lib.rs` — note it derives `PartialEq`,
    not `Eq`, once it holds an `f32`), viewer readouts, headless
    reports. The gallery scene table's `GenParams` literals must gain
    the new fields — give scenes honest character (the archipelago is
    tropical, the badlands hot).
11. **Docs + backlog**: add the channel to the "later channels" list
    in `docs/maps/map-design.md` (mark landed), and close a backlog
    Done entry describing the channel and its pins.
12. **Gates** — the repo's law (every commit CI-green):
    ```sh
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cargo check --target wasm32-unknown-unknown -p bw-core -p bw-map-gallery
    cargo xtask perf check
    ```
    `unwrap`/`expect` are denied lints — use `let … else { panic!(…) }`
    in tests. Artifact budgets carry 1–2% headroom; a channel adds
    code and per-tile data, so check the perf findings — if a budget
    breaches, the remedy is a visible budget raise with a note, not
    silence.

## Worked example: temperature (the whole recipe in one diff)

- Knobs: `temp_base_c: 15.0`, `temp_span_c: 14.0`, `temp_noise_c: 2.5`
  (°C; base at the z=0 north edge, gradient to base+span south).
- Stream: `CH_TEMP = 2`.
- Fill: `gradient = base + span * z / (height-1).max(1)` (the `.max(1)`
  guards single-row maps), plus `(fbm - 0.5) * 2 * noise` — symmetric
  around the gradient, so the envelope test bounds it exactly.
- Mutator asserts `is_finite()`; checksum tag `*b"TEMPERAT"`.
- Both goldens repinned in the landing commit, comments dated.

## Laws you must not break

- **One mutator per channel.** Every write the checksum can't see is
  a determinism bug waiting for a replay test.
- **No `NaN`/`inf` in `f32` channels** — assert in the mutator, at
  the door.
- **Generation params join the checksum** the day they are born.
- **Goldens repin deliberately or not at all** — an unexplained golden
  change in a diff is a review flag, not a chore.
- **FixedUpdate reads only**; channels are data, not systems — a
  channel that wants to *tick* (vegetation regrowth) is a later
  design note, not a loophole.
