//! Shared harness for the enso-visual examples.
//!
//! Cargo builds every `examples/*.rs` and `examples/*/main.rs` as an example.
//! This directory has neither, so it is not an example itself; each example
//! includes it with `mod common;`.
//!
//! An example implements [`Example`] and gets, for free:
//! * a window with a fixed-timestep, interpolated render loop ([`run`]);
//!   keys: **Space** toggles interpolation, **Esc** quits;
//! * shader hot reload via [`Setup::mesh_shader`];
//! * a headless golden-image test ([`assert_golden`], test builds only).

// Each example uses a different subset of this module; don't warn about the rest.
#![allow(dead_code)]

#[cfg(test)]
mod golden;
mod window;

use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use enso_visual::{
    BlendMode, MeshShaderId, RenderError, RenderWorld, Renderer, ShaderDesc, SkinnedShaderId, SkyShaderId, TextShaderId,
    WgpuRenderer,
    glam::Vec4,
    sky::SkyShaderDesc,
};

#[cfg(test)]
pub use golden::assert_golden;
pub use window::run;

// ---- what an example implements ---------------------------------------------

pub trait Example: 'static {
    /// Shown in the window title.
    const TITLE: &'static str;

    /// Simulation ticks per second. Rendering interpolates between ticks.
    const TICK_HZ: f64 = 60.0;

    /// Simulation time (seconds) captured by the golden-image test. It
    /// renders halfway between the tick at this time and the one before, so
    /// interpolation is covered too.
    const GOLDEN_TIME: f64 = 1.0;

    /// Create GPU resources. Called once, after the GPU exists.
    fn init(setup: &mut Setup<'_>) -> Self;

    /// Describe the world at `time` (seconds). Called `TICK_HZ` times per
    /// second. `world` arrives with draws and material params cleared;
    /// camera, time, clear colour and sky still hold the previous tick's values.
    fn simulate(&mut self, time: f64, world: &mut RenderWorld);
}

/// Handed to [`Example::init`].
pub struct Setup<'a> {
    pub renderer: &'a mut WgpuRenderer,
    shaders: &'a mut ShaderWatcher,
}

impl Setup<'_> {
    /// Load `examples/shaders/<file>` as a static mesh shader. In the window
    /// it is recompiled whenever the file is saved; a broken edit logs the
    /// error and keeps the previous version running.
    pub fn mesh_shader(&mut self, file: &str, blend: BlendMode) -> MeshShaderId {
        let source = ShaderWatcher::read(file);
        let id = self
            .renderer
            .create_mesh_shader(&ShaderDesc::new(file, &source).blend(blend))
            .unwrap_or_else(|err| panic!("initial shader must compile: {err}"));
        self.shaders.watch(file, ShaderKind::Mesh { id, blend });
        id
    }

    /// Load `examples/shaders/<file>` as a skinned mesh shader, with the
    /// same hot reload behaviour as [`mesh_shader`](Self::mesh_shader).
    pub fn skinned_shader(&mut self, file: &str, blend: BlendMode) -> SkinnedShaderId {
        let source = ShaderWatcher::read(file);
        let id = self
            .renderer
            .create_skinned_shader(&ShaderDesc::new(file, &source).blend(blend))
            .unwrap_or_else(|err| panic!("initial shader must compile: {err}"));
        self.shaders.watch(file, ShaderKind::Skinned { id, blend });
        id
    }

    /// Load `examples/shaders/<file>` as a text shader, with the same hot
    /// reload behaviour as [`mesh_shader`](Self::mesh_shader).
    pub fn text_shader(&mut self, file: &str, blend: BlendMode) -> TextShaderId {
        let source = ShaderWatcher::read(file);
        let id = self
            .renderer
            .create_text_shader(&ShaderDesc::new(file, &source).blend(blend))
            .unwrap_or_else(|err| panic!("initial shader must compile: {err}"));
        self.shaders.watch(file, ShaderKind::Text { id, blend });
        id
    }

    /// Load `examples/shaders/<file>` as a sky shader, with the same hot
    /// reload behaviour as [`mesh_shader`](Self::mesh_shader).
    pub fn sky_shader(&mut self, file: &str) -> SkyShaderId {
        let source = ShaderWatcher::read(file);
        let id = self
            .renderer
            .create_sky_shader(&SkyShaderDesc::new(file, &source))
            .unwrap_or_else(|err| panic!("initial shader must compile: {err}"));
        self.shaders.watch(file, ShaderKind::Sky { id });
        id
    }
}

// ---- fixed-timestep simulation (shared by window and golden test) ----------

/// The last two simulation ticks of an example.
struct Simulation<E> {
    example: E,
    prev: RenderWorld,
    curr: RenderWorld,
    ticks: u64,
}

impl<E: Example> Simulation<E> {
    fn tick_seconds() -> f64 {
        1.0 / E::TICK_HZ
    }

    /// Simulate t = 0 into both worlds, so there is something to render.
    fn new(mut example: E) -> Self {
        let mut curr = RenderWorld::new();
        example.simulate(0.0, &mut curr);
        Self { example, prev: curr.clone(), curr, ticks: 0 }
    }

    fn step(&mut self) {
        self.prev.clone_from(&self.curr); // reuses prev's allocations
        self.curr.clear();
        self.ticks += 1;
        // Multiplying (rather than summing tick lengths) keeps time exact
        // and identical between runs.
        let time = self.ticks as f64 * Self::tick_seconds();
        self.example.simulate(time, &mut self.curr);
    }
}

// ---- shader hot reload ------------------------------------------------------------

#[derive(Default)]
struct ShaderWatcher {
    shaders: Vec<WatchedShader>,
    last_check: Option<Instant>,
}

struct WatchedShader {
    kind: ShaderKind,
    path: PathBuf,
    modified: Option<SystemTime>,
}

/// Which renderer entry point recompiles this shader.
enum ShaderKind {
    Mesh { id: MeshShaderId, blend: BlendMode },
    Skinned { id: SkinnedShaderId, blend: BlendMode },
    Sky { id: SkyShaderId },
    Text { id: TextShaderId, blend: BlendMode },
}

impl ShaderKind {
    /// Written against the `Renderer` trait: it works with any implementation.
    fn update(&self, renderer: &mut impl Renderer, label: &str, source: &str) -> Result<(), RenderError> {
        match *self {
            Self::Mesh { id, blend } => renderer.update_mesh_shader(id, &ShaderDesc::new(label, source).blend(blend)),
            Self::Skinned { id, blend } => {
                renderer.update_skinned_shader(id, &ShaderDesc::new(label, source).blend(blend))
            }
            Self::Sky { id } => renderer.update_sky_shader(id, &SkyShaderDesc::new(label, source)),
            Self::Text { id, blend } => renderer.update_text_shader(id, &ShaderDesc::new(label, source).blend(blend)),
        }
    }
}

impl ShaderWatcher {
    fn path(file: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/shaders").join(file)
    }

    /// Read `examples/shaders/<file>`, panicking if it's missing (setup only).
    fn read(file: &str) -> String {
        let path = Self::path(file);
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
    }

    fn watch(&mut self, file: &str, kind: ShaderKind) {
        let path = Self::path(file);
        self.shaders.push(WatchedShader { kind, modified: modified_time(&path), path });
    }

    /// Check (at most 4 times a second) for edited shader files.
    fn reload_changed(&mut self, renderer: &mut impl Renderer) {
        if self.last_check.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            return;
        }
        self.last_check = Some(Instant::now());

        for shader in &mut self.shaders {
            let modified = modified_time(&shader.path);
            if modified == shader.modified {
                continue;
            }
            shader.modified = modified;

            let label = shader.path.file_name().unwrap().to_string_lossy();
            let source = match std::fs::read_to_string(&shader.path) {
                Ok(source) => source,
                Err(err) => {
                    log::error!("{label}: {err}");
                    continue;
                }
            };
            match shader.kind.update(renderer, &label, &source) {
                Ok(()) => log::info!("reloaded {label}"),
                Err(err) => log::error!("{err}\n(keeping the previous shader)"),
            }
        }
    }
}

fn modified_time(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

// ---- small helpers for scenes ---------------------------------------------------

/// RGBA pixels of a procedural checkerboard (image loading will live in enso-io).
pub fn checkerboard(size: u32, cells: u32) -> Vec<u8> {
    let cell = size / cells;
    (0..size * size)
        .flat_map(|i| {
            let (x, y) = (i % size, i / size);
            let v = if (x / cell + y / cell).is_multiple_of(2) { 255 } else { 60 };
            [v, v, v, 255]
        })
        .collect()
}

/// Fully saturated colour from a hue in 0..1 (wraps).
pub fn hue(h: f32) -> Vec4 {
    let h = h.rem_euclid(1.0) * 6.0;
    let channel = |offset: f32| (((h + offset) % 6.0 - 3.0).abs() - 1.0).clamp(0.0, 1.0);
    Vec4::new(channel(0.0), channel(4.0), channel(2.0), 1.0)
}
