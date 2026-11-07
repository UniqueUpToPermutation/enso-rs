//! A ring of textured cubes around a breathing sphere.
//!
//! Run with:   cargo run -p enso-visual --example static_mesh
//!
//! The simulation runs at a deliberately low 20 Hz; press **Space** to turn
//! interpolation off and see the stutter it hides. Shaders in
//! `examples/shaders/` reload when saved.

mod common;

use common::{Example, Setup, checkerboard, hue};
use enso_visual::{
    BlendMode, Camera, MaterialDesc, MaterialId, MeshId, RenderWorld, Renderer, TextureDesc, Transform,
    glam::{EulerRot, Quat, Vec3, Vec4},
    static_mesh::{MeshData, MeshDraw},
};

fn main() {
    common::run::<StaticMesh>();
}

struct StaticMesh {
    cube: MeshId,
    sphere: MeshId,
    cube_material: MaterialId,
    blob_material: MaterialId,
}

const CUBE_COUNT: u64 = 12;
const SPHERE_KEY: u64 = 1_000;

impl Example for StaticMesh {
    const TITLE: &'static str = "static mesh";
    const TICK_HZ: f64 = 20.0;

    /// Create all GPU resources once, up front.
    fn init(setup: &mut Setup<'_>) -> Self {
        let lit = setup.mesh_shader("lit.wgsl", BlendMode::Opaque);
        let wobble = setup.mesh_shader("wobble.wgsl", BlendMode::Opaque);

        let renderer = &mut *setup.renderer;
        let pixels = checkerboard(64, 8);
        let checker = renderer
            .create_texture(&TextureDesc::rgba8(64, 64, Some(&pixels)).with_label("checker"))
            .unwrap();

        Self {
            cube: renderer.create_mesh(&MeshData::cube(0.8)),
            sphere: renderer.create_mesh(&MeshData::uv_sphere(1.2, 64, 32)),
            cube_material: renderer
                .create_material(&MaterialDesc::new(lit).with_textures(&[checker]))
                .unwrap(),
            blob_material: renderer.create_material(&MaterialDesc::new(wobble)).unwrap(),
        }
    }

    /// One simulation tick: describe the world at time `t`. Pure data out.
    fn simulate(&mut self, time: f64, world: &mut RenderWorld) {
        let t = time as f32;
        world.time = time;
        world.clear_color = Vec4::new(0.01, 0.01, 0.02, 1.0);
        world.camera = Camera {
            eye: Vec3::new((t * 0.2).sin() * 7.0, 3.0, (t * 0.2).cos() * 7.0),
            ..Default::default()
        };

        for i in 0..CUBE_COUNT {
            let f = i as f32 / CUBE_COUNT as f32;
            let angle = f * std::f32::consts::TAU + t * 0.8;
            let transform = Transform::from_translation(Vec3::new(
                angle.cos() * 3.0,
                (t * 2.0 + f * 6.0).sin() * 0.4,
                angle.sin() * 3.0,
            ))
            .with_rotation(Quat::from_euler(EulerRot::YXZ, t * 1.5 + f * 3.0, t, 0.0));

            world.draw_mesh(
                MeshDraw::new(self.cube, self.cube_material, transform)
                    .with_key(i) // stable identity -> interpolated
                    .with_color(hue(f)),
            );
        }

        // Material knobs: where MIDI CCs will plug in later.
        world.set_material_params(
            self.blob_material,
            [
                hue(t * 0.05),                                           // params[0]: glow colour
                Vec4::new(0.08 + 0.06 * (t * 0.5).sin(), 0.0, 0.0, 0.0), // params[1].x: wobble
                Vec4::ZERO,
                Vec4::ZERO,
            ],
        );
        world.draw_mesh(MeshDraw::new(self.sphere, self.blob_material, Transform::IDENTITY).with_key(SPHERE_KEY));
    }
}

/// `cargo test` renders this example headless and compares it with
/// `examples/golden/static_mesh.png` (see `common::assert_golden`).
#[cfg(test)]
mod tests {
    #[test]
    fn matches_golden_image() {
        crate::common::assert_golden::<crate::StaticMesh>();
    }
}
