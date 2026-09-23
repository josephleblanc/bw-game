//! Steady-state zero-alloc contract (ADR 0004) for pathfinding: once
//! the scratchpad and the caller's buffer exist, queries allocate
//! nothing — generation stamps instead of clears, results into a
//! reused `Vec`. Own integration-test binary on purpose (the dhat
//! global allocator is process-wide; only one Profiler may run at a
//! time), the same isolation as the sim and character steady tests.

use bw_core::map::{GenParams, Map, Tile};
use bw_core::path::Pathfinder;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const QUERIES: usize = 300;

#[test]
fn steady_state_path_queries_allocate_nothing() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let map = Map::generate(42, GenParams::default());
    let mut pf = Pathfinder::new(&map);
    let mut out = Vec::new();

    // Deterministic walkable endpoints on the meadow (chosen before
    // the window opens — endpoint scanning may allocate, queries may
    // not).
    let mut endpoints = [(0, 0i32), (63, 63)];
    for &(x, z) in &[(0, 0), (63, 63)] {
        let mut t = Tile { x, z };
        while !map.walkable(t) {
            t.x = (t.x + 1) % 64;
        }
        endpoints[if x == 0 { 0 } else { 1 }] = (t.x, t.z);
    }
    let (a, b) = (
        Tile {
            x: endpoints[0].0,
            z: endpoints[0].1,
        },
        Tile {
            x: endpoints[1].0,
            z: endpoints[1].1,
        },
    );
    // A walled-off goal for the failing path (also steady-state).
    let mut blocked_map = Map::blank(16, 16);
    for z in 0..16i32 {
        for x in 0..16i32 {
            if (x - 8).abs() + (z - 8).abs() == 1 {
                blocked_map.set_occupancy(Tile { x, z }, bw_core::map::Occupancy::BLOCKED);
            }
        }
    }
    let mut blocked_pf = Pathfinder::new(&blocked_map);

    // The measured loop, as a function so warm-up runs the identical
    // query mix: capacities (the heap's first grow) and dhat's callsite
    // bookkeeping must be established before the window opens.
    fn run_queries(
        pf: &mut Pathfinder,
        map: &Map,
        a: Tile,
        b: Tile,
        blocked_pf: &mut Pathfinder,
        blocked_map: &Map,
        out: &mut Vec<Tile>,
    ) {
        for k in 0..QUERIES {
            // Alternate directions and interleave failures so both the
            // success and refusal paths are exercised.
            if k % 2 == 0 {
                pf.find_path_into(map, a, b, out);
            } else {
                pf.find_path_into(map, b, a, out);
            }
            if k % 50 == 0 {
                blocked_pf.find_path_into(
                    blocked_map,
                    Tile { x: 2, z: 2 },
                    Tile { x: 8, z: 8 },
                    out,
                );
                assert!(
                    out.is_empty(),
                    "the refused query must leave the buffer empty"
                );
            }
        }
    }

    run_queries(&mut pf, &map, a, b, &mut blocked_pf, &blocked_map, &mut out);
    let before = dhat::HeapStats::get();
    run_queries(&mut pf, &map, a, b, &mut blocked_pf, &blocked_map, &mut out);
    let after = dhat::HeapStats::get();

    assert_eq!(
        after.total_blocks,
        before.total_blocks,
        "steady state allocated {} blocks ({} bytes) over {QUERIES} queries",
        after.total_blocks - before.total_blocks,
        after.total_bytes - before.total_bytes,
    );
}
