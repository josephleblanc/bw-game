# perf-data

Append-only measurement-record history ([ADR 0001](https://github.com/josephleblanc/bw-game/blob/main/docs/adr/0001-mandatory-performance-tracking.md),
D5). This orphan branch keeps perf history out of `main`'s blame; nothing
here is ever edited, only appended.

## Layout

- `records/records-YYYY-MM.jsonl` — one record per line (compact JSON),
  sharded by month to keep appends conflict-free.

## Who writes

- The weekly `perf-trend.yml` job appends automatically (runner
  `github-perf-trend`).
- Anyone, by hand: from `main`, run
  `cargo xtask perf measure --out record.json --runner local`, then append
  `jq -c . record.json >> records/<shard>` here and push. Hand records
  with `runner: local` are how designated-machine measurements land
  (D8.1: cross-env comparisons are flagged in reports, never merged).

## Reading

```sh
git worktree add ../perf-data perf-data
cargo xtask perf report --records ../perf-data/records --since 30d
```

The report diffs only records sharing the latest record's
runner/rustc/host, so CI and local records never contaminate one trend
line.
