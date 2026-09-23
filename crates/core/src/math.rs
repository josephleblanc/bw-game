//! Minimal 3D math for engine-agnostic simulation (ADR 0002): a `Vec3`
//! and a unit `Quat`, hand-rolled in plain f32 so `bw-core` stays
//! dependency-free and benchmarkable without the engine — the same call
//! that kept the RNG, checksums, and stats in-house. Renderers convert
//! these to their engine's types at the boundary; nothing here knows
//! Bevy exists.
//!
//! Conventions: right-handed, `+y` up. Quaternions are Hamilton products
//! of axis-angle rotations; `from_axis_angle` requires a unit axis (the
//! one thing callers must guarantee — asserting it on the hot path would
//! cost more than the rotation itself).

/// Three-component vector (f32) with the usual arithmetic operators;
/// products and readings with names (`dot`, `cross`, `lerp`) stay named.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

impl std::ops::Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub const fn splat(v: f32) -> Self {
        Self { x: v, y: v, z: v }
    }

    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// Unit vector in the same direction; `None` for the zero vector
    /// (normalizing zero silently produces NaNs — callers decide).
    pub fn normalized(self) -> Option<Self> {
        let len = self.length();
        if len > 0.0 {
            Some(self * (1.0 / len))
        } else {
            None
        }
    }

    /// `self·(1−t) + other·t` — endpoint-exact at t = 0 and t = 1 (the
    /// `a + (b−a)·t` form is not), the same form the tree viewer's pose
    /// interpolation pinned by test.
    pub fn lerp(self, other: Self, t: f32) -> Self {
        self * (1.0 - t) + other * t
    }

    /// Bit pattern for checksum mixing (order-stable, NaN-free paths only).
    pub fn to_bits(self) -> [u32; 3] {
        [self.x.to_bits(), self.y.to_bits(), self.z.to_bits()]
    }
}

/// Unit quaternion: rotation as `w + xi + yj + zk`. Products of unit
/// quaternions stay unit to float rounding; renormalization is the
/// renderer's business if it accumulates them for long chains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl std::ops::Mul for Quat {
    type Output = Self;
    /// Hamilton product: applying `self * o` rotates as `o` first, then
    /// `self` — i.e. `o` is the local/child rotation.
    fn mul(self, o: Self) -> Self {
        let Self {
            x: ax,
            y: ay,
            z: az,
            w: aw,
        } = self;
        let Self {
            x: bx,
            y: by,
            z: bz,
            w: bw,
        } = o;
        Self {
            x: aw * bx + ax * bw + ay * bz - az * by,
            y: aw * by - ax * bz + ay * bw + az * bx,
            z: aw * bz + ax * by - ay * bx + az * bw,
            w: aw * bw - ax * bx - ay * by - az * bz,
        }
    }
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// Right-handed rotation by `angle` radians about a unit `axis`.
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Self {
        let half = angle * 0.5;
        let s = half.sin();
        Self {
            x: axis.x * s,
            y: axis.y * s,
            z: axis.z * s,
            w: half.cos(),
        }
    }

    /// Rotation about `+x` by `angle` radians (right-handed: `+y` turns
    /// toward `+z`). The sagittal-plane joint of the character rig.
    pub fn from_rotation_x(angle: f32) -> Self {
        Self::from_axis_angle(Vec3::new(1.0, 0.0, 0.0), angle)
    }

    /// Rotation about `+y` by `angle` radians (right-handed: `+z` turns
    /// toward `+x`). Yaw/headings and pelvis/torso counter-rotation.
    pub fn from_rotation_y(angle: f32) -> Self {
        Self::from_axis_angle(Vec3::new(0.0, 1.0, 0.0), angle)
    }

    /// Rotate `v` by this quaternion (unit assumed).
    pub fn rotate(self, v: Vec3) -> Vec3 {
        // The cross-product form: cheaper and more accurate than
        // converting to a matrix for one-off vectors.
        let axis = Vec3::new(self.x, self.y, self.z);
        let t = axis.cross(v) * 2.0;
        v + t * self.w + axis.cross(t)
    }

    /// Length (1.0 ± float drift for composed unit rotations).
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    #[test]
    fn axis_angle_quarter_turns_match_known_vectors() {
        // +x quarter turn sends +y to +z (right-handed).
        let q = Quat::from_rotation_x(FRAC_PI_2);
        let v = q.rotate(Vec3::new(0.0, 1.0, 0.0));
        assert!((v.x - 0.0).abs() < 1e-6 && (v.y - 0.0).abs() < 1e-6 && (v.z - 1.0).abs() < 1e-6);
        // +y quarter turn sends +z to +x.
        let q = Quat::from_rotation_y(FRAC_PI_2);
        let v = q.rotate(Vec3::new(0.0, 0.0, 1.0));
        assert!((v.x - 1.0).abs() < 1e-6 && (v.z - 0.0).abs() < 1e-6);
    }

    #[test]
    fn identity_rotates_nothing() {
        let v = Vec3::new(0.3, -1.2, 0.7);
        assert_eq!(Quat::IDENTITY.rotate(v), v);
    }

    /// Composition order: `a * b` applies `b` first. Rotating +y by
    /// (+x 90° then +y 90°) must land on +x — the inner rotation moves
    /// the vector into the plane the outer rotation acts on.
    #[test]
    fn composition_applies_inner_rotation_first() {
        let a = Quat::from_rotation_y(FRAC_PI_2);
        let b = Quat::from_rotation_x(FRAC_PI_2);
        let v = Vec3::new(0.0, 1.0, 0.0);
        let composed = (a * b).rotate(v); // b first: +y -> +z, then a: +z -> +x
        assert!((composed.x - 1.0).abs() < 1e-6);
        let separate = a.rotate(b.rotate(v));
        let d = composed - separate;
        assert!(
            d.x.abs() < 1e-5 && d.y.abs() < 1e-5 && d.z.abs() < 1e-5,
            "composition and chained rotation disagree: {composed:?} vs {separate:?}"
        );
    }

    #[test]
    fn unit_rotations_stay_unit() {
        let mut q =
            Quat::from_rotation_x(0.37) * Quat::from_rotation_y(-1.1) * Quat::from_rotation_x(2.3);
        for _ in 0..64 {
            q = q * Quat::from_rotation_y(0.05);
        }
        assert!((q.length() - 1.0).abs() < 1e-4, "drifted to {}", q.length());
    }

    #[test]
    fn vec_ops_match_manual_math() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(-4.0, 0.5, 2.0);
        assert_eq!(a + b, Vec3::new(-3.0, 2.5, 5.0));
        assert_eq!(a - b, Vec3::new(5.0, 1.5, 1.0));
        assert_eq!(a * 2.0, Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(a.dot(b), 3.0);
        // Right-handed frame: x × y = z.
        assert_eq!(
            Vec3::new(1.0, 0.0, 0.0).cross(Vec3::new(0.0, 1.0, 0.0)),
            Vec3::new(0.0, 0.0, 1.0)
        );
    }

    #[test]
    fn normalize_and_lerp_behave() {
        let a = Vec3::new(3.0, 0.0, 4.0);
        let Some(n) = a.normalized() else {
            panic!("nonzero vector must normalize");
        };
        assert_eq!(n, Vec3::new(0.6, 0.0, 0.8));
        assert_eq!(Vec3::ZERO.normalized(), None);
        let b = Vec3::new(0.0, 10.0, 0.0);
        assert_eq!(a.lerp(b, 0.0), a, "endpoint t=0 exact");
        assert_eq!(a.lerp(b, 1.0), b, "endpoint t=1 exact");
        let mid = a.lerp(b, 0.5);
        assert!((mid.x - 1.5).abs() < 1e-6 && (mid.y - 5.0).abs() < 1e-6);
    }

    /// A diagonal rotation round-trips under the inverse (conjugate),
    /// proving `rotate` and composition are consistent rotations, not skew.
    #[test]
    fn rotation_inverts_through_conjugate() {
        let Some(axis) = Vec3::new(1.0, 1.0, 1.0).normalized() else {
            panic!("diagonal axis must normalize");
        };
        let q = Quat::from_axis_angle(axis, FRAC_PI_4);
        let v = Vec3::new(0.2, 0.9, -0.4);
        let conj = Quat {
            x: -q.x,
            y: -q.y,
            z: -q.z,
            w: q.w,
        };
        let round = conj.rotate(q.rotate(v));
        assert!(
            (round.x - v.x).abs() < 1e-6
                && (round.y - v.y).abs() < 1e-6
                && (round.z - v.z).abs() < 1e-6
        );
    }
}
