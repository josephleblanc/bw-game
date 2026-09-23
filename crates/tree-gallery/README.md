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

# tree-playground: the live windowed viewer (dev-only build; see below)
cargo run -p bw-tree-gallery --features viewer -- --viewer
```

The animated SVG uses SMIL path morphing with one path per (tree, depth)
group plus two canopy layers and one shadow layer, all driven by the same
sampled `pose(t)` frames — no scripting, no dependencies, no renderer
features. When the real renderer ADR lands (ADR 0002 keeps Bevy minimal
for now), the Bevy camera copies the same projection constants.

The render path allocates freely — it is *not* the measured hot loop. The
measured loop (`step_trees` + `sync_segments`) is zero-allocation at
steady state, gated by `[steady.*]` entries in `perf/budgets.toml`.

## tree-playground (live viewer)

`--viewer` opens a 1280×800 window with the tree animating in real time:
growth then continuous sway, on the same tactical projection as the SVG.
It exists only behind the dev-only `viewer` cargo feature (bevy's
winit/render/sprite/text stack + PNG encode for smoke shots), so the
default dependency graph (77 external crates), the wasm gate, and the
size budgets are untouched — the first `--features viewer` build compiles
wgpu and takes noticeably longer, once.

The viewer is the first real consumer of the tick contract
(`bw_core::time`): sim time advances only in fixed `SIM_DT` steps in
`FixedUpdate` (Bevy's accumulator is the contract's interactive
boundary), and the render mirror in `Update` only *samples* — it
interpolates between the last two sim poses at the accumulator's
overstep fraction, so render lags the sim by under one tick and never
leads it. Keyboard input maps through a pure `key → Action` function
onto `Playground` state (the first sliver of the input→action layer):

| keys            | nudge (step)                            | clamps        |
| --------------- | --------------------------------------- | ------------- |
| ← / →           | fork spread (1°)                        | 10–90°        |
| ↑ / ↓           | fork angle jitter (0.01)                | 0–0.6         |
| A / D           | wind amplitude (0.02 rad)               | 0–1.2         |
| W / S           | wind gust frequency (0.1)               | 0.1–6         |
| Q / E           | canopy blob scale (0.1)                 | 0.2–3         |
| − / =           | zoom out / in (×1.12)                   | 0.3–4         |
| R               | reroll seed (restarts growth)           |               |
| G               | replay growth from t=0                  |               |
| Space           | pause / resume                          |               |
| Esc             | quit                                    |               |

The readout (top-left) shows scene/seed, the tunables, live
`t / tick / alpha`, and the key map. Spread/jitter changes and rerolls
regenerate the trees (same per-anchor seeds as the headless scenes, so a
tuned look reproduces in `--render`).

Smoke test without touching the keyboard:

```sh
cargo run -p bw-tree-gallery --features viewer -- \
  --viewer --viewer-shot /tmp/viewer.png
# writes /tmp/viewer.png (t=1.5s) + /tmp/viewer-grown.png (t=8s), exits at t=10s
```

Tests for the viewer's pure logic (lerp endpoints, connectivity under
interpolation, key map, clamps, reroll determinism, camera fit) run with
`cargo test -p bw-tree-gallery --features viewer`; the default test pass
never compiles the windowing stack.
