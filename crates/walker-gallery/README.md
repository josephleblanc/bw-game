# bw-walker-gallery

The third gallery — and the first 3D one: walking characters. A stick
figure locomotes through a procedural, speed-driven gait while the
crate's library side (`CharacterMovementPlugin`) provides the 3D-
compatible Bevy plugin every surface consumes.

Simulation lives in `bw_core::character` (engine-agnostic,
allocate-once per ADR 0004, with hand-rolled vec3/quat math in
`bw_core::math` so `bw-core` stays dependency-free); this crate owns
the plugin, the Bevy ECS glue, the headless measurement contract, and
two visual surfaces that read identical world-space data.

## The plugin

`CharacterMovementPlugin` (the crate's library target) owns locomotion
under the tick contract (`bw_core::time`):

- **Sim steps in `FixedUpdate`**, exactly once per run, always by
  `SIM_DT` — never the wall clock. Live apps let Bevy's `Time<Fixed>`
  accumulator drive it; the headless harness runs the schedule
  directly (`world.try_run_schedule(FixedUpdate)`), so the same
  `(scene, seed, ticks)` reproduces the same checksum in both hosts.
- **Controllers write `MovementInput` before the step** — the
  scripted `CirclePath` ships with the plugin; the viewer's keyboard
  controller lives in `viewer.rs`. Input is data.
- **The render mirror samples, never advances**: `sync_bones` runs in
  `Update` (the `MovementMirror` set) and lerps each bone between the
  last two sim poses at `MovementAlpha`, which live hosts refresh from
  the accumulator's overstep fraction and headless hosts leave at 1.0.

Components: `Walker` (sim state + pose pair), `BoneSegment`
(world-space bone endpoints — what any 3D renderer reads), and
`CirclePath`. Helpers: `spawn_walker`, `state_checksum`,
`total_footfalls`.

## The character

- **Rig**: a 13-bone stick figure (hips, torso, head, two arms with
  forearms, two legs with feet) as flat arrays, parents before
  children, so forward kinematics is one forward loop. Rest pose is a
  ~1.86 m standing figure; bone lengths never change — renderers
  build geometry once and only transform it.
- **Gait**: a single axis — `speed` — blends idle → walk → run
  continuously (`smoothstep` bands), so running is a faster target,
  not a state to switch. Gait phase advances as `2π·speed/stride`:
  cycles lock to ground distance, keeping foot contact honest.
- **The cycle**: sinusoidal thigh swing, half-rectified knee flexion
  gated into swing, pelvis yaw with torso counter-yaw (the head
  counter-counters to stay level), arm counter-swing with flexing
  elbows, and a root that bobs twice per cycle with lateral weight
  shift. Jumping and punching are planned as additive channels on the
  same phase state (see `action-states` in backlog.md).

## Scenes

| id            | what                                                       |
| ------------- | ---------------------------------------------------------- |
| `walk`        | one figure on a 5 m ring at walk speed (1.35 m/s)          |
| `walk-run`    | the same ring at run speed (3.6 m/s)                       |
| `crowd`       | 96 seeded walkers on nested rings (1248 segments)         |
| `mandala-100` | stress: 100 figures, a painting that re-forms every ~9 s   |
| `mandala-1k`  | stress: 1,000 figures, re-forms every ~25 s                |
| `mandala-10k` | stress: 10,000 figures, re-forms every ~71 s               |

### The mandala stress scenes (a painting that walks)

`mandala-100` / `mandala-1k` / `mandala-10k` scale the crowd until the
sim has to earn it, with choreography instead of seed scatter: every
figure walks a circle of **one shared radius and speed**, but the
circles' centers are scattered across the canvas (a jittered sunflower
distribution) with alternating directions and seeded phases. The loops
interlock and cross — a dense mesh with no empty ring gaps, and
counter-rotating pairs stream through every shared intersection in
constant near-miss traffic.

Each figure's color is frozen at spawn: the viewer samples a
procedural painting (a blue circle, a red circle, a rotated purple
square, an orange square, a green ring on dark ink) at the figure's
t = 0 tile. At t = 0 the crowd reads as colored tiles forming that
image; as the loops carry the figures across each other's territory
the shapes dissolve — a "blue circle" is just a set of blue tiles, now
scattered along many different loops. Because every loop shares the
period `T₀ = 2πr/s`, the painting snaps back exactly at every multiple
of `T₀` (~9 s at 100, ~25 s at 1k, ~71 s at 10k). The layout is
seeded: the same `(scene, seed)` reproduces the same field.

## Running

```sh
# headless measurement pass (the ADR 0003 D4 contract, same flags as
# the other galleries)
cargo run -p bw-walker-gallery -- --perf-headless --scene walk --json
cargo run -p bw-walker-gallery -- --perf-scenes

# the mandala stress tiers, headless (seeded; same seed, same field)
cargo run -p bw-walker-gallery -- --perf-headless --scene mandala-10k --json

# visual output: self-contained animated SVG (a perspective 3D camera
# projecting the same world-space poses the viewer renders) and a
# static frame — open in any browser (mandala-100 only; the bigger
# tiers are viewer territory)
cargo run -p bw-walker-gallery -- --render walk.svg --render-frame walk-frame.svg

# walker-playground: the live 3D environment (dev-only build; see below)
cargo run -p bw-walker-gallery --features viewer -- --viewer

# mandala-view: the live stress scene (same dev-only feature)
cargo run -p bw-walker-gallery --features viewer -- --viewer --scene mandala-1k
```

The render path allocates freely — it is *not* the measured hot loop.
The measured loop (the plugin's step + mirror, pumped by the harness)
is zero-allocation at steady state, gated by `[steady.*]` entries in
`perf/budgets.toml` and unit-tested with dhat in
`bw-core/tests/steady_character_alloc.rs`.

## walker-playground (the live 3D environment)

`--viewer` opens a 1280×800 3D scene: a 24×24 m checkerboard meadow,
the driven stick figure (warm ink), seven ambient circle-walkers
(slate), blob shadows, and a follow camera at the SVG renderer's
establishing-shot orientation (the eye→target direction held constant
while tracking the selected figure — offline renders and the live view
share an angle). It exists only behind the dev-only `viewer` cargo
feature (bevy's winit/3D-pbr/sprite/text stack), so the default
dependency graph (77 external crates), the wasm gate, and the size
budgets are untouched.

`Tonemapping::None` keeps the dev feature list free of the
tonemapping-LUT/zstd dependency chain (the flat palette needs no film
curve).

Control is colony-sim style. Click a figure to select it (a gold ring
marks the selection; the camera and keyboard follow), click the ground
to send it walking there (a red disc marks the goal until arrival —
the plugin's `MoveTarget` controller, pinned by headless tests).
Keyboard drive applies to the selected figure and overrides a send;
clicking the selected figure again returns control to the player
figure.

| input       | action                                    |
| ----------- | ----------------------------------------- |
| L-click     | figure: select · ground: send walking     |
| W / S       | walk forward / brake to idle              |
| Shift       | toggle run (3.6 m/s — watch the blend)    |
| A / D       | turn                                      |
| Space       | jump (hold: bounce on every landing)      |
| F           | punch (hold: chained cycles)              |
| P           | pause (the sim, not the renderer)         |
| R           | reset the player figure home              |
| − / =       | zoom out / in                             |
| Esc         | quit                                      |

The readout (top-left) shows scene/seed with the selection, live speed
with the gait band, the active action (air height / punch phase),
`t / tick / alpha`, heel strikes, and the key map.

Smoke test without touching the keyboard:

```sh
cargo run -p bw-walker-gallery --features viewer -- \
  --viewer --viewer-shot /tmp/walker.png
# writes /tmp/walker-{walk,air,strike,run,sent}.png (walk at t=1.5s,
# hop apex ~2.3s, run-punch strike ~5.25s, full run 6.5s, and a
# scripted click-to-move send walking to its goal disc at 7.4s),
# exits at t=8.6s
```

Tests for the viewer's pure logic (key map, follow-camera geometry,
ground/figure picking, shot naming, scene builders) run with
`cargo test -p bw-walker-gallery --features viewer`; the default test
pass never compiles the windowing stack.

## mandala-view (the live stress scene)

`--viewer --scene mandala-100|1k|10k` opens the stress scene: an orbit
camera steep enough to read the t = 0 painting, the readout's entity
counts, and a countdown to the next re-formation. Colors never change —
each figure carries its tile's color forever; only the arrangement
dissolves and re-forms. Same dev-only `viewer` feature as the
playground; same tick contract (the sim steps at `SIM_DT`, the camera
alone reads wall time).

| keys   | action                                        |
| ------ | --------------------------------------------- |
| 1 / 2 / 3 | respawn at 100 / 1,000 / 10,000 figures    |
| − / =  | zoom out / in                                 |
| Space  | pause (the sim, not the camera)              |
| Esc    | quit                                          |

Smoke test without touching the keyboard (captures the painting just
after launch, then the dispersed field at ~40% of the period):

```sh
cargo run -p bw-walker-gallery --features viewer -- \
  --viewer --scene mandala-10k --viewer-shot /tmp/mandala.png
# writes /tmp/mandala-image.png + /tmp/mandala-dispersed.png, exits
```
