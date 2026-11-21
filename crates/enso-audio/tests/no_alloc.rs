//! The audio thread must never allocate: allocation can take a lock inside
//! the allocator and stall for an unbounded time. This test installs an
//! allocator that **aborts** on any allocation inside `assert_no_alloc`, and
//! drives the driver through everything it does live: MIDI, chunking, every
//! block, knob changes, and rebuilds swapped in mid-performance.
//!
//! (It's its own test binary because the global allocator is per binary.)

use std::time::Duration;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use enso_audio::{
    AudioConfig, AudioDriver,
    blocks::{AudioOut, Delay, Gain, LowPass, MidiFilter, MidiSource, TestSynth, Transpose},
    enso_block::{Block, Child, Engine, LinkError, Linker, Ports, Registrar, Workstream},
};
use enso_core::{Channel, MidiEvent, MidiMessage, PortId, Timestamp};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const CONFIG: AudioConfig = AudioConfig { sample_rate: 48_000, max_block: 128 };

/// source → filter → transpose → synth(s) → low-pass → delay → gain → out.
#[derive(Ports)]
struct Patch {
    synths: usize,
    #[child]
    source: Child<MidiSource>,
    #[child(init = MidiFilter::new(None, None))]
    filter: Child<MidiFilter>,
    #[child]
    transpose: Child<Transpose>,
    voices: Vec<Child<TestSynth>>,
    #[child(init = LowPass::new(CONFIG))]
    low_pass: Child<LowPass>,
    #[child(init = Delay::new(CONFIG))]
    delay: Child<Delay>,
    #[child(init = Gain::new(CONFIG))]
    gain: Child<Gain>,
    #[child(init = AudioOut::new(CONFIG))]
    output: Child<AudioOut>,
}

impl Patch {
    fn new(synths: usize) -> Self {
        Self {
            synths,
            source: Child::default(),
            filter: Child::default(),
            transpose: Child::default(),
            voices: Vec::new(),
            low_pass: Child::default(),
            delay: Child::default(),
            gain: Child::default(),
            output: Child::default(),
        }
    }
}

impl Block for Patch {
    fn register(&mut self, r: &mut Registrar) {
        self.voices = (0..self.synths).map(|i| r.child(format!("synth{i}"), TestSynth::new(CONFIG))).collect();
    }
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.source].out, l[self.filter].input)?;
        l.connect(l[self.filter].out, l[self.transpose].input)?;
        for &synth in &self.voices {
            l.connect(l[self.transpose].out, l[synth].midi)?;
            l.connect(l[synth].out, l[self.low_pass].input)?;
        }
        l.connect(l[self.low_pass].out, l[self.delay].input)?;
        l.connect(l[self.delay].out, l[self.gain].input)?;
        l.connect(l[self.gain].out, l[self.output].input)
    }
}

#[test]
fn the_audio_path_never_allocates() {
    let mut engine = Engine::new();
    let id = engine.add("audio", Workstream::build_realtime(Patch::new(1)).unwrap());
    let root = engine.root::<Patch>(id).unwrap();
    let midi = engine.sender(engine.get(root.source).unwrap().input).unwrap();
    let output = engine.get(root.output).unwrap().input;
    let cutoff = engine.get(root.low_pass).unwrap().cutoff;
    let shift = engine.get(root.transpose).unwrap().semitones;
    let delay_time = engine.get(root.delay).unwrap().time;
    let mut driver = AudioDriver::new(engine.take(id).unwrap(), output, CONFIG);

    let channel = Channel::new(0).unwrap();
    // 480 frames: not a multiple of max_block, like a WASAPI period.
    let mut out = vec![0.0f32; 480 * 2];
    let period = Duration::from_secs_f64(480.0 / 48_000.0);
    let mut now = Timestamp::ZERO + Duration::from_secs(1);

    for i in 0..300u32 {
        // A chord every few buffers, released later; plus pitch bend.
        if i % 4 == 0 {
            for key in [60, 64, 67, 71] {
                let key = key + (i % 24) as u8;
                let on = MidiMessage::NoteOn { channel, key, velocity: 90 };
                midi.send(MidiEvent { time: now - period / 2, port: PortId(0), message: on });
                let off = MidiMessage::NoteOff { channel, key, velocity: 64 };
                midi.send(MidiEvent { time: now + period * 3, port: PortId(0), message: off });
            }
            let bend = MidiMessage::PitchBend { channel, value: (i as i16 * 50) % 8000 };
            midi.send(MidiEvent { time: now, port: PortId(0), message: bend });
        }
        // Control-side work (may allocate: it's not the audio thread).
        engine.set(cutoff, 200.0 + 50.0 * (i % 100) as f32).unwrap();
        engine.set(shift, (i % 13) as f32 - 6.0).unwrap();
        engine.set(delay_time, 0.05 + 0.005 * (i % 50) as f32).unwrap();
        if i % 60 == 30 {
            engine.rebuild(id, Patch::new(1 + (i as usize / 60) % 3)).unwrap();
        }

        assert_no_alloc(|| driver.render(&mut out, 2, now));
        now = now + period;
        engine.collect_garbage();
    }

    let stats = driver.stats().read();
    assert_eq!(stats.callbacks, 300);
    assert_eq!(stats.disabled_blocks, 0);
    assert!(out.iter().any(|s| *s != 0.0), "it made sound");
}
