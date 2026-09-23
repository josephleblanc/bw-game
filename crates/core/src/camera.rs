//! The tactical camera orientation (ADR 0006), as law: one set of
//! constants every surface reads — the walker viewer's follow camera,
//! the map gallery's SVG orthographic projection, and the future map
//! viewer's pan/zoom. The constants were promoted out of the walker
//! viewer when the map gallery became a second reader, so the
//! surfaces cannot drift apart (a test here pins the numbers; each
//! surface pins its own use of them).

use crate::math::Vec3;

/// Azimuth of the eye about +y, degrees off the +z axis toward +x.
/// Deliberately off the 45° diagonal: at exactly 45° a tiled ground
/// reads as perfectly regular wallpaper (one family of tile edges
/// dominates); the offset gives the view a directional hierarchy —
/// the same trick isometric-style games use when they offset the
/// classic angle.
pub const TACTICAL_AZIMUTH_DEG: f32 = 49.0;

/// Elevation of the eye above the ground plane, degrees — the SVG
/// establishing shot's steepness.
pub const TACTICAL_ELEVATION_DEG: f32 = 15.5;

/// Unit vector from the focus toward the eye: the direction a follow
/// camera offsets along, and the direction an orthographic render
/// projects against.
pub fn tactical_dir() -> Vec3 {
    let az = TACTICAL_AZIMUTH_DEG.to_radians();
    let el = TACTICAL_ELEVATION_DEG.to_radians();
    Vec3::new(el.cos() * az.sin(), el.sin(), el.cos() * az.cos())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pinned orientation: ADR 0006's numbers — unit length, above
    /// the ground, 49° azimuth deliberately clear of the 45° diagonal,
    /// ~15.5° elevation. Any change to the constants or the formula
    /// trips this.
    #[test]
    fn tactical_orientation_is_pinned() {
        let d = tactical_dir();
        assert!(
            (d.length() - 1.0).abs() < 1e-5,
            "not a unit direction: {d:?}"
        );
        let azimuth = d.x.atan2(d.z).to_degrees();
        let elevation = d.y.asin().to_degrees();
        assert!(
            (azimuth - TACTICAL_AZIMUTH_DEG).abs() < 0.01,
            "azimuth drifted: {azimuth}"
        );
        assert!(
            (elevation - TACTICAL_ELEVATION_DEG).abs() < 0.01,
            "elevation drifted: {elevation}"
        );
        assert!(
            (azimuth - 45.0).abs() >= 4.0,
            "back on the 45° diagonal — tiles read as wallpaper"
        );
        assert!(d.y > 0.0, "the eye must sit above the ground plane");
        assert!(
            d.y < 0.5,
            "the elevation is a tactical rake, not a top-down"
        );
    }
}
