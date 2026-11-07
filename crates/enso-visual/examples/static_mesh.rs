//! A ring of textured cubes around a breathing sphere, simulated at a fixed
//! (deliberately low) 20 Hz and rendered at display rate by interpolating
//! between the last two simulation ticks.
//!
//! Run with:   cargo run -p enso-visual --example static_mesh
//!
//! * Press **Space** to toggle interpolation and see the 20 Hz stutter.
//! * Shaders in `examples/shaders/` reload when you save them. A broken
//!   shader logs its error and the last good version keeps running.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use enso_visual::{
    Camera, Gpu, MaterialId, MeshId, RenderTarget, RenderWorld, Renderer, ShaderId, TextureDesc, Transform,
    glam::{EulerRot, Quat, Vec3, Vec4},
    static_mesh::{BlendMode, MaterialDesc, MeshData, MeshDraw, ShaderDesc},
    wgpu,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

/// Simulation step. Real use would be much faster (120-240 Hz); 20 Hz makes
/// the effect of interpolation obvious.
const TICK: f64 = 1.0 / 20.0;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,enso_visual=info,static_mesh=info"))
        .init();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App { state: None };
    event_loop.run_app(&mut app).expect("event loop failed");
}

// ---- application shell (winit) ---------------------------------------------

struct App {
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    scene: Scene,

    // Fixed-timestep simulation state.
    prev: RenderWorld,
    curr: RenderWorld,
    sim_time: f64,
    accumulator: f64,
    last_frame: Instant,
    interpolate: bool,
}

impl ApplicationHandler for App {
    // winit tells us when we may create windows (on desktop: once, at start).
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
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
        let scene = Scene::new(&mut renderer);

        // Both worlds start at t = 0 so the first frame has something to show.
        let mut curr = RenderWorld::new();
        scene.simulate(0.0, &mut curr);
        let prev = curr.clone();

        let state = State {
            window,
            surface,
            config,
            renderer,
            scene,
            prev,
            curr,
            sim_time: 0.0,
            accumulator: 0.0,
            last_frame: Instant::now(),
            interpolate: true,
        };
        state.update_title();
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    state.config.width = size.width;
                    state.config.height = size.height;
                    state.surface.configure(&state.renderer.gpu().device, &state.config);
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && event.logical_key == Key::Named(NamedKey::Space) =>
            {
                state.interpolate = !state.interpolate;
                state.update_title();
            }
            WindowEvent::RedrawRequested => {
                state.frame();
                state.window.request_redraw(); // render continuously
            }
            _ => {}
        }
    }
}

impl State {
    fn update_title(&self) {
        let mode = if self.interpolate { "ON" } else { "OFF" };
        self.window.set_title(&format!(
            "enso · static mesh · {} Hz sim · interpolation {mode} (Space)",
            (1.0 / TICK).round()
        ));
    }

    fn frame(&mut self) {
        // 1. Advance the simulation in fixed steps. "Fix your timestep":
        //    wall-clock time accumulates, and is consumed one TICK at a time.
        let now = Instant::now();
        // Clamp so a long stall (e.g. dragging the window) can't trigger a
        // huge burst of catch-up ticks.
        self.accumulator += (now - self.last_frame).as_secs_f64().min(0.25);
        self.last_frame = now;

        while self.accumulator >= TICK {
            std::mem::swap(&mut self.prev, &mut self.curr);
            self.curr.clear();
            self.sim_time += TICK;
            self.scene.simulate(self.sim_time, &mut self.curr);
            self.accumulator -= TICK;
        }

        // 2. How far are we into the next tick? That's the blend factor.
        let alpha = if self.interpolate { (self.accumulator / TICK) as f32 } else { 1.0 };

        // 3. Live-coding: pick up edited shaders.
        self.scene.reload_changed_shaders(&mut self.renderer);

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

// ---- the scene: everything below uses only the enso-visual API -------------

struct Scene {
    cube: MeshId,
    sphere: MeshId,
    cube_material: MaterialId,
    blob_material: MaterialId,
    shaders: Vec<LiveShader>,
}

const CUBE_COUNT: u64 = 12;
const SPHERE_KEY: u64 = 1_000;

impl Scene {
    /// Create all GPU resources once, up front.
    fn new(renderer: &mut Renderer) -> Self {
        let pixels = checkerboard(64, 8);
        let checker = renderer
            .create_texture(&TextureDesc::rgba8(64, 64, Some(&pixels)).with_label("checker"))
            .unwrap();

        let lit = LiveShader::load(renderer, "lit.wgsl", BlendMode::Opaque);
        let wobble = LiveShader::load(renderer, "wobble.wgsl", BlendMode::Opaque);

        Self {
            cube: renderer.create_mesh(&MeshData::cube(0.8)),
            sphere: renderer.create_mesh(&MeshData::uv_sphere(1.2, 64, 32)),
            cube_material: renderer
                .create_material(&MaterialDesc::new(lit.id).with_textures(&[checker]))
                .unwrap(),
            blob_material: renderer.create_material(&MaterialDesc::new(wobble.id)).unwrap(),
            shaders: vec![lit, wobble],
        }
    }

    /// One simulation tick: describe the world at time `t`. Pure data out;
    /// this could run on another thread (or be an ECS system).
    fn simulate(&self, t: f64, world: &mut RenderWorld) {
        let t = t as f32;
        world.time = f64::from(t);
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
                hue(t * 0.05),                                         // params[0]: glow colour
                Vec4::new(0.08 + 0.06 * (t * 0.5).sin(), 0.0, 0.0, 0.0), // params[1].x: wobble
                Vec4::ZERO,
                Vec4::ZERO,
            ],
        );
        world.draw_mesh(MeshDraw::new(self.sphere, self.blob_material, Transform::IDENTITY).with_key(SPHERE_KEY));
    }

    fn reload_changed_shaders(&mut self, renderer: &mut Renderer) {
        for shader in &mut self.shaders {
            shader.reload_if_changed(renderer);
        }
    }
}

/// A shader read from `examples/shaders/` and recompiled in place (same
/// `ShaderId`, so materials keep working) whenever the file changes.
struct LiveShader {
    id: ShaderId,
    path: PathBuf,
    blend: BlendMode,
    modified: Option<SystemTime>,
    last_check: Instant,
}

impl LiveShader {
    fn load(renderer: &mut Renderer, file: &str, blend: BlendMode) -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/shaders").join(file);
        let source = std::fs::read_to_string(&path).expect("failed to read shader");
        let id = renderer
            .create_mesh_shader(&ShaderDesc::new(file, &source).blend(blend))
            .unwrap_or_else(|err| panic!("initial shader must compile: {err}"));
        Self { id, modified: modified_time(&path), path, blend, last_check: Instant::now() }
    }

    fn reload_if_changed(&mut self, renderer: &mut Renderer) {
        if self.last_check.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_check = Instant::now();

        let modified = modified_time(&self.path);
        if modified == self.modified {
            return;
        }
        self.modified = modified;

        let source = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(err) => return log::error!("{}: {err}", self.path.display()),
        };
        let label = self.path.file_name().unwrap().to_string_lossy();
        match renderer.update_mesh_shader(self.id, &ShaderDesc::new(&label, &source).blend(self.blend)) {
            Ok(()) => log::info!("reloaded {label}"),
            Err(err) => log::error!("{err}\n(keeping the previous shader)"),
        }
    }
}

fn modified_time(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// RGBA pixels of a procedural checkerboard (image loading will live in enso-io).
fn checkerboard(size: u32, cells: u32) -> Vec<u8> {
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
fn hue(h: f32) -> Vec4 {
    let h = h.rem_euclid(1.0) * 6.0;
    let channel = |offset: f32| (((h + offset) % 6.0 - 3.0).abs() - 1.0).clamp(0.0, 1.0);
    Vec4::new(channel(0.0), channel(4.0), channel(2.0), 1.0)
}
