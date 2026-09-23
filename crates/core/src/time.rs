//! The tick contract: the law for how time advances in this game.
//!
//! 1. **The simulation advances only in fixed steps of `SIM_DT`** (60 Hz).
//!    Sim systems receive or read `SIM_DT`; they never sample a wall clock
//!    and never use a variable dt. This is what makes `(scene, seed,
//!    ticks)` reproduce a checksum exactly (ADR 0003, D4) and what keeps
//!    the benchmark convention honest (`600 ticks` = 10 s of sim time).
//! 2. **Render paths sample time; they never advance it.** The offline
//!    SVG renderer evaluates `pose(t)` at fixed sample points; a live
//!    viewer draws some `t` the sim already reached. If a renderer seems
//!    to need to move time, the fix is more sim steps, not a render-side
//!    clock.
//! 3. **Wall-clock time exists only at an interactive boundary**, as the
//!    classic fix-your-timestep accumulator: on each presented frame,
//!    `acc += frame_delta; while acc >= SIM_DT { tick(); acc -= SIM_DT }`,
//!    then draw with `alpha = acc / SIM_DT`, interpolating between the
//!    last two sim states (the pose buffers are the sim side of that
//!    mirror; the `Segment` components are the render side). Headless
//!    measurement binaries have no interactive boundary and no wall clock
//!    at all — they call the tick directly.
//!
//! Everything else about scheduling (sub-stepping fast objects, network
//! lockstep, replays) builds on these three rules; changing the value
//! later invalidates tuned constants and trend series alike, so treat
//! `TICK_HZ` as a compatibility surface.

/// Fixed simulation rate in hertz. 60 because the galleries, budgets, and
/// trend baselines were all measured at this rate (see the module docs).
pub const TICK_HZ: u32 = 60;

/// Seconds of sim time per tick — the only dt the simulation may use.
pub const SIM_DT: f32 = 1.0 / TICK_HZ as f32;

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the benchmark convention: 600 ticks is 10 s of sim time, and
    /// the value stays bit-identical to the historical `1.0 / 60.0`.
    /// (f32 cannot represent 1/60 exactly, hence the absolute tolerance
    /// on the 10-second product rather than an epsilon comparison.)
    #[test]
    fn tick_rate_matches_benchmark_convention() {
        assert_eq!(TICK_HZ, 60);
        assert_eq!(SIM_DT, 1.0 / 60.0);
        assert!((600.0 * SIM_DT - 10.0).abs() < 1e-4);
    }
}
