//! Joint hierarchies and the maths that turns a pose into skinning matrices.

use glam::{Mat4, Vec3};

use crate::{RenderError, Transform};

/// A joint hierarchy and its rest (bind) pose.
///
/// Joint `j`'s transform is relative to its parent `parents[j]`. Joints
/// must be ordered so every parent comes before its children (index 0 is
/// a root). The rest pose is the pose the mesh was modelled in: drawing a
/// skinned mesh with its rest pose shows it undeformed.
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonDesc {
    pub parents: Vec<Option<usize>>,
    /// Local transform of each joint in the rest pose.
    pub rest_pose: Vec<Transform>,
}

impl SkeletonDesc {
    /// A straight chain of `joints` joints along +Y, from y = 0 to
    /// y = `length`. Joint `j`'s parent is `j - 1`. Pairs with
    /// [`SkinnedMeshData::tube`](super::SkinnedMeshData::tube).
    pub fn chain(joints: usize, length: f32) -> Self {
        let joints = joints.max(1);
        let segment = chain_segment(joints, length);
        Self {
            parents: (0..joints).map(|j| j.checked_sub(1)).collect(),
            rest_pose: (0..joints)
                .map(|j| {
                    let offset = if j == 0 { 0.0 } else { segment };
                    Transform::from_translation(Vec3::new(0.0, offset, 0.0))
                })
                .collect(),
        }
    }

    pub fn joint_count(&self) -> usize {
        self.parents.len()
    }
}

/// Distance between neighbouring joints of a [`SkeletonDesc::chain`].
pub(crate) fn chain_segment(joints: usize, length: f32) -> f32 {
    if joints > 1 { length / (joints - 1) as f32 } else { 0.0 }
}

/// A validated skeleton with precomputed inverse bind matrices.
#[derive(Debug, Clone)]
pub(crate) struct Skeleton {
    parents: Vec<Option<usize>>,
    rest_pose: Vec<Transform>,
    /// Inverse of each joint's model-space rest transform: takes a vertex
    /// from model space into that joint's local space.
    inverse_bind: Vec<Mat4>,
}

impl Skeleton {
    pub(crate) fn new(desc: &SkeletonDesc) -> Result<Self, RenderError> {
        let n = desc.parents.len();
        if n == 0 {
            return Err(RenderError::InvalidSkeleton("no joints".into()));
        }
        if desc.rest_pose.len() != n {
            return Err(RenderError::InvalidSkeleton(format!(
                "{n} parents but {} rest-pose transforms",
                desc.rest_pose.len()
            )));
        }
        for (joint, parent) in desc.parents.iter().enumerate() {
            if let Some(parent) = *parent
                && parent >= joint
            {
                return Err(RenderError::InvalidSkeleton(format!(
                    "joint {joint} has parent {parent}; parents must come before their children"
                )));
            }
        }

        let mut skeleton = Self {
            parents: desc.parents.clone(),
            rest_pose: desc.rest_pose.clone(),
            inverse_bind: Vec::new(),
        };
        let mut globals = Vec::new();
        skeleton.model_space(&desc.rest_pose, &mut globals);
        skeleton.inverse_bind = globals.iter().map(Mat4::inverse).collect();
        Ok(skeleton)
    }

    pub(crate) fn joint_count(&self) -> usize {
        self.parents.len()
    }

    /// Model-space transform of every joint for `pose` (local transforms).
    /// Joints missing from `pose` use the rest pose; extra entries are ignored.
    fn model_space(&self, pose: &[Transform], out: &mut Vec<Mat4>) {
        out.clear();
        for (joint, parent) in self.parents.iter().enumerate() {
            let local = pose.get(joint).unwrap_or(&self.rest_pose[joint]).to_matrix();
            // Parents come first, so out[parent] is already computed.
            let global = match parent {
                Some(parent) => out[*parent] * local,
                None => local,
            };
            out.push(global);
        }
    }

    /// Append one skinning matrix per joint to `out`:
    /// `model_space(pose)[j] * inverse_bind[j]`, which moves a vertex from
    /// where it sits in the rest pose to where joint `j` now carries it.
    /// `scratch` is reused between calls to avoid allocating.
    pub(crate) fn skinning_matrices(&self, pose: &[Transform], scratch: &mut Vec<Mat4>, out: &mut Vec<Mat4>) {
        self.model_space(pose, scratch);
        out.extend(scratch.iter().zip(&self.inverse_bind).map(|(global, inverse)| *global * *inverse));
    }
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec3};

    use super::*;

    #[test]
    fn rest_pose_gives_identity_matrices() {
        let skeleton = Skeleton::new(&SkeletonDesc::chain(5, 4.0)).unwrap();
        let mut out = Vec::new();
        skeleton.skinning_matrices(&[], &mut Vec::new(), &mut out);
        assert_eq!(out.len(), 5);
        for m in out {
            assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-5), "{m:?}");
        }
    }

    #[test]
    fn rotating_the_root_carries_descendants() {
        // Two joints: root at the origin, child 1 unit up.
        let desc = SkeletonDesc::chain(2, 1.0);
        let skeleton = Skeleton::new(&desc).unwrap();

        // Rotate the root 90° about Z: +Y swings to -X.
        let mut pose = desc.rest_pose.clone();
        pose[0].rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

        let mut out = Vec::new();
        skeleton.skinning_matrices(&pose, &mut Vec::new(), &mut out);

        // A vertex at the child joint's rest position follows the child.
        let moved = out[1].transform_point3(Vec3::new(0.0, 1.0, 0.0));
        assert!(moved.abs_diff_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-5), "{moved:?}");
    }

    #[test]
    fn rejects_children_before_parents() {
        let desc = SkeletonDesc {
            parents: vec![Some(1), None],
            rest_pose: vec![Transform::IDENTITY; 2],
        };
        assert!(Skeleton::new(&desc).is_err());
    }
}
