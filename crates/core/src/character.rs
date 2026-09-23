//! The walking stick figure: a fixed humanoid rig and a procedural gait,
//! engine-agnostic per ADR 0002 and allocate-once per ADR 0004 — the
//! skeleton is flat arrays built once, `pose_into` writes a reused pose
//! buffer, and `Character::step` mutates plain fields. No wall clock:
//! callers pass dt (the galleries pass [`crate::time::SIM_DT`]), so the
//! same `(state, ticks)` always reproduces the same checksum, the same
//! way `sim` and `tree` pin theirs.
//!
//! ## Model
//!
//! The rig is a stick figure of 13 bones (hips, torso, head, two arms,
//! two legs with feet) as flat arrays with parents always preceding
//! children, so forward kinematics is one forward loop: a bone's joint
//! sits at `joint[parent] + rot[parent] · from[bone]` and its rotation
//! composes `rot[bone] = rot[parent] · q_anim[bone]`. Rest pose is a
//! standing figure (~1.86 m); animation is per-bone local quaternions
//! driven by the gait phase, expressed in the character's root frame
//! (+x right, +y up, +z forward, heading yaw about +y). The pose pass
//! keeps per-bone world rotations on the stack (ADR 0005).
//!
//! ## Gait
//!
//! A single axis drives everything: `speed`. It blends idle → walk →
//! run continuously (`smoothstep` on speed), so running is not a state
//! to switch, just a faster target. Phase advances as `2π·speed/stride`
//! — cycles lock to ground distance, which keeps foot contact honest
//! (no sliding at any speed). The cycle: thighs swing sinusoidally,
//! knees flex through a half-rectified bump during swing, the pelvis
//! yaws while the torso counter-yaws (the head counter-counter-yaws to
//! stay level), arms counter-swing the same-side leg with flexing
//! elbows, and the root bobs twice per cycle (vault over the stance
//! leg) with a lateral weight shift.
//!
//! ## Actions
//!
//! Two action channels join the gait (tier 1 of
//! `docs/animation/animation-set.md`): a vertical **jump** (ballistic
//! root, airborne tuck, landing absorption on the knee channel — the
//! stride pauses while airborne, since cycles lock to ground contact)
//! and a one-shot **punch** (windup/strike/recover envelopes additive
//! on the right arm, blending to zero at both ends so the gait swing
//! resumes untouched). Both are plain fields on `Character`, driven
//! by request bits in `MovementInput`, so the same input sequence
//! still reproduces the same checksum.
//!
//! Amplitude conventions: positive thigh/arm pitch swings a hanging
//! limb *backward* (right-handed rot_x on a downward rest direction);
//! positive knee flexion bends the knee the anatomical way (foot
//! trailing); elbows flex forward, hence their negative sign.

use std::f32::consts::TAU;

use crate::math::{Quat, Vec3};

// ---------------------------------------------------------------------------
// Skeleton
// ---------------------------------------------------------------------------

/// Number of bones in the humanoid rig.
pub const BONE_COUNT: usize = 13;

/// Bone indices into the skeleton arrays (also the pose arrays).
pub mod bone {
    pub const HIPS: usize = 0;
    pub const TORSO: usize = 1;
    pub const HEAD: usize = 2;
    pub const UPPER_ARM_L: usize = 3;
    pub const LOWER_ARM_L: usize = 4;
    pub const UPPER_ARM_R: usize = 5;
    pub const LOWER_ARM_R: usize = 6;
    pub const UPPER_LEG_L: usize = 7;
    pub const LOWER_LEG_L: usize = 8;
    pub const FOOT_L: usize = 9;
    pub const UPPER_LEG_R: usize = 10;
    pub const LOWER_LEG_R: usize = 11;
    pub const FOOT_R: usize = 12;
}

/// Bone names, index-aligned (readouts and debugging).
pub const BONE_NAMES: [&str; BONE_COUNT] = [
    "hips",
    "torso",
    "head",
    "upper-arm-l",
    "lower-arm-l",
    "upper-arm-r",
    "lower-arm-r",
    "upper-leg-l",
    "lower-leg-l",
    "foot-l",
    "upper-leg-r",
    "lower-leg-r",
    "foot-r",
];

/// Head sphere radius at scale 1 (the head is drawn as a ball at the
/// HEAD bone's tip, not as a stick).
pub const HEAD_RADIUS: f32 = 0.105;

/// Hip joint height at scale 1 (the rest pose's ground contact).
pub const HIP_HEIGHT: f32 = 0.90;

const PARENT_NONE: i8 = -1;

/// The humanoid rig: per bone, the offset from the parent's joint to
/// this bone's joint (in the parent's frame), the bone vector itself
/// (own frame), and the render width. Parents always precede children,
/// so the pose pass is a single forward loop.
#[derive(Debug, Clone)]
pub struct Skeleton {
    parent: Vec<i8>,
    from: Vec<Vec3>,
    dir: Vec<Vec3>,
    width: Vec<f32>,
}

impl Skeleton {
    /// The stick figure at height scale 1 (~1.86 m at rest).
    pub fn humanoid() -> Self {
        use bone::*;
        let v = Vec3::new;
        // (parent, from, dir, width) — meters; rest pose: standing,
        // arms hanging, feet on the ground.
        let rows: [(usize, Vec3, Vec3, f32); BONE_COUNT] = [
            (usize::MAX, v(0.0, 0.0, 0.0), v(0.0, 0.10, 0.0), 0.050), // HIPS (root)
            (HIPS, v(0.0, 0.10, 0.0), v(0.0, 0.50, 0.0), 0.055),      // TORSO
            (TORSO, v(0.0, 0.59, 0.0), v(0.0, 0.16, 0.0), 0.040),     // HEAD
            (TORSO, v(0.19, 0.48, 0.0), v(0.0, -0.30, 0.0), 0.038),   // UPPER_ARM_L
            (UPPER_ARM_L, v(0.0, -0.30, 0.0), v(0.0, -0.27, 0.0), 0.034), // LOWER_ARM_L
            (TORSO, v(-0.19, 0.48, 0.0), v(0.0, -0.30, 0.0), 0.038),  // UPPER_ARM_R
            (UPPER_ARM_R, v(0.0, -0.30, 0.0), v(0.0, -0.27, 0.0), 0.034), // LOWER_ARM_R
            (HIPS, v(0.10, 0.0, 0.0), v(0.0, -0.46, 0.0), 0.048),     // UPPER_LEG_L
            (UPPER_LEG_L, v(0.0, -0.46, 0.0), v(0.0, -0.40, 0.0), 0.042), // LOWER_LEG_L
            (LOWER_LEG_L, v(0.0, -0.40, 0.0), v(0.0, -0.035, 0.16), 0.038), // FOOT_L
            (HIPS, v(-0.10, 0.0, 0.0), v(0.0, -0.46, 0.0), 0.048),    // UPPER_LEG_R
            (UPPER_LEG_R, v(0.0, -0.46, 0.0), v(0.0, -0.40, 0.0), 0.042), // LOWER_LEG_R
            (LOWER_LEG_R, v(0.0, -0.40, 0.0), v(0.0, -0.035, 0.16), 0.038), // FOOT_R
        ];
        let mut skeleton = Self {
            parent: Vec::with_capacity(BONE_COUNT),
            from: Vec::with_capacity(BONE_COUNT),
            dir: Vec::with_capacity(BONE_COUNT),
            width: Vec::with_capacity(BONE_COUNT),
        };
        for (parent, from, dir, width) in rows {
            assert!(
                parent == usize::MAX || parent <= skeleton.parent.len(),
                "parents must precede children"
            );
            skeleton.parent.push(if parent == usize::MAX {
                PARENT_NONE
            } else {
                parent as i8
            });
            skeleton.from.push(from);
            skeleton.dir.push(dir);
            skeleton.width.push(width);
        }
        skeleton
    }

    pub fn bone_count(&self) -> usize {
        self.parent.len()
    }

    pub fn parent(&self, b: usize) -> Option<usize> {
        let p = self.parent[b];
        (p != PARENT_NONE).then_some(p as usize)
    }

    /// Rest render width of bone `b` (meters at scale 1).
    pub fn width(&self, b: usize) -> f32 {
        self.width[b]
    }

    /// Rest bone length `b` (meters at scale 1) — the joint-to-tip
    /// distance renderers build their geometry around.
    pub fn length(&self, b: usize) -> f32 {
        self.dir[b].length()
    }
}

// ---------------------------------------------------------------------------
// Gait parameters
// ---------------------------------------------------------------------------

/// Locomotion amplitudes for one gait (walk or run); the effective gait
/// is a per-field lerp of the two presets, driven by speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GaitParams {
    /// Ground distance per full gait cycle (both feet). Sets phase rate:
    /// `2π · speed / stride` — cycles lock to ground travel.
    pub stride_len: f32,
    /// Max thigh pitch from vertical (rad).
    pub thigh: f32,
    /// Swing-phase knee flexion (rad).
    pub knee: f32,
    /// Knee flexion retained through stance (rad) — run absorbs load.
    pub stance_knee: f32,
    /// Ankle push-off / swing-clearance amplitude (rad).
    pub foot: f32,
    /// Upper-arm swing amplitude (rad).
    pub arm: f32,
    /// Base elbow flexion (rad) — arms pump hard at run.
    pub elbow: f32,
    /// Root vertical oscillation (m), twice per cycle.
    pub bob: f32,
    /// Lateral weight shift (m), once per cycle.
    pub lat: f32,
    /// Forward torso lean (rad).
    pub lean: f32,
    /// Pelvis yaw oscillation (rad).
    pub hip_sway: f32,
    /// Torso counter-yaw (rad).
    pub torso_sway: f32,
}

/// The walk preset (~1.35 m/s at the gallery's cruise speed).
pub const WALK: GaitParams = GaitParams {
    stride_len: 1.35,
    thigh: 0.42,
    knee: 0.95,
    stance_knee: 0.06,
    foot: 0.22,
    arm: 0.32,
    elbow: 0.28,
    bob: 0.016,
    lat: 0.018,
    lean: 0.07,
    hip_sway: 0.10,
    torso_sway: 0.12,
};

/// The run preset (~3.6 m/s): longer stride, deeper knee, pumped arms,
/// more lean and bob.
pub const RUN: GaitParams = GaitParams {
    stride_len: 2.75,
    thigh: 0.68,
    knee: 1.25,
    stance_knee: 0.22,
    foot: 0.35,
    arm: 0.72,
    elbow: 1.05,
    bob: 0.045,
    lat: 0.012,
    lean: 0.24,
    hip_sway: 0.16,
    torso_sway: 0.20,
};

impl GaitParams {
    /// Per-field lerp `self·(1−t) + other·t` — endpoint-exact at 0 and 1.
    pub fn lerp(self, other: Self, t: f32) -> Self {
        let f = |a: f32, b: f32| a * (1.0 - t) + b * t;
        Self {
            stride_len: f(self.stride_len, other.stride_len),
            thigh: f(self.thigh, other.thigh),
            knee: f(self.knee, other.knee),
            stance_knee: f(self.stance_knee, other.stance_knee),
            foot: f(self.foot, other.foot),
            arm: f(self.arm, other.arm),
            elbow: f(self.elbow, other.elbow),
            bob: f(self.bob, other.bob),
            lat: f(self.lat, other.lat),
            lean: f(self.lean, other.lean),
            hip_sway: f(self.hip_sway, other.hip_sway),
            torso_sway: f(self.torso_sway, other.torso_sway),
        }
    }
}

/// Speeds where the blends sit (m/s): below `MOVE_LO` the figure idles,
/// `MOVE_HI` is fully walking; `RUN_LO..RUN_HI` blends walk→run.
pub const MOVE_LO: f32 = 0.15;
pub const MOVE_HI: f32 = 0.9;
pub const RUN_LO: f32 = 2.1;
pub const RUN_HI: f32 = 3.1;

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Locomotion blend 0..1 (idle → moving) for `speed`.
pub fn move_blend(speed: f32) -> f32 {
    smoothstep(MOVE_LO, MOVE_HI, speed)
}

/// Run blend 0..1 (walk → run) for `speed`.
pub fn run_blend(speed: f32) -> f32 {
    smoothstep(RUN_LO, RUN_HI, speed)
}

/// The effective gait at `speed` (walk preset lerped toward run).
pub fn gait_at(speed: f32) -> GaitParams {
    WALK.lerp(RUN, run_blend(speed))
}

// ---------------------------------------------------------------------------
// Action parameters (jump + punch, the first action-layer channels)
// ---------------------------------------------------------------------------

/// Gravity on the vertical channel (m/s²).
pub const GRAVITY: f32 = 9.81;
/// Jump launch speed (m/s): apex ≈ `JUMP_V0²/(2·GRAVITY)` ≈ 0.28 m,
/// air time ≈ `2·JUMP_V0/GRAVITY` ≈ 0.48 s.
pub const JUMP_V0: f32 = 2.35;
/// Landing absorption window (s): the root dip and knee take-up ease
/// to zero over this span after touchdown.
pub const LAND_WIN: f32 = 0.30;
/// Root dip at touchdown (m) — matched to the leg shortening of
/// `LAND_KNEE` (lower leg 0.40 m: `0.40·(1−cos LAND_KNEE)`) so the
/// feet stay planted while the knees absorb.
pub const LAND_DIP: f32 = 0.024;
/// Knee take-up at touchdown (rad), both legs.
pub const LAND_KNEE: f32 = 0.35;
/// Airborne tuck amplitudes (rad): thighs swing forward (negative
/// pitch, knees up), knees flex, feet point.
const TUCK_THIGH: f32 = -0.45;
const TUCK_KNEE: f32 = 0.85;
const TUCK_FOOT: f32 = -0.30;
/// Airborne balance: outward arm spread (rad) added on each side's
/// outside (left toward +x, right toward −x) through the flight.
const TUCK_SPLAY: f32 = 0.40;

/// Punch duration (s) — one windup/strike/recover cycle.
pub const PUNCH_DUR: f32 = 0.45;

/// Landing absorption envelope: 1 at touchdown easing quadratically
/// to 0 over [`LAND_WIN`] seconds (a decay that reads as muscular,
/// not spring-loaded).
pub fn land_bump(t: f32) -> f32 {
    if t >= LAND_WIN {
        0.0
    } else {
        let k = 1.0 - t / LAND_WIN;
        k * k
    }
}

// ---------------------------------------------------------------------------
// Character state and stepping
// ---------------------------------------------------------------------------

/// Locomotion intent for one character, one tick. Controllers (keyboard,
/// scripted paths) produce this; [`Character::step`] consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MovementInput {
    /// Desired forward speed (m/s, ≥ 0); the character approaches it.
    pub target_speed: f32,
    /// Turn rate about +y (rad/s), applied directly.
    pub turn_rate: f32,
    /// Jump request, honored only while grounded. Level-triggered: a
    /// held request re-launches on every landing (bouncing); the
    /// airborne state itself guards against double impulses.
    pub jump: bool,
    /// Punch request: starts the one-shot when the arm channel is
    /// idle; requests during the action are ignored (no queueing).
    /// Held, it chains cycles back-to-back.
    pub punch: bool,
}

/// Deterministic circle-path input: walk a circle of `radius` at `speed`
/// (positive `dir` = counterclockwise seen from above). Shared by the
/// scripted gallery scenes and the viewer's ambient walkers so both
/// locomote identically.
pub fn circle_input(radius: f32, speed: f32, dir: f32) -> MovementInput {
    debug_assert!(radius > 0.0, "circle radius must be positive");
    MovementInput {
        target_speed: speed,
        turn_rate: dir * speed / radius.max(0.1),
        jump: false,
        punch: false,
    }
}

/// Walk acceleration limits (m/s²): braking is snappier than launching.
const ACCEL: f32 = 5.0;
const DECEL: f32 = 9.0;

/// One character: root state plus gait phase. All fields are plain
/// values — `step` is a pure state transition given the same input
/// sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct Character {
    /// Root position on the ground plane (world; the pelvis sits above).
    pub pos: Vec3,
    /// Heading yaw about +y (rad); +z at 0.
    pub heading: f32,
    /// Current forward speed (m/s).
    pub speed: f32,
    /// Gait phase (rad, unbounded; the left leg's cycle). `2π` apart =
    /// the same pose; kept unbounded so footfall counting needs no wrap
    /// handling.
    pub phase: f32,
    /// Seconds since spawn (idle channels: breathing).
    pub t: f32,
    /// Heel strikes landed so far (the galleries' `interactions` count).
    pub footfalls: u64,
    /// Height scale (crowd variation).
    pub scale: f32,
    /// Root height above the ground plane (m); `pos` stays on the
    /// plane and this lifts the root through the jump arc. 0 =
    /// standing.
    pub y: f32,
    /// Vertical velocity (m/s), positive up.
    pub vy: f32,
    /// Seconds since the last touchdown (drives landing absorption);
    /// saturates at [`LAND_WIN`] so a freshly spawned figure stands.
    pub land_t: f32,
    /// Punch phase in `0..1` (one [`PUNCH_DUR`] cycle); `None` = the
    /// arm channel is idle and the gait swing owns it.
    pub punch: Option<f32>,
}

impl Character {
    pub fn new(pos: Vec3, heading: f32) -> Self {
        Self {
            pos,
            heading,
            speed: 0.0,
            phase: 0.0,
            t: 0.0,
            footfalls: 0,
            scale: 1.0,
            y: 0.0,
            vy: 0.0,
            land_t: LAND_WIN,
            punch: None,
        }
    }

    /// Ground contact: on the floor and not mid-launch. The stride
    /// only advances (and jumps only launch) while grounded.
    pub fn grounded(&self) -> bool {
        self.y <= 0.0 && self.vy <= 0.0
    }

    /// Unit forward vector for `heading` (+z at 0, yaw about +y).
    pub fn forward(heading: f32) -> Vec3 {
        Vec3::new(heading.sin(), 0.0, heading.cos())
    }

    /// Advance exactly one `dt` under `input`: integrate heading and
    /// position, ease speed toward its target, fly the vertical
    /// channel (jump requests launch only while grounded), advance
    /// the gait phase by ground distance while in contact, count heel
    /// strikes (one per half cycle), and progress the punch one-shot.
    pub fn step(&mut self, dt: f32, input: &MovementInput) {
        self.t += dt;
        self.heading += input.turn_rate * dt;
        let rate = if input.target_speed >= self.speed {
            ACCEL
        } else {
            DECEL
        };
        let max_delta = rate * dt;
        self.speed += (input.target_speed - self.speed).clamp(-max_delta, max_delta);
        self.speed = self.speed.max(0.0);
        self.pos = self.pos + Self::forward(self.heading) * (self.speed * dt);

        // Vertical channel: a grounded request launches ballistics;
        // the arc is pure integration, and the first tick that falls
        // to y ≤ 0 is touchdown — it arms the landing absorption.
        if input.jump && self.grounded() {
            self.vy = JUMP_V0;
        }
        let was_air = self.y > 0.0 || self.vy > 0.0;
        self.y += self.vy * dt;
        if self.y > 0.0 {
            self.vy -= GRAVITY * dt;
        } else {
            self.y = 0.0;
            if self.vy < 0.0 {
                self.vy = 0.0;
            }
            self.land_t = if was_air {
                0.0
            } else {
                (self.land_t + dt).min(LAND_WIN)
            };
        }

        // Stride locks to ground travel: no contact, no stepping and
        // no heel strikes — the gait freezes mid-stride through the
        // air and resumes on touchdown.
        if self.grounded() {
            let stride = gait_at(self.speed).stride_len.max(0.1);
            let before = self.phase;
            self.phase += TAU * self.speed / stride * dt;
            let strikes =
                (self.phase / std::f32::consts::PI) as i64 - (before / std::f32::consts::PI) as i64;
            self.footfalls += strikes.max(0) as u64;
        }

        // Punch: one request runs one windup/strike/recover cycle;
        // requests during the action fall on deaf ears (no queueing,
        // no restart) and the cycle always runs to completion.
        self.punch = match self.punch {
            None if input.punch => Some(0.0),
            Some(p) => {
                let next = p + dt / PUNCH_DUR;
                (next < 1.0).then_some(next)
            }
            None => None,
        };
    }

    /// Joint starts and tips in world space at the current state, into
    /// the reused `pose` buffer. Pure function of `(self, skeleton)`;
    /// allocates nothing.
    pub fn pose_into(&self, skeleton: &Skeleton, pose: &mut CharacterPose) {
        assert_eq!(
            pose.start.len(),
            skeleton.bone_count(),
            "pose buffer sized to another skeleton"
        );
        let gait = gait_at(self.speed);
        let b_w = move_blend(self.speed);
        let phi = self.phase;
        let sin_phi = phi.sin();

        // Root: heading yaw with pelvis sway; the root offset carries
        // the lateral weight shift (once per cycle) and the bob (twice
        // per cycle — vault over the stance leg), both scaled like the
        // body, riding on top of the hip height.
        let root_rot = Quat::from_rotation_y(self.heading);
        let hip_yaw = -gait.hip_sway * b_w * sin_phi; // swinging leg's hip forward
        let offset = Vec3::new(
            gait.lat * b_w * sin_phi,
            gait.bob * b_w * (2.0 * phi).cos(),
            0.0,
        ) * self.scale;
        // Action envelopes: the airborne blend and landing absorption
        // move the root off standing height; both, like the punch
        // envelopes below, ease to zero at their boundaries so
        // actions start and end pose-continuous with the gait.
        let air = smoothstep(0.0, 0.06, self.y);
        let land = land_bump(self.land_t);
        let (p_wind, p_ext) = match self.punch {
            None => (0.0, 0.0),
            Some(p) => (
                smoothstep(0.0, 0.30, p) * (1.0 - smoothstep(0.30, 0.55, p)),
                smoothstep(0.30, 0.45, p) * (1.0 - smoothstep(0.60, 0.95, p)),
            ),
        };
        let p_act = p_wind.max(p_ext);
        let root_pos = self.pos
            + root_rot.rotate(offset)
            + Vec3::new(
                0.0,
                (HIP_HEIGHT + self.y - LAND_DIP * land) * self.scale,
                0.0,
            );

        let breath = 0.02 * (1.0 - b_w) * (self.t * 1.7).sin();
        let lean = gait.lean * b_w;
        let elbow_base = gait.elbow * b_w + 0.05;

        // Per-bone animated local rotations. L/R channels sit π apart;
        // half-rectified gates confine knee flexion and ankle push-off
        // to their phases of the cycle.
        let swing_gate = |ph: f32| (ph - 0.35).sin().max(0.0); // swing phase
        let push_gate = |ph: f32| (ph - 2.6).sin().max(0.0); // late stance

        let mut q_anim = [Quat::IDENTITY; BONE_COUNT];
        q_anim[bone::HIPS] = Quat::from_rotation_y(hip_yaw);
        // The punch rides the torso too: windup coils the right
        // shoulder back, the strike throws it forward with a lean.
        q_anim[bone::TORSO] =
            Quat::from_rotation_y(gait.torso_sway * b_w * sin_phi + 0.50 * p_ext - 0.20 * p_wind)
                * Quat::from_rotation_x(lean + breath + 0.12 * p_ext);
        q_anim[bone::HEAD] = Quat::from_rotation_y(-gait.torso_sway * 0.6 * b_w * sin_phi)
            * Quat::from_rotation_x(-lean * 0.6);

        for is_left in [true, false] {
            let ph = phi + if is_left { 0.0 } else { std::f32::consts::PI };
            let s = ph.sin();
            // Gait channels, plus the airborne tuck (knees up, feet
            // pointed) and the landing take-up on the knees.
            let thigh_pitch = -gait.thigh * b_w * s + air * TUCK_THIGH;
            let knee_flex = gait.stance_knee * b_w
                + gait.knee * b_w * swing_gate(ph)
                + air * TUCK_KNEE
                + land * LAND_KNEE;
            let ankle = gait.foot * b_w * (0.4 * swing_gate(ph) - push_gate(ph)) + air * TUCK_FOOT;
            let mut arm_pitch = gait.arm * b_w * s; // same-side arm counters leg
            let mut elbow_flex = elbow_base + 0.35 * gait.arm * b_w * (-s).max(0.0) + air * 0.25;
            if is_left {
                // Left arm guards through the strike: raised, curled.
                arm_pitch += -0.35 * p_ext;
                elbow_flex += 1.10 * p_ext;
            } else {
                // Right arm carries the punch: the gait swing fades
                // with activation, windup coils (arm back, elbow
                // curled), strike extends (arm forward-horizontal,
                // elbow straight).
                arm_pitch = arm_pitch * (1.0 - p_act) + 0.55 * p_wind - 1.60 * p_ext;
                elbow_flex = elbow_flex * (1.0 - p_act) + 1.55 * p_wind;
            }
            // Rest splay keeps the arms off the torso line; through
            // the flight the arms additionally spread outward for
            // balance (left toward +x, right toward −x).
            let side = if is_left { 1.0 } else { -1.0 };
            let splay = (if is_left { -0.08 } else { 0.08 }) + air * TUCK_SPLAY * side;
            let (ua, la, ul, ll, ft) = if is_left {
                (
                    bone::UPPER_ARM_L,
                    bone::LOWER_ARM_L,
                    bone::UPPER_LEG_L,
                    bone::LOWER_LEG_L,
                    bone::FOOT_L,
                )
            } else {
                (
                    bone::UPPER_ARM_R,
                    bone::LOWER_ARM_R,
                    bone::UPPER_LEG_R,
                    bone::LOWER_LEG_R,
                    bone::FOOT_R,
                )
            };
            q_anim[ua] = Quat::from_rotation_x(arm_pitch)
                * Quat::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), splay);
            q_anim[la] = Quat::from_rotation_x(-elbow_flex); // flexes forward
            q_anim[ul] = Quat::from_rotation_x(thigh_pitch);
            q_anim[ll] = Quat::from_rotation_x(knee_flex);
            q_anim[ft] = Quat::from_rotation_x(ankle);
        }

        // Forward pass: joint positions and world rotations, parents
        // before children (generation order guarantees it).
        let mut world_rot = [Quat::IDENTITY; BONE_COUNT];
        for b in 0..skeleton.bone_count() {
            let (joint, parent_rot) = match skeleton.parent(b) {
                None => (root_pos, root_rot),
                Some(p) => {
                    let from = world_rot[p].rotate(skeleton.from[b] * self.scale);
                    (pose.start[p] + from, world_rot[p])
                }
            };
            world_rot[b] = parent_rot * q_anim[b];
            pose.start[b] = joint;
            pose.tip[b] = joint + world_rot[b].rotate(skeleton.dir[b] * self.scale);
        }
    }
}

/// Reused pose buffer for one character (ADR 0004): allocated once per
/// character at setup, rewritten every tick. Renderers orient geometry
/// from the joint positions; world rotations stay stack-local to the
/// pose pass.
#[derive(Debug, Clone)]
pub struct CharacterPose {
    pub start: Vec<Vec3>,
    pub tip: Vec<Vec3>,
}

impl CharacterPose {
    pub fn new(skeleton: &Skeleton) -> Self {
        let n = skeleton.bone_count();
        Self {
            start: vec![Vec3::ZERO; n],
            tip: vec![Vec3::ZERO; n],
        }
    }

    pub fn len(&self) -> usize {
        self.start.len()
    }

    pub fn is_empty(&self) -> bool {
        self.start.is_empty()
    }

    /// Order-independent digest of the pose (the gallery state checksum).
    pub fn checksum(&self) -> u64 {
        let mut h = 0xCBF2_9CE4_8422_2325u64;
        let mix = |h: u64, v: Vec3| {
            let [x, y, z] = v.to_bits();
            h.wrapping_mul(0x1000_0000_01B3)
                ^ (x as u64)
                ^ ((y as u64) << 32)
                ^ ((z as u64).rotate_left(17))
        };
        for b in 0..self.start.len() {
            h = mix(h, self.start[b]);
            h = mix(h, self.tip[b]);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::SIM_DT;

    fn walker(speed: f32) -> Character {
        let mut c = Character::new(Vec3::ZERO, 0.0);
        c.speed = speed;
        c
    }

    #[test]
    fn skeleton_parents_precede_children() {
        let skel = Skeleton::humanoid();
        assert_eq!(skel.bone_count(), BONE_COUNT);
        for (b, name) in (0..skel.bone_count()).zip(&BONE_NAMES) {
            if let Some(p) = skel.parent(b) {
                assert!(p < b, "{name} has parent {p} after it");
            }
        }
    }

    /// Rest pose: standing upright, feet essentially on the ground,
    /// arms hanging. Pins the rig's dimensions against tuning drift.
    #[test]
    fn rest_pose_stands_with_feet_on_ground() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        Character::new(Vec3::ZERO, 0.0).pose_into(&skel, &mut pose);
        for (b, name) in [bone::TORSO, bone::HEAD].iter().zip(&BONE_NAMES) {
            let b = *b;
            // Vertical = no lateral displacement; the tip sits exactly
            // one bone length above the joint.
            assert!(
                (pose.tip[b].x - pose.start[b].x).abs() < 1e-5
                    && (pose.tip[b].z - pose.start[b].z).abs() < 1e-5,
                "{name} must rest vertical: dx {} dz {}",
                pose.tip[b].x - pose.start[b].x,
                pose.tip[b].z - pose.start[b].z
            );
        }
        // Head crown ≈ 1.75 + radius.
        assert!((pose.tip[bone::HEAD].y - 1.75).abs() < 1e-4);
        // Arms hang: wrists below elbows, near hip height.
        for (ua, la) in [
            (bone::UPPER_ARM_L, bone::LOWER_ARM_L),
            (bone::UPPER_ARM_R, bone::LOWER_ARM_R),
        ] {
            assert!(pose.tip[ua].y < pose.start[ua].y);
            assert!(pose.tip[la].y < pose.start[la].y);
            // Wrists hang near hip height; the tiny idle elbow bend and
            // outward splay account for the slack.
            assert!(
                pose.tip[la].y > 0.90 && pose.tip[la].y < 0.925,
                "wrist rests at {}",
                pose.tip[la].y
            );
        }
        // Feet on the ground, toes forward.
        for ft in [bone::FOOT_L, bone::FOOT_R] {
            assert!(pose.tip[ft].y >= -0.02 && pose.tip[ft].y <= 0.02);
            assert!((pose.tip[ft].z - 0.16).abs() < 1e-4);
        }
        // Left/right bones mirror across x = 0.
        assert!((pose.tip[bone::UPPER_ARM_L].x + pose.tip[bone::UPPER_ARM_R].x).abs() < 1e-5);
    }

    /// Heading rotates the whole figure: forward (+z) at 0, +x at π/2.
    #[test]
    fn heading_rotates_the_rig() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        // Heading π/2 maps local +z (rest toe) to world +x and local +x
        // (left side) to world −z: rest left toe sits at +0.16 x, −0.10 z
        // around the root.
        Character::new(Vec3::new(5.0, 0.0, -2.0), std::f32::consts::FRAC_PI_2)
            .pose_into(&skel, &mut pose);
        assert!((pose.tip[bone::FOOT_L].x - 5.16).abs() < 1e-4);
        assert!((pose.tip[bone::FOOT_L].z - (-2.10)).abs() < 1e-4);
        assert!((pose.start[bone::HIPS].x - 5.0).abs() < 1e-5);
    }

    /// Scale multiplies every bone: a 1.1× figure stands 1.1× tall.
    #[test]
    fn scale_stretches_the_rig() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        let mut c = Character::new(Vec3::ZERO, 0.0);
        c.scale = 1.1;
        c.pose_into(&skel, &mut pose);
        assert!((pose.tip[bone::HEAD].y - 1.75 * 1.1).abs() < 1e-3);
    }

    /// Stepping is a pure transition: same inputs, same checksums; a
    /// different input sequence diverges.
    #[test]
    fn stepping_is_deterministic() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        let mut run = || {
            let mut c = Character::new(Vec3::ZERO, 0.3);
            let input = circle_input(5.0, 1.35, 1.0);
            for _ in 0..600 {
                c.step(SIM_DT, &input);
                c.pose_into(&skel, &mut pose);
            }
            (c.pos, c.phase, c.footfalls, pose.checksum())
        };
        assert_eq!(run(), run());
        let mut other = Character::new(Vec3::ZERO, 0.3);
        for _ in 0..600 {
            other.step(SIM_DT, &circle_input(5.0, 1.4, 1.0));
        }
        assert_ne!(other.phase, run().1);
    }

    /// Footfall rate matches the phase: two heel strikes per cycle, and
    /// the cycle count follows ground distance over stride length.
    #[test]
    fn footfalls_track_stride_cycles() {
        let mut c = Character::new(Vec3::ZERO, 0.0);
        c.speed = 1.35;
        let input = MovementInput {
            target_speed: 1.35,
            turn_rate: 0.0,
            jump: false,
            punch: false,
        };
        let ticks = 600;
        for _ in 0..ticks {
            c.step(SIM_DT, &input);
        }
        let seconds = ticks as f32 * SIM_DT;
        let cycles = 1.35 * seconds / WALK.stride_len;
        let expected = (cycles * 2.0) as u64;
        assert!(
            (c.footfalls as i64 - expected as i64).abs() <= 1,
            "footfalls {} vs expected ~{expected}",
            c.footfalls
        );
    }

    /// Walking stays grounded: over a full cycle each foot's minimum
    /// height sits near the ground (stance contact) while the swing
    /// foot clearly lifts. This is the anti-foot-sliding/floating pin.
    #[test]
    fn walk_keeps_feet_near_ground_and_swings_clear() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        let mut c = walker(1.35);
        let input = MovementInput {
            target_speed: 1.35,
            turn_rate: 0.0,
            jump: false,
            punch: false,
        };
        let mut min_y = [f32::MAX; 2];
        let mut max_y = [-f32::MAX; 2];
        for _ in 0..240 {
            c.step(SIM_DT, &input);
            c.pose_into(&skel, &mut pose);
            for (side, ft) in [bone::FOOT_L, bone::FOOT_R].iter().enumerate() {
                let y = pose.tip[*ft].y;
                min_y[side] = min_y[side].min(y);
                max_y[side] = max_y[side].max(y);
            }
        }
        for side in 0..2 {
            assert!(
                min_y[side] > -0.08 && min_y[side] < 0.06,
                "stance foot {side} sinks/floats: min {}",
                min_y[side]
            );
            assert!(
                max_y[side] > 0.03,
                "swing foot {side} never clears: max {}",
                max_y[side]
            );
        }
    }

    /// Arms counter-swing: when the left leg is maximally forward, the
    /// left arm is behind the torso and the right arm ahead of it.
    #[test]
    fn arms_counter_swing_the_legs() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        let mut c = walker(1.35);
        // Left thigh forward peaks at φ = π/2 (thigh = −amp·sin φ).
        c.phase = std::f32::consts::FRAC_PI_2;
        c.pose_into(&skel, &mut pose);
        let shoulder_z = pose.start[bone::TORSO].z;
        let elbow_l_z = pose.tip[bone::UPPER_ARM_L].z;
        let elbow_r_z = pose.tip[bone::UPPER_ARM_R].z;
        let knee_l_z = pose.tip[bone::UPPER_LEG_L].z;
        let knee_r_z = pose.tip[bone::UPPER_LEG_R].z;
        assert!(knee_l_z > knee_r_z, "left leg must be forward");
        assert!(
            elbow_l_z < elbow_r_z,
            "left arm must trail while left leg leads"
        );
        let _ = shoulder_z;
    }

    /// Idle is calm: near-zero speed leaves the standing pose (bar the
    /// breath), and the blends are monotone in speed.
    #[test]
    fn idle_stands_and_blends_are_monotone() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        let mut c = walker(0.0);
        c.t = 0.0; // breath at t=0 is 0: exact rest pose
        c.pose_into(&skel, &mut pose);
        assert!((pose.tip[bone::HEAD].y - 1.75).abs() < 1e-4);
        assert!((pose.tip[bone::FOOT_L].z - 0.16).abs() < 1e-4);

        assert_eq!(move_blend(0.0), 0.0);
        assert_eq!(move_blend(1.35), 1.0);
        assert_eq!(run_blend(1.35), 0.0);
        assert_eq!(run_blend(3.6), 1.0);
        let mut last = 0.0;
        for k in 0..100 {
            let s = k as f32 / 20.0;
            let b = move_blend(s) + run_blend(s);
            assert!(b >= last - 1e-6, "blend not monotone at {s}");
            last = b;
        }
    }

    /// Run visibly differs from walk: deeper knee, more lean, longer
    /// stride — pinned through the blended parameters.
    #[test]
    fn run_gait_escalates_from_walk() {
        let walk = gait_at(1.35);
        let run = gait_at(3.6);
        assert!(run.knee > walk.knee + 0.2);
        assert!(run.lean > walk.lean + 0.1);
        assert!(run.stride_len > walk.stride_len + 0.5);
        assert!(run.arm > walk.arm);
        assert_eq!(gait_at(1.35).thigh, WALK.thigh, "walk band is pure WALK");
    }

    /// Speed eases toward its target within the accel/decel limits.
    #[test]
    fn speed_eases_and_clamps_positive() {
        let mut c = walker(0.0);
        let fast = MovementInput {
            target_speed: 3.6,
            turn_rate: 0.0,
            jump: false,
            punch: false,
        };
        c.step(SIM_DT, &fast);
        assert!((c.speed - ACCEL * SIM_DT).abs() < 1e-6);
        for _ in 0..600 {
            c.step(SIM_DT, &fast);
        }
        assert!((c.speed - 3.6).abs() < 1e-4);
        let stop = MovementInput::default();
        for _ in 0..120 {
            c.step(SIM_DT, &stop);
        }
        assert_eq!(c.speed, 0.0);
    }

    /// Circle input walks a circle: from (0, 0, r) facing +x, a positive
    /// turn rate keeps the ring centered on the origin; a quarter lap
    /// turns the heading by ~π/2.
    #[test]
    fn circle_input_curves_the_path() {
        let radius = 5.0f32;
        let mut c = Character::new(Vec3::new(0.0, 0.0, radius), std::f32::consts::FRAC_PI_2);
        // Already at cruise speed: the ring assertion checks steady
        // curvature, not the launch ramp.
        c.speed = 1.35;
        let input = circle_input(radius, 1.35, 1.0);
        let quarter = (std::f32::consts::FRAC_PI_2 * radius / 1.35 / SIM_DT) as usize;
        for _ in 0..quarter {
            c.step(SIM_DT, &input);
        }
        assert!(
            (c.heading - std::f32::consts::PI).abs() < 0.05,
            "quarter lap should turn heading to ~π, turned {}",
            c.heading
        );
        let dist = (c.pos.x.powi(2) + c.pos.z.powi(2)).sqrt();
        assert!(
            (dist - radius).abs() < 0.1,
            "drifted off the ring: r = {dist}"
        );
    }

    /// A jump is ballistic: it leaves the ground on request, peaks
    /// near `JUMP_V0²/(2·GRAVITY)`, lands after `2·JUMP_V0/GRAVITY`,
    /// and while airborne never advances the stride (at most the one
    /// touchdown tick) or lands a heel strike.
    #[test]
    fn jump_flies_ballistic_and_lands() {
        let mut c = walker(1.35);
        let launch = MovementInput {
            target_speed: 1.35,
            jump: true,
            ..Default::default()
        };
        c.step(SIM_DT, &launch);
        assert!(c.y > 0.0, "must leave the ground");
        let phase_at_launch = c.phase;
        let mut apex = 0.0f32;
        let mut ticks = 1u32;
        while !c.grounded() {
            c.step(SIM_DT, &MovementInput::default());
            apex = apex.max(c.y);
            ticks += 1;
            assert!(ticks < 100, "never landed");
        }
        let want_apex = JUMP_V0 * JUMP_V0 / (2.0 * GRAVITY);
        assert!(
            (apex - want_apex).abs() < 0.02,
            "apex {apex} vs {want_apex}"
        );
        let want_air = 2.0 * JUMP_V0 / GRAVITY;
        assert!(
            (ticks as f32 * SIM_DT - want_air).abs() < 3.0 * SIM_DT,
            "air time {} vs {want_air}",
            ticks as f32 * SIM_DT
        );
        assert_eq!((c.y, c.vy), (0.0, 0.0));
        assert_eq!(c.land_t, 0.0, "touchdown must arm the absorption");
        let per_tick = TAU * 1.35 / WALK.stride_len * SIM_DT;
        assert!(
            c.phase - phase_at_launch <= per_tick * 1.5,
            "stride must freeze through the flight"
        );
        assert_eq!(c.footfalls, 0, "no heel strikes without ground contact");
    }

    /// Requests while airborne change nothing: the arc is a single
    /// ballistic flight whatever the input claims mid-air.
    #[test]
    fn jump_requests_midair_are_ignored() {
        let run = |midair: bool| {
            let mut c = walker(1.35);
            let launch = MovementInput {
                jump: true,
                ..Default::default()
            };
            c.step(SIM_DT, &launch);
            for k in 0..40 {
                let input = MovementInput {
                    jump: midair && k == 5,
                    ..Default::default()
                };
                c.step(SIM_DT, &input);
            }
            (c.y, c.vy, c.pos, c.phase, c.footfalls)
        };
        assert_eq!(run(false), run(true));
    }

    /// Touchdown absorbs: the root dips below standing height while
    /// both knees take up and the feet stay planted, recovering to
    /// the standing rig within the absorption window.
    #[test]
    fn landing_dips_then_recovers() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        let mut c = walker(0.0);
        c.land_t = 0.0; // touchdown just happened
        c.pose_into(&skel, &mut pose);
        assert!(
            pose.start[bone::HIPS].y < HIP_HEIGHT - 0.01,
            "no dip at touchdown: {}",
            pose.start[bone::HIPS].y
        );
        for ft in [bone::FOOT_L, bone::FOOT_R] {
            assert!(
                pose.tip[ft].y > -0.05 && pose.tip[ft].y < 0.05,
                "feet must stay planted through the absorb: {}",
                pose.tip[ft].y
            );
        }
        c.land_t = LAND_WIN;
        c.pose_into(&skel, &mut pose);
        assert!(
            (pose.start[bone::HIPS].y - HIP_HEIGHT).abs() < 1e-4,
            "must recover to standing"
        );
    }

    /// The airborne pose tucks: mid-arc both feet clear their standing
    /// height by a visible margin and the arms ease outward.
    #[test]
    fn airborne_pose_tucks_legs_and_arms_out() {
        let skel = Skeleton::humanoid();
        let mut ground = CharacterPose::new(&skel);
        let mut air = CharacterPose::new(&skel);
        let mut c = walker(0.0);
        c.pose_into(&skel, &mut ground);
        c.y = 0.15; // mid-arc
        c.pose_into(&skel, &mut air);
        for ft in [bone::FOOT_L, bone::FOOT_R] {
            assert!(
                air.tip[ft].y > ground.tip[ft].y + 0.10,
                "tucked foot must clear: {} vs {}",
                air.tip[ft].y,
                ground.tip[ft].y
            );
        }
        for la in [bone::LOWER_ARM_L, bone::LOWER_ARM_R] {
            assert!(
                air.tip[la].x.abs() > ground.tip[la].x.abs() + 0.05,
                "arms must ease out in the air"
            );
        }
    }

    /// The punch is one-shot with recovery: one request runs exactly
    /// one cycle, requests during the action don't restart or extend
    /// it, and the channel returns to idle ready for the next.
    #[test]
    fn punch_is_one_shot_and_recovers() {
        let mut c = walker(0.0);
        let hit = MovementInput {
            punch: true,
            ..Default::default()
        };
        c.step(SIM_DT, &hit);
        assert_eq!(c.punch, Some(0.0));
        let mut active = 0;
        while c.punch.is_some() {
            c.step(SIM_DT, &hit); // still requesting: must not restart
            active += 1;
            assert!(active < 100, "punch never ends");
        }
        let expected = (PUNCH_DUR / SIM_DT).ceil();
        assert!(
            (active as f32 - expected).abs() <= 1.0,
            "active ticks {active} vs ~{expected}"
        );
        c.step(SIM_DT, &MovementInput::default());
        assert_eq!(c.punch, None);
        c.step(SIM_DT, &hit);
        assert_eq!(c.punch, Some(0.0));
    }

    /// At the strike the right arm extends forward of the shoulder,
    /// near straight, torso thrown in and the left arm home; at the
    /// cycle's end the pose is bit-for-bit the gait's again.
    #[test]
    fn punch_strikes_forward_and_returns() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        let mut plain = CharacterPose::new(&skel);
        walker(0.0).pose_into(&skel, &mut plain);
        let mut c = walker(0.0);
        // Strike plateau: `p_ext` sits at 1 through phase 0.45..0.60.
        c.punch = Some(0.5);
        c.pose_into(&skel, &mut pose);
        let reach = pose.tip[bone::LOWER_ARM_R].z - pose.start[bone::UPPER_ARM_R].z;
        assert!(reach > 0.45, "strike must extend forward: {reach}");
        let span = (pose.tip[bone::LOWER_ARM_R] - pose.start[bone::UPPER_ARM_R]).length();
        let arm = skel.length(bone::UPPER_ARM_R) + skel.length(bone::LOWER_ARM_R);
        assert!(
            (span - arm).abs() < 0.03,
            "elbow straight at strike: {span} vs {arm}"
        );
        assert!(
            pose.tip[bone::LOWER_ARM_L].z < pose.tip[bone::LOWER_ARM_R].z - 0.30,
            "left arm must stay home while the right strikes"
        );
        // Recovery blends fully out: end-of-cycle pose equals the
        // plain gait pose bit for bit.
        c.punch = Some(0.99);
        c.pose_into(&skel, &mut pose);
        assert_eq!(pose.checksum(), plain.checksum());
    }

    /// Punching composes with the gait: one request tick runs one
    /// cycle while the stride keeps locking to the ground (heel
    /// strikes keep landing throughout the action).
    #[test]
    fn punch_composes_with_the_walk() {
        let mut c = walker(1.35);
        let walk = MovementInput {
            target_speed: 1.35,
            ..Default::default()
        };
        let mut hit = walk;
        hit.punch = true;
        c.step(SIM_DT, &hit); // the one request
        c.step(SIM_DT, &walk); // request gone: no chaining without it
        let mut active = 0;
        while c.punch.is_some() {
            c.step(SIM_DT, &walk);
            active += 1;
            assert!(active < 100, "punch never ends");
        }
        let expected = PUNCH_DUR / SIM_DT;
        assert!(
            (active as f32 - expected).abs() <= 1.0,
            "one cycle per request: {active} vs ~{expected}"
        );
        let ticks = 2 + active;
        let strides = 1.35 * (ticks as f32 * SIM_DT) / WALK.stride_len;
        let want = (strides * 2.0) as u64;
        assert!(
            (c.footfalls as i64 - want as i64).abs() <= 1,
            "heel strikes {} vs ~{want}",
            c.footfalls
        );
    }

    /// The action channels keep stepping deterministic: the same
    /// jump/punch request pattern reproduces the same state and pose
    /// checksum over a 10 s walk.
    #[test]
    fn action_stepping_is_deterministic() {
        let skel = Skeleton::humanoid();
        let mut pose = CharacterPose::new(&skel);
        let mut run = || {
            let mut c = walker(1.35);
            for k in 0..600 {
                let input = MovementInput {
                    target_speed: 1.35,
                    jump: k % 40 == 0,
                    punch: k % 13 == 0,
                    ..Default::default()
                };
                c.step(SIM_DT, &input);
                c.pose_into(&skel, &mut pose);
            }
            (
                c.y,
                c.vy,
                c.land_t,
                c.phase,
                c.punch,
                c.footfalls,
                pose.checksum(),
            )
        };
        assert_eq!(run(), run());
    }
}
