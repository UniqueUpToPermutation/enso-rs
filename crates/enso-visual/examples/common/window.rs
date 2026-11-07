//! The interactive window: winit + surface + real-time loop.

use std::{sync::Arc, time::Instant};

use enso_visual::{Gpu, RenderTarget, Renderer, wgpu};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

use super::{Example, Setup, ShaderWatcher, Simulation};

/// Open a window and run `E` until it is closed.
pub fn run<E: Example>() {
    // `CARGO_CRATE_NAME` is the example's own name, so its logs show too.
    let filter = format!("warn,enso_visual=info,{}=info", env!("CARGO_CRATE_NAME"));
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(filter)).init();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::<E> { state: None };
    event_loop.run_app(&mut app).expect("event loop failed");
}

struct App<E> {
    state: Option<State<E>>,
}

struct State<E> {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    shaders: ShaderWatcher,
    sim: Simulation<E>,
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
        let example = E::init(&mut Setup { renderer: &mut renderer, shaders: &mut shaders });

        let state = Self {
            window,
            surface,
            config,
            renderer,
            shaders,
            sim: Simulation::new(example),
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
        let tick = Simulation::<E>::tick_seconds();

        // 1. Advance the simulation in fixed steps ("fix your timestep"):
        //    wall-clock time accumulates and is consumed one tick at a time.
        //    Clamped so a stall (e.g. dragging the window) can't trigger a
        //    huge burst of catch-up ticks.
        let now = Instant::now();
        self.accumulator += (now - self.last_frame).as_secs_f64().min(0.25);
        self.last_frame = now;

        while self.accumulator >= tick {
            self.sim.step();
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
            &self.sim.prev,
            &self.sim.curr,
            alpha,
        );

        self.window.pre_present_notify();
        self.renderer.gpu().queue.present(surface_texture);
    }
}
