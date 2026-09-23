# Movement & tone: the qualities vocabulary

Status: living document (first cut 2026-09-23, from the influence sort in
[`influence-sorter.html`](influence-sorter.html)). This is where tone gets
named so it can be chosen deliberately instead of emerging by accident.
When a tone decision hardens into law (a pinned constant, a chosen
behavior), it gets a dated decision line under its axis — and moves to an
ADR if it outgrows this page, the way the camera angles did (ADR 0006).

The thesis: every nebulous quality below is a set of hidden constants.
Naming the quality is step one; finding its measurable proxies is step two.

## The LLM-native pillar (cross-cutting)

The game is LLM-native, and no existing game captures this direction
adequately — which is why it is a pillar here rather than one more axis
with a games column.

What is known (2026-09-23):

- **LLM-driven characters** — both the colony's NPCs and hostile NPCs in
  nearby colonies with conflicts.
- **A Dungeon-Master-like LLM** — helps manage conflict pacing and global
  narratives.
- **An LLM-driven world-state** — spawning events, changing global
  constants, expanding or authoring new maps.

What is not yet decided: the granularity of character control (per-action
tool use vs. some kind of scripting) and how it plays with a more
mechanical AI tree.

Tone implications for the axes below:

1. **Two axes exist because of it** — *agent presence & autonomy* and
   *directoredness (the DM)*. Both lack full precedent; only partial
   anchors exist in shipped games.
2. **Glance readability becomes load-bearing.** Emergent, LLM-driven
   behavior that cannot be read at a glance breaks the player's trust in
   what they are seeing. The busier the agents, the harder the readability
   law must work.
3. **Pacing gets an owner.** Conflict pacing is the DM's dial, not an
   emergent accident — the world-pace axis gains the question "who turns
   the dial, and how visible is their hand?"
4. **Layering keeps the vocabulary valid regardless of granularity.** The
   locomotion axes are the deterministic substrate: LLMs set intents,
   mechanical controllers (`steer_toward`, `FollowPath`) execute them,
   animation reads them out. The granularity decision lives above that
   line and does not invalidate anything under it.

## The axes

Each axis: what it is, what wrong looks like, the hidden constants it
decomposes into (current values live in
`crates/walker-gallery/src/lib.rs` unless noted), then the 2026-09-23
sort. Influences/counter-influences are recorded as sorted facts;
interpretations under them are working hypotheses until a decision line
says otherwise.

### 1. Path fluency — freedom of movement

How the actor relates to the discrete route: hugging tile centers versus
cutting corners into arcs.

**Wrong looks like:** the stair-step shuffle — routed diagonal sends in
the map viewer halt-and-pivot on every 90° leg. (Mechanism: 4-neighborhood
routes lay diagonals as ~1.3 m stair legs, and a waypoint releasing at
`FOLLOW_ARRIVE` 0.30 off-center presents a bearing error just past
`MOVE_TURN_HALT` (90°), so nearly every leg trips the turn-in-place rule.)

**Hidden constants:** `FOLLOW_ARRIVE` (0.30), `MOVE_TURN_HALT`, corner-cut
/ string-pull tolerance (none yet), waypoint release vs. turn sharpness,
any-angle routing (currently 4-neighborhood only).

- Influences: Amazing Cultivation Simulator, Dwarf Fortress, Final Fantasy
  Tactics, Dungeon Keeper 2
- Counter-influences: Songs of Syx
- Working hypothesis: the sort pulls toward *grid-faithful and fluent*,
  not free striding — FFT is tile-locked and never shuffles because turns
  happen in stride during the walk. The likely fix family is turns-in-
  stride plus corner tolerance, not abandoning tile routes.

### 2. Agility — sprite agility

Latency from command to visible response; whether turns happen in stride
or plant-first.

**Wrong looks like:** a turret that rotates before it obeys.

**Hidden constants:** `MOVE_TURN` (2.6 rad/s), `MOVE_TURN_GAIN` (6.0),
`MOVE_TURN_HALT` (90°), acceleration ramp (none today — cruise is
instant, which makes every stop stark), first-response latency.

- Influences: Zelda: BotW, Amazing Cultivation Simulator, Baldur's Gate
  II, Hollow Knight
- Counter-influences: Shadow of the Colossus, RimWorld, Kenshi
- Note: Shadow of the Colossus as a *counter* is informative — deliberate
  input lag as an aesthetic is explicitly rejected for colony command
  feel.

### 3. Weight — the impression of mass

How the figure starts, stops, and carries momentum. Lumbering = heavy
plus slow to respond.

**Hidden constants:** acceleration/deceleration ramps (none today), turn
authority curve, bob amplitude coupling. Genre tension noted: xianxia
implies lightness (qinggong glide); the colony-sim baseline implies
groundedness.

- Influences: Zelda: BotW, Amazing Cultivation Simulator
- Counter-influences: Kenshi
- The sort pulls toward *light* — effortless poise over haulage.

### 4. Gait identity — the walk cycle as character

What the walk cycle says about who this is: scholar mince, sword stride,
child bob.

**Hidden constants:** stride frequency vs. speed coupling, bob amplitude,
arm carriage, lean. See world pace below — gaits must be **speed
-parametric**, not tuned to one cruise speed.

- Influences: Kenshi, Amazing Cultivation Simulator, Zelda: BotW
- Counter-influences: (none sorted)

### 5. Idle & reaction life

Idle shifts, look-ats, flinches at events. Statues between orders = wrong.

Under the LLM pillar this axis grows a verbal/emotive surface: reaction
life becomes the visible skin of LLM cognition (Baldur's Gate II banter
is the sprite-era ancestor of that).

- Influences: Amazing Cultivation Simulator, Kenshi, Stardew Valley,
  Baldur's Gate II
- Counter-influences: Dwarf Fortress, RimWorld

### 6. Glance readability — who is doing what, without UI

Whether a player can tell who is doing what from the tactical camera
alone.

**Hidden constants:** silhouette distinctness, palette roles, motion-as
-information (a walker visibly *en route* vs. ambling).

- Influences: Amazing Cultivation Simulator, StarCraft, Kenshi
- Counter-influences: Dwarf Fortress, Factorio
- Load-bearing under the LLM pillar: emergent behavior must stay glance
  -readable or trust in the simulation breaks.

### 7. Camera temperament

The angles are already law (ADR 0006: 49° azimuth, ~15.5° elevation,
shared in `bw_core::camera`). Behavior is not: pan damping, follow lead,
snap vs. ease.

**Hidden constants:** pan response (currently linear drag-to-focus),
follow lead distance, zoom easing.

- Influences: Amazing Cultivation Simulator, Kenshi, Zelda: BotW
- Counter-influences: (none sorted)

### 8. World pace — contemplation vs. crisis, and subverting it

Day length, the tempo the player's eye settles into.

**Decision (2026-09-23): pace subvertability is a designed pleasure, not
a leak.** The base pace targets the contemplative middle — but players
working against it (ACS turtle yaoguai at one end; a max-Constitution
bull yaoguai with speed talismans at the other) experience overcoming the
barrier as satisfying and empowering. Consequences:

- The middle ground remains the tuning target.
- Locomotion and animation must be **speed-parametric across the whole
  band** — figures must look fine at a crawl and at a sprint. A little
  silly is allowed; immersion-breaking is not.
- **Hidden constants:** the speed band endpoints (walk/run/trudge),
  stride-frequency coupling to actual speed, the silly-vs-broken
  boundary.

- Influences: Amazing Cultivation Simulator, Stardew Valley, Baldur's
  Gate II, Factorio, Dwarf Fortress, Kenshi
- Counter-influences: Songs of Syx, Zelda: BotW, Diablo II, They Are
  Billions, The Sims
- Note: BotW as a *pace* counter while being a body-axes influence —
  excellent movement in service of a more urgent game than this one.

### 9. Agent presence & autonomy — the LLM-driven character

How visibly characters think: initiative, idiosyncrasy, speech — how much
presence an agent projects while remaining glance-readable (axis 6). No
existing game does this fully; partial anchors only (The Sims autonomy;
RimWorld storyteller is world-side, not agent-side). Counter-space:
incoherent LLM-driven games, emergent behavior that reads as noise.

- Sort: pending (axis added after the 2026-09-23 pass)

### 10. Directoredness — the DM

How visibly the world bends to narrative: event and conflict pacing,
authored feel vs. simulated feel, the DM's hand on the global constants.
Mechanical ancestors: RimWorld's AI storyteller (pacing personalities),
Left 4 Dead's AI Director (intensity pacing). Counter-space: scripted
story beats that fight the simulation; procedural generation that
masquerades as meaning.

- Sort: pending (axis added after the 2026-09-23 pass)

## Appendix: the 2026-09-23 sort, as sorted

| Axis | Influences | Counter-influences |
| --- | --- | --- |
| Path fluency | ACS, Dwarf Fortress, FFT, Dungeon Keeper 2 | Songs of Syx |
| Agility | BotW, ACS, BG2, Hollow Knight | SotC, RimWorld, Kenshi |
| Weight | BotW, ACS | Kenshi |
| Gait identity | Kenshi, ACS, BotW | — |
| Idle & reaction life | ACS, Kenshi, Stardew, BG2 | Dwarf Fortress, RimWorld |
| Glance readability | ACS, StarCraft, Kenshi | Dwarf Fortress, Factorio |
| Camera temperament | ACS, Kenshi, BotW | — |
| World pace | ACS, Stardew, BG2, Factorio, DF, Kenshi | Songs of Syx, BotW, Diablo II, They Are Billions, The Sims |

Read of the pattern: ACS is an influence on all eight sorted axes — the
anchor. Kenshi splits into "characterful world, sloppy body control"
(influence on gait/idle/readability/camera/pace, counter on
agility/weight). RimWorld never makes the influence column for characters
(its storyteller resurfaces as a DM ancestor instead). Recurring
counters: Dwarf Fortress (idle, readability) and Songs of Syx (fluency,
pace).
