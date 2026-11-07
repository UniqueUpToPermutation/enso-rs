//! CPU-side skinned mesh data and procedural primitives.

use std::f32::consts::TAU;

use glam::{Vec2, Vec3};

use super::skeleton::chain_segment;

/// One vertex of a skinned mesh. Matches `SkinnedVertexIn` in `prelude.wgsl`.
///
/// Each vertex follows up to four joints: its final position is the
/// weighted sum of where each joint would carry it. Weights should add up
/// to 1; unused slots have weight 0.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SkinnedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Indices into the skeleton's joints.
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

/// Skinned geometry on the CPU, before upload. Triangles are
/// counter-clockwise when seen from the front.
#[derive(Debug, Clone, Default)]
pub struct SkinnedMeshData {
    pub vertices: Vec<SkinnedVertex>,
    pub indices: Vec<u32>,
}

impl SkinnedMeshData {
    /// An open tube along +Y from y = 0 to y = `length`, skinned to a
    /// [`SkeletonDesc::chain`](super::SkeletonDesc::chain) with the same
    /// `joints` and `length`. Each ring blends between its two nearest
    /// joints, so the tube bends smoothly. `around` = segments around the
    /// circumference, `along` = rings along the length.
    pub fn tube(radius: f32, length: f32, around: u32, along: u32, joints: usize) -> Self {
        let around = around.max(3);
        let along = along.max(1);
        let joints = joints.max(1);
        let segment = chain_segment(joints, length);
        let mut mesh = Self::default();

        for ring in 0..=along {
            let v = ring as f32 / along as f32;
            let y = v * length;

            // Position along the chain, in joints: 2.25 = a quarter of the
            // way from joint 2 to joint 3.
            let (lower, blend) = if joints > 1 {
                let t = y / segment;
                let lower = (t.floor() as usize).min(joints - 2);
                (lower, (t - lower as f32).clamp(0.0, 1.0))
            } else {
                (0, 0.0)
            };
            let upper = (lower + 1).min(joints - 1);

            for step in 0..=around {
                let u = step as f32 / around as f32;
                let angle = u * TAU;
                let normal = Vec3::new(angle.cos(), 0.0, angle.sin());
                mesh.vertices.push(SkinnedVertex {
                    position: (normal * radius + Vec3::Y * y).to_array(),
                    normal: normal.to_array(),
                    uv: Vec2::new(u, v).to_array(),
                    joints: [lower as u16, upper as u16, 0, 0],
                    weights: [1.0 - blend, blend, 0.0, 0.0],
                });
            }
        }

        let stride = around + 1;
        for ring in 0..along {
            for step in 0..around {
                let a = ring * stride + step; // this ring, this angle
                let b = a + 1; //                this ring, next angle
                let c = a + stride; //           next ring up, this angle
                let d = c + 1;
                mesh.indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tube_faces_point_outward() {
        let mesh = SkinnedMeshData::tube(0.5, 2.0, 12, 6, 4);
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| mesh.vertices[i as usize]);
            let (pa, pb, pc) = (Vec3::from(a.position), Vec3::from(b.position), Vec3::from(c.position));
            let face_normal = (pb - pa).cross(pc - pa);
            let vertex_normal = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
            assert!(face_normal.dot(vertex_normal) > 0.0, "inward-facing triangle {tri:?}");
        }
    }

    #[test]
    fn tube_weights_sum_to_one_and_stay_in_range() {
        let joints = 5;
        let mesh = SkinnedMeshData::tube(0.5, 3.0, 8, 9, joints);
        for v in &mesh.vertices {
            let sum: f32 = v.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "{v:?}");
            assert!(v.joints.iter().all(|&j| usize::from(j) < joints), "{v:?}");
        }
    }
}
