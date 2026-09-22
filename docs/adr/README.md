# Architecture Decision Records

ADRs capture *why* a decision was made, not just what was decided. Code and
configuration show the current state; an ADR preserves the reasoning, the
alternatives we rejected, and the consequences we accepted when that state was
chosen. When a future change would invalidate that reasoning, the ADR is what
tells us to stop and reconsider rather than drift.

## When to write an ADR

Write one whenever a decision:

- constrains future work across crates or milestones (module boundaries,
  data-layout choices, engine-integration strategy),
- establishes or changes a project-wide practice or tooling contract
  (CI gates, benchmarking, release process),
- moves or could move a performance budget (new heavy dependency, feature-set
  changes, renderer or asset-pipeline choices), or
- is expensive to reverse.

Smaller calls (function design, crate-internal structure) belong in code
review, not here.

## Conventions

- ADRs live in `docs/adr/` as `NNNN-short-slug.md`, numbered monotonically
  starting at `0001`. Numbers are never reused or resequenced.
- Status is one of: **Proposed**, **Accepted**, **Superseded by NNNN**,
  **Deprecated**. A proposed ADR becomes Accepted when the decider approves it;
  no further ceremony required.
- Accepted ADRs are immutable except for their Status line. To change a
  decision, write a new ADR that supersedes the old one and update the old
  Status line to point at it. We do not rewrite history.
- Non-normative implementation notes may be appended under a clearly marked
  `## Implementation notes` section as reality diverges from plan.

## Template

```markdown
# ADR NNNN: <short title in the imperative>

- Status: <Proposed | Accepted | Superseded by NNNN | Deprecated>
- Date: YYYY-MM-DD

## Context

What forces are in play — goals, constraints, facts about the codebase
or environment — that make this decision necessary *now*.

## Decision

The decision, stated actively ("We adopt…", "We do not…"). Split large
decisions into numbered sub-decisions.

## Alternatives considered

Options rejected and why. Keeping these prevents re-litigating settled
questions from scratch.

## Consequences

What becomes easier, what becomes harder, what we accept, and the
signals that should trigger revisiting this decision.

## Implementation notes (optional)

Phasing, pointers to the code that realizes the decision.
```

## Index

| ADR    | Title                                                     | Status   |
|--------|-----------------------------------------------------------|----------|
| 0001   | [Mandatory performance tracking for time and space](0001-mandatory-performance-tracking.md) | Accepted |
| 0002   | [Adopt Bevy with a contained feature footprint](0002-adopt-bevy-minimal-footprint.md) | Accepted |
| 0003   | [Headless gallery harness and benchmark contracts](0003-headless-gallery-harness.md) | Accepted |
| 0004   | [Allocation policy — allocate once, steady-state zero](0004-allocation-policy.md) | Accepted |
| 0005   | [Storage-class policy — the stack is the default](0005-storage-policy-stack-by-default.md) | Accepted |
