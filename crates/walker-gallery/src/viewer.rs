//! walker-playground: the live 3D environment for character animation —
//! a stick figure walking (and running) in a real perspective 3D scene,
//! driven through the same [`CharacterMovementPlugin`] the headless
//! harness measures. Run with `bw-walker-gallery --viewer` from a build
//! with `--features viewer`.
//!
//! Everything windowing/3D lives behind the dev-only `viewer` cargo
//! feature (see Cargo.toml): the default build — the measured headless
//! gallery — never sees this module, so the dependency graph, wasm
//! gate, and size budgets are untouched (ADR 0002).
//!
//! ## Contract wiring
//!
//! - Sim time advances only in fixed `SIM_DT` steps, in `FixedUpdate`
//!   (Bevy's `Time<Fixed>` accumulator is the interactive boundary);
//!   the plugin steps the characters, this module never touches sim
//!   time.
//! - The render mirrors only sample: this module refreshes the
//!   plugin's [`MovementAlpha`] from the accumulator each frame, and
//!   the bone/head/shadow meshes update `.after(MovementMirror)`.
//! - Keys map to [`Action`]s through a pure function; applying an
//!   action is a state transition on viewer-local or plugin-owned
//!   state (the input→action layer, growing the tree-playground
//!   pattern).
//!
//! ## Scene
//!
//! A 24×24 m checkerboard meadow, the driven figure (warm ink), seven
//! ambient circle-walkers (slate), blob shadows, and an orthographic
//! tactical follow camera (ADR 0006): ~15.5° elevation at 49° azimuth
//! — deliberately off the 45° diagonal so the tiles never read as
//! regular wallpaper — tracking the selected figure. `O` toggles a
//! perspective view of the same orientation.
//!
//! ## Control
//!
//! Colony-sim style: click a figure to select it (gold ring), click
//! the ground to send it walking there (the plugin's `MoveTarget`
//! controller; a marker disc marks the goal until arrival). Keyboard
//! drive applies to the selected figure and overrides a send; `Shift`
//! is the run toggle, `Space` jumps (held: bounce), `F` punches
//! (held: chained cycles). Clicking the selected figure again returns
//! control to the player figure.
//!
//! This path may allocate (readout strings, entity setup): ADR 0004
//! governs the headless sim, not the dev viewer.

use std::f32::consts::FRAC_PI_2;
use std::time::Duration;

use bevy::camera::PerspectiveProjection;
use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::DirectionalLight;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::{PrimaryWindow, WindowResolution};

use bw_core::character::{Carry, Character, MovementInput, Skeleton, bone};
use bw_core::math::Vec3 as BwVec3;
use bw_core::time::SIM_DT;

use bw_walker_gallery::{
    BoneSegment, CharacterMovementPlugin, CirclePath, MoveTarget, MovementAlpha, MovementPause,
    Walker, WalkerSkeleton, spawn_walker,
};

use crate::svg::{EYE, FOV_Y_DEG, TARGET};
use crate::{RUN_SPEED, WALK_SPEED, build_scene};

const WIN_W: f32 = 1280.0;
const WIN_H: f32 = 800.0;

/// Ground checker size and extent (meters).
const TILE: f32 = 1.0;
const TILES: i32 = 24;

/// Tactical camera orientation (ADR 0006): azimuth 49° off the +z
/// axis toward +x and ~15.5° elevation — the SVG establishing shot's
/// steepness, but deliberately off the 45° diagonal so the
/// checkerboard doesn't read as a perfectly regular wallpaper (one
/// family of tile edges dominates; the asymmetric faces also give the
/// view a directional hierarchy, the same trick isometric-style games
/// use when they offset the classic angle).
const CAM_AZIMUTH_DEG: f32 = 49.0;
const CAM_ELEVATION_DEG: f32 = 15.5;

/// The follow-camera offset direction (unit; a test pins azimuth and
/// elevation so this and the ADR can't drift apart).
fn follow_dir() -> Vec3 {
    let az = CAM_AZIMUTH_DEG.to_radians();
    let el = CAM_ELEVATION_DEG.to_radians();
    Vec3::new(el.cos() * az.sin(), el.sin(), el.cos() * az.cos())
}
/// Follow distance at zoom 1 (m). Meaningful to the perspective
/// projection; the default orthographic camera frames by view height
/// instead (zoom rides its `scale`).
const FOLLOW_DIST: f32 = 11.0;
const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 2.5;
const ZOOM_STEP: f32 = 1.12;
/// Camera smoothing rate (1/s): higher snappier.
const CAM_LERP: f32 = 5.0;

const TURN_RATE: f32 = 2.2; // rad/s while A/D held

/// Player ink vs ambient-walker slate.
const PLAYER_INK: Color = Color::srgb(0.13, 0.19, 0.33);
const NPC_INK: Color = Color::srgb(0.36, 0.42, 0.51);
const HEAD_INK: Color = Color::srgb(0.10, 0.14, 0.25);
const TILE_A: Color = Color::srgb(0.39, 0.66, 0.30);
const TILE_B: Color = Color::srgb(0.30, 0.56, 0.23);
const SKY: Color = Color::srgb(0.78, 0.89, 0.96);
/// Selection ring and goal-marker inks.
const SELECT_INK: Color = Color::srgba(0.95, 0.72, 0.20, 0.55);
const GOAL_INK: Color = Color::srgba(0.90, 0.25, 0.15, 0.75);
/// Selection sphere radius and goal-arrival radius (m at scale 1).
const PICK_RADIUS: f32 = 0.85;
const ARRIVE_RADIUS: f32 = 0.30;

// ---------------------------------------------------------------------------
// Input → action (pure)
// ---------------------------------------------------------------------------

/// One viewer input. `Zoom`'s payload steps by ×1.12 in/out.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Forward(bool),
    Run,
    Turn(i8),
    Jump,
    Punch,
    Reach,
    CarryCycle,
    OrthoToggle,
    PauseToggle,
    Reset,
    Zoom(i8),
    Quit,
}

/// Keyboard → action mapping (pure; Bevy only calls it). Held keys are
/// polled (`Forward`, `Turn`, `Jump`, `Punch`, `Reach`); the rest are
/// just-pressed.
fn key_action(key: &KeyCode) -> Option<Action> {
    Some(match key {
        KeyCode::KeyW | KeyCode::ArrowUp => Action::Forward(true),
        KeyCode::KeyS | KeyCode::ArrowDown => Action::Forward(false),
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Action::Run,
        KeyCode::KeyA | KeyCode::ArrowLeft => Action::Turn(-1),
        KeyCode::KeyD | KeyCode::ArrowRight => Action::Turn(1),
        KeyCode::Space => Action::Jump,
        KeyCode::KeyF => Action::Punch,
        KeyCode::KeyE => Action::Reach,
        KeyCode::KeyC => Action::CarryCycle,
        KeyCode::KeyO => Action::OrthoToggle,
        KeyCode::KeyP => Action::PauseToggle,
        KeyCode::KeyR => Action::Reset,
        KeyCode::Minus => Action::Zoom(1),
        KeyCode::Equal => Action::Zoom(-1),
        KeyCode::Escape => Action::Quit,
        _ => return None,
    })
}

/// The orthographic alternative: same orientation, flat tactical
/// read. The vertical view height matches the perspective framing at
/// the follow distance (2·d·tan(fov/2) ≈ 8 m); zoom rides `scale`,
/// which multiplies on top of the scaling mode.
const ORTHO_VIEW_H: f32 = 8.0;

fn ortho_projection() -> Projection {
    Projection::Orthographic(bevy::camera::OrthographicProjection {
        near: 0.1,
        far: 200.0,
        scaling_mode: bevy::camera::ScalingMode::FixedVertical {
            viewport_height: ORTHO_VIEW_H,
        },
        ..bevy::camera::OrthographicProjection::default_3d()
    })
}

/// The perspective default: the SVG renderer's fov at the follow rig.
fn perspective_projection() -> Projection {
    Projection::Perspective(PerspectiveProjection {
        fov: FOV_Y_DEG.to_radians(),
        ..default()
    })
}

// ---------------------------------------------------------------------------
// Viewer state
// ---------------------------------------------------------------------------

/// Per-walker handle: mesh entities carry their own link components
/// (`BoneMesh`, `HeadBall`, `ShadowDisc`), so the rig only needs the
/// walker it refers to.
struct Rig {
    walker: Entity,
}

#[derive(Resource)]
struct Viewer {
    scene: String,
    seed: u64,
    player: Rig,
    npcs: Vec<Rig>,
    /// The figure under control (keyboard input, click-to-move sends,
    /// camera focus). Starts as the player figure; clicking another
    /// figure switches, clicking it again returns to the player.
    selected: Entity,
    forward: bool,
    run: bool,
    zoom: f32,
    /// The selected figure's locomotion intent, refreshed from held
    /// keys each frame and applied to the Walker every tick thereafter.
    input: MovementInput,
    /// Set by `Action::Reset`, consumed by the input applier.
    reset_pending: bool,
    /// Set by `Action::CarryCycle`, consumed by the input applier
    /// (carry is character state, not per-tick input).
    carry_pending: bool,
    /// Whether the main camera renders orthographically (`O`
    /// toggles; zoom then rides the ortho `scale` instead of the
    /// follow distance).
    ortho: bool,
    /// Bumped on every applied action so the readout refreshes.
    edit_count: u32,
    /// Shared marker geometry/materials (spawn-once handles).
    kit: MarkerKit,
}

/// Marker assets created once at startup: the selection ring and the
/// goal disc (flat unlit circles at the ground).
struct MarkerKit {
    ring_mesh: Handle<Mesh>,
    ring_mat: Handle<StandardMaterial>,
    goal_mesh: Handle<Mesh>,
    goal_mat: Handle<StandardMaterial>,
}

#[derive(Resource, Default)]
struct CameraState {
    position: Option<Vec3>,
    look: Option<Vec3>,
}

/// Component tagging one bone mesh; `seg` is its BoneSegment.
#[derive(Component)]
struct BoneMesh {
    seg: Entity,
}

/// The head ball; `seg` is the HEAD BoneSegment.
#[derive(Component)]
struct HeadBall {
    seg: Entity,
}

/// The ground blob shadow; `walker` locates the character.
#[derive(Component)]
struct ShadowDisc {
    walker: Entity,
}

/// The selection ring under the controlled figure; `walker` locates
/// the character it marks.
#[derive(Component)]
struct SelectionRing {
    walker: Entity,
}

/// A click-to-move goal disc; lives until its walker's [`MoveTarget`]
/// is replaced, cancelled, or arrived.
#[derive(Component)]
struct GoalMarker {
    walker: Entity,
}

// ---------------------------------------------------------------------------
// Picking (pure)
// ---------------------------------------------------------------------------

/// Ground-plane hit (y = 0) of a cursor ray; `None` when the ray runs
/// parallel to the ground or away from it (upward).
fn ground_point(origin: Vec3, dir: Vec3) -> Option<Vec3> {
    if dir.y.abs() < 1e-6 {
        return None;
    }
    let t = -origin.y / dir.y;
    (t > 0.0).then(|| origin + dir * t)
}

/// Ray-vs-selection-sphere pick over walkers: each figure offers a
/// sphere at mid-body height (`PICK_RADIUS` × scale); the nearest hit
/// along the ray wins — the figure in front, not the one behind it.
fn pick_walker<'a>(
    origin: Vec3,
    dir: Vec3,
    walkers: impl Iterator<Item = (Entity, &'a Walker)>,
) -> Option<Entity> {
    let mut best: Option<(f32, Entity)> = None;
    for (entity, walker) in walkers {
        let c = &walker.character;
        let center = Vec3::new(c.pos.x, 0.55 * c.scale, c.pos.z);
        let radius = PICK_RADIUS * c.scale;
        let t = (center - origin).dot(dir).max(0.0);
        let miss = (center - (origin + dir * t)).length();
        if miss < radius && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, entity));
        }
    }
    best.map(|(_, entity)| entity)
}

#[derive(Component)]
struct MainCamera;

#[derive(Component)]
struct UiCamera;

#[derive(Component)]
struct Readout;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(scene_id: &str, seed: u64, shot: Option<&str>) {
    // Circle scenes only — mandala scenes route to mandala_view from
    // main. parse_args validated the id; the fallback is unreachable.
    let Some(preset) = crate::scene_preset(scene_id) else {
        eprintln!("bw-walker-gallery: viewer needs a circle scene, got {scene_id}");
        std::process::exit(2);
    };
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("bw-walker-gallery — {scene_id} (walker-playground)"),
            resolution: WindowResolution::new(WIN_W as u32, WIN_H as u32),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(SKY))
    // The tick contract's interactive boundary: fixed SIM_DT steps.
    .insert_resource(Time::<Fixed>::from_duration(Duration::from_secs_f32(
        SIM_DT,
    )))
    .add_plugins(CharacterMovementPlugin)
    .insert_resource(WalkerSkeleton(Skeleton::humanoid()));

    let viewer = {
        let world = app.world_mut();
        let skeleton = world.resource::<WalkerSkeleton>().0.clone();
        let player = build_rig(
            world,
            &skeleton,
            Character::new(BwVec3::new(0.0, 0.0, 3.0), std::f32::consts::PI),
            PLAYER_INK,
        );
        // Ambient walkers: the same seeded scene builders the headless
        // gallery and SVG renderer use, minus the driven figure.
        let mut npcs = Vec::new();
        for (k, spec) in build_scene(&preset, seed).into_iter().enumerate() {
            if k >= 7 {
                break; // the meadow wants a handful, not the crowd
            }
            let rig = build_rig(world, &skeleton, spec.character, NPC_INK);
            world.entity_mut(rig.walker).insert(spec.path);
            npcs.push(rig);
        }
        let kit = {
            let flat = |radius: f32| {
                bevy::mesh::Mesh::from(Circle::new(radius))
                    .rotated_by(Quat::from_rotation_x(-FRAC_PI_2))
            };
            let unlit = |ink: Color| StandardMaterial {
                base_color: ink,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            };
            let ring_mesh = world.resource_mut::<Assets<Mesh>>().add(flat(0.55));
            let goal_mesh = world.resource_mut::<Assets<Mesh>>().add(flat(0.16));
            let ring_mat = world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(unlit(SELECT_INK));
            let goal_mat = world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(unlit(GOAL_INK));
            MarkerKit {
                ring_mesh,
                ring_mat,
                goal_mesh,
                goal_mat,
            }
        };
        // The player figure starts selected: mark it from frame one.
        world.spawn((
            SelectionRing {
                walker: player.walker,
            },
            Mesh3d(kit.ring_mesh.clone()),
            MeshMaterial3d(kit.ring_mat.clone()),
            Transform::from_xyz(0.0, 0.02, 3.0),
        ));
        Viewer {
            scene: scene_id.to_string(),
            seed,
            selected: player.walker,
            player,
            npcs,
            forward: false,
            run: false,
            zoom: 1.0,
            input: MovementInput::default(),
            reset_pending: false,
            carry_pending: false,
            ortho: true, // ADR 0006: orthographic is the tactical default
            edit_count: 0,
            kit,
        }
    };
    app.insert_resource(viewer)
        .insert_resource(CameraState::default())
        .add_systems(Startup, setup_scene)
        .add_systems(
            Update,
            (
                handle_input,
                apply_selection_input,
                markers_follow,
                update_movement_alpha,
                rig_follow,
                camera_follow,
                ortho_zoom,
                update_readout,
                auto_screenshot,
            )
                .chain(),
        );
    if let Some(path) = shot {
        SHOT_PLAN_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        app.insert_resource(ShotPlan {
            path: path.to_string(),
        });
    }
    app.run();
}

/// Spawn a walker with viewer meshes: capsule per bone (exact rest
/// length — bones never change length), head sphere, blob shadow.
fn build_rig(world: &mut World, skeleton: &Skeleton, character: Character, ink: Color) -> Rig {
    let scale = character.scale;
    let walker = spawn_walker(world, skeleton, character, MovementInput::default());
    let (limb_mat, head_mat, shadow_mat) = {
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        (
            materials.add(StandardMaterial {
                base_color: ink,
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: HEAD_INK,
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: Color::srgba(0.05, 0.08, 0.04, 0.30),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            }),
        )
    };

    let own_segments: Vec<(u16, Entity)> = {
        let mut seg_query = world.query::<(Entity, &BoneSegment)>();
        seg_query
            .iter(world)
            .filter(|(_, seg)| seg.walker == walker)
            .map(|(entity, seg)| (seg.bone, entity))
            .collect()
    };
    let mut head_seg = None;
    for (bone_index, entity) in own_segments {
        if bone_index as usize == bone::HEAD {
            head_seg = Some(entity);
            continue;
        }
        let length = skeleton.length(bone_index as usize) * scale;
        let width = skeleton.width(bone_index as usize) * scale;
        let capsule = world
            .resource_mut::<Assets<Mesh>>()
            .add(bevy::mesh::Mesh::from(Capsule3d::new(
                (width * 0.5).max(0.008),
                (length - width).max(0.001),
            )));
        world.spawn((
            BoneMesh { seg: entity },
            Mesh3d(capsule),
            MeshMaterial3d(limb_mat.clone()),
            Transform::default(),
        ));
    }
    let Some(head_seg) = head_seg else {
        panic!("the rig always has a HEAD bone");
    };

    let head_mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(bevy::mesh::Mesh::from(Sphere::new(
            bw_core::character::HEAD_RADIUS * scale,
        )));
    world.spawn((
        HeadBall { seg: head_seg },
        Mesh3d(head_mesh),
        MeshMaterial3d(head_mat),
        Transform::default(),
    ));

    let shadow_mesh = world.resource_mut::<Assets<Mesh>>().add(
        bevy::mesh::Mesh::from(Circle::new(0.34 * scale))
            .rotated_by(Quat::from_rotation_x(-FRAC_PI_2)),
    );
    world.spawn((
        ShadowDisc { walker },
        Mesh3d(shadow_mesh),
        MeshMaterial3d(shadow_mat),
        Transform::from_xyz(0.0, 0.012, 0.0),
    ));

    Rig { walker }
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<bevy::mesh::Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Scene camera: opens on the SVG establishing shot; the chase
    // camera eases in behind the walker on the first frames.
    // Tonemapping::None: the default TonyMcMapFace tonemapper needs
    // the tonemapping_luts cargo feature (plus a zstd backend) to load
    // its lookup tables; skipping tonemapping keeps the dev-only
    // feature list minimal and the flat palette needs no film curve.
    commands.spawn((
        Camera3d::default(),
        Tonemapping::None,
        ortho_projection(), // ADR 0006: the tactical default; O toggles
        Transform::from_xyz(EYE.x, EYE.y, EYE.z)
            .looking_at(Vec3::new(TARGET.x, TARGET.y, TARGET.z), Vec3::Y),
        MainCamera,
        RenderLayers::layer(0),
    ));
    // Readout overlay camera: fixed 1:1 window pixels.
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        UiCamera,
        RenderLayers::layer(1),
    ));

    // Lights: soft ambient plus a sun matching the SVG shadow
    // direction (from the walker's upper right).
    commands.spawn(AmbientLight {
        color: Color::WHITE,
        brightness: 120.0,
        ..default()
    });
    commands.spawn(DirectionalLight {
        illuminance: 9_000.0,
        ..default()
    });

    // The meadow: a 24×24 checkerboard of 1 m tiles.
    let tile = meshes.add(bevy::mesh::Mesh::from(Plane3d::new(
        Vec3::Y,
        Vec2::splat(TILE * 0.5),
    )));
    let mat_a = materials.add(StandardMaterial {
        base_color: TILE_A,
        ..default()
    });
    let mat_b = materials.add(StandardMaterial {
        base_color: TILE_B,
        ..default()
    });
    for i in -TILES / 2..TILES / 2 {
        for j in -TILES / 2..TILES / 2 {
            let mat = if (i + j) % 2 == 0 {
                mat_a.clone()
            } else {
                mat_b.clone()
            };
            commands.spawn((
                Mesh3d(tile.clone()),
                MeshMaterial3d(mat),
                Transform::from_translation(Vec3::new(
                    (i as f32 + 0.5) * TILE,
                    0.0,
                    (j as f32 + 0.5) * TILE,
                )),
            ));
        }
    }

    // Readout: center-anchored block (same placement scheme as the
    // tree playground's).
    commands.spawn((
        Text2d::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgba(0.06, 0.09, 0.05, 0.92)),
        Transform::from_xyz(-WIN_W / 2.0 + 284.0, WIN_H / 2.0 - 84.0, 0.0),
        Readout,
        RenderLayers::layer(1),
    ));
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Refresh the plugin's mirror alpha from the fixed-time accumulator
/// (the contract's sampling rule; headless hosts leave it at 1.0).
fn update_movement_alpha(
    fixed: Res<Time<Fixed>>,
    pause: Res<MovementPause>,
    mut alpha: ResMut<MovementAlpha>,
) {
    alpha.0 = if pause.0 {
        1.0
    } else {
        fixed.overstep_fraction()
    };
}

fn handle_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    primary: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut projections: Query<&mut Projection, With<MainCamera>>,
    walkers: Query<(Entity, &Walker)>,
    rings: Query<Entity, With<SelectionRing>>,
    goals: Query<Entity, With<GoalMarker>>,
    mut viewer: ResMut<Viewer>,
    mut pause: ResMut<MovementPause>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    // With a shot plan the smoke script owns the drive (see
    // `auto_screenshot`); the keyboard and mouse stay inert.
    if viewer_has_shot_plan(&viewer) {
        return;
    }
    // Clicks first: figure-under-cursor selects, ground sends.
    if buttons.just_pressed(MouseButton::Left) {
        if let Some((origin, dir)) = cursor_ray(&primary, &camera_q) {
            let picked = pick_walker(origin, dir, walkers.iter());
            match picked {
                Some(hit) if hit != viewer.selected => {
                    // Selecting takes the figure off its scripted ring
                    // (a CirclePath would keep fighting the sends) and
                    // moves the ring marker to it.
                    commands.entity(hit).remove::<CirclePath>();
                    viewer.selected = hit;
                    viewer.edit_count += 1;
                    for ring in rings.iter() {
                        commands.entity(ring).despawn();
                    }
                    for goal in goals.iter() {
                        commands.entity(goal).despawn();
                    }
                    spawn_selection_ring(&mut commands, &viewer.kit, hit);
                }
                Some(_) => {
                    // Clicking the selected figure returns control to
                    // the player figure.
                    viewer.selected = viewer.player.walker;
                    viewer.edit_count += 1;
                    for ring in rings.iter() {
                        commands.entity(ring).despawn();
                    }
                    for goal in goals.iter() {
                        commands.entity(goal).despawn();
                    }
                    spawn_selection_ring(&mut commands, &viewer.kit, viewer.selected);
                }
                None => {
                    // Ground click: send the selected figure walking.
                    if let Some(ground) = ground_point(origin, dir) {
                        let goal = BwVec3::new(ground.x, 0.0, ground.z);
                        commands.entity(viewer.selected).insert(MoveTarget {
                            pos: goal,
                            arrive: ARRIVE_RADIUS,
                        });
                        for goal_marker in goals.iter() {
                            commands.entity(goal_marker).despawn();
                        }
                        spawn_goal_marker(&mut commands, &viewer.kit, viewer.selected, goal);
                        viewer.edit_count += 1;
                    }
                }
            }
        }
    }
    // Held movement keys: forward target and turn rate.
    let mut turn = 0.0f32;
    viewer.forward = keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp])
        || keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]);
    for key in keys.get_pressed() {
        if let Some(Action::Turn(d)) = key_action(key) {
            turn += d as f32 * TURN_RATE;
        }
    }
    for key in keys.get_just_pressed() {
        if let Some(action) = key_action(key) {
            viewer.edit_count += 1;
            match action {
                Action::Run => viewer.run = !viewer.run,
                Action::PauseToggle => pause.0 = !pause.0,
                Action::Reset => viewer.reset_pending = true,
                Action::Zoom(d) => {
                    let f = if d > 0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                    viewer.zoom = (viewer.zoom * f).clamp(ZOOM_MIN, ZOOM_MAX);
                }
                Action::CarryCycle => viewer.carry_pending = true,
                Action::OrthoToggle => {
                    viewer.ortho = !viewer.ortho;
                    if let Ok(mut projection) = projections.single_mut() {
                        *projection = if viewer.ortho {
                            ortho_projection()
                        } else {
                            perspective_projection()
                        };
                    }
                }
                Action::Quit => {
                    exit.write(AppExit::Success);
                }
                Action::Forward(_)
                | Action::Turn(_)
                | Action::Jump
                | Action::Punch
                | Action::Reach => {}
            }
        }
    }
    viewer.input = MovementInput {
        target_speed: if viewer.forward {
            if viewer.run { RUN_SPEED } else { WALK_SPEED }
        } else {
            0.0
        },
        turn_rate: turn,
        // Level-triggered on purpose: the character's own state
        // machine ignores airborne launches and mid-action restarts,
        // so held keys read as bounce and chained cycles.
        jump: keys.any_pressed([KeyCode::Space]),
        punch: keys.any_pressed([KeyCode::KeyF]),
        reach: keys.any_pressed([KeyCode::KeyE]),
    };
}

/// The cursor as a world ray through the main camera; `None` when the
/// cursor is outside the window or the math refuses.
fn cursor_ray(
    primary: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) -> Option<(Vec3, Vec3)> {
    let window = primary.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, transform) = camera_q.single().ok()?;
    let ray = camera.viewport_to_world(transform, cursor).ok()?;
    Some((
        ray.origin,
        Vec3::new(ray.direction.x, ray.direction.y, ray.direction.z),
    ))
}

/// Spawn the gold ring under a newly selected figure.
fn spawn_selection_ring(commands: &mut Commands, kit: &MarkerKit, walker: Entity) {
    commands.spawn((
        SelectionRing { walker },
        Mesh3d(kit.ring_mesh.clone()),
        MeshMaterial3d(kit.ring_mat.clone()),
        Transform::default(),
    ));
}

/// Spawn the goal disc for a click-to-move send.
fn spawn_goal_marker(commands: &mut Commands, kit: &MarkerKit, walker: Entity, pos: BwVec3) {
    commands.spawn((
        GoalMarker { walker },
        Mesh3d(kit.goal_mesh.clone()),
        MeshMaterial3d(kit.goal_mat.clone()),
        Transform::from_xyz(pos.x, 0.03, pos.z),
    ));
}

/// Whether the smoke plan is active (resource presence, checked without
/// a second system param so the chain stays uniform).
fn viewer_has_shot_plan(_viewer: &Viewer) -> bool {
    SHOT_PLAN_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Shot-plan presence flag: set when `run()` received `--viewer-shot`.
static SHOT_PLAN_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Apply the recorded intent to the selected walker (input is data;
/// the plugin consumes it on every fixed step that follows). Direct
/// control overrides a pending send; `Action::Reset` teleports the
/// player figure home and returns control to it.
#[allow(clippy::type_complexity)]
fn apply_selection_input(
    mut viewer: ResMut<Viewer>,
    mut walkers: Query<&mut Walker>,
    goals: Query<Entity, With<GoalMarker>>,
    mut commands: Commands,
) {
    if viewer.reset_pending {
        viewer.reset_pending = false;
        viewer.selected = viewer.player.walker;
        commands.entity(viewer.player.walker).remove::<MoveTarget>();
        for goal in goals.iter() {
            commands.entity(goal).despawn();
        }
        let Ok(mut walker) = walkers.get_mut(viewer.player.walker) else {
            return;
        };
        let scale = walker.character.scale;
        walker.character = Character::new(BwVec3::new(0.0, 0.0, 3.0), std::f32::consts::PI);
        walker.character.scale = scale;
        walker.input = MovementInput::default();
        return;
    }
    // `C` toggles the selected figure's chest carry (the other holds —
    // like `Side` — are gameplay states the scenes set; the key is a
    // plain two-state toggle like every other viewer control). Carry
    // is character state, not per-tick input, so it lands here.
    if viewer.carry_pending {
        viewer.carry_pending = false;
        if let Ok(mut walker) = walkers.get_mut(viewer.selected) {
            walker.character.carry = match walker.character.carry {
                Carry::Chest => Carry::None,
                _ => Carry::Chest,
            };
        }
    }
    let Ok(mut walker) = walkers.get_mut(viewer.selected) else {
        return;
    };
    walker.input = viewer.input;
    if viewer.input != MovementInput::default() {
        // Keyboard intent takes the channel back from a send.
        commands.entity(viewer.selected).remove::<MoveTarget>();
        for goal in goals.iter() {
            commands.entity(goal).despawn();
        }
    }
}

/// Marker housekeeping: the ring rides its selected figure (a stale
/// one waits for its despawn command); the goal disc despawns when
/// its walker's target is gone or arrived.
fn markers_follow(
    viewer: Res<Viewer>,
    walkers: Query<&Walker>,
    moves: Query<&MoveTarget>,
    rings: Query<(Entity, &SelectionRing)>,
    goals: Query<(Entity, &GoalMarker)>,
    mut transforms: Query<&mut Transform>,
    mut commands: Commands,
) {
    for (entity, ring) in rings.iter() {
        if ring.walker != viewer.selected {
            continue; // stale until the despawn command lands
        }
        if let Ok(walker) = walkers.get(ring.walker) {
            if let Ok(mut tf) = transforms.get_mut(entity) {
                tf.translation = Vec3::new(walker.character.pos.x, 0.02, walker.character.pos.z);
            }
        }
    }
    for (entity, goal) in goals.iter() {
        let arrived = match (moves.get(goal.walker), walkers.get(goal.walker)) {
            (Ok(mt), Ok(walker)) => {
                let dx = mt.pos.x - walker.character.pos.x;
                let dz = mt.pos.z - walker.character.pos.z;
                (dx * dx + dz * dz).sqrt() <= mt.arrive
            }
            _ => true, // target gone (cancelled or replaced)
        };
        if arrived {
            commands.entity(entity).despawn();
        }
    }
}

/// Keep the meshes on the mirror: capsules between bone endpoints,
/// head at the HEAD tip, shadow under the root. Two-phase reads (gather
/// segment data, then write transforms) keep the world borrows clean.
fn rig_follow(world: &mut World) {
    let bone_targets: Vec<(Entity, Vec3, Vec3)> = {
        let mut query = world.query::<(Entity, &BoneMesh)>();
        query
            .iter(world)
            .filter_map(|(entity, bone)| {
                let seg = world.get::<BoneSegment>(bone.seg)?;
                Some((entity, to_bevy(seg.a), to_bevy(seg.b)))
            })
            .collect()
    };
    for (entity, a, b) in bone_targets {
        let dir = b - a;
        if dir.length_squared() < 1e-9 {
            continue;
        }
        let Some(mut transform) = world.get_mut::<Transform>(entity) else {
            continue;
        };
        transform.translation = (a + b) * 0.5;
        transform.rotation = Quat::from_rotation_arc(Vec3::Y, dir.normalize());
    }

    let head_targets: Vec<(Entity, Vec3)> = {
        let mut query = world.query::<(Entity, &HeadBall)>();
        query
            .iter(world)
            .filter_map(|(entity, head)| {
                let seg = world.get::<BoneSegment>(head.seg)?;
                Some((entity, to_bevy(seg.b)))
            })
            .collect()
    };
    for (entity, tip) in head_targets {
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            transform.translation = tip;
        }
    }

    let shadow_targets: Vec<(Entity, Vec3)> = {
        let mut query = world.query::<(Entity, &ShadowDisc)>();
        query
            .iter(world)
            .filter_map(|(entity, disc)| {
                let walker = world.get::<Walker>(disc.walker)?;
                let p = walker.character.pos;
                Some((entity, Vec3::new(p.x, 0.012, p.z)))
            })
            .collect()
    };
    for (entity, pos) in shadow_targets {
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            transform.translation = pos;
        }
    }
}

fn to_bevy(v: BwVec3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

/// Follow camera at the SVG renderer's establishing-shot orientation:
/// the eye→target direction held constant, tracking the selected
/// figure at `FOLLOW_DIST` (× zoom) and easing between positions. The
/// offline renders and the live view share an angle, so a pose that
/// reads in one reads in the other.
fn camera_follow(
    viewer: Res<Viewer>,
    mut state: ResMut<CameraState>,
    time: Res<Time>,
    walkers: Query<&Walker>,
    mut camera: Query<&mut Transform, (With<MainCamera>, Without<UiCamera>)>,
) {
    let Ok(mut transform) = camera.single_mut() else {
        return;
    };
    let Ok(walker) = walkers.get(viewer.selected) else {
        return;
    };
    let c = &walker.character;
    let dir = follow_dir();
    let focus = Vec3::new(c.pos.x, 1.0, c.pos.z);
    let desired = focus + dir * (FOLLOW_DIST * viewer.zoom);
    let k = 1.0 - (-CAM_LERP * time.delta_secs()).exp();
    let (position, look) = match (state.position, state.look) {
        (Some(p), Some(l)) => (p.lerp(desired, k), l.lerp(focus, k)),
        _ => (desired, focus),
    };
    state.position = Some(position);
    state.look = Some(look);
    *transform = Transform::from_translation(position).looking_at(look, Vec3::Y);
}

/// Orthographic zoom: distance is meaningless to an ortho camera, so
/// `−/+` ride the projection's `scale` instead (scale up = smaller
/// figures, the same "zoom out" semantics as the distance multiplier).
fn ortho_zoom(viewer: Res<Viewer>, mut projections: Query<&mut Projection, With<MainCamera>>) {
    if !viewer.ortho {
        return;
    }
    if let Ok(mut projection) = projections.single_mut() {
        if let Projection::Orthographic(ortho) = &mut *projection {
            ortho.scale = viewer.zoom.max(0.05);
        }
    }
}

fn update_readout(
    mut readout: Query<&mut Text2d, With<Readout>>,
    viewer: Res<Viewer>,
    pause: Res<MovementPause>,
    fixed: Res<Time<Fixed>>,
    walkers: Query<&Walker>,
    mut last_edit: Local<u32>,
    mut frame: Local<u32>,
) {
    *frame = frame.wrapping_add(1);
    if viewer.edit_count == *last_edit && !(*frame).is_multiple_of(6) {
        return;
    }
    *last_edit = viewer.edit_count;
    let Ok(mut text) = readout.single_mut() else {
        return;
    };
    let Ok(walker) = walkers.get(viewer.selected) else {
        return;
    };
    let c = &walker.character;
    let gait = if c.speed < 0.15 {
        "idle"
    } else if c.speed < 2.1 {
        "walk"
    } else {
        "run"
    };
    let sel = if viewer.selected == viewer.player.walker {
        "player".to_string()
    } else {
        format!("#{}", walker.id)
    };
    let action = if c.y > 0.0 {
        format!("air {:4.2} m", c.y)
    } else {
        match (c.punch, c.reach) {
            (Some(p), _) => format!("punch {:3.0}%", p * 100.0),
            (None, Some(p)) => format!("reach {:3.0}%", p * 100.0),
            (None, None) => "—".to_string(),
        }
    };
    let carry = match c.carry {
        Carry::None if c.carry_b > 0.01 => format!("set down {:2.0}%", c.carry_b * 100.0),
        Carry::None => "—".to_string(),
        other => format!(
            "{} {:2.0}%",
            match other {
                Carry::Chest => "chest",
                Carry::Side => "side",
                Carry::None => unreachable!(),
            },
            c.carry_b * 100.0
        ),
    };
    let cam = if viewer.ortho { "ortho" } else { "persp" };
    let lines = [
        format!(
            "walker-playground  scene {}  seed {}  sel {sel}  {cam}",
            viewer.scene, viewer.seed
        ),
        format!(
            "speed {:4.2} m/s  gait {gait}  action {action}  carry {carry}  [{}]",
            c.speed,
            if pause.0 { "PAUSED" } else { "running" }
        ),
        format!(
            "t {:7.2}s  tick {}  alpha {:.2}  heel strikes {}  figures {}",
            c.t,
            (c.t / SIM_DT) as u64,
            fixed.overstep_fraction(),
            c.footfalls,
            viewer.npcs.len() + 1,
        ),
        "click fig select · click ground send · W/S walk · Shift run".to_string(),
        "A/D turn · Space jump · F punch · E reach · C carry · O ortho".to_string(),
        "R reset  P pause  -/+ zoom  Esc quit".to_string(),
    ];
    let width = lines.iter().map(String::len).max().unwrap_or(0);
    text.0 = lines
        .iter()
        .map(|l| format!("{l:<width$}"))
        .collect::<Vec<_>>()
        .join("\n");
}

// ---------------------------------------------------------------------------
// Dev smoke: scripted drive + two screenshots + exit
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct ShotPlan {
    path: String,
}

/// The smoke's shot times (sim seconds): walking, mid-arc of the hop,
/// mid-bend of a walk-reach, a chest carry on the late walk, mid-strike
/// of the run-punch, full run, mid-walk of a scripted click-to-move
/// send — all under the default orthographic camera — then the send
/// again after the scripted swap to perspective.
const SHOTS: [(f32, &str); 8] = [
    (1.5, "walk"),
    (2.3, "air"),
    (3.25, "grab"),
    (4.3, "carry"),
    (5.25, "strike"),
    (6.5, "run"),
    (7.4, "sent"),
    (8.15, "persp"),
];
const SHOT_EXIT_T: f32 = 8.8;
/// The scripted send fires here (sim seconds), after the run shot.
const SHOT_SEND_T: f32 = 6.6;
/// The camera swaps to perspective here, during the sent walk (the
/// default is orthographic — ADR 0006 — so the swap verifies the
/// other projection).
const SHOT_PERSP_T: f32 = 7.9;
/// Drive schedule (sim seconds): walk from 0.3, run from 4.5; one hop
/// at 2.0 (airborne to ~2.48); reach chained 2.8–3.7 through the walk;
/// a chest carry held 3.9–4.4 (blending out as the run ramps); punch
/// held 5.0–5.8 so the strike lands mid-run, arcing the figure — a
/// strike straight down the camera axis foreshortens to nothing, and
/// the trailing camera reads the turning figure roughly side-on.
const RUN_START: f32 = 4.5;
const JUMP_WINDOW: (f32, f32) = (2.0, 2.2);
const REACH_WINDOW: (f32, f32) = (2.8, 3.7);
const CARRY_WINDOW: (f32, f32) = (3.9, 4.4);
const PUNCH_WINDOW: (f32, f32) = (5.0, 5.8);
const PUNCH_ARC_TURN: f32 = 0.9; // rad/s through the punch window

/// With `--viewer-shot`: auto-drive the figure (walk from 0.3 s, hop at
/// 2.0 s, walk-reaches 2.8–3.7 s, chest carry 3.9–4.4 s, run from 4.5 s,
/// punch from 5.0 s, click-to-move send at 6.6 s, perspective camera at
/// 7.9 s), capture the eight shots, exit — the viewer's headless-ish
/// verification hook, sim-time driven so captures are refresh-rate
/// independent.
fn auto_screenshot(
    plan: Option<Res<ShotPlan>>,
    mut viewer: ResMut<Viewer>,
    mut walkers: Query<&mut Walker>,
    mut projections: Query<&mut Projection, With<MainCamera>>,
    mut fired: Local<[bool; 8]>,
    mut sent: Local<bool>,
    mut ortho_on: Local<bool>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(plan) = plan else { return };
    let t = match walkers.get(viewer.player.walker) {
        Ok(walker) => walker.character.t,
        Err(_) => return,
    };
    // Auto-drive: walk, then run, then a scripted send (actions and
    // carries compose with the gait by construction).
    if t >= 0.3 && t < SHOT_SEND_T {
        viewer.input.target_speed = if t >= RUN_START {
            RUN_SPEED
        } else {
            WALK_SPEED
        };
    } else if t >= SHOT_SEND_T {
        // Release the drive: the scripted send owns the channel from
        // here (a nonzero keyboard input would cancel it).
        viewer.input = MovementInput::default();
        if !*sent {
            *sent = true;
            let Ok(walker) = walkers.get(viewer.player.walker) else {
                return;
            };
            let c = &walker.character;
            let fwd = BwVec3::new(c.heading.sin(), 0.0, c.heading.cos());
            let left = BwVec3::new(c.heading.cos(), 0.0, -c.heading.sin());
            let goal = c.pos + fwd * 4.5 + left * 2.0;
            commands.entity(viewer.player.walker).insert(MoveTarget {
                pos: goal,
                arrive: ARRIVE_RADIUS,
            });
            spawn_goal_marker(&mut commands, &viewer.kit, viewer.player.walker, goal);
        }
    }
    viewer.input.jump = (JUMP_WINDOW.0..JUMP_WINDOW.1).contains(&t);
    viewer.input.punch = (PUNCH_WINDOW.0..PUNCH_WINDOW.1).contains(&t);
    viewer.input.reach = (REACH_WINDOW.0..REACH_WINDOW.1).contains(&t);
    viewer.input.turn_rate = if viewer.input.punch {
        PUNCH_ARC_TURN
    } else {
        0.0
    };
    // Scripted carry: a chest hold through the late walk (character
    // state, set directly — idempotent within the window).
    if let Ok(mut walker) = walkers.get_mut(viewer.player.walker) {
        walker.character.carry = if (CARRY_WINDOW.0..CARRY_WINDOW.1).contains(&t) {
            Carry::Chest
        } else {
            Carry::None
        };
    }
    // Scripted camera: perspective through the sent walk's tail (the
    // default is orthographic, so this verifies the toggle's other
    // side).
    if !*ortho_on && t >= SHOT_PERSP_T {
        *ortho_on = true;
        viewer.ortho = false;
        if let Ok(mut projection) = projections.single_mut() {
            *projection = perspective_projection();
        }
    }
    for (k, (at, tag)) in SHOTS.iter().enumerate() {
        if !fired[k] && t >= *at {
            fired[k] = true;
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(shot_variant(&plan.path, tag)));
        }
    }
    if t >= SHOT_EXIT_T {
        exit.write(AppExit::Success);
    }
}

/// `foo.png` → `foo-<tag>.png` (the smoke's tagged shots).
fn shot_variant(path: &str, tag: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-{tag}.{ext}"),
        None => format!("{path}-{tag}"),
    }
}

// ---------------------------------------------------------------------------
// Tests (run with `cargo test -p bw-walker-gallery --features viewer`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_preset;

    #[test]
    fn key_action_maps_the_playground_keys() {
        assert_eq!(key_action(&KeyCode::KeyW), Some(Action::Forward(true)));
        assert_eq!(key_action(&KeyCode::KeyS), Some(Action::Forward(false)));
        assert_eq!(key_action(&KeyCode::ShiftLeft), Some(Action::Run));
        assert_eq!(key_action(&KeyCode::KeyD), Some(Action::Turn(1)));
        assert_eq!(key_action(&KeyCode::Space), Some(Action::Jump));
        assert_eq!(key_action(&KeyCode::KeyF), Some(Action::Punch));
        assert_eq!(key_action(&KeyCode::KeyE), Some(Action::Reach));
        assert_eq!(key_action(&KeyCode::KeyC), Some(Action::CarryCycle));
        assert_eq!(key_action(&KeyCode::KeyO), Some(Action::OrthoToggle));
        assert_eq!(key_action(&KeyCode::KeyP), Some(Action::PauseToggle));
        assert_eq!(key_action(&KeyCode::KeyR), Some(Action::Reset));
        assert_eq!(key_action(&KeyCode::Minus), Some(Action::Zoom(1)));
        assert_eq!(key_action(&KeyCode::Escape), Some(Action::Quit));
        assert_eq!(key_action(&KeyCode::F5), None);
    }

    /// The smoke's screenshot names tag their action moment.
    #[test]
    fn shot_variants_tag_the_action() {
        assert_eq!(shot_variant("/tmp/shot.png", "walk"), "/tmp/shot-walk.png");
        assert_eq!(shot_variant("/tmp/shot.png", "air"), "/tmp/shot-air.png");
        assert_eq!(shot_variant("prefix", "strike"), "prefix-strike");
    }

    /// The follow camera holds the ADR 0006 orientation: unit length,
    /// azimuth 49° off +z toward +x (off the 45° diagonal — the tile
    /// wallpaper breaker), elevation ~15.5°, at a sane range.
    #[test]
    fn follow_camera_holds_the_tactical_orientation() {
        let d = follow_dir();
        assert!(
            (d.length() - 1.0).abs() < 1e-5,
            "not a unit direction: {d:?}"
        );
        let azimuth = d.x.atan2(d.z).to_degrees();
        let elevation = d.y.asin().to_degrees();
        assert!(
            (azimuth - CAM_AZIMUTH_DEG).abs() < 0.01,
            "azimuth drifted: {azimuth}"
        );
        assert!(
            (elevation - CAM_ELEVATION_DEG).abs() < 0.01,
            "elevation drifted: {elevation}"
        );
        assert!(azimuth != 45.0, "back on the 45° diagonal");
        for zoom in [ZOOM_MIN, 1.0, ZOOM_MAX] {
            let focus = Vec3::new(2.0, 1.0, -1.0);
            let cam = focus + d * (FOLLOW_DIST * zoom);
            assert!(cam.y > 1.0, "camera below head height: {cam:?}");
            assert!(
                (cam - focus).length() > 3.0,
                "camera collapsed onto the focus"
            );
        }
    }

    /// Ground picking: a downward ray hits y = 0 at the crossing
    /// point; parallel and upward rays miss.
    #[test]
    fn ground_point_hits_the_plane_and_rejects_strays() {
        let origin = Vec3::new(3.0, 5.0, 2.0);
        let dir = Vec3::new(0.0, -1.0, -0.5).normalize();
        let hit = ground_point(origin, dir).expect("downward ray must hit");
        assert!(hit.y.abs() < 1e-4, "hit off the plane: {hit:?}");
        // The hit lies along the ray, at the y = 0 crossing.
        let t = (hit - origin).dot(dir);
        assert!(t > 0.0);
        assert!((hit - (origin + dir * t)).length() < 1e-4);
        // Parallel to the ground: no hit.
        assert_eq!(ground_point(origin, Vec3::new(1.0, 0.0, 0.0)), None);
        // Pointing up and away: the crossing is behind the origin.
        assert_eq!(ground_point(origin, Vec3::new(0.0, 1.0, 0.0)), None);
    }

    /// Walker picking: the ray takes the figure under it — and when
    /// two figures line up, the nearer one wins.
    #[test]
    fn pick_walker_takes_the_figure_under_the_ray() {
        let skel = bw_core::character::Skeleton::humanoid();
        let mk = |id: u32, pos: BwVec3| Walker {
            id,
            character: Character::new(pos, 0.0),
            input: MovementInput::default(),
            prev: bw_core::character::CharacterPose::new(&skel),
            curr: bw_core::character::CharacterPose::new(&skel),
        };
        let near = mk(1, BwVec3::new(0.0, 0.0, 0.0));
        let far = mk(2, BwVec3::new(0.0, 0.0, -4.0));
        let side = mk(3, BwVec3::new(2.0, 0.0, 1.0));
        // A camera-ish ray straight down onto the near figure, with the
        // far figure directly behind it.
        let origin = Vec3::new(0.0, 8.0, 4.0);
        let dir = (Vec3::new(0.0, 0.5, 0.0) - origin).normalize();
        let hit = pick_walker(
            origin,
            dir,
            [
                (Entity::from_raw_u32(1).unwrap(), &near),
                (Entity::from_raw_u32(2).unwrap(), &far),
                (Entity::from_raw_u32(3).unwrap(), &side),
            ]
            .into_iter(),
        )
        .expect("ray over the near figure must pick it");
        assert_eq!(
            hit,
            Entity::from_raw_u32(1).unwrap(),
            "nearest along the ray wins"
        );
        // A ray into open meadow picks nothing.
        let miss = Vec3::new(0.9, -1.0, 0.2).normalize();
        assert_eq!(
            pick_walker(
                origin,
                miss,
                [(Entity::from_raw_u32(1).unwrap(), &near)].into_iter()
            ),
            None
        );
    }

    /// The scene preset exists and builds walkers (the ambient ring).
    #[test]
    fn scene_presets_build() {
        let Some(preset) = scene_preset("walk") else {
            panic!("walk preset missing");
        };
        let specs = build_scene(&preset, 42);
        assert_eq!(specs.len(), preset.count);
        for spec in &specs {
            assert!(spec.path.radius > 0.0);
        }
    }
}
