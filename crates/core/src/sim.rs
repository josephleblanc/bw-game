//! Deterministic simulation core: a seeded RNG, a structure-of-arrays
//! particle simulation, and a dense spatial grid for broadphase pair
//! queries. Pure Rust, engine-agnostic (ADR 0002): the same code runs
//! under Bevy in the gallery binaries and under criterion/iai-callgrind in
//! the benches.

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

/// Dense uniform-grid broadphase over a fixed `[0, width] x [0, height]`
/// arena, rebuilt in place: counting-sort storage is allocated once at
/// construction (growing only if the entity count grows), so steady-state
/// rebuilds and queries allocate nothing (ADR 0004).
#[derive(Debug, Clone)]
pub struct SpatialGrid {
    cell: f32,
    cols: usize,
    rows: usize,
    /// Per-cell entity counts, zeroed at the start of every rebuild.
    counts: Vec<u32>,
    /// Prefix sum of `counts`; cell k's entries live in
    /// `entries[cell_start[k]..cell_start[k + 1]]`.
    cell_start: Vec<u32>,
    /// Placement cursor over `cell_start` during the second pass.
    cursor: Vec<u32>,
    /// Entity indices, sorted by cell.
    entries: Vec<u32>,
}

impl SpatialGrid {
    pub fn new(width: f32, height: f32, cell: f32, capacity: usize) -> Self {
        assert!(cell > 0.0, "cell size must be positive");
        assert!(
            width.is_finite() && width >= 0.0,
            "arena width must be finite"
        );
        assert!(
            height.is_finite() && height >= 0.0,
            "arena height must be finite"
        );
        let cols = ((width / cell).ceil() as usize).max(1);
        let rows = ((height / cell).ceil() as usize).max(1);
        let Some(cells) = cols.checked_mul(rows).filter(|c| *c <= u32::MAX as usize) else {
            panic!("arena of {cols}x{rows} cells overflows the dense grid");
        };
        Self {
            cell,
            cols,
            rows,
            counts: vec![0; cells],
            cell_start: vec![0; cells + 1],
            cursor: vec![0; cells],
            entries: Vec::with_capacity(capacity),
        }
    }

    fn cell_coords(&self, x: f32, y: f32) -> (usize, usize) {
        // Clamp out-of-arena positions onto the border cells: `Sim` keeps
        // positions inside the arena, so this is a safety net, not a
        // semantic branch.
        let cx = ((x / self.cell).floor() as i64).clamp(0, (self.cols - 1) as i64) as usize;
        let cy = ((y / self.cell).floor() as i64).clamp(0, (self.rows - 1) as i64) as usize;
        (cx, cy)
    }

    /// Rebuild the grid in place: two passes over the entities plus a
    /// prefix sum over cells. Storage resizes only when the entity count
    /// grows; steady-state rebuilds allocate nothing.
    pub fn rebuild(&mut self, xs: &[f32], ys: &[f32]) {
        assert_eq!(xs.len(), ys.len(), "position slices must have equal length");
        if self.entries.len() != xs.len() {
            self.entries.resize(xs.len(), 0);
        }
        self.counts.fill(0);
        for i in 0..xs.len() {
            let (cx, cy) = self.cell_coords(xs[i], ys[i]);
            self.counts[cy * self.cols + cx] += 1;
        }
        let mut running = 0u32;
        for (start, &count) in self.cell_start.iter_mut().zip(&self.counts) {
            *start = running;
            running += count;
        }
        self.cell_start[self.counts.len()] = running;
        self.cursor
            .copy_from_slice(&self.cell_start[..self.counts.len()]);
        for i in 0..xs.len() {
            let (cx, cy) = self.cell_coords(xs[i], ys[i]);
            let c = cy * self.cols + cx;
            self.entries[self.cursor[c] as usize] = i as u32;
            self.cursor[c] += 1;
        }
    }

    /// Append all pairs `(i, j)` with `i < j` and distance <= `radius` to
    /// `out` (cleared first), scanning each entity's 3x3 cell
    /// neighborhood. Correct for `radius <= cell`. Zero allocation at
    /// steady state when `out` is a reused scratch buffer.
    pub fn query_pairs_into(&self, xs: &[f32], ys: &[f32], radius: f32, out: &mut Vec<(u32, u32)>) {
        assert!(
            radius <= self.cell,
            "radius {radius} exceeds cell {} — the 3x3 neighborhood would miss pairs",
            self.cell
        );
        assert_eq!(xs.len(), ys.len(), "position slices must have equal length");
        let r2 = radius * radius;
        out.clear();
        for i in 0..xs.len() {
            let (cx, cy) = self.cell_coords(xs[i], ys[i]);
            let y_lo = cy.saturating_sub(1);
            let y_hi = (cy + 1).min(self.rows - 1);
            let x_lo = cx.saturating_sub(1);
            let x_hi = (cx + 1).min(self.cols - 1);
            for gy in y_lo..=y_hi {
                for gx in x_lo..=x_hi {
                    let c = gy * self.cols + gx;
                    for k in self.cell_start[c] as usize..self.cell_start[c + 1] as usize {
                        let j = self.entries[k];
                        if j <= i as u32 {
                            continue;
                        }
                        let (ex, ey) = (xs[i] - xs[j as usize], ys[i] - ys[j as usize]);
                        if ex * ex + ey * ey <= r2 {
                            out.push((i as u32, j));
                        }
                    }
                }
            }
        }
    }

    /// Convenience wrapper allocating its own output — for tests and
    /// off-hot-path callers. Per-frame paths use `query_pairs_into` with a
    /// reused scratch (ADR 0004).
    pub fn query_pairs(&self, xs: &[f32], ys: &[f32], radius: f32) -> Vec<(u32, u32)> {
        let mut pairs = Vec::new();
        self.query_pairs_into(xs, ys, radius, &mut pairs);
        pairs
    }
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
            a.step(crate::time::SIM_DT);
            b.step(crate::time::SIM_DT);
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
            sim.step(crate::time::SIM_DT);
        }
        for i in 0..sim.len() {
            assert!((0.0..=64.0).contains(&sim.xs[i]));
            assert!((0.0..=32.0).contains(&sim.ys[i]));
        }
    }

    #[test]
    fn grid_finds_close_pairs_and_no_far_ones() {
        let xs = vec![0.0, 0.5, 50.0];
        let ys = vec![0.0, 0.5, 50.0];
        let mut grid = SpatialGrid::new(64.0, 64.0, 4.0, 3);
        grid.rebuild(&xs, &ys);
        let mut pairs = grid.query_pairs(&xs, &ys, 1.0);
        pairs.sort_unstable();
        assert_eq!(pairs, vec![(0, 1)]);
    }

    #[test]
    fn grid_query_is_deterministic() {
        let sim = Sim::seeded(300, 9, 50.0, 50.0);
        let mut grid = SpatialGrid::new(50.0, 50.0, 2.0, sim.len());
        grid.rebuild(&sim.xs, &sim.ys);
        let a = grid.query_pairs(&sim.xs, &sim.ys, 1.5);
        let b = grid.query_pairs(&sim.xs, &sim.ys, 1.5);
        assert_eq!(a, b);
    }

    #[test]
    fn grid_matches_bruteforce() {
        let sim = Sim::seeded(120, 3, 30.0, 30.0);
        let mut grid = SpatialGrid::new(30.0, 30.0, 2.0, sim.len());
        grid.rebuild(&sim.xs, &sim.ys);
        let mut via_grid = grid.query_pairs(&sim.xs, &sim.ys, 1.0);
        via_grid.sort_unstable();
        let mut bruteforce = Vec::new();
        for i in 0..sim.len() {
            for j in (i + 1)..sim.len() {
                let (ex, ey) = (sim.xs[i] - sim.xs[j], sim.ys[i] - sim.ys[j]);
                if ex * ex + ey * ey <= 1.0 {
                    bruteforce.push((i as u32, j as u32));
                }
            }
        }
        assert_eq!(via_grid, bruteforce);
    }

    /// The reuse contract: rebuilding over new positions must produce
    /// exactly what a freshly constructed grid produces, twice in a row —
    /// counting-sort state must not leak between rebuilds.
    #[test]
    fn grid_rebuild_reuse_matches_fresh_construction() {
        let mut sim = Sim::seeded(400, 5, 40.0, 40.0);
        let mut reused = SpatialGrid::new(40.0, 40.0, 1.0, sim.len());
        for _ in 0..50 {
            sim.step(crate::time::SIM_DT);
            reused.rebuild(&sim.xs, &sim.ys);
            let mut fresh = SpatialGrid::new(40.0, 40.0, 1.0, sim.len());
            fresh.rebuild(&sim.xs, &sim.ys);
            let mut a = reused.query_pairs(&sim.xs, &sim.ys, 1.0);
            let mut b = fresh.query_pairs(&sim.xs, &sim.ys, 1.0);
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(a, b);
        }
    }

    /// Entity-count growth resizes storage without disturbing results.
    #[test]
    fn grid_grows_with_entity_count() {
        let sim = Sim::seeded(50, 11, 20.0, 20.0);
        let mut grid = SpatialGrid::new(20.0, 20.0, 1.0, 10); // under-sized on purpose
        grid.rebuild(&sim.xs, &sim.ys);
        let before = grid.query_pairs(&sim.xs, &sim.ys, 1.0).len();
        let bigger = Sim::seeded(200, 11, 20.0, 20.0);
        grid.rebuild(&bigger.xs, &bigger.ys);
        let after = grid.query_pairs(&bigger.xs, &bigger.ys, 1.0);
        let mut brute = Vec::new();
        for i in 0..bigger.len() {
            for j in (i + 1)..bigger.len() {
                let (ex, ey) = (bigger.xs[i] - bigger.xs[j], bigger.ys[i] - bigger.ys[j]);
                if ex * ex + ey * ey <= 1.0 {
                    brute.push((i as u32, j as u32));
                }
            }
        }
        let mut sorted = after.clone();
        sorted.sort_unstable();
        brute.sort_unstable();
        assert_eq!(sorted, brute);
        assert!(before <= 50 * 49 / 2); // sanity: pair count within bounds
    }

    /// Out-of-arena positions clamp onto border cells instead of panicking
    /// or wrapping into the wrong neighborhood.
    #[test]
    fn grid_clamps_out_of_arena_positions() {
        let xs = vec![-5.0, 65.0, 10.0];
        let ys = vec![70.0, -1.0, 10.0];
        let mut grid = SpatialGrid::new(64.0, 64.0, 4.0, 3);
        grid.rebuild(&xs, &ys);
        // No panic; far-apart clamped entities produce no pairs.
        assert!(grid.query_pairs(&xs, &ys, 1.0).is_empty());
    }
}
