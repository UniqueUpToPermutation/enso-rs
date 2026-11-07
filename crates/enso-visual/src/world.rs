//! [`RenderWorld`]: a description of everything to draw for one simulation
//! tick.

use std::collections::HashMap;

use glam::Vec4;

use crate::{
    Camera, MaterialId, Transform,
    skinned_mesh::{PoseRange, SkinnedDraw},
    sky::Sky,
    static_mesh::MeshDraw,
};

/// The four `vec4` knobs of a material (`material.params[0..4]` in WGSL).
pub type MaterialParams = [Vec4; 4];

/// Everything the renderer should draw for one simulation tick.
///
/// Pure data: only IDs, numbers and glam types, no GPU handles. It is
/// `Send`, so the simulation can build it on its own thread.
///
/// The renderer is given *two* of these, the previous and current tick, plus
/// an `alpha` to blend between them (see [`Renderer::render`]). A typical
/// loop keeps two worlds and advances them each tick:
///
/// ```ignore
/// prev.clone_from(&curr);  // copy, reusing prev's allocations
/// curr.clear();            // draws/params gone; camera, time, clear colour carry over
/// simulate(&mut curr);     // fill in this tick
/// ```
///
/// (Swapping instead of copying would leave `curr` holding the world from
/// *two* ticks ago, so anything not re-set every tick would jump back.)
///
/// [`Renderer::render`]: crate::Renderer::render
#[derive(Debug, Clone)]
pub struct RenderWorld {
    /// Simulation time in seconds. Interpolated, then given to shaders as
    /// `globals.time`.
    pub time: f64,
    pub camera: Camera,
    /// Linear RGBA. Visible wherever nothing is drawn (i.e. when there is
    /// no sky).
    pub clear_color: Vec4,
    /// Drawn first, behind everything. Like the camera, it is kept by
    /// [`clear`](Self::clear); set it to `None` to remove it.
    pub sky: Option<Sky>,
    /// Static mesh instances.
    pub meshes: Vec<MeshDraw>,
    /// Skinned mesh instances. Add with [`draw_skinned`](Self::draw_skinned).
    pub skinned: Vec<SkinnedDraw>,
    /// Per-tick material knob values. Materials not listed fall back to the
    /// params they were created with.
    pub material_params: HashMap<MaterialId, MaterialParams>,
    /// Joint transforms of every skinned draw, packed one after another so
    /// a tick's poses don't need an allocation each.
    poses: Vec<Transform>,
}

impl Default for RenderWorld {
    fn default() -> Self {
        Self {
            time: 0.0,
            camera: Camera::default(),
            clear_color: Vec4::new(0.0, 0.0, 0.0, 1.0),
            sky: None,
            meshes: Vec::new(),
            skinned: Vec::new(),
            material_params: HashMap::new(),
            poses: Vec::new(),
        }
    }
}

impl RenderWorld {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remove all draws and material params, keeping allocated capacity.
    /// Time, camera, clear colour and sky are left as they are.
    pub fn clear(&mut self) {
        self.meshes.clear();
        self.skinned.clear();
        self.material_params.clear();
        self.poses.clear();
    }

    pub fn draw_mesh(&mut self, draw: MeshDraw) {
        self.meshes.push(draw);
    }

    /// Draw a skinned mesh in `pose`: the *local* transform of each joint,
    /// in skeleton order (start from the skeleton's
    /// [`rest_pose`](crate::skinned_mesh::SkeletonDesc::rest_pose) and change
    /// what moves). Missing trailing joints use the rest pose; an empty
    /// pose draws the rest pose.
    pub fn draw_skinned(&mut self, mut draw: SkinnedDraw, pose: &[Transform]) {
        draw.pose = PoseRange { start: self.poses.len() as u32, len: pose.len() as u32 };
        self.poses.extend_from_slice(pose);
        self.skinned.push(draw);
    }

    /// The pose given with `draw` (which must belong to this world).
    pub(crate) fn pose_of(&self, draw: &SkinnedDraw) -> &[Transform] {
        let start = draw.pose.start as usize;
        self.poses.get(start..start + draw.pose.len as usize).unwrap_or(&[])
    }

    pub fn set_material_params(&mut self, material: MaterialId, params: MaterialParams) {
        self.material_params.insert(material, params);
    }
}
