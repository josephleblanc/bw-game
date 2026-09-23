# Animation sourcing

Companion to [animation-set.md](animation-set.md): that document is *what*
we animate; this one is *where the motion can come from*. Written 2026-09-23
after a survey of traditional and newer techniques, each scored against the
project's laws — determinism under the tick contract, zero steady-state
allocation, the contained dependency footprint (ADR 0002), and a 13-bone
rig with no hands or face.

## Decision (for now)

**Procedural stays the runtime law.** Open motion data and offline
generation are content *sources for baking* (they inform parameters; they
never ship as runtime assets or dependencies). Robotics literature is a
reference for control feel, not a runtime. Physics is reserved for
reactions, behind a feature gate, when the combat tier wants it. Runtime
ML is parked.

## The landscape

### Hand-authoring

- **Procedural/parametric code** (current path). The gait in
  `bw_core::character` is what robotics calls a *central pattern
  generator* — coupled oscillators phase-locked to ground distance — so
  "write it by hand" draws on a deep literature (CPG controllers,
  SIMBICON-style planned walking, biomechanics of gait), not intuition.
  Perfectly deterministic, zero assets, zero runtime dependencies, and
  continuous parameters (the `run_blend` pattern) rather than discrete
  clips. Cost: authoring effort scales with realism demands.
- **DCC keyframing** (Blender → glTF → `bevy_animation`'s
  `AnimationPlayer`/animation graphs, or the third-party
  `bevy_animation_graph` crate). The traditional industry path and the
  natural graduation route once characters carry real meshes; mostly
  irrelevant while the cast is stick figures, since keyframes are replayed
  data the procedural rig would have to consume.

### Openly available motion

- **CMU Motion Capture Database** — ~2,600 motions, free to use without
  licensing restrictions; 1990s optical quality but strong on locomotion
  and everyday action. The classic source.
- **Mixamo (Adobe)** — thousands of free game-ready animations usable in
  games under their terms, but a service rather than open data (account
  required, raw redistribution restricted). Fine as a source of *derived*
  parameters in a public repo.
- **AMASS** — the unified SMPL-fitted archive of most major mocap
  datasets. Superb coverage, **research-only licensing** — reference and
  inspiration, not shippable data. HDM05 and HumanEva are similarly
  non-commercial.
- **Gesture research datasets** — BEAT (~76 h of speech-synchronized
  gesture) and the GENEA challenge data (full-body conversational
  gesture, multiple styles). Gold for the social tier; audio-conditioned
  and research-licensed, so reference only.
- **Capture-your-own** — phone/video mocap (Rokoko Vision, iPi Soft) or a
  used Kinect. Cheap, unencumbered, worth remembering for gestures that
  no library records (much of the xianxia vocabulary).

The architectural move that makes mocap fit our laws: **never ship motion
data — bake it offline into parametric constants.** A BVH clip retargeted
to 13 bones and reduced to per-channel amplitudes and phases is a handful
 of `GaitParams`-style numbers: zero runtime footprint, deterministic,
 and no license question in the artifact. Two uses: parameter mining (a
 CMU walk cycle is ground truth for thigh/knee/bob ratios) and validation
 (procedural output pinned against recorded motion the way tests already
 pin geometric invariants).

### Motion matching (the data-driven bridge)

A database of pose frames searched every tick for the frame best matching
current pose and desired trajectory (Ubisoft's MLDP; Gears of War 4, The
Last of Us Part II). Not ML, not clips: search replaces transition logic.
Compatible with our laws — deterministic over fixed data, zero-alloc with
precomputed feature vectors — and it consumes exactly the retargeted
13-bone clips above. Cost: a real clip corpus, and per-tick search × crowd
 scale needs budgeting (the learned-compression variant — Holden et al.,
 *Learned Motion Matching* — exists to shrink this). Community Bevy
 implementations exist but are young; hand-rolling in `bw_core` is
 realistic. Verdict: an upgrade path to keep open once clip volume
 justifies it, not a current need.

### Machine learning

Split by *where in the pipeline the model runs*:

- **Offline generation (text-to-motion).** Diffusion and language models
  — MDM (the 2022 baseline), MotionGPT, and 2025's scaled
  DiT/flow-matching successors (e.g. Tencent's HY-Motion) — generate
  motion clips from prompts, in SMPL-skeleton form. For us a *content
  tool*, exactly like a mocap session: generate → curate → bake to
  parameters offline; no runtime cost, no determinism question, no
  dependency. Research-grade polish and retargeting needed, but the
  cultivation tier ("cross-legged meditation loop", "ceremonial bow",
  "palm-thrust cast") is promptable where no mocap library exists. The
  speech-driven gesture line (BEAT/GENEA research) maps to the talk/emote
  tier if dialogue ever gets audio.
- **Runtime learned controllers.** The DeepMimic → AMP → PULSE/MaskedMimic
  lineage: a humanoid trained in massively parallel physics simulation to
  imitate mocap, deployed as a network emitting joint targets per tick.
  Impressive and unifying — but against our laws it fails today: neural
  inference per character per tick is a non-starter at mandala-10k scale;
  ONNX drags a C library through ADR 0002's gate (pure-Rust `tract` is
  slower); determinism holds per-binary but not the cheap bit-exact
  discipline our checksums enforce; and photoreal-humanoid motion
  mismatches a stick figure's abstraction. Parked; revisit only if a
  single hero character ever wants emergent physicality.

### Robotics-style physics simulation

A continuum, and our gait already sits on its shallow end (CPG = open-loop
robotics control):

- **Pragmatic middle — animation-driven with physics reactions.**
  Characters animate as today; a passive/active ragdoll takes over
  transiently for hit reactions, staggers, death falls. The
  industry-standard hybrid and a direct fit for the combat tier — death
  and flinch are exactly where hand-authored motion looks worst and
  physics looks best. In-engine via Avian (formerly bevy_xpbd) or
  Rapier, both deterministic under fixed timesteps per binary. The
  dependency is real, but the tree-playground `viewer` feature-gating
  pattern is the proven route: physics behind a flag until it earns a
  default-graph slot.
- **Full physics characters.** Every joint motorized (PD servos tracking
  targets), motion emerging from simulation — Exanima builds a whole
  combat game this way; Gang Beasts/Human Fall Flat are looser active
  ragdolls. Offline, MuJoCo (Apache-2.0, deterministic) is the research
  gold standard and the same toolchain that trains the ML controllers
  above. For thousands of background workers, per-character rigid-body
  simulation spends compute solving problems (ground-contact realism,
  push recovery) our aesthetic does not have. What *is* worth borrowing
  now: PD-servo smoothing ideas for one-shot blend-outs (motor-control
  decay curves read better than linear fades).

## The rig as universal adapter

The 13-bone, no-hands, no-face rig quietly cheapens every source. Mocap
downsamples onto it forgivingly (no finger retargeting; SMPL fits map
loosely and error hides), ML outputs need only coarse fidelity, procedural
authoring tunes few channels. Whatever the source, the destination is the
same flat arrays of local quaternions driven by phase — the animation-set
vocabulary stays source-agnostic, and different tiers can use different
sources without any runtime knowing.

## Summary

| Technique | Role for us |
|---|---|
| Procedural (current) | The runtime law — everything lands as deterministic parameters |
| Open mocap + baking | Parameter mining and validation now; clip source later |
| Text-to-motion (offline) | Reference and first drafts for the cultivation/gesture tiers |
| Motion matching | Parked upgrade for locomotion once clip volume justifies it |
| Physics reactions (ragdoll) | Combat tier's hit/death layer, feature-gated like the viewer |
| Runtime ML controllers | Parked; revisit only for a single hero character |
| Full robotics sim | Research reference (CPG/PD literature), not a runtime |
