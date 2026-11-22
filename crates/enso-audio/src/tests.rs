//! Block-level tests (the blocks and driver are private), rendered offline: `AudioDriver::render`
//! with chosen timestamps, so timing is exact and no sound card is needed.
//! Patches run in a real-time workstream controlled by an `Engine`, as in
//! the app.

use std::time::Duration;

use enso_block::{Block, Child, Engine, In, Io, LinkError, Linker, MailSender, Out, Ports, Workstream, WorkstreamId};

use crate::{
    AudioBuf, AudioConfig, AudioContext, MidiEvents,
    blocks::{AudioOut, Gain, MidiFilter, MidiSource, TestSynth, Transpose},
    driver::AudioDriver,
    midi_events,
};
use enso_core::{Channel, MidiEvent, MidiMessage, PortId, Timestamp};

const RATE: u32 = 48_000;
/// Frames per simulated device buffer.
const BUFFER: usize = 256;
/// A small `max_block`, so device buffers are processed in several steps.
const CONFIG: AudioConfig = AudioConfig { sample_rate: RATE, max_block: 64 };

// ---- timing helpers -----------------------------------------------------------------

/// Time of the first device buffer's end. Later buffers follow on.
const T0: Duration = Duration::from_secs(1);

fn buffer_end(index: u64) -> Timestamp {
    Timestamp::ZERO + T0 + frames(index * BUFFER as u64)
}

fn frames(n: u64) -> Duration {
    Duration::from_nanos((n * 1_000_000_000).div_ceil(u64::from(RATE)))
}

/// The time of frame `frame` of buffer `index` (buffers cover the period
/// *before* their end time).
fn at(index: u64, frame: u64) -> Timestamp {
    buffer_end(index) - frames(BUFFER as u64) + frames(frame)
}

fn note_on(time: Timestamp, channel: u8, key: u8) -> MidiEvent {
    MidiEvent {
        time,
        port: PortId(0),
        message: MidiMessage::NoteOn { channel: Channel::new(channel).unwrap(), key, velocity: 100 },
    }
}

fn note_off(time: Timestamp, key: u8) -> MidiEvent {
    MidiEvent {
        time,
        port: PortId(0),
        message: MidiMessage::NoteOff { channel: Channel::new(0).unwrap(), key, velocity: 64 },
    }
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

// ---- a test rig ---------------------------------------------------------------------

/// Blocks that turn MIDI into audio (what `Chain` puts in the middle).
trait Instrument: Block {
    fn midi(&self) -> In<MidiEvents>;
    fn out(&self) -> Out<AudioBuf>;
}

impl Instrument for TestSynth {
    fn midi(&self) -> In<MidiEvents> {
        self.midi
    }
    fn out(&self) -> Out<AudioBuf> {
        self.out
    }
}

/// MIDI source → instrument → output.
#[derive(Ports)]
struct Chain<I: Instrument> {
    #[child]
    source: Child<MidiSource>,
    instrument_value: Option<I>,
    instrument: Child<I>,
    #[child(init = AudioOut::new(CONFIG))]
    output: Child<AudioOut>,
}

impl<I: Instrument> Chain<I> {
    fn new(instrument: I) -> Self {
        Self { source: Child::default(), instrument_value: Some(instrument), instrument: Child::default(), output: Child::default() }
    }
}

impl<I: Instrument> Block for Chain<I> {
    fn register(&mut self, r: &mut enso_block::Registrar) {
        let instrument = self.instrument_value.take().expect("registered once");
        self.instrument = r.child("instrument", instrument);
    }
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.source].out, l[self.instrument].midi())?;
        l.connect(l[self.instrument].out(), l[self.output].input)
    }
}

/// An engine with one real-time audio workstream, taken out into a driver.
struct Rig {
    engine: Engine,
    id: WorkstreamId,
    driver: AudioDriver,
    midi: MailSender<MidiEvent>,
}

impl Rig {
    fn new<I: Instrument>(instrument: I) -> Self {
        let mut engine = Engine::new();
        let id = engine.add("audio", Workstream::build_realtime(Chain::new(instrument)).unwrap());
        let root = engine.root::<Chain<I>>(id).unwrap();
        let input = engine.get(root.source).unwrap().input;
        let output = engine.get(root.output).unwrap().input;
        let midi = engine.sender(input).unwrap();
        let runner = engine.take(id).unwrap();
        Self { engine, id, driver: AudioDriver::new(runner, output, CONFIG), midi }
    }

    /// Render buffer `index` as interleaved stereo; returns the left channel.
    fn render(&mut self, index: u64) -> Vec<f32> {
        let mut out = vec![0.0; BUFFER * 2];
        self.driver.render(&mut out, 2, buffer_end(index));
        self.engine.collect_garbage();
        out.chunks(2).map(|f| f[0]).collect()
    }
}

/// Writes 1.0 at the frame of every MIDI event it receives.
#[derive(Ports)]
struct Impulses {
    #[input(name = "midi", default = midi_events(), merge = Append)]
    midi: In<MidiEvents>,
    #[output(initial = AudioBuf::new(CONFIG))]
    out: Out<AudioBuf>,
}

impl Impulses {
    fn new() -> Self {
        Self { midi: In::default(), out: Out::default() }
    }
}

impl Block for Impulses {
    fn process(&mut self, io: &Io) {
        let Some(ctx) = io.context::<AudioContext>() else { return };
        let events = io.read(self.midi);
        let mut out = io.write(self.out);
        let [left, right] = out.write(ctx.frames);
        left.fill(0.0);
        right.fill(0.0);
        for event in events.as_slice() {
            left[event.offset as usize] = 1.0;
        }
    }
}

impl Instrument for Impulses {
    fn midi(&self) -> In<MidiEvents> {
        self.midi
    }
    fn out(&self) -> Out<AudioBuf> {
        self.out
    }
}

fn hits(samples: &[f32]) -> Vec<usize> {
    samples.iter().enumerate().filter(|(_, s)| **s != 0.0).map(|(i, _)| i).collect()
}

// ---- timing -------------------------------------------------------------------------

#[test]
fn midi_lands_on_its_exact_frame() {
    let mut rig = Rig::new(Impulses::new());
    // Frame 100 is in the second 64-frame step: offsets survive chunking.
    rig.midi.send(note_on(at(0, 100), 0, 60));
    // Stamped after the first buffer's end: must wait for the second.
    rig.midi.send(note_on(at(1, 10), 0, 62));
    assert_eq!(hits(&rig.render(0)), [100]);
    assert_eq!(hits(&rig.render(1)), [10]);
}

#[test]
fn late_midi_plays_at_once() {
    let mut rig = Rig::new(Impulses::new());
    rig.midi.send(note_on(at(0, 0) - frames(BUFFER as u64), 0, 60));
    assert_eq!(hits(&rig.render(0)), [0]);
}

// ---- the synth ----------------------------------------------------------------------

#[test]
fn test_synth_plays_and_releases() {
    let mut rig = Rig::new(TestSynth::new(CONFIG));
    rig.midi.send(note_on(at(0, 0), 0, 69));
    rig.midi.send(note_off(at(20, 0), 69));
    let sounding = rig.render(1);
    assert!(rms(&sounding) > 0.02, "rms {}", rms(&sounding));
    // 3 s after the release starts, the voice has switched itself off.
    for i in 2..580 {
        rig.render(i);
    }
    assert_eq!(rms(&rig.render(580)), 0.0);
}

#[test]
fn rebuilding_keeps_notes_sounding() {
    let mut rig = Rig::new(TestSynth::new(CONFIG));
    rig.midi.send(note_on(at(0, 0), 0, 69));
    rig.render(0);
    let before = rms(&rig.render(1));
    // Rebuild while the note sounds: the synth's voices are #[state].
    rig.engine.rebuild(rig.id, Chain::new(TestSynth::new(CONFIG))).unwrap();
    let after = rms(&rig.render(2));
    assert!(after > 0.5 * before, "voice survived the rebuild ({before} → {after})");
    // The MIDI sender from before the rebuild still reaches the new graph.
    rig.midi.send(note_off(at(3, 0), 69));
    for i in 3..600 {
        rig.render(i);
    }
    assert_eq!(rms(&rig.render(600)), 0.0, "the note-off arrived");
}

// ---- routing, knobs, output ---------------------------------------------------------

/// MIDI source → (transpose, filter) → synth → gain → output.
#[derive(Ports)]
struct Processed {
    #[child]
    source: Child<MidiSource>,
    #[child]
    transpose: Child<Transpose>,
    #[child(init = MidiFilter::new(None, Channel::new(1)))]
    filter: Child<MidiFilter>,
    #[child(init = Impulses::new())]
    keys: Child<Impulses>,
    #[child(init = Gain::new(CONFIG))]
    gain: Child<Gain>,
    #[child(init = AudioOut::new(CONFIG))]
    output: Child<AudioOut>,
}

impl Block for Processed {
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.source].out, l[self.filter].input)?;
        l.connect(l[self.filter].out, l[self.transpose].input)?;
        l.connect(l[self.transpose].out, l[self.keys].midi)?;
        l.connect(l[self.keys].out, l[self.gain].input)?;
        l.connect(l[self.gain].out, l[self.output].input)
    }
}

#[test]
fn midi_blocks_and_knobs() {
    let mut engine = Engine::new();
    let id = engine.add("audio", Workstream::build_realtime(Processed {
        source: Child::default(),
        transpose: Child::default(),
        filter: Child::default(),
        keys: Child::default(),
        gain: Child::default(),
        output: Child::default(),
    }).unwrap());
    let root = engine.root::<Processed>(id).unwrap();
    let midi = engine.sender(engine.get(root.source).unwrap().input).unwrap();
    let output = engine.get(root.output).unwrap().input;
    let semitones = engine.get(root.transpose).unwrap().semitones;
    let db = engine.get(root.gain).unwrap().db;
    let mut driver = AudioDriver::new(engine.take(id).unwrap(), output, CONFIG);
    let render = |driver: &mut AudioDriver, index| {
        let mut out = vec![0.0; BUFFER * 2];
        driver.render(&mut out, 2, buffer_end(index));
        out.chunks(2).map(|f| f[0]).collect::<Vec<_>>()
    };

    // Knobs set from the control side, while the driver owns the workstream.
    engine.set(semitones, 12.0).unwrap();
    engine.set(db, 0.0).unwrap();
    for i in 0..20 {
        render(&mut driver, i); // ~100 ms: the gain glides from -6 dB to 0 dB
    }
    midi.send(note_on(at(20, 10), 0, 60)); // channel 1 (index 0): filtered out
    midi.send(note_on(at(20, 20), 1, 60)); // channel 2: passes
    let out = render(&mut driver, 20);
    assert_eq!(hits(&out), [20]);
    assert!((out[20] - 1.0).abs() < 1e-3, "0 dB passes the impulse unchanged: {}", out[20]);

    engine.set(db, -60.0).unwrap();
    for i in 21..40 {
        render(&mut driver, i);
    }
    midi.send(note_on(at(40, 5), 1, 61));
    // Smoothing approaches zero exponentially: inaudible, not exactly 0.
    let level = render(&mut driver, 40)[5];
    assert!(level.abs() < 1e-3, "-60 dB is (nearly) silence: {level}");
}

/// Outputs a constant (left, right).
#[derive(Ports)]
struct Constant {
    value: [f32; 2],
    #[output(initial = AudioBuf::new(CONFIG))]
    out: Out<AudioBuf>,
}

impl Block for Constant {
    fn process(&mut self, io: &Io) {
        let Some(ctx) = io.context::<AudioContext>() else { return };
        let mut out = io.write(self.out);
        let [l, r] = out.write(ctx.frames);
        l.fill(self.value[0]);
        r.fill(self.value[1]);
    }
}

#[derive(Ports)]
struct TwoConstants {
    a: [f32; 2],
    b: [f32; 2],
    #[child(init = Constant { value: self.a, out: Out::default() })]
    first: Child<Constant>,
    #[child(init = Constant { value: self.b, out: Out::default() })]
    second: Child<Constant>,
    #[child(init = AudioOut::new(CONFIG))]
    output: Child<AudioOut>,
}

impl Block for TwoConstants {
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.first].out, l[self.output].input)?;
        l.connect(l[self.second].out, l[self.output].input)
    }
}

fn constants(a: [f32; 2], b: [f32; 2]) -> AudioDriver {
    let root = TwoConstants { a, b, first: Child::default(), second: Child::default(), output: Child::default() };
    let ws = Workstream::build_realtime(root).unwrap();
    let output = ws.get(ws.root::<TwoConstants>().unwrap().output).unwrap().input;
    AudioDriver::new(ws, output, CONFIG)
}

#[test]
fn the_output_sums_its_inputs() {
    let mut driver = constants([0.25, 0.1], [0.5, 0.2]);
    let mut out = vec![0.0; BUFFER * 2];
    driver.render(&mut out, 2, buffer_end(0));
    assert!(out.chunks(2).all(|f| (f[0] - 0.75).abs() < 1e-6 && (f[1] - 0.3).abs() < 1e-6));
}

#[test]
fn output_is_speaker_safe() {
    let mut driver = constants([f32::NAN, 10.0], [0.0, 0.0]);
    let mut out = vec![0.0; BUFFER * 2];
    driver.render(&mut out, 2, buffer_end(0));
    assert!(out.chunks(2).all(|f| f == [0.0, 1.0]));
}

#[test]
fn mono_and_multichannel_devices() {
    let mut driver = constants([0.2, 0.4], [0.0, 0.0]);
    let mut mono = vec![0.0; BUFFER];
    driver.render(&mut mono, 1, buffer_end(0));
    assert!(mono.iter().all(|s| (s - 0.3).abs() < 1e-6));
    let mut quad = vec![9.0; BUFFER * 4];
    driver.render(&mut quad, 4, buffer_end(1));
    assert!(quad.chunks(4).all(|f| f == [0.2, 0.4, 0.0, 0.0]));
}

#[test]
fn a_panicking_block_is_switched_off_and_counted() {
    #[derive(Ports)]
    struct Faulty {
        #[output(initial = AudioBuf::new(CONFIG))]
        out: Out<AudioBuf>,
    }
    impl Block for Faulty {
        fn process(&mut self, _: &Io) {
            panic!("bug in a block");
        }
    }
    #[derive(Ports)]
    struct Root {
        #[child(init = Faulty { out: Out::default() })]
        faulty: Child<Faulty>,
        #[child(init = Constant { value: [0.5, 0.5], out: Out::default() })]
        good: Child<Constant>,
        #[child(init = AudioOut::new(CONFIG))]
        output: Child<AudioOut>,
    }
    impl Block for Root {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect(l[self.faulty].out, l[self.output].input)?;
            l.connect(l[self.good].out, l[self.output].input)
        }
    }
    let root = Root { faulty: Child::default(), good: Child::default(), output: Child::default() };
    let ws = Workstream::build_realtime(root).unwrap();
    let output = ws.get(ws.root::<Root>().unwrap().output).unwrap().input;
    let mut driver = AudioDriver::new(ws, output, CONFIG);
    let stats = driver.stats();
    let mut out = vec![0.0; BUFFER * 2];
    driver.render(&mut out, 2, buffer_end(0));
    assert_eq!(out[2 * (BUFFER - 1)], 0.5, "the rest keeps playing");
    let read = stats.read();
    assert_eq!((read.disabled_blocks, read.callbacks, read.frames), (1, 1, BUFFER as u32));
}
