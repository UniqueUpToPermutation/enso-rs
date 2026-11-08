//! The engine rendered offline: `Processor::process` called directly with
//! chosen timestamps, so timing is exact and no sound card is needed.

use std::time::Duration;

use enso_audio::{
    AudioConfig, AudioEngine, BlockEvent, EngineError, MidiFrom, Patch, Process, Processor,
    nodes::{FnNode, TestSynth},
};
use enso_core::{Channel, MidiEvent, MidiMessage, PortId, Timestamp};

const RATE: u32 = 48_000;
/// Frames per simulated device buffer.
const BUFFER: usize = 256;

/// A small `max_block`, so device buffers are processed in several chunks.
fn engine() -> (AudioEngine, Processor) {
    AudioEngine::new(AudioConfig { sample_rate: RATE, max_block: 64 })
}

/// Time of the first device buffer's end. Later buffers follow on.
const T0: Duration = Duration::from_secs(1);

fn buffer_end(index: u64) -> Timestamp {
    Timestamp::ZERO + T0 + frames(index * BUFFER as u64)
}

fn frames(n: u64) -> Duration {
    // Round up so the event lands *on* frame n, not just before it.
    Duration::from_nanos((n * 1_000_000_000).div_ceil(u64::from(RATE)))
}

/// The time of frame `frame` of buffer `index` (buffers cover the period
/// *before* their end time).
fn at(index: u64, frame: u64) -> Timestamp {
    buffer_end(index) - frames(BUFFER as u64) + frames(frame)
}

/// Render buffer `index` as interleaved stereo; returns the left channel.
fn render(processor: &mut Processor, index: u64) -> Vec<f32> {
    let mut out = vec![0.0; BUFFER * 2];
    processor.process(&mut out, 2, buffer_end(index));
    out.chunks(2).map(|f| f[0]).collect()
}

fn note_on(time: Timestamp, key: u8) -> MidiEvent {
    MidiEvent {
        time,
        port: PortId(0),
        message: MidiMessage::NoteOn { channel: Channel::new(0).unwrap(), key, velocity: 100 },
    }
}

fn note_off(time: Timestamp, key: u8) -> MidiEvent {
    MidiEvent {
        time,
        port: PortId(0),
        message: MidiMessage::NoteOff { channel: Channel::new(0).unwrap(), key, velocity: 64 },
    }
}

/// Writes 1.0 at the frame of every MIDI event it receives.
fn impulses() -> FnNode<impl FnMut(Process<'_>) + Send> {
    FnNode(|p: Process<'_>| {
        for event in p.events {
            p.output[0][event.offset as usize] = 1.0;
        }
    })
}

/// Outputs `value` on both channels.
fn constant(value: f32) -> FnNode<impl FnMut(Process<'_>) + Send> {
    FnNode(move |p: Process<'_>| {
        let [l, r] = p.output;
        l.fill(value);
        r.fill(value);
    })
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[test]
fn midi_lands_on_its_exact_frame() {
    let (mut engine, mut processor) = engine();
    let node = engine.add_node(impulses()).unwrap();
    let mut patch = Patch::new();
    patch.midi(MidiFrom::inputs(), node).output(node);
    engine.set_patch(patch).unwrap();

    let midi = engine.midi_queue();
    // Frame 100 is in the second 64-frame chunk: offsets survive chunking.
    midi.push(note_on(at(0, 100), 60)).unwrap();
    // Stamped after the first buffer's end: must wait for the second.
    midi.push(note_on(at(1, 10), 62)).unwrap();

    let first = render(&mut processor, 0);
    let hits: Vec<_> = first.iter().enumerate().filter(|(_, s)| **s != 0.0).map(|(i, _)| i).collect();
    assert_eq!(hits, vec![100]);

    let second = render(&mut processor, 1);
    let hits: Vec<_> = second.iter().enumerate().filter(|(_, s)| **s != 0.0).map(|(i, _)| i).collect();
    assert_eq!(hits, vec![10]);
}

#[test]
fn late_midi_plays_at_once() {
    let (mut engine, mut processor) = engine();
    let node = engine.add_node(impulses()).unwrap();
    let mut patch = Patch::new();
    patch.midi(MidiFrom::inputs(), node).output(node);
    engine.set_patch(patch).unwrap();

    // Stamped a whole buffer before this window.
    engine.midi_queue().push(note_on(at(0, 0) - frames(BUFFER as u64), 60)).unwrap();
    assert_eq!(render(&mut processor, 0)[0], 1.0);
}

#[test]
fn test_synth_plays_and_releases() {
    let (mut engine, mut processor) = engine();
    let synth = engine.add_node(TestSynth::new(engine.config())).unwrap();
    let mut patch = Patch::new();
    patch.midi(MidiFrom::inputs(), synth).output(synth);
    engine.set_patch(patch).unwrap();

    let midi = engine.midi_queue();
    midi.push(note_on(at(0, 0), 69)).unwrap();
    midi.push(note_off(at(20, 0), 69)).unwrap();

    let sounding = render(&mut processor, 1);
    assert!(rms(&sounding) > 0.02, "rms {}", rms(&sounding));
    assert!(sounding.iter().all(|s| s.abs() <= 1.0));

    // 3 s after the release starts, the voice has switched itself off.
    for i in 2..580 {
        render(&mut processor, i);
    }
    assert_eq!(rms(&render(&mut processor, 580)), 0.0);
}

#[test]
fn rewiring_keeps_node_state() {
    let (mut engine, mut processor) = engine();
    // Outputs (blocks processed so far) / 100: state that must survive
    // swaps. (Scaled: output is clipped to -1..=1.)
    let counter = engine
        .add_node(FnNode({
            let mut blocks = 0.0;
            move |p: Process<'_>| {
                blocks += 1.0;
                p.output[0].fill(blocks / 100.0);
            }
        }))
        .unwrap();
    let half = engine
        .add_node(FnNode(|p: Process<'_>| {
            for (o, i) in p.output[0].iter_mut().zip(p.input[0]) {
                *o = 0.5 * i;
            }
        }))
        .unwrap();

    let mut direct = Patch::new();
    direct.output(counter);
    engine.set_patch(direct).unwrap();
    // 256 frames in 64-frame chunks: 4 blocks.
    assert_eq!(*render(&mut processor, 0).last().unwrap(), 0.04);

    let mut through_half = Patch::new();
    through_half.audio(counter, half).output(half);
    engine.set_patch(through_half).unwrap();
    assert_eq!(*render(&mut processor, 1).last().unwrap(), 0.5 * 0.08);

    // Removing a node disconnects it; its ID is dead afterwards.
    engine.remove_node(half).unwrap();
    assert_eq!(render(&mut processor, 2), vec![0.0; BUFFER]);
    assert_eq!(engine.remove_node(half), Err(EngineError::UnknownNode(half)));
    let mut stale = Patch::new();
    stale.output(half);
    assert_eq!(engine.set_patch(stale), Err(EngineError::UnknownNode(half)));
}

#[test]
fn outputs_sum_and_cycles_are_rejected() {
    let (mut engine, mut processor) = engine();
    let a = engine.add_node(constant(0.25)).unwrap();
    let b = engine.add_node(constant(0.5)).unwrap();

    let mut both = Patch::new();
    both.output(a).output(b);
    engine.set_patch(both).unwrap();
    assert_eq!(render(&mut processor, 0)[0], 0.75);

    let mut cycle = Patch::new();
    cycle.audio(a, b).audio(b, a).output(b);
    assert_eq!(engine.set_patch(cycle), Err(EngineError::Cycle));
    // The old patch keeps playing.
    assert_eq!(render(&mut processor, 1)[0], 0.75);
}

#[test]
fn nodes_can_send_midi_to_nodes() {
    let (mut engine, mut processor) = engine();
    // Transposes every note an octave up, 5 frames later.
    let transpose = engine
        .add_node(FnNode(|p: Process<'_>| {
            for event in p.events {
                if let MidiMessage::NoteOn { channel, key, velocity } = event.message {
                    p.events_out.push(BlockEvent {
                        offset: (event.offset + 5).min(p.frames as u32 - 1),
                        port: event.port,
                        message: MidiMessage::NoteOn { channel, key: key + 12, velocity },
                    });
                }
            }
        }))
        .unwrap();
    let keys = engine
        .add_node(FnNode(|p: Process<'_>| {
            for event in p.events {
                if let MidiMessage::NoteOn { key, .. } = event.message {
                    p.output[0][event.offset as usize] = f32::from(key) / 128.0;
                }
            }
        }))
        .unwrap();
    let mut patch = Patch::new();
    // Listed in "wrong" order: the schedule must still run `transpose` first.
    patch.output(keys).midi(MidiFrom::Node(transpose), keys).midi(MidiFrom::inputs(), transpose);
    engine.set_patch(patch).unwrap();

    engine.midi_queue().push(note_on(at(0, 30), 60)).unwrap();
    assert_eq!(render(&mut processor, 0)[35], 72.0 / 128.0);
}

#[test]
fn midi_filters_select_port_and_channel() {
    let (mut engine, mut processor) = engine();
    let node = engine.add_node(impulses()).unwrap();
    let mut patch = Patch::new();
    patch.midi(MidiFrom::channel(Channel::new(1).unwrap()), node).output(node);
    engine.set_patch(patch).unwrap();

    let midi = engine.midi_queue();
    midi.push(note_on(at(0, 10), 60)).unwrap(); // channel 0: filtered out
    let mut on_channel_1 = note_on(at(0, 20), 60);
    on_channel_1.message = MidiMessage::NoteOn { channel: Channel::new(1).unwrap(), key: 60, velocity: 1 };
    midi.push(on_channel_1).unwrap();
    let out = render(&mut processor, 0);
    assert_eq!((out[10], out[20]), (0.0, 1.0));
}

#[test]
fn a_panicking_node_is_switched_off() {
    let (mut engine, mut processor) = engine();
    let bad = engine.add_node(FnNode(|_: Process<'_>| panic!("bug in a node"))).unwrap();
    let good = engine.add_node(constant(0.5)).unwrap();
    let mut patch = Patch::new();
    patch.output(bad).output(good);
    engine.set_patch(patch).unwrap();

    assert_eq!(render(&mut processor, 0)[0], 0.5);
    assert_eq!(render(&mut processor, 1)[0], 0.5);
    let stats = engine.stats();
    assert_eq!(stats.node_panics, 1, "panicked once, then stayed off");
    assert_eq!(stats.callbacks, 2);
}

#[test]
fn output_is_speaker_safe() {
    let (mut engine, mut processor) = engine();
    let wild = engine
        .add_node(FnNode(|p: Process<'_>| {
            let [l, r] = p.output;
            l.fill(f32::NAN);
            r.fill(10.0);
        }))
        .unwrap();
    let mut patch = Patch::new();
    patch.output(wild);
    engine.set_patch(patch).unwrap();

    let mut out = vec![0.0; BUFFER * 2];
    processor.process(&mut out, 2, buffer_end(0));
    assert!(out.chunks(2).all(|f| f == [0.0, 1.0]));
}

#[test]
fn mono_and_multichannel_devices() {
    let (mut engine, mut processor) = engine();
    let node = engine
        .add_node(FnNode(|p: Process<'_>| {
            let [l, r] = p.output;
            l.fill(0.2);
            r.fill(0.4);
        }))
        .unwrap();
    let mut patch = Patch::new();
    patch.output(node);
    engine.set_patch(patch).unwrap();

    let mut mono = vec![0.0; BUFFER];
    processor.process(&mut mono, 1, buffer_end(0));
    assert!(mono.iter().all(|s| (s - 0.3).abs() < 1e-6));

    let mut quad = vec![9.0; BUFFER * 4];
    processor.process(&mut quad, 4, buffer_end(1));
    assert!(quad.chunks(4).all(|f| f == [0.2, 0.4, 0.0, 0.0]));
}
