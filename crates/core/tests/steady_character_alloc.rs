//! Steady-state zero-alloc contract for the character sim (ADR 0004):
//! once a character, its skeleton, and its pose buffer exist, stepping
//! and posing must allocate nothing — locomotion is pure math over
//! preallocated arrays and stack scratch. Own integration-test binary
//! for the same reason as `steady_alloc.rs`: the dhat global allocator
//! is process-wide.

use bw_core::character::{Character, CharacterPose, MovementInput, Skeleton, circle_input};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const TICKS: usize = 600;
const DT: f32 = bw_core::time::SIM_DT;

#[test]
fn steady_state_character_step_and_pose_allocate_nothing() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let skel = Skeleton::humanoid();
    let mut walkers = [
        Character::new(bw_core::math::Vec3::ZERO, 0.0),
        Character::new(bw_core::math::Vec3::new(5.0, 0.0, 0.0), 1.2),
    ];
    walkers[1].scale = 1.07;
    let mut poses: Vec<CharacterPose> = walkers.iter().map(|_| CharacterPose::new(&skel)).collect();
    // Idle, walk, and run input bands all exercise the same pose pass;
    // the action requests drive the jump/punch channels (launch, arc,
    // landing absorption, windup/strike/recover) through it too.
    let mut inputs = vec![
        MovementInput::default(),
        circle_input(5.0, 1.35, 1.0),
        circle_input(7.0, 3.6, -1.0),
    ];
    let mut bounce = circle_input(5.0, 1.35, 1.0);
    bounce.jump = true;
    let mut flurry = circle_input(7.0, 3.6, -1.0);
    flurry.punch = true;
    inputs.push(bounce);
    inputs.push(flurry);

    // Warm: dhat callsite bookkeeping and first-touch growth settle
    // before the measured window opens.
    for (round, input) in inputs.iter().enumerate() {
        walkers[round % walkers.len()].step(DT, input);
        for (w, pose) in walkers.iter().zip(poses.iter_mut()) {
            w.pose_into(&skel, pose);
        }
    }

    let before = dhat::HeapStats::get();
    for k in 0..TICKS {
        let input = &inputs[k % inputs.len()];
        for w in &mut walkers {
            w.step(DT, input);
        }
        for (w, pose) in walkers.iter().zip(poses.iter_mut()) {
            w.pose_into(&skel, pose);
        }
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
