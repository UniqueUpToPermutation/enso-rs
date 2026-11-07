//! MSDF text in the scene: a waving title, a subtitle, and a ring of note
//! names that light up in turn.
//!
//! Run with:   cargo run -p enso-visual --example text
//!
//! The font (Noto Sans, SIL Open Font License) is turned into a multi-channel
//! signed distance field atlas at startup. `examples/shaders/text_glow.wgsl`
//! draws outlines and the per-letter wave; it reloads when saved.

mod common;

use common::{Example, Setup, hue};
use enso_visual::{
    BlendMode, Camera, FontId, MaterialDesc, MaterialId, Param, RenderWorld, Renderer, Transform,
    glam::{Quat, Vec3, Vec4},
    text::{FontDesc, TextAlign, TextDraw},
};

fn main() {
    common::run::<TextExample>();
}

const FONT: &[u8] = include_bytes!("fonts/NotoSans-Regular.ttf");
// Noto Sans has no ♯ (U+266F), so '#' it is; characters a font lacks show as '?'.
const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

struct TextExample {
    font: FontId,
    sky: MaterialId,
    /// Big text: thick outline, waving letters.
    title: MaterialId,
    /// Small text: thin outline, no wave.
    small: MaterialId,
    energy: Param<f32>,
}

impl Example for TextExample {
    const TITLE: &'static str = "text";

    fn init(setup: &mut Setup<'_>) -> Self {
        let text_shader = setup.text_shader("text_glow.wgsl", BlendMode::Alpha);
        let sky_shader = setup.sky_shader("sky.wgsl");
        let renderer = &mut *setup.renderer;

        let font = renderer.create_font(&FontDesc::new("Noto Sans", FONT)).unwrap();
        let title = renderer
            .create_material(
                &MaterialDesc::new(text_shader)
                    .with_param("outline_color", Vec3::new(0.05, 0.02, 0.10))
                    .with_param("outline_px", 3.0f32)
                    .with_param("wave", 0.12f32),
            )
            .unwrap();
        let small = renderer
            .create_material(
                &MaterialDesc::new(text_shader)
                    .with_param("outline_color", Vec3::new(0.02, 0.02, 0.06))
                    .with_param("outline_px", 1.5f32),
            )
            .unwrap();
        let sky = renderer
            .create_material(
                &MaterialDesc::new(sky_shader)
                    .with_param("sun_dir", Vec3::new(-0.4, 0.15, -1.0).normalize())
                    .with_param("horizon", Vec3::new(0.55, 0.30, 0.45))
                    .with_param("zenith", Vec3::new(0.05, 0.05, 0.18)),
            )
            .unwrap();

        Self { font, sky, title, small, energy: renderer.instance_param(small, "energy").unwrap() }
    }

    fn simulate(&mut self, time: f64, world: &mut RenderWorld) {
        let t = time as f32;
        world.time = time;
        world.sky = Some(self.sky);
        world.camera = Camera {
            eye: Vec3::new((t * 0.15).sin() * 1.5, 1.0, 6.5),
            target: Vec3::new(0.0, 0.1, 0.0),
            ..Default::default()
        };

        world.draw_text(
            TextDraw::new(self.font, self.title, Vec3::new(0.0, 1.0, 0.0))
                .with_key(0)
                .with_size(1.6)
                .with_align(TextAlign::Center)
                .with_color(Vec4::new(1.0, 0.85, 0.6, 1.0)),
            "enso",
        );
        world.draw_text(
            TextDraw::new(self.font, self.small, Vec3::new(0.0, 0.5, 0.0))
                .with_key(1)
                .with_size(0.32)
                .with_align(TextAlign::Center)
                .with_color(Vec4::new(0.85, 0.85, 1.0, 1.0)),
            "improvise · listen · play",
        );

        // A ring of note names turning slowly; a "playhead" lights them in turn.
        for (k, name) in NOTES.iter().enumerate() {
            let f = k as f32 / NOTES.len() as f32;
            let angle = f * std::f32::consts::TAU + t * 0.2;
            let position = Vec3::new(angle.sin() * 3.2, -1.1, angle.cos() * 3.2);
            let playhead = (t * 2.0 - k as f32 * 0.5).cos().max(0.0).powi(6);
            world
                .draw_text(
                    TextDraw::new(self.font, self.small, Transform::from_translation(position).with_rotation(Quat::from_rotation_y(angle)))
                        .with_key(10 + k as u64)
                        .with_size(0.5)
                        .with_align(TextAlign::Center)
                        .with_color(hue(f) * 0.6 + Vec4::splat(0.4)),
                    name,
                )
                .with(self.energy, playhead);
        }
    }
}

/// `cargo test` renders this example headless and compares it with
/// `examples/golden/text.png` (see `common::assert_golden`).
#[cfg(test)]
mod tests {
    #[test]
    fn matches_golden_image() {
        crate::common::assert_golden::<crate::TextExample>();
    }
}
