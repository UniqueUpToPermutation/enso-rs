//! Named shader parameters: a row of keys struck by a looping melody.
//!
//! Run with:   cargo run -p enso-visual --example notes
//!
//! * `note_key.wgsl` declares `struct Material { base_color, glow_color, decay }`
//!   (shared by all keys) and `struct Instance { velocity, age }` (per key).
//! * `note_spark.wgsl` declares a *different* `struct Instance { tint, energy }`
//!   for the sparks rising from struck keys.
//!
//! Every value is set by name each tick and interpolated between ticks.
//! Try adding a field to a shader's struct while the example runs.

mod common;

use common::{Example, Setup, hue};
use enso_visual::{
    BlendMode, Camera, MaterialDesc, MaterialId, MeshId, Param, RenderWorld, Renderer, Transform,
    glam::{Vec3, Vec4},
    static_mesh::{MeshData, MeshDraw},
};

fn main() {
    common::run::<Notes>();
}

const KEYS: usize = 12;
/// Seconds between melody steps.
const STEP: f64 = 0.25;
/// How long sparks live, in seconds.
const SPARK_LIFE: f64 = 1.5;
/// Key index for each step of the looping melody.
const MELODY: [usize; 16] = [0, 4, 7, 11, 9, 7, 4, 2, 0, 5, 9, 7, 3, 7, 10, 7];

/// A note-on event of the melody.
struct Note {
    /// Its position in the (endless) sequence, unique: used as a draw key.
    index: u64,
    key: usize,
    time: f64,
    velocity: f32,
}

/// The melody's notes struck in `[from, to]`.
fn notes_between(from: f64, to: f64) -> impl Iterator<Item = Note> {
    let first = (from.max(0.0) / STEP).ceil() as u64;
    let last = (to / STEP).floor() as u64;
    (first..=last).map(|index| Note {
        index,
        key: MELODY[index as usize % MELODY.len()],
        time: index as f64 * STEP,
        velocity: 0.45 + 0.55 * ((index * 7 % 5) as f32 / 4.0),
    })
}

struct Notes {
    key_mesh: MeshId,
    spark_mesh: MeshId,
    key_material: MaterialId,
    spark_material: MaterialId,

    // Material fields of note_key.wgsl.
    glow_color: Param<Vec3>,
    decay: Param<f32>,
    // Instance fields of note_key.wgsl.
    velocity: Param<f32>,
    age: Param<f32>,
    // Instance fields of note_spark.wgsl.
    tint: Param<Vec3>,
    energy: Param<f32>,
}

impl Example for Notes {
    const TITLE: &'static str = "notes";

    fn init(setup: &mut Setup<'_>) -> Self {
        let key_shader = setup.mesh_shader("note_key.wgsl", BlendMode::Opaque);
        let spark_shader = setup.mesh_shader("note_spark.wgsl", BlendMode::Additive);

        let renderer = &mut *setup.renderer;
        // `with_param` sets defaults, used whenever a tick doesn't set the field.
        let key_material = renderer
            .create_material(&MaterialDesc::new(key_shader).with_param("base_color", Vec3::new(0.12, 0.12, 0.16)))
            .unwrap();
        let spark_material = renderer.create_material(&MaterialDesc::new(spark_shader)).unwrap();

        // The renderer knows each shader's fields:
        for (what, fields) in [
            ("key material", renderer.material_params(key_material)),
            ("key instance", renderer.instance_params(key_material)),
            ("spark instance", renderer.instance_params(spark_material)),
        ] {
            log::info!("{what} params: {:?}", fields.unwrap_or_default());
        }

        Self {
            key_mesh: renderer.create_mesh(&MeshData::cube(1.0)),
            spark_mesh: renderer.create_mesh(&MeshData::uv_sphere(1.0, 16, 8)),
            key_material,
            spark_material,
            // Checked handles: wrong names or types fail here, not silently later.
            glow_color: renderer.material_param(key_material, "glow_color").unwrap(),
            decay: renderer.material_param(key_material, "decay").unwrap(),
            velocity: renderer.instance_param(key_material, "velocity").unwrap(),
            age: renderer.instance_param(key_material, "age").unwrap(),
            tint: renderer.instance_param(spark_material, "tint").unwrap(),
            energy: renderer.instance_param(spark_material, "energy").unwrap(),
        }
    }

    fn simulate(&mut self, time: f64, world: &mut RenderWorld) {
        let t = time as f32;
        world.time = time;
        world.clear_color = Vec4::new(0.015, 0.015, 0.03, 1.0);
        world.camera = Camera {
            eye: Vec3::new((t * 0.15).sin() * 2.0, 4.5, 8.0),
            target: Vec3::new(0.0, 0.6, 0.0),
            ..Default::default()
        };

        // Shared knobs: the glow colour drifts, the decay breathes.
        let glow = hue(0.55 + t * 0.03).truncate();
        world.set_param(self.key_material, self.glow_color, glow);
        world.set_param(self.key_material, self.decay, 2.5 + 1.5 * (t * 0.4).sin());

        // When was each key last struck, and how hard?
        let mut last: [Option<(f64, f32)>; KEYS] = [None; KEYS];
        for note in notes_between(time - 4.0, time) {
            last[note.key] = Some((note.time, note.velocity));
        }

        for (key, struck) in last.iter().enumerate() {
            let (velocity, age) = struck.map_or((0.0, 10.0), |(at, v)| (v, (time - at) as f32));
            let x = (key as f32 - (KEYS as f32 - 1.0) / 2.0) * 0.85;
            let placement = Transform::from_translation(Vec3::new(x, 0.0, 0.0)).with_scale(Vec3::new(0.7, 0.3, 2.4));
            world
                .draw_mesh(MeshDraw::new(self.key_mesh, self.key_material, placement).with_key(key as u64))
                .with(self.velocity, velocity)
                .with(self.age, age);
        }

        // One spark per recent note, rising and fading.
        for note in notes_between(time - SPARK_LIFE, time) {
            let age = (time - note.time) as f32;
            let life = 1.0 - age / SPARK_LIFE as f32;
            let x = (note.key as f32 - (KEYS as f32 - 1.0) / 2.0) * 0.85;
            let placement = Transform::from_translation(Vec3::new(x, 0.5 + age * 2.2, 0.3 * (age * 5.0).sin()))
                .with_uniform_scale(0.15 + 0.3 * note.velocity * life);
            world
                .draw_mesh(
                    // Keys from a separate range so they never collide with the keys' keys.
                    MeshDraw::new(self.spark_mesh, self.spark_material, placement).with_key(1_000 + note.index),
                )
                .with(self.tint, hue(note.key as f32 / KEYS as f32).truncate())
                .with(self.energy, note.velocity * life);
        }
    }
}

/// `cargo test` renders this example headless and compares it with
/// `examples/golden/notes.png` (see `common::assert_golden`).
#[cfg(test)]
mod tests {
    #[test]
    fn matches_golden_image() {
        crate::common::assert_golden::<crate::Notes>();
    }
}
