# bw-map-gallery

The fourth gallery: generated tile maps from `bw_core::map`, tier 2
of [the map design](../../docs/maps/map-design.md) — headless scenes
plus the tactical SVG ortho render. No engine dependency at this
tier: the map is static, so there is no ECS to drive and no tick to
step. Pathfinding (`bw_core::path::Pathfinder`, zero-alloc A*)
arrived with tier 3 and is exercised by this crate's tests; the
**live map viewer** — walkers standing on generated terrain,
click-to-send routed through A*, right-drag pan — lives in the
walker playground: `cargo run -p bw-walker-gallery --features
viewer -- --viewer --scene map`.

## Scenes

| id            | character                                            |
| ------------- | ---------------------------------------------------- |
| `meadow`      | `GenParams::default()`: ponds, stone patches, mostly soil |
| `archipelago` | wet (22% budget), broad chunky features, one octave less |
| `badlands`    | dry, big slow stone masses                            |
| `blank`       | the control — the blank law as a scene, all soil       |

Every scene is a pure parameter set: `(scene, seed)` reproduces the
map bit-for-bit, and the report carries the map checksum (which folds
in every generation parameter — tuning churn trips the golden hashes
in `bw-core`'s tests).

## Run

Headless report (the perf-harness contract, same flags as the other
galleries — a map is static, so the pass measures generation, the one
real op):

```sh
cargo run -p bw-map-gallery -- --perf-headless --scene meadow --seed 42
```

Open a map — the reason this gallery exists:

```sh
cargo run -p bw-map-gallery -- --scene meadow --seed 42 --render /tmp/meadow.svg
# then open /tmp/meadow.svg in a browser
```

The render is an orthographic projection along the shared ADR 0006
orientation (`bw_core::camera` — the same constants the walker
viewer's camera reads, so the surfaces cannot drift): 49° azimuth,
~15.5° elevation. Soil is the plate; stone and water paint as
per-tile subpaths; faint 1 m grid lines and stronger 8 m sector lines
carry the scale; the caption carries full provenance (scene, seed,
checksum, walkable count). `--render` output is a pure function of
`(scene, seed)`, pinned by a golden hash test — no image needed.

## Tests

`cargo test -p bw-map-gallery` pins: the scene table, the
meadow-is-defaults law, the blank-scene-equals-`Map::blank` law,
determinism and seed sensitivity per scene, summary partitioning,
scene character bands, the affine ortho projection, layer presence in
the SVG, and the golden render hash.
