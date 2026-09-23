//! A* pathfinding over the map's tile grid (docs/maps/map-design.md,
//! the pathfinding tier). Dependency-free and deterministic beyond
//! the usual fixed-point inputs: the **neighbor order is fixed**
//! (East, North, West, South — north is −z, the temperature
//! gradient's convention) and frontier ties break totally on
//! `(f, then node index)`, so the same `(map, query)` always produces
//! the same route — pinned by tests, not by hope.
//!
//! Allocation contract (ADR 0004): a [`Pathfinder`] owns its
//! scratchpad, allocated once for a map size — the open heap and the
//! `g_score` / `came_from` / stamp arrays. Queries stamp entries with
//! a generation counter instead of clearing arrays, so a query costs
//! O(visited) with **zero allocation**; results land in a
//! caller-reused `Vec<Tile>` (the `CharacterPose` pattern: the buffer
//! is the API). The cost law rides the map's derived cache: a step
//! costs the *entered* tile's fixed-point cost, so stone is the
//! cheaper walk and water/blocked tiles (`cost 0`) never open.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::map::{Map, Tile};

/// The heuristic's per-step floor: the cheapest walkable terrain cost
/// (stone, 9 in fixed point). Manhattan × this stays admissible on
/// the 4-neighborhood — every step moves one tile and costs at least
/// this much — and consistent, so the first expansion of a node is
/// its optimum and lazy heap duplicates are safe to skip.
const MIN_COST: u32 = 9;

/// The neighbor law: East, North, West, South, in exactly this order
/// (north is −z). Equal-f frontier ties resolve toward the smallest
/// node index, and this order decides which neighbors even reach the
/// heap — both facts are pinned by test.
const NEIGHBORS: [(i32, i32); 4] = [(1, 0), (0, -1), (-1, 0), (0, 1)];

/// `came_from` sentinel: no parent (the start).
const NO_PARENT: u32 = u32::MAX;

/// Reusable A* scratchpad over one map size. Build one per map and
/// keep it: queries reset nothing but the generation counter.
#[derive(Debug, Clone)]
pub struct Pathfinder {
    width: u32,
    height: u32,
    /// Query generation: scratch entries are valid for the query that
    /// stamped them. Wraps only after ~4 billion queries, and the wrap
    /// itself does one full clear.
    generation: u32,
    /// Min-heap of `(f, node index)` — the total tie-break.
    open: BinaryHeap<Reverse<(u32, u32)>>,
    g_score: Vec<u32>,
    came_from: Vec<u32>,
    stamp: Vec<u32>,
    closed: Vec<u32>,
}

impl Pathfinder {
    pub fn new(map: &Map) -> Self {
        let tiles = map.width() as usize * map.height() as usize;
        Self {
            width: map.width(),
            height: map.height(),
            generation: 0,
            open: BinaryHeap::new(),
            g_score: vec![u32::MAX; tiles],
            came_from: vec![NO_PARENT; tiles],
            stamp: vec![0; tiles],
            closed: vec![0; tiles],
        }
    }

    /// Find a route from `start` to `goal` (both inclusive in the
    /// result — `start == goal` yields `[start]`), appending it to
    /// `out` (cleared first). Returns false — `out` empty — when
    /// either endpoint is out of bounds or unwalkable, or no route
    /// connects them. Zero allocation at steady state with a reused
    /// `out`.
    pub fn find_path_into(
        &mut self,
        map: &Map,
        start: Tile,
        goal: Tile,
        out: &mut Vec<Tile>,
    ) -> bool {
        assert_eq!(
            (self.width, self.height),
            (map.width(), map.height()),
            "pathfinder scratchpad is sized for a different map"
        );
        out.clear();
        if !map.walkable(start) || !map.walkable(goal) {
            return false;
        }
        if start == goal {
            out.push(start);
            return true;
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // Wrapped: one full clear, then continue from 1 so 0 stays
            // "never stamped".
            self.stamp.fill(0);
            self.closed.fill(0);
            self.generation = 1;
        }
        self.open.clear();
        let (si, gi) = (self.index(start), self.index(goal));
        let heuristic =
            |t: Tile| -> u32 { (t.x.abs_diff(goal.x) + t.z.abs_diff(goal.z)) * MIN_COST };
        self.stamp[si] = self.generation;
        self.g_score[si] = 0;
        self.came_from[si] = NO_PARENT;
        self.open.push(Reverse((heuristic(start), si as u32)));

        while let Some(Reverse((_, i))) = self.open.pop() {
            if self.closed[i as usize] == self.generation {
                continue; // lazy duplicate: a better route already expanded it
            }
            self.closed[i as usize] = self.generation;
            if i as usize == gi {
                self.reconstruct(si, gi, out);
                return true;
            }
            let here = self.tile(i as usize);
            for (dx, dz) in NEIGHBORS {
                let n = Tile {
                    x: here.x + dx,
                    z: here.z + dz,
                };
                let step = map.cost(n) as u32; // 0 = blocked or out of bounds
                if step == 0 {
                    continue;
                }
                let ni = self.index(n);
                let g = self.g_score[i as usize].saturating_add(step);
                if self.stamp[ni] != self.generation || g < self.g_score[ni] {
                    self.stamp[ni] = self.generation;
                    self.g_score[ni] = g;
                    self.came_from[ni] = i;
                    self.open.push(Reverse((g + heuristic(n), ni as u32)));
                }
            }
        }
        false
    }

    /// Allocating convenience for tests and off-hot-path callers; hot
    /// paths use [`Pathfinder::find_path_into`] with a reused buffer.
    pub fn find_path(&mut self, map: &Map, start: Tile, goal: Tile) -> Option<Vec<Tile>> {
        let mut out = Vec::new();
        self.find_path_into(map, start, goal, &mut out)
            .then_some(out)
    }

    fn index(&self, t: Tile) -> usize {
        t.z as usize * self.width as usize + t.x as usize
    }

    fn tile(&self, i: usize) -> Tile {
        Tile {
            x: (i % self.width as usize) as i32,
            z: (i / self.width as usize) as i32,
        }
    }

    /// Walk `came_from` back from the goal, reversing in place into
    /// `out` — no allocation, no recursion.
    fn reconstruct(&self, start: usize, goal: usize, out: &mut Vec<Tile>) {
        let mut i = goal;
        while i != start {
            out.push(self.tile(i));
            i = self.came_from[i] as usize;
        }
        out.push(self.tile(start));
        out.reverse();
    }
}

/// Line of sight between two tile centers, conservative by law: every
/// tile the segment touches must be walkable, and an exact lattice-corner
/// crossing counts **both** flanking tiles, so a smoothed leg never
/// clips a blocked corner. The crossing order is decided by
/// cross-multiplied integer comparison (t_x ∝ (2k+1)/2·|dx| versus
/// t_z ∝ (2m+1)/2·|dz|), so the perfect diagonal's corner touches can
/// never be missed to float rounding. The start tile is assumed (the
/// caller stands on it); every tile after it, goal included, is checked.
pub fn line_of_sight(map: &Map, from: Tile, to: Tile) -> bool {
    let (dx, dz) = (to.x - from.x, to.z - from.z);
    if dx == 0 && dz == 0 {
        return map.walkable(from);
    }
    let (adx, adz) = (dx.unsigned_abs() as u128, dz.unsigned_abs() as u128);
    let (sx, sz) = (dx.signum(), dz.signum());
    let (mut x, mut z) = (from.x, from.z);
    let (mut kx, mut kz) = (0u128, 0u128); // boundary crossings consumed
    let walk = |x: i32, z: i32| map.walkable(Tile { x, z });
    while (x, z) != (to.x, to.z) {
        // Both exhausted is impossible inside the loop: it means the
        // walk already reached `to`.
        let tx = if kx < adx {
            (2 * kx + 1) * adz
        } else {
            u128::MAX
        };
        let tz = if kz < adz {
            (2 * kz + 1) * adx
        } else {
            u128::MAX
        };
        if tx < tz {
            x += sx;
            kx += 1;
            if !walk(x, z) {
                return false;
            }
        } else if tz < tx {
            z += sz;
            kz += 1;
            if !walk(x, z) {
                return false;
            }
        } else {
            // Exact corner: the segment passes through the lattice
            // point, touching both flanks on the way.
            if !walk(x + sx, z) || !walk(x, z + sz) {
                return false;
            }
            x += sx;
            z += sz;
            kx += 1;
            kz += 1;
        }
    }
    true
}

/// String-pull smoothing over a found route (the fluency half of the
/// stair-shuffle fix, `docs/animation/qualities.md` "Path fluency"):
/// keep exactly the waypoints needed so consecutive kept waypoints
/// have walkable [`line_of_sight`] — a blank-map stair collapses into
/// one straight diagonal leg, while bends a wall forces survive as
/// corners. The output is always a **subsequence** of `route` (start
/// and goal kept), so the walk stays grid-faithful: it only ever aims
/// at tiles A* already chose. `out` is caller-reused, cleared first —
/// the buffer-is-the-API law. Send-time by design (Update on click),
/// never per tick.
pub fn smooth_route(map: &Map, route: &[Tile], out: &mut Vec<Tile>) {
    out.clear();
    let Some(&start) = route.first() else {
        return;
    };
    out.push(start);
    let mut i = 0;
    while i + 1 < route.len() {
        // Furthest waypoint still visible from the last kept one;
        // adjacency (j == i+1) is always acceptable — the route itself
        // vouches for that hop.
        let mut j = route.len() - 1;
        while j > i + 1 && !line_of_sight(map, route[i], route[j]) {
            j -= 1;
        }
        out.push(route[j]);
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{GenParams, Occupancy};

    fn meadow() -> Map {
        Map::generate(42, GenParams::default())
    }

    /// First walkable tile at or after the given scan origin, walking
    /// rows — deterministic endpoint picker for generated-map tests.
    fn walkable_from(map: &Map, x0: i32, z0: i32) -> Option<Tile> {
        let (w, h) = (map.width() as i32, map.height() as i32);
        let (xs, zs) = (x0.clamp(0, w - 1), z0.clamp(0, h - 1));
        for z in zs..h {
            for x in if z == zs { xs } else { 0 }..w {
                let t = Tile { x, z };
                if map.walkable(t) {
                    return Some(t);
                }
            }
        }
        None
    }

    /// A route honors the grid law: found, both endpoints inclusive,
    /// every hop 4-adjacent, every tile walkable.
    fn assert_valid_route(map: &Map, route: &[Tile], start: Tile, goal: Tile) {
        assert_eq!(route.first(), Some(&start), "route must begin at the start");
        assert_eq!(route.last(), Some(&goal), "route must end at the goal");
        for pair in route.windows(2) {
            let d = (pair[0].x - pair[1].x).abs() + (pair[0].z - pair[1].z).abs();
            assert_eq!(d, 1, "non-adjacent hop {:?} -> {:?}", pair[0], pair[1]);
        }
        for &t in route {
            assert!(map.walkable(t), "route crosses unwalkable {t:?}");
        }
    }

    /// On a uniform blank map the straight route is the only shortest
    /// route — the tie-break has nothing to choose.
    #[test]
    fn straight_routes_on_blank_maps_are_direct() {
        let map = Map::blank(16, 16);
        let mut pf = Pathfinder::new(&map);
        let route = pf
            .find_path(&map, Tile { x: 2, z: 3 }, Tile { x: 8, z: 3 })
            .unwrap_or_default();
        assert_eq!(route.len(), 7);
        assert!(route.iter().all(|t| t.z == 3), "route drifted off the row");
    }

    /// The determinism law, pinned exactly: on a blank map's diagonal
    /// every same-cost route ties, so what comes out is pure
    /// `(neighbor order, f-then-index tie-break)`. Any change to
    /// either law trips this pin — retune deliberately and repin.
    #[test]
    fn diagonal_tiebreak_is_pinned() {
        let map = Map::blank(16, 16);
        let mut pf = Pathfinder::new(&map);
        let route = pf
            .find_path(&map, Tile { x: 2, z: 2 }, Tile { x: 6, z: 6 })
            .unwrap_or_default();
        let pinned: Vec<Tile> = [
            (2, 2),
            (3, 2),
            (4, 2),
            (5, 2),
            (6, 2),
            (6, 3),
            (6, 4),
            (6, 5),
            (6, 6),
        ]
        .iter()
        .map(|&(x, z)| Tile { x, z })
        .collect();
        assert_eq!(route, pinned, "the tie-break law drifted");
    }

    /// Blocked tiles shape routes: a wall with one gap forces the
    /// detour through the gap, at exactly the detour's length.
    #[test]
    fn routes_detour_around_blocked_walls() {
        let mut map = Map::blank(16, 16);
        for z in 0..16 {
            if z != 10 {
                map.set_occupancy(Tile { x: 8, z }, Occupancy::BLOCKED);
            }
        }
        let mut pf = Pathfinder::new(&map);
        let start = Tile { x: 2, z: 4 };
        let goal = Tile { x: 14, z: 4 };
        let route = pf.find_path(&map, start, goal).unwrap_or_default();
        assert_valid_route(&map, &route, start, goal);
        assert!(route.contains(&Tile { x: 8, z: 10 }), "must thread the gap");
        assert_eq!(route.len(), 25, "12 east + 6 north + 6 south + start");
    }

    /// Failing queries fail closed: no route returns false with an
    /// empty buffer; bad endpoints (out of bounds, unwalkable) are
    /// refusals, not panics; `start == goal` is the trivial route.
    #[test]
    fn failing_queries_fail_closed() {
        let mut map = Map::blank(16, 16);
        for &(dx, dz) in &[
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            map.set_occupancy(
                Tile {
                    x: 8 + dx,
                    z: 8 + dz,
                },
                Occupancy::BLOCKED,
            );
        }
        let mut pf = Pathfinder::new(&map);
        let mut out = Vec::new();

        assert!(!pf.find_path_into(&map, Tile { x: 2, z: 2 }, Tile { x: 8, z: 8 }, &mut out));
        assert!(out.is_empty(), "a failed query must clear the buffer");

        assert!(!pf.find_path_into(&map, Tile { x: -1, z: 0 }, Tile { x: 2, z: 2 }, &mut out));
        assert!(!pf.find_path_into(&map, Tile { x: 2, z: 2 }, Tile { x: 99, z: 99 }, &mut out));

        assert!(pf.find_path_into(&map, Tile { x: 5, z: 5 }, Tile { x: 5, z: 5 }, &mut out));
        assert_eq!(out, vec![Tile { x: 5, z: 5 }]);
    }

    /// Generated maps route for real: across the meadow the route is
    /// found, valid, and repeatable — same query, same bytes.
    #[test]
    fn meadow_routes_are_valid_and_repeatable() {
        let map = meadow();
        let start = walkable_from(&map, 0, 0).unwrap_or(Tile { x: 0, z: 0 });
        let goal = walkable_from(&map, 32, 32).unwrap_or(Tile { x: 63, z: 63 });
        let mut pf = Pathfinder::new(&map);
        let a = pf.find_path(&map, start, goal).unwrap_or_default();
        let mut out = Vec::new();
        assert!(pf.find_path_into(&map, start, goal, &mut out));
        assert_eq!(a, out, "same query must reproduce the same route");
        assert!(a.len() > 32, "a cross-map route should be substantial");
        assert_valid_route(&map, &a, start, goal);
    }

    /// The reuse contract (the sim grid's precedent): a scratchpad
    /// reused across many queries produces exactly what a fresh one
    /// does — generation stamps must not leak between queries.
    #[test]
    fn reused_scratch_matches_fresh_construction() {
        let map = meadow();
        let mut rng = crate::sim::Rng::new(7);
        let mut reused = Pathfinder::new(&map);
        let mut out = Vec::new();
        for _ in 0..30 {
            let start = walkable_from(
                &map,
                (rng.next_u64() % 60) as i32,
                (rng.next_u64() % 60) as i32,
            )
            .unwrap_or(Tile { x: 0, z: 0 });
            let goal = walkable_from(
                &map,
                (rng.next_u64() % 60) as i32,
                (rng.next_u64() % 60) as i32,
            )
            .unwrap_or(Tile { x: 63, z: 63 });
            let via_reuse = reused.find_path(&map, start, goal);
            let mut fresh = Pathfinder::new(&map);
            let via_fresh = fresh.find_path(&map, start, goal);
            assert_eq!(via_reuse, via_fresh, "stale stamps leaked between queries");
            // And the reused buffer agrees with the allocating wrapper.
            assert!(reused.find_path_into(&map, start, goal, &mut out));
            assert_eq!(out, via_fresh.unwrap_or_default());
        }
    }

    /// The smoothing laws, once, for every route this module's other
    /// tests produce: a subsequence of the input (endpoints kept) with
    /// walkable line of sight between consecutive kept waypoints.
    fn assert_valid_smooth(map: &Map, route: &[Tile], smooth: &[Tile]) {
        assert_eq!(smooth.first(), route.first(), "smooth must keep the start");
        let Some(&last) = route.last() else { return };
        assert_eq!(smooth.last(), Some(&last), "smooth must keep the goal");
        let mut next = 0; // order-preserving subset check
        for &t in smooth {
            let Some(pos) = route[next..].iter().position(|&r| r == t) else {
                panic!("smoothed waypoint {t:?} is not on the route");
            };
            next += pos + 1;
        }
        for pair in smooth.windows(2) {
            assert!(
                line_of_sight(map, pair[0], pair[1]),
                "smoothed leg {:?} -> {:?} lost line of sight",
                pair[0],
                pair[1]
            );
        }
    }

    /// Line of sight is conservative: every touched tile must be
    /// walkable, an exact corner crossing counts both flanks, and the
    /// integer-exact comparison catches the perfect diagonal's corners.
    #[test]
    fn line_of_sight_is_conservative() {
        let mut map = Map::blank(16, 16);
        let a = Tile { x: 2, z: 2 };
        let b = Tile { x: 6, z: 6 };
        assert!(line_of_sight(&map, a, b), "blank diagonal must be visible");
        assert!(line_of_sight(&map, b, a), "sight is symmetric");
        assert!(line_of_sight(
            &map,
            Tile { x: 0, z: 0 },
            Tile { x: 0, z: 0 }
        ));
        // The center-to-center diagonal clips every lattice corner, so
        // blocking either flank at the first corner breaks it.
        map.set_occupancy(Tile { x: 3, z: 2 }, Occupancy::BLOCKED);
        assert!(
            !line_of_sight(&map, a, b),
            "a blocked flank must break the diagonal"
        );
        // Axis lines die on a mid-line tile.
        let mut map2 = Map::blank(16, 16);
        map2.set_occupancy(Tile { x: 0, z: 2 }, Occupancy::BLOCKED);
        assert!(!line_of_sight(
            &map2,
            Tile { x: 0, z: 0 },
            Tile { x: 0, z: 4 }
        ));
        // Off the blocking tile's row, sight returns.
        assert!(line_of_sight(
            &map2,
            Tile { x: 1, z: 0 },
            Tile { x: 1, z: 4 }
        ));
    }

    /// The stair-shuffle fix, pinned: on open ground a stair-stepped
    /// diagonal (exactly what 4-neighborhood A* hands back) collapses
    /// to one straight leg, and a wall in the diagonal's flank forces a
    /// bend to survive. Trivial routes pass through untouched.
    #[test]
    fn smooth_route_collapses_stairs_and_keeps_corners() {
        let map = Map::blank(16, 16);
        let stair: Vec<Tile> = [
            (2, 2),
            (3, 2),
            (3, 3),
            (4, 3),
            (4, 4),
            (5, 4),
            (5, 5),
            (6, 5),
            (6, 6),
        ]
        .iter()
        .map(|&(x, z)| Tile { x, z })
        .collect();
        let mut out = Vec::new();
        smooth_route(&map, &stair, &mut out);
        assert_eq!(
            out,
            vec![Tile { x: 2, z: 2 }, Tile { x: 6, z: 6 }],
            "an open stair must collapse to one diagonal leg"
        );

        // A blocked flank tile kills the far sightline, so the smooth
        // keeps an intermediate bend around it — route on the walled
        // map (A* detours; the fixture must be a route of that map).
        let mut walled = Map::blank(16, 16);
        walled.set_occupancy(Tile { x: 3, z: 2 }, Occupancy::BLOCKED);
        let mut pf = Pathfinder::new(&walled);
        let start = Tile { x: 2, z: 2 };
        let goal = Tile { x: 6, z: 6 };
        let route = pf.find_path(&walled, start, goal).unwrap_or_default();
        assert!(!route.is_empty());
        smooth_route(&walled, &route, &mut out);
        assert_valid_smooth(&walled, &route, &out);
        assert!(out.len() > 2, "the wall must force a bend: {out:?}");

        // The wall-detour route from the A* tests smooths legally and
        // loses the staircase around the gap's corners.
        let mut gap_map = Map::blank(16, 16);
        for z in 0..16 {
            if z != 10 {
                gap_map.set_occupancy(Tile { x: 8, z }, Occupancy::BLOCKED);
            }
        }
        let mut pf = Pathfinder::new(&gap_map);
        let start = Tile { x: 2, z: 4 };
        let goal = Tile { x: 14, z: 4 };
        let route = pf.find_path(&gap_map, start, goal).unwrap_or_default();
        assert_eq!(route.len(), 25);
        smooth_route(&gap_map, &route, &mut out);
        assert_valid_smooth(&gap_map, &route, &out);
        assert!(out.len() < route.len(), "the detour must shorten: {out:?}");

        // Trivial routes pass through; empty stays empty.
        let one = vec![Tile { x: 5, z: 5 }];
        smooth_route(&map, &one, &mut out);
        assert_eq!(out, one);
        smooth_route(&map, &[], &mut out);
        assert!(out.is_empty(), "out must be cleared, not appended");
    }

    /// Generated meadows smooth for real: every A* route across the
    /// seed-42 map obeys the smoothing laws, and the smoothed walk is
    /// never longer-legged into unwalkable ground.
    #[test]
    fn meadow_routes_smooth_lawfully() {
        let map = meadow();
        let mut pf = Pathfinder::new(&map);
        let mut rng = crate::sim::Rng::new(11);
        let mut route = Vec::new();
        let mut out = Vec::new();
        for _ in 0..30 {
            let start = walkable_from(
                &map,
                (rng.next_u64() % 60) as i32,
                (rng.next_u64() % 60) as i32,
            )
            .unwrap_or(Tile { x: 0, z: 0 });
            let goal = walkable_from(
                &map,
                (rng.next_u64() % 60) as i32,
                (rng.next_u64() % 60) as i32,
            )
            .unwrap_or(Tile { x: 63, z: 63 });
            if !pf.find_path_into(&map, start, goal, &mut route) {
                continue; // islands refuse; only found routes smooth
            }
            smooth_route(&map, &route, &mut out);
            assert_valid_smooth(&map, &route, &out);
        }
    }
}
