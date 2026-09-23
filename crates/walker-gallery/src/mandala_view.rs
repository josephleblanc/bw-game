//! mandala-view: the live stress scene for the character gallery — 100,
//! 1,000, or 10,000 stick figures on interlocking loops
//! (`crate::mandala` computes the choreography; see its module docs).
//! Run with `bw-walker-gallery --viewer --scene mandala-1k` from a
//! build with `--features viewer`.
//!
//! Same contract wiring as walker-playground: the sim steps only in
//! `FixedUpdate` at `SIM_DT`, this module only samples
//! (`MovementAlpha` from the accumulator; meshes update
//! `.after(MovementMirror)`), and the orbit camera alone uses wall time
//! (the interactive boundary). This path may allocate — ADR 0004
//! governs the headless sim, not the dev viewer.
//!
//! ## The painting (colors are frozen per figure)
//!
//! Each figure's color is [`design`] sampled once at its t = 0 tile —
//! a procedural painting on the canvas: a blue circle, a red circle, a
//! rotated purple square, an orange square, and a green ring on a dark
//! ink field. At t = 0 the crowd reads as colored tiles forming that
//! image; as the loops carry the figures across each other's
//! territory, the shapes dissolve (a "blue circle" is just a set of
//! blue tiles, now scattered along many different loops), and because
//! every loop_spec shares one period, the painting snaps back into focus at
//! each multiple of `T₀`. No color ever changes — only the arrangement.
//!
//! Rendering stays trivially batched: the palette is exactly the
//! painting's colors (6 shared materials), assigned by handle at spawn
//! and never touched again.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::time::Duration;

use bevy::camera::PerspectiveProjection;
use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::DirectionalLight;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::WindowResolution;

use bw_core::character::{Skeleton, bone, circle_input};
use bw_core::math::Vec3 as BwVec3;
use bw_core::time::SIM_DT;

use bw_walker_gallery::{
    BoneSegment, CharacterMovementPlugin, MovementAlpha, MovementMirror, MovementPause, Walker,
    WalkerSkeleton, spawn_walker,
};

use crate::SpawnSpec;
use crate::mandala::{self, MandalaSpec};

const WIN_W: f32 = 1280.0;
const WIN_H: f32 = 800.0;

// ---------------------------------------------------------------------------
// The painting (pure)
// ---------------------------------------------------------------------------

/// Palette slots — the painting's full color set. Indices are the
/// [`design`] function's return values; the background ink is 0.
const INK: u8 = 0;
const BLUE: u8 = 1;
const RED: u8 = 2;
const PURPLE: u8 = 3;
const ORANGE: u8 = 4;
const GREEN: u8 = 5;

/// (hue°, saturation, value) per slot: dark ink field, then the
/// painting's shapes.
const PAINT: [(f32, f32, f32); 6] = [
    (222.0, 0.28, 0.26), // ink field
    (212.0, 0.88, 0.62), // blue circle
    (4.0, 0.82, 0.60),   // red circle
    (278.0, 0.58, 0.58), // purple square
    (30.0, 0.92, 0.66),  // orange square
    (140.0, 0.72, 0.52), // green ring
];

/// Point-in-painting at `(x, z)`, first match wins. Shapes are
/// parameterized by the canvas radius so the composition scales with
/// the population. This is sampled once per figure at spawn.
fn design(x: f32, z: f32, canvas: f32) -> u8 {
    let c = canvas;
    // Blue circle, upper left.
    if dist(x, z, -0.45 * c, -0.30 * c) < 0.28 * c {
        return BLUE;
    }
    // Red circle, lower right.
    if dist(x, z, 0.50 * c, 0.35 * c) < 0.22 * c {
        return RED;
    }
    // Purple square, rotated 30°.
    if in_square(x, z, 0.38 * c, -0.42 * c, 0.17 * c, 30.0) {
        return PURPLE;
    }
    // Orange square, axis-aligned.
    if in_square(x, z, -0.42 * c, 0.48 * c, 0.15 * c, 0.0) {
        return ORANGE;
    }
    // Green ring around the whole composition.
    if (dist(x, z, 0.0, 0.0) - 0.64 * c).abs() < 0.045 * c {
        return GREEN;
    }
    INK
}

fn dist(x: f32, z: f32, cx: f32, cz: f32) -> f32 {
    ((x - cx) * (x - cx) + (z - cz) * (z - cz)).sqrt()
}

/// Axis-aligned or rotated square membership (center, half-extent,
/// rotation in degrees).
fn in_square(x: f32, z: f32, cx: f32, cz: f32, half: f32, rot_deg: f32) -> bool {
    let (dx, dz) = (x - cx, z - cz);
    if rot_deg == 0.0 {
        return dx.abs() < half && dz.abs() < half;
    }
    let a = rot_deg.to_radians();
    let (s, co) = a.sin_cos();
    // Rotate the offset INTO the square's frame.
    let u = dx * co + dz * s;
    let v = -dx * s + dz * co;
    u.abs() < half && v.abs() < half
}

// ---------------------------------------------------------------------------
// Viewer state
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct MandalaView {
    scene: String,
    spec: MandalaSpec,
    /// Figures currently in the world.
    count: usize,
    /// Requested population (keys 1/2/3); respawn when it differs.
    pending_count: usize,
    /// Population seed (the layout's jitter and phases).
    seed: u64,
    zoom: f32,
    /// Orbit azimuth (rad) — camera-side state on the wall clock.
    azimuth: f32,
    /// Bumped on every applied action so the readout refreshes.
    edit_count: u32,
}

#[derive(Resource)]
struct Palette {
    materials: Vec<Handle<StandardMaterial>>,
}

/// The 13 shared capsule meshes + head ball (scale 1 figures only).
struct SharedRigMeshes {
    capsules: Vec<Handle<bevy::mesh::Mesh>>,
    head: Handle<bevy::mesh::Mesh>,
}

/// Static color marker on a mesh entity — the material is assigned at
/// spawn from the figure's t = 0 tile and never changes.
#[derive(Component)]
struct Tinted;

#[derive(Component)]
struct MdlBone {
    seg: Entity,
}

#[derive(Component)]
struct MdlHead {
    seg: Entity,
}

#[derive(Component)]
struct MainCamera;

#[derive(Component)]
struct UiCamera;

#[derive(Component)]
struct Readout;

// ---------------------------------------------------------------------------
// Input → action (pure)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Population(usize),
    PauseToggle,
    Zoom(i8),
    Quit,
}

fn key_action(key: &KeyCode) -> Option<Action> {
    Some(match key {
        KeyCode::Digit1 | KeyCode::Numpad1 => Action::Population(100),
        KeyCode::Digit2 | KeyCode::Numpad2 => Action::Population(1_000),
        KeyCode::Digit3 | KeyCode::Numpad3 => Action::Population(10_000),
        KeyCode::Space => Action::PauseToggle,
        KeyCode::Minus => Action::Zoom(1),
        KeyCode::Equal => Action::Zoom(-1),
        KeyCode::Escape => Action::Quit,
        _ => return None,
    })
}

const ZOOM_MIN: f32 = 0.35;
const ZOOM_MAX: f32 = 3.0;
const ZOOM_STEP: f32 = 1.12;
/// One orbit per ~95 s of wall time.
const ORBIT_RATE: f32 = TAU / 95.0;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(scene_id: &str, count: usize, seed: u64, shot: Option<&str>) {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("bw-walker-gallery — {scene_id} (mandala-view)"),
            resolution: WindowResolution::new(WIN_W as u32, WIN_H as u32),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.10)))
    .insert_resource(Time::<Fixed>::from_duration(Duration::from_secs_f32(
        SIM_DT,
    )))
    .add_plugins(CharacterMovementPlugin)
    .insert_resource(WalkerSkeleton(Skeleton::humanoid()));

    let spec = mandala::build(count, seed);
    let palette = {
        let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
        Palette {
            materials: PAINT
                .iter()
                .map(|&(h, s, v)| {
                    materials.add(StandardMaterial {
                        base_color: Color::hsv(h, s, v),
                        ..default()
                    })
                })
                .collect(),
        }
    };
    app.insert_resource(palette)
        .insert_resource(MandalaView {
            scene: scene_id.to_string(),
            spec,
            count: 0,
            pending_count: count,
            seed,
            zoom: 1.0,
            azimuth: 0.0,
            edit_count: 0,
        })
        .add_systems(Startup, setup_scene)
        // The exclusive respawn system runs first, unordered-relative
        // only to the engine's own machinery.
        .add_systems(
            Update,
            respawn_pending.before(handle_input).after(MovementMirror),
        )
        .add_systems(
            Update,
            (
                handle_input,
                update_movement_alpha,
                rig_follow,
                orbit_camera,
                update_readout,
                auto_screenshot,
            )
                .chain()
                .after(MovementMirror),
        );
    if let Some(path) = shot {
        SHOT_PLAN_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        app.insert_resource(ShotPlan {
            path: path.to_string(),
        });
    }
    app.run();
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<bevy::mesh::Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        Tonemapping::None,
        Projection::Perspective(PerspectiveProjection {
            fov: 40.0_f32.to_radians(),
            ..default()
        }),
        Transform::from_xyz(20.0, 12.0, 20.0).looking_at(Vec3::Y, Vec3::Y),
        MainCamera,
        RenderLayers::layer(0),
    ));
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
    commands.spawn(AmbientLight {
        color: Color::WHITE,
        brightness: 320.0,
        ..default()
    });
    commands.spawn(DirectionalLight {
        illuminance: 6_000.0,
        ..default()
    });

    // The plate: one deep disc under the canvas (sized generously; the
    // orbit camera never lets the edge approach the view plane).
    let ground = meshes.add(
        bevy::mesh::Mesh::from(Circle::new(700.0))
            .rotated_by(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
    );
    let ground_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.07, 0.08, 0.12),
        ..default()
    });
    commands.spawn((
        Mesh3d(ground),
        MeshMaterial3d(ground_mat),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    commands.spawn((
        Text2d::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgba(0.85, 0.88, 0.95, 0.92)),
        Transform::from_xyz(-WIN_W / 2.0 + 284.0, WIN_H / 2.0 - 84.0, 0.0),
        Readout,
        RenderLayers::layer(1),
    ));
}

// ---------------------------------------------------------------------------
// Population spawn / respawn
// ---------------------------------------------------------------------------

fn shared_rig_meshes(world: &mut World) -> SharedRigMeshes {
    let skeleton = world.resource::<WalkerSkeleton>().0.clone();
    let mut assets = world.resource_mut::<Assets<bevy::mesh::Mesh>>();
    let mut capsules = Vec::with_capacity(skeleton.bone_count());
    for b in 0..skeleton.bone_count() {
        let length = skeleton.length(b);
        let width = skeleton.width(b);
        capsules.push(assets.add(bevy::mesh::Mesh::from(Capsule3d::new(
            (width * 0.5).max(0.008),
            (length - width).max(0.001),
        ))));
    }
    let head = assets.add(bevy::mesh::Mesh::from(Sphere::new(
        bw_core::character::HEAD_RADIUS,
    )));
    SharedRigMeshes { capsules, head }
}

/// Spawn the whole population: walkers, bone meshes with the painting's
/// colors (sampled at each figure's t = 0 tile, assigned once).
fn spawn_population(
    world: &mut World,
    spec: &MandalaSpec,
    shared: &SharedRigMeshes,
    palette: &Palette,
) {
    let skeleton = world.resource::<WalkerSkeleton>().0.clone();
    let mut walkers: Vec<Entity> = Vec::with_capacity(spec.walker_total());
    for loop_spec in &spec.loops {
        for j in 0..loop_spec.walkers {
            let SpawnSpec { character, path } =
                mandala::figure(loop_spec, j, spec.radius, spec.speed);
            let input = circle_input(path.radius, path.speed, path.dir);
            let entity = spawn_walker(world, &skeleton, character, input);
            world.entity_mut(entity).insert(path);
            walkers.push(entity);
        }
    }

    // Walker → bone segments, in one pass (setup-window cost).
    let mut by_walker: HashMap<Entity, (Vec<(u16, Entity)>, u8)> =
        HashMap::with_capacity(walkers.len());
    {
        let mut query = world.query::<(Entity, &BoneSegment)>();
        for (entity, seg) in query.iter(world) {
            by_walker
                .entry(seg.walker)
                .or_default()
                .0
                .push((seg.bone, entity));
        }
    }
    // The tiles' colors: sampled at the t = 0 positions, frozen.
    for walker in &walkers {
        let slot = world.get::<Walker>(*walker).map_or(INK, |w| {
            design(w.character.pos.x, w.character.pos.z, spec.canvas)
        });
        if let Some(entry) = by_walker.get_mut(walker) {
            entry.1 = slot;
        }
    }

    for (walker, (bones, slot)) in by_walker {
        let _ = walker;
        let material = MeshMaterial3d(palette.materials[slot as usize].clone());
        for (b, seg) in bones {
            if b as usize == bone::HEAD {
                world.spawn((
                    MdlHead { seg },
                    Mesh3d(shared.head.clone()),
                    material.clone(),
                    Transform::default(),
                    Tinted,
                ));
            } else {
                world.spawn((
                    MdlBone { seg },
                    Mesh3d(shared.capsules[b as usize].clone()),
                    material.clone(),
                    Transform::default(),
                    Tinted,
                ));
            }
        }
    }
}

/// Despawn every population entity (walkers, segments, meshes), leaving
/// the scene scaffolding (cameras, ground, readout) alone.
fn despawn_population(world: &mut World) {
    macro_rules! despawn_all {
        ($filter:ty) => {{
            let mut query = world.query_filtered::<Entity, $filter>();
            let entities: Vec<Entity> = query.iter(world).collect();
            for entity in entities {
                world.despawn(entity);
            }
        }};
    }
    despawn_all!(With<MdlBone>);
    despawn_all!(With<MdlHead>);
    despawn_all!(With<BoneSegment>);
    despawn_all!(With<Walker>);
}

/// Apply a population switch (keys 1/2/3): despawn, rebuild the field
/// for the new count, respawn. The hitch is the stress demo — watch it
/// churn, then watch the readout's entity counts settle.
fn respawn_pending(world: &mut World) {
    let (pending, count, seed) = {
        let view = world.resource::<MandalaView>();
        (view.pending_count, view.count, view.seed)
    };
    if pending == count {
        return;
    }
    despawn_population(world);
    let spec = mandala::build(pending, seed);
    let shared = shared_rig_meshes(world);
    let palette = world.resource::<Palette>().materials.clone();
    spawn_population(world, &spec, &shared, &Palette { materials: palette });
    let mut view = world.resource_mut::<MandalaView>();
    view.spec = spec;
    view.count = pending;
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

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
    mut view: ResMut<MandalaView>,
    mut pause: ResMut<MovementPause>,
    mut exit: MessageWriter<AppExit>,
) {
    if SHOT_PLAN_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        return; // the smoke script owns the session
    }
    for key in keys.get_just_pressed() {
        if let Some(action) = key_action(key) {
            view.edit_count += 1;
            match action {
                Action::Population(count) => view.pending_count = count,
                Action::PauseToggle => pause.0 = !pause.0,
                Action::Zoom(d) => {
                    let f = if d > 0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                    view.zoom = (view.zoom * f).clamp(ZOOM_MIN, ZOOM_MAX);
                }
                Action::Quit => {
                    exit.write(AppExit::Success);
                }
            }
        }
    }
}

/// Keep the meshes on the mirror: capsules between bone endpoints, head
/// at the HEAD tip. Joined queries, no per-entity lookups — this runs
/// for 10,000 × 14 meshes. The `Without` filters keep the three
/// `Transform` writers disjoint (bones, heads, camera). (Query type
/// aliases in param position break `IntoSystem`'s HRTB elaboration
/// here, so the disjoint filters stay inline.)
#[allow(
    clippy::type_complexity,
    reason = "disjoint Transform writers need two Without filters each"
)]
fn rig_follow(
    mut bones: Query<(&MdlBone, &mut Transform), (Without<MdlHead>, Without<MainCamera>)>,
    mut heads: Query<(&MdlHead, &mut Transform), (Without<MdlBone>, Without<MainCamera>)>,
    segments: Query<&BoneSegment>,
) {
    for (bone, mut transform) in &mut bones {
        let Ok(seg) = segments.get(bone.seg) else {
            continue;
        };
        let a = to_bevy(seg.a);
        let b = to_bevy(seg.b);
        let dir = b - a;
        if dir.length_squared() < 1e-9 {
            continue;
        }
        transform.translation = (a + b) * 0.5;
        transform.rotation = Quat::from_rotation_arc(Vec3::Y, dir.normalize());
    }
    for (head, mut transform) in &mut heads {
        let Ok(seg) = segments.get(head.seg) else {
            continue;
        };
        transform.translation = to_bevy(seg.b);
    }
}

fn to_bevy(v: BwVec3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

/// Slow orbit high over the canvas — the only wall-clock reader in this
/// module (the interactive boundary; the sim never sees it). Opens
/// steep enough that the t = 0 painting reads as an image.
fn orbit_camera(
    mut view: ResMut<MandalaView>,
    time: Res<Time>,
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    let Ok(mut transform) = camera.single_mut() else {
        return;
    };
    view.azimuth = (view.azimuth + ORBIT_RATE * time.delta_secs()) % TAU;
    let canvas = view.spec.canvas;
    let horizontal = canvas * 1.05 * view.zoom;
    let height = canvas * 1.55 * view.zoom;
    transform.translation = Vec3::new(
        view.azimuth.cos() * horizontal,
        height,
        view.azimuth.sin() * horizontal,
    );
    transform.look_at(Vec3::ZERO, Vec3::Y);
}

fn update_readout(
    mut readout: Query<&mut Text2d, With<Readout>>,
    view: Res<MandalaView>,
    pause: Res<MovementPause>,
    walkers: Query<&Walker>,
    mut last_edit: Local<u32>,
    mut frame: Local<u32>,
) {
    *frame = frame.wrapping_add(1);
    if view.edit_count == *last_edit && !(*frame).is_multiple_of(6) {
        return;
    }
    *last_edit = view.edit_count;
    let Ok(mut text) = readout.single_mut() else {
        return;
    };
    let Some(walker) = walkers.iter().next() else {
        return;
    };
    let t = walker.character.t;
    let reforms_in = view.spec.period - (t % view.spec.period);
    let segments = view.count * bw_core::character::BONE_COUNT;
    let lines = [
        format!(
            "mandala-view  scene {}  {} figures  {} loops",
            view.scene,
            view.count,
            view.spec.loops.len()
        ),
        format!(
            "t {:7.2}s  tick {:6}  [{}]  painting re-forms in {:6.1}s",
            t,
            (t / SIM_DT) as u64,
            if pause.0 { "PAUSED" } else { "running" },
            reforms_in
        ),
        format!(
            "{} bone segments  {} draw entities  speed {:.2} m/s  seed {}",
            segments,
            view.count * (bw_core::character::BONE_COUNT + 1),
            view.spec.speed,
            view.seed
        ),
        "1/2/3 population 100/1k/10k  -/+ zoom  Space pause  Esc quit".to_string(),
        "colors are frozen: tiles of a painting that walks".to_string(),
    ];
    let width = lines.iter().map(String::len).max().unwrap_or(0);
    text.0 = lines
        .iter()
        .map(|l| format!("{l:<width$}"))
        .collect::<Vec<_>>()
        .join("\n");
}

// ---------------------------------------------------------------------------
// Dev smoke: two screenshots + exit
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct ShotPlan {
    path: String,
}

static SHOT_PLAN_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// With `--viewer-shot`: capture the painting just after launch (the
/// image reads as tiles), then the dispersed field partway through the
/// period, then exit. Sim-time driven.
fn auto_screenshot(
    plan: Option<Res<ShotPlan>>,
    view: Res<MandalaView>,
    walkers: Query<&Walker>,
    mut fired: Local<(bool, bool)>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(plan) = plan else { return };
    let Some(walker) = walkers.iter().next() else {
        return;
    };
    let t = walker.character.t;
    let (t_image, t_dispersed) = shot_times(view.spec.period);
    if !fired.0 && t >= t_image {
        fired.0 = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(variant(&plan.path, "image")));
    } else if !fired.1 && t >= t_dispersed {
        fired.1 = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(variant(&plan.path, "dispersed")));
    } else if t >= t_dispersed + 2.5 {
        exit.write(AppExit::Success);
    }
}

/// The smoke's two capture times: the image early (the launch easing
/// holds the tiles nearly still), dispersed at ~40% of the period.
fn shot_times(period: f32) -> (f32, f32) {
    (0.6, (period * 0.4).max(4.0))
}

/// `foo.png` → `foo-image.png` / `foo-dispersed.png`.
fn variant(path: &str, tag: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-{tag}.{ext}"),
        None => format!("{path}-{tag}"),
    }
}

// ---------------------------------------------------------------------------
// Tests (cargo test -p bw-walker-gallery --features viewer)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_action_maps_the_mandala_keys() {
        assert_eq!(key_action(&KeyCode::Digit1), Some(Action::Population(100)));
        assert_eq!(
            key_action(&KeyCode::Digit3),
            Some(Action::Population(10_000))
        );
        assert_eq!(
            key_action(&KeyCode::Numpad2),
            Some(Action::Population(1_000))
        );
        assert_eq!(key_action(&KeyCode::Space), Some(Action::PauseToggle));
        assert_eq!(key_action(&KeyCode::Minus), Some(Action::Zoom(1)));
        assert_eq!(key_action(&KeyCode::Escape), Some(Action::Quit));
        assert_eq!(key_action(&KeyCode::KeyW), None);
    }

    /// The painting's shapes are where the design says they are: shape
    /// centers sample their own color, points between shapes sample the
    /// ink field, and the ring reads on its arc.
    #[test]
    fn design_paints_the_declared_shapes() {
        let c = 10.0;
        assert_eq!(design(-0.45 * c, -0.30 * c, c), BLUE);
        assert_eq!(design(0.50 * c, 0.35 * c, c), RED);
        assert_eq!(design(0.38 * c, -0.42 * c, c), PURPLE);
        assert_eq!(design(-0.42 * c, 0.48 * c, c), ORANGE);
        // Ring: on-arc green, just inside/outside ink.
        assert_eq!(design(0.64 * c, 0.0, c), GREEN);
        assert_eq!(design(0.0, -0.64 * c, c), GREEN);
        assert_eq!(design(0.5 * c, 0.0, c), INK);
        assert_eq!(design(0.78 * c, 0.0, c), INK);
        // Empty middle (between all shapes) is ink.
        assert_eq!(design(0.02 * c, 0.1 * c, c), INK);
    }

    /// Rotated-square membership is a true rotation, not a bounding
    /// box: a diagonal point the 45° square reaches but the
    /// axis-aligned one doesn't, and vice versa.
    #[test]
    fn rotated_square_rotates() {
        let (cx, cz, half) = (0.0f32, 0.0f32, 1.0f32);
        // (1.2, 0): outside axis-aligned, inside the 45° square.
        assert!(!in_square(half * 1.2, 0.0, cx, cz, half, 0.0));
        assert!(in_square(half * 1.2, 0.0, cx, cz, half, 45.0));
        // (0.95, 0.55): inside axis-aligned, outside the 45° square.
        assert!(in_square(half * 0.95, half * 0.55, cx, cz, half, 0.0));
        assert!(!in_square(half * 0.95, half * 0.55, cx, cz, half, 45.0));
    }

    /// Every tile color comes from the palette, and the field's t = 0
    /// tiles actually draw every shape (a shape with no tiles would be
    /// invisible — the painting must cover its own canvas).
    #[test]
    fn the_field_tiles_every_shape() {
        for &count in &[100usize, 1_000, 10_000] {
            let m = crate::mandala::build(count, 42);
            let mut seen = [false; PAINT.len()];
            for spec in crate::mandala::spawn_specs(&m) {
                let p = spec.character.pos;
                let slot = design(p.x, p.z, m.canvas) as usize;
                assert!(slot < PAINT.len());
                seen[slot] = true;
            }
            for (slot, &painted) in seen.iter().enumerate() {
                assert!(
                    painted,
                    "count {count}: palette slot {slot} has no tiles — a shape is invisible"
                );
            }
        }
    }

    /// Smoke timing: the image shot is early; the dispersed shot sits
    /// at ~40% of the period (never less than the launch transient).
    #[test]
    fn shot_times_follow_the_period() {
        assert_eq!(shot_times(9.0), (0.6, 4.0));
        assert_eq!(shot_times(25.0), (0.6, 10.0));
    }

    #[test]
    fn variants_tag_the_shot() {
        assert_eq!(variant("/tmp/shot.png", "image"), "/tmp/shot-image.png");
        assert_eq!(variant("prefix", "dispersed"), "prefix-dispersed");
    }
}
