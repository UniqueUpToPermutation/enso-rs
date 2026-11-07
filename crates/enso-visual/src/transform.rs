use glam::{Mat4, Quat, Vec3};

/// Position, rotation and scale of an object.
///
/// Kept as separate parts (rather than a matrix) so that two transforms can
/// be interpolated correctly: translation and scale lerp, rotation slerps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_translation(translation: Vec3) -> Self {
        Self { translation, ..Self::IDENTITY }
    }

    pub fn from_rotation(rotation: Quat) -> Self {
        Self { rotation, ..Self::IDENTITY }
    }

    pub fn with_translation(mut self, translation: Vec3) -> Self {
        self.translation = translation;
        self
    }

    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    pub fn with_uniform_scale(self, scale: f32) -> Self {
        self.with_scale(Vec3::splat(scale))
    }

    pub fn to_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// `t = 0` gives `self`, `t = 1` gives `other`.
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            translation: self.translation.lerp(other.translation, t),
            rotation: self.rotation.slerp(other.rotation, t),
            scale: self.scale.lerp(other.scale, t),
        }
    }
}

impl From<Vec3> for Transform {
    fn from(translation: Vec3) -> Self {
        Self::from_translation(translation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_endpoints_and_midpoint() {
        let a = Transform::from_translation(Vec3::ZERO);
        let b = Transform::from_translation(Vec3::new(2.0, 0.0, 0.0))
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
            .with_uniform_scale(3.0);

        assert_eq!(a.lerp(&b, 0.0), a);
        let end = a.lerp(&b, 1.0);
        assert!(end.translation.abs_diff_eq(b.translation, 1e-6));
        assert!(end.rotation.abs_diff_eq(b.rotation, 1e-6));

        let mid = a.lerp(&b, 0.5);
        assert!(mid.translation.abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-6));
        assert!(mid.scale.abs_diff_eq(Vec3::splat(2.0), 1e-6));
        let expected = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        assert!(mid.rotation.abs_diff_eq(expected, 1e-5));
    }
}
