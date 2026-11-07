//! Shared harness for the enso-visual examples.
//!
//! Cargo builds every `examples/*.rs` and `examples/*/main.rs` as an example.
//! This file is `examples/common/mod.rs`, which matches neither, so it is not
//! an example itself; each example includes it with `mod common;`.
//!
//! The harness owns everything that isn't the point of an example: logging,
//! the window and surface, the fixed-timestep simulation loop with
//! interpolated rendering, keyboard shortcuts and shader hot reload.
//! An example only implements [`Example`].
//!
//! Keys: **Space** toggles interpolation, **Esc** quits.

// Each example uses a different subset of this module; don't warn about the rest.
#![allow(dead_code)]

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use enso_visual::{
    Gpu, RenderTarget, RenderWorld, Renderer, ShaderId,
    glam::Vec4,
    static_mesh::{BlendMode, ShaderDesc},
    wgpu,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

// ---- what an example implements ---------------------------------------------

pub trait Example: 'static {
    /// Shown in the window title.
    const TITLE: &'static str;

    /// Simulation ticks per second. Rendering interpolates between ticks.
    const TICK_HZ: f64 = 60.0;

    /// Create GPU resources. Called once, after the window and GPU exist.
    fn init(setup: &mut Setup<'_>) -> Self;

    /// Describe the world at `time` (seconds). Called `TICK_HZ` times per
    /// second. `world` arrives with draws and material params cleared;
    /// camera, time and clear colour still hold the previous tick's values.
    fn simulate(&mut self, time: f64, world: &mut RenderWorld);
}

/// Handed to [`Example::init`].
pub struct Setup<'a> {
    pub renderer: &'a mut Renderer,
    shaders: &'a mut ShaderWatcher,
}

impl Setup<'_> {
    /// Load `examples/shaders/<file>` as a static mesh shader. It is
    /// recompiled in place whenever the file is saved; a broken edit logs
    /// the error and keeps the previous version running.
    pub fn mesh_shader(&mut self, file: &str, blend: BlendMode) -> ShaderId {
        self.shaders.load(self.renderer, file, blend)
    }
}

/// Open a window and run `E` until it is closed.
pub fn run<E: Example>() {
    // `CARGO_CRATE_NAME` is the example's own name, so its logs show too.
    let filter = format!("warn,enso_visual=info,{}=info", env!("CARGO_CRATE_NAME"));
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(filter)).init();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::<E> { state: None };
    event_loop.run_app(&mut app).expect("event loop failed");
}

// ---- window + loop ----------------------------------------------------------------

struct App<E> {
    state: Option<State<E>>,
}

struct State<E> {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    shaders: ShaderWatcher,
    example: E,

    // Fixed-timestep simulation.
    prev: RenderWorld,
    curr: RenderWorld,
    sim_time: f64,
    accumulator: f64,
    last_frame: Instant,
    interpolate: bool,
}

impl<E: Example> ApplicationHandler for App<E> {
    // winit tells us when we may create windows (on desktop: once, at start).
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            self.state = Some(State::new(event_loop));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.resize(size.width, size.height),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Named(NamedKey::Space) => {
                        state.interpolate = !state.interpolate;
                        state.update_title();
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                state.frame();
                state.window.request_redraw(); // render continuously
            }
            _ => {}
        }
    }
}

impl<E: Example> State<E> {
    fn new(event_loop: &ActiveEventLoop) -> Self {
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes())
                .expect("failed to create window"),
        );

        let instance = Gpu::create_instance();
        let surface = instance.create_surface(window.clone()).expect("failed to create surface");
        let gpu = Gpu::new_blocking(instance, Some(&surface)).expect("failed to open GPU");

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .expect("surface not supported by adapter");
        // Prefer an sRGB format so colours are gamma-correct.
        let caps = surface.get_capabilities(&gpu.adapter);
        if let Some(srgb) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
            config.format = srgb;
        }
        surface.configure(&gpu.device, &config);

        let mut renderer = Renderer::new(gpu, config.format);
        let mut shaders = ShaderWatcher::default();
        let mut example = E::init(&mut Setup { renderer: &mut renderer, shaders: &mut shaders });

        // Both worlds start at t = 0 so the first frame has something to show.
        let mut curr = RenderWorld::new();
        example.simulate(0.0, &mut curr);
        let prev = curr.clone();

        let state = Self {
            window,
            surface,
            config,
            renderer,
            shaders,
            example,
            prev,
            curr,
            sim_time: 0.0,
            accumulator: 0.0,
            last_frame: Instant::now(),
            interpolate: true,
        };
        state.update_title();
        state
    }

    fn update_title(&self) {
        let mode = if self.interpolate { "ON" } else { "OFF" };
        self.window.set_title(&format!(
            "enso · {} · {} Hz sim · interpolation {mode} (Space)",
            E::TITLE,
            E::TICK_HZ
        ));
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.renderer.gpu().device, &self.config);
        }
    }

    fn frame(&mut self) {
        let tick = 1.0 / E::TICK_HZ;

        // 1. Advance the simulation in fixed steps ("fix your timestep"):
        //    wall-clock time accumulates and is consumed one tick at a time.
        //    Clamped so a stall (e.g. dragging the window) can't trigger a
        //    huge burst of catch-up ticks.
        let now = Instant::now();
        self.accumulator += (now - self.last_frame).as_secs_f64().min(0.25);
        self.last_frame = now;

        while self.accumulator >= tick {
            self.prev.clone_from(&self.curr); // reuses prev's allocations
            self.curr.clear();
            self.sim_time += tick;
            self.example.simulate(self.sim_time, &mut self.curr);
            self.accumulator -= tick;
        }

        // 2. How far into the next tick are we? That's the blend factor.
        let alpha = if self.interpolate { (self.accumulator / tick) as f32 } else { 1.0 };

        // 3. Live coding: pick up edited shaders.
        self.shaders.reload_changed(&mut self.renderer);

        // 4. Render between the two most recent ticks.
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                // Draw this one, reconfigure for the next.
                self.surface.configure(&self.renderer.gpu().device, &self.config);
                t
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.renderer.gpu().device, &self.config);
                return;
            }
            // Minimised, timed out, etc.: skip this frame.
            _ => return,
        };
        let view = surface_texture.texture.create_view(&Default::default());

        self.renderer.render(
            RenderTarget { view: &view, width: self.config.width, height: self.config.height },
            &self.prev,
            &self.curr,
            alpha,
        );

        self.window.pre_present_notify();
        self.renderer.gpu().queue.present(surface_texture);
    }
}

// ---- shader hot reload ------------------------------------------------------------

#[derive(Default)]
struct ShaderWatcher {
    shaders: Vec<WatchedShader>,
    last_check: Option<Instant>,
}

struct WatchedShader {
    id: ShaderId,
    path: PathBuf,
    blend: BlendMode,
    modified: Option<SystemTime>,
}

impl ShaderWatcher {
    fn load(&mut self, renderer: &mut Renderer, file: &str, blend: BlendMode) -> ShaderId {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/shaders").join(file);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let id = renderer
            .create_mesh_shader(&ShaderDesc::new(file, &source).blend(blend))
            .unwrap_or_else(|err| panic!("initial shader must compile: {err}"));
        self.shaders.push(WatchedShader { id, modified: modified_time(&path), path, blend });
        id
    }

    /// Check (at most 4 times a second) for edited shader files.
    fn reload_changed(&mut self, renderer: &mut Renderer) {
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
            match renderer.update_mesh_shader(shader.id, &ShaderDesc::new(&label, &source).blend(shader.blend)) {
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
