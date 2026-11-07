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
    BlendMode, Camera, MaterialDesc, MaterialId, MeshId, RenderWorld, Renderer, SkyShaderId, Transform,
    glam::{Quat, Vec3, Vec4},
    sky::Sky,
    static_mesh::{MeshData, MeshDraw},
};

fn main() {
    common::run::<SkyExample>();
}

struct SkyExample {
    sky: SkyShaderId,
    cube: MeshId,
    material: MaterialId,
}

const CUBE_COUNT: u64 = 8;

impl Example for SkyExample {
    const TITLE: &'static str = "sky";

    fn init(setup: &mut Setup<'_>) -> Self {
        let sky = setup.sky_shader("sky.wgsl");
        let lit = setup.mesh_shader("lit.wgsl", BlendMode::Opaque);
        let renderer = &mut *setup.renderer;
        Self {
            sky,
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
        world.sky = Some(Sky::new(self.sky).with_params([
            sun.extend(0.0),
            Vec4::new(0.85, 0.55, 0.45, 1.0), // horizon
            Vec4::new(0.10, 0.20, 0.45, 1.0), // zenith
            Vec4::ZERO,
        ]));

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
