//! Procedural golden-ratio trees: deterministic generation and a pure
//! time-parameterized pose (growth + wind sway), engine-agnostic per ADR
//! 0002 and allocate-once per ADR 0004 — generation allocates the node
//! arrays once; `pose_into` writes into a reused `TreePose` buffer and
//! never allocates.
//!
//! ## Golden-ratio scheme ("semi-fractal")
//!
//! - The trunk starts as a straight segment pointing straight up.
//! - Every node forks into a near-straight **leader** (slight seeded lean,
//!   slower length decay) and a **side branch** whose angular offset from
//!   the parent is the explement of the golden angle,
//!   `FORK_SPREAD = π − GOLDEN_ANGLE ≈ 42.492°`, with seeded jitter. An
//!   occasional third shoot forks off at a `1/φ`-scaled fraction of the
//!   spread. Because the golden angle is an irrational multiple of π, the
//!   fork pattern never exactly repeats — self-similar at every scale but
//!   never a perfect fractal, which is the look we want.
//! - Side-branch length ratio is `1/φ ≈ 0.618` (± seeded jitter); leaders
//!   decay more slowly so a readable trunk emerges.
//! - Widths taper toward the da Vinci rule of conservation of area
//!   (children thinner than the parent), quantized per depth for grouped
//!   stroke rendering in the galleries.
//!
//! ## Determinism
//!
//! Same `(params, seed)` always generates the same structure, and
//! `pose_into(t)` is a pure function of the structure and `t` (no wall
//! clock, no query-order dependence). `structure_checksum` and
//! `TreePose::checksum` pin both, the same way `sim` does.

use crate::sim::Rng;

/// φ, the golden ratio.
pub const GOLDEN_RATIO: f32 = 1.618_034;
/// The golden angle: `π(2 − φ) ≈ 137.5078°`.
pub const GOLDEN_ANGLE: f32 = 2.399_963_2;
/// Explement of the golden angle (`π − GOLDEN_ANGLE ≈ 42.492°`): the base
/// angular offset of a side branch from its parent direction.
pub const FORK_SPREAD: f32 = std::f32::consts::PI - GOLDEN_ANGLE;

/// Hard safety cap on node count: generous headroom over every current
/// scene preset; generation silently stops adding branches beyond it so a
/// mis-tuned preset cannot run away.
const MAX_NODES: usize = 8_192;

/// Generation parameters. Ranges are documented per field; presets live in
/// the gallery binaries.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeParams {
    /// Length of the trunk segment (world units). Positive.
    pub trunk_len: f32,
    /// Stroke width of the trunk segment. Positive.
    pub trunk_width: f32,
    /// Maximum fork depth (the trunk is depth 0).
    pub max_depth: u8,
    /// Leader length ratio per level (~0.8): keeps a readable trunk.
    pub leader_ratio: f32,
    /// Leader angular jitter, ± radians (~0.06).
    pub leader_jitter: f32,
    /// Side-branch length ratio per level: `1/φ` for the golden scheme.
    pub side_ratio: f32,
    /// Multiplier jitter on every ratio draw (± fraction, ~0.15).
    pub ratio_jitter: f32,
    /// Multiplier jitter on the fork angle (~0.15).
    pub angle_jitter: f32,
    /// Probability of a third shoot at depth >= 2.
    pub extra_prob: f32,
    /// Branches shorter than this are not spawned.
    pub min_len: f32,
    /// Per-branch growth duration (seconds); a child sprouts when its
    /// parent is 55% grown.
    pub grow_dur: f32,
}

impl TreeParams {
    /// The default "oak" preset used by the `tree` gallery scene.
    pub fn oak() -> Self {
        Self {
            trunk_len: 110.0,
            trunk_width: 9.0,
            max_depth: 8,
            leader_ratio: 0.82,
            leader_jitter: 0.06,
            side_ratio: 1.0 / GOLDEN_RATIO,
            ratio_jitter: 0.15,
            angle_jitter: 0.15,
            extra_prob: 0.28,
            min_len: 3.0,
            grow_dur: 0.9,
        }
    }
}

/// Wind parameters: `amp_rad` is the maximum tip bend in radians (0 calms
/// the tree completely); `freq_scale` multiplies the gust frequencies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindParams {
    pub amp_rad: f32,
    pub freq_scale: f32,
}

pub const CALM_WIND: WindParams = WindParams {
    amp_rad: 0.10,
    freq_scale: 1.0,
};

const PARENT_NONE: u32 = u32::MAX;

/// A generated tree: flat node arrays with parents always preceding
/// children (generation order), so the pose pass is a single forward loop.
#[derive(Debug, Clone)]
pub struct Tree {
    parent: Vec<u32>,
    depth: Vec<u8>,
    /// Rest direction offset from the parent's direction (radians). The
    /// root's offset is relative to straight up.
    rest_offset: Vec<f32>,
    len: Vec<f32>,
    width: Vec<f32>,
    sway_phase: Vec<f32>,
    sway_speed: Vec<f32>,
    /// Seconds after scene start when this branch starts growing.
    birth: Vec<f32>,
    /// Rest pose (fully grown, zero wind): absolute angles and endpoints,
    /// computed once at generation. Bounds, canopy, and the structure
    /// checksum read these.
    rest_angle: Vec<f32>,
    rest_sx: Vec<f32>,
    rest_sy: Vec<f32>,
    rest_tx: Vec<f32>,
    rest_ty: Vec<f32>,
    base_x: f32,
    base_y: f32,
    /// `max(birth + grow_dur)` over all nodes.
    growth_end: f32,
    grow_dur: f32,
    max_depth_f32: f32,
    /// Terminal-branch flags (the canopy), for renderers.
    is_leaf: Vec<bool>,
    leaves: u64,
}

impl Tree {
    /// Deterministic generation: same `(params, seed)` always produces the
    /// same structure (unit-tested).
    pub fn generate(params: &TreeParams, seed: u64) -> Self {
        Self::generate_at(params, seed, 0.0, 0.0)
    }

    /// Generate a tree rooted at `(base_x, base_y)` (multiple trees in a
    /// grove share the coordinate space).
    pub fn generate_at(params: &TreeParams, seed: u64, base_x: f32, base_y: f32) -> Self {
        assert!(params.trunk_len > 0.0, "trunk_len must be positive");
        assert!(params.trunk_width > 0.0, "trunk_width must be positive");
        assert!(params.max_depth > 0, "max_depth must be at least 1");
        let mut rng = Rng::new(seed);
        let mut tree = Self {
            parent: Vec::new(),
            depth: Vec::new(),
            rest_offset: Vec::new(),
            len: Vec::new(),
            width: Vec::new(),
            sway_phase: Vec::new(),
            sway_speed: Vec::new(),
            birth: Vec::new(),
            rest_angle: Vec::new(),
            rest_sx: Vec::new(),
            rest_sy: Vec::new(),
            rest_tx: Vec::new(),
            rest_ty: Vec::new(),
            base_x,
            base_y,
            growth_end: 0.0,
            grow_dur: params.grow_dur,
            max_depth_f32: params.max_depth as f32,
            is_leaf: Vec::new(),
            leaves: 0,
        };

        let root = tree.push_node(
            PARENT_NONE,
            0,
            0.0,
            params.trunk_len,
            params.trunk_width,
            0.0,
            &mut rng,
        );
        tree.fork(root, params, &mut rng);

        // Rest pose: fully grown (t = ∞), zero wind. Computed through the
        // same pose pass the hot path uses, then copied into the rest
        // arrays so bounds/checksums never need a live TreePose.
        let mut pose = TreePose::new(&tree);
        tree.pose_into(
            &mut pose,
            f32::INFINITY,
            &WindParams {
                amp_rad: 0.0,
                freq_scale: 1.0,
            },
        );
        tree.rest_angle = pose.angle.clone();
        tree.rest_sx = pose.start_x.clone();
        tree.rest_sy = pose.start_y.clone();
        tree.rest_tx = pose.tip_x.clone();
        tree.rest_ty = pose.tip_y.clone();
        let mut has_child = vec![false; tree.len.len()];
        for &p in &tree.parent {
            if p != PARENT_NONE {
                has_child[p as usize] = true;
            }
        }
        tree.leaves = has_child.iter().filter(|&&c| !c).count() as u64;
        tree.is_leaf = has_child.iter().map(|&c| !c).collect();

        tree
    }

    /// Number of nodes (segments).
    pub fn node_count(&self) -> usize {
        self.len.len()
    }

    /// Terminal branch count (the canopy).
    pub fn leaf_count(&self) -> u64 {
        self.leaves
    }

    /// Seconds after scene start at which the last branch finishes growing.
    pub fn growth_end(&self) -> f32 {
        self.growth_end
    }

    /// Root anchor in world space.
    pub fn base(&self) -> (f32, f32) {
        (self.base_x, self.base_y)
    }

    /// Rest (fully grown, windless) tip positions, for bounds and canopy
    /// layout at render time.
    pub fn rest_tips(&self) -> (&[f32], &[f32]) {
        (&self.rest_tx, &self.rest_ty)
    }

    /// Node depths (the trunk is depth 0).
    pub fn depths(&self) -> &[u8] {
        &self.depth
    }

    /// Rest stroke widths per node.
    pub fn widths(&self) -> &[f32] {
        &self.width
    }

    /// Terminal-branch flags: true where the node has no children.
    pub fn leaf_flags(&self) -> &[bool] {
        &self.is_leaf
    }

    /// Order-independent digest of the generated structure (rest pose).
    pub fn structure_checksum(&self) -> u64 {
        fn mix(hash: u64, values: &[f32]) -> u64 {
            values.iter().fold(hash, |h, v| {
                h.wrapping_mul(0x1000_0000_01B3) ^ (v.to_bits() as u64)
            })
        }
        let mut h = 0xCBF2_9CE4_8422_2325;
        h = mix(h, &self.rest_angle);
        h = mix(h, &self.rest_tx);
        h = mix(h, &self.rest_ty);
        h = mix(h, &self.width);
        h
    }

    /// Write the pose at time `t` (seconds since scene start) into `pose`.
    /// Pure function of `(self, t, wind)`; allocates nothing.
    pub fn pose_into(&self, pose: &mut TreePose, t: f32, wind: &WindParams) {
        assert_eq!(
            pose.len(),
            self.node_count(),
            "pose buffer sized to another tree"
        );
        let gust = wind_shape(t, wind.freq_scale);
        let bend_scale = if wind.amp_rad <= 0.0 {
            0.0
        } else {
            wind.amp_rad * gust
        };
        for i in 0..self.node_count() {
            let (sx, sy, parent_angle) = if self.parent[i] == PARENT_NONE {
                (self.base_x, self.base_y, std::f32::consts::FRAC_PI_2)
            } else {
                let p = self.parent[i] as usize;
                (pose.tip_x[p], pose.tip_y[p], pose.angle[p])
            };
            let flex = (self.depth[i] as f32 / self.max_depth_f32).powi(2);
            // Zero wind must skip the flutter entirely: the rest pose is
            // sampled at t = infinity, where sin(phase + speed * t) is NaN
            // and even 0 * NaN would poison the angle.
            let bend = if bend_scale > 0.0 {
                bend_scale * flex * (self.sway_phase[i] + self.sway_speed[i] * t).sin()
            } else {
                0.0
            };
            let angle = parent_angle + self.rest_offset[i] + bend;
            let g = growth_factor(t - self.birth[i], self.grow_dur);
            pose.angle[i] = angle;
            pose.start_x[i] = sx;
            pose.start_y[i] = sy;
            pose.tip_x[i] = sx + angle.cos() * self.len[i] * g;
            pose.tip_y[i] = sy + angle.sin() * self.len[i] * g;
            pose.width[i] = self.width[i] * g;
        }
    }

    /// Spawn a node and return its index.
    #[allow(clippy::too_many_arguments)] // private recursion builder
    fn push_node(
        &mut self,
        parent: u32,
        depth: u8,
        rest_offset: f32,
        len: f32,
        width: f32,
        birth: f32,
        rng: &mut Rng,
    ) -> usize {
        let i = self.len.len();
        self.parent.push(parent);
        self.depth.push(depth);
        self.rest_offset.push(rest_offset);
        self.len.push(len);
        self.width.push(width);
        self.sway_phase
            .push(rng.range_f32(0.0, std::f32::consts::TAU));
        self.sway_speed.push(rng.range_f32(3.0, 7.0));
        self.birth.push(birth);
        self.growth_end = self.growth_end.max(birth + self.grow_dur);
        i
    }

    /// Fork `node` recursively: leader + side branch (+ occasional third
    /// shoot), depth- and length-capped.
    fn fork(&mut self, node: usize, params: &TreeParams, rng: &mut Rng) {
        if self.depth[node] >= params.max_depth || self.len.len() >= MAX_NODES {
            return;
        }
        let birth = self.birth[node] + 0.55 * params.grow_dur;
        // The side branch alternates sides down each chain, flipped by the
        // RNG often enough to break strict alternation.
        let mut side_sign = if self.depth[node].is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        if rng.next_f32() < 0.25 {
            side_sign = -side_sign;
        }

        // Leader: near-straight continuation, slowly thinning.
        let leader_len = self.len[node]
            * params.leader_ratio
            * rng.range_f32(1.0 - params.ratio_jitter, 1.0 + params.ratio_jitter);
        if leader_len >= params.min_len {
            let offset = params.leader_jitter * rng.range_f32(-1.0, 1.0);
            let leader = self.push_node(
                node as u32,
                self.depth[node] + 1,
                offset,
                leader_len,
                self.width[node] * 0.85,
                birth,
                rng,
            );
            self.fork(leader, params, rng);
        }

        // Side branch: golden fork angle off the parent direction.
        let side_len = self.len[node]
            * params.side_ratio
            * rng.range_f32(1.0 - params.ratio_jitter, 1.0 + params.ratio_jitter);
        if side_len >= params.min_len {
            let offset = side_sign
                * FORK_SPREAD
                * rng.range_f32(1.0 - params.angle_jitter, 1.0 + params.angle_jitter);
            let side = self.push_node(
                node as u32,
                self.depth[node] + 1,
                offset,
                side_len,
                self.width[node] * 0.68,
                birth,
                rng,
            );
            self.fork(side, params, rng);
        }

        // Occasional third shoot at a 1/φ-scaled fraction of the spread,
        // on the other side: the "semi-fractal" spice.
        if self.depth[node] >= 2 && self.len.len() < MAX_NODES && rng.next_f32() < params.extra_prob
        {
            let extra_len = self.len[node] * params.side_ratio / GOLDEN_RATIO
                * rng.range_f32(1.0 - params.ratio_jitter, 1.0 + params.ratio_jitter);
            if extra_len >= params.min_len {
                let offset = -side_sign * FORK_SPREAD / GOLDEN_RATIO
                    * rng.range_f32(1.0 - params.angle_jitter, 1.0 + params.angle_jitter);
                let extra = self.push_node(
                    node as u32,
                    self.depth[node] + 1,
                    offset,
                    extra_len,
                    self.width[node] * 0.55,
                    birth,
                    rng,
                );
                self.fork(extra, params, rng);
            }
        }
    }
}

/// Reused pose buffer for one tree (ADR 0004): allocated once per tree at
/// scene setup, written by `Tree::pose_into` every tick.
#[derive(Debug, Clone)]
pub struct TreePose {
    pub angle: Vec<f32>,
    pub start_x: Vec<f32>,
    pub start_y: Vec<f32>,
    pub tip_x: Vec<f32>,
    pub tip_y: Vec<f32>,
    pub width: Vec<f32>,
}

impl TreePose {
    pub fn new(tree: &Tree) -> Self {
        let n = tree.node_count();
        Self {
            angle: vec![0.0; n],
            start_x: vec![0.0; n],
            start_y: vec![0.0; n],
            tip_x: vec![0.0; n],
            tip_y: vec![0.0; n],
            width: vec![0.0; n],
        }
    }

    pub fn len(&self) -> usize {
        self.tip_x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tip_x.is_empty()
    }

    /// Order-independent digest of the pose (the gallery state checksum).
    pub fn checksum(&self) -> u64 {
        fn mix(hash: u64, values: &[f32]) -> u64 {
            values.iter().fold(hash, |h, v| {
                h.wrapping_mul(0x1000_0000_01B3) ^ (v.to_bits() as u64)
            })
        }
        let mut h = 0xCBF2_9CE4_8422_2325;
        h = mix(h, &self.angle);
        h = mix(h, &self.tip_x);
        h = mix(h, &self.tip_y);
        h = mix(h, &self.width);
        h
    }
}

/// Global gust shape, range ~[-1, 1]: two incommensurate sinusoids so the
/// wind never loops exactly.
fn wind_shape(t: f32, freq_scale: f32) -> f32 {
    0.68 * (std::f32::consts::TAU * 0.32 * freq_scale * t).sin()
        + 0.32 * (std::f32::consts::TAU * 0.19 * freq_scale * t + 1.1).sin()
}

/// Smoothstep growth in [0, 1] for a branch born `age` seconds ago over
/// `dur` seconds of growth.
fn growth_factor(age: f32, dur: f32) -> f32 {
    if dur <= 0.0 {
        return 1.0;
    }
    let u = (age / dur).clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic() {
        let params = TreeParams::oak();
        let a = Tree::generate(&params, 42);
        let b = Tree::generate(&params, 42);
        assert_eq!(a.node_count(), b.node_count());
        assert_eq!(a.structure_checksum(), b.structure_checksum());
        let c = Tree::generate(&params, 43);
        assert_ne!(a.structure_checksum(), c.structure_checksum());
    }

    /// The trunk is a straight line going straight up: the root's rest
    /// angle is exactly π/2 and its leader descendants stay near vertical
    /// for the first few levels.
    #[test]
    fn trunk_starts_straight_up() {
        let tree = Tree::generate(&TreeParams::oak(), 7);
        assert_eq!(tree.rest_angle[0], std::f32::consts::FRAC_PI_2);
        // Walk the leader chain (first child of each node, which generation
        // order guarantees) and check the cumulative lean stays small.
        let mut node = 0usize;
        for _ in 0..3 {
            node += 1; // first pushed child is the leader
            let lean = (tree.rest_angle[node] - std::f32::consts::FRAC_PI_2).abs();
            assert!(
                lean < 0.35,
                "leader at depth {} leans {lean} rad — trunk not straight",
                tree.depth[node]
            );
        }
    }

    /// Side branches fork at the golden spread: their rest offsets land in
    /// a jitter band around ±FORK_SPREAD, never near 0 and never beyond
    /// ~1.35× the spread.
    #[test]
    fn side_branches_use_the_golden_spread() {
        let params = TreeParams::oak();
        let tree = Tree::generate(&params, 11);
        let mut sides = 0usize;
        for i in 1..tree.node_count() {
            let off = tree.rest_offset[i].abs();
            // Leaders are near-zero offsets; side branches cluster at the
            // spread. Only count clear side branches to keep the band
            // assertion meaningful.
            if off > FORK_SPREAD * 0.6 {
                sides += 1;
                assert!(
                    off <= FORK_SPREAD * 1.35,
                    "offset {off} exceeds the golden spread band"
                );
            }
        }
        assert!(sides > 10, "expected many golden fork sides, saw {sides}");
    }

    /// Growth: at t = 0 every branch has zero length (tips sit on their
    /// starts); by growth_end the windless pose matches the rest pose.
    #[test]
    fn growth_runs_from_nothing_to_rest() {
        let params = TreeParams::oak();
        let tree = Tree::generate(&params, 5);
        let mut pose = TreePose::new(&tree);
        let calm = WindParams {
            amp_rad: 0.0,
            freq_scale: 1.0,
        };

        tree.pose_into(&mut pose, 0.0, &calm);
        for i in 0..tree.node_count() {
            assert_eq!(pose.tip_x[i], pose.start_x[i], "tip leaves start at t=0");
            assert_eq!(pose.tip_y[i], pose.start_y[i], "tip leaves start at t=0");
        }

        tree.pose_into(&mut pose, tree.growth_end(), &calm);
        for i in 0..tree.node_count() {
            assert!((pose.tip_x[i] - tree.rest_tx[i]).abs() < 1e-3);
            assert!((pose.tip_y[i] - tree.rest_ty[i]).abs() < 1e-3);
        }
    }

    /// Growth is monotone: total branch length never decreases.
    #[test]
    fn growth_is_monotone() {
        let tree = Tree::generate(&TreeParams::oak(), 9);
        let mut pose = TreePose::new(&tree);
        let calm = WindParams {
            amp_rad: 0.0,
            freq_scale: 1.0,
        };
        let mut last = -1.0;
        let mut t = 0.0;
        while t <= tree.growth_end() {
            tree.pose_into(&mut pose, t, &calm);
            let total: f32 = (0..tree.node_count())
                .map(|i| {
                    let (dx, dy) = (
                        pose.tip_x[i] - pose.start_x[i],
                        pose.tip_y[i] - pose.start_y[i],
                    );
                    (dx * dx + dy * dy).sqrt()
                })
                .sum();
            assert!(total >= last, "total length shrank at t={t}");
            last = total;
            t += 1.0 / 30.0;
        }
    }

    /// Pose is a pure function of (tree, t, wind): same inputs, same
    /// checksum, at any sampled time.
    #[test]
    fn pose_is_deterministic() {
        let tree = Tree::generate(&TreeParams::oak(), 3);
        let mut a = TreePose::new(&tree);
        let mut b = TreePose::new(&tree);
        let wind = WindParams {
            amp_rad: 0.3,
            freq_scale: 1.4,
        };
        for k in 0..600u32 {
            let t = k as f32 / 60.0;
            tree.pose_into(&mut a, t, &wind);
            tree.pose_into(&mut b, t, &wind);
            assert_eq!(a.checksum(), b.checksum(), "pose diverged at t={t}");
        }
    }

    /// Sway stays bounded: with wind on, tips deviate from their rest
    /// position by no more than the path length times the bend amplitude.
    #[test]
    fn sway_stays_bounded() {
        let params = TreeParams::oak();
        let tree = Tree::generate(&params, 13);
        let mut pose = TreePose::new(&tree);
        let wind = WindParams {
            amp_rad: 0.25,
            freq_scale: 1.0,
        };
        // A generous geometric bound: every segment rotates at most
        // amp·flex ≤ amp, so a tip can move at most (total chain length)·amp.
        let chain: f32 = (0..tree.node_count()).map(|i| tree.len[i]).sum();
        let bound = chain * wind.amp_rad * 1.5;
        for k in 0..240u32 {
            let t = tree.growth_end() + k as f32 / 60.0;
            tree.pose_into(&mut pose, t, &wind);
            for i in 0..tree.node_count() {
                let d = ((pose.tip_x[i] - tree.rest_tx[i]).powi(2)
                    + (pose.tip_y[i] - tree.rest_ty[i]).powi(2))
                .sqrt();
                assert!(d <= bound, "tip {i} moved {d} > bound {bound} at t={t}");
            }
        }
    }

    /// Node counts sit in the expected band for the oak preset: big enough
    /// to read as a tree, far under the safety cap.
    #[test]
    fn oak_preset_node_count_is_sane() {
        let tree = Tree::generate(&TreeParams::oak(), 1);
        assert!(
            tree.node_count() > 200,
            "too few nodes: {}",
            tree.node_count()
        );
        assert!(
            tree.node_count() < 4_000,
            "too many nodes: {}",
            tree.node_count()
        );
        assert!(tree.leaf_count() > 100);
    }

    /// Multiple trees (a grove) can share a coordinate space without
    /// interfering: structure checksums depend only on (params, seed).
    #[test]
    fn grove_trees_are_independent() {
        let params = TreeParams::oak();
        let a = Tree::generate_at(&params, 42, -100.0, 0.0);
        let b = Tree::generate_at(&params, 43, 100.0, 0.0);
        assert_ne!(a.structure_checksum(), b.structure_checksum());
        assert_eq!(a.base(), (-100.0, 0.0));
        assert_eq!(b.base(), (100.0, 0.0));
    }
}
