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
- [ ] **tree-playground** — Make tree parameters playable without code edits: CLI flags on the render path (fork spread, jitter, wind amplitude/frequency, canopy widths; `--seed` already exists) so tuning is a render-and-look loop. Feeds tree-tune. Added 2026-09-22.
- [ ] **tree-bench** — Bench tier for `bw_core::tree::pose_into` mirroring the sim (criterion trend + iai gate with per-bench budget entries). Deferred until the tree shape settles so budgets don't churn mid-tuning. Added 2026-09-22.
- [ ] **renderer-adr** — Sketch the real-renderer ADR: graduating galleries off the SVG path (offscreen wgpu vs `bevy_render` headless), feature diff plus size/dep budget step per ADR 0002. Added 2026-09-22.

## Done

(none yet — checked items move here before eventual deletion)
