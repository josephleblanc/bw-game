//! bw-map-gallery: the fourth gallery — generated tile maps from
//! `bw_core::map`, tier 2 of docs/maps/map-design.md (the
//! `map-gallery` skeleton). Headless scenes build a map from
//! `(scene, seed)` and report the checksum, walkable count, and
//! terrain mix; the visual surface is the tactical SVG ortho render
//! (`svg`) under the shared ADR 0006 orientation (`bw_core::camera`),
//! which the walker viewer reads too — one set of constants, so the
//! surfaces cannot drift.
//!
//! No engine dependency at this tier: the map is static, so there is
//! no ECS to drive and no tick to step. The headless pass measures
//! generation, the one real op; per-tick work arrives with
//! pathfinding (tier 3) and the walkers-on-maps viewer (tier 4).

pub mod svg;

use bw_core::map::{GenParams, Map, Terrain, Tile};

/// The scene table: `(id, generation params)` in `--perf-scenes`
/// order. Scenes are pure parameter sets — the same law as the tree
/// gallery's anchors and the walker gallery's rings.
pub const SCENES: &[(&str, GenParams)] = &[
    // The default meadow: GenParams::default() exactly (pinned by
    // test so the scene and the defaults cannot drift apart).
    (
        "meadow",
        GenParams {
            width: 64,
            height: 64,
            octaves: 4,
            scale: 8.0,
            stone_threshold: 0.60,
            water_fraction: 0.08,
            temp_base_c: 15.0,
            temp_span_c: 14.0,
            temp_noise_c: 2.5,
        },
    ),
    // Islands: a wet budget, broader features, one octave less so the
    // coastlines stay chunky — and a tropical, near-equatorial climate.
    (
        "archipelago",
        GenParams {
            width: 64,
            height: 64,
            octaves: 3,
            scale: 5.0,
            stone_threshold: 0.68,
            water_fraction: 0.22,
            temp_base_c: 24.0,
            temp_span_c: 4.0,
            temp_noise_c: 1.5,
        },
    ),
    // Dry, paved: big slow stone masses, almost no water, hot.
    (
        "badlands",
        GenParams {
            width: 64,
            height: 64,
            octaves: 4,
            scale: 10.0,
            stone_threshold: 0.48,
            water_fraction: 0.02,
            temp_base_c: 28.0,
            temp_span_c: 8.0,
            temp_noise_c: 3.0,
        },
    ),
    // The control: the blank law as a scene — a provenance no
    // threshold reaches, so every tile is soil (equivalent to
    // `Map::blank(64, 64)`, pinned by test); default climate.
    (
        "blank",
        GenParams {
            width: 64,
            height: 64,
            octaves: 4,
            scale: 8.0,
            stone_threshold: 1.0,
            water_fraction: 0.0,
            temp_base_c: 15.0,
            temp_span_c: 14.0,
            temp_noise_c: 2.5,
        },
    ),
];

/// Scene lookup by id.
pub fn scene_preset(id: &str) -> Option<GenParams> {
    SCENES
        .iter()
        .find(|(scene_id, _)| *scene_id == id)
        .map(|(_, params)| *params)
}

/// Build a scene's map: `(scene, seed)` is the whole input — the same
/// pair always reproduces the same map, bit-for-bit (map-core's law).
pub fn build_scene(params: &GenParams, seed: u64) -> Map {
    Map::generate(seed, *params)
}

/// What the headless pass reports: layer counts plus the map
/// checksum. Walkable is tiles minus water (generated maps carry no
/// occupancy — buildings are a later tier).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneSummary {
    pub tiles: usize,
    pub walkable: usize,
    pub soil: usize,
    pub stone: usize,
    pub water: usize,
    /// Channel means over all tiles — the read side of the tile
    /// property pattern, as the headless scenes see it.
    pub mean_temperature_c: f32,
    pub mean_fertility: f32,
    pub checksum: u64,
}

pub fn summarize(map: &Map) -> SceneSummary {
    let mut soil = 0;
    let mut stone = 0;
    let mut water = 0;
    let mut temp_sum = 0.0f32;
    let mut fert_sum = 0.0f32;
    for z in 0..map.height() as i32 {
        for x in 0..map.width() as i32 {
            match map.terrain_at(Tile { x, z }) {
                Some(Terrain::Soil) => soil += 1,
                Some(Terrain::Stone) => stone += 1,
                _ => water += 1,
            }
            temp_sum += map.temperature_at(Tile { x, z }).unwrap_or(0.0);
            fert_sum += map.fertility_at(Tile { x, z }).unwrap_or(0.0);
        }
    }
    let tiles = map.tile_count().max(1) as f32;
    SceneSummary {
        tiles: map.tile_count(),
        walkable: map.walkable_count(),
        soil,
        stone,
        water,
        mean_temperature_c: temp_sum / tiles,
        mean_fertility: fert_sum / tiles,
        checksum: map.checksum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scene table is the contract: ids in `--perf-scenes` order.
    #[test]
    fn scene_table_ids_are_stable() {
        let ids: Vec<&str> = SCENES.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["meadow", "archipelago", "badlands", "blank"]);
        for (id, _) in SCENES {
            assert!(scene_preset(id).is_some(), "lookup missed {id}");
        }
        assert!(scene_preset("nope").is_none());
    }

    /// The meadow scene is exactly `GenParams::default()` — the scene
    /// table and the defaults are one law, not two.
    #[test]
    fn meadow_is_the_default_params() {
        assert_eq!(scene_preset("meadow"), Some(GenParams::default()));
    }

    /// The blank scene is the blank law: a provenance no threshold
    /// reaches, whatever the seed — every tile soil, every tile
    /// walkable, layers identical to `Map::blank` at the same extents
    /// (the checksums differ only by seed provenance, which they fold
    /// in by design).
    #[test]
    fn blank_scene_matches_map_blank() {
        let params = scene_preset("blank").unwrap_or_else(|| panic!("blank scene"));
        let blank_map = Map::blank(params.width, params.height);
        for seed in [0, 1234] {
            let scene_map = build_scene(&params, seed);
            let s = summarize(&scene_map);
            assert_eq!(s.soil, s.tiles, "seed {seed}: blank must be all soil");
            assert_eq!(s.walkable, s.tiles, "seed {seed}: all walkable");
            for z in 0..params.height as i32 {
                for x in 0..params.width as i32 {
                    let t = scene_map.terrain_at(Tile { x, z });
                    assert_eq!(
                        t,
                        blank_map.terrain_at(Tile { x, z }),
                        "seed {seed}: layers diverged at ({x}, {z})"
                    );
                }
            }
        }
    }

    /// `(scene, seed)` reproduces the map bit-for-bit; a different
    /// seed does not — for every scene.
    #[test]
    fn scenes_are_deterministic_and_seed_sensitive() {
        for (id, params) in SCENES {
            let a = build_scene(params, 42);
            let b = build_scene(params, 42);
            assert_eq!(a, b, "{id}: same (scene, seed) must reproduce");
            assert_eq!(summarize(&a).checksum, summarize(&b).checksum);
            let c = build_scene(params, 43);
            assert_ne!(
                summarize(&a).checksum,
                summarize(&c).checksum,
                "{id}: reroll seed should move the map"
            );
        }
    }

    /// Summaries add up for every scene: the three terrains partition
    /// the tiles, and walkable is everything but water.
    #[test]
    fn summaries_partition_the_tiles() {
        for (id, params) in SCENES {
            let s = summarize(&build_scene(params, 7));
            assert_eq!(s.soil + s.stone + s.water, s.tiles, "{id}: layers leaked");
            assert_eq!(s.walkable, s.tiles - s.water, "{id}: walkability law");
        }
    }

    /// The channel means read like the scenes' climates — temperate
    /// meadow, tropical archipelago, hot badlands — and fertility is a
    /// fraction everywhere. The read side of the tile property
    /// pattern, exercised where the scenes live.
    #[test]
    fn summaries_carry_the_channel_means() {
        let s = |id: &str| {
            let params = scene_preset(id).unwrap_or_else(|| panic!("{id} scene"));
            summarize(&build_scene(&params, 42))
        };
        let meadow = s("meadow");
        let archipelago = s("archipelago");
        let badlands = s("badlands");
        assert!(
            (15.0..=29.0).contains(&meadow.mean_temperature_c),
            "meadow mean temperature {}",
            meadow.mean_temperature_c
        );
        assert!(
            archipelago.mean_temperature_c > meadow.mean_temperature_c,
            "archipelago should outrank the meadow"
        );
        assert!(
            badlands.mean_temperature_c > archipelago.mean_temperature_c,
            "badlands should be the hottest"
        );
        let blank = s("blank");
        for (id, sum) in [
            ("meadow", meadow),
            ("archipelago", archipelago),
            ("badlands", badlands),
            ("blank", blank),
        ] {
            assert!(
                (0.0..=1.0).contains(&sum.mean_fertility),
                "{id} mean fertility {} not a fraction",
                sum.mean_fertility
            );
        }
    }

    /// Each scene reads like its name on the canonical seed — bands,
    /// not exact counts, so noise retuning only trips the map-core
    /// pins when the character genuinely changes.
    #[test]
    fn scenes_match_their_character() {
        let meadow = summarize(&build_scene(
            &scene_preset("meadow").unwrap_or_default(),
            42,
        ));
        let f = meadow.tiles as f32;
        assert!(meadow.water as f32 / f > 0.01, "meadow has ponds");
        assert!(meadow.stone as f32 / f > 0.05, "meadow has stone");
        assert!(meadow.soil as f32 / f > 0.60, "meadow is mostly soil");

        let arch = summarize(&build_scene(
            &scene_preset("archipelago").unwrap_or_default(),
            42,
        ));
        assert!(
            arch.water as f32 / arch.tiles as f32 > 0.20,
            "archipelago is wet: {}",
            arch.water
        );

        let bad = summarize(&build_scene(
            &scene_preset("badlands").unwrap_or_default(),
            42,
        ));
        assert!(
            bad.stone as f32 / bad.tiles as f32 > 0.30,
            "badlands are stony: {}",
            bad.stone
        );
        assert!(bad.water == 0 || (bad.water as f32 / bad.tiles as f32) < 0.05);

        let blank = summarize(&build_scene(&scene_preset("blank").unwrap_or_default(), 42));
        assert_eq!(blank.soil, blank.tiles, "blank is all soil");
        assert_eq!(blank.walkable, blank.tiles);
    }
}
