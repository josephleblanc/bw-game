//! Animated-SVG gallery output: the interim visual surface for galleries
//! while the real renderer stays deferred (ADR 0002). Zero new
//! dependencies — an SVG document is assembled with plain string writing,
//! and animation comes free from SMIL `<animate>` morphing of path data,
//! so one self-contained file plays the growth + sway loop in any browser.
//!
//! Scene -> screen uses the tactical-RPG projection from `proj`: diamond
//! checkerboard ground, upright tree billboards, sheared ground shadows.
//! This path allocates freely; it is not the measured hot loop (the
//! headless harness is), so ADR 0004's allocate-once rule does not apply
//! here — only to `Tree::pose_into`, which this module calls once per
//! tree per sampled frame.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use bw_core::tree::{Tree, TreePose, WindParams};

use crate::proj::{
    PITCH_DEG, TILE, YAW_DEG, project_billboard, project_ground, project_shadow, tile_center,
};

/// Seconds of pure sway appended after growth completes in the animation.
const SWAY_HOLD: f32 = 3.5;
/// Stroke widths (screen units) of the two canopy layers.
const CANOPY_DARK_W: f32 = 26.0;
const CANOPY_LIGHT_W: f32 = 13.0;
/// Base canopy stroke length per leaf (drawn from the tip inward); each
/// leaf scales it deterministically so the two layers interleave instead
/// of reading as concentric rings.
const CANOPY_STROKE_LEN: f32 = 11.0;
const MARGIN: f32 = 40.0;
const CAPTION_PT: f32 = 15.0;
/// Checkerboard tile fills (FFT-ish saturated greens).
const TILE_LIGHT: &str = "#63a84c";
const TILE_DARK: &str = "#4d8e3b";

/// Provenance caption carried into the rendered file.
pub struct RenderMeta<'a> {
    pub scene: &'a str,
    pub seed: u64,
    pub fps: u32,
}

/// Render the full animation (growth, then sway, then loop) as one
/// self-contained SMIL-animated SVG file. `poses` are scratch buffers
/// reused while sampling.
pub fn render_animation(
    trees: &[Tree],
    poses: &mut [TreePose],
    anchors: &[(i32, i32)],
    wind: &WindParams,
    tile_radius: i32,
    meta: &RenderMeta,
    path: &str,
) -> Result<(), String> {
    let growth_end = trees.iter().map(Tree::growth_end).fold(0.0, f32::max);
    let dur = growth_end + SWAY_HOLD;
    let frames = (((dur * meta.fps as f32).ceil() as usize) + 1).max(2);
    let fps = meta.fps as f32;
    let scene = collect(trees, poses, anchors, wind, tile_radius, frames, |k| {
        (k as f32 / fps).min(dur)
    })?;
    let svg = emit(scene, meta, Some(dur));
    std::fs::write(path, svg).map_err(|e| format!("failed to write {path}: {e}"))
}

/// Render one static frame at scene time `t` — the screenshot-friendly
/// companion to the animation.
#[allow(clippy::too_many_arguments)] // render-kit signature; not API surface
pub fn render_frame(
    trees: &[Tree],
    poses: &mut [TreePose],
    anchors: &[(i32, i32)],
    wind: &WindParams,
    tile_radius: i32,
    meta: &RenderMeta,
    t: f32,
    path: &str,
) -> Result<(), String> {
    let scene = collect(trees, poses, anchors, wind, tile_radius, 1, |_| t)?;
    let svg = emit(scene, meta, None);
    std::fs::write(path, svg).map_err(|e| format!("failed to write {path}: {e}"))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    Shadow,
    Branch,
    CanopyDark,
    CanopyLight,
}

/// One path element: fixed styling plus one path-data string per frame.
struct Element {
    group: Group,
    stroke: String,
    stroke_width: f32,
    opacity: f32,
    frames: Vec<String>,
}

/// Where each tree's elements live in the element vector.
struct TreeSlots {
    /// Per non-empty depth: (shadow index, branch index).
    limbs: Vec<(u8, usize, usize)>,
    canopy_dark: usize,
    canopy_light: usize,
    /// Node indices per non-empty depth, parallel to `limbs`.
    nodes_by_depth: Vec<Vec<u32>>,
    leaves: Vec<u32>,
}

struct Scene {
    elements: Vec<Element>,
    tiles: BTreeSet<(i32, i32)>,
    bounds: (f32, f32, f32, f32), // minx, miny, maxx, maxy
}

fn collect(
    trees: &[Tree],
    poses: &mut [TreePose],
    anchors: &[(i32, i32)],
    wind: &WindParams,
    tile_radius: i32,
    frames: usize,
    t_of: impl Fn(usize) -> f32,
) -> Result<Scene, String> {
    if trees.len() != poses.len() || trees.len() != anchors.len() {
        return Err("trees, poses, and anchors must be parallel arrays".to_string());
    }
    if trees.is_empty() {
        return Err("no trees to render".to_string());
    }

    // Group each tree's nodes by depth once; create the element vector and
    // remember each tree's slots in it.
    let mut elements = Vec::new();
    let mut slots = Vec::new();
    for tree in trees {
        let max_depth = tree.depths().iter().copied().max().unwrap_or(0);
        let mut by_depth: Vec<Vec<u32>> = vec![Vec::new(); max_depth as usize + 1];
        let mut sum_w = vec![0.0f32; max_depth as usize + 1];
        for (i, &d) in tree.depths().iter().enumerate() {
            by_depth[d as usize].push(i as u32);
            sum_w[d as usize] += tree.widths()[i];
        }
        let shade = max_depth.max(1) as f32;
        let mut limbs = Vec::new();
        for (d, nodes) in by_depth.iter().enumerate() {
            if nodes.is_empty() {
                continue;
            }
            let width = (sum_w[d] / nodes.len() as f32).max(1.2);
            let shadow = elements.len();
            elements.push(Element {
                group: Group::Shadow,
                stroke: "#1e2a17".to_string(),
                stroke_width: (width * 0.9).max(1.5),
                opacity: 0.16,
                frames: vec![String::new(); frames],
            });
            let branch = elements.len();
            elements.push(Element {
                group: Group::Branch,
                stroke: branch_color(d as f32 / shade),
                stroke_width: width,
                opacity: 1.0,
                frames: vec![String::new(); frames],
            });
            limbs.push((d as u8, shadow, branch));
        }
        let leaves: Vec<u32> = tree
            .leaf_flags()
            .iter()
            .enumerate()
            .filter(|&(_, l)| *l)
            .map(|(i, _)| i as u32)
            .collect();
        let canopy_dark = elements.len();
        elements.push(Element {
            group: Group::CanopyDark,
            stroke: "#3e7c34".to_string(),
            stroke_width: CANOPY_DARK_W,
            opacity: 0.9,
            frames: vec![String::new(); frames],
        });
        let canopy_light = elements.len();
        elements.push(Element {
            group: Group::CanopyLight,
            stroke: "#57a244".to_string(),
            stroke_width: CANOPY_LIGHT_W,
            opacity: 0.95,
            frames: vec![String::new(); frames],
        });
        slots.push(TreeSlots {
            limbs,
            canopy_dark,
            canopy_light,
            nodes_by_depth: by_depth,
            leaves,
        });
    }

    // Sample: one pose evaluation per tree per frame, fanned out into that
    // tree's elements.
    for k in 0..frames {
        let t = t_of(k);
        for (ti, tree) in trees.iter().enumerate() {
            tree.pose_into(&mut poses[ti], t, wind);
            let pose = &poses[ti];
            let slot = &slots[ti];
            let (gx, gz) = tile_center(anchors[ti].0, anchors[ti].1);
            for (d, shadow, branch) in slot.limbs.iter().copied() {
                let nodes = &slot.nodes_by_depth[d as usize];
                elements[shadow].frames[k] = segment_paths(pose, nodes, gx, gz, false);
                elements[branch].frames[k] = segment_paths(pose, nodes, gx, gz, true);
            }
            elements[slot.canopy_dark].frames[k] = canopy_path(pose, &slot.leaves, gx, gz, 0);
            elements[slot.canopy_light].frames[k] = canopy_path(pose, &slot.leaves, gx, gz, 1);
        }
    }

    // Bounds from every projected coordinate in the collected frames.
    let mut bounds = Bounds::default();
    for el in &elements {
        for f in &el.frames {
            bounds.grow_path(f);
        }
    }
    let mut tiles = BTreeSet::new();
    for &(ti, tj) in anchors {
        for di in -tile_radius..=tile_radius {
            for dj in -tile_radius..=tile_radius {
                tiles.insert((ti + di, tj + dj));
            }
        }
    }
    for &(i, j) in &tiles {
        for (dx, dz) in [(0.0, 0.0), (TILE, 0.0), (TILE, TILE), (0.0, TILE)] {
            let (x, y) = project_ground(i as f32 * TILE + dx, j as f32 * TILE + dz);
            bounds.grow(x, y);
        }
    }

    Ok(Scene {
        elements,
        tiles,
        bounds: bounds.finish(),
    })
}

/// "M x0 y0 L x1 y1" per segment (integer screen coordinates). `lit`
/// picks the billboard projection (true) or its ground shadow (false).
fn segment_paths(pose: &TreePose, nodes: &[u32], gx: f32, gz: f32, lit: bool) -> String {
    let project = |tx: f32, ty: f32| {
        if lit {
            project_billboard(gx, gz, tx, ty)
        } else {
            project_shadow(gx, gz, tx, ty)
        }
    };
    let mut s = String::with_capacity(nodes.len() * 22);
    for &n in nodes {
        let i = n as usize;
        let (x0, y0) = project(pose.start_x[i], pose.start_y[i]);
        let (x1, y1) = project(pose.tip_x[i], pose.tip_y[i]);
        let _ = write!(
            s,
            "M{} {}L{} {}",
            x0.round() as i64,
            y0.round() as i64,
            x1.round() as i64,
            y1.round() as i64
        );
    }
    s
}

/// Fat short strokes at every leaf tip, drawn inward along the branch —
/// the canopy layer. Each leaf scales the stroke length deterministically
/// (and per layer), so the light and dark layers interleave instead of
/// reading as concentric rings.
fn canopy_path(pose: &TreePose, leaves: &[u32], gx: f32, gz: f32, layer: u8) -> String {
    let mut s = String::with_capacity(leaves.len() * 20);
    for &n in leaves {
        let i = n as usize;
        let vary = 0.8 + 0.4 * ((n % 7) as f32 / 7.0);
        let len = CANOPY_STROKE_LEN * vary * if layer == 0 { 1.0 } else { 1.35 };
        let (tx, ty) = (pose.tip_x[i], pose.tip_y[i]);
        let a = pose.angle[i];
        let (x0, y0) = project_billboard(gx, gz, tx, ty);
        let (x1, y1) = project_billboard(gx, gz, tx - a.cos() * len, ty - a.sin() * len);
        let _ = write!(
            s,
            "M{} {}L{} {}",
            x0.round() as i64,
            y0.round() as i64,
            x1.round() as i64,
            y1.round() as i64
        );
    }
    s
}

/// Trunk brown fading toward a leafy olive at the twigs.
fn branch_color(u: f32) -> String {
    let lerp = |a: u8, b: u8| a as f32 + (b as f32 - a as f32) * u;
    format!(
        "#{:02x}{:02x}{:02x}",
        lerp(0x6b, 0x86) as u32,
        lerp(0x4a, 0x70) as u32,
        lerp(0x2f, 0x3d) as u32
    )
}

#[derive(Default)]
struct Bounds {
    minx: f32,
    miny: f32,
    maxx: f32,
    maxy: f32,
    set: bool,
}

impl Bounds {
    fn grow(&mut self, x: f32, y: f32) {
        if !self.set {
            self.minx = x;
            self.maxx = x;
            self.miny = y;
            self.maxy = y;
            self.set = true;
            return;
        }
        self.minx = self.minx.min(x);
        self.maxx = self.maxx.max(x);
        self.miny = self.miny.min(y);
        self.maxy = self.maxy.max(y);
    }

    /// Recover the integer coordinate pairs from a collected path string.
    /// Cheap enough on the render path, and it keeps projection
    /// single-sourced (no second projection pass just for bounds).
    fn grow_path(&mut self, path: &str) {
        let mut nums = path.split(|c: char| !c.is_ascii_digit() && c != '-');
        while let (Some(xs), Some(ys)) = (nums.next(), nums.next()) {
            if let (Ok(x), Ok(y)) = (xs.parse::<f32>(), ys.parse::<f32>()) {
                self.grow(x, y);
            }
        }
    }

    fn finish(self) -> (f32, f32, f32, f32) {
        (
            self.minx - MARGIN,
            self.miny - MARGIN - CAPTION_PT,
            self.maxx + MARGIN,
            self.maxy + MARGIN + CAPTION_PT * 2.0,
        )
    }
}

/// Assemble the final SVG document. `dur` = None renders the first
/// collected frame statically; Some(dur) emits SMIL animation across all
/// frames, looping with that duration.
fn emit(scene: Scene, meta: &RenderMeta, dur: Option<f32>) -> String {
    let (minx, miny, maxx, maxy) = scene.bounds;
    let (w, h) = (maxx - minx, maxy - miny);
    let mut out = String::with_capacity(1 << 16);
    let o = &mut out;

    let _ = writeln!(
        o,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{:.0} {:.0} {:.0} {:.0}" width="{:.0}" height="{:.0}">"##,
        minx, miny, w, h, w, h
    );
    let _ = writeln!(
        o,
        "  <title>bw-tree-gallery: {}</title>",
        xml_escape(meta.scene)
    );
    let _ = writeln!(
        o,
        "  <desc>Procedural golden-ratio tree (fork spread pi-137.5deg = 42.49deg, side length ratio 1/phi), tactical camera pitch {PITCH_DEG:.0}deg yaw {YAW_DEG:.0}deg, seed {}.</desc>",
        meta.seed
    );
    let _ = writeln!(
        o,
        r##"  <defs><linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">"##
    );
    let _ = writeln!(o, r##"    <stop offset="0" stop-color="#cfe7f7"/>"##);
    let _ = writeln!(o, r##"    <stop offset="1" stop-color="#eef8e6"/>"##);
    let _ = writeln!(o, "  </linearGradient></defs>");
    let _ = writeln!(
        o,
        r##"  <rect x="{minx:.0}" y="{miny:.0}" width="{w:.0}" height="{h:.0}" fill="url(#sky)"/>"##
    );

    // Ground: the diamond checkerboard.
    let _ = writeln!(o, r##"  <g stroke="#3f7534" stroke-width="1.5">"##);
    for &(i, j) in &scene.tiles {
        let corner = |dx: f32, dz: f32| project_ground(i as f32 * TILE + dx, j as f32 * TILE + dz);
        let (x0, y0) = corner(0.0, 0.0);
        let (x1, y1) = corner(TILE, 0.0);
        let (x2, y2) = corner(TILE, TILE);
        let (x3, y3) = corner(0.0, TILE);
        let fill = if (i + j) % 2 == 0 {
            TILE_LIGHT
        } else {
            TILE_DARK
        };
        let _ = writeln!(
            o,
            r##"    <polygon points="{:.0},{:.0} {:.0},{:.0} {:.0},{:.0} {:.0},{:.0}" fill="{fill}"/>"##,
            x0, y0, x1, y1, x2, y2, x3, y3
        );
    }
    let _ = writeln!(o, "  </g>");

    // Element groups in paint order: shadows, twigs-to-trunk branches,
    // then the two canopy layers on top.
    for group in [
        Group::Shadow,
        Group::Branch,
        Group::CanopyDark,
        Group::CanopyLight,
    ] {
        let _ = writeln!(
            o,
            r##"  <g fill="none" stroke-linecap="round" stroke-linejoin="round">"##
        );
        for el in scene.elements.iter().filter(|e| e.group == group) {
            let Some(first) = el.frames.first() else {
                continue;
            };
            let _ = writeln!(
                o,
                r##"    <path d="{}" stroke="{}" stroke-width="{:.1}" opacity="{:.2}">"##,
                first, el.stroke, el.stroke_width, el.opacity
            );
            if let Some(dur) = dur
                && el.frames.len() > 1
            {
                let _ = write!(
                    o,
                    r##"      <animate attributeName="d" calcMode="linear" dur="{dur:.2}s" repeatCount="indefinite" keyTimes=""##
                );
                let n = el.frames.len();
                for k in 0..n {
                    if k > 0 {
                        let _ = write!(o, ";");
                    }
                    let _ = write!(o, "{:.4}", k as f32 / (n - 1) as f32);
                }
                let _ = write!(o, r##"" values=""##);
                for (k, f) in el.frames.iter().enumerate() {
                    if k > 0 {
                        let _ = write!(o, ";");
                    }
                    let _ = write!(o, "{f}");
                }
                let _ = writeln!(o, r##""/>"##);
            }
            let _ = writeln!(o, "    </path>");
        }
        let _ = writeln!(o, "  </g>");
    }

    let _ = writeln!(
        o,
        r##"  <text x="{:.0}" y="{:.0}" font-family="monospace" font-size="{CAPTION_PT}" fill="#33402a">{} &#183; seed {} &#183; golden-angle forking &#183; pitch {PITCH_DEG:.0}&#176; yaw {YAW_DEG:.0}&#176;</text>"##,
        minx + MARGIN,
        maxy - MARGIN * 0.5,
        xml_escape(meta.scene),
        meta.seed
    );
    let _ = writeln!(o, "</svg>");
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bw_core::tree::{CALM_WIND, TreeParams};

    fn fixture() -> (Vec<Tree>, Vec<TreePose>, Vec<(i32, i32)>) {
        let mut params = TreeParams::oak();
        params.max_depth = 5; // small and fast for tests
        let tree = Tree::generate(&params, 42);
        let pose = TreePose::new(&tree);
        (vec![tree], vec![pose], vec![(0, 0)])
    }

    fn write_to_temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("bw-tree-gallery-test");
        std::fs::create_dir_all(&dir).ok();
        dir.join(name)
    }

    #[test]
    fn animation_writes_a_multi_frame_svg() -> Result<(), String> {
        let (trees, mut poses, anchors) = fixture();
        let meta = RenderMeta {
            scene: "tree",
            seed: 42,
            fps: 4,
        };
        let path = write_to_temp("anim.svg");
        render_animation(
            &trees,
            &mut poses,
            &anchors,
            &CALM_WIND,
            2,
            &meta,
            path.to_str().ok_or("temp path not utf-8")?,
        )?;
        let svg = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        assert!(svg.contains("<svg"), "missing svg root");
        assert!(svg.contains("<animate"), "no SMIL animation");
        assert!(svg.contains("keyTimes"), "no keyTimes");
        assert!(svg.contains("repeatCount=\"indefinite\""));
        assert!(svg.contains("<polygon"), "no checkerboard ground");
        assert!(svg.contains("golden-angle"), "no caption provenance");
        Ok(())
    }

    #[test]
    fn static_frame_has_no_animation() -> Result<(), String> {
        let (trees, mut poses, anchors) = fixture();
        let meta = RenderMeta {
            scene: "tree",
            seed: 42,
            fps: 1,
        };
        let path = write_to_temp("frame.svg");
        render_frame(
            &trees,
            &mut poses,
            &anchors,
            &CALM_WIND,
            2,
            &meta,
            99.0,
            path.to_str().ok_or("temp path not utf-8")?,
        )?;
        let svg = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        assert!(svg.contains("<svg"));
        assert!(!svg.contains("<animate"), "static frame must not animate");
        Ok(())
    }

    #[test]
    fn mismatched_inputs_are_rejected() {
        let (trees, mut poses, _) = fixture();
        let meta = RenderMeta {
            scene: "tree",
            seed: 1,
            fps: 2,
        };
        let err = render_animation(&trees, &mut poses, &[], &CALM_WIND, 2, &meta, "/dev/null");
        assert!(err.is_err());
    }

    #[test]
    fn branch_color_interpolates() {
        assert_eq!(branch_color(0.0), "#6b4a2f");
        assert_eq!(branch_color(1.0), "#86703d");
    }

    #[test]
    fn xml_escape_handles_markup() {
        assert_eq!(xml_escape("a<b&c>"), "a&lt;b&amp;c&gt;");
    }
}
