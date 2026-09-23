//! Steady-state zero-alloc contract for the tree pose pass (ADR 0004):
//! once a tree and its pose buffer exist, ticking the pose must allocate
//! nothing — growth and sway are pure math over preallocated arrays.
//! Own integration-test binary for the same reason as `steady_alloc.rs`:
//! the dhat global allocator is process-wide.

use bw_core::tree::{CALM_WIND, Tree, TreeParams, TreePose};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const TICKS: usize = 600;
const DT: f32 = bw_core::time::SIM_DT;

#[test]
fn steady_state_tree_pose_allocates_nothing() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let tree = Tree::generate(&TreeParams::oak(), 42);
    let mut pose = TreePose::new(&tree);

    // Warm: dhat callsite bookkeeping and any first-touch growth settle
    // before the measured window opens.
    tree.pose_into(&mut pose, 0.0, &CALM_WIND);

    let before = dhat::HeapStats::get();
    for k in 0..TICKS {
        tree.pose_into(&mut pose, k as f32 * DT, &CALM_WIND);
    }
    let after = dhat::HeapStats::get();

    assert_eq!(
        after.total_blocks,
        before.total_blocks,
        "steady state allocated {} blocks ({} bytes) over {TICKS} ticks",
        after.total_blocks - before.total_blocks,
        after.total_bytes - before.total_bytes,
    );
}
