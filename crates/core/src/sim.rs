//! Deterministic simulation core: a seeded RNG, a structure-of-arrays
//! particle simulation, and a uniform-grid spatial hash for broadphase
//! pair queries. Pure Rust, engine-agnostic (ADR 0002): the same code runs
//! under Bevy in the gallery binaries and under criterion/iai-callgrind in
//! the benches.

use std::collections::HashMap;

/// splitmix64 — small, fast, deterministic. Adequate for scene generation;
/// not cryptographic.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / 16_777_216.0
    }

    /// Uniform in `[min, max)`.
    pub fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        min + self.next_f32() * (max - min)
    }
}

/// Bouncing-particle simulation in a `[0, width] x [0, height]` box,
/// stored as structure-of-arrays for cache-friendly stepping.
#[derive(Debug, Clone)]
pub struct Sim {
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
    pub vxs: Vec<f32>,
    pub vys: Vec<f32>,
    pub width: f32,
    pub height: f32,
}

impl Sim {
    /// Deterministic scene: same `(n, seed, width, height)` always produces
    /// the same state (unit-tested).
    pub fn seeded(n: usize, seed: u64, width: f32, height: f32) -> Self {
        let mut rng = Rng::new(seed);
        let mut sim = Self {
            xs: Vec::with_capacity(n),
            ys: Vec::with_capacity(n),
            vxs: Vec::with_capacity(n),
            vys: Vec::with_capacity(n),
            width,
            height,
        };
        for _ in 0..n {
            sim.xs.push(rng.range_f32(0.0, width));
            sim.ys.push(rng.range_f32(0.0, height));
            sim.vxs.push(rng.range_f32(-30.0, 30.0));
            sim.vys.push(rng.range_f32(-30.0, 30.0));
        }
        sim
    }

    pub fn len(&self) -> usize {
        self.xs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.xs.is_empty()
    }

    /// Advance one tick. Pure float arithmetic over the arrays: no
    /// allocation, no wall-clock dependence, no iteration-order surprises.
    pub fn step(&mut self, dt: f32) {
        for i in 0..self.xs.len() {
            let (x, vx) = reflect(self.xs[i] + self.vxs[i] * dt, self.vxs[i], 0.0, self.width);
            let (y, vy) = reflect(self.ys[i] + self.vys[i] * dt, self.vys[i], 0.0, self.height);
            self.xs[i] = x;
            self.ys[i] = y;
            self.vxs[i] = vx;
            self.vys[i] = vy;
        }
    }

    /// Order-independent digest of the full state, for determinism checks.
    pub fn state_checksum(&self) -> u64 {
        fn mix(hash: u64, values: &[f32]) -> u64 {
            values.iter().fold(hash, |h, v| {
                h.wrapping_mul(0x1000_0000_01B3) ^ (v.to_bits() as u64)
            })
        }
        let mut h = 0xCBF2_9CE4_8422_2325;
        h = mix(h, &self.xs);
        h = mix(h, &self.ys);
        h = mix(h, &self.vxs);
        h = mix(h, &self.vys);
        h
    }
}

/// Integrate with wall reflection: returns the clamped position and the
/// (possibly negated) velocity.
fn reflect(pos: f32, vel: f32, min: f32, max: f32) -> (f32, f32) {
    if pos < min {
        (min, vel.abs())
    } else if pos > max {
        (max, -vel.abs())
    } else {
        (pos, vel)
    }
}

/// Uniform-grid spatial hash over point positions, for broadphase
/// neighbor queries.
#[derive(Debug, Clone)]
pub struct SpatialHash {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<usize>>,
}

impl SpatialHash {
    /// Build the grid. Insertion order is entity-index order, which makes
    /// query output deterministic despite `HashMap`'s internal ordering:
    /// queries iterate entities, never the map.
    pub fn build(xs: &[f32], ys: &[f32], cell: f32) -> Self {
        assert!(cell > 0.0, "cell size must be positive");
        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::with_capacity(xs.len());
        for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
            cells
                .entry((key(*x, cell), key(*y, cell)))
                .or_default()
                .push(i);
        }
        Self { cell, cells }
    }

    /// All pairs `(i, j)` with `i < j` and distance <= `radius`, using the
    /// 3x3 cell neighborhood per entity.
    pub fn query_pairs(&self, xs: &[f32], ys: &[f32], radius: f32) -> Vec<(usize, usize)> {
        let r2 = radius * radius;
        let mut pairs = Vec::new();
        for i in 0..xs.len() {
            let (cx, cy) = (key(xs[i], self.cell), key(ys[i], self.cell));
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let Some(bucket) = self.cells.get(&(cx + dx, cy + dy)) else {
                        continue;
                    };
                    for &j in bucket {
                        if j <= i {
                            continue;
                        }
                        let (ex, ey) = (xs[i] - xs[j], ys[i] - ys[j]);
                        if ex * ex + ey * ey <= r2 {
                            pairs.push((i, j));
                        }
                    }
                }
            }
        }
        pairs
    }
}

fn key(value: f32, cell: f32) -> i32 {
    value.div_euclid(cell) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_varied() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut c = Rng::new(43);
        assert_ne!(a.next_u64(), c.next_u64());
    }

    #[test]
    fn rng_ranges_are_bounded() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v));
            let r = rng.range_f32(-30.0, 30.0);
            assert!((-30.0..30.0).contains(&r));
        }
    }

    #[test]
    fn sim_stepping_is_deterministic() {
        let mut a = Sim::seeded(500, 42, 100.0, 100.0);
        let mut b = Sim::seeded(500, 42, 100.0, 100.0);
        for _ in 0..600 {
            a.step(1.0 / 60.0);
            b.step(1.0 / 60.0);
        }
        assert_eq!(a.state_checksum(), b.state_checksum());
    }

    #[test]
    fn sim_reflects_off_walls() {
        let mut sim = Sim {
            xs: vec![99.9],
            ys: vec![50.0],
            vxs: vec![10.0],
            vys: vec![0.0],
            width: 100.0,
            height: 100.0,
        };
        sim.step(0.5); // moves to 104.9 -> clamped, velocity negated
        assert_eq!(sim.xs[0], 100.0);
        assert_eq!(sim.vxs[0], -10.0);
    }

    #[test]
    fn sim_keeps_entities_inside_bounds() {
        let mut sim = Sim::seeded(200, 1, 64.0, 32.0);
        for _ in 0..2000 {
            sim.step(1.0 / 60.0);
        }
        for i in 0..sim.len() {
            assert!((0.0..=64.0).contains(&sim.xs[i]));
            assert!((0.0..=32.0).contains(&sim.ys[i]));
        }
    }

    #[test]
    fn hash_finds_close_pairs_and_no_far_ones() {
        let xs = vec![0.0, 0.5, 50.0];
        let ys = vec![0.0, 0.5, 50.0];
        let hash = SpatialHash::build(&xs, &ys, 4.0);
        let mut pairs = hash.query_pairs(&xs, &ys, 1.0);
        pairs.sort_unstable();
        assert_eq!(pairs, vec![(0, 1)]);
    }

    #[test]
    fn hash_query_is_deterministic() {
        let sim = Sim::seeded(300, 9, 50.0, 50.0);
        let hash = SpatialHash::build(&sim.xs, &sim.ys, 2.0);
        let a = hash.query_pairs(&sim.xs, &sim.ys, 1.5);
        let b = hash.query_pairs(&sim.xs, &sim.ys, 1.5);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_matches_bruteforce() {
        let sim = Sim::seeded(120, 3, 30.0, 30.0);
        let hash = SpatialHash::build(&sim.xs, &sim.ys, 2.0);
        let mut via_hash = hash.query_pairs(&sim.xs, &sim.ys, 1.0);
        via_hash.sort_unstable();
        let mut bruteforce = Vec::new();
        for i in 0..sim.len() {
            for j in (i + 1)..sim.len() {
                let (ex, ey) = (sim.xs[i] - sim.xs[j], sim.ys[i] - sim.ys[j]);
                if ex * ex + ey * ey <= 1.0 {
                    bruteforce.push((i, j));
                }
            }
        }
        assert_eq!(via_hash, bruteforce);
    }
}
