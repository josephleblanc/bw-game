//! Tactical SVG render for the map (ADR 0006): the renderer-free
//! visual surface, the same role the tactical SVG plays for the tree
//! and walker galleries. The projection is orthographic along the
//! shared `bw_core::camera` orientation, so the plate reads exactly
//! like the walker viewer's default camera — one law, no drift.
//! Terrain paints as one subpath-combined path per kind (soil is the
//! plate itself), the tile grid and sector lines give the scale, and
//! the caption carries full provenance: scene, seed, checksum, and
//! the walkable count.
//!
//! Zero new dependencies: the document is plain string writing. This
//! path allocates freely; it is not the measured pass (generation
//! is), so ADR 0004's allocate-once rule does not reach here.

use std::fmt::Write as _;

use bw_core::camera;
use bw_core::map::{Map, Terrain, Tile};
use bw_core::math::Vec3;

use crate::SceneSummary;

/// Provenance carried into the rendered file.
pub struct RenderMeta<'a> {
    pub scene: &'a str,
    pub seed: u64,
}

// ---------------------------------------------------------------------------
// The orthographic camera
// ---------------------------------------------------------------------------

/// Orthographic projection looking along −`tactical_dir()` at a
/// focus: a world point's screen position is its camera-basis offset
/// times the focal, with no divide — ground-plane geometry maps
/// affinely (tile quads stay parallelograms; a test pins that).
struct Ortho {
    focus: Vec3,
    right: Vec3,
    up: Vec3,
    focal: f32,
}

impl Ortho {
    fn tactical(focus: Vec3, focal: f32) -> Self {
        let fwd = tactical_back();
        let right = fwd
            .cross(Vec3::new(0.0, 1.0, 0.0))
            .normalized()
            .unwrap_or(Vec3::new(1.0, 0.0, 0.0));
        let up = right.cross(fwd);
        Self {
            focus,
            right,
            up,
            focal,
        }
    }

    /// World ground point → screen (y down).
    fn project(&self, p: Vec3) -> (f32, f32) {
        let d = p - self.focus;
        (self.focal * d.dot(self.right), -self.focal * d.dot(self.up))
    }
}

fn tactical_back() -> Vec3 {
    camera::tactical_dir() * -1.0
}

// ---------------------------------------------------------------------------
// Palette and layout
// ---------------------------------------------------------------------------

/// Screen units per meter (the coordinate density; the viewBox
/// auto-fits, so this only sets stroke-width scale).
const FOCAL: f32 = 30.0;
const MARGIN: f32 = 48.0;
const CAPTION_PT: f32 = 15.0;
/// Sector lines every 8 m; the faint tile grid carries the 1 m read.
const SECTOR: i32 = 8;

const PAPER: &str = "#e9e4d4";
const SOIL: &str = "#5d9c4b";
const STONE: &str = "#a89f8c";
const STONE_EDGE: &str = "#8d8471";
const WATER: &str = "#49799c";
const WATER_EDGE: &str = "#395f80";
const INK: &str = "#2c3540";
const CAPTION_INK: &str = "#4a4636";

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the map to a self-contained static SVG at `path`.
pub fn render(map: &Map, meta: &RenderMeta, path: &str) -> Result<(), String> {
    let svg = render_string(map, meta);
    std::fs::write(path, svg).map_err(|e| format!("failed to write {path}: {e}"))
}

/// The render as a string — pure function of `(map, meta)`, so a hash
/// of it is a golden-image regression test without the image (the
/// `render-snapshot` law, applied here from day one).
pub fn render_string(map: &Map, meta: &RenderMeta) -> String {
    let summary: SceneSummary = crate::summarize(map);
    let (w, h) = (map.width() as i32, map.height() as i32);
    let cam = Ortho::tactical(Vec3::new(w as f32 * 0.5, 0.0, h as f32 * 0.5), FOCAL);
    let p = |x: i32, z: i32| cam.project(Vec3::new(x as f32, 0.0, z as f32));
    let r = |c: (f32, f32)| (c.0.round() as i64, c.1.round() as i64);

    // Fit the viewBox to the projected plate (affine, so the four map
    // corners bound everything).
    let corners = [p(0, 0), p(w, 0), p(w, h), p(0, h)];
    let minx = corners.iter().fold(f32::MAX, |m, c| m.min(c.0)) - MARGIN;
    let miny = corners.iter().fold(f32::MAX, |m, c| m.min(c.1)) - MARGIN;
    let maxx = corners.iter().fold(f32::MIN, |m, c| m.max(c.0)) + MARGIN;
    let maxy = corners.iter().fold(f32::MIN, |m, c| m.max(c.1)) + MARGIN;
    let (vw, vh) = (maxx - minx, maxy - miny);

    // The map outline as one closed quad: soil fill, ink border, and
    // the fit reference — the projection is affine, so these four
    // corners bound everything the plate can draw.
    let mut quad = String::with_capacity(64);
    for c in [p(w, 0), p(w, h), p(0, h)] {
        let (x, y) = r(c);
        let _ = write!(quad, "L{x} {y}");
    }
    let (x0, y0) = r(p(0, 0));

    let mut stone = String::with_capacity(1 << 12);
    let mut water = String::with_capacity(1 << 12);
    for z in 0..h {
        for x in 0..w {
            let target = match map.terrain_at(Tile { x, z }) {
                Some(Terrain::Stone) => &mut stone,
                Some(Terrain::Water) => &mut water,
                _ => continue,
            };
            let (a, b, c, d) = (
                r(p(x, z)),
                r(p(x + 1, z)),
                r(p(x + 1, z + 1)),
                r(p(x, z + 1)),
            );
            let _ = write!(
                target,
                "M{} {}L{} {}L{} {}L{} {}Z",
                a.0, a.1, b.0, b.1, c.0, c.1, d.0, d.1
            );
        }
    }

    // Grid: faint per-tile lines, stronger sector lines.
    let mut tiles = String::with_capacity(1 << 12);
    let mut sectors = String::with_capacity(1 << 10);
    for g in 0..=w {
        let (a, b) = (r(p(g, 0)), r(p(g, h)));
        let target = if g % SECTOR == 0 {
            &mut sectors
        } else {
            &mut tiles
        };
        let _ = write!(target, "M{} {}L{} {}", a.0, a.1, b.0, b.1);
    }
    for g in 0..=h {
        let (a, b) = (r(p(0, g)), r(p(w, g)));
        let target = if g % SECTOR == 0 {
            &mut sectors
        } else {
            &mut tiles
        };
        let _ = write!(target, "M{} {}L{} {}", a.0, a.1, b.0, b.1);
    }

    let mut out = String::with_capacity(1 << 16);
    let o = &mut out;
    let _ = writeln!(
        o,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{minx:.0} {miny:.0} {vw:.0} {vh:.0}" width="{vw:.0}" height="{vh:.0}">"#,
    );
    let _ = writeln!(
        o,
        "  <title>bw-map-gallery: {}</title>",
        xml_escape(meta.scene)
    );
    let _ = writeln!(
        o,
        "  <desc>Seeded value-noise tile map, {w}&#215;{h} m: {} walkable of {} tiles ({} soil / {} stone / {} water). Orthographic projection at azimuth {:.0}&#176; / elevation {:.1}&#176; (ADR 0006), seed {}, checksum {:016x}.</desc>",
        summary.walkable,
        summary.tiles,
        summary.soil,
        summary.stone,
        summary.water,
        camera::TACTICAL_AZIMUTH_DEG,
        camera::TACTICAL_ELEVATION_DEG,
        meta.seed,
        summary.checksum
    );
    let _ = writeln!(
        o,
        r#"  <rect x="{minx:.0}" y="{miny:.0}" width="{vw:.0}" height="{vh:.0}" fill="{PAPER}"/>"#
    );
    let _ = writeln!(o, r#"  <path d="M{x0} {y0}{quad}Z" fill="{SOIL}"/>"#);
    if !stone.is_empty() {
        let _ = writeln!(
            o,
            r#"  <path d="{stone}" fill="{STONE}" stroke="{STONE_EDGE}" stroke-width="1.0" stroke-linejoin="round"/>"#
        );
    }
    if !water.is_empty() {
        let _ = writeln!(
            o,
            r#"  <path d="{water}" fill="{WATER}" stroke="{WATER_EDGE}" stroke-width="1.2" stroke-linejoin="round"/>"#
        );
    }
    let _ = writeln!(
        o,
        r#"  <path d="{tiles}" fill="none" stroke="{INK}" stroke-width="0.5" opacity="0.10"/>"#
    );
    let _ = writeln!(
        o,
        r#"  <path d="{sectors}" fill="none" stroke="{INK}" stroke-width="1.2" opacity="0.22"/>"#
    );
    let _ = writeln!(
        o,
        r#"  <path d="M{x0} {y0}{quad}Z" fill="none" stroke="{INK}" stroke-width="3.0" stroke-linejoin="round"/>"#
    );
    let _ = writeln!(
        o,
        r#"  <text x="{:.0}" y="{:.0}" font-family="monospace" font-size="{CAPTION_PT}" fill="{CAPTION_INK}">{} &#183; seed {} &#183; checksum {:016x} &#183; {}/{} walkable &#183; ortho {:.0}&#176;/{:.1}&#176;</text>"#,
        minx + MARGIN,
        maxy - MARGIN * 0.5,
        xml_escape(meta.scene),
        meta.seed,
        summary.checksum,
        summary.walkable,
        summary.tiles,
        camera::TACTICAL_AZIMUTH_DEG,
        camera::TACTICAL_ELEVATION_DEG
    );
    let _ = writeln!(o, "</svg>");
    out
}

/// FNV-1a over the document — the golden render hash: the render is a
/// pure function of `(map, meta)`, so the hash pins it without
/// keeping an image around.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xCBF2_9CE4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x1000_0000_01B3)
    })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_scene, scene_preset, summarize};

    fn meadow(seed: u64) -> Map {
        let params = scene_preset("meadow").unwrap_or_else(|| panic!("meadow scene"));
        build_scene(&params, seed)
    }

    /// The golden render hash: the canonical meadow pins byte-for-byte.
    /// Moves whenever the map's own golden checksum moves — the render
    /// caption carries the map checksum as provenance, so a channel
    /// landing repins both (repinned 2026-09-23 for temperature +
    /// fertility).
    #[test]
    fn golden_render_hash_is_pinned() {
        let meta = RenderMeta {
            scene: "meadow",
            seed: 42,
        };
        let svg = render_string(&meadow(42), &meta);
        assert_eq!(
            fnv1a(svg.as_bytes()),
            0x6F72_83DA_1147_9EE2,
            "render churned — retune intentionally and repin"
        );
    }

    /// The render is a pure function of (map, meta): identical inputs
    /// reproduce byte-for-byte, a rerolled seed does not.
    #[test]
    fn render_is_deterministic_and_seed_sensitive() {
        let meta = RenderMeta {
            scene: "meadow",
            seed: 42,
        };
        assert_eq!(
            render_string(&meadow(42), &meta),
            render_string(&meadow(42), &meta)
        );
        assert_ne!(
            render_string(&meadow(42), &meta),
            render_string(&meadow(43), &meta)
        );
    }

    /// Layer presence: the meadow paints stone and water; the blank
    /// scene paints neither (soil plate only).
    #[test]
    fn terrain_layers_render_by_kind() {
        let meta = RenderMeta {
            scene: "meadow",
            seed: 42,
        };
        let svg = render_string(&meadow(42), &meta);
        assert!(svg.contains(&format!(r#"fill="{STONE}""#)), "no stone");
        assert!(svg.contains(&format!(r#"fill="{WATER}""#)), "no water");
        assert!(svg.contains(&format!(r#"fill="{SOIL}""#)), "no soil plate");

        let blank_params = scene_preset("blank").unwrap_or_else(|| panic!("blank scene"));
        let blank = build_scene(&blank_params, 7);
        let blank_meta = RenderMeta {
            scene: "blank",
            seed: 7,
        };
        let svg = render_string(&blank, &blank_meta);
        assert!(!svg.contains(&format!(r#"fill="{STONE}""#)));
        assert!(!svg.contains(&format!(r#"fill="{WATER}""#)));
    }

    /// Orthographic means affine on the ground plane: the projected
    /// map quad is a parallelogram (diagonals share a midpoint), and
    /// tile quads project inside the fitted frame.
    #[test]
    fn ortho_projection_is_affine_on_the_ground() {
        let cam = Ortho::tactical(Vec3::new(32.0, 0.0, 32.0), FOCAL);
        let a = cam.project(Vec3::new(0.0, 0.0, 0.0));
        let b = cam.project(Vec3::new(64.0, 0.0, 0.0));
        let c = cam.project(Vec3::new(64.0, 0.0, 64.0));
        let d = cam.project(Vec3::new(0.0, 0.0, 64.0));
        let mid1 = ((a.0 + c.0) * 0.5, (a.1 + c.1) * 0.5);
        let mid2 = ((b.0 + d.0) * 0.5, (b.1 + d.1) * 0.5);
        assert!(
            (mid1.0 - mid2.0).abs() < 1e-3 && (mid1.1 - mid2.1).abs() < 1e-3,
            "projected quad is not a parallelogram — perspective crept in"
        );
        // The projection looks along the shared tactical direction:
        // depth orders the map. The eye sits in the +x/+z quadrant
        // (49° azimuth), so (0,0) is the far corner and must render
        // above (screen-y smaller) the near corner (64,64).
        assert!(
            d.1 < b.1,
            "far corner must render above the near corner: far {} near {}",
            d.1,
            b.1
        );
    }

    /// The file writes and carries its provenance caption.
    #[test]
    fn render_writes_the_file_with_provenance() {
        let map = meadow(9);
        let summary = summarize(&map);
        let dir = std::env::temp_dir().join("bw-map-gallery-test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("meadow.svg");
        let meta = RenderMeta {
            scene: "meadow",
            seed: 9,
        };
        render(&map, &meta, path.to_str().unwrap_or("")).unwrap_or_else(|e| panic!("{e}"));
        let svg = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(svg.contains("<svg"), "missing svg root");
        assert!(svg.contains("seed 9"), "no seed caption");
        assert!(
            svg.contains(&format!("{:016x}", summary.checksum)),
            "no checksum caption"
        );
    }
}
