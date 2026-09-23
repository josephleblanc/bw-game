# bw-tree-gallery

The second gallery: procedurally generated golden-ratio trees that grow
from a straight trunk and sway in the wind, viewed through the fixed
tilted camera of PS2-era tactical RPGs (Final Fantasy Tactics, Disgaea).

Simulation lives in `bw_core::tree` (engine-agnostic, allocate-once per
ADR 0004); this binary owns the Bevy ECS glue, the headless measurement
contract, and the interim visual output.

## The tree

- **Straight trunk**: generation starts as a single segment pointing
  straight up.
- **Golden-ratio forks**: every node forks into a near-straight *leader*
  (slight seeded lean, ~0.82 length ratio) and a *side branch* whose
  angular offset from the parent is the explement of the golden angle,
  `π − 137.5078° ≈ 42.49°`, with seeded jitter. Side length ratio is
  `1/φ ≈ 0.618`. An occasional third shoot forks at a `1/φ`-scaled
  fraction of the spread on the other side.
- **Semi-fractal**: self-similar at every depth, but seeded jitter on
  every angle/length draw plus the golden angle's irrationality (fork
  patterns never exactly repeat) keep it from being a perfect fractal.
- **Animation**: a pure `pose(t)` function — branches sprout in order
  (a child starts when its parent is 55% grown) and bend under a
  deterministic two-frequency gust field, with flexibility rising toward
  the tips. Same `(scene, seed, t)` always produces the same pose.

## The camera

`src/proj.rs` implements the tactical look as a fixed projection:
a 36° depression / 45° yaw diamond checkerboard ground, upright
billboard trees standing on their tiles (height maps 1:1, exactly how
FFT/Disgaea sprites behave), and branch shadows sheared onto the ground
along a fixed sun direction.

## Scenes

| id           | what                                                    |
| ------------ | ------------------------------------------------------- |
| `tree`       | a single oak, gentle breeze                              |
| `tree-storm` | the same oak under strong, fast wind                      |
| `tree-grove` | a 3×3 stand of younger trees across the checkerboard      |

## Running

```sh
# headless measurement pass (the ADR 0003 D4 contract, same flags as bw-demo)
cargo run -p bw-tree-gallery -- --perf-headless --scene tree --json
cargo run -p bw-tree-gallery -- --perf-scenes

# visual output: self-contained animated SVG (growth + sway, looping) and
# a static fully-grown frame — open in any browser
cargo run -p bw-tree-gallery -- --render tree.svg --render-frame tree-frame.svg
```

The animated SVG uses SMIL path morphing with one path per (tree, depth)
group plus two canopy layers and one shadow layer, all driven by the same
sampled `pose(t)` frames — no scripting, no dependencies, no renderer
features. When the real renderer ADR lands (ADR 0002 keeps Bevy minimal
for now), the Bevy camera copies the same projection constants.

The render path allocates freely — it is *not* the measured hot loop. The
measured loop (`step_trees` + `sync_segments`) is zero-allocation at
steady state, gated by `[steady.*]` entries in `perf/budgets.toml`.
