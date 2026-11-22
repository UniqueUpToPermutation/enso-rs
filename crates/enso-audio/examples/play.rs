//! Play the test synth from any MIDI input, on the default audio output.
//!
//! Run with:   cargo run -p enso-audio --example play
//!             cargo run -p enso-audio --example play -- <output name>   (e.g. "speakers")
//!
//! MIDI in → synth → low-pass → speakers, through the `AudioEngine`
//! interface. Plays a short arpeggio at startup (scheduled ahead with
//! timestamps), then whatever you play. Every 4 seconds the filter's
//! cutoff moves, and every 8 seconds a delay is wired in or out while notes
//! keep sounding. Ctrl+C quits.

use std::time::Duration;

use enso_audio::{AudioEngine, AudioWorld, BlockAudioEngine, NodeDesc, NodeId, OutputDevice, output_device_names};
use enso_core::{Channel, Clock, MidiEvent, MidiMessage, PortId};
use enso_io::midi::{FnSink, MidiInputs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("audio outputs: {:?}", output_device_names());

    // One clock for MIDI stamps and the audio thread.
    let clock = Clock::new();
    let device = match std::env::args().nth(1) {
        Some(pattern) => OutputDevice::open(&pattern)?,
        None => OutputDevice::open_default()?,
    };

    // Resources first: the nodes, and a handle to the parameter we'll move.
    let mut audio = BlockAudioEngine::new(device.audio_config());
    let midi_in = audio.create_node(&NodeDesc::MidiIn { port: None, channel: None })?;
    let synth = audio.create_node(&NodeDesc::TestSynth)?;
    let filter = audio.create_node(&NodeDesc::LowPass)?;
    let delay = audio.create_node(&NodeDesc::Delay)?;
    let cutoff = audio.param::<f32>(filter, "cutoff")?;

    let stream = audio.start(device, clock)?;
    log::info!(
        "playing on {:?}: {} Hz, {} channels, ~{:?} frames per buffer",
        stream.name(),
        stream.sample_rate(),
        stream.channels(),
        stream.buffer_frames()
    );

    // Every MIDI input sends straight to the audio thread.
    let midi = audio.midi_sender();
    let mut inputs = MidiInputs::new(clock, FnSink({
        let midi = midi.clone();
        move |event: MidiEvent| midi.send(event)
    }));
    inputs.refresh();
    let names: Vec<_> = inputs.connected().iter().map(|i| i.name()).collect();
    log::info!("MIDI inputs: {names:?}");

    // A hello arpeggio, scheduled in the future: events wait on the audio
    // thread until their timestamps come round.
    let channel = Channel::new(0).unwrap();
    let start = clock.now() + Duration::from_millis(300);
    for (i, key) in [60u8, 64, 67, 72, 76].into_iter().enumerate() {
        let on = start + Duration::from_millis(120 * i as u64);
        let note = |time, message| MidiEvent { time, port: PortId(u16::MAX), message };
        midi.send(note(on, MidiMessage::NoteOn { channel, key, velocity: 90 }));
        midi.send(note(on + Duration::from_millis(400), MidiMessage::NoteOff { channel, key, velocity: 64 }));
    }

    // The "simulation": describe the patch each tick (here, once a second).
    let mut world = AudioWorld::new();
    for second in 0u64..u64::MAX {
        let with_delay = second % 16 >= 8;
        let value = if second % 8 >= 4 { 800.0 } else { 6000.0 };

        world.clear();
        world.connect(midi_in, synth);
        world.connect(synth, filter);
        if with_delay {
            world.connect(filter, delay);
            world.connect(delay, NodeId::SPEAKERS);
        } else {
            world.connect(filter, NodeId::SPEAKERS);
        }
        world.set_param(filter, cutoff, value);
        if let Err(err) = audio.submit(&world) {
            log::error!("{err}");
        }
        if second % 4 == 0 {
            log::info!("cutoff {value} Hz, delay {}", if with_delay { "on" } else { "off" });
        }

        let s = audio.stats();
        let buffer_ms = 1000.0 * s.frames as f32 / stream.sample_rate() as f32;
        log::info!(
            "buffer {} frames ({buffer_ms:.1} ms, max {}), load {:.2}% (peak {:.2}%), blocks off {}",
            s.frames,
            s.max_frames,
            s.load * 100.0,
            s.peak_load * 100.0,
            s.disabled_blocks
        );

        std::thread::sleep(Duration::from_secs(1));
        inputs.refresh(); // hot-plugged devices
    }
    Ok(())
}
