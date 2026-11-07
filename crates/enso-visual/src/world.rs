//! [`RenderWorld`]: a description of everything to draw for one simulation
//! tick.

use std::collections::HashMap;

use glam::Vec4;

use crate::{
    Camera, MaterialId, Transform,
    params::{Param, ParamName, ParamType, ParamValue},
    skinned_mesh::SkinnedDraw,
    static_mesh::MeshDraw,
};

/// A slice of one of `RenderWorld`'s packed arrays (poses, instance values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Span {
    pub start: u32,
    pub len: u32,
}

impl Span {
    fn of<T>(self, items: &[T]) -> &[T] {
        let start = self.start as usize;
        items.get(start..start + self.len as usize).unwrap_or(&[])
    }
}

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
/// curr.clear();            // draws/values gone; camera, time, clear colour, sky carry over
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
    /// A material whose shader is a sky shader
    /// ([`SkyShaderId`](crate::SkyShaderId)), drawn first, behind everything.
    /// Its knobs are set with [`set_param`](Self::set_param) like any
    /// material's. Like the camera, it is kept by [`clear`](Self::clear);
    /// set it to `None` to remove it.
    pub sky: Option<MaterialId>,
    /// Static mesh instances. Add with [`draw_mesh`](Self::draw_mesh).
    pub meshes: Vec<MeshDraw>,
    /// Skinned mesh instances. Add with [`draw_skinned`](Self::draw_skinned).
    pub skinned: Vec<SkinnedDraw>,

    /// Material parameter values set this tick ([`set_param`](Self::set_param)).
    pub(crate) material_values: HashMap<(MaterialId, ParamName), ParamValue>,
    /// Per-instance parameter values of every draw, packed.
    instance_values: Vec<(ParamName, ParamValue)>,
    /// Joint transforms of every skinned draw, packed.
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
            material_values: HashMap::new(),
            instance_values: Vec::new(),
            poses: Vec::new(),
        }
    }
}

impl RenderWorld {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remove all draws and parameter values, keeping allocated capacity.
    /// Time, camera, clear colour and sky are left as they are.
    pub fn clear(&mut self) {
        self.meshes.clear();
        self.skinned.clear();
        self.material_values.clear();
        self.instance_values.clear();
        self.poses.clear();
    }

    /// Set a field of a material's `struct Material` for this tick.
    /// Values are interpolated with the previous tick's; fields not set
    /// this tick use the material's defaults.
    pub fn set_param<T: ParamType>(&mut self, material: MaterialId, param: Param<T>, value: T) {
        let (name, value) = param.value(value);
        self.material_values.insert((material, name), value);
    }

    /// Draw a static mesh. Chain [`with`](InstanceParams::with) to set
    /// fields of its shader's `struct Instance`:
    ///
    /// ```ignore
    /// world.draw_mesh(draw).with(velocity, 0.8).with(age, 0.1);
    /// ```
    pub fn draw_mesh(&mut self, mut draw: MeshDraw) -> InstanceParams<'_> {
        draw.params = Span { start: self.instance_values.len() as u32, len: 0 };
        self.meshes.push(draw);
        InstanceParams { values: &mut self.instance_values, span: &mut self.meshes.last_mut().unwrap().params }
    }

    /// Draw a skinned mesh in `pose`: the *local* transform of each joint,
    /// in skeleton order (start from the skeleton's
    /// [`rest_pose`](crate::skinned_mesh::SkeletonDesc::rest_pose) and change
    /// what moves). Missing trailing joints use the rest pose; an empty
    /// pose draws the rest pose. Chain [`with`](InstanceParams::with) to set
    /// per-instance parameters.
    pub fn draw_skinned(&mut self, mut draw: SkinnedDraw, pose: &[Transform]) -> InstanceParams<'_> {
        draw.pose = Span { start: self.poses.len() as u32, len: pose.len() as u32 };
        self.poses.extend_from_slice(pose);
        draw.params = Span { start: self.instance_values.len() as u32, len: 0 };
        self.skinned.push(draw);
        InstanceParams { values: &mut self.instance_values, span: &mut self.skinned.last_mut().unwrap().params }
    }

    pub(crate) fn pose_of(&self, draw: &SkinnedDraw) -> &[Transform] {
        draw.pose.of(&self.poses)
    }

    pub(crate) fn instance_values(&self, span: Span) -> &[(ParamName, ParamValue)] {
        span.of(&self.instance_values)
    }
}

/// Sets per-instance parameters of the draw just added. Returned by
/// [`RenderWorld::draw_mesh`] and [`RenderWorld::draw_skinned`]; ignoring it
/// is fine (the instance then gets zeros).
pub struct InstanceParams<'w> {
    values: &'w mut Vec<(ParamName, ParamValue)>,
    span: &'w mut Span,
}

impl InstanceParams<'_> {
    /// Set one field of the shader's `struct Instance` for this instance.
    /// Interpolated with the same keyed draw's value from the previous tick.
    pub fn with<T: ParamType>(self, param: Param<T>, value: T) -> Self {
        self.values.push(param.value(value));
        self.span.len += 1;
        self
    }
}
