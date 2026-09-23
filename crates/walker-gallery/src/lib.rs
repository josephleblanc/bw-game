//! bw-walker-gallery: the character-locomotion crate — a 3D-compatible
//! Bevy plugin (this library) plus the gallery binary that measures and
//! renders it (see `main.rs`).
//!
//! The plugin owns no renderer: it advances [`Walker`] characters under
//! the tick contract and mirrors world-space 3D bone positions into
//! [`BoneSegment`] components. Any 3D surface can consume that — the
//! offline SVG render (3D camera projection, default build) and the
//! live 3D viewer (`viewer` feature) are the two this crate ships, and
//! both read identical data.
//!
//! ## Tick contract wiring ([`bw_core::time`])
//!
//! - The sim steps only in `FixedUpdate`, exactly once per run, always
//!   by `SIM_DT` — never Bevy's wall-clock `Time`. Live apps let Bevy's
//!   `Time<Fixed>` accumulator drive the schedule (the contract's
//!   interactive boundary); the headless harness runs the schedule
//!   directly once per measured tick
//!   (`world.try_run_schedule(FixedUpdate)` + `app.update()`), so the
//!   same `(scene, seed, ticks)` reproduces the same checksum in both.
//! - `sync_bones` runs in `Update` and only *samples*: it interpolates
//!   between the last two sim poses at [`MovementAlpha`] (the live host
//!   refreshes it from the accumulator's overstep fraction; the
//!   headless default is 1.0 — the current tick, endpoint-exact).
//! - Controllers write [`MovementInput`] before the step: the scripted
//!   [`CirclePath`] ships with the plugin; the keyboard controller
//!   lives in the viewer. Input is data, never a wall clock.
//!
//! Like every gallery this crate allocates at setup (pose buffers per
//! walker) and not per tick (ADR 0004): stepping and syncing are
//! zero-allocation at steady state, gated by `[steady.*]` budgets.

use alloc_probe::alloc_probe;
use bevy::app::{App, FixedUpdate, Plugin, Update};
use bevy::ecs::prelude::*;
use bevy::ecs::schedule::SystemSet;

use bw_core::character::{Character, CharacterPose, MovementInput, Skeleton, circle_input};
use bw_core::math::Vec3;
use bw_core::time::SIM_DT;

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// The shared humanoid rig. One skeleton serves every walker; per-figure
/// variation is character state (scale, phase), not rig data.
#[derive(Resource)]
pub struct WalkerSkeleton(pub Skeleton);

/// Interpolation alpha for the render mirror: 0 = previous tick's pose,
/// 1 = current. Live hosts refresh it from `Time<Fixed>` each frame;
/// headless hosts leave the default 1.0.
#[derive(Resource)]
pub struct MovementAlpha(pub f32);

impl Default for MovementAlpha {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Pause gate for the sim step: the tick still runs (schedules,
/// mirrors, measurement all keep going) but characters hold still.
#[derive(Resource, Default)]
pub struct MovementPause(pub bool);

/// Walker spawn counter — stable ids for order-independent checksums.
#[derive(Resource, Default)]
struct NextWalkerId(u32);

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// One character: its sim state and the interpolation pair of poses
/// (`prev` = two steps ago, `curr` = the latest tick). Controllers set
/// `input`; the plugin does the rest.
#[derive(Component)]
pub struct Walker {
    /// Stable spawn id (checksum ordering, camera targeting).
    pub id: u32,
    /// Sim state (position, heading, speed, gait phase).
    pub character: Character,
    /// This tick's locomotion intent, written by a controller.
    pub input: MovementInput,
    /// Pose at the previous tick (interpolation source).
    pub prev: CharacterPose,
    /// Pose at the current tick (interpolation target).
    pub curr: CharacterPose,
}

/// Render mirror of one bone: world-space endpoints, interpolated
/// between the last two sim ticks. `width` is the skeleton's rest width
/// (bones do not change length — renderers scale geometry at spawn).
#[derive(Component)]
pub struct BoneSegment {
    /// The walker this bone belongs to.
    pub walker: Entity,
    /// Bone index into the skeleton/pose arrays.
    pub bone: u16,
    /// Joint (start) position, world space.
    pub a: Vec3,
    /// Tip position, world space.
    pub b: Vec3,
    /// Rest width (meters, scale-adjusted at spawn).
    pub width: f32,
}

/// Scripted controller: walk a circle of `radius` at `speed` (positive
/// `dir` = counterclockwise seen from above). Drives the gallery scenes
/// and the viewer's ambient walkers.
#[derive(Component)]
pub struct CirclePath {
    pub radius: f32,
    pub speed: f32,
    pub dir: f32,
}

/// Scripted controller: walk to a point on the ground, then hold. The
/// figure turns toward the bearing first (halting to turn in place
/// when it is more than a quarter-turn off), walks at
/// [`MOVE_TO_SPEED`], and stops inside the arrival radius. An arrived
/// target stays as an inert component instead of being removed: the
/// fixed-step systems take no `Commands`, so the tick stays
/// allocation-free (ADR 0004) and controllers can only write input.
#[derive(Component)]
pub struct MoveTarget {
    /// Goal on the ground plane.
    pub pos: Vec3,
    /// Arrive when within this radius (m).
    pub arrive: f32,
}

/// Cruise speed of point-to-point walks (m/s) — the walk band.
pub const MOVE_TO_SPEED: f32 = 1.35;
/// Turn authority of the move-to controller (rad/s).
const MOVE_TURN: f32 = 2.6;
/// Bearing-error gain (1/s), saturating at `MOVE_TURN`.
const MOVE_TURN_GAIN: f32 = 6.0;
/// Bearing error beyond which the figure halts and turns in place.
const MOVE_TURN_HALT: f32 = std::f32::consts::FRAC_PI_2;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// The render-mirror system set: viewer-side systems that read
/// [`BoneSegment`]s schedule themselves `.after(MovementMirror)` so they
/// never see a half-updated mirror.
#[derive(Debug, Hash, PartialEq, Eq, Clone, Copy, SystemSet)]
pub struct MovementMirror;

/// Character movement in fixed steps: scripted controllers write their
/// inputs, `step_walkers` advances every character exactly one `SIM_DT`
/// and re-poses it, and the `Update` mirror samples the result. This is
/// the crate's reason to exist — everything else is surfaces on top.
pub struct CharacterMovementPlugin;

impl Plugin for CharacterMovementPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MovementAlpha>()
            .init_resource::<MovementPause>()
            .init_resource::<NextWalkerId>()
            .add_systems(
                FixedUpdate,
                (step_circle_paths, step_move_targets, step_walkers).chain(),
            )
            .add_systems(Update, sync_bones.in_set(MovementMirror));
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

#[alloc_probe]
fn step_circle_paths(mut query: Query<(&CirclePath, &mut Walker)>) {
    for (path, mut walker) in &mut query {
        walker.input = circle_input(path.radius, path.speed, path.dir);
    }
}

/// Point-to-point control: turn toward the bearing, walk, stop inside
/// the arrival radius (then hold — see [`MoveTarget`]). The bearing
/// uses the character's own heading convention (+z at 0,
/// `atan2(x, z)`), with the error wrapped to `−π..π` because heading
/// is unbounded.
#[alloc_probe]
fn step_move_targets(mut query: Query<(&MoveTarget, &mut Walker)>) {
    for (target, mut walker) in &mut query {
        let Walker {
            character, input, ..
        } = &mut *walker;
        let to = target.pos - character.pos;
        let dist = Vec3::new(to.x, 0.0, to.z).length();
        if dist <= target.arrive {
            *input = MovementInput::default();
            continue;
        }
        let bearing = to.x.atan2(to.z);
        let err = (bearing - character.heading + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        *input = MovementInput {
            target_speed: if err.abs() > MOVE_TURN_HALT {
                0.0
            } else {
                MOVE_TO_SPEED
            },
            turn_rate: (err * MOVE_TURN_GAIN).clamp(-MOVE_TURN, MOVE_TURN),
            jump: false,
            punch: false,
            reach: false,
        };
    }
}

/// The sim: one fixed `SIM_DT` per character, pose buffers swapped and
/// rewritten in place. Deterministic from the input sequence alone.
#[alloc_probe]
fn step_walkers(
    pause: Res<MovementPause>,
    skeleton: Res<WalkerSkeleton>,
    mut query: Query<&mut Walker>,
) {
    if pause.0 {
        return;
    }
    for mut walker in &mut query {
        let Walker {
            character,
            input,
            prev,
            curr,
            ..
        } = &mut *walker;
        core::mem::swap(prev, curr);
        character.step(SIM_DT, input);
        character.pose_into(&skeleton.0, curr);
    }
}

/// The render mirror: sample without advancing. Each bone lerps between
/// the last two sim poses at the host-provided alpha — endpoint-exact
/// at 0 and 1, so a paused or headless host (alpha = 1) draws the
/// current tick bit for bit.
#[alloc_probe]
fn sync_bones(
    alpha: Res<MovementAlpha>,
    mut segments: Query<&mut BoneSegment>,
    walkers: Query<&Walker>,
) {
    let a = alpha.0.clamp(0.0, 1.0);
    for mut seg in &mut segments {
        let Ok(walker) = walkers.get(seg.walker) else {
            continue; // bone outlived its walker for a frame
        };
        let b = seg.bone as usize;
        seg.a = walker.prev.start[b].lerp(walker.curr.start[b], a);
        seg.b = walker.prev.tip[b].lerp(walker.curr.tip[b], a);
    }
}

// ---------------------------------------------------------------------------
// Setup and readout helpers (shared by the harness and the viewer)
// ---------------------------------------------------------------------------

/// Spawn a walker and its `BONE_COUNT` bone segments. Setup-window cost
/// only (ADR 0004): pose buffers allocate here, never per tick.
pub fn spawn_walker(
    world: &mut World,
    skeleton: &Skeleton,
    character: Character,
    input: MovementInput,
) -> Entity {
    let id = world.resource::<NextWalkerId>().0;
    world.resource_mut::<NextWalkerId>().0 = id.wrapping_add(1);
    let scale = character.scale;
    let walker = world
        .spawn(Walker {
            id,
            character,
            input,
            prev: CharacterPose::new(skeleton),
            curr: CharacterPose::new(skeleton),
        })
        .id();
    for b in 0..skeleton.bone_count() {
        world.spawn(BoneSegment {
            walker,
            bone: b as u16,
            a: Vec3::ZERO,
            b: Vec3::ZERO,
            width: skeleton.width(b) * scale,
        });
    }
    walker
}

/// Order-independent state digest: sums an avalanche-mixed per-walker
/// checksum over the current poses. Query iteration order cannot change
/// it, exactly like the pose checksums it builds on.
pub fn state_checksum(world: &mut World) -> u64 {
    let mut query = world.query::<&Walker>();
    let mut sum = 0u64;
    for walker in query.iter(world) {
        let mixed = walker
            .curr
            .checksum()
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .rotate_left(23)
            ^ (walker.id as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
        sum = sum.wrapping_add(mixed);
    }
    sum
}

/// Total heel strikes landed so far (the report's `interactions`).
pub fn total_footfalls(world: &mut World) -> u64 {
    let mut query = world.query::<&Walker>();
    query.iter(world).map(|w| w.character.footfalls).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bw_core::character::BONE_COUNT;

    /// The plugin steps characters under the harness's own pump: one
    /// FixedUpdate run + one update per tick, all through the public
    /// plugin registration (this is exactly what main.rs measures).
    #[test]
    fn plugin_walks_a_circle_deterministically() {
        let run = |ticks: u32| -> (u64, u64, Vec3) {
            let mut app = App::new();
            app.add_plugins(CharacterMovementPlugin)
                .insert_resource(WalkerSkeleton(Skeleton::humanoid()));
            let skel = app.world().resource::<WalkerSkeleton>().0.clone();
            let mut character =
                Character::new(Vec3::new(0.0, 0.0, 5.0), std::f32::consts::FRAC_PI_2);
            character.speed = 1.35;
            let walker = spawn_walker(
                app.world_mut(),
                &skel,
                character,
                circle_input(5.0, 1.35, 1.0),
            );
            app.world_mut().entity_mut(walker).insert(CirclePath {
                radius: 5.0,
                speed: 1.35,
                dir: 1.0,
            });
            for _ in 0..ticks {
                assert!(
                    app.world_mut().try_run_schedule(FixedUpdate).is_ok(),
                    "plugin must register FixedUpdate systems"
                );
                app.update();
            }
            let Some(w) = app.world().get::<Walker>(walker) else {
                panic!("walker must stay alive");
            };
            let pos = w.character.pos;
            (
                state_checksum(app.world_mut()),
                total_footfalls(app.world_mut()),
                pos,
            )
        };
        let a = run(600);
        let b = run(600);
        assert_eq!(a, b, "same tick count must reproduce the same state");
        let c = run(601);
        assert_ne!(a.0, c.0, "one more tick must change the checksum");
        assert!(a.1 > 0, "a 10s walk must land heel strikes");
        // The scripted controller drives through input data: the
        // harness never injected a controller of its own.
        assert!(
            (a.2.x.powi(2) + a.2.z.powi(2)).sqrt() < 5.5,
            "walker left the ring: {a:?}"
        );
    }

    /// Point-to-point control: a walker sent to a point ahead walks
    /// there and stops inside the arrival radius, with the gait easing
    /// to idle afterwards.
    #[test]
    fn move_target_walks_to_the_point_and_stops() {
        let mut app = App::new();
        app.add_plugins(CharacterMovementPlugin)
            .insert_resource(WalkerSkeleton(Skeleton::humanoid()));
        let skel = app.world().resource::<WalkerSkeleton>().0.clone();
        let walker = spawn_walker(
            app.world_mut(),
            &skel,
            Character::new(Vec3::ZERO, 0.0),
            MovementInput::default(),
        );
        app.world_mut().entity_mut(walker).insert(MoveTarget {
            pos: Vec3::new(0.0, 0.0, 4.0),
            arrive: 0.30,
        });
        for _ in 0..900 {
            let _ = app.world_mut().try_run_schedule(FixedUpdate);
            app.update();
        }
        let Some(w) = app.world().get::<Walker>(walker) else {
            panic!("walker must stay alive");
        };
        let dist = w.character.pos.length();
        assert!(
            (dist - 4.0).abs() < 0.35,
            "should stand at the goal: dist {dist}"
        );
        assert!(
            w.character.speed < 0.05,
            "gait must ease to idle after arrival: {}",
            w.character.speed
        );
        assert_eq!(w.input, MovementInput::default());
    }

    /// A goal behind the figure turns it in place first: while the
    /// bearing error exceeds a quarter turn the speed command is zero,
    /// so the figure pivots before committing to a stride — no
    /// orbiting around the goal.
    #[test]
    fn move_target_turns_in_place_when_goal_behind() {
        let mut app = App::new();
        app.add_plugins(CharacterMovementPlugin)
            .insert_resource(WalkerSkeleton(Skeleton::humanoid()));
        let skel = app.world().resource::<WalkerSkeleton>().0.clone();
        let walker = spawn_walker(
            app.world_mut(),
            &skel,
            Character::new(Vec3::ZERO, 0.0),
            MovementInput::default(),
        );
        app.world_mut().entity_mut(walker).insert(MoveTarget {
            pos: Vec3::new(0.0, 0.0, -3.0),
            arrive: 0.30,
        });
        // First half second: pivoting, essentially no ground covered.
        for _ in 0..30 {
            let _ = app.world_mut().try_run_schedule(FixedUpdate);
            app.update();
        }
        let Some(w) = app.world().get::<Walker>(walker) else {
            panic!("walker must stay alive");
        };
        assert!(
            w.character.pos.length() < 0.3,
            "must turn in place before walking: {:?}",
            w.character.pos
        );
        // The pivot converges on the −z bearing (π, from either
        // side — heading is unbounded, so wrap like the controller).
        let err = (w.character.heading - std::f32::consts::PI + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        assert!(err.abs() < 2.0, "should be mostly turned: err {}", err);
        // And it still arrives eventually.
        for _ in 0..900 {
            let _ = app.world_mut().try_run_schedule(FixedUpdate);
            app.update();
        }
        let Some(w) = app.world().get::<Walker>(walker) else {
            panic!("walker must stay alive");
        };
        assert!(
            (w.character.pos.z + 3.0).abs() < 0.35 && w.character.pos.x.abs() < 0.35,
            "should reach the behind-goal: {:?}",
            w.character.pos
        );
    }

    /// An arrived (or trivial) target holds inertly: input idles and
    /// the figure stays put for as long as the component remains.
    #[test]
    fn arrived_move_target_holds_inert() {
        let mut app = App::new();
        app.add_plugins(CharacterMovementPlugin)
            .insert_resource(WalkerSkeleton(Skeleton::humanoid()));
        let skel = app.world().resource::<WalkerSkeleton>().0.clone();
        let mut character = Character::new(Vec3::new(2.0, 0.0, 2.0), 0.7);
        character.speed = 1.35; // arriving at speed: must brake, not orbit
        let walker = spawn_walker(app.world_mut(), &skel, character, MovementInput::default());
        app.world_mut().entity_mut(walker).insert(MoveTarget {
            pos: Vec3::new(2.0, 0.0, 2.0),
            arrive: 0.30,
        });
        for _ in 0..240 {
            let _ = app.world_mut().try_run_schedule(FixedUpdate);
            app.update();
        }
        let Some(w) = app.world().get::<Walker>(walker) else {
            panic!("walker must stay alive");
        };
        let p = w.character.pos;
        assert!(
            ((p.x - 2.0).powi(2) + (p.z - 2.0).powi(2)).sqrt() < 0.65,
            "braking overshoot must stay bounded: {p:?}"
        );
        assert_eq!(w.character.speed, 0.0);
        assert_eq!(w.input, MovementInput::default());
    }

    /// The mirror is endpoint-exact at alpha 1: segments equal the
    /// current pose bit for bit, and interpolation at 0 shows the
    /// previous tick.
    #[test]
    fn mirror_interpolates_between_the_last_two_ticks() {
        let mut app = App::new();
        app.add_plugins(CharacterMovementPlugin)
            .insert_resource(WalkerSkeleton(Skeleton::humanoid()));
        let skel = app.world().resource::<WalkerSkeleton>().0.clone();
        let character = Character::new(Vec3::ZERO, 0.0);
        let walker = spawn_walker(
            app.world_mut(),
            &skel,
            character,
            MovementInput {
                target_speed: 1.35,
                turn_rate: 0.0,
                jump: false,
                punch: false,
                reach: false,
            },
        );
        for _ in 0..30 {
            let _ = app.world_mut().try_run_schedule(FixedUpdate);
            app.update();
        }
        let mut seg_query = app.world_mut().query::<&BoneSegment>();
        let Some(w) = app.world().get::<Walker>(walker) else {
            panic!("walker must stay alive");
        };
        for seg in seg_query.iter(app.world()) {
            let b = seg.bone as usize;
            assert_eq!(seg.a, w.curr.start[b]);
            assert_eq!(seg.b, w.curr.tip[b]);
            assert_ne!(seg.a, w.prev.start[b], "must have moved by now");
        }
        // Alpha 0 must freeze the mirror on the previous tick.
        app.insert_resource(MovementAlpha(0.0));
        app.update();
        let Some(w) = app.world().get::<Walker>(walker) else {
            panic!("walker must stay alive");
        };
        for seg in seg_query.iter(app.world()) {
            let b = seg.bone as usize;
            assert_eq!(seg.a, w.prev.start[b]);
        }
    }

    /// Pause holds characters but keeps the mirror working.
    #[test]
    fn pause_freezes_the_sim_not_the_schedule() {
        let mut app = App::new();
        app.add_plugins(CharacterMovementPlugin)
            .insert_resource(WalkerSkeleton(Skeleton::humanoid()));
        let skel = app.world().resource::<WalkerSkeleton>().0.clone();
        spawn_walker(
            app.world_mut(),
            &skel,
            Character::new(Vec3::ZERO, 0.0),
            MovementInput {
                target_speed: 1.35,
                turn_rate: 0.0,
                jump: false,
                punch: false,
                reach: false,
            },
        );
        for _ in 0..10 {
            let _ = app.world_mut().try_run_schedule(FixedUpdate);
            app.update();
        }
        let before = state_checksum(app.world_mut());
        app.insert_resource(MovementPause(true));
        for _ in 0..10 {
            let _ = app.world_mut().try_run_schedule(FixedUpdate);
            app.update();
        }
        assert_eq!(before, state_checksum(app.world_mut()));
    }

    #[test]
    fn bone_segments_match_skeleton_count() {
        let mut app = App::new();
        app.add_plugins(CharacterMovementPlugin)
            .insert_resource(WalkerSkeleton(Skeleton::humanoid()));
        let skel = app.world().resource::<WalkerSkeleton>().0.clone();
        spawn_walker(
            app.world_mut(),
            &skel,
            Character::new(Vec3::ZERO, 0.0),
            MovementInput::default(),
        );
        let mut q = app.world_mut().query::<&BoneSegment>();
        assert_eq!(q.iter(app.world()).count(), BONE_COUNT);
    }
}
