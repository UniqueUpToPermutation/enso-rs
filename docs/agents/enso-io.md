# enso-core and enso-io

## enso-core: shared plain types

No dependencies, by design: everything depends on it, so it must stay tiny.

- `time.rs`: `Clock` (an `Instant` epoch, `Copy`) and `Timestamp` (u64
  nanoseconds since the epoch). **Create one `Clock` at startup and share
  copies**; the MIDI thread, the audio thread (later) and the simulation
  only agree on time if they share an epoch. `Timestamp` is an integer, not
  an `Instant`, so tests can build "the event at 1.5 s" and values can be
  sent and compared exactly.
- `midi.rs`: `MidiMessage` (channel messages only), `parse(&[u8])` /
  `encode()`. `encode` returns `EncodedMessage` (a `[u8; 3]` plus a length)
  and never allocates, so it's safe on the audio thread. `MidiEvent { time,
  port, message }`, `Channel` (0..=15 stored, 1..=16 displayed), `PortId`,
  `NoteName` (60 = C4), `key_frequency`.
  - A note-on with velocity 0 parses as `NoteOff { velocity: 64 }` (MIDI spec).
  - System messages (SysEx, clock, active sensing) parse to `None`.

## enso-io: devices

`midi.rs`, on midir 0.10 (WinMM on Windows):

- `MidiSink`: where events go, called **on midir's thread**. Implemented for
  `mpsc::Sender<MidiEvent>` (to the sim), `rtrb::Producer<MidiEvent>`
  (to the audio thread; drops events when full), `(A, B)` (fan-out) and
  `FnSink(closure)`. There's no blanket impl for closures because it would
  conflict with the impl for `Producer` (coherence).
- `MidiIn::open(pattern, port_id, clock, sink)`: one input, by
  case-insensitive substring. It's closed on drop.
- `MidiInputs<S: MidiSink + Clone>`: all inputs (or `.only(pattern)`),
  with hot-plug support. Call `refresh()` about once a second: it opens new
  devices, drops vanished ones, and keeps each device name's `PortId`
  stable across replugging. Failures are logged once, then retried quietly.
  `rtrb::Producer` isn't `Clone` (single producer), so the audio engine
  takes MIDI through `enso_audio::MidiQueue` (`Arc<ArrayQueue<MidiEvent>>`,
  lock-free multi-producer) instead, which also implements `MidiSink`.
- `MidiOut::open(pattern)` / `send(MidiMessage)`: hardware synths, or a
  virtual port (loopMIDI) into a DAW.
- `input_names()` / `output_names()`.

### Facts worth knowing

- **Events are stamped on arrival** (`clock.now()` first thing in the
  callback). midir's own timestamp counts from an arbitrary per-port
  origin, so it can't be compared with our clock.
- **Windows: an input can be open in only one program at a time** (classic
  WinMM). If a DAW holds the keyboard, `MidiInputs` logs a warning and keeps
  retrying. The newer Windows MIDI Services lifts this; midir doesn't use it
  yet.
- `midir::Ignore::All` is set, so SysEx, clock and active sensing never
  reach the callback.
- Tests that touch the real MIDI system (`listing_ports_works_without_devices`)
  must pass with no devices attached.

## Examples

- `cargo run -p enso-io --example midi_monitor`: prints every message
  with its timestamp and the delay between arrival and printing.
- `cargo run -p enso-visual --example notes`: the keys light up by pitch
  class from any MIDI input. Its demo melody stops once you play.
  `enso-visual` has **dev**-dependencies on enso-core/enso-io for this; the
  library itself doesn't depend on them. The golden test never opens MIDI
  (`cfg!(test)`).

### Mapping event time to simulation time (as in `notes`)

The simulation has its own clock (`time` in `simulate`). The example fixes
`offset = clock.now() - sim_time` at the first tick, converts each event
to `event.time - offset`, and holds events that are later than the current
tick ("early"): the fixed-timestep loop may run slightly ahead of the wall
clock. Strikes keep their exact time, so ages and animations aren't
quantised to ticks. Expect this to move into `enso-engine` later.
