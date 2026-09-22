# Build configuration

The workspace defines three custom profiles, kept as separate objectives per
the [Rust Performance Book](https://nnethercote.github.io/perf-book/) —
optimizing one of "fast binary", "small binary", "fast builds" optimizes
against the others, so we build and measure each concern under its own
profile instead of compromising all of them in `release`.

| Profile | Inherits | Purpose | Use for |
|---|---|---|---|
| `quick` | `dev` | iteration speed | day-to-day development, tests |
| `runtime` | `release` | maximum runtime speed | benchmarks, frame-time measurement |
| `size` | `release` | minimum binary size | size budgets, distribution builds |

All three keep `panic = "unwind"`: unwinding is required by
rust-analyzer/Salsa cancellation and test observations.

## Commands

```bash
# Day-to-day development
cargo run                          # uses dev (or set CARGO_PROFILE_DEV_… overrides)
cargo check --workspace
cargo test --workspace

# Performance measurement (benches, frame timing)
cargo build --profile runtime
cargo bench --profile runtime      # criterion benches, once they exist

# Size measurement and distribution builds
cargo build --profile size         # artifacts land in target/size/
cargo xtask perf measure           # records sizes + benches (ADR 0001, D1)
```

Notes:

- Custom-profile artifacts go to `target/<profile-name>/` (e.g.
  `target/size/bw-demo`), unlike `dev`/`release` whose names differ from
  their directories.
- `runtime` keeps symbols (`strip = "none"`) so profiler and crash-report
  output remains useful; `size` strips them because its product is the
  shipped artifact.
- `lto = "fat"` and `codegen-units = 1` make `runtime`/`size` builds slow —
  by design. Never use them for iteration; that is what `quick` is for.

## Measurement guidance

Which profile a measurement must use, accuracy rules, and how measurements
are gated and recorded are defined by
[ADR 0001](adr/0001-mandatory-performance-tracking.md) (see D2–D3): sizes are
measured on `size`-profile builds, benches on `runtime`, and `quick` is never
a measurement target.
