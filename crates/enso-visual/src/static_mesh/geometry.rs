//! CPU-side mesh data and a few procedural primitives.

use std::f32::consts::{PI, TAU};

use glam::{Vec2, Vec3};

/// One vertex of a static mesh. Matches `VertexIn` in `prelude.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

impl Vertex {
    pub fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self {
            position: position.to_array(),
            normal: normal.to_array(),
            uv: uv.to_array(),
        }
    }
}

/// Geometry on the CPU, before upload. Triangles are counter-clockwise when
/// seen from the front.
#[derive(Debug, Clone, Default)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshData {
    /// A unit quad in the XY plane, facing +Z.
    pub fn quad(size: f32) -> Self {
        let mut m = Self::default();
        m.push_face(Vec3::Z, Vec3::Y, size * 0.5, Vec3::ZERO);
        m
    }

    /// An axis-aligned cube centred on the origin. 24 vertices so each face
    /// has its own flat normal and full 0..1 UVs.
    pub fn cube(size: f32) -> Self {
        let h = size * 0.5;
        let mut m = Self::default();
        // (face normal, face "up" direction)
        for (n, up) in [
            (Vec3::X, Vec3::Y),
            (Vec3::NEG_X, Vec3::Y),
            (Vec3::Z, Vec3::Y),
            (Vec3::NEG_Z, Vec3::Y),
            (Vec3::Y, Vec3::NEG_Z),
            (Vec3::NEG_Y, Vec3::Z),
        ] {
            m.push_face(n, up, h, n * h);
        }
        m
    }

    /// A UV sphere. `segments` around the equator, `rings` from pole to pole.
    pub fn uv_sphere(radius: f32, segments: u32, rings: u32) -> Self {
        let segments = segments.max(3);
        let rings = rings.max(2);
        let mut m = Self::default();

        for r in 0..=rings {
            let v = r as f32 / rings as f32;
            let theta = v * PI;
            for s in 0..=segments {
                let u = s as f32 / segments as f32;
                let phi = u * TAU;
                let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
                m.vertices.push(Vertex::new(n * radius, n, Vec2::new(u, v)));
            }
        }

        let stride = segments + 1;
        for r in 0..rings {
            for s in 0..segments {
                let a = r * stride + s;
                let b = a + stride;
                m.indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
            }
        }
        m
    }

    /// Append a square face with the given outward normal, centred at `center`.
    /// `up` must be perpendicular to `normal`; `half` is half the edge length.
    fn push_face(&mut self, normal: Vec3, up: Vec3, half: f32, center: Vec3) {
        // right x up = normal, so the corners below wind counter-clockwise
        // when viewed from the side the normal points to.
        let right = up.cross(normal);
        let base = self.vertices.len() as u32;
        let corners = [
            (-right - up, Vec2::new(0.0, 1.0)),
            (right - up, Vec2::new(1.0, 1.0)),
            (right + up, Vec2::new(1.0, 0.0)),
            (-right + up, Vec2::new(0.0, 0.0)),
        ];
        for (offset, uv) in corners {
            self.vertices.push(Vertex::new(center + offset * half, normal, uv));
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every triangle's winding should agree with its vertex normals,
    /// otherwise back-face culling would hide it.
    fn assert_outward_winding(mesh: &MeshData) {
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| mesh.vertices[i as usize]);
            let (pa, pb, pc) = (Vec3::from(a.position), Vec3::from(b.position), Vec3::from(c.position));
            let face_normal = (pb - pa).cross(pc - pa);
            if face_normal.length_squared() < 1e-12 {
                continue; // degenerate triangle at a sphere pole
            }
            let vertex_normal = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
            assert!(face_normal.dot(vertex_normal) > 0.0, "inward-facing triangle {tri:?}");
        }
    }

    #[test]
    fn cube_faces_point_outward() {
        let cube = MeshData::cube(1.0);
        assert_eq!(cube.vertices.len(), 24);
        assert_eq!(cube.indices.len(), 36);
        assert_outward_winding(&cube);
    }

    #[test]
    fn sphere_faces_point_outward() {
        assert_outward_winding(&MeshData::uv_sphere(1.0, 16, 8));
    }

    #[test]
    fn quad_faces_plus_z() {
        assert_outward_winding(&MeshData::quad(1.0));
    }
}
