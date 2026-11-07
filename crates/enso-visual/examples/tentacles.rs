//! Swaying tentacles: skinned tubes on chains of joints, posed every tick.
//!
//! Run with:   cargo run -p enso-visual --example tentacles
//!
//! Each tentacle is one skinned mesh instance. Every tick the simulation
//! writes a local rotation for each joint (a wave travelling up the chain);
//! the renderer interpolates the poses joint by joint between ticks.
//! `examples/shaders/skinned_lit.wgsl` reloads when saved.

mod common;

use common::{Example, Setup, hue};
use enso_visual::{
    BlendMode, Camera, MaterialDesc, MaterialId, RenderWorld, Renderer, SkeletonId, SkinnedMeshId, Transform,
    glam::{Quat, Vec3, Vec4},
    skinned_mesh::{SkeletonDesc, SkinnedDraw, SkinnedMeshData},
};

fn main() {
    common::run::<Tentacles>();
}

const COUNT: u64 = 7;
const JOINTS: usize = 10;
const LENGTH: f32 = 3.0;

struct Tentacles {
    mesh: SkinnedMeshId,
    skeleton: SkeletonId,
    /// Kept for its rest pose, which every tick's pose starts from.
    skeleton_desc: SkeletonDesc,
    material: MaterialId,
    /// A material of sky.wgsl.
    sky: MaterialId,
    /// Reused each tick to build a pose without allocating.
    pose: Vec<Transform>,
}

impl Example for Tentacles {
    const TITLE: &'static str = "tentacles";

    fn init(setup: &mut Setup<'_>) -> Self {
        let shader = setup.skinned_shader("skinned_lit.wgsl", BlendMode::Opaque);
        let sky_shader = setup.sky_shader("sky.wgsl");

        let renderer = &mut *setup.renderer;
        // A fixed sky: every knob is a default, nothing is set per tick.
        let sky = renderer
            .create_material(
                &MaterialDesc::new(sky_shader)
                    .with_param("sun_dir", Vec3::new(0.3, 0.35, -1.0).normalize())
                    .with_param("horizon", Vec3::new(0.30, 0.45, 0.55))
                    .with_param("zenith", Vec3::new(0.04, 0.08, 0.20)),
            )
            .unwrap();
        let skeleton_desc = SkeletonDesc::chain(JOINTS, LENGTH);
        Self {
            mesh: renderer.create_skinned_mesh(&SkinnedMeshData::tube(0.18, LENGTH, 16, 40, JOINTS)),
            skeleton: renderer.create_skeleton(&skeleton_desc).unwrap(),
            skeleton_desc,
            material: renderer.create_material(&MaterialDesc::new(shader)).unwrap(),
            sky,
            pose: Vec::with_capacity(JOINTS),
        }
    }

    fn simulate(&mut self, time: f64, world: &mut RenderWorld) {
        let t = time as f32;
        world.time = time;
        world.camera = Camera {
            eye: Vec3::new((t * 0.1).sin() * 7.0, 3.5, (t * 0.1).cos() * 7.0),
            target: Vec3::new(0.0, 1.4, 0.0),
            ..Default::default()
        };
        world.sky = Some(self.sky);

        for k in 0..COUNT {
            let f = k as f32 / COUNT as f32;
            let phase = k as f32 * 1.3;

            // Start from the rest pose and bend every joint a little. The
            // phase shift per joint makes the wave travel up the tentacle;
            // scaling each joint slightly tapers it towards the tip.
            self.pose.clear();
            for (j, rest) in self.skeleton_desc.rest_pose.iter().enumerate() {
                let j = j as f32;
                // Rotations accumulate down the chain, so small angles per
                // joint already make a big curve at the tip.
                let sway = 0.15 * (t * 1.6 - j * 0.55 + phase).sin();
                let nod = 0.09 * (t * 1.1 - j * 0.4 + phase * 0.7).sin();
                let scale = if j == 0.0 { 1.0 } else { 0.94 };
                self.pose.push(
                    rest.with_rotation(Quat::from_rotation_z(sway) * Quat::from_rotation_x(nod))
                        .with_uniform_scale(scale),
                );
            }

            let angle = f * std::f32::consts::TAU;
            let placement = Transform::from_translation(Vec3::new(angle.cos() * 2.2, 0.0, angle.sin() * 2.2))
                .with_rotation(Quat::from_rotation_y(-angle));
            world.draw_skinned(
                SkinnedDraw::new(self.mesh, self.skeleton, self.material, placement)
                    .with_key(k)
                    .with_color(hue(f) * 0.85 + Vec4::splat(0.15)),
                &self.pose,
            );
        }
    }
}

/// `cargo test` renders this example headless and compares it with
/// `examples/golden/tentacles.png` (see `common::assert_golden`).
#[cfg(test)]
mod tests {
    #[test]
    fn matches_golden_image() {
        crate::common::assert_golden::<crate::Tentacles>();
    }
}
