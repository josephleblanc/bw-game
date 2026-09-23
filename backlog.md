# Backlog

Deferred-but-wanted work, tracked so nothing rots silently. The weekly
perf-trend job renders every open item below (with its age) into the
[perf-review](docs/perf-review.md) report; anything older than two weekly
cycles without progress is flagged there for a schedule / split / close
decision.

Conventions (what `cargo xtask perf report` parses):

- one item per line under `## Open`: `- [ ]` open, `- [x]` done
- short id in bold, then the description
- `Added YYYY-MM-DD.` at the end so age is computable

Close an item by checking its box; move it to Done when archiving.

## Open

- [ ] **tree-tune** — Refine the golden-scheme tree visuals: fork spread, ratio/angle jitter, canopy density and layer widths, plus the minor nits from the first render pass (faint shadow overshoot at the map edge — also visible in the live viewer, where the shadow ellipse spills past the tile rug; checker contrast). Hands-on play is now possible in the live viewer (run instructions in crates/tree-gallery/README.md); tuned values reproduce in `--render` because the viewer shares the headless scenes' per-anchor seeds. Added 2026-09-22.
- [ ] **param-presets** — Move scene presets from Rust structs to data (TOML/RON via the existing serde graph): the first miniature asset system, and the persistence layer for playground findings (shareable tuned presets). Optional file-watch reload comes with the live viewer. Added 2026-09-22.
- [ ] **render-snapshot** — Pin tuned visuals with a determinism freebie: the SVG render is a pure function of (scene, seed, params), so a hash of the output file is a golden-image regression test without images. Add after the next tree-tune pass. Added 2026-09-22.
- [ ] **tree-bench** — Bench tier for `bw_core::tree::pose_into` mirroring the sim (criterion trend + iai gate with per-bench budget entries). Deferred until the tree shape settles so budgets don't churn mid-tuning. Added 2026-09-22.
- [ ] **renderer-adr** — Sketch the real-renderer ADR: graduating galleries off the SVG path. The escalation ladder: (1) offline batch — done, today's default; (2) live windowed viewer — done, the feature-gated `viewer` cargo feature on bw-tree-gallery (tree-playground); (3) full adoption (renderer features in the default graph). Remaining for the ADR: record the tier-2 decision and its measured non-impact (deps stay 77, budgets untouched; the viewer graph builds ~308 unique crates — 231 of them only behind the flag), the size/dep budget step required if tier 3 ever pulls the renderer into the default graph per ADR 0002, the accumulator/interpolation point (now proven in viewer.rs), and the input→action layer (first slice landed there). Added 2026-09-22.

## Done

- [x] **tree-playground** — The live windowed viewer: tree animating in real time under the tick contract (fixed SIM_DT steps in FixedUpdate; the Update render mirror alpha-interpolates between the last two sim poses), keyboard nudges on fork spread / angle jitter / wind amp+freq / canopy scale (with clamps), seed reroll (R), growth replay (G), pause, zoom, and an on-screen readout with live t/tick/alpha. Backed by the dev-only `viewer` cargo feature (bevy winit/render/sprite/text + png): default graph unchanged at 77 deps, wasm gate and budgets untouched. Input maps through a pure key→Action transition; `--viewer-shot <path>` is a screenshot smoke hook (captures at t=1.5s and t=8s, exits at t=10s); viewer logic tests run under `--features viewer`. Feeds tree-tune. Added 2026-09-22. Done 2026-09-22.
- [x] **tick-contract** — Formalized the tick rate as law: `bw_core::time` (`TICK_HZ = 60`, `SIM_DT`) with the scheduling contract in its module docs (fixed sim steps only; renderers sample, never advance; wall-clock accumulators only at an interactive boundary). All hard-coded `1.0/60.0`s in the galleries, benches, and tests now route through it; a pin test guards the benchmark convention (600 ticks = 10 s). Added 2026-09-22. Done 2026-09-23.
