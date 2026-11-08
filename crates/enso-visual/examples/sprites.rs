//! Sprites in all three orientation modes, seen from above:
//!
//! * **Upright** trees: turn towards the camera but stay standing.
//! * **Camera** orbs: always face the screen (round from every angle).
//! * A **Fixed** decal lying flat on the ground, oriented by its transform.
//!
//! Run with:   cargo run -p enso-visual --example sprites
//!
//! All textures are generated in code. `examples/shaders/sprite.wgsl` is
//! used three times with different blend modes; it reloads when saved.

mod common;

use common::{Example, Setup, hue};
use enso_visual::{
    BlendMode, Camera, MaterialDesc, MaterialId, RenderWorld, Renderer, TextureDesc, Transform,
    glam::{Quat, Vec2, Vec3, Vec4},
    sprite::{Billboard, SpriteDraw},
};

fn main() {
    common::run::<Sprites>();
}

const TREES: u64 = 10;
const ORBS: u64 = 14;

struct Sprites {
    /// Cut-out (alpha-tested, opaque) material with the tree texture.
    tree: MaterialId,
    /// Additive glow with the soft-dot texture.
    orb: MaterialId,
    /// Alpha-blended ring lying on the ground.
    decal: MaterialId,
}

impl Example for Sprites {
    const TITLE: &'static str = "sprites";

    fn init(setup: &mut Setup<'_>) -> Self {
        // One shader file, three pipelines: blending is fixed per pipeline.
        let cutout = setup.sprite_shader("sprite.wgsl", BlendMode::Opaque);
        let glow = setup.sprite_shader("sprite.wgsl", BlendMode::Additive);
        let blended = setup.sprite_shader("sprite.wgsl", BlendMode::Alpha);
        let renderer = &mut *setup.renderer;

        let mut texture = |label: &str, (w, h, pixels): (u32, u32, Vec<u8>)| {
            renderer.create_texture(&TextureDesc::rgba8(w, h, Some(&pixels)).with_label(label)).unwrap()
        };
        let tree_texture = texture("tree", tree_pixels(64, 128));
        let dot_texture = texture("soft dot", soft_dot_pixels(64));
        let ring_texture = texture("ring", ring_pixels(256));

        Self {
            tree: renderer
                .create_material(&MaterialDesc::new(cutout).with_textures(&[tree_texture]).with_param("cutoff", 0.5f32))
                .unwrap(),
            orb: renderer
                .create_material(&MaterialDesc::new(glow).with_textures(&[dot_texture]).with_param("emissive", 0.5f32))
                .unwrap(),
            decal: renderer.create_material(&MaterialDesc::new(blended).with_textures(&[ring_texture])).unwrap(),
        }
    }

    fn simulate(&mut self, time: f64, world: &mut RenderWorld) {
        let t = time as f32;
        world.time = time;
        world.clear_color = Vec4::new(0.02, 0.03, 0.06, 1.0);
        // Orbit high above: the upright trees should still stand straight.
        world.camera = Camera {
            eye: Vec3::new((t * 0.25).sin() * 6.5, 4.5, (t * 0.25).cos() * 6.5),
            target: Vec3::new(0.0, 0.6, 0.0),
            ..Default::default()
        };

        // Fixed: a ring lying flat (the quad's XY plane rotated onto XZ).
        world.draw_sprite(
            SpriteDraw::new(self.decal, Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)))
                .with_key(0)
                .with_size(Vec2::splat(6.5))
                .with_spin(t * 0.3)
                .with_color(Vec4::new(0.4, 0.8, 1.0, 0.8)),
        );

        // Upright: trees standing on the ring, anchored at their base.
        for i in 0..TREES {
            let angle = i as f32 / TREES as f32 * std::f32::consts::TAU;
            let height = 1.6 + 0.5 * ((i * 7 % 5) as f32 / 4.0);
            world.draw_sprite(
                SpriteDraw::new(self.tree, Vec3::new(angle.cos() * 2.6, 0.0, angle.sin() * 2.6))
                    .with_key(100 + i)
                    .with_billboard(Billboard::Upright)
                    .with_anchor(Vec2::new(0.5, 0.0))
                    .with_size(Vec2::new(height * 0.5, height)),
            );
        }

        // Camera: glowing orbs drifting above, always facing the screen.
        for i in 0..ORBS {
            let f = i as f32 / ORBS as f32;
            let angle = f * std::f32::consts::TAU + t * 0.4;
            let radius = 1.0 + 1.2 * ((i * 3 % 4) as f32 / 3.0);
            let position = Vec3::new(angle.cos() * radius, 1.2 + (t * 1.5 + f * 9.0).sin() * 0.5, angle.sin() * radius);
            world.draw_sprite(
                SpriteDraw::new(self.orb, position)
                    .with_key(200 + i)
                    .with_billboard(Billboard::Camera)
                    .with_size(Vec2::splat(0.45 + 0.15 * (t * 3.0 + f * 5.0).sin()))
                    .with_color(hue(f)),
            );
        }
    }
}

// ---- procedural textures (RGBA8, sRGB) ------------------------------------------

fn pixels(w: u32, h: u32, f: impl Fn(f32, f32) -> [u8; 4]) -> (u32, u32, Vec<u8>) {
    let data = (0..w * h)
        .flat_map(|i| {
            // Normalised coordinates of the pixel centre: x 0..1 left to right,
            // y 0..1 bottom to top.
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            f(x / w as f32, 1.0 - y / h as f32)
        })
        .collect();
    (w, h, data)
}

/// White with a soft round alpha falloff: a glow.
fn soft_dot_pixels(size: u32) -> (u32, u32, Vec<u8>) {
    pixels(size, size, |x, y| {
        let r = ((x - 0.5).hypot(y - 0.5) * 2.0).min(1.0);
        [255, 255, 255, ((1.0 - r).powi(2) * 255.0) as u8]
    })
}

/// A stylised tree: two stacked green triangles on a brown trunk.
fn tree_pixels(w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    pixels(w, h, |x, y| {
        let dx = (x - 0.5).abs();
        let trunk = dx < 0.07 && y < 0.25;
        // Triangle with its base at `base` (width 2 * half) and apex at `top`.
        let tri = |base: f32, top: f32, half: f32| y > base && y < top && dx < half * (top - y) / (top - base);
        if tri(0.15, 0.7, 0.46) || tri(0.45, 0.97, 0.36) {
            let shade = (110.0 + 60.0 * y) as u8;
            [30, shade, 55, 255]
        } else if trunk {
            [90, 60, 35, 255]
        } else {
            [0, 0, 0, 0]
        }
    })
}

/// A thin white ring with soft edges.
fn ring_pixels(size: u32) -> (u32, u32, Vec<u8>) {
    pixels(size, size, |x, y| {
        let r = (x - 0.5).hypot(y - 0.5) * 2.0;
        let band = (1.0 - ((r - 0.85).abs() / 0.06)).clamp(0.0, 1.0);
        let inner = (1.0 - ((r - 0.6).abs() / 0.02)).clamp(0.0, 1.0) * 0.5;
        [255, 255, 255, ((band + inner).min(1.0) * 255.0) as u8]
    })
}

/// `cargo test` renders this example headless and compares it with
/// `examples/golden/sprites.png` (see `common::assert_golden`).
#[cfg(test)]
mod tests {
    #[test]
    fn matches_golden_image() {
        crate::common::assert_golden::<crate::Sprites>();
    }
}
