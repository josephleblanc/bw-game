# Character animation set

The target animation vocabulary for the cultivation colony sim (the
"Amazing Cultivation Simulator" direction: colony management with xianxia
fantasy elements). Written 2026-09-23 as the reference the `action-states`
backlog work and its successors build against. It is grounded in the rig
that exists — `bw_core::character`'s 13-bone stick figure with its
speed-blended gait — not a wishlist for a future skeleton; where an entry
needs a mechanism the rig does not have yet, it says so.

## Design principle: primitives, not a clip zoo

A colony sim needs surprisingly few *skeleton* motions. What multiplies is
tools, props, and FX riding on top: woodcutting, mining, construction, and
forge hammering are one overhead swing at different timings; dozens of
spells are one palm-thrust with different particles. So the catalog below
names, for every entry, the primitive it rides — and the primitives are
designed as continuous parameter blends, following the gait's own pattern
(walk→run is `run_blend` on speed, not a state to switch).

## Mechanisms

Four mechanisms, in dependency order; 1–2 landed with `action-states`
(2026-09-23), 3–4 are still beyond the code. Everything in the catalog
is some combination of these plus the gait that exists.

1. **Upper-body additive layer** *(landed)* — partial-bone pose
   overrides (arms, torso, head) blended onto the locomotion channels,
   so a figure can carry, channel, gesture, or read while walking.
   Punching is the first consumer; carry is its first locomotion-safe
   one (the layer must compose with a moving root, not just a standing
   one).
2. **One-shot actions with recovery** *(landed)* — an action phase
   that plays, blends out, and returns control. Punch is the first;
   bow, breakthrough, and death follow the same shape.
3. **Posture channel** — the real new surface: `pose_into` hard-codes
   `HIP_HEIGHT`, so seated/lying/kneeling postures need a root-height and
   pose-set state, plus down/up transitions. The main mechanism the
   cultivation vocabulary depends on (meditation is the signature pose of
   the genre).
4. **Gait asymmetry** — left/right amplitude split inside `GaitParams`
   (limp). Nearly free; large colony-readability payoff for health
   states.

## The catalog

Status markers: **have** (exists today), **layer** (mechanism 1),
**one-shot** (mechanism 2), **posture** (mechanism 3), **gait-param**
(a parameter of the existing gait, not a new motion).

### A. Locomotion & carry

- **Walk / run** — have; one continuous speed axis already.
- **Idle stand** — have (breathing). Variants as pose offsets, not new
  motions: arms clasped behind back (elder/scholar read), combat-ready.
- **Carry two-handed** (crate, jar at chest) — layer; upper-body hold
  with stride shortening under load.
- **Carry one-handed** (bucket, tool at side) — layer; lighter override.
- **Panic run** — gait-param (higher lean/arm amplitude on the run end).
- **Limp** — gait-param via mechanism 4.
- Sneak — deliberately absent until stealth mechanics exist.

### B. Postures & transitions (the enabler tier)

- **Bend-and-reach** — torso pitch plus arm extend. The colony-sim
  primitive: pickup, place, harvest, scraping. Highest reuse per motion
  in the catalog; needs no root-height change (a lean, not a posture).
- **Kneel / crouch** — posture; ground work (sowing, low harvest,
  worship).
- **Sit cross-legged** — posture; the meditation pose, with sit-down and
  stand-up transitions (one-shots).
- **Sit on stool/chair** — posture variant; eating, desk work.
- **Lie down** — posture; sleep, injury, death, with get-down/get-up
  transitions.
- **Flinch / stagger** — one-shot; bridges into combat.

### C. Work loops

- **Two-handed overhead swing** — layer + one-shot strokes; woodcutting,
  mining, construction, forge hammering. One primitive, different
  timing/amplitude/tool.
- **One-handed low swing** — layer; hoeing, sweeping, stirring a
  cauldron, shoveling.
- **Pluck** — bend-and-reach instance (harvest).
- **Sow** — kneel + place.
- **Pickup / set-down** — bend-and-reach instances (hauling).
- **Eat / drink** — layer; hand-to-mouth loop and cup-raise, seated or
  standing.
- **Read** — layer; scroll or book held with occasional page/roll
  gesture, standing and seated.
- **Draw talisman** — posture (seated at desk) + small brush-stroke
  layer.
- **Tend station** — layer; generic hands-at-a-bench loop that props and
  FX differentiate (alchemy prep, assembly).

### D. Cultivation & magic (the genre differentiator)

- **Seated meditation** — posture + breathing loop with a *depth*
  parameter scaling breath and sway amplitude (early Qi Refining vs.
  deep trance). Qi VFX rides the same phase channel, keeping everything
  reproducible under the tick contract.
- **Standing meditation** (zhan zhuang) — layer; arms rounded, held
  posture, micro-sway.
- **Qi gathering** — layer; standing, arms sweep up and draw in, looped
  (spirit springs, pre-meditation).
- **Cast one-handed** — one-shot; palm push / finger-point thrust. The
  generic spell release; FX differentiate spells.
- **Cast two-handed channel** — layer; sustained both-arms-forward for
  formations, rituals, big spells.
- **Breakthrough** — one-shot sequence: strain → surge → release →
  settle back into meditation. The realm-up moment; polish priority
  disproportionate to its length.
- **Sword form / kata** — layer sequence; looping practice that doubles
  as the melee combo source.
- **Flying-sword riding** — layer on a moving root attachment;
  balance-lean, banking into turns. Hero-tier, late, but iconic enough
  to keep the root-attachment story open for.
- **Instruct / demonstrate** — layer; master's teaching gesture, paired
  with a student nod-bow.

### E. Social & mood

- **Talk A / talk B** — layer; two facing gesture loops that alternate
  (chat, teaching). The head bone gives cheap nods.
- **Bow** — one-shot; greeting/respect, core xianxia etiquette. Deep
  ceremonial bow is an amplified variant.
- **Trade hand-off** — extends place (bend-and-reach).
- **Emotes** — layer; laugh, cry, shake head, fist-shake. Small loops,
  large mood-readability payoff.
- **Collapse** — one-shot into the lie-down posture (hunger/exhaustion).
- Drunken stagger — limp variant with different asymmetry; flavor, low
  priority.

### F. Combat

- **Combat idle** — pose offset (guard stance).
- **Punch** — one-shot; already planned as the first additive channel.
- **Kick** — one-shot; shares punch machinery.
- **One-handed slash** — one-shot; sword/saber, also serves work axes.
- **Two-handed chop** — the woodcutting primitive with intent change and
  different follow-through.
- **Thrust** — one-shot; spear/sword point.
- **Block / brace** — layer.
- **Death fall** — one-shot ending in the lie-down posture (which then
  persists).
- Celebrate — victory emote; low priority.

## Absent by design

- **Hands / fingers** — gestures read fine at forearm scale on a stick
  figure; tools and weapons attach at the lower-arm tips.
- **Facial animation** — the head is a ball; mood reads through body
  language, which suits the aesthetic.
- **A second torso bone** — deferred until seated meditation proves the
  single-bone torso reads stiff; add then, not before (ADR 0005's
  stack-default pose pass stays O(bones) either way).

## Phasing

Build order in reuse-per-effort terms, sliced for the gallery cadence.
Each tier lands as a gallery extension with tests pinning its contract,
the way the walk is pinned today.

1. **Action layer foundation** — landed 2026-09-23 (`action-states`,
   done): jump + punch established mechanisms 1 and 2, plus the
   vertical channel the jump owns (ballistic arc, stride freeze
   mid-air, landing absorption).
2. **Carry + bend-and-reach** — landed 2026-09-23 (`carry-reach`,
   done): chest/side holds with eased blends and stride shortening,
   and the bend/hold/rise one-shot (a lean — the posture tier still
   owns root-height changes; ground-level pickup deepens then).
3. **Work tier** — two-handed swing, one-handed swing, tend-station
   loop.
4. **Posture tier** — mechanism 3, then meditation, sleep, death.
5. **Magic + social tier** — casts, qi gathering, talk, bow.
6. **Combat tier** — mostly recombination of swings and one-shots.
7. **Hero tier** — breakthrough set-piece, flying sword.
