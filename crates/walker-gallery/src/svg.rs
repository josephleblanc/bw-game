//! Animated-SVG gallery output for the walker: the renderer-free
//! visual surface (ADR 0002/0003) — the same role the tactical SVG
//! plays for the tree gallery, but the projection is a real 3D camera
//! (perspective, look-at) so this path and the live 3D viewer read the
//! scene identically: world-space joint positions from
//! `Character::pose_into`, projected to screen here, fed to Bevy's 3D
//! camera there. Zero new dependencies: the document is assembled with
//! plain string writing and SMIL `<animate>` plays the walk cycle.
//!
//! This path allocates freely; it is not the measured hot loop (the
//! headless harness is), so ADR 0004's allocate-once rule does not
//! apply here — only to `Character::step`/`pose_into`, which this
//! module calls once per walker per sampled frame.

use std::fmt::Write as _;

use bw_core::character::{
    BONE_COUNT, Character, CharacterPose, HEAD_RADIUS, MovementInput, Skeleton, bone,
};
use bw_core::math::Vec3;

/// Provenance caption carried into the rendered file.
pub struct RenderMeta<'a> {
    pub scene: &'a str,
    pub seed: u64,
    pub fps: u32,
}
/// One character in the render: state, input, and the reused pose buffer.
pub struct RenderWalker {
    pub character: Character,
    pub input: MovementInput,
    pose: CharacterPose,
}

impl RenderWalker {
    pub fn new(character: Character, input: MovementInput, skeleton: &Skeleton) -> Self {
        Self {
            character,
            input,
            pose: CharacterPose::new(skeleton),
        }
    }
}

// ---------------------------------------------------------------------------
// The 3D camera
// ---------------------------------------------------------------------------

/// A perspective pinhole: look-at basis plus a vertical field of view.
/// `focal` is the screen-space focal length (the projection's pixel
/// scale); SVG output auto-fits its viewBox, so its absolute value only
/// sets the coordinate density.
pub struct Camera {
    eye: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    focal: f32,
}

impl Camera {
    /// Camera at `eye` looking at `target` with vertical FOV
    /// `fov_y_deg` and the given screen focal length.
    pub fn looking(eye: Vec3, target: Vec3, fov_y_deg: f32, focal: f32) -> Self {
        let forward = (target - eye)
            .normalized()
            .unwrap_or(Vec3::new(0.0, 0.0, 1.0));
        let right = forward
            .cross(Vec3::new(0.0, 1.0, 0.0))
            .normalized()
            .unwrap_or(Vec3::new(1.0, 0.0, 0.0));
        let up = right.cross(forward);
        Self {
            eye,
            forward,
            right,
            up,
            focal: focal / (fov_y_deg.to_radians() * 0.5).tan(),
        }
    }

    /// View-space depth of a world point (negative = behind the eye).
    pub fn view_depth(&self, p: Vec3) -> f32 {
        (p - self.eye).dot(self.forward)
    }

    /// Project a world point to screen coordinates (y down). `None`
    /// when the point is behind the near plane (z ≤ 0).
    pub fn project(&self, p: Vec3) -> Option<(f32, f32)> {
        let d = p - self.eye;
        let z = d.dot(self.forward);
        if z <= 0.01 {
            return None;
        }
        Some((
            self.focal * d.dot(self.right) / z,
            -self.focal * d.dot(self.up) / z,
        ))
    }

    /// Screen-space radius of a sphere of world radius `r` at `p`
    /// (small-angle approximation of the silhouette).
    pub fn project_radius(&self, p: Vec3, r: f32) -> Option<f32> {
        let d = p - self.eye;
        let z = d.dot(self.forward);
        if z <= 0.01 {
            return None;
        }
        Some(self.focal * r / z)
    }
}

/// The gallery's establishing shot — also the viewer's opening framing
/// (the chase camera takes over from there). The eye sits outside and
/// above the ground grid, so no ground geometry approaches the view
/// plane (the perspective divide stays tame everywhere on the plate).
pub const EYE: Vec3 = Vec3::new(13.0, 6.0, 13.0);
pub const TARGET: Vec3 = Vec3::new(0.0, 0.9, 0.0);
pub const FOV_Y_DEG: f32 = 40.0;

/// Sun direction for the ground shadow (pointing down; shadows cast
/// toward the viewer's right).
const SUN: Vec3 = Vec3::new(0.5, -1.0, 0.35);

/// Project a point along the sun onto the y = 0 ground plane.
fn ground_shadow(p: Vec3) -> Vec3 {
    p + SUN * (p.y / -SUN.y)
}

// ---------------------------------------------------------------------------
// Palette and layout
// ---------------------------------------------------------------------------

const MARGIN: f32 = 40.0;
const CAPTION_PT: f32 = 15.0;
/// Ground grid extent (meters from the origin).
const GRID: f32 = 13.0;
const GRID_MAJOR: &str = "#4d8e3b";
const GRID_MINOR: &str = "#63a84c";
const GROUND_FILL: &str = "#57a244";
const SKY_TOP: &str = "#cfe7f7";
const SKY_BOT: &str = "#eef8e6";
const FIGURE: &str = "#25324a";
const FIGURE_SOFT: &str = "#3d4d6b";
const SHADOW: &str = "#1e2a17";

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the walk cycle as one self-contained SMIL-animated SVG:
/// `cycles` full gait cycles of the first walker, looping.
pub fn render_animation(
    walkers: &mut [RenderWalker],
    skeleton: &Skeleton,
    meta: &RenderMeta,
    cycles: f32,
    path: &str,
) -> Result<(), String> {
    if walkers.is_empty() {
        return Err("no walkers to render".to_string());
    }
    let speed = walkers[0].input.target_speed.max(0.1);
    let stride = bw_core::character::gait_at(speed).stride_len.max(0.1);
    let rate = speed / stride; // cycles per second
    let dur = cycles / rate;
    let frames = (((dur * meta.fps as f32).ceil() as usize) + 1).max(2);
    let fps = meta.fps as f32;
    let scene = collect(walkers, skeleton, 1.0 / fps, frames, 0)?;
    let svg = emit(scene, meta, Some(dur));
    std::fs::write(path, svg).map_err(|e| format!("failed to write {path}: {e}"))
}

/// Render one static frame at scene time `t`.
pub fn render_frame(
    walkers: &mut [RenderWalker],
    skeleton: &Skeleton,
    meta: &RenderMeta,
    t: f32,
    path: &str,
) -> Result<(), String> {
    let fps = meta.fps as f32;
    let scene = collect(walkers, skeleton, 1.0 / fps, 1, (t * fps).round() as usize)?;
    let svg = emit(scene, meta, None);
    std::fs::write(path, svg).map_err(|e| format!("failed to write {path}: {e}"))
}

/// One drawn element: fixed styling plus one path/position per frame.
struct Element {
    kind: Kind,
    stroke: &'static str,
    stroke_width: f32,
    opacity: f32,
    frames: Vec<String>,
    /// Circle center + radius triples for `Kind::Head` frames.
    circles: Vec<(f32, f32, f32)>,
}

enum Kind {
    Path,
    Head,
}

struct Scene {
    elements: Vec<Element>,
}

/// The output frame (screen units at focal 900): the camera's own
/// viewport, centered on the target's projection. Content outside
/// crops at the SVG root, exactly like a real camera.
const FRAME_W: f32 = 1460.0;
const FRAME_H: f32 = 920.0;

fn collect(
    walkers: &mut [RenderWalker],
    skeleton: &Skeleton,
    dt: f32,
    frames: usize,
    pre_steps: usize,
) -> Result<Scene, String> {
    let cam = Camera::looking(EYE, TARGET, FOV_Y_DEG, 900.0);

    // Ground plate: one big quad under the grid.
    let mut elements = Vec::new();
    let corners = [
        cam.project(Vec3::new(-GRID, 0.0, -GRID)),
        cam.project(Vec3::new(GRID, 0.0, -GRID)),
        cam.project(Vec3::new(GRID, 0.0, GRID)),
        cam.project(Vec3::new(-GRID, 0.0, GRID)),
    ];
    if let Some(first) = corners[0] {
        let mut d = format!("M{} {}", first.0.round() as i64, first.1.round() as i64);
        for c in corners.iter().skip(1).flatten() {
            let _ = write!(d, "L{} {}", c.0.round() as i64, c.1.round() as i64);
        }
        let _ = write!(
            d,
            "L{} {}",
            corners[0].unwrap_or((0.0, 0.0)).0.round() as i64,
            corners[0].unwrap_or((0.0, 0.0)).1.round() as i64
        );
        elements.push(Element {
            kind: Kind::Path,
            stroke: GROUND_FILL,
            stroke_width: 0.0,
            opacity: 1.0,
            frames: vec![d; frames],
            circles: Vec::new(),
        });
    }
    // Grid lines: majors every 2 m over the plate.
    for line in -GRID as i32..=GRID as i32 {
        let f = line as f32;
        let major = line % 2 == 0;
        for (a, b) in [
            (Vec3::new(f, 0.0, -GRID), Vec3::new(f, 0.0, GRID)),
            (Vec3::new(-GRID, 0.0, f), Vec3::new(GRID, 0.0, f)),
        ] {
            // Near-plane guard: skip lines reaching toward the view
            // plane (impossible with the establishing-shot constants —
            // insurance against retuning the eye inside the grid).
            if cam.view_depth(a) < 0.5 || cam.view_depth(b) < 0.5 {
                continue;
            }
            let Some((x0, y0)) = cam.project(a) else {
                continue;
            };
            let Some((x1, y1)) = cam.project(b) else {
                continue;
            };
            let d = format!(
                "M{} {}L{} {}",
                x0.round() as i64,
                y0.round() as i64,
                x1.round() as i64,
                y1.round() as i64
            );
            elements.push(Element {
                kind: Kind::Path,
                stroke: if major { GRID_MAJOR } else { GRID_MINOR },
                stroke_width: if major { 2.0 } else { 1.0 },
                opacity: if major { 0.9 } else { 0.55 },
                frames: vec![d; frames],
                circles: Vec::new(),
            });
        }
    }

    // Figures: one shadow path and one limb path per walker per frame,
    // plus an animated head circle. `pre_steps` advances the scene to
    // the static frame's time; the animation starts at t = 0.
    for _ in 0..pre_steps {
        for w in walkers.iter_mut() {
            w.character.step(dt, &w.input);
        }
    }
    for w in walkers.iter_mut() {
        let mut shadow_frames = Vec::with_capacity(frames);
        let mut limb_frames = Vec::with_capacity(frames);
        let mut head_frames = Vec::with_capacity(frames);
        for f in 0..frames {
            if f > 0 {
                w.character.step(dt, &w.input);
            }
            w.character.pose_into(skeleton, &mut w.pose);
            let mut shadow = String::with_capacity(BONE_COUNT * 24);
            let mut limbs = String::with_capacity(BONE_COUNT * 24);
            let mut head = (0.0, 0.0, 0.0);
            for b in 0..skeleton.bone_count() {
                if let (Some((x0, y0)), Some((x1, y1))) = (
                    cam.project(ground_shadow(w.pose.start[b])),
                    cam.project(ground_shadow(w.pose.tip[b])),
                ) {
                    let _ = write!(
                        shadow,
                        "M{} {}L{} {}",
                        x0.round() as i64,
                        y0.round() as i64,
                        x1.round() as i64,
                        y1.round() as i64
                    );
                }
                if b == bone::HEAD {
                    if let (Some((hx, hy)), Some(r)) = (
                        cam.project(w.pose.tip[b]),
                        cam.project_radius(w.pose.tip[b], HEAD_RADIUS * w.character.scale),
                    ) {
                        head = (hx, hy, r);
                    }
                    continue; // the head is a ball, not a stick
                }
                if let (Some((x0, y0)), Some((x1, y1))) =
                    (cam.project(w.pose.start[b]), cam.project(w.pose.tip[b]))
                {
                    let _ = write!(
                        limbs,
                        "M{} {}L{} {}",
                        x0.round() as i64,
                        y0.round() as i64,
                        x1.round() as i64,
                        y1.round() as i64
                    );
                }
            }
            shadow_frames.push(shadow);
            limb_frames.push(limbs);
            head_frames.push(head);
        }
        elements.push(Element {
            kind: Kind::Path,
            stroke: SHADOW,
            stroke_width: 5.0,
            opacity: 0.16,
            frames: shadow_frames,
            circles: Vec::new(),
        });
        elements.push(Element {
            kind: Kind::Path,
            stroke: FIGURE,
            stroke_width: 0.0, // per-bone width applied below
            opacity: 1.0,
            frames: limb_frames,
            circles: Vec::new(),
        });
        elements.push(Element {
            kind: Kind::Head,
            stroke: FIGURE,
            stroke_width: 0.0,
            opacity: 1.0,
            frames: Vec::new(),
            circles: head_frames,
        });
    }

    Ok(Scene { elements })
}

impl RenderMeta<'_> {}

/// Assemble the final SVG document. `dur` = None renders the first
/// collected frame statically; Some(dur) emits SMIL animation across
/// all frames, looping with that duration.
fn emit(scene: Scene, meta: &RenderMeta, dur: Option<f32>) -> String {
    let (minx, miny) = (-FRAME_W, -FRAME_H);
    let (w, h) = (2.0 * FRAME_W, 2.0 * FRAME_H);
    let mut out = String::with_capacity(1 << 16);
    let o = &mut out;

    let _ = writeln!(
        o,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{:.0} {:.0} {:.0} {:.0}" width="{:.0}" height="{:.0}">"##,
        minx, miny, w, h, w, h
    );
    let _ = writeln!(
        o,
        "  <title>bw-walker-gallery: {}</title>",
        xml_escape(meta.scene)
    );
    let _ = writeln!(
        o,
        "  <desc>Procedural walk cycle (stick figure, 13 bones, speed-driven gait blend), perspective camera fov {FOV_Y_DEG:.0}deg at ({:.1}, {:.1}, {:.1}), seed {}.</desc>",
        EYE.x, EYE.y, EYE.z, meta.seed
    );
    let _ = writeln!(
        o,
        r##"  <defs><linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">"##
    );
    let _ = writeln!(o, r##"    <stop offset="0" stop-color="{SKY_TOP}"/>"##);
    let _ = writeln!(o, r##"    <stop offset="1" stop-color="{SKY_BOT}"/>"##);
    let _ = writeln!(o, "  </linearGradient></defs>");
    let _ = writeln!(
        o,
        r##"  <rect x="{minx:.0}" y="{miny:.0}" width="{w:.0}" height="{h:.0}" fill="url(#sky)"/>"##
    );

    for el in &scene.elements {
        match el.kind {
            Kind::Path => {
                let Some(first) = el.frames.first() else {
                    continue;
                };
                if el.stroke == FIGURE {
                    // Stick limbs: round strokes, width from the rig.
                    let _ = writeln!(
                        o,
                        r##"  <path d="{first}" fill="none" stroke="{FIGURE_SOFT}" stroke-width="{:.1}" stroke-linecap="round">"##,
                        3.2
                    );
                    if let Some(dur) = dur
                        && el.frames.len() > 1
                    {
                        write_animate(o, &el.frames, dur);
                    }
                    let _ = writeln!(o, "  </path>");
                } else if el.stroke_width > 0.0 {
                    let _ = writeln!(
                        o,
                        r##"  <path d="{first}" fill="{}" stroke="{first_stroke}" stroke-width="{:.1}" opacity="{:.2}">"##,
                        if el.stroke == GROUND_FILL {
                            el.stroke
                        } else {
                            "none"
                        },
                        el.stroke_width,
                        el.opacity,
                        first_stroke = el.stroke,
                    );
                    let _ = writeln!(o, "  </path>");
                } else {
                    let _ = writeln!(o, r##"  <path d="{first}" fill="{GROUND_FILL}"/>"##);
                }
            }
            Kind::Head => {
                let Some(first) = el.circles.first() else {
                    continue;
                };
                let _ = writeln!(
                    o,
                    r##"  <circle cx="{:.0}" cy="{:.0}" r="{:.1}" fill="{FIGURE}">"##,
                    first.0, first.1, first.2
                );
                if let Some(dur) = dur
                    && el.circles.len() > 1
                {
                    for (attr, pick) in [("cx", 0usize), ("cy", 1), ("r", 2)] {
                        let _ = write!(
                            o,
                            r##"    <animate attributeName="{attr}" calcMode="linear" dur="{dur:.2}s" repeatCount="indefinite" values=""##
                        );
                        for (k, c) in el.circles.iter().enumerate() {
                            if k > 0 {
                                let _ = write!(o, ";");
                            }
                            let _ = write!(
                                o,
                                "{:.1}",
                                match pick {
                                    0 => c.0,
                                    1 => c.1,
                                    _ => c.2,
                                }
                            );
                        }
                        let _ = writeln!(o, r##""/>"##);
                    }
                }
                let _ = writeln!(o, "  </circle>");
            }
        }
    }

    let _ = writeln!(
        o,
        r##"  <text x="{:.0}" y="{:.0}" font-family="monospace" font-size="{CAPTION_PT}" fill="#33402a">{} &#183; seed {} &#183; procedural gait &#183; fov {FOV_Y_DEG:.0}&#176; at ({:.0}, {:.0}, {:.0})</text>"##,
        minx + MARGIN,
        FRAME_H - MARGIN * 0.5,
        xml_escape(meta.scene),
        meta.seed,
        EYE.x,
        EYE.y,
        EYE.z
    );
    let _ = writeln!(o, "</svg>");
    out
}

fn write_animate(o: &mut String, frames: &[String], dur: f32) {
    let _ = write!(
        o,
        r##"    <animate attributeName="d" calcMode="linear" dur="{dur:.2}s" repeatCount="indefinite" keyTimes=""##
    );
    let n = frames.len();
    for k in 0..n {
        if k > 0 {
            let _ = write!(o, ";");
        }
        let _ = write!(o, "{:.4}", k as f32 / (n - 1) as f32);
    }
    let _ = write!(o, r##"" values=""##);
    for (k, f) in frames.iter().enumerate() {
        if k > 0 {
            let _ = write!(o, ";");
        }
        let _ = write!(o, "{f}");
    }
    let _ = writeln!(o, r##""/>"##);
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bw_core::character::circle_input;

    const WALK_SPEED: f32 = 1.35;

    fn fixture(count: usize) -> (Vec<RenderWalker>, Skeleton) {
        let skel = Skeleton::humanoid();
        let walkers = (0..count)
            .map(|_| {
                RenderWalker::new(
                    Character::new(Vec3::new(0.0, 0.0, 5.0), std::f32::consts::FRAC_PI_2),
                    circle_input(5.0, WALK_SPEED, 1.0),
                    &skel,
                )
            })
            .collect();
        (walkers, skel)
    }

    fn write_to_temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("bw-walker-gallery-test");
        std::fs::create_dir_all(&dir).ok();
        dir.join(name)
    }

    #[test]
    fn animation_writes_a_multi_frame_svg() -> Result<(), String> {
        let (mut walkers, skel) = fixture(1);
        let meta = RenderMeta {
            scene: "walk",
            seed: 42,
            fps: 12,
        };
        let path = write_to_temp("anim.svg");
        render_animation(
            &mut walkers,
            &skel,
            &meta,
            1.5,
            path.to_str().ok_or("utf-8")?,
        )?;
        let svg = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        assert!(svg.contains("<svg"), "missing svg root");
        assert!(svg.contains("<animate"), "no SMIL animation");
        assert!(svg.contains("repeatCount=\"indefinite\""));
        assert!(svg.contains("<circle"), "no head ball");
        assert!(svg.contains("procedural gait"), "no caption provenance");
        Ok(())
    }

    #[test]
    fn static_frame_has_no_animation() -> Result<(), String> {
        let (mut walkers, skel) = fixture(1);
        let meta = RenderMeta {
            scene: "walk",
            seed: 42,
            fps: 1,
        };
        let path = write_to_temp("frame.svg");
        render_frame(
            &mut walkers,
            &skel,
            &meta,
            1.0,
            path.to_str().ok_or("utf-8")?,
        )?;
        let svg = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        assert!(svg.contains("<svg"));
        assert!(!svg.contains("<animate"), "static frame must not animate");
        Ok(())
    }

    #[test]
    fn empty_scene_is_rejected() {
        let (_, skel) = fixture(0);
        let mut empty: Vec<RenderWalker> = Vec::new();
        let meta = RenderMeta {
            scene: "walk",
            seed: 1,
            fps: 2,
        };
        assert!(render_animation(&mut empty, &skel, &meta, 1.0, "/dev/null").is_err());
    }

    /// The target projects to the screen origin; closer points project
    /// larger; points behind the eye clip to None.
    #[test]
    fn camera_projects_target_to_origin_and_scales_with_depth() {
        let cam = Camera::looking(EYE, TARGET, FOV_Y_DEG, 900.0);
        let (tx, ty) = cam.project(TARGET).unwrap_or((f32::MAX, f32::MAX));
        assert!(
            tx.abs() < 1e-3 && ty.abs() < 1e-3,
            "target off-center: {tx} {ty}"
        );
        let (nx, _) = cam
            .project(TARGET + Vec3::new(1.0, 0.0, 0.0))
            .unwrap_or((f32::MAX, f32::MAX));
        let along_view = EYE - TARGET; // away from the target = farther out
        let (fx, _) = cam
            .project(TARGET + Vec3::new(1.0, 0.0, 0.0) + along_view)
            .unwrap_or((f32::MIN, f32::MIN));
        assert!(nx > fx, "closer points must project larger: {nx} vs {fx}");
        let Some(forward) = (TARGET - EYE).normalized() else {
            panic!("camera axis must normalize");
        };
        assert!(
            cam.project(EYE - forward).is_none(),
            "points behind the camera must clip"
        );
    }

    /// Ground shadows land on the ground plane.
    #[test]
    fn shadow_projection_lands_on_ground() {
        let p = Vec3::new(0.3, 1.4, -0.7);
        let g = ground_shadow(p);
        assert!(g.y.abs() < 1e-5, "shadow at y={}", g.y);
        let g0 = ground_shadow(Vec3::new(0.3, 0.0, -0.7));
        assert_eq!(g0, Vec3::new(0.3, 0.0, -0.7), "ground points are fixed");
    }
}
