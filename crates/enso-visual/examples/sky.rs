//! A sky with a slowly moving sun, behind a ring of cubes.
//!
//! Run with:   cargo run -p enso-visual --example sky
//!
//! The sky is a fullscreen triangle drawn before everything else; the cubes
//! show that meshes land in front of it. `examples/shaders/sky.wgsl`
//! reloads when saved.

mod common;

use common::{Example, Setup, hue};
use enso_visual::{
    BlendMode, Camera, MaterialDesc, MaterialId, MeshId, Param, RenderWorld, Renderer, Transform,
    glam::{Quat, Vec3, Vec4},
    static_mesh::{MeshData, MeshDraw},
};

fn main() {
    common::run::<SkyExample>();
}

struct SkyExample {
    /// A material of sky.wgsl: the sky is a material like any other.
    sky: MaterialId,
    sun_dir: Param<Vec3>,
    cube: MeshId,
    material: MaterialId,
}

const CUBE_COUNT: u64 = 8;

impl Example for SkyExample {
    const TITLE: &'static str = "sky";

    fn init(setup: &mut Setup<'_>) -> Self {
        let sky_shader = setup.sky_shader("sky.wgsl");
        let lit = setup.mesh_shader("lit.wgsl", BlendMode::Opaque);
        let renderer = &mut *setup.renderer;

        // Colours are defaults: they stay unless a tick sets them.
        let sky = renderer
            .create_material(
                &MaterialDesc::new(sky_shader)
                    .with_param("horizon", Vec3::new(0.85, 0.55, 0.45))
                    .with_param("zenith", Vec3::new(0.10, 0.20, 0.45)),
            )
            .unwrap();

        Self {
            sky,
            sun_dir: renderer.material_param(sky, "sun_dir").unwrap(),
            cube: renderer.create_mesh(&MeshData::cube(0.8)),
            // No textures: lit.wgsl samples texture0, which defaults to white.
            material: renderer.create_material(&MaterialDesc::new(lit)).unwrap(),
        }
    }

    fn simulate(&mut self, time: f64, world: &mut RenderWorld) {
        let t = time as f32;
        world.time = time;
        world.camera = Camera {
            eye: Vec3::new((t * 0.2).sin() * 6.0, 1.5, (t * 0.2).cos() * 6.0),
            target: Vec3::new(0.0, 1.0, 0.0),
            ..Default::default()
        };

        // The sun circles slowly, low over the horizon.
        let azimuth = -1.4 + t * 0.15;
        let sun = Vec3::new(azimuth.cos(), 0.25, azimuth.sin()).normalize();
        world.sky = Some(self.sky);
        world.set_param(self.sky, self.sun_dir, sun);

        for i in 0..CUBE_COUNT {
            let f = i as f32 / CUBE_COUNT as f32;
            let angle = f * std::f32::consts::TAU;
            let transform = Transform::from_translation(Vec3::new(angle.cos() * 3.0, 0.4, angle.sin() * 3.0))
                .with_rotation(Quat::from_rotation_y(t * 0.5 + f * 2.0));
            world.draw_mesh(
                MeshDraw::new(self.cube, self.material, transform)
                    .with_key(i)
                    .with_color(hue(f) * 0.8 + Vec4::splat(0.2)),
            );
        }
    }
}

/// `cargo test` renders this example headless and compares it with
/// `examples/golden/sky.png` (see `common::assert_golden`).
#[cfg(test)]
mod tests {
    #[test]
    fn matches_golden_image() {
        crate::common::assert_golden::<crate::SkyExample>();
    }
}
