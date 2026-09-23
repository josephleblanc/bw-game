# Perf review ritual

The operating cadence for the measurement system ([ADR 0001](adr/0001-mandatory-performance-tracking.md),
D9). The gates catch deterministic regressions per-PR; this ritual is what
keeps the *trend* data — and the budgets themselves — honest over time.

## Weekly (automated)

`perf-trend.yml` (Mondays 09:00 UTC, or manual dispatch) measures, appends
a record to the `perf-data` branch, and posts the rendered report as a
comment on the rolling `perf-review` issue (label: `perf-review`). The
report carries: budget state with headroom, open `backlog.md` items with
age (stale ones flagged), 30-day deltas per metric, flagged findings each
with a recommended action, and the skipped-metrics log.

## Biweekly (human, ~20 minutes)

Walk the `perf-review` issue top to bottom:

1. **Findings**: for each flagged delta, confirm or dismiss it. A
   confirmed finding gets an owner and an issue of its own; a dismissed
   one gets a reply saying why (noise, one-off, accepted trade).
2. **Backlog**: every open `backlog.md` item the report flags as older
   than two weekly cycles gets a decision now — schedule it, split it, or
   close it. The backlog is the "at the latest" net for deferred work; it
   must not become a graveyard.
3. **Budget staleness**: check `reviewed` dates in
   `perf/budgets.toml`. Any budget not reviewed in ~6 weeks either gets
   its `reviewed` date bumped with intent, is ratcheted down (always
   allowed), or is retired. Budgets that no longer reflect intent are
   debt.
4. **Skips**: every skip in the report needs either a fix (install the
   tool, add the metric) or a conscious "fine as skip" — no skip should
   survive two reviews unexamined.
5. **Headroom**: artifacts or benches sitting within ~2% of their budget
   get a decision now: ratchet, raise with a note, or plan the work —
   not discovered later by a blocked PR.

## Milestone gate

No release cuts with red budgets or unreviewed `perf-review` findings
(`release.yml` enforces the former mechanically; the latter is on you).

## Commands

```sh
gh issue list --label perf-review          # the rolling issue
cargo xtask perf report --since 30d        # render locally (needs a
                                           # perf-data records checkout)
cargo xtask perf check                     # the gate itself
```

Records live on the `perf-data` branch (append-only JSONL,
`records/records-YYYY-MM.jsonl`); see that branch's README. To inspect
locally: `git worktree add ../perf-data perf-data` then
`cargo xtask perf report --records ../perf-data/records --since 30d`.
