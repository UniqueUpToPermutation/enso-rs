# enso-audio

The audio engine. Its **public interface mirrors enso-visual's**: an
`AudioEngine` trait (`lib.rs`) implemented by `BlockAudioEngine`, with
resources (nodes) created up front and a plain-data `AudioWorld` submitted
per tick. **Underneath, and private,** it runs on the block platform
(`enso-block`; design: [design/blocks.md](../design/blocks.md)): a tree of
blocks in a **real-time workstream**; an `enso_block::Engine` on the
control thread builds and rebuilds it; an `AudioDriver` steps it from the
sound card's callback. Platform-agnostic: cpal 0.18 (WASAPI, CoreAudio,
ALSA/PipeWire). Nothing in the engine is OS-specific; keep it that way.

## The interface (what users see)

| enso-visual | enso-audio |
|---|---|
| `Renderer` trait / `WgpuRenderer` | `AudioEngine` trait / `BlockAudioEngine` |
| `create_mesh`, `create_material`… → `Copy` IDs | `create_node(&NodeDesc)` / `update_node` / `destroy_node` → `NodeId` |
| `material_params` + `material_param::<T>(id, "name")` → `Param<T>` | `node_params` + `param::<T>(node, "name")` → `Param<T>` (also `node_inputs`/`node_outputs`, `input`/`output(node, "name")` → pins) |
| `RenderWorld` (draws, `set_param`) | `AudioWorld` (`connect(from, to)`, `set_param`) |
| `render(target, prev, curr, alpha)` | `submit(&world)` |
| subrenderers private | blocks, `AudioDriver`, `enso_block` private |
| fake in `tests/renderer_trait.rs` | fake in `tests/audio_engine_trait.rs` |

- `node.rs`: `NodeId` (counter, never reused; `NodeId::SPEAKERS` = 0, built
  in), `NodeDesc` (enum, plain data; static `title`/`inputs`/`outputs`/
  `params`), `Signal`, `PinInfo`, `ParamInfo` (range, default, log scale,
  unit), `InputPin`/`OutputPin` (node + index; a bare `NodeId` is index 0),
  `Param<T>` (a hashed name: `ParamKey`), sealed `ParamType` (`f32` only).
  **Pin and param names are the blocks' field names**, so
  `Key::named(name)` is the block interface's key.
- `world.rs`: `AudioWorld { wires, params }`. Wiring is the whole truth
  each submit (a free graph); **params persist** until set again;
  wires/params naming destroyed nodes are skipped.
- `engine.rs`: `BlockAudioEngine`, `AudioError`, `MidiSender` (a cloneable
  `Arc<dyn Fn(MidiEvent)>`, so fakes can make one too).
  - `submit` = `spec(world)` (validates pins and signals →
    `PatchSpec`), rebuild if it differs from `applied` (a failing spec is
    remembered in `failed` and its error returned again without
    rebuilding; `Busy` isn't remembered), then `set_id` for param values
    that differ from `sent`, then `collect_garbage`.
  - Node changes are recorded immediately and applied by the next submit
    (batched into one rebuild).
  - `start(device, clock)` moves the driver to the device;
    `render(out, channels, now)` steps it offline (tests, no device).
- `patch.rs` (private): `PatchRoot`, the data-defined composite: fixed
  `MidiSource` and `AudioOut` children, one `child_with_id(node's random
  BlockId)` per node, "MIDI in" nodes are `MidiFilter`s fed from the
  source, wires via `connect_ids` (`Endpoint::Speakers` → the `AudioOut`
  input).
- Live MIDI bypasses ticks (`MidiSender` → `MidiSource` inbox) for
  latency. A future **sequencer will be a node** (on the audio thread), not
  something that feeds the world per tick (owner, 2026-11-22).

### Adding a node kind

1. The block in `src/blocks/` (knobs as `In<f32>` with `DEFAULT_*`
   consts, state as `#[state]`).
2. A `NodeDesc` variant and its `title`/`inputs`/`outputs`/`params` (names
   = field names) in `node.rs`; an arm in `PatchRoot::register`.
3. Use it in `tests/no_alloc.rs`; add it to the editor's `MENU`.

## Shape (underneath)

```
control thread                                    audio thread (device callback)
Engine ──rebuild / set / connect (rtrb)────────▶ AudioDriver → Workstream::step_with(&AudioContext)
       ◀──garbage (old graphs, values)──────────   per step (≤ max_block frames):
MIDI threads ──MailSender<MidiEvent>────────────▶   MidiSource inbox → blocks → AudioOut
                                                     then AudioOut's sound → device (speaker-safe)
any thread ◀──StatsHandle (atomics)──────────────
```

- **Ports:** audio ports carry `AudioBuf` (stereo planar, `max_block` long,
  first `frames()` valid, **zero beyond**: an invariant `write()`
  maintains). MIDI ports carry `MidiEvents` = `Events<BlockEvent>`
  (`time`, `offset` within the step, `port`, `message`), capacity
  `EVENT_CAPACITY`, dropping (never allocating) when full. Audio inputs
  that mix are declared `merge = Sum` (decision 15: the default is a single
  link); MIDI inputs are `merge = Append` (time-sorted).
- **`AudioContext`** (from `io.context::<AudioContext>()`): `sample_rate`,
  `frames`, `start..end`. `frame_of(time)`: rounded, late → 0, `None` if
  at/after `end`.
- **Knobs** are unconnected `In<f32>` (defaults as `DEFAULT_*` consts on
  the block), set with `Engine::set` / `set_id`, and smoothed in the block
  (`blocks::Smoothed`). Knob values survive rebuilds.
- **State** that should survive rebuilds is `#[state]` (synth voices,
  filter integrators, delay lines, MIDI pending queue).

## Blocks (`blocks/`)

- `MidiSource`: inbox `midi` (`MailSender` from the engine; survives
  rebuilds) → `#[state] pending` (sorted, capacity 4096) → `out` events of
  this step at their frames. Future-stamped events wait (scheduling).
- `MidiFilter { port, channel }`: structural configuration.
- `Transpose` (knob `semitones`, remembers sounding shifts),
  `TestSynth` (16 voices, ADSR, sustain pedal, pitch bend),
  `LowPass` (TPT SVF; `cutoff`, `resonance`), `Delay` (`time`,
  `feedback`, `mix`; lines allocated in `new`), `Gain` (`db`).
- `AudioOut`: the sink (`input`, `merge = Sum`). Expose it from the root
  and give its input handle to `AudioDriver::new`.

## Driver and device

- `AudioDriver::render(out, channels, now)`: the buffer stands for the
  `frames / rate` seconds **before** `now` (one buffer of latency, no
  jitter). It's cut into steps of ≤ `max_block`, each with its own
  `AudioContext` window, then reads `AudioOut`'s input with
  `Workstream::peek` (never allocates; silence if the output is gone),
  interleaves it (stereo → first two channels, mono → mix, extra channels
  silent), and makes it speaker-safe (NaN → 0, clip ±1). Stats are atomics
  (`StatsHandle::read`: load, peak, frames, disabled blocks).
- `OutputDevice::start(driver, clock)`: f32 scratch allocated up front,
  converted to the device's sample format; callbacks larger than the scratch
  are rendered in pieces with adjusted `now`.
- Windows shared mode: 480-frame callbacks (10 ms) at 48 kHz, first ~1056
  (prefill). Lower latency would mean exclusive WASAPI or ASIO (future).

## Real-time rules, enforced

- `tests/no_alloc.rs`: `assert_no_alloc` around `BlockAudioEngine::render`
  through MIDI, every node kind, param changes and **rewiring** (swaps apply inside
  `step`). Extend it when adding blocks.
- Blocks must not allocate in `process`: push to `Events` (bounded), write
  into preallocated `AudioBuf`s, keep `Vec`s within capacity.
- A panicking block is switched off by the workstream (counted in
  `Stats::disabled_blocks`); a rebuild gives it a fresh start.

## Using it (see `examples/play.rs`)

```rust
let mut audio = BlockAudioEngine::new(device.audio_config());
let synth = audio.create_node(&NodeDesc::TestSynth)?;
let filter = audio.create_node(&NodeDesc::LowPass)?;
let cutoff = audio.param::<f32>(filter, "cutoff")?;
let stream = audio.start(device, clock)?;          // or audio.render(..) offline
let midi = audio.midi_sender();                    // for MIDI threads

// each tick:
world.clear();
world.connect(midi_in, synth);
world.connect(synth, filter);
world.connect(filter, NodeId::SPEAKERS);
world.set_param(filter, cutoff, 800.0);
audio.submit(&world)?;                             // rewiring while it plays
```

Tests: `tests/audio_engine.rs` (public API, offline), `tests/audio_engine_trait.rs`
(a fake), `tests/no_alloc.rs` (public API), and block-level unit tests in
`src/tests.rs` (custom blocks, timing to the frame, speaker safety).

## Next

VST3 hosting (`enso-plugin`, as a block whose ports come from the plugin),
analysis blocks sending levels to visuals by mailbox, MIDI learn, delay
blocks for feedback.
