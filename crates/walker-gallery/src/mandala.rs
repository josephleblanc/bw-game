//! The mandala stress scenes: 100 / 1,000 / 10,000 walkers on
//! interlocking loops — the crowd scene scaled until the sim has to
//! earn it. No new sim code: every figure is a plain [`CirclePath`]
//! walker, so the measured path (and its zero-allocation proof, ADR
//! 0004) is exactly the crowd's.
//!
//! ## The choreography (a painting that walks)
//!
//! At `t = 0` the figures tile a canvas at a fixed density — each one a
//! single-color tile of a procedural painting (the viewer's design
//! function paints it; see `mandala_view`). Then every figure walks a
//! circle of **one shared radius `r` at one shared speed** — but the
//! circles' centers are scattered across the canvas (a jittered
//! sunflower distribution), with directions alternating and phases
//! drawn per loop_spec. The result:
//!
//! - the loops **interlock and cross** — a dense mesh of arcs with no
//!   empty gaps between "rings", and counter-rotating pairs stream
//!   through every shared intersection: near misses, constantly;
//! - the painting **dissolves** — a blue figure wanders at most `2r`
//!   from its tile, but that carries it across other shapes'
//!   territory, and its loop_spec-mates are strangers of every color;
//! - the painting **re-forms exactly**: same radius and speed give
//!   every loop_spec the same period `T₀ = 2πr/s` (direction only flips the
//!   sign of rotation), so at every multiple of `T₀` all figures are
//!   back on their tiles simultaneously. The alignment is periodic,
//!   not gradual — nothing slides into place; it snaps.
//!
//! Deterministic per `(count, seed)`: the sunflower centers, jitter,
//! phases, and loop_spec populations all draw from the seeded RNG, the same
//! contract as the other scenes.
//!
//! ## Gait phase
//!
//! Each figure's gait phase equals its starting angle on its loop_spec, so
//! footfalls ripple around every loop_spec and re-phase with the image at
//! each re-formation.

use std::f32::consts::PI;
use std::f32::consts::TAU;

use bw_core::character::Character;
use bw_core::math::Vec3;
use bw_core::sim::Rng;

use crate::CirclePath;
use crate::SpawnSpec;

/// One loop_spec of the field: center, direction, starting phase, walker
/// count. Every loop_spec shares the field's radius and speed — that shared
/// period is what re-forms the image.
#[derive(Debug, Clone, Copy)]
pub struct LoopSpec {
    /// Loop center (on the ground plane).
    pub center: Vec3,
    /// +1 counterclockwise, −1 clockwise (alternates per loop_spec, so the
    /// field mixes directions at every scale).
    pub dir: f32,
    /// Phase of walker 0 at t = 0 (rad).
    pub phase0: f32,
    /// Walkers on this loop_spec, evenly spaced.
    pub walkers: usize,
}

/// A fully resolved field: loops plus the numbers readouts and framing
/// need.
#[derive(Debug, Clone)]
pub struct MandalaSpec {
    pub loops: Vec<LoopSpec>,
    /// Shared walking speed (m/s).
    pub speed: f32,
    /// Shared loop_spec radius (m).
    pub radius: f32,
    /// Canvas radius (m): every figure stays inside it for all t.
    /// Frames the viewer's camera and bounds the layout (read by the
    /// viewer and the layout's own tests; the headless harness only
    /// writes it).
    #[cfg_attr(
        all(not(feature = "viewer"), not(test)),
        expect(dead_code, reason = "viewer/test-only framing bound")
    )]
    pub canvas: f32,
    /// The re-formation period `T₀ = 2πr/s`: at every multiple, all
    /// figures are back on their t = 0 tiles.
    #[cfg_attr(
        all(not(feature = "viewer"), not(test)),
        expect(dead_code, reason = "viewer-only readout input")
    )]
    pub period: f32,
}

impl MandalaSpec {
    pub fn walker_total(&self) -> usize {
        self.loops.iter().map(|l| l.walkers).sum()
    }
}

/// Population tiers: tile spacing (m between loop_spec-mates, which sets the
/// canvas size for the count) and shared walking speed.
fn tier(count: usize) -> (f32, f32) {
    if count < 300 {
        (0.85, 1.35)
    } else if count < 3_000 {
        (1.0, 1.8)
    } else {
        (1.1, 2.2)
    }
}

/// The golden angle (rad): consecutive sunflower centers land maximally
/// far apart — even coverage with no seed dependence in the base
/// layout.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// Build the field for exactly `count` walkers, seeded. The canvas
/// grows with the population at fixed tile density; the loop_spec radius is
/// a fixed fraction of it (big enough that figures cross into other
/// shapes' territory — full dispersal — small enough that the swarm
/// stays framed).
pub fn build(count: usize, seed: u64) -> MandalaSpec {
    let (spacing, speed) = tier(count);
    let canvas = (count as f32 * spacing * spacing / PI).sqrt();
    let radius = canvas * 0.40;
    let period = TAU * radius / speed;

    // Loops of ~2πr/spacing walkers each, patched ±1 to hit the count
    // exactly (no symmetry constraint — any count works).
    let per_loop = ((TAU * radius / spacing).round() as usize).max(3);
    let loop_count = count.div_ceil(per_loop);
    let base = count / loop_count;
    let extra = count % loop_count;

    let center_disc = (canvas - radius).max(radius * 0.2);
    let step = (PI * center_disc * center_disc / loop_count as f32).sqrt();
    let mut rng = Rng::new(seed);
    let loops = (0..loop_count)
        .map(|m| {
            let t = (m as f32 + 0.5) / loop_count as f32;
            let rr = center_disc * t.sqrt();
            let aa = m as f32 * GOLDEN_ANGLE;
            let jx = rng.range_f32(-0.25, 0.25) * step;
            let jz = rng.range_f32(-0.25, 0.25) * step;
            // Jitter must not push an outer loop's center past the
            // disc: every figure would leave the canvas.
            let (mut cx, mut cz) = (rr * aa.sin() + jx, rr * aa.cos() + jz);
            let cd = (cx * cx + cz * cz).sqrt();
            if cd > center_disc {
                let k = center_disc / cd;
                cx *= k;
                cz *= k;
            }
            LoopSpec {
                center: Vec3::new(cx, 0.0, cz),
                dir: if m.is_multiple_of(2) { 1.0 } else { -1.0 },
                phase0: rng.range_f32(0.0, TAU),
                walkers: base + usize::from(m < extra),
            }
        })
        .collect();
    MandalaSpec {
        loops,
        speed,
        radius,
        canvas,
        period,
    }
}

/// Scene-id → population for the mandala stress scenes.
pub fn preset(id: &str) -> Option<usize> {
    match id {
        "mandala-100" => Some(100),
        "mandala-1k" => Some(1_000),
        "mandala-10k" => Some(10_000),
        _ => None,
    }
}

/// One figure: slot `j` of `loop_spec`, walking that loop_spec's circle. The
/// single construction point shared by every host (harness, SVG,
/// viewer).
pub fn figure(loop_spec: &LoopSpec, j: usize, radius: f32, speed: f32) -> SpawnSpec {
    let angle = loop_spec.phase0 + TAU * j as f32 / loop_spec.walkers as f32;
    let offset = Vec3::new(radius * angle.sin(), 0.0, radius * angle.cos());
    let mut character = Character::new(
        loop_spec.center + offset,
        angle + loop_spec.dir * std::f32::consts::FRAC_PI_2,
    );
    character.speed = 0.0; // eased in by the first ticks
    character.phase = angle; // footfalls ripple around the loop_spec
    SpawnSpec {
        character,
        path: CirclePath {
            radius,
            speed,
            dir: loop_spec.dir,
        },
    }
}

/// Spawn specs for the field, loop_spec-major. Shared by the headless
/// harness, the SVG path, and the viewer.
pub fn spawn_specs(mandala: &MandalaSpec) -> Vec<SpawnSpec> {
    let mut specs = Vec::with_capacity(mandala.walker_total());
    for loop_spec in &mandala.loops {
        for j in 0..loop_spec.walkers {
            specs.push(figure(loop_spec, j, mandala.radius, mandala.speed));
        }
    }
    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIERS: [usize; 3] = [100, 1_000, 10_000];

    /// Every tier hits its population exactly — the stress claim ("this
    /// scene has N characters") must not be approximate.
    #[test]
    fn tiers_hit_exact_populations() {
        for &count in &TIERS {
            assert_eq!(build(count, 42).walker_total(), count, "population {count}");
        }
    }

    /// The field is dense and framed: t = 0 positions pack the canvas,
    /// every figure stays inside it, and loops carry a sane walker
    /// count (the tile spacing contract).
    #[test]
    fn tiles_pack_the_canvas() {
        for &count in &TIERS {
            let m = build(count, 42);
            let specs = spawn_specs(&m);
            assert_eq!(specs.len(), count);
            let mut mean_d = 0.0f32;
            for spec in &specs {
                let p = spec.character.pos;
                let d = (p.x * p.x + p.z * p.z).sqrt();
                assert!(
                    d <= m.canvas + 1e-3,
                    "figure outside the canvas: {d} > {}",
                    m.canvas
                );
                mean_d += d;
            }
            // Individual figures may pass near the origin (a loop can
            // enclose it — good coverage); the *population* must not be
            // collapsed into the middle.
            mean_d /= specs.len() as f32;
            assert!(
                mean_d > m.canvas * 0.4,
                "population collapsed toward the center: mean {mean_d}"
            );
            for loop_spec in &m.loops {
                assert!(
                    loop_spec.walkers >= 3,
                    "loop_spec too small to read as a loop_spec"
                );
                assert!(loop_spec.dir.abs() == 1.0);
                let cd = (loop_spec.center.x * loop_spec.center.x
                    + loop_spec.center.z * loop_spec.center.z)
                    .sqrt();
                assert!(cd <= m.canvas, "loop_spec center outside the canvas");
            }
            assert!(m.loops.iter().any(|l| l.dir > 0.0));
            assert!(m.loops.iter().any(|l| l.dir < 0.0));
            assert!((0.9..=2.4).contains(&m.speed), "speed {}", m.speed);
        }
    }

    /// The same radius and speed for every loop_spec make each loop_spec's
    /// rotation rigid — the whole field is periodic with one period.
    /// Checked through the real `Character` stepper: at half period
    /// every figure stands across its loop_spec (fully dispersed), at the
    /// full period every figure is back on its tile (the painting
    /// re-forms).
    #[test]
    fn the_painting_dissolves_and_reforms() {
        use bw_core::character::circle_input;
        use bw_core::time::SIM_DT;

        let m = build(100, 42);
        let mut walkers: Vec<Character> = spawn_specs(&m)
            .into_iter()
            .map(|spec| spec.character)
            .collect();
        let inputs: Vec<_> = m
            .loops
            .iter()
            .map(|l| circle_input(m.radius, m.speed, l.dir))
            .collect();
        // Loop index per walker (specs are loop_spec-major).
        let mut loop_of = Vec::with_capacity(walkers.len());
        for (k, loop_spec) in m.loops.iter().enumerate() {
            for _ in 0..loop_spec.walkers {
                loop_of.push(k);
            }
        }
        let starts: Vec<Vec3> = walkers.iter().map(|w| w.pos).collect();

        let run_for = |walkers: &mut Vec<Character>, secs: f32| {
            for _ in 0..(secs / SIM_DT) as usize {
                for (w, &k) in walkers.iter_mut().zip(&loop_of) {
                    w.step(SIM_DT, &inputs[k]);
                }
            }
        };

        // Half period: every figure diametrically across its loop_spec.
        run_for(&mut walkers, m.period / 2.0);
        for (w, start) in walkers.iter().zip(&starts) {
            let d = (w.pos - *start).length();
            assert!(
                d > m.radius * 1.5,
                "not dispersed at T₀/2: displacement {d} vs radius {}",
                m.radius
            );
        }

        // Full period: back on the tile (a small launch-transient smear
        // — speed eases in over the first ~0.3 s — is expected and
        // sub-tile).
        run_for(&mut walkers, m.period / 2.0);
        for (w, start) in walkers.iter().zip(&starts) {
            let d = (w.pos - *start).length();
            assert!(
                d < m.radius * 0.3,
                "did not re-form at T₀: displacement {d} vs radius {}",
                m.radius
            );
        }
    }

    /// The seed enters the layout (jitter, phases) but not the
    /// population: same seed reproduces, different seeds diverge.
    #[test]
    fn seeds_shape_the_field() {
        let positions = |seed: u64| -> Vec<Vec3> {
            spawn_specs(&build(1_000, seed))
                .into_iter()
                .map(|spec| spec.character.pos)
                .collect()
        };
        let a = positions(7);
        let b = positions(7);
        let c = positions(8);
        assert_eq!(a.len(), b.len());
        assert_eq!(a.len(), c.len());
        assert_eq!(a, b, "same seed must reproduce");
        assert_ne!(a, c, "different seed must diverge");
    }

    /// Scene ids map to the three populations.
    #[test]
    fn presets_map_ids() {
        assert_eq!(preset("mandala-100"), Some(100));
        assert_eq!(preset("mandala-1k"), Some(1_000));
        assert_eq!(preset("mandala-10k"), Some(10_000));
        assert_eq!(preset("walk"), None);
    }

    /// The field walks deterministically through the real plugin: same
    /// (scene, seed, ticks), same checksum; one more tick moves it.
    #[test]
    fn mandala_walks_deterministically_through_the_plugin() {
        use crate::{CharacterMovementPlugin, WalkerSkeleton, spawn_walker, state_checksum};
        use bevy::app::{App, FixedUpdate};

        let run = |ticks: u32, seed: u64| -> (u64, u64) {
            let mut app = App::new();
            app.add_plugins(CharacterMovementPlugin)
                .insert_resource(WalkerSkeleton(bw_core::character::Skeleton::humanoid()));
            let skel = app.world().resource::<WalkerSkeleton>().0.clone();
            for spec in spawn_specs(&build(100, seed)) {
                let input = bw_core::character::circle_input(
                    spec.path.radius,
                    spec.path.speed,
                    spec.path.dir,
                );
                let entity = spawn_walker(app.world_mut(), &skel, spec.character, input);
                app.world_mut().entity_mut(entity).insert(spec.path);
            }
            for _ in 0..ticks {
                assert!(app.world_mut().try_run_schedule(FixedUpdate).is_ok());
                app.update();
            }
            (
                state_checksum(app.world_mut()),
                crate::total_footfalls(app.world_mut()),
            )
        };
        let a = run(600, 42);
        let b = run(600, 42);
        assert_eq!(a, b, "same (scene, seed, ticks) must reproduce");
        let c = run(601, 42);
        assert_ne!(a.0, c.0, "one more tick must move it");
        let d = run(600, 43);
        assert_ne!(a.0, d.0, "the seed must enter the layout");
        assert!(a.1 > 90, "100 walking figures must land footfalls");
    }
}
