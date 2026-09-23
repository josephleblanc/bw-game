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

- [ ] **tree-tune** — Refine the golden-scheme tree visuals: fork spread, ratio/angle jitter, canopy density and layer widths, plus the minor nits from the first render pass (faint shadow overshoot at the map edge, checker contrast). Owner wants hands-on play with renders first. Added 2026-09-22.
- [ ] **tree-playground** — A live windowed viewer for tuning: the tree animating in real time (growth + sway under the tick contract's accumulator + interpolation), param nudges (fork spread, jitter, wind amplitude/frequency, canopy widths) and seed reroll via keyboard, on-screen readouts of current values. Backing: the **feature-gated Bevy renderer** — `bevy`'s winit/render/sprite features enabled only behind a dev-only `viewer` cargo feature on bw-tree-gallery, so the default graph, wasm gate, and size budgets stay untouched. First slice of renderer-adr (decision made 2026-09-22; see also the feature-diff + budget-note obligations there). Feeds tree-tune. Added 2026-09-22.
- [ ] **param-presets** — Move scene presets from Rust structs to data (TOML/RON via the existing serde graph): the first miniature asset system, and the persistence layer for playground findings (shareable tuned presets). Optional file-watch reload comes with the live viewer. Added 2026-09-22.
- [ ] **render-snapshot** — Pin tuned visuals with a determinism freebie: the SVG render is a pure function of (scene, seed, params), so a hash of the output file is a golden-image regression test without images. Add after the next tree-tune pass. Added 2026-09-22.
- [ ] **tree-bench** — Bench tier for `bw_core::tree::pose_into` mirroring the sim (criterion trend + iai gate with per-bench budget entries). Deferred until the tree shape settles so budgets don't churn mid-tuning. Added 2026-09-22.
- [ ] **renderer-adr** — Sketch the real-renderer ADR: graduating galleries off the SVG path. The escalation ladder: (1) offline batch — done, today's default; (2) live windowed viewer — now decided as the feature-gated Bevy renderer via tree-playground, which makes this ADR partially implemented before it is written; (3) full adoption (renderer features in the default graph). The ADR records the feature diff, the size/dep budget step per ADR 0002 when/if the renderer enters the default graph, the accumulator/interpolation point, and the input→action layer. Added 2026-09-22.

## Done

- [x] **tick-contract** — Formalized the tick rate as law: `bw_core::time` (`TICK_HZ = 60`, `SIM_DT`) with the scheduling contract in its module docs (fixed sim steps only; renderers sample, never advance; wall-clock accumulators only at an interactive boundary). All hard-coded `1.0/60.0`s in the galleries, benches, and tests now route through it; a pin test guards the benchmark convention (600 ticks = 10 s). Added 2026-09-22. Done 2026-09-23.
