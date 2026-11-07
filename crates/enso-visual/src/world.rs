//! [`RenderWorld`]: a description of everything to draw for one simulation
//! tick.

use std::collections::HashMap;

use glam::Vec4;

use crate::{Camera, MaterialId, static_mesh::MeshDraw};

/// The four `vec4` knobs of a material (`material.params[0..4]` in WGSL).
pub type MaterialParams = [Vec4; 4];

/// Everything the renderer should draw for one simulation tick.
///
/// Pure data: only IDs, numbers and glam types, no GPU handles. It is
/// `Send`, so the simulation can build it on its own thread.
///
/// The renderer is given *two* of these, the previous and current tick, plus
/// an `alpha` to blend between them (see [`Renderer::render`]). A typical
/// loop keeps two worlds and swaps them each tick:
///
/// ```ignore
/// std::mem::swap(&mut prev, &mut curr);
/// curr.clear();            // keeps allocations
/// simulate(&mut curr);     // fill in this tick
/// ```
///
/// [`Renderer::render`]: crate::Renderer::render
#[derive(Debug, Clone)]
pub struct RenderWorld {
    /// Simulation time in seconds. Interpolated, then given to shaders as
    /// `globals.time`.
    pub time: f64,
    pub camera: Camera,
    /// Linear RGBA.
    pub clear_color: Vec4,
    /// Static mesh instances.
    pub meshes: Vec<MeshDraw>,
    /// Per-tick material knob values. Materials not listed fall back to the
    /// params they were created with.
    pub material_params: HashMap<MaterialId, MaterialParams>,
}

impl Default for RenderWorld {
    fn default() -> Self {
        Self {
            time: 0.0,
            camera: Camera::default(),
            clear_color: Vec4::new(0.0, 0.0, 0.0, 1.0),
            meshes: Vec::new(),
            material_params: HashMap::new(),
        }
    }
}

impl RenderWorld {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remove all draws and material params, keeping allocated capacity.
    /// Time, camera and clear colour are left as they are.
    pub fn clear(&mut self) {
        self.meshes.clear();
        self.material_params.clear();
    }

    pub fn draw_mesh(&mut self, draw: MeshDraw) {
        self.meshes.push(draw);
    }

    pub fn set_material_params(&mut self, material: MaterialId, params: MaterialParams) {
        self.material_params.insert(material, params);
    }
}
