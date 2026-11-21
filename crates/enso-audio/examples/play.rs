//! Play the test synth from any MIDI input, on the default audio output.
//!
//! Run with:   cargo run -p enso-audio --example play
//!             cargo run -p enso-audio --example play -- <output name>   (e.g. "speakers")
//!
//! The patch is a block tree (MIDI source → synth → low-pass → output) in a
//! real-time workstream; the audio callback steps it. Plays a short arpeggio
//! at startup (scheduled ahead with timestamps), then whatever you play.
//! Every 4 seconds the filter's cutoff is moved from this thread, and every
//! 8 seconds the patch is rebuilt (a delay is added or removed) while
//! notes keep sounding. Ctrl+C quits.

use std::time::Duration;

use enso_audio::{
    AudioConfig, AudioDriver, OutputDevice,
    blocks::{AudioOut, Delay, LowPass, MidiSource, TestSynth},
    enso_block::{Block, Child, Engine, LinkError, Linker, Ports, Registrar, Workstream},
    output_device_names,
};
use enso_core::{Channel, Clock, MidiEvent, MidiMessage, PortId};
use enso_io::midi::{FnSink, MidiInputs};

/// MIDI → synth → low-pass → (delay) → output.
#[derive(Ports)]
struct Patch {
    config: AudioConfig,
    with_delay: bool,
    #[child]
    source: Child<MidiSource>,
    #[child(init = TestSynth::new(self.config))]
    synth: Child<TestSynth>,
    #[child(init = LowPass::new(self.config))]
    filter: Child<LowPass>,
    delay: Option<Child<Delay>>,
    #[child(init = AudioOut::new(self.config))]
    output: Child<AudioOut>,
}

impl Patch {
    fn new(config: AudioConfig, with_delay: bool) -> Self {
        Self {
            config,
            with_delay,
            source: Child::default(),
            synth: Child::default(),
            filter: Child::default(),
            delay: None,
            output: Child::default(),
        }
    }
}

impl Block for Patch {
    fn register(&mut self, r: &mut Registrar) {
        self.delay = self.with_delay.then(|| r.child("delay", Delay::new(self.config)));
    }
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.source].out, l[self.synth].midi)?;
        l.connect(l[self.synth].out, l[self.filter].input)?;
        match self.delay {
            Some(delay) => {
                l.connect(l[self.filter].out, l[delay].input)?;
                l.connect(l[delay].out, l[self.output].input)
            }
            None => l.connect(l[self.filter].out, l[self.output].input),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("audio outputs: {:?}", output_device_names());

    // One clock for MIDI stamps and the audio thread.
    let clock = Clock::new();
    let device = match std::env::args().nth(1) {
        Some(pattern) => OutputDevice::open(&pattern)?,
        None => OutputDevice::open_default()?,
    };
    let config = device.audio_config();

    // Build the patch, find the handles we need, move the stepping side to
    // the audio thread. The engine keeps the control side.
    let mut engine = Engine::new();
    let audio = engine.add("audio", Workstream::build_realtime(Patch::new(config, false))?);
    let root = engine.root::<Patch>(audio)?;
    let midi_inbox = engine.get(root.source)?.input;
    let output = engine.get(root.output)?.input;
    let cutoff = engine.get(root.filter)?.cutoff;
    let midi = engine.sender(midi_inbox)?;
    let driver = AudioDriver::new(engine.take(audio).expect("just added"), output, config);
    let stats = driver.stats();
    let stream = device.start(driver, clock)?;
    log::info!(
        "playing on {:?}: {} Hz, {} channels, ~{:?} frames per buffer",
        stream.name(),
        stream.sample_rate(),
        stream.channels(),
        stream.buffer_frames()
    );

    // Every MIDI input sends straight into the source's inbox.
    let mut inputs = MidiInputs::new(clock, FnSink({
        let midi = midi.clone();
        move |event: MidiEvent| midi.send(event)
    }));
    inputs.refresh();
    let names: Vec<_> = inputs.connected().iter().map(|i| i.name()).collect();
    log::info!("MIDI inputs: {names:?}");

    // A hello arpeggio, scheduled in the future: events wait in the
    // MidiSource until their timestamps come round.
    let channel = Channel::new(0).unwrap();
    let start = clock.now() + Duration::from_millis(300);
    for (i, key) in [60u8, 64, 67, 72, 76].into_iter().enumerate() {
        let on = start + Duration::from_millis(120 * i as u64);
        let note = |time, message| MidiEvent { time, port: PortId(u16::MAX), message };
        midi.send(note(on, MidiMessage::NoteOn { channel, key, velocity: 90 }));
        midi.send(note(on + Duration::from_millis(400), MidiMessage::NoteOff { channel, key, velocity: 64 }));
    }

    for second in 1u64.. {
        std::thread::sleep(Duration::from_secs(1));
        inputs.refresh(); // hot-plugged devices
        if second % 4 == 0 {
            let value = if second % 8 == 0 { 800.0 } else { 6000.0 };
            engine.set(cutoff, value)?;
            log::info!("cutoff → {value} Hz");
        }
        if second % 8 == 0 {
            let with_delay = second % 16 == 8;
            engine.rebuild(audio, Patch::new(config, with_delay))?;
            log::info!("rebuilt: delay {}", if with_delay { "added" } else { "removed" });
        }
        engine.collect_garbage();

        let s = stats.read();
        let buffer_ms = 1000.0 * s.frames as f32 / stream.sample_rate() as f32;
        log::info!(
            "buffer {} frames ({buffer_ms:.1} ms, max {}), load {:.2}% (peak {:.2}%), blocks off {}",
            s.frames,
            s.max_frames,
            s.load * 100.0,
            s.peak_load * 100.0,
            s.disabled_blocks
        );
    }
    Ok(())
}
