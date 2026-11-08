//! Named shader parameters: a row of keys struck by a MIDI keyboard, or by
//! a looping melody while nobody plays.
//!
//! Run with:   cargo run -p enso-visual --example notes
//!
//! Play any connected MIDI input: each note lights its pitch class
//! (C, C#, ... B) and sends up a spark. Devices can be plugged in while it
//! runs. The golden-image test always uses the melody.
//!
//! * `note_key.wgsl` declares `struct Material { base_color, glow_color, decay }`
//!   (shared by all keys) and `struct Instance { velocity, age }` (per key).
//! * `note_spark.wgsl` declares a *different* `struct Instance { tint, energy }`
//!   for the sparks rising from struck keys.
//!
//! Every value is set by name each tick and interpolated between ticks.
//! Try adding a field to a shader's struct while the example runs.

mod common;

use std::{
    collections::VecDeque,
    sync::mpsc,
    time::{Duration, Instant},
};

use common::{Example, Setup, hue};
use enso_core::{Clock, MidiEvent, MidiMessage};
use enso_io::midi::MidiInputs;
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
/// After this many seconds a key counts as never struck.
const KEY_MEMORY: f64 = 4.0;
/// Key index for each step of the looping melody.
const MELODY: [usize; 16] = [0, 4, 7, 11, 9, 7, 4, 2, 0, 5, 9, 7, 3, 7, 10, 7];

/// A key being struck, by the melody or a player.
#[derive(Clone, Copy)]
struct Strike {
    /// Unique and increasing: used as the spark's draw key.
    index: u64,
    /// 0..KEYS: the pitch class (0 = C).
    key: usize,
    /// Simulation time of the strike, in seconds.
    time: f64,
    velocity: f32,
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

    /// Live MIDI, if not running as a test.
    midi: Option<LiveMidi>,
    /// The next melody step to play; the melody stops once someone plays.
    melody_step: u64,
    strikes: u64,
    /// The last strike of each key.
    last_strike: [Option<Strike>; KEYS],
    /// Strikes younger than `SPARK_LIFE`, oldest first.
    sparks: VecDeque<Strike>,
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

            // The golden test must not depend on what's plugged in.
            midi: if cfg!(test) { None } else { Some(LiveMidi::connect()) },
            melody_step: 0,
            strikes: 0,
            last_strike: [None; KEYS],
            sparks: VecDeque::new(),
        }
    }

    fn simulate(&mut self, time: f64, world: &mut RenderWorld) {
        self.take_input(time);

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

        for (key, strike) in self.last_strike.iter().enumerate() {
            let (velocity, age) = match strike {
                Some(s) if time - s.time <= KEY_MEMORY => (s.velocity, (time - s.time) as f32),
                _ => (0.0, 10.0),
            };
            let placement = Transform::from_translation(Vec3::new(key_x(key), 0.0, 0.0))
                .with_scale(Vec3::new(0.7, 0.3, 2.4));
            world
                .draw_mesh(MeshDraw::new(self.key_mesh, self.key_material, placement).with_key(key as u64))
                .with(self.velocity, velocity)
                .with(self.age, age);
        }

        // One spark per recent strike, rising and fading.
        for strike in &self.sparks {
            let age = (time - strike.time) as f32;
            let life = 1.0 - age / SPARK_LIFE as f32;
            let placement =
                Transform::from_translation(Vec3::new(key_x(strike.key), 0.5 + age * 2.2, 0.3 * (age * 5.0).sin()))
                    .with_uniform_scale(0.15 + 0.3 * strike.velocity * life);
            world
                .draw_mesh(
                    // Keys from a separate range so they never collide with the keys' keys.
                    MeshDraw::new(self.spark_mesh, self.spark_material, placement).with_key(1_000 + strike.index),
                )
                .with(self.tint, hue(strike.key as f32 / KEYS as f32).truncate())
                .with(self.energy, strike.velocity * life);
        }
    }
}

impl Notes {
    /// Apply every strike that happened up to `time`, then forget old sparks.
    fn take_input(&mut self, time: f64) {
        let mut played = Vec::new();
        if let Some(midi) = &mut self.midi {
            midi.notes_until(time, &mut played);
        }
        if !played.is_empty() {
            self.melody_step = u64::MAX; // someone's playing: the demo stops for good
        }
        for (key, at, velocity) in played {
            self.strike(key, at, velocity);
        }

        // The melody, while nobody has played.
        while self.melody_step != u64::MAX && self.melody_step as f64 * STEP <= time {
            let step = self.melody_step;
            let velocity = 0.45 + 0.55 * ((step * 7 % 5) as f32 / 4.0);
            self.strike(MELODY[step as usize % MELODY.len()], step as f64 * STEP, velocity);
            self.melody_step += 1;
        }

        while self.sparks.front().is_some_and(|s| s.time < time - SPARK_LIFE) {
            self.sparks.pop_front();
        }
    }

    fn strike(&mut self, key: usize, time: f64, velocity: f32) {
        let strike = Strike { index: self.strikes, key, time, velocity };
        self.strikes += 1;
        self.last_strike[key] = Some(strike);
        self.sparks.push_back(strike);
    }
}

/// X position of a key's centre.
fn key_x(key: usize) -> f32 {
    (key as f32 - (KEYS as f32 - 1.0) / 2.0) * 0.85
}

/// Every MIDI input, connected via enso-io.
struct LiveMidi {
    clock: Clock,
    inputs: MidiInputs<mpsc::Sender<MidiEvent>>,
    events: mpsc::Receiver<MidiEvent>,
    /// Arrived, but stamped later than the current simulation time.
    early: Vec<MidiEvent>,
    /// Clock seconds minus simulation seconds, fixed at the first tick.
    sim_offset: Option<f64>,
    last_refresh: Instant,
}

impl LiveMidi {
    fn connect() -> Self {
        let clock = Clock::new();
        let (sender, events) = mpsc::channel();
        let mut inputs = MidiInputs::new(clock, sender);
        inputs.refresh();
        let names: Vec<_> = inputs.connected().iter().map(|i| i.name()).collect();
        log::info!("MIDI inputs: {names:?}. Play one; the demo melody stops when you do.");
        Self { clock, inputs, events, early: Vec::new(), sim_offset: None, last_refresh: Instant::now() }
    }

    /// Note-ons stamped up to simulation `time`, as (pitch class, sim time,
    /// velocity 0..1), oldest first.
    fn notes_until(&mut self, time: f64, out: &mut Vec<(usize, f64, f32)>) {
        // Look for plugged-in devices once a second. (A real app would do
        // this off the main thread: listing ports takes a moment.)
        if self.last_refresh.elapsed() >= Duration::from_secs(1) {
            self.inputs.refresh();
            self.last_refresh = Instant::now();
        }

        // Events are stamped on the shared clock; the simulation has its own
        // time. Line them up once, at the first tick.
        let offset = *self.sim_offset.get_or_insert_with(|| self.clock.now().as_secs_f64() - time);

        self.early.extend(self.events.try_iter());
        // Several devices can interleave; keep strikes in time order.
        self.early.sort_by_key(|event| event.time);
        // The fixed-timestep loop can run slightly ahead of the wall clock,
        // so an event may be "from the future": keep it for a later tick.
        let due = self.early.partition_point(|event| event.time.as_secs_f64() - offset <= time);
        for event in self.early.drain(..due) {
            if let MidiMessage::NoteOn { key, velocity, .. } = event.message {
                let at = event.time.as_secs_f64() - offset;
                out.push((usize::from(key) % KEYS, at, f32::from(velocity) / 127.0));
            }
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
