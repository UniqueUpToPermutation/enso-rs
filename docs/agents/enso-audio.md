# enso-audio

The audio engine, **built on the block platform** (`enso-block`; design:
[design/blocks.md](../design/blocks.md), build-order step 4). A patch is a
tree of blocks in a **real-time workstream**; an `enso_block::Engine` on the
control thread builds and rebuilds it; an `AudioDriver` steps it from the
sound card's callback. Platform-agnostic: cpal 0.18 (WASAPI, CoreAudio,
ALSA/PipeWire). Nothing in the engine is OS-specific; keep it that way.

## Shape

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

- `tests/no_alloc.rs`: `assert_no_alloc` around `driver.render` through
  MIDI, every block, knob changes and **rebuilds** (swaps apply inside
  `step`). Extend it when adding blocks.
- Blocks must not allocate in `process`: push to `Events` (bounded), write
  into preallocated `AudioBuf`s, keep `Vec`s within capacity.
- A panicking block is switched off by the workstream (counted in
  `Stats::disabled_blocks`); a rebuild gives it a fresh start.

## Using it (see `examples/play.rs`)

```rust
let mut engine = Engine::new();
let audio = engine.add("audio", Workstream::build_realtime(Patch::new(config))?);
let root = engine.root::<Patch>(audio)?;
let midi = engine.sender(engine.get(root.source)?.input)?;   // for MIDI threads
let output = engine.get(root.output)?.input;
let driver = AudioDriver::new(engine.take(audio).unwrap(), output, config);
let stream = device.start(driver, clock)?;
engine.rebuild(audio, Patch::other(config))?;                // while it plays
```

A crate using `#[derive(Ports)]` must depend on `enso-block` directly (the
generated code names `::enso_block`); the `enso_audio::enso_block`
re-export isn't enough.

## Next

VST3 hosting (`enso-plugin`, as a block whose ports come from the plugin),
analysis blocks sending levels to visuals by mailbox, MIDI learn, delay
blocks for feedback.
