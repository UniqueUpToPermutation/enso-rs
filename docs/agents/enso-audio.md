# enso-audio

The audio engine: a graph of nodes that can be rewired while it plays,
run on the real-time audio thread. Platform-agnostic: cpal 0.18 (WASAPI,
CoreAudio, ALSA/PipeWire). Nothing in the engine is OS-specific; keep it
that way.

## The two halves

```
control thread                              audio thread (device callback)
AudioEngine ──Command (rtrb)──────────────▶ Processor
  add_node / remove_node / set_patch          applies commands between buffers
  compiles Patch → Schedule (allocates)       MIDI → sample offsets, runs steps
  frees Garbage ◀──Garbage (rtrb)────────── old schedules, removed nodes, panic payloads
any thread ──MidiEvent (MidiQueue)────────▶ pending (sorted by time)
           ◀──SharedStats (atomics)─────────
```

- **`Node`** (`node.rs`): `process(Process)`. Process gives stereo planar
  `input` (the sum of everything connected) and `output` (zeroed first),
  `events: &[BlockEvent]` (sorted by `offset`), and `events_out`
  (fixed capacity, for MIDI to other nodes).
- **`Patch`** (`patch.rs`): plain data. It lists audio edges, MIDI edges
  (`MidiFrom::Input(filter)` or `MidiFrom::Node`) and which nodes feed the
  output. `compile` does a topological sort (Kahn's algorithm, in mention
  order, with `EngineError::Cycle` on a cycle) and allocates **every
  buffer** into a `Schedule`. Only nodes mentioned in the patch run.
- **`AudioEngine`** (`engine.rs`): slot table with generations
  (`NodeId { slot, generation }`), so stale IDs are rejected. Commands
  are applied in order, so Insert → Schedule → Remove is always safe.
  `collect_garbage()` runs in every method; `stats()` also resets
  `peak_load`.
- **`Processor`** (`processor.rs`): node table of `MAX_NODES` (1024)
  `Option<Box<dyn Node>>`. It applies a command only while the garbage
  queue has room, so it never has to free memory itself. Device buffers
  larger than `max_block` (512) are processed in chunks.
- **`OutputDevice` / `AudioStream`** (`device.rs`): picks the device and
  format. The callback renders f32 into a scratch buffer allocated up
  front, then converts to the device's sample type. Every cpal format
  except the 48-bit ones is supported.

## Real-time rules, enforced

- `tests/no_alloc.rs` installs `assert_no_alloc`'s allocator, which
  **aborts** on allocation inside `assert_no_alloc(|| processor.process(..))`.
  It drives MIDI, chunking, MIDI between nodes, patch swaps and node
  inserts. Any new feature on the audio path should be added to this test.
- Things that would allocate on the audio thread, and what we do instead:
  - Stable `sort`: replaced by an insertion sort (`sort_by_offset`).
  - `Vec::push` beyond capacity: everything is pushed only while
    `len < capacity`.
  - Dropping a `Box`: sent back as `Garbage`.
  - Logging: never in `Processor`; the stream's error callback runs on
    cpal's own thread.
- A panicking node is caught with `catch_unwind`, counted in
  `Stats::node_panics`, and removed (sent back as garbage). The rest keeps
  playing.
- Output is speaker-safe: NaN or infinity becomes 0, and samples are
  hard-clipped to ±1. **Tests must keep test signals within ±1**, or the
  clip will confuse them.

## Timing

`Processor::process(out, channels, now)`: the buffer stands for the
`frames / rate` seconds *before* `now`, so events land at their exact
frame (rounded to nearest) and the engine sounds one buffer late, without
jitter. Late events play at frame 0; events stamped after `now` wait in
`pending`. The device callback passes `clock.now()`, and tests pass chosen
timestamps, so offline rendering is exact. Future events pushed into the
`MidiQueue` act as a simple scheduler (the `play` example's arpeggio).

On the owner's machine (Windows shared mode), callbacks are 480 frames
(10 ms) at 48 kHz, and the first callback is ~1056 (prefill). Total
latency is roughly 2 periods plus the OS mixer. Lowering it would mean
exclusive-mode WASAPI or ASIO (cpal `asio` feature), which is future work.

## MIDI in

`engine.midi_queue()` is an `Arc<ArrayQueue<MidiEvent>>` (bounded,
lock-free, multi-producer). enso-io implements `MidiSink` for it, so
`MidiInputs::new(clock, engine.midi_queue())` wires every device straight
in. The engine doesn't depend on enso-io (only the example does).

## Built-in nodes (`nodes/`)

- `FnNode(closure)`: for tests and experiments.
- `TestSynth`: 16 voices, sine with 2nd and 3rd harmonics, ADSR, velocity,
  sustain pedal, pitch bend (±2 semitones), oldest-voice stealing. It's a
  stand-in until plugin hosting exists.

## Next (see architecture.md)

Parameters with smoothing, analysis taps for visuals, crossfading on
rewire, a VST3 hosting spike (`enso-plugin`), a hot-reloaded patch file,
and MIDI processor nodes (the sequencer).
