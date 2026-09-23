//! Tactical-RPG camera projection (the PS2-era look of Final Fantasy
//! Tactics / Disgaea): a diamond checkerboard ground plane seen from a
//! fixed ~3/4 angle, with trees drawn as upright billboards standing on
//! their tiles — the classic sprite-on-tilted-grid trick. The tree itself
//! is 2D (its own plane, +y up); this module maps world ground/height
//! coordinates into screen space.
//!
//! The real renderer is still deliberately deferred (ADR 0002); the SVG
//! gallery output consumes this projection today, and the Bevy camera that
//! eventually replaces it will copy the same numbers.

use std::f32::consts::FRAC_1_SQRT_2;

/// Camera depression angle: how far the camera looks down from horizontal.
/// FFT reads as ~35°; 36° keeps a readable grid while tree billboards stay
/// dominant.
pub const PITCH_DEG: f32 = 36.0;
/// Ground-plane rotation: 45° turns the square tile grid into the classic
/// isometric diamond.
pub const YAW_DEG: f32 = 45.0;
/// World units per tile edge (the checkerboard square size).
pub const TILE: f32 = 110.0;

/// sin(36°), precomputed (trig is not const); derived from `PITCH_DEG`.
const SIN_PITCH: f32 = 0.587_785_27;
/// cos(45°) = sin(45°); the yaw factors collapse to this constant.
const COS_YAW: f32 = FRAC_1_SQRT_2;

/// Sun direction for ground shadows: how far a point at height `ty` (and
/// billboard offset `tx`) drifts over the ground, in world x/z. Tuned for
/// a shadow roughly 40% of tree height reaching toward the viewer's left.
const SHADOW_DX: f32 = 0.50;
const SHADOW_DZ: f32 = 0.42;

/// World-space center of tile `(i, j)` — where a tree billboard stands.
pub fn tile_center(i: i32, j: i32) -> (f32, f32) {
    ((i as f32 + 0.5) * TILE, (j as f32 + 0.5) * TILE)
}

/// Half-diagonals of one tile's projected diamond (screen units): the
/// exact screen shape a rendered tile must cover. The diamond is a
/// sheared square, so only a mesh can draw it exactly — sprites handle
/// everything else in this projection. (Consumed by the `viewer` build.)
#[cfg_attr(not(feature = "viewer"), allow(dead_code))]
pub fn tile_diamond() -> (f32, f32) {
    (TILE * COS_YAW, TILE * COS_YAW * SIN_PITCH)
}

/// Project a ground-plane point (world x, z) to screen space. Screen y
/// grows downward; +z runs toward the viewer (down-right on screen).
pub fn project_ground(gx: f32, gz: f32) -> (f32, f32) {
    ((gx - gz) * COS_YAW, (gx + gz) * COS_YAW * SIN_PITCH)
}

/// Project a billboard point: the tree plane at ground anchor `(gx, gz)`
/// with plane coordinates `(tx, ty)` (tx right, ty up). Billboards face
/// the camera, so height maps 1:1 to screen height (no foreshortening) —
/// exactly how FFT/Disgaea sprites stand on their tiles.
pub fn project_billboard(gx: f32, gz: f32, tx: f32, ty: f32) -> (f32, f32) {
    let (sx, sy) = project_ground(gx, gz);
    (sx + tx, sy - ty)
}

/// Project a tree-plane point's shadow onto the ground plane: the billboard
/// shape sheared onto the tiles along the sun direction.
pub fn project_shadow(gx: f32, gz: f32, tx: f32, ty: f32) -> (f32, f32) {
    // Billboard x runs along the screen-x ground direction (1, -1)/√2;
    // height pushes the shadow along the sun direction.
    project_ground(
        gx + (tx + SHADOW_DX * ty) * FRAC_1_SQRT_2,
        gz + (-tx + SHADOW_DZ * ty) * FRAC_1_SQRT_2,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_origin_is_screen_origin() {
        let (x, y) = project_ground(0.0, 0.0);
        assert_eq!((x, y), (0.0, 0.0));
    }

    /// Tiles project to diamonds: the four corners of a tile form a
    /// parallelogram wider than tall (foreshortened by sin(pitch)).
    #[test]
    fn tile_corners_form_a_wide_diamond() {
        let corners = [
            project_ground(0.0, 0.0),
            project_ground(TILE, 0.0),
            project_ground(TILE, TILE),
            project_ground(0.0, TILE),
        ];
        let width = corners[1].0 - corners[3].0;
        let height = corners[2].1 - corners[0].1;
        assert!(width > height, "ground not foreshortened: {width}x{height}");
        let side = TILE * std::f32::consts::FRAC_1_SQRT_2;
        assert!((width - 2.0 * side).abs() < 1e-3);
        assert!((height - 2.0 * side * SIN_PITCH).abs() < 1e-3);
    }

    /// The tile-diamond helper matches the projected tile corners exactly.
    #[test]
    fn tile_diamond_matches_projected_corners() {
        let (a, b) = tile_diamond();
        let (cx, cy) = project_ground(TILE * 0.5, TILE * 0.5);
        let corners = [
            project_ground(0.0, 0.0),
            project_ground(TILE, 0.0),
            project_ground(TILE, TILE),
            project_ground(0.0, TILE),
        ];
        for (x, y) in corners {
            assert!((x - cx).abs() <= a + 1e-3 && (y - cy).abs() <= b + 1e-3);
        }
        // The extreme corners sit exactly on the diagonal tips.
        assert!((corners[1].0 - (cx + a)).abs() < 1e-3);
        assert!((corners[2].1 - (cy + b)).abs() < 1e-3);
    }

    /// Billboards stand fully upright: height maps 1:1 to screen height,
    /// and +tx moves straight right on screen.
    #[test]
    fn billboard_height_is_undistorted() {
        let (bx, by) = project_billboard(0.0, 0.0, 0.0, 100.0);
        assert!((bx - 0.0).abs() < 1e-4);
        assert!((by - (-100.0)).abs() < 1e-4);
        let (rx, _) = project_billboard(0.0, 0.0, 40.0, 0.0);
        assert!((rx - 40.0).abs() < 1e-4);
    }

    /// Shadows stay on the ground side: a point's shadow always projects
    /// below the billboard point itself and shares the base at height 0.
    #[test]
    fn shadow_lies_below_and_anchors_at_base() {
        let (sx0, sy0) = project_shadow(10.0, 20.0, 0.0, 0.0);
        let (gx, gy) = project_ground(10.0, 20.0);
        assert!((sx0 - gx).abs() < 1e-4 && (sy0 - gy).abs() < 1e-4);
        let (_, sy_h) = project_shadow(10.0, 20.0, 0.0, 80.0);
        assert!(sy_h > sy0, "shadow must move toward the viewer with height");
    }
}
