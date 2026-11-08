//! The audio thread must never allocate: allocation can take a lock inside
//! the allocator and stall for an unbounded time. This test installs an
//! allocator that **aborts** on any allocation inside `assert_no_alloc`, and
//! drives the processor through everything it does live: MIDI, chunking,
//! patch swaps, node inserts and removals, MIDI between nodes.
//!
//! (It's its own test binary because the global allocator is per binary.)

use std::time::Duration;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use enso_audio::{
    AudioConfig, AudioEngine, BlockEvent, MidiFrom, Patch, Process,
    nodes::{FnNode, TestSynth},
};
use enso_core::{Channel, MidiEvent, MidiMessage, PortId, Timestamp};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

#[test]
fn the_audio_path_never_allocates() {
    let config = AudioConfig { sample_rate: 48_000, max_block: 128 };
    let (mut engine, mut processor) = AudioEngine::new(config);
    let synth = engine.add_node(TestSynth::new(config)).unwrap();
    let echo = engine
        .add_node(FnNode(|p: Process<'_>| {
            for event in p.events {
                p.events_out.push(BlockEvent { offset: event.offset, ..*event });
            }
        }))
        .unwrap();
    let mut patch = Patch::new();
    patch.midi(MidiFrom::inputs(), echo).midi(MidiFrom::Node(echo), synth).output(synth);
    engine.set_patch(patch.clone()).unwrap();

    let midi = engine.midi_queue();
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
                let message = MidiMessage::NoteOn { channel, key, velocity: 90 };
                midi.push(MidiEvent { time: now - period / 2, port: PortId(0), message }).unwrap();
                let off = MidiMessage::NoteOff { channel, key, velocity: 64 };
                midi.push(MidiEvent { time: now + period * 3, port: PortId(0), message: off }).unwrap();
            }
            let bend = MidiMessage::PitchBend { channel, value: (i as i16 * 50) % 8000 };
            midi.push(MidiEvent { time: now, port: PortId(0), message: bend }).unwrap();
        }
        // Control-thread work, interleaved (it may allocate: not on the audio thread).
        match i {
            100 => engine.set_patch(patch.clone()).unwrap(),
            150 => {
                let extra = engine.add_node(TestSynth::new(config)).unwrap();
                let mut bigger = patch.clone();
                bigger.midi(MidiFrom::inputs(), extra).output(extra);
                engine.set_patch(bigger).unwrap();
            }
            _ => {}
        }

        assert_no_alloc(|| processor.process(&mut out, 2, now));
        now = now + period;
        engine.collect_garbage();
    }

    let stats = engine.stats();
    assert_eq!(stats.callbacks, 300);
    assert_eq!(stats.node_panics, 0);
    assert_eq!(stats.midi_dropped, 0);
}
