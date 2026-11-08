//! Play the test synth from any MIDI input, on the default audio output.
//!
//! Run with:   cargo run -p enso-audio --example play
//!             cargo run -p enso-audio --example play -- <output name>   (e.g. "speakers")
//!
//! Plays a short arpeggio at startup (scheduled ahead of time with
//! timestamps), then whatever you play. Prints the audio thread's stats every
//! second. Ctrl+C quits.

use std::time::Duration;

use enso_audio::{AudioEngine, MidiFrom, OutputDevice, Patch, nodes::TestSynth, output_device_names};
use enso_core::{Channel, Clock, MidiEvent, MidiMessage, PortId};
use enso_io::midi::MidiInputs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("audio outputs: {:?}", output_device_names());

    // One clock for MIDI stamps and the audio thread.
    let clock = Clock::new();

    let device = match std::env::args().nth(1) {
        Some(pattern) => OutputDevice::open(&pattern)?,
        None => OutputDevice::open_default()?,
    };
    let (mut engine, processor) = AudioEngine::new(device.audio_config());
    let stream = device.start(processor, clock)?;
    log::info!(
        "playing on {:?}: {} Hz, {} channels, ~{:?} frames per buffer",
        stream.name(),
        stream.sample_rate(),
        stream.channels(),
        stream.buffer_frames()
    );

    let synth = engine.add_node(TestSynth::new(engine.config()))?;
    let mut patch = Patch::new();
    patch.midi(MidiFrom::inputs(), synth).output(synth);
    engine.set_patch(patch)?;

    // Every MIDI input feeds the engine's lock-free queue directly.
    let mut inputs = MidiInputs::new(clock, engine.midi_queue());
    inputs.refresh();
    let names: Vec<_> = inputs.connected().iter().map(|i| i.name()).collect();
    log::info!("MIDI inputs: {names:?}");

    // A hello arpeggio, scheduled in the future: events wait in the engine
    // until their timestamps come round.
    let channel = Channel::new(0).unwrap();
    let start = clock.now() + Duration::from_millis(300);
    for (i, key) in [60u8, 64, 67, 72, 76].into_iter().enumerate() {
        let on = start + Duration::from_millis(120 * i as u64);
        let off = on + Duration::from_millis(400);
        let note = |time, message| MidiEvent { time, port: PortId(u16::MAX), message };
        let queue = engine.midi_queue();
        let _ = queue.push(note(on, MidiMessage::NoteOn { channel, key, velocity: 90 }));
        let _ = queue.push(note(off, MidiMessage::NoteOff { channel, key, velocity: 64 }));
    }

    loop {
        std::thread::sleep(Duration::from_secs(1));
        inputs.refresh(); // hot-plugged devices
        let stats = engine.stats(); // also frees what the audio thread sent back
        let buffer_ms = 1000.0 * stats.frames as f32 / stream.sample_rate() as f32;
        log::info!(
            "buffer {} frames ({buffer_ms:.1} ms, max {}), load {:.2}% (peak {:.2}%), panics {}, MIDI dropped {}",
            stats.frames,
            stats.max_frames,
            stats.load * 100.0,
            stats.peak_load * 100.0,
            stats.node_panics,
            stats.midi_dropped
        );
    }
}
