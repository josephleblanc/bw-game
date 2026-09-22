# ADR 0002: Adopt Bevy with a contained feature footprint

- Status: Accepted
- Date: 2026-09-22

## Context

bw-game's stated goals include a contained dependency footprint: Bevy is the
engine choice, but its default feature set is large (renderer, audio, UI,
assets, …) and would permanently anchor the project's binary size, compile
time, and dependency graph to a maximum instead of a deliberate selection.
The first Bevy dependency is also the first real test of the budget process
(ADR 0001, D3): it is a large, justified step-up rather than an accretion.

Toolchain fact: bevy 0.19.1 requires rustc 1.95.0. Our pinned measurement
toolchain is already 1.95.0; only the scaffold-default MSRV (1.91) lagged.

## Decision

1. **Adopt `bevy` 0.19.1 via the facade crate with
   `default-features = false` and no additional features** for now. The
   minimal set (bevy_app, bevy_ecs, and their support crates) is sufficient
   for headless gallery work; every future feature enablement (renderer,
   assets, audio, …) is its own visible, budget-cited change.
2. **Bevy is a dependency of gallery/demo crates only** (`bw-demo` today).
   `bw-core` stays engine-agnostic: simulation and domain logic in plain
   Rust, benchmarkable without the engine, with Bevy glue living in the
   gallery binaries. This keeps the core benches fast to compile and keeps
   engine churn out of the game's logic.
3. **Raise MSRV to 1.95**, aligning with the pinned measurement toolchain
   (ADR 0001, D8.2). There is no environment where we build with an older
   rustc on purpose, so carrying a lower MSRV is untested ceremony.
4. **wasm32 stays green**: `cargo check --target wasm32-unknown-unknown` is
   a blocking gate for every scope member (wired in `xtask perf
   measure`/`check`). Bevy minimal compiles for wasm today; any future
   feature that breaks this must be caught, not discovered.
5. **Budget step-up recorded with this ADR as its justification**: size
   289,800 → 1,030,544 measured (budget 300,000 → 1,050,000); external
   deps 12 → 78 (budget exact); `syn` (2/3) and `hashbrown` (0.16/0.17)
   allowlisted as duplicates originating in Bevy's own graph.

## Alternatives considered

- **bevy 0.18.1 to preserve MSRV 1.91** — starts the project a version
  behind for a constraint nothing enforces; rejected.
- **Direct `bevy_ecs`/`bevy_app` crates instead of the facade** — slightly
  leaner graph, but loses the single-version facade upgrade path and makes
  feature accounting messier; revisit only if the facade's overhead shows up
  in measurement.
- **Default features (windowed app from day one)** — rejected: footprint
  anchor, and the headless-first gallery strategy (ADR 0001, D4) doesn't
  need a renderer yet.

## Consequences

- Binary size and dep count budgets stepped up ~3.6×/6.5× in one visible,
  justified change — the ratchet now guards a real engine, not a
  hello-world.
- Compile times grow accordingly (first clean dev build ~10 s, size-profile
  builds slower still); the advisory compile-time trend will quantify this.
- Bevy's upstream duplicate versions are permanently grandfathered until
  Bevy unifies them; the allowlist makes that visible and re-checkable.
- Adding any renderer/asset/audio feature later requires: feature diff,
  measured size/dep delta, budget raise with note, and attribution
  (`xtask perf attribute`) naming what grew.
