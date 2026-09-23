//! The tile map — the game's ground, per docs/maps/map-design.md
//! (tier 1, `map-core`). 1 m tiles addressed by a `Tile { x, z }`
//! newtype; the `Map` is structure-of-arrays flat layers over one
//! row-major index space — terrain (the ground itself), occupancy
//! (event-driven state on top), and a derived pathfinding cost cache
//! rebuilt whole on every mutation, the same rebuild-on-change law as
//! `bw_core::sim`'s spatial grid. Coordinates are map-local: tile
//! `(x, z)` occupies `[x, x+1) × [z, z+1)`; a world offset arrives only
//! when a mechanic demands more than one map. Generation is seeded
//! value noise hand-rolled on the splitmix64 law behind `sim::Rng`
//! (ADR 0002: the core stays dependency-free), and `(seed, params)`
//! reproduce the map bit-for-bit — the checksum folds in every
//! generation parameter, so tuning churn trips the golden hashes
//! (ADR 0003's determinism discipline).

use crate::math::Vec3;
use crate::sim::splitmix_mix;

// ---------------------------------------------------------------------------
// Coordinates
// ---------------------------------------------------------------------------

/// A tile address in map-local grid coordinates: tile `(x, z)` covers
/// `[x, x+1) × [z, z+1)` meters. A `Copy`/`Eq`/`Hash` newtype from day
/// one, so raw `(i32, i32)` pairs never propagate through APIs — the
/// same semantic-identity call as the character rig's `bone` indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Tile {
    pub x: i32,
    pub z: i32,
}

/// World position → containing tile. Floor, never truncate: `as`-casts
/// round toward zero, so `-0.25 as i32 == 0` would claim the wrong
/// tile — negatives are exactly where the two disagree.
pub fn tile_of(pos: Vec3) -> Tile {
    Tile {
        x: pos.x.floor() as i32,
        z: pos.z.floor() as i32,
    }
}

/// Tile → its center on the ground plane (the point walkers aim at;
/// round-trips through `tile_of`).
pub fn tile_center(tile: Tile) -> Vec3 {
    Vec3::new(tile.x as f32 + 0.5, 0.0, tile.z as f32 + 0.5)
}

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

/// The ground itself. `#[repr(u8)]` for flat storage and checksums;
/// migrates to data-driven terrain ids when the `param-presets`
/// backlog item lands (tile definitions in TOML).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Terrain {
    /// Default ground.
    #[default]
    Soil = 0,
    /// Paved: slightly faster to walk.
    Stone = 1,
    /// Ponds: blocking.
    Water = 2,
}

impl Terrain {
    /// Pathfinding cost in fixed point (tenths: 10 = 1.0×). Zero is
    /// reserved for "blocked", so no walkable terrain may cost 0.
    pub const fn cost(self) -> u8 {
        match self {
            Terrain::Soil => 10,
            Terrain::Stone => 9,
            Terrain::Water => 0,
        }
    }
}

/// Event-driven state on top of terrain, as bitflags: `RESERVED` is a
/// construction blueprint holding tiles, `BLOCKED` a completed
/// building. Hand-rolled — the core is dependency-free, so no bitflags
/// crate — with just the set algebra the map needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Occupancy(u8);

impl Occupancy {
    pub const EMPTY: Self = Self(0);
    /// A completed building footprint: unwalkable.
    pub const BLOCKED: Self = Self(1 << 0);
    /// A construction reservation holding tiles: still a ghost, but
    /// paths must already route around it.
    pub const RESERVED: Self = Self(1 << 1);

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn insert(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn remove(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Raw bits — checksums and tests.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl std::ops::BitOr for Occupancy {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Occupancy {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Generation parameters — every field participates in the map
/// checksum, so tuning churn is caught by the golden hashes. A struct
/// today; TOML when `param-presets` lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenParams {
    /// Grid extents in tiles (row-major `z * width + x`).
    pub width: u32,
    pub height: u32,
    /// Value-noise octaves per channel.
    pub octaves: u32,
    /// Base feature size in tiles: one lattice cell spans `scale` tiles.
    pub scale: f32,
    /// Stone where the stone channel rises to this threshold, `[0, 1]`.
    pub stone_threshold: f32,
    /// Approximate water coverage, `[0, 1]`: water where the water
    /// channel pools above `1 - water_fraction`.
    pub water_fraction: f32,
    /// Temperature at the z = 0 (north) edge, °C. The channel gradients
    /// to `temp_base_c + temp_span_c` at the south edge, plus noise.
    pub temp_base_c: f32,
    /// North→south temperature span, °C — the map's latitude sweep.
    pub temp_span_c: f32,
    /// Temperature noise amplitude, °C — local variation around the
    /// gradient.
    pub temp_noise_c: f32,
}

impl Default for GenParams {
    /// The 64×64 default from the design doc — fixed size, not chunked
    /// (infinite maps are absent by design until a measured need).
    fn default() -> Self {
        Self {
            width: 64,
            height: 64,
            octaves: 4,
            scale: 8.0,
            stone_threshold: 0.60,
            water_fraction: 0.08,
            temp_base_c: 15.0,
            temp_span_c: 14.0,
            temp_noise_c: 2.5,
        }
    }
}

/// Noise channels: decorrelated lattice streams for stone shapes,
/// water pools, temperature variation, and fertility, keyed into the
/// lattice hash (octaves spread each channel across further streams:
/// `channel * 64 + octave`). New channels take the next id — the
/// tile-property recipe (see the add-map-tile-property skill).
const CH_STONE: u32 = 0;
const CH_WATER: u32 = 1;
const CH_TEMP: u32 = 2;
const CH_FERT: u32 = 3;

/// Random-access lattice value in `[0, 1)`: the splitmix64 avalanche
/// (`sim::Rng`'s mixing law) applied to a pure coordinate hash, with
/// the RNG's own 24-bit quantization. Random access is the point —
/// the RNG itself is sequential, and octave sampling must stay
/// position-addressable and evaluation-order-free.
fn lattice(seed: u64, channel: u32, x: i32, z: i32) -> f32 {
    let h = seed
        ^ (x as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (z as u64).wrapping_mul(0x94D0_49BB_1331_11EB)
        ^ u64::from(channel).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (splitmix_mix(h) >> 40) as f32 / 16_777_216.0
}

/// Smoothstep: C1 continuity at the lattice points — the reason value
/// noise doesn't show grid creases.
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// One octave of value noise: lattice values on integer points,
/// smoothstep-interpolated. A convex combination of the corners, so
/// the output stays strictly inside their `[0, 1)` range.
fn value_noise(seed: u64, channel: u32, x: f32, z: f32) -> f32 {
    let (x0, z0) = (x.floor(), z.floor());
    let (tx, tz) = (smooth(x - x0), smooth(z - z0));
    let (xi, zi) = (x0 as i32, z0 as i32);
    let c00 = lattice(seed, channel, xi, zi);
    let c10 = lattice(seed, channel, xi + 1, zi);
    let c01 = lattice(seed, channel, xi, zi + 1);
    let c11 = lattice(seed, channel, xi + 1, zi + 1);
    let a = c00 * (1.0 - tx) + c10 * tx;
    let b = c01 * (1.0 - tx) + c11 * tx;
    a * (1.0 - tz) + b * tz
}

/// Fractal sum of value-noise octaves — amplitude halving as frequency
/// doubles, normalized back into `[0, 1)` (a convex blend of values
/// each strictly below 1 stays strictly below 1, so threshold
/// extremes behave exactly).
fn fbm(seed: u64, channel: u32, x: f32, z: f32, octaves: u32, scale: f32) -> f32 {
    let mut freq = 1.0 / scale;
    let mut amp = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for octave in 0..octaves {
        sum += amp * value_noise(seed, channel * 64 + octave, x * freq, z * freq);
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

// ---------------------------------------------------------------------------
// The map
// ---------------------------------------------------------------------------

/// The tile map: SoA flat `Vec` layers over one row-major index space
/// (`z * width + x` — renders read rows naturally). Allocated once at
/// generation; afterwards mutated only by events, and every mutation
/// rebuilds the derived cost cache whole — at these sizes (4 KiB at
/// the 64×64 default) dirty-tile bookkeeping would cost more than it
/// saves, the sim grid's precedent.
#[derive(Debug, Clone, PartialEq)]
pub struct Map {
    /// Generation provenance — folded into the checksum.
    seed: u64,
    params: GenParams,
    width: u32,
    height: u32,
    terrain: Vec<Terrain>,
    occupancy: Vec<Occupancy>,
    /// Temperature per tile, °C — a north→south gradient plus noise.
    /// The first scalar gameplay channel: written by generation and by
    /// events (`set_temperature`), read by anything that cares about
    /// climate. Folds into the checksum.
    temperature: Vec<f32>,
    /// Fertility per tile as a fraction in `(0, 1)` — noise, damped on
    /// stone, near zero under water. No generation knobs: the range is
    /// pinned by semantics (a fraction), not tuning.
    fertility: Vec<f32>,
    /// Derived pathfinding cache: 0 = blocked, else the terrain cost in
    /// fixed point. Rebuilt by every mutation.
    cost: Vec<u8>,
}

impl Map {
    /// An all-soil map of the given size: one law — `generate` with a
    /// provenance no threshold ever reaches (no stone threshold met,
    /// no water budgeted), so blanks are deterministic too.
    pub fn blank(width: u32, height: u32) -> Self {
        Self::generate(
            0,
            GenParams {
                width,
                height,
                stone_threshold: 1.0,
                water_fraction: 0.0,
                ..GenParams::default()
            },
        )
    }

    /// Seeded, pure, checksummed: stone patches where the stone channel
    /// crosses `stone_threshold`, ponds where the water channel pools
    /// above the `water_fraction` budget (water wins where both fire).
    pub fn generate(seed: u64, params: GenParams) -> Self {
        assert!(
            params.width > 0 && params.height > 0,
            "map must have extent"
        );
        let Some(tiles) = (params.width as usize).checked_mul(params.height as usize) else {
            panic!(
                "{}×{} overflows the tile count",
                params.width, params.height
            );
        };
        assert!(tiles <= 1 << 24, "{tiles} tiles exceeds the 16 MiB cap");
        assert!(
            params.octaves > 0 && params.octaves <= 16,
            "octaves {} outside 1..=16",
            params.octaves
        );
        assert!(
            params.scale > 0.0 && params.scale.is_finite(),
            "noise scale must be positive and finite"
        );
        assert!(
            (0.0..=1.0).contains(&params.stone_threshold),
            "stone_threshold {} outside [0, 1]",
            params.stone_threshold
        );
        assert!(
            (0.0..=1.0).contains(&params.water_fraction),
            "water_fraction {} outside [0, 1]",
            params.water_fraction
        );
        for (name, v) in [
            ("temp_base_c", params.temp_base_c),
            ("temp_span_c", params.temp_span_c),
            ("temp_noise_c", params.temp_noise_c),
        ] {
            assert!(v.is_finite(), "{name} {v} must be finite");
        }
        assert!(
            params.temp_noise_c >= 0.0,
            "temp_noise_c {} must be non-negative",
            params.temp_noise_c
        );
        let mut map = Self {
            seed,
            params,
            width: params.width,
            height: params.height,
            terrain: Vec::with_capacity(tiles),
            occupancy: vec![Occupancy::EMPTY; tiles],
            temperature: Vec::with_capacity(tiles),
            fertility: Vec::with_capacity(tiles),
            cost: vec![0; tiles],
        };
        // The temperature gradient's denominator: `(height - 1).max(1)`
        // keeps single-row maps at the base temperature.
        let grad = 1.0 / (map.height - 1).max(1) as f32;
        for z in 0..map.height as i32 {
            for x in 0..map.width as i32 {
                // Sample at tile centers so no tile straddles a lattice
                // point.
                let (cx, cz) = (x as f32 + 0.5, z as f32 + 0.5);
                let p = &map.params;
                // Ponds sit where the water channel *pools*: the square
                // tail pushes the channel toward 1 so a small fraction
                // budget actually wets the map (value-noise fbm
                // concentrates near 0.5 — a raw `fbm >= 1 - f` threshold
                // would need huge budgets to flood anything). Monotone,
                // and still strictly inside [0, 1), so the f = 0 / f = 1
                // extremes behave exactly.
                let pooled = 1.0 - (1.0 - fbm(seed, CH_WATER, cx, cz, p.octaves, p.scale)).powi(2);
                let terrain = if pooled >= 1.0 - p.water_fraction {
                    Terrain::Water
                } else if fbm(seed, CH_STONE, cx, cz, p.octaves, p.scale) >= p.stone_threshold {
                    Terrain::Stone
                } else {
                    Terrain::Soil
                };
                map.terrain.push(terrain);
                // Temperature: latitude gradient plus symmetric noise
                // (`(fbm - 0.5) * 2` spans ±temp_noise_c around it).
                let gradient = p.temp_base_c + p.temp_span_c * z as f32 * grad;
                let noise = (fbm(seed, CH_TEMP, cx, cz, p.octaves, p.scale) - 0.5) * 2.0;
                map.temperature.push(gradient + noise * p.temp_noise_c);
                // Fertility: its own noise stream, damped by terrain —
                // rock supports little, water nearly nothing.
                let raw = fbm(seed, CH_FERT, cx, cz, p.octaves, p.scale);
                let damped = match terrain {
                    Terrain::Soil => raw,
                    Terrain::Stone => raw * 0.60,
                    Terrain::Water => raw * 0.10,
                };
                map.fertility.push(damped);
            }
        }
        map.rebuild_cost();
        map
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn tile_count(&self) -> usize {
        self.terrain.len()
    }

    pub fn in_bounds(&self, tile: Tile) -> bool {
        tile.x >= 0 && tile.z >= 0 && (tile.x as u32) < self.width && (tile.z as u32) < self.height
    }

    fn index_of(&self, tile: Tile) -> Option<usize> {
        if self.in_bounds(tile) {
            Some(tile.z as usize * self.width as usize + tile.x as usize)
        } else {
            None
        }
    }

    pub fn terrain_at(&self, tile: Tile) -> Option<Terrain> {
        self.index_of(tile).map(|i| self.terrain[i])
    }

    pub fn occupancy_at(&self, tile: Tile) -> Option<Occupancy> {
        self.index_of(tile).map(|i| self.occupancy[i])
    }

    /// Pathfinding cost from the derived cache: 0 = blocked or out of
    /// bounds.
    pub fn cost(&self, tile: Tile) -> u8 {
        self.index_of(tile).map_or(0, |i| self.cost[i])
    }

    /// The walkability law, read from the layers (the cache answers
    /// `cost`, this answers the question): terrain ≠ Water and no
    /// occupancy. Out of bounds is not walkable.
    pub fn walkable(&self, tile: Tile) -> bool {
        match self.index_of(tile) {
            Some(i) => self.terrain[i] != Terrain::Water && self.occupancy[i].is_empty(),
            None => false,
        }
    }

    /// Number of walkable tiles — the count the gallery scenes report.
    pub fn walkable_count(&self) -> usize {
        self.terrain
            .iter()
            .zip(&self.occupancy)
            .filter(|&(t, o)| *t != Terrain::Water && o.is_empty())
            .count()
    }

    /// Set a tile's occupancy (the building slice layers footprints on
    /// this primitive). Returns false — no-op — out of bounds; any
    /// change rebuilds the cost cache, the rebuild-on-change contract.
    pub fn set_occupancy(&mut self, tile: Tile, occ: Occupancy) -> bool {
        match self.index_of(tile) {
            Some(i) => {
                self.occupancy[i] = occ;
                self.rebuild_cost();
                true
            }
            None => false,
        }
    }

    /// Temperature at a tile, °C (`None` out of bounds). Reads are free
    /// from any system; writes go through `set_temperature` so the
    /// checksum sees them.
    pub fn temperature_at(&self, tile: Tile) -> Option<f32> {
        self.index_of(tile).map(|i| self.temperature[i])
    }

    /// Event-driven temperature write (a campfire's warmth, a spell's
    /// frost). Returns false — no-op — out of bounds. Nothing derives
    /// from temperature yet, so there is no cache to rebuild; when a
    /// consumer derives one, its rebuild belongs here (the
    /// `set_occupancy` precedent).
    pub fn set_temperature(&mut self, tile: Tile, celsius: f32) -> bool {
        assert!(
            celsius.is_finite(),
            "temperature {celsius} must be finite — NaN would poison the checksum"
        );
        match self.index_of(tile) {
            Some(i) => {
                self.temperature[i] = celsius;
                true
            }
            None => false,
        }
    }

    /// Fertility at a tile as a fraction in `(0, 1)` (`None` out of
    /// bounds): how well plants would take to this ground.
    pub fn fertility_at(&self, tile: Tile) -> Option<f32> {
        self.index_of(tile).map(|i| self.fertility[i])
    }

    /// Event-driven fertility write (tilling, crop exhaustion). Returns
    /// false — no-op — out of bounds.
    pub fn set_fertility(&mut self, tile: Tile, fraction: f32) -> bool {
        assert!(
            (0.0..=1.0).contains(&fraction),
            "fertility {fraction} outside [0, 1] — it is a fraction by law"
        );
        match self.index_of(tile) {
            Some(i) => {
                self.fertility[i] = fraction;
                true
            }
            None => false,
        }
    }

    /// Derive the cost cache from terrain + occupancy, whole.
    fn rebuild_cost(&mut self) {
        for ((c, &t), &o) in self.cost.iter_mut().zip(&self.terrain).zip(&self.occupancy) {
            *c = if t == Terrain::Water || !o.is_empty() {
                0
            } else {
                t.cost()
            };
        }
    }

    /// Digest of provenance + layers, the sim checksum's FNV law. Same
    /// `(seed, params, mutations)` → same checksum; any generation
    /// parameter or layer value moves it. Layer tags keep the byte
    /// streams from aliasing; f32 layers fold through `to_bits`
    /// (deterministic for the finite values the mutators assert).
    pub fn checksum(&self) -> u64 {
        fn step(h: u64, v: u64) -> u64 {
            h.wrapping_mul(0x1000_0000_01B3) ^ v
        }
        let mut h = 0xCBF2_9CE4_8422_2325;
        h = step(h, self.seed);
        h = step(h, u64::from(self.width));
        h = step(h, u64::from(self.height));
        h = step(h, u64::from(self.params.octaves));
        h = step(h, u64::from(self.params.scale.to_bits()));
        h = step(h, u64::from(self.params.stone_threshold.to_bits()));
        h = step(h, u64::from(self.params.water_fraction.to_bits()));
        h = step(h, u64::from(self.params.temp_base_c.to_bits()));
        h = step(h, u64::from(self.params.temp_span_c.to_bits()));
        h = step(h, u64::from(self.params.temp_noise_c.to_bits()));
        h = step(h, u64::from_be_bytes(*b"TERRAIN0"));
        h = self.terrain.iter().fold(h, |h, t| step(h, *t as u64));
        h = step(h, u64::from_be_bytes(*b"OCCUPANC"));
        h = self
            .occupancy
            .iter()
            .fold(h, |h, o| step(h, u64::from(o.bits())));
        h = step(h, u64::from_be_bytes(*b"TEMPERAT"));
        h = self
            .temperature
            .iter()
            .fold(h, |h, v| step(h, u64::from(v.to_bits())));
        h = step(h, u64::from_be_bytes(*b"FERTILIT"));
        h = self
            .fertility
            .iter()
            .fold(h, |h, v| step(h, u64::from(v.to_bits())));
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Rng;

    /// Floor, not truncate: negative positions land in the tile below
    /// zero — the exact spot `as`-casts silently get wrong — and tile
    /// boundaries belong to the upper tile.
    #[test]
    fn tile_of_floors_negative_coordinates() {
        assert_eq!(tile_of(Vec3::new(-0.25, 0.0, 3.7)), Tile { x: -1, z: 3 });
        assert_eq!(tile_of(Vec3::new(-1.0, 2.0, -0.9)), Tile { x: -1, z: -1 });
        assert_eq!(tile_of(Vec3::new(2.0, 0.0, 7.0)), Tile { x: 2, z: 7 });
        assert_eq!(tile_of(Vec3::new(2.999, 0.0, 7.999)), Tile { x: 2, z: 7 });
    }

    /// The center is the point walkers aim at: inside the tile, on the
    /// ground plane, converting back to its own tile.
    #[test]
    fn tile_center_round_trips() {
        let t = Tile { x: -3, z: 5 };
        assert_eq!(tile_center(t), Vec3::new(-2.5, 0.0, 5.5));
        assert_eq!(tile_of(tile_center(t)), t);
    }

    /// Bounds: corners belong, one step out does not, and
    /// out-of-bounds queries fail closed — not walkable, cost 0,
    /// `None` reads, no-op mutations.
    #[test]
    fn bounds_are_checked_and_queries_fail_closed() {
        let mut m = Map::generate(
            7,
            GenParams {
                width: 16,
                height: 8,
                ..GenParams::default()
            },
        );
        assert!(m.in_bounds(Tile { x: 0, z: 0 }));
        assert!(m.in_bounds(Tile { x: 15, z: 7 }));
        for t in [
            Tile { x: -1, z: 0 },
            Tile { x: 0, z: -1 },
            Tile { x: 16, z: 0 },
            Tile { x: 0, z: 8 },
        ] {
            assert!(!m.in_bounds(t), "{t:?} should be out of bounds");
            assert!(!m.walkable(t));
            assert_eq!(m.cost(t), 0);
            assert_eq!(m.terrain_at(t), None);
            assert_eq!(m.occupancy_at(t), None);
            assert!(!m.set_occupancy(t, Occupancy::BLOCKED));
        }
    }

    /// Blank maps are all soil, all walkable — and deterministic, one
    /// law with a provenance no threshold reaches.
    #[test]
    fn blank_maps_are_all_soil() {
        let m = Map::blank(8, 8);
        assert_eq!(m.walkable_count(), 64);
        assert_eq!(m.terrain_at(Tile { x: 7, z: 7 }), Some(Terrain::Soil));
        assert_eq!(Map::blank(8, 8).checksum(), m.checksum());
    }

    /// Determinism: same (seed, params) reproduce the map bit-for-bit;
    /// a different seed does not.
    #[test]
    fn generation_is_deterministic() {
        let p = GenParams::default();
        let a = Map::generate(42, p);
        let b = Map::generate(42, p);
        assert_eq!(a, b, "same (seed, params) must reproduce the whole map");
        assert_ne!(a.checksum(), Map::generate(43, p).checksum());
    }

    /// Every generation parameter participates in the checksum —
    /// tuning churn cannot hide from the golden hashes.
    #[test]
    fn every_generation_parameter_moves_the_checksum() {
        let base = GenParams::default();
        let want = Map::generate(42, base).checksum();
        let varied = [
            GenParams { octaves: 5, ..base },
            GenParams { scale: 8.5, ..base },
            GenParams {
                stone_threshold: 0.55,
                ..base
            },
            GenParams {
                water_fraction: 0.12,
                ..base
            },
            GenParams {
                temp_base_c: 16.0,
                ..base
            },
            GenParams {
                temp_span_c: 15.0,
                ..base
            },
            GenParams {
                temp_noise_c: 3.0,
                ..base
            },
            GenParams { width: 63, ..base },
            GenParams { height: 65, ..base },
        ];
        for p in varied {
            assert_ne!(
                Map::generate(42, p).checksum(),
                want,
                "params {p:?} slipped through the checksum"
            );
        }
        assert_ne!(Map::generate(41, base).checksum(), want);
    }

    /// The golden hash: the default seed's exact map is contract now —
    /// any accidental change to the noise law, mixing constants, or
    /// thresholds trips this pin. Repinned 2026-09-23 when the
    /// temperature and fertility channels joined the checksum.
    #[test]
    fn default_generation_checksum_is_pinned() {
        assert_eq!(
            Map::generate(42, GenParams::default()).checksum(),
            0x02AF_DE33_3145_E043
        );
    }

    /// The temperature channel is a climate, not static noise: the
    /// north edge is cooler than the south edge (row means — the noise
    /// averages out over a row), and every tile stays inside the
    /// gradient-plus-noise envelope.
    #[test]
    fn temperature_gradients_southward_within_its_envelope() {
        let p = GenParams::default();
        let m = Map::generate(42, p);
        let row_mean = |z: i32| {
            let n = m.width() as f32;
            (0..m.width() as i32)
                .map(|x| m.temperature_at(Tile { x, z }).unwrap_or(f32::NAN))
                .sum::<f32>()
                / n
        };
        let north = row_mean(0);
        let south = row_mean(m.height() as i32 - 1);
        assert!(
            south - north > p.temp_span_c * 0.8,
            "gradient too weak: north {north}, south {south}"
        );
        for z in 0..m.height() as i32 {
            for x in 0..m.width() as i32 {
                let t = m.temperature_at(Tile { x, z }).unwrap_or(f32::NAN);
                let gradient =
                    p.temp_base_c + p.temp_span_c * z as f32 / (m.height() - 1).max(1) as f32;
                assert!(
                    (gradient - p.temp_noise_c..=gradient + p.temp_noise_c).contains(&t),
                    "tile ({x}, {z}) at {t}°C escaped the envelope"
                );
            }
        }
    }

    /// Fertility is a fraction damped by terrain: every value inside
    /// (0, 1), and the mean on stone sits clearly under the mean on
    /// soil, water clearly under both.
    #[test]
    fn fertility_is_a_fraction_and_respects_terrain() {
        let m = Map::generate(42, GenParams::default());
        let mut soil = (0.0f64, 0usize);
        let mut stone = (0.0f64, 0usize);
        let mut water = (0.0f64, 0usize);
        for z in 0..m.height() as i32 {
            for x in 0..m.width() as i32 {
                let t = Tile { x, z };
                let f = m.fertility_at(t).unwrap_or(f32::NAN);
                assert!((0.0..=1.0).contains(&f), "fertility {f} not a fraction");
                let bucket = match m.terrain_at(t) {
                    Some(Terrain::Soil) => &mut soil,
                    Some(Terrain::Stone) => &mut stone,
                    _ => &mut water,
                };
                bucket.0 += f64::from(f);
                bucket.1 += 1;
            }
        }
        let mean = |b: (f64, usize)| b.0 / b.1.max(1) as f64;
        assert!(mean(water) < mean(stone), "water out-ferts stone");
        assert!(mean(stone) < mean(soil), "stone out-ferts soil");
        assert!(
            mean(water) < 0.10,
            "water should be near-barren: {}",
            mean(water)
        );
    }

    /// Channel writes are event-driven and checksum-seen: a write
    /// moves the checksum, writing the original value back restores it
    /// exactly, and out-of-bounds writes no-op. Nothing derives from
    /// these channels yet, so no cache is involved — the pin is the
    /// contract for when one is.
    #[test]
    fn channel_writes_round_trip_checksum_exactly() {
        let mut m = Map::generate(42, GenParams::default());
        let t = Tile { x: 30, z: 30 };
        let pristine = m.checksum();
        let t0 = m.temperature_at(t).unwrap_or(f32::NAN);
        let f0 = m.fertility_at(t).unwrap_or(f32::NAN);
        assert!(m.set_temperature(t, t0 + 12.5), "in-bounds write");
        assert_ne!(m.checksum(), pristine);
        assert_eq!(m.temperature_at(t), Some(t0 + 12.5));
        assert!(m.set_fertility(t, 0.05));
        assert!(m.set_temperature(t, t0));
        assert!(m.set_fertility(t, f0));
        assert_eq!(m.checksum(), pristine, "restoring values restores the map");
        assert!(!m.set_temperature(Tile { x: -1, z: 0 }, 0.0));
        assert!(!m.set_fertility(Tile { x: 0, z: 99 }, 0.5));
        assert_eq!(m.checksum(), pristine, "out-of-bounds writes are no-ops");
    }

    /// The water budget is monotone: raising the fraction can only
    /// flood more of the same seed's map.
    #[test]
    fn water_fraction_is_monotone() {
        let p = GenParams::default();
        let dry = Map::generate(
            42,
            GenParams {
                water_fraction: 0.05,
                ..p
            },
        )
        .walkable_count();
        let mid = Map::generate(
            42,
            GenParams {
                water_fraction: 0.15,
                ..p
            },
        )
        .walkable_count();
        let wet = Map::generate(
            42,
            GenParams {
                water_fraction: 0.30,
                ..p
            },
        )
        .walkable_count();
        assert!(
            dry >= mid && mid >= wet,
            "water only floods: {dry} >= {mid} >= {wet}"
        );
        assert!(
            wet < dry,
            "a 30% water budget must actually flood something"
        );
    }

    /// Threshold extremes are exact, not approximate: a full water
    /// budget floods everything; an empty one with an unreachable
    /// stone threshold leaves plain soil.
    #[test]
    fn threshold_extremes_are_exact() {
        let p = GenParams::default();
        let ocean = Map::generate(
            1,
            GenParams {
                water_fraction: 1.0,
                ..p
            },
        );
        assert_eq!(ocean.walkable_count(), 0, "water everywhere");
        let plain = Map::generate(
            1,
            GenParams {
                water_fraction: 0.0,
                stone_threshold: 1.0,
                ..p
            },
        );
        assert_eq!(plain.walkable_count(), plain.tile_count());
        assert!(plain.terrain.iter().all(|&t| t == Terrain::Soil));
    }

    /// The default params paint a plausible meadow on the canonical
    /// seed: mostly soil, real patches of stone and water, stone
    /// actually cheaper to walk.
    #[test]
    fn default_generation_paints_a_meadow() {
        let m = Map::generate(42, GenParams::default());
        let n = m.tile_count() as f32;
        let count = |t: Terrain| m.terrain.iter().filter(|&&v| v == t).count() as f32 / n;
        let water = count(Terrain::Water);
        let stone = count(Terrain::Stone);
        assert!(water > 0.0 && water < 0.30, "water fraction {water}");
        assert!(stone > 0.0 && stone < 0.50, "stone fraction {stone}");
        assert!(
            water + stone < 0.75,
            "should be mostly soil: {}",
            water + stone
        );
        let Some(stone_tile) = (0..64)
            .flat_map(|z| (0..64).map(move |x| Tile { x, z }))
            .find(|&t| m.terrain_at(t) == Some(Terrain::Stone))
        else {
            panic!("the default seed paints stone");
        };
        assert_eq!(m.cost(stone_tile), 9, "paved stone is the cheaper walk");
    }

    /// The occupancy law: reservations and blocks both revoke
    /// walkability, the cost cache follows (0 = blocked), and clearing
    /// restores the generated map — checksum-exactly.
    #[test]
    fn occupancy_blocks_and_the_cost_cache_follows() {
        let mut m = Map::generate(42, GenParams::default());
        let t = Tile { x: 10, z: 12 };
        assert!(m.walkable(t), "the pin needs a walkable tile");
        let pristine = m.checksum();
        let soil_cost = m.cost(t);
        assert!(m.set_occupancy(t, Occupancy::RESERVED));
        assert!(!m.walkable(t), "a reservation holds its tiles");
        assert_eq!(m.cost(t), 0);
        assert_ne!(m.checksum(), pristine);
        assert!(m.set_occupancy(t, Occupancy::BLOCKED | Occupancy::RESERVED));
        assert!(!m.walkable(t));
        assert_eq!(m.cost(t), 0);
        assert!(m.set_occupancy(t, Occupancy::EMPTY));
        assert!(m.walkable(t));
        assert_eq!(m.cost(t), soil_cost);
        assert_eq!(
            m.checksum(),
            pristine,
            "clearing restores the generated map exactly"
        );
    }

    /// Rebuild-on-change correctness (the sim grid's
    /// rebuild-matches-fresh precedent): after a spread of mutations
    /// the cache equals a fresh derivation everywhere.
    #[test]
    fn cost_cache_matches_a_fresh_derivation_after_mutations() {
        let mut m = Map::generate(
            9,
            GenParams {
                width: 24,
                height: 24,
                ..GenParams::default()
            },
        );
        let mut rng = Rng::new(3);
        for _ in 0..40 {
            let t = Tile {
                x: (rng.next_u64() % 24) as i32,
                z: (rng.next_u64() % 24) as i32,
            };
            let occ = match rng.next_u64() % 3 {
                0 => Occupancy::EMPTY,
                1 => Occupancy::RESERVED,
                _ => Occupancy::BLOCKED,
            };
            m.set_occupancy(t, occ);
        }
        for z in 0..24 {
            for x in 0..24 {
                let t = Tile { x, z };
                let want = match (m.terrain_at(t), m.occupancy_at(t)) {
                    (Some(ter), Some(o)) if ter != Terrain::Water && o.is_empty() => ter.cost(),
                    _ => 0,
                };
                assert_eq!(m.cost(t), want, "cost cache drifted at {t:?}");
            }
        }
    }

    /// The bitflags themselves: insert/remove/contains, or-composition.
    #[test]
    fn occupancy_flags_compose() {
        let both = Occupancy::BLOCKED | Occupancy::RESERVED;
        assert!(both.contains(Occupancy::BLOCKED));
        assert!(both.contains(Occupancy::RESERVED));
        assert_eq!(both.remove(Occupancy::RESERVED), Occupancy::BLOCKED);
        assert_eq!(
            Occupancy::EMPTY.insert(Occupancy::RESERVED),
            Occupancy::RESERVED
        );
        assert!(!Occupancy::RESERVED.contains(Occupancy::BLOCKED));
        assert!(Occupancy::EMPTY.is_empty());
    }

    /// Terrain costs: fixed-point tenths, water blocked, soil default.
    #[test]
    fn terrain_costs_are_fixed_point() {
        assert_eq!(Terrain::Soil.cost(), 10);
        assert_eq!(Terrain::Stone.cost(), 9);
        assert_eq!(Terrain::Water.cost(), 0);
        assert_eq!(Terrain::default(), Terrain::Soil);
    }
}
