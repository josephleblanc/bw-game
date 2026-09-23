//! tree-playground: the live windowed viewer for tuning the golden-ratio
//! tree — the first real consumer of the tick contract
//! ([`bw_core::time`]). Run with `bw-tree-gallery --viewer --scene tree`
//! from a build with `--features viewer`.
//!
//! Everything windowing/render lives behind the dev-only `viewer` cargo
//! feature (see Cargo.toml): the default build — the measured headless
//! gallery — never sees this module, so the dependency graph, wasm gate,
//! and size budgets are untouched (ADR 0002).
//!
//! ## Contract wiring
//!
//! - Sim time advances only in fixed `SIM_DT` steps, in `FixedUpdate`.
//!   Bevy's `Time<Fixed>` accumulator *is* the contract's "interactive
//!   boundary"; the sim never reads the wall clock.
//! - The render mirror runs in `Update` and only *samples*: it
//!   interpolates between the last two sim poses with
//!   `Time<Fixed>::overstep_fraction` as alpha, never advancing sim time.
//!   Render lags the sim by less than one tick — never ahead of it.
//! - Keys map to [`Action`]s through a pure function; applying an action
//!   is a pure state transition on [`Playground`] — the first sliver of
//!   the input→action layer the renderer ADR mentions.
//!
//! This path may allocate (readout strings, sprite respawn on
//! regeneration): ADR 0004 governs the headless sim, not the dev viewer.

use std::collections::BTreeSet;
use std::time::Duration;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::{Indices, Mesh, Mesh2d, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::WindowResolution;

use bw_core::time::SIM_DT;
use bw_core::tree::{Tree, TreeParams, TreePose, WindParams};

use crate::ScenePreset;
use crate::proj::{project_billboard, project_ground, project_shadow, tile_center, tile_diamond};
use crate::tree_seed;

// ---------------------------------------------------------------------------
// Tuning surface
// ---------------------------------------------------------------------------

const WIN_W: f32 = 1280.0;
const WIN_H: f32 = 800.0;
const MARGIN: f32 = 48.0;
/// Vertical window strip reserved for the readout, in pixels.
const READOUT_H: f32 = 108.0;

/// Nudge steps and clamps for the keyboard-tunable parameters.
const SPREAD_STEP: f32 = 0.017_453_3; // 1°
const SPREAD_MIN: f32 = 0.174_533; // 10°
const SPREAD_MAX: f32 = std::f32::consts::FRAC_PI_2; // 90°
const JITTER_STEP: f32 = 0.01;
const JITTER_MAX: f32 = 0.6;
const AMP_STEP: f32 = 0.02;
const AMP_MAX: f32 = 1.2;
const FREQ_STEP: f32 = 0.1;
const FREQ_MIN: f32 = 0.1;
const FREQ_MAX: f32 = 6.0;
const CANOPY_STEP: f32 = 0.1;
const CANOPY_MIN: f32 = 0.2;
const CANOPY_MAX: f32 = 3.0;
/// Viewport zoom multiplier limits (`-`/`=` step by ~1.12x).
const ZOOM_MIN: f32 = 0.3;
const ZOOM_MAX: f32 = 4.0;
const ZOOM_STEP: f32 = 1.12;

/// Palette, mirroring `svg.rs` so both visual surfaces read the same
/// scene. (Kept local rather than shared to guarantee the SVG byte
/// output can never drift from a refactor here.)
const TILE_LIGHT: [f32; 3] = hex03(0x63, 0xa8, 0x4c);
const TILE_DARK: [f32; 3] = hex03(0x4d, 0x8e, 0x3b);
const CANOPY_DARK: [f32; 3] = hex03(0x3e, 0x7c, 0x34);
const CANOPY_LIGHT: [f32; 3] = hex03(0x57, 0xa2, 0x44);
const SHADOW: [f32; 3] = hex03(0x1e, 0x2a, 0x17);
const SHADOW_ALPHA: f32 = 0.16;
const BRANCH_DEEP: [f32; 3] = hex03(0x6b, 0x4a, 0x2f);
const BRANCH_TIP: [f32; 3] = hex03(0x86, 0x70, 0x3d);
const SKY: [f32; 3] = hex03(0x87, 0xc1, 0xe0);

/// `0xRR/255` per channel — sRGB floats from the SVG palette's hex bytes.
const fn hex03(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

fn rgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}

/// Branch stroke color at depth fraction `t` (bark brown → olive), the
/// same lerp `svg.rs` draws.
fn branch_color(t: f32) -> Color {
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    Color::srgb(
        lerp(BRANCH_DEEP[0], BRANCH_TIP[0]),
        lerp(BRANCH_DEEP[1], BRANCH_TIP[1]),
        lerp(BRANCH_DEEP[2], BRANCH_TIP[2]),
    )
}

/// Z layers in the 2d transparent sort (tiles are an opaque mesh at 0).
const Z_SHADOW: f32 = 1.0;
const Z_BRANCH_BASE: f32 = 2.0;
const Z_CANOPY_DARK: f32 = 30.0;
const Z_CANOPY_LIGHT: f32 = 30.5;

// ---------------------------------------------------------------------------
// Input → action (pure)
// ---------------------------------------------------------------------------

/// One playground input. The signed payloads are nudge directions.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Spread(i8),
    Jitter(i8),
    WindAmp(i8),
    WindFreq(i8),
    Canopy(i8),
    /// `+1` zooms out (sees more), `-1` zooms in.
    Zoom(i8),
    Reroll,
    Replay,
    PauseToggle,
    Quit,
}

/// Keyboard → action mapping (pure; Bevy only calls it).
fn key_action(key: &KeyCode) -> Option<Action> {
    use Action as A;
    Some(match key {
        KeyCode::ArrowRight => A::Spread(1),
        KeyCode::ArrowLeft => A::Spread(-1),
        KeyCode::ArrowUp => A::Jitter(1),
        KeyCode::ArrowDown => A::Jitter(-1),
        KeyCode::KeyD => A::WindAmp(1),
        KeyCode::KeyA => A::WindAmp(-1),
        KeyCode::KeyW => A::WindFreq(1),
        KeyCode::KeyS => A::WindFreq(-1),
        KeyCode::KeyE => A::Canopy(1),
        KeyCode::KeyQ => A::Canopy(-1),
        KeyCode::Minus => A::Zoom(1),
        KeyCode::Equal => A::Zoom(-1),
        KeyCode::KeyR => A::Reroll,
        KeyCode::KeyG => A::Replay,
        KeyCode::Space => A::PauseToggle,
        KeyCode::Escape => A::Quit,
        _ => return None,
    })
}

/// What applying an action obliges the caller to do next.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Effect {
    /// Read live at the next tick/frame; nothing else to do.
    Live,
    /// Generation params changed: rebuild trees, then re-pose.
    Regen,
    /// Sim time jumped or paused: copy the current pose over the previous
    /// one so interpolation has nothing left to blend.
    Snap,
    Quit,
}

/// Deterministic seed hop for reroll. Any odd-multiplier step mixes well
/// enough for playground exploration; reroll replay is
/// `(initial seed, number of hops)`.
fn hop_seed(seed: u64) -> u64 {
    seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xD1B5_4A32_D192_ED03)
}

// ---------------------------------------------------------------------------
// Playground state
// ---------------------------------------------------------------------------

/// Crown summary for the (static) ground shadow, in tree-plane coords.
#[derive(Clone, Copy)]
struct Crown {
    cx: f32,
    cy: f32,
    r: f32,
}

#[derive(Resource)]
struct Playground {
    scene: String,
    params: TreeParams,
    wind: WindParams,
    seed: u64,
    anchors: Vec<(i32, i32)>,
    tile_radius: i32,
    /// The interpolation pair: poses at the last two sim ticks.
    prev: Vec<TreePose>,
    curr: Vec<TreePose>,
    /// Scratch the render mirror lerps into (reused, never reallocated
    /// except on regeneration).
    scratch: Vec<TreePose>,
    trees: Vec<Tree>,
    crowns: Vec<Crown>,
    sim_t: f32,
    ticks: u64,
    paused: bool,
    canopy_scale: f32,
    /// Viewport zoom multiplier (applied on top of the fitted base scale).
    zoom: f32,
    /// Set when tree sprites must be despawned/respawned.
    dirty_sprites: bool,
    /// Bumped on every applied action so the readout refreshes.
    edit_count: u32,
}

impl Playground {
    fn new(preset: &ScenePreset, scene: &str, seed: u64) -> Self {
        let mut pg = Self {
            scene: scene.to_string(),
            params: preset.params.clone(),
            wind: preset.wind,
            seed,
            anchors: preset.anchors.to_vec(),
            tile_radius: preset.tile_radius,
            prev: Vec::new(),
            curr: Vec::new(),
            scratch: Vec::new(),
            trees: Vec::new(),
            crowns: Vec::new(),
            sim_t: 0.0,
            ticks: 0,
            paused: false,
            canopy_scale: 1.0,
            zoom: 1.0,
            dirty_sprites: true,
            edit_count: 0,
        };
        pg.rebuild_trees();
        pg
    }

    /// Regenerate every tree (same per-anchor seeds) and rebuild the
    /// pose buffers and crown summaries.
    fn rebuild_trees(&mut self) {
        self.trees = self
            .anchors
            .iter()
            .enumerate()
            .map(|(k, _)| Tree::generate(&self.params, tree_seed(self.seed, k)))
            .collect();
        self.prev = self.trees.iter().map(TreePose::new).collect();
        self.curr = self.trees.iter().map(TreePose::new).collect();
        self.scratch = self.trees.iter().map(TreePose::new).collect();
        self.crowns = self.trees.iter().map(crown_of).collect();
        self.dirty_sprites = true;
    }

    /// Pose both buffers at the current sim time (kills interpolation
    /// across regenerations and sim-time jumps).
    fn repose(&mut self) {
        let Playground {
            trees,
            prev,
            curr,
            wind,
            sim_t,
            ..
        } = self;
        let t = *sim_t;
        for i in 0..trees.len() {
            trees[i].pose_into(&mut curr[i], t, wind);
            copy_pose(&curr[i], &mut prev[i]);
        }
    }

    /// Advance the sim exactly one tick: swap the interpolation pair and
    /// re-pose at `t + SIM_DT`. The `tick_trees` system is this plus the
    /// pause gate; tests call this directly.
    fn step(&mut self) {
        self.sim_t += SIM_DT;
        self.ticks += 1;
        let t = self.sim_t;
        let Playground {
            trees,
            prev,
            curr,
            wind,
            ..
        } = self;
        for i in 0..trees.len() {
            // prev becomes the last tick's curr (swap, no allocation).
            core::mem::swap(&mut prev[i], &mut curr[i]);
            trees[i].pose_into(&mut curr[i], t, wind);
        }
    }

    /// Pure-enough state transition: mutates only this resource.
    fn apply(&mut self, action: Action) -> Effect {
        let dir = |d: i8| d as f32;
        match action {
            Action::Spread(d) => {
                self.params.spread =
                    (self.params.spread + dir(d) * SPREAD_STEP).clamp(SPREAD_MIN, SPREAD_MAX);
                Effect::Regen
            }
            Action::Jitter(d) => {
                self.params.angle_jitter =
                    (self.params.angle_jitter + dir(d) * JITTER_STEP).clamp(0.0, JITTER_MAX);
                Effect::Regen
            }
            Action::WindAmp(d) => {
                self.wind.amp_rad = (self.wind.amp_rad + dir(d) * AMP_STEP).clamp(0.0, AMP_MAX);
                Effect::Live
            }
            Action::WindFreq(d) => {
                self.wind.freq_scale =
                    (self.wind.freq_scale + dir(d) * FREQ_STEP).clamp(FREQ_MIN, FREQ_MAX);
                Effect::Live
            }
            Action::Canopy(d) => {
                self.canopy_scale =
                    (self.canopy_scale + dir(d) * CANOPY_STEP).clamp(CANOPY_MIN, CANOPY_MAX);
                Effect::Live
            }
            Action::Zoom(d) => {
                let f = if d > 0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                self.zoom = (self.zoom * f).clamp(ZOOM_MIN, ZOOM_MAX);
                Effect::Live
            }
            Action::Reroll => {
                self.seed = hop_seed(self.seed);
                self.sim_t = 0.0;
                self.ticks = 0;
                Effect::Regen
            }
            Action::Replay => {
                self.sim_t = 0.0;
                self.ticks = 0;
                Effect::Snap
            }
            Action::PauseToggle => {
                self.paused = !self.paused;
                Effect::Snap
            }
            Action::Quit => Effect::Quit,
        }
    }
}

fn copy_pose(src: &TreePose, dst: &mut TreePose) {
    dst.angle.copy_from_slice(&src.angle);
    dst.start_x.copy_from_slice(&src.start_x);
    dst.start_y.copy_from_slice(&src.start_y);
    dst.tip_x.copy_from_slice(&src.tip_x);
    dst.tip_y.copy_from_slice(&src.tip_y);
    dst.width.copy_from_slice(&src.width);
}

/// `out = prev·(1−α) + curr·α` — endpoint-exact at α = 0 and α = 1 (the
/// `a + (b−a)·α` form is not). Writes into `out` without allocating.
fn lerp_pose(prev: &TreePose, curr: &TreePose, alpha: f32, out: &mut TreePose) {
    let lerp = |p: &[f32], c: &[f32], o: &mut Vec<f32>| {
        for k in 0..p.len() {
            o[k] = p[k] * (1.0 - alpha) + c[k] * alpha;
        }
    };
    lerp(&prev.angle, &curr.angle, &mut out.angle);
    lerp(&prev.start_x, &curr.start_x, &mut out.start_x);
    lerp(&prev.start_y, &curr.start_y, &mut out.start_y);
    lerp(&prev.tip_x, &curr.tip_x, &mut out.tip_x);
    lerp(&prev.tip_y, &curr.tip_y, &mut out.tip_y);
    lerp(&prev.width, &curr.width, &mut out.width);
}

/// Mean and radius of the leaf tips (the crown), for the ground shadow.
fn crown_of(tree: &Tree) -> Crown {
    let (tx, ty) = tree.rest_tips();
    if tx.is_empty() {
        return Crown {
            cx: 0.0,
            cy: 0.0,
            r: 20.0,
        };
    }
    let n = tx.len() as f32;
    let cx = tx.iter().sum::<f32>() / n;
    let cy = ty.iter().sum::<f32>() / n;
    let r = tx
        .iter()
        .zip(ty)
        .map(|(&x, &y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
        .fold(0.0, f32::max)
        + 14.0;
    Crown { cx, cy, r }
}

// ---------------------------------------------------------------------------
// Camera framing (pure)
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct CameraRig {
    /// Projection scale that fits the scene at zoom = 1.
    base_scale: f32,
}

struct Extents {
    minx: f32,
    miny: f32,
    maxx: f32,
    maxy: f32,
}

impl Extents {
    fn new() -> Self {
        Self {
            minx: f32::MAX,
            miny: f32::MAX,
            maxx: f32::MIN,
            maxy: f32::MIN,
        }
    }

    fn grow(&mut self, x: f32, y: f32) {
        self.minx = self.minx.min(x);
        self.miny = self.miny.min(y);
        self.maxx = self.maxx.max(x);
        self.maxy = self.maxy.max(y);
    }

    fn tuple(&self) -> (f32, f32, f32, f32) {
        (self.minx, self.miny, self.maxx, self.maxy)
    }
}

/// Content bounds in projected screen coords: the tile ring, every tree's
/// rest crown (with canopy blob pad), and the cast shadows.
fn scene_bounds(pg: &Playground) -> (f32, f32, f32, f32) {
    let mut b = Extents::new();

    // Tile ring extremes.
    let mut i0 = i32::MAX;
    let mut i1 = i32::MIN;
    let mut j0 = i32::MAX;
    let mut j1 = i32::MIN;
    for &(i, j) in &pg.anchors {
        i0 = i0.min(i - pg.tile_radius);
        i1 = i1.max(i + 1 + pg.tile_radius);
        j0 = j0.min(j - pg.tile_radius);
        j1 = j1.max(j + 1 + pg.tile_radius);
    }
    for (gx, gz) in [
        (i0 as f32, j0 as f32),
        (i1 as f32, j0 as f32),
        (i1 as f32, j1 as f32),
        (i0 as f32, j1 as f32),
    ] {
        let (x, y) = project_ground(gx * crate::proj::TILE, gz * crate::proj::TILE);
        b.grow(x, y);
    }

    // Trees and their shadows.
    const CANOPY_PAD: f32 = 20.0;
    for (k, tree) in pg.trees.iter().enumerate() {
        let (gx, gz) = tile_center(pg.anchors[k].0, pg.anchors[k].1);
        let (x, y) = project_billboard(gx, gz, 0.0, 0.0);
        b.grow(x, y);
        let (tx, ty) = tree.rest_tips();
        for (&x, &y) in tx.iter().zip(ty) {
            for (px, py) in [
                (x - CANOPY_PAD, y),
                (x + CANOPY_PAD, y),
                (x, y - CANOPY_PAD),
                (x, y + CANOPY_PAD),
            ] {
                let (bx, by) = project_billboard(gx, gz, px, py);
                b.grow(bx, by);
            }
            let (sx, sy) = project_shadow(gx, gz, x, y);
            b.grow(sx, sy);
        }
    }
    b.tuple()
}

/// Fit scene bounds into the window: camera center (projected coords)
/// and projection scale (`visible = window / scale`).
fn fit_camera(scene: (f32, f32, f32, f32), win: Vec2, margin: f32, readout_h: f32) -> (Vec2, f32) {
    let (minx, miny, maxx, maxy) = scene;
    let center = Vec2::new((minx + maxx) * 0.5, (miny + maxy) * 0.5);
    let avail = Vec2::new(
        (win.x - 2.0 * margin).max(1.0),
        (win.y - 2.0 * margin - readout_h).max(1.0),
    );
    let extent = Vec2::new((maxx - minx).max(1.0), (maxy - miny).max(1.0));
    let scale = (extent.x / avail.x).max(extent.y / avail.y);
    (center, scale)
}

// ---------------------------------------------------------------------------
// Assets (procedural: no asset files)
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct ViewerAssets {
    /// 1×1 white pixel: every branch rectangle, tinted per sprite.
    white: Handle<Image>,
    /// Soft radial blob: canopy masses and shadows.
    blob: Handle<Image>,
    /// One checkerboard tile's exact projected diamond.
    diamond: Handle<Mesh>,
    tile_light: Handle<ColorMaterial>,
    tile_dark: Handle<ColorMaterial>,
}

fn setup_assets(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let white = images.add(Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[255, 255, 255, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    ));
    let blob = images.add(blob_image(64));
    let diamond = meshes.add(diamond_mesh());
    let tile_light = materials.add(ColorMaterial::from_color(rgb(TILE_LIGHT)));
    let tile_dark = materials.add(ColorMaterial::from_color(rgb(TILE_DARK)));
    commands.insert_resource(ViewerAssets {
        white,
        blob,
        diamond,
        tile_light,
        tile_dark,
    });
}

/// Soft-edged white disc: opaque core, smoothstep falloff over the outer
/// 30% — overlapping canopy blobs blend into a foliage mass.
fn blob_image(size: u32) -> Image {
    let mut data = vec![0u8; (size * size * 4) as usize];
    let r = (size as f32 - 1.0) * 0.5;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - r;
            let dy = y as f32 - r;
            let d = (dx * dx + dy * dy).sqrt() / r;
            let edge = ((1.0 - d) / 0.3).clamp(0.0, 1.0);
            let a = (edge * edge * (3.0 - 2.0 * edge) * 255.0) as u8;
            let o = ((y * size + x) * 4) as usize;
            data[o] = 255;
            data[o + 1] = 255;
            data[o + 2] = 255;
            data[o + 3] = a;
        }
    }
    Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// The checkerboard diamond: sprites cannot draw a sheared square, so
/// tiles are meshes with the exact projected corners.
fn diamond_mesh() -> Mesh {
    let (a, b) = tile_diamond();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[a, 0.0, 0.0], [0.0, b, 0.0], [-a, 0.0, 0.0], [0.0, -b, 0.0]],
    );
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    mesh
}

// ---------------------------------------------------------------------------
// Sprite entities
// ---------------------------------------------------------------------------

#[derive(Component)]
struct BranchSprite {
    tree: u32,
    node: u32,
}

#[derive(Component)]
struct CanopySprite {
    tree: u32,
    node: u32,
    /// Blob radius at canopy scale 1 and full growth.
    base_r: f32,
    /// Rest width of the node: the growth fade denominator.
    full_w: f32,
}

#[derive(Component)]
struct ShadowSprite;

#[derive(Component)]
struct MainCamera;

#[derive(Component)]
struct UiCamera;

#[derive(Component)]
struct Readout;

struct BranchSpec {
    tree: u32,
    node: u32,
    z: f32,
    color: Color,
}

struct CanopySpec {
    tree: u32,
    node: u32,
    z: f32,
    base_r: f32,
    full_w: f32,
    color: Color,
}

struct ShadowSpec {
    x: f32,
    y: f32,
    rot: f32,
    len: f32,
    wid: f32,
}

fn tree_specs(pg: &Playground) -> (Vec<BranchSpec>, Vec<CanopySpec>, Vec<ShadowSpec>) {
    let mut branches = Vec::new();
    let mut canopy = Vec::new();
    let mut shadows = Vec::new();
    for (ti, tree) in pg.trees.iter().enumerate() {
        let shade = tree.depths().iter().copied().max().unwrap_or(1).max(1) as f32;
        let depths = tree.depths();
        let widths = tree.widths();
        for node in 0..tree.node_count() {
            branches.push(BranchSpec {
                tree: ti as u32,
                node: node as u32,
                z: Z_BRANCH_BASE + depths[node] as f32,
                color: branch_color(depths[node] as f32 / shade),
            });
            if tree.leaf_flags()[node] {
                // Same deterministic per-leaf variation as the SVG canopy.
                let vary = 0.8 + 0.4 * ((node % 7) as f32 / 7.0);
                canopy.push(CanopySpec {
                    tree: ti as u32,
                    node: node as u32,
                    z: Z_CANOPY_DARK,
                    base_r: 12.5 * vary,
                    full_w: widths[node],
                    color: Color::srgba(CANOPY_DARK[0], CANOPY_DARK[1], CANOPY_DARK[2], 0.92),
                });
                canopy.push(CanopySpec {
                    tree: ti as u32,
                    node: node as u32,
                    z: Z_CANOPY_LIGHT,
                    base_r: 16.5 * vary,
                    full_w: widths[node],
                    color: Color::srgba(CANOPY_LIGHT[0], CANOPY_LIGHT[1], CANOPY_LIGHT[2], 0.88),
                });
            }
        }
        // One sheared ellipse per tree: trunk base to the crown's shadow.
        let (gx, gz) = tile_center(pg.anchors[ti].0, pg.anchors[ti].1);
        let crown = pg.crowns[ti];
        let (bx, by) = project_shadow(gx, gz, 0.0, 0.0);
        let (cx, cy) = project_shadow(gx, gz, crown.cx, crown.cy);
        let dx = cx - bx;
        let dy = -(cy - by); // bevy y-up
        let axis = (dx * dx + dy * dy).sqrt() + 1.6 * crown.r;
        shadows.push(ShadowSpec {
            x: (bx + cx) * 0.5,
            y: -(by + cy) * 0.5,
            rot: dy.atan2(dx),
            len: axis,
            wid: 1.5 * crown.r,
        });
    }
    (branches, canopy, shadows)
}

fn spawn_tree_sprites(world: &mut World) {
    let (branches, canopy, shadows) = {
        let pg = world.resource::<Playground>();
        tree_specs(pg)
    };
    let (white, blob) = {
        let a = world.resource::<ViewerAssets>();
        (a.white.clone(), a.blob.clone())
    };
    for s in branches {
        world.spawn((
            BranchSprite {
                tree: s.tree,
                node: s.node,
            },
            Sprite {
                image: white.clone(),
                color: s.color,
                custom_size: Some(Vec2::ONE),
                ..default()
            },
            Transform::from_xyz(0.0, 0.0, s.z),
        ));
    }
    for s in canopy {
        world.spawn((
            CanopySprite {
                tree: s.tree,
                node: s.node,
                base_r: s.base_r,
                full_w: s.full_w,
            },
            Sprite {
                image: blob.clone(),
                color: s.color,
                custom_size: Some(Vec2::ZERO),
                ..default()
            },
            Transform::from_xyz(0.0, 0.0, s.z),
        ));
    }
    for s in shadows {
        world.spawn((
            ShadowSprite,
            Sprite {
                image: blob.clone(),
                color: Color::srgba(SHADOW[0], SHADOW[1], SHADOW[2], SHADOW_ALPHA),
                custom_size: Some(Vec2::new(s.len, s.wid)),
                ..default()
            },
            Transform::from_xyz(s.x, s.y, Z_SHADOW).with_rotation(Quat::from_rotation_z(s.rot)),
        ));
    }
}

// ---------------------------------------------------------------------------
// Entry point and systems
// ---------------------------------------------------------------------------

pub fn run(preset: &ScenePreset, scene_id: &str, seed: u64, shot: Option<&str>) {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("bw-tree-gallery — {scene_id} (tree-playground)"),
            resolution: WindowResolution::new(WIN_W as u32, WIN_H as u32),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(rgb(SKY)))
    // The tick contract's interactive boundary: fixed SIM_DT steps.
    .insert_resource(Time::<Fixed>::from_duration(Duration::from_secs_f32(
        SIM_DT,
    )))
    .insert_resource(Playground::new(preset, scene_id, seed))
    .add_systems(Startup, (setup_assets, setup_scene).chain())
    .add_systems(FixedUpdate, tick_trees)
    .add_systems(
        Update,
        (
            handle_input,
            rebuild_sprites,
            render_mirror,
            update_readout,
            auto_screenshot,
        )
            .chain(),
    );
    if let Some(path) = shot {
        app.insert_resource(ShotPlan {
            grow_path: path.to_string(),
            grown_path: grown_variant(path),
        });
    }
    app.run();
}

/// `foo.png` → `foo-grown.png` (the second smoke shot's path).
fn grown_variant(path: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-grown.{ext}"),
        None => format!("{path}-grown"),
    }
}

/// Dev smoke test (`--viewer-shot <path>`): capture the window mid-growth
/// and fully grown, then exit — the viewer's headless-ish verification
/// hook (no GPU-less CI yet, so this runs on a dev machine).
#[derive(Resource)]
struct ShotPlan {
    grow_path: String,
    grown_path: String,
}

/// Sim-time thresholds (seconds) for the smoke shots and exit — sim time,
/// not frames, so the captures are the same on any refresh rate.
const SHOT_GROW_T: f32 = 1.5;
const SHOT_GROWN_T: f32 = 8.0;
const SHOT_EXIT_T: f32 = 10.0;

fn auto_screenshot(
    mut fired: Local<(bool, bool)>,
    plan: Option<Res<ShotPlan>>,
    pg: Res<Playground>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(plan) = plan else { return };
    if !fired.0 && pg.sim_t >= SHOT_GROW_T {
        fired.0 = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(plan.grow_path.clone()));
    } else if !fired.1 && pg.sim_t >= SHOT_GROWN_T {
        fired.1 = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(plan.grown_path.clone()));
    } else if pg.sim_t >= SHOT_EXIT_T {
        exit.write(AppExit::Success);
    }
}

fn setup_scene(mut commands: Commands, pg: Res<Playground>, assets: Res<ViewerAssets>) {
    let bounds = scene_bounds(&pg);
    let (center, base_scale) = fit_camera(bounds, Vec2::new(WIN_W, WIN_H), MARGIN, READOUT_H);
    commands.insert_resource(CameraRig { base_scale });

    // Scene camera: projected coords are y-down, bevy world is y-up.
    commands.spawn((
        Camera2d,
        Transform::from_xyz(center.x, -center.y, 0.0),
        Projection::Orthographic(OrthographicProjection {
            scale: base_scale,
            ..OrthographicProjection::default_2d()
        }),
        MainCamera,
        RenderLayers::layer(0),
    ));
    // Readout overlay camera: fixed 1:1 window pixels, draws over the
    // scene without zooming with it.
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

    // Checkerboard: the union of per-anchor tile rings, FFT checker.
    let mut tiles = BTreeSet::new();
    for &(i, j) in &pg.anchors {
        for di in -pg.tile_radius..=pg.tile_radius {
            for dj in -pg.tile_radius..=pg.tile_radius {
                tiles.insert((i + di, j + dj));
            }
        }
    }
    for (i, j) in tiles {
        let (gx, gz) = tile_center(i, j);
        let (x, y) = project_ground(gx, gz);
        let mat = if (i + j) % 2 == 0 {
            assets.tile_light.clone()
        } else {
            assets.tile_dark.clone()
        };
        commands.spawn((
            Mesh2d(assets.diamond.clone()),
            MeshMaterial2d(mat),
            Transform::from_xyz(x, -y, 0.0),
        ));
    }

    // Readout: center-anchored block, placed by monospace extents
    // (~9 px/char at 15 px, ~21 px/line).
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

/// The sim: advance exactly one SIM_DT step and re-pose every tree.
/// Deterministic from `(scene, seed, ticks)` exactly like the headless
/// gallery — the wall clock never enters.
fn tick_trees(mut pg: ResMut<Playground>) {
    if !pg.paused {
        pg.step();
    }
}

fn handle_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut pg: ResMut<Playground>,
    rig: Res<CameraRig>,
    mut camera: Query<&mut Projection, With<MainCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    let mut zoomed = false;
    for key in keys.get_just_pressed() {
        let Some(action) = key_action(key) else {
            continue;
        };
        let effect = pg.apply(action);
        pg.edit_count += 1;
        match effect {
            Effect::Quit => {
                exit.write(AppExit::Success);
            }
            Effect::Snap => pg.repose(),
            Effect::Regen => {
                pg.rebuild_trees();
                pg.repose();
            }
            Effect::Live => zoomed |= matches!(action, Action::Zoom(_)),
        }
    }
    if zoomed
        && let Ok(mut projection) = camera.single_mut()
        && let Projection::Orthographic(ref mut ortho) = *projection
    {
        ortho.scale = rig.base_scale * pg.zoom;
    }
}

/// Despawn and respawn the tree sprites whenever regeneration changed
/// the structures (also performs the very first spawn).
fn rebuild_sprites(world: &mut World) {
    if !world.resource::<Playground>().dirty_sprites {
        return;
    }
    world.resource_mut::<Playground>().dirty_sprites = false;
    let mut query = world
        .query_filtered::<Entity, Or<(With<BranchSprite>, With<CanopySprite>, With<ShadowSprite>)>>(
        );
    let ids: Vec<Entity> = query.iter(world).collect();
    for id in ids {
        world.despawn(id);
    }
    spawn_tree_sprites(world);
}

/// The render mirror: sample the sim without advancing it. Poses are
/// lerped between the last two ticks at the accumulator's alpha; the
/// projected segment quads, canopy blobs read out per sprite.
fn render_mirror(
    mut pg: ResMut<Playground>,
    fixed: Res<Time<Fixed>>,
    mut branches: Query<(&BranchSprite, &mut Transform, &mut Sprite), Without<CanopySprite>>,
    mut canopies: Query<(&CanopySprite, &mut Transform, &mut Sprite), Without<BranchSprite>>,
) {
    if pg.trees.is_empty() {
        return;
    }
    let alpha = if pg.paused {
        1.0
    } else {
        fixed.overstep_fraction()
    };
    let Playground {
        anchors,
        canopy_scale,
        prev,
        curr,
        scratch,
        ..
    } = &mut *pg;
    for i in 0..scratch.len() {
        lerp_pose(&prev[i], &curr[i], alpha, &mut scratch[i]);
    }

    for (branch, mut transform, mut sprite) in branches.iter_mut() {
        let pose = &scratch[branch.tree as usize];
        let i = branch.node as usize;
        let (gx, gz) = tile_center(
            anchors[branch.tree as usize].0,
            anchors[branch.tree as usize].1,
        );
        let (x0, y0) = project_billboard(gx, gz, pose.start_x[i], pose.start_y[i]);
        let (x1, y1) = project_billboard(gx, gz, pose.tip_x[i], pose.tip_y[i]);
        let dx = x1 - x0;
        let dy = -(y1 - y0); // screen-down → bevy y-up
        let len = (dx * dx + dy * dy).sqrt();
        transform.translation.x = (x0 + x1) * 0.5;
        transform.translation.y = -(y0 + y1) * 0.5;
        transform.rotation = Quat::from_rotation_z(dy.atan2(dx));
        sprite.custom_size = Some(Vec2::new(len, pose.width[i].max(0.0)));
    }

    for (blob, mut transform, mut sprite) in canopies.iter_mut() {
        let pose = &scratch[blob.tree as usize];
        let i = blob.node as usize;
        let (gx, gz) = tile_center(anchors[blob.tree as usize].0, anchors[blob.tree as usize].1);
        let (x, y) = project_billboard(gx, gz, pose.tip_x[i], pose.tip_y[i]);
        // Blobs bloom in with their branch: width grows 0 → full.
        let growth = if blob.full_w > 0.0 {
            (pose.width[i] / blob.full_w).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let d = blob.base_r * *canopy_scale * 2.0 * growth;
        transform.translation.x = x;
        transform.translation.y = -y;
        sprite.custom_size = Some(Vec2::splat(d));
    }
}

fn update_readout(
    mut readout: Query<&mut Text2d, With<Readout>>,
    pg: Res<Playground>,
    fixed: Res<Time<Fixed>>,
    mut last_edit: Local<u32>,
    mut frame: Local<u32>,
) {
    *frame = frame.wrapping_add(1);
    if pg.edit_count == *last_edit && !(*frame).is_multiple_of(6) {
        return;
    }
    *last_edit = pg.edit_count;
    let Ok(mut text) = readout.single_mut() else {
        return;
    };
    text.0 = readout_text(&pg, fixed.overstep_fraction());
}

/// Monospace readout block, every line padded to equal width so the
/// center-anchored text block aligns.
fn readout_text(pg: &Playground, alpha: f32) -> String {
    let state = if pg.paused { "PAUSED" } else { "running" };
    let lines = [
        format!("tree-playground  scene {}  seed {}", pg.scene, pg.seed),
        format!(
            "spread {:.1}deg  fork jitter {:.2}",
            pg.params.spread.to_degrees(),
            pg.params.angle_jitter
        ),
        format!(
            "wind amp {:.2} rad  freq {:.2}  canopy {:.2}x  [{}]",
            pg.wind.amp_rad, pg.wind.freq_scale, pg.canopy_scale, state
        ),
        format!("t {:7.2}s  tick {}  alpha {:.2}", pg.sim_t, pg.ticks, alpha),
        "arrows spread/jitter  A/D wind amp  W/S wind freq  Q/E canopy".to_string(),
        "R reroll  G replay growth  Space pause  -/+ zoom  Esc quit".to_string(),
    ];
    let width = lines.iter().map(String::len).max().unwrap_or(0);
    lines
        .iter()
        .map(|l| format!("{l:<width$}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Tests (run with `cargo test -p bw-tree-gallery --features viewer`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn playground(seed: u64) -> Playground {
        let preset = crate::ScenePreset {
            params: TreeParams {
                trunk_len: 40.0,
                max_depth: 4,
                ..TreeParams::oak()
            },
            wind: WindParams {
                amp_rad: 0.1,
                freq_scale: 1.0,
            },
            anchors: &crate::SINGLE_ANCHOR,
            tile_radius: 1,
        };
        Playground::new(&preset, "test", seed)
    }

    /// The lerp form must hit both endpoints exactly: alpha 0 renders the
    /// previous tick, alpha 1 the current tick, bit for bit.
    #[test]
    fn lerp_pose_hits_endpoints_exactly() {
        let pg = playground(3);
        let tree = &pg.trees[0];
        let mut prev = TreePose::new(tree);
        let mut curr = TreePose::new(tree);
        let mut out = TreePose::new(tree);
        tree.pose_into(&mut prev, 1.0, &pg.wind);
        tree.pose_into(&mut curr, 2.0, &pg.wind);
        lerp_pose(&prev, &curr, 0.0, &mut out);
        assert_eq!(out.checksum(), prev.checksum());
        lerp_pose(&prev, &curr, 1.0, &mut out);
        assert_eq!(out.checksum(), curr.checksum());
        lerp_pose(&prev, &curr, 0.5, &mut out);
        for k in 0..out.tip_x.len() {
            let mid = (prev.tip_x[k] + curr.tip_x[k]) * 0.5;
            assert!((out.tip_x[k] - mid).abs() < 1e-3);
        }
    }

    /// Interpolation never breaks the tree: a child's start stays glued
    /// to its parent's tip at any alpha (both endpoints agree exactly,
    /// so every lerp of them agrees exactly too).
    #[test]
    fn lerp_preserves_connectivity() {
        let chain = |tip: f32| TreePose {
            angle: vec![1.0, 0.6],
            start_x: vec![0.0, tip],
            start_y: vec![0.0, tip],
            tip_x: vec![tip, tip - 0.5],
            tip_y: vec![tip, tip + 0.7],
            width: vec![4.0, 2.0],
        };
        let prev = chain(1.0);
        let curr = chain(2.0);
        let mut out = chain(0.0);
        for k in 0..=8 {
            let alpha = k as f32 / 8.0;
            lerp_pose(&prev, &curr, alpha, &mut out);
            assert_eq!(out.start_x[1], out.tip_x[0], "broken at alpha {alpha}");
            assert_eq!(out.start_y[1], out.tip_y[0], "broken at alpha {alpha}");
        }
    }

    #[test]
    fn key_action_maps_the_playground_keys() {
        assert_eq!(key_action(&KeyCode::ArrowRight), Some(Action::Spread(1)));
        assert_eq!(key_action(&KeyCode::Minus), Some(Action::Zoom(1)));
        assert_eq!(key_action(&KeyCode::KeyR), Some(Action::Reroll));
        assert_eq!(key_action(&KeyCode::Escape), Some(Action::Quit));
        assert_eq!(key_action(&KeyCode::F5), None);
    }

    #[test]
    fn actions_clamp_and_flag_regen() {
        let mut pg = playground(7);
        assert_eq!(pg.apply(Action::Spread(100)), Effect::Regen);
        assert!((pg.params.spread - SPREAD_MAX).abs() < 1e-6);
        assert_eq!(pg.apply(Action::Jitter(-100)), Effect::Regen);
        assert_eq!(pg.params.angle_jitter, 0.0);
        assert_eq!(pg.apply(Action::WindAmp(-100)), Effect::Live);
        assert_eq!(pg.wind.amp_rad, 0.0);
        assert_eq!(pg.apply(Action::WindFreq(100)), Effect::Live);
        assert_eq!(pg.wind.freq_scale, FREQ_MAX);
        assert_eq!(pg.apply(Action::Canopy(-100)), Effect::Live);
        assert_eq!(pg.canopy_scale, CANOPY_MIN);
        // Zoom is multiplicative: many steps in must reach the floor.
        for _ in 0..100 {
            assert_eq!(pg.apply(Action::Zoom(-1)), Effect::Live);
        }
        assert!((pg.zoom - ZOOM_MIN).abs() < 1e-6);
    }

    /// Reroll: new seed, sim restarts, and the seed hop is deterministic.
    #[test]
    fn reroll_restarts_and_hops_deterministically() {
        assert_ne!(hop_seed(42), 42);
        assert_eq!(hop_seed(42), hop_seed(42));
        let mut pg = playground(42);
        pg.sim_t = 9.0;
        pg.ticks = 540;
        assert_eq!(pg.apply(Action::Reroll), Effect::Regen);
        assert_eq!(pg.seed, hop_seed(42));
        assert_eq!(pg.sim_t, 0.0);
        assert_eq!(pg.ticks, 0);
        // Replay keeps the seed but restarts time.
        assert_eq!(pg.apply(Action::Replay), Effect::Snap);
        assert_eq!(pg.seed, hop_seed(42));
        assert_eq!(pg.sim_t, 0.0);
    }

    /// Repose freezes interpolation: prev and curr agree exactly.
    #[test]
    fn repose_makes_prev_and_curr_agree() {
        let mut pg = playground(11);
        pg.sim_t = 3.0;
        pg.repose();
        assert_eq!(pg.prev[0].checksum(), pg.curr[0].checksum());
        // Determinism: the same tick count from replay reaches the same
        // pose (the wall clock is not involved).
        let mut a = playground(11);
        for _ in 0..30 {
            a.step();
        }
        let mut b = playground(11);
        for _ in 0..30 {
            b.step();
        }
        assert_eq!(a.curr[0].checksum(), b.curr[0].checksum());
        // ~30 ticks at 60 Hz = half a second (f32 accumulation tolerance).
        assert!((a.sim_t - 0.5).abs() < 1e-3);
    }

    #[test]
    fn fit_camera_frames_the_scene() {
        let (center, scale) = fit_camera(
            (-100.0, -50.0, 100.0, 50.0),
            Vec2::new(400.0, 200.0),
            0.0,
            0.0,
        );
        assert_eq!(center, Vec2::ZERO);
        assert!((scale - 0.5).abs() < 1e-6);
        // The readout strip shrinks the available height (floored at 1 px).
        let (c2, s2) = fit_camera(
            (-10.0, -10.0, 10.0, 10.0),
            Vec2::new(400.0, 200.0),
            8.0,
            184.0,
        );
        assert_eq!(c2, Vec2::ZERO);
        assert!((s2 - 20.0).abs() < 1e-6, "avail height floors at 1: {s2}");
    }

    /// Scene bounds cover every tree's crown in projection space.
    #[test]
    fn scene_bounds_contain_the_tree() {
        let pg = playground(5);
        let (minx, miny, maxx, maxy) = scene_bounds(&pg);
        let (gx, gz) = tile_center(0, 0);
        let (tx, ty) = pg.trees[0].rest_tips();
        for (&x, &y) in tx.iter().zip(ty) {
            let (sx, sy) = project_billboard(gx, gz, x, y);
            assert!(sx >= minx && sx <= maxx && sy >= miny && sy <= maxy);
        }
    }
}
