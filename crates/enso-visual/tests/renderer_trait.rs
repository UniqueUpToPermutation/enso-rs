//! The `Renderer` trait can be implemented without a GPU, so code that
//! builds scenes can be written once against the trait and tested anywhere.

use enso_visual::{
    MaterialDesc, MaterialId, MeshId, MeshShaderId, ParamInfo, RenderError, RenderWorld, Renderer, ShaderDesc,
    SkeletonId,
    SkinnedMeshId, SkinnedShaderId, SkyShaderId, TextureDesc, TextureId, Transform,
    skinned_mesh::{SkeletonDesc, SkinnedMeshData},
    FontId, TextShaderId,
    sky::SkyShaderDesc,
    static_mesh::{MeshData, MeshDraw},
    text::FontDesc,
};

/// A fake renderer that records what it was asked to do.
#[derive(Default)]
struct Recording {
    meshes: Vec<MeshId>,
    mesh_shaders: Vec<MeshShaderId>,
    materials: Vec<MaterialId>,
    /// Number of mesh draws seen by each `render` call.
    frames: Vec<usize>,
}

impl Renderer for Recording {
    // Nothing to draw into.
    type Target<'a> = ();

    fn create_texture(&mut self, _: &TextureDesc<'_>) -> Result<TextureId, RenderError> {
        Ok(TextureId::next())
    }
    fn write_texture(&mut self, _: TextureId, _: &[u8]) -> Result<(), RenderError> {
        Ok(())
    }
    fn destroy_texture(&mut self, _: TextureId) {}

    fn create_mesh(&mut self, _: &MeshData) -> MeshId {
        let id = MeshId::next();
        self.meshes.push(id);
        id
    }
    fn destroy_mesh(&mut self, id: MeshId) {
        self.meshes.retain(|m| *m != id);
    }
    fn create_mesh_shader(&mut self, _: &ShaderDesc<'_>) -> Result<MeshShaderId, RenderError> {
        let id = MeshShaderId::next();
        self.mesh_shaders.push(id);
        Ok(id)
    }
    fn update_mesh_shader(&mut self, _: MeshShaderId, _: &ShaderDesc<'_>) -> Result<(), RenderError> {
        Ok(())
    }
    fn destroy_mesh_shader(&mut self, _: MeshShaderId) {}

    fn create_skinned_mesh(&mut self, _: &SkinnedMeshData) -> SkinnedMeshId {
        SkinnedMeshId::next()
    }
    fn destroy_skinned_mesh(&mut self, _: SkinnedMeshId) {}
    fn create_skeleton(&mut self, _: &SkeletonDesc) -> Result<SkeletonId, RenderError> {
        Ok(SkeletonId::next())
    }
    fn destroy_skeleton(&mut self, _: SkeletonId) {}
    fn create_skinned_shader(&mut self, _: &ShaderDesc<'_>) -> Result<SkinnedShaderId, RenderError> {
        Ok(SkinnedShaderId::next())
    }
    fn update_skinned_shader(&mut self, _: SkinnedShaderId, _: &ShaderDesc<'_>) -> Result<(), RenderError> {
        Ok(())
    }
    fn destroy_skinned_shader(&mut self, _: SkinnedShaderId) {}

    fn create_material(&mut self, _: &MaterialDesc<'_>) -> Result<MaterialId, RenderError> {
        let id = MaterialId::next();
        self.materials.push(id);
        Ok(id)
    }
    fn destroy_material(&mut self, _: MaterialId) {}

    // No shaders are compiled, so there are no parameter structs.
    fn material_params(&self, _: MaterialId) -> Option<&[ParamInfo]> {
        None
    }
    fn instance_params(&self, _: MaterialId) -> Option<&[ParamInfo]> {
        None
    }

    fn create_sky_shader(&mut self, _: &SkyShaderDesc<'_>) -> Result<SkyShaderId, RenderError> {
        Ok(SkyShaderId::next())
    }
    fn update_sky_shader(&mut self, _: SkyShaderId, _: &SkyShaderDesc<'_>) -> Result<(), RenderError> {
        Ok(())
    }
    fn destroy_sky_shader(&mut self, _: SkyShaderId) {}

    fn create_font(&mut self, _: &FontDesc<'_>) -> Result<FontId, RenderError> {
        Ok(FontId::next())
    }
    fn destroy_font(&mut self, _: FontId) {}
    fn create_text_shader(&mut self, _: &ShaderDesc<'_>) -> Result<TextShaderId, RenderError> {
        Ok(TextShaderId::next())
    }
    fn update_text_shader(&mut self, _: TextShaderId, _: &ShaderDesc<'_>) -> Result<(), RenderError> {
        Ok(())
    }
    fn destroy_text_shader(&mut self, _: TextShaderId) {}

    fn render(&mut self, _: (), _prev: &RenderWorld, curr: &RenderWorld, _alpha: f32) {
        self.frames.push(curr.meshes.len());
    }
}

/// Scene setup written once, against the trait. The same function works
/// with `WgpuRenderer`.
fn build_scene(renderer: &mut impl Renderer) -> Result<(MeshId, MaterialId), RenderError> {
    let shader = renderer.create_mesh_shader(&ShaderDesc::new("lit", "/* WGSL */"))?;
    let mesh = renderer.create_mesh(&MeshData::cube(1.0));
    let material = renderer.create_material(&MaterialDesc::new(shader))?;
    Ok((mesh, material))
}

#[test]
fn scene_code_runs_against_a_fake_renderer() {
    let mut renderer = Recording::default();
    let (mesh, material) = build_scene(&mut renderer).unwrap();

    let mut world = RenderWorld::new();
    for i in 0..3 {
        world.draw_mesh(MeshDraw::new(mesh, material, Transform::IDENTITY).with_key(i));
    }
    renderer.render((), &world, &world, 1.0);

    assert_eq!(renderer.meshes, vec![mesh]);
    assert_eq!(renderer.materials, vec![material]);
    assert_eq!(renderer.mesh_shaders.len(), 1);
    assert_eq!(renderer.frames, vec![3]);
}
