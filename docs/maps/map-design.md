# Map and tile design

The design reference for the game's ground: the tile map, its
coordinates, its layers, generation, pathfinding, and the first
building slice. Written 2026-09-23 before implementation, the same way
`docs/animation/animation-set.md` preceded the action tiers — it pins
conventions so the code doesn't have to relitigate them, and it phases
the work for the gallery cadence. Companion decisions live in the
ADR index: the tick contract and determinism (backlog/ADR 0003
discipline), allocation law (ADR 0004), storage law (ADR 0005), and
the tactical camera (ADR 0006).

## Principles (inherited, not new)

- **Deterministic from data**: `(seed, params)` reproduces the map
  bit-for-bit; scenes checksum their state; renders hash to
  golden-test themselves. The tree gallery's per-anchor seeds and the
  `render-snapshot` backlog item are the precedents.
- **Allocate once, zero per tick** (ADR 0004): the map allocates at
  generation; pathfinding queries reuse a scratchpad and an
  caller-owned out-vector, never allocating per query.
- **Dependency-free core** (ADR 0002): `bw_core::map` hand-rolls its
  noise from the existing `Rng` — no noise crate, no pathfinding crate.
- **Renderer-free first** (ADR 0003): headless scenes + checksums,
  then the SVG render, then the feature-gated live viewer.
- **The camera is settled** (ADR 0006): orthographic, 49° azimuth,
  ~15.5° elevation. The map viewer promotes the orientation constants
  to a shared module so map and walker viewers cannot drift.

## Coordinates and tiles

- **Tile size: 1 m**, matching the walker playground's checkerboard
  and the walkers' f32-meter world (stride 1.35 m ≈ 1.35 tiles per
  cycle; `MoveTarget` arrival radius 0.30 m sits comfortably inside a
  tile).
- **`Tile { x: i32, z: i32 }`** — a Copy/Eq/Hash newtype from day one
  (semantic-identity discipline; no raw `(i32, i32)` propagating
  through APIs). World↔tile converts at one place:
  `tile_of(pos)` = `floor` of each component (floor, never truncate —
  `as` casts go the wrong way for negatives), and
  `tile_center(tile)` = center at `(x + 0.5, z + 0.5)`.
- **Tile (x, z) occupies `[x, x+1) × [z, z+1)`** in map-local
  coordinates; row-major index `z * width + x` (renders read rows
  naturally). Map-local now; a world offset arrives only when a
  mechanic demands multiple maps or a larger world.
- **Fixed size, power-of-two-ish defaults** (scene presets choose;
  64×64 the default). Not chunked, not infinite — the trigger to
  revisit is a concrete perf or scale need, recorded here so it's a
  decision, not drift.

## Representation: flat arrays, one per layer

The map is SoA flat `Vec`s over the same index space, built once at
generation and mutated only by events (building placement). The
`bw_core::sim` spatial grid's rebuild-on-change semantics (and its
rebuild-matches-fresh test) are the precedent for the derived cache.

1. **`terrain: Vec<Terrain>`** — the ground itself:
   `Soil` (default), `Stone` (paved, faster), `Water` (blocking).
   A `#[repr(u8)]` enum for now; the migration trigger to data-driven
   `TerrainId` + registry is the `param-presets` backlog item landing
   (when tile definitions want to live in TOML).
2. **`occupancy: Vec<Occupancy>`** — event-driven state on top:
   bitflags `BLOCKED` (a completed building), `RESERVED` (a
   construction blueprint holding the tiles). Walkability is derived:
   terrain ≠ Water and occupancy empty.
3. **`cost: Vec<u8>`** — the pathfinding cache derived from the two
   above (0 = blocked; otherwise a terrain multiplier in fixed point,
   e.g. Soil 10, Stone 9). Rebuilt on any mutation; cheap enough at
   these sizes to rebuild whole rather than dirty-tile.
4. Later channels, each its own array when it earns one — see
   [Tile channels](#tile-channels-the-property-pattern) for the law
   and the recipe: **qi / spirit richness** per tile (the xianxia
   flavor — where spirit herbs want to grow, what makes a site good
   for cultivation), **zones** (player designations: grow, harvest,
   stockpile), and vegetation with its own `SIM_DT` tick when regrowth
   exists. Temperature and fertility landed 2026-09-23 as the first
   two, proving the pattern.

## Tile channels (the property pattern)

A *channel* is a per-tile property the whole map owns at once:
temperature, fertility, qi. The landed law (temperature and fertility
are the reference implementations, and the
`add-map-tile-property` skill encodes the recipe):

- **One flat `Vec<T>` per channel** on `Map`, over the same row-major
  index space — SoA, cache-friendly, allocated once at generation
  (ADR 0004). The type follows semantics: `f32` for scalars with
  units documented in the field's doc comment, small `#[repr(u8)]`
  enums for categorical state, bitflags for orthogonal booleans. No
  generic channel registry: each channel is explicit, and its
  accessors stay concrete.
- **Generated from noise when the property is a place, written by
  events when it is gameplay.** Generation gets the next
  `CH_*` stream id and only real tuning knobs in `GenParams` (a range
  pinned by semantics — fertility is a fraction — needs none). Every
  new `GenParams` field joins `checksum()` and the parameter-
  sensitivity test; that is the tuning-churn tripwire.
- **Reads are free and everywhere** (`*_at(tile) -> Option<T>`, `None`
  out of bounds — fail closed). **Writes go through one mutator**
  (`set_*(tile, value) -> bool`, false = out-of-bounds no-op) so the
  checksum sees every change; gameplay systems call mutators from
  `Update`-side events, never per fixed tick. If a channel ever feeds
  a derived cache, its mutator rebuilds it — the `set_occupancy` →
  cost-cache precedent.
- **The checksum folds every channel** behind an 8-byte tag, f32
  values through `to_bits` (mutators assert finiteness/range first,
  so the digest stays well-defined). Golden hashes are repinned
  deliberately, in the same commit that changed the law.
- **Tests every channel carries**: determinism (covered by the whole-
  map checksum pin), parameter sensitivity, fixed-seed character
  bands, write round-trip checksum-exact, out-of-bounds fail-closed.

## Generation

Seeded, pure, checksummed: `Map::generate(seed, params)`. Value noise
hand-rolled from the existing `Rng` — a hashed lattice with smooth
interpolation, a few octaves — because the core is dependency-free
(ADR 0002). v0 shapes: stone patches where a noise channel crosses a
threshold, ponds where a second channel pools; parameters (octaves,
thresholds, water fraction) are structs today and TOML when
`param-presets` lands. Every generation parameter participates in the
checksum so tuning churn is caught by the golden hashes.

## Pathfinding

A\* over the **4-neighborhood** (no diagonals at v0: corner rules are
where grid pathfinding stops being boring-simple; 8-way with
no-corner-cutting arrives only if paths look visibly wrong).
Manhattan heuristic (admissible on 4-way). Determinism beyond the
usual fixed-point inputs: **fixed neighbor order** (E, N, W, S —
pinned by test) and a total tie-break on `(f, then index)` so the
same map and query always produce the same path.

Allocation contract, in the ADR 0004 spirit:

- A `Pathfinder` owns its scratchpad, allocated once: an open
  `BinaryHeap<(Key, u32)>`, and `g_score` / `came_from` /
  visited-stamp arrays with a **generation counter** — stamps, not
  clearing, make queries O(visited) with zero allocation.
- Results land in a caller-owned reused `&mut Vec<Tile>` (the
  `CharacterPose` pattern: the buffer is the API).

**Send-time smoothing** (`smooth_route` + `line_of_sight`): the raw
4-neighborhood route is string-pulled at send time — keep exactly the
waypoints needed so consecutive kept ones have walkable line of sight,
so stair-stepped diagonals collapse into straight legs. The sight
check is a conservative supercover: an exact lattice-corner crossing
counts **both** flanking tiles, and the crossing order is decided by
integer cross-multiplied comparison, so the perfect diagonal's corner
touches can never be missed to float rounding. The output is always a
subsequence of the A* route (start and goal kept) — walks stay
grid-faithful, only ever aiming at tiles A* chose. Send-time by
design: `Update` on click, never per tick (the fluency half of the
stair-shuffle fix, docs/animation/qualities.md "Path fluency").

## Walker integration

- **`FollowPath`** — a plugin-side controller next to `CirclePath` and
  `MoveTarget`, owning the remaining tile list and writing input per
  tick (input-only, no `Commands` in `FixedUpdate` — the established
  law). It advances a waypoint when the walker is inside the arrival
  radius, steering with the shared turn law (turns-in-stride: cruise
  scaled by alignment, a planted pivot only past a quarter-turn); the
  final waypoint aims at the tile center. Cancelling is a component
  removal in `Update`, exactly like `MoveTarget` today.
- **Send becomes routed**: the viewer's ground click converts to a
  tile, runs one A\* query (event-driven, in `Update`), and spawns the
  `FollowPath`. Straight-line `MoveTarget` remains as the primitive
  the tests pin; the gallery scenes choose which a click uses.

## Buildings: the first slice

A building is a **footprint** — an anchored rectangle of tiles —
nothing more, until the animation work tier can swing a hammer:

1. **Designate**: the player picks a rectangle; tiles become
   `RESERVED`, a ghost renders (wireframe footprint).
2. **Assign**: a walker gets a `FollowPath` to an adjacent walkable
   tile of the footprint's perimeter.
3. **Work**: the future two-handed-swing tier plays at the site;
   progress accrues (v0: time-based, no animation dependency).
4. **Complete**: tiles flip to `BLOCKED`, the ghost swaps to a solid
   render, the cost cache rebuilds, and pathfinding routes around it
   from the next query on.

The point of the slice is the **contract**, not the look: reservations
block paths, completion changes walkability, and walkers end up
standing at the right tile. Walls first (1×N footprints); multi-tile
rooms and door logic follow when wall placement feels good.

## Gallery phasing

Each tier lands like the walker tiers: headless + tests first, then
SVG, then the viewer, gates throughout.

1. **`bw_core::map`** — `Tile`, `Terrain`, `Map` (layers, checksum,
   world↔tile), seeded generation, pinning tests (bounds, negative
   coordinates, determinism, gen-parameter sensitivity). Tracked as
   `map-core`.
2. **`map-gallery` skeleton** — headless scenes (generate, checksum,
   count walkable tiles, run recorded path queries), SVG ortho render
   with the shared ADR 0006 camera constants, golden render hash.
3. **Pathfinding + `FollowPath`** — the `Pathfinder` with its
   scratchpad and a steady-state zero-alloc test driving queries;
   headless scenes asserting routes avoid water and blocked tiles.
4. **Map viewer** — feature-gated live surface: pan (drag and/or edge
   scroll — the new camera capability a map needs; zoom already rides
   the ortho scale), walkers placed on generated maps, click-to-send
   routed through A\*, goal marker generalized to path destinations.
5. **Building slice** — footprints, reservations, ghost render,
   click-drag wall designation, completion flipping walkability.
6. **Later, with triggers**: zones and designations; vegetation tick;
   the qi channel (feng shui reading of a site — the ACS-shaped
   flavor, worth its own design note when it lands); multi-level
   terrain.

## Absent by design

- **Hex tiles** — square matches the meter-world, the checkerboard,
  the 4-way paths, and ACS itself; hex's advantages (equidistant
  neighbors) solve problems we don't have.
- **Multi-level elevation at v0** — triples pathfinding and rendering
  surface; revisit when a mechanic (cliffs, multi-storey sect halls)
  demands it.
- **Infinite/chunked maps** — fixed size until a measured need.
- **A pathfinding or noise crate** — the core stays dependency-free;
  both are small, testable, and ours.
