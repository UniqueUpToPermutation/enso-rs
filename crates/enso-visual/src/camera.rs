use glam::{Mat4, Vec3, camera};

/// A perspective camera, part of every [`RenderWorld`](crate::RenderWorld).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    /// Vertical field of view, in radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            eye: Vec3::new(0.0, 2.0, 6.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: 60f32.to_radians(),
            near: 0.1,
            far: 100.0,
        }
    }
}

impl Camera {
    /// Projection * view, right-handed. wgpu's clip space matches the
    /// DirectX convention (depth 0..1, Y up), hence `proj::directx`.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = camera::rh::view::look_at_mat4(self.eye, self.target, self.up);
        let proj = camera::rh::proj::directx::perspective(self.fov_y, aspect, self.near, self.far);
        proj * view
    }

    /// `t = 0` gives `self`, `t = 1` gives `other`. Clip planes snap to `other`.
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            eye: self.eye.lerp(other.eye, t),
            target: self.target.lerp(other.target, t),
            up: self.up.lerp(other.up, t).normalize_or(other.up),
            fov_y: self.fov_y + (other.fov_y - self.fov_y) * t,
            near: other.near,
            far: other.far,
        }
    }
}
