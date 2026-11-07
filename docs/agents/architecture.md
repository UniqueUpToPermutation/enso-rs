# Architecture

## Planned crates

```
enso/
├── crates/
│   ├── enso-core/    shared plain types: events, timestamps, IDs, messages (few deps)
│   ├── enso-io/      MIDI in/out (midir), asset loading/decoding, file watching
│   ├── enso-audio/   cpal output stream, synth/DSP, real-time-safe queues
│   ├── enso-visual/  wgpu renderer                                  ← exists
│   ├── enso-engine/  bevy_ecs world + systems; bridges io/audio/visual
│   └── enso-ui/      egui panels (MIDI learn, parameters)
└── enso/             binary: owns winit window(s) + main loop, wires everything
```

Dependencies only point downward: `enso` → `enso-engine`/`enso-ui` →
`enso-audio`/`enso-visual`/`enso-io` → `enso-core`. Lower crates never know
about the ECS.

## Threads and data flow

```
MIDI callback thread ──(crossbeam channel)──▶ ECS / simulation (fixed tick, 120-240 Hz intended)
        │                                          │                  │
        │ (lock-free ring buffer, lowest latency)  │ commands (rtrb)  │ RenderWorld per tick
        ▼                                          ▼                  ▼
   audio thread (cpal callback)  ◀───────────  enso-audio        main thread: winit loop
   never allocates/locks/blocks   analysis ──▶ (meters, FFT)     Renderer::render(prev, curr, alpha)
```

- Notes go straight from MIDI to audio for latency, and are copied to the
  ECS for visuals and logic.
- Events carry high-resolution timestamps. Don't quantise; the player
  doesn't play to a fixed grid. Rhythm, if needed, is *derived* from what
  they play.
- Rendering is interpolated between the last two simulation ticks, so it
  lags the sim by up to one tick. Keep the tick rate high.

## Asset loading (planned, enso-io)

`enso-io` decodes files into CPU-side data types defined in `enso-core`
(`ImageData`, `SampleData`, `ShaderSource`). The consuming crate uploads them
(`enso-visual` → GPU texture, `enso-audio` → in-memory sample), so `enso-io`
never depends on wgpu or cpal. Loading happens off the main thread. Hot
reload is via `notify`. Feature flags gate formats (`image` with
`default-features = false`).

## Windowing

The application owns winit and the `wgpu::Surface` (configure, acquire,
present). Expected: an output window (projector) plus a control window
(egui), sharing one `WgpuRenderer`. A winit-free `WindowSurface` helper in
`enso-visual` was proposed but not built.
