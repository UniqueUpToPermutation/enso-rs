//! The public interface, offline: `BlockAudioEngine::render` with chosen
//! timestamps instead of a sound card.

use std::time::Duration;

use enso_audio::{AudioConfig, AudioEngine, AudioError, AudioWorld, BlockAudioEngine, NodeDesc, NodeId, Signal};
use enso_core::{Channel, MidiEvent, MidiMessage, PortId, Timestamp};

const CONFIG: AudioConfig = AudioConfig { sample_rate: 48_000, max_block: 512 };
const ANY_MIDI: NodeDesc = NodeDesc::MidiIn { port: None, channel: None };

/// Renders `buffers` 10 ms buffers starting at buffer `first`; returns the
/// peak level.
fn peak(audio: &mut BlockAudioEngine, first: u64, buffers: u64) -> f32 {
    let mut out = vec![0.0; 480 * 2];
    let mut peak = 0.0f32;
    for i in first..first + buffers {
        audio.render(&mut out, 2, Timestamp::ZERO + Duration::from_millis(1000 + 10 * i));
        audio.collect_garbage();
        peak = out.iter().fold(peak, |p, s| p.max(s.abs()));
    }
    peak
}

fn note(channel: u8, key: u8) -> MidiEvent {
    let message = MidiMessage::NoteOn { channel: Channel::new(channel).unwrap(), key, velocity: 100 };
    MidiEvent { time: Timestamp::ZERO + Duration::from_millis(1000), port: PortId(3), message }
}

/// MIDI in → synth, plus a world wiring them to the speakers.
fn synth_patch() -> (BlockAudioEngine, NodeId, NodeId, AudioWorld) {
    let mut audio = BlockAudioEngine::new(CONFIG);
    let midi_in = audio.create_node(&ANY_MIDI).unwrap();
    let synth = audio.create_node(&NodeDesc::TestSynth).unwrap();
    let mut world = AudioWorld::new();
    world.connect(midi_in, synth);
    world.connect(synth, NodeId::SPEAKERS);
    (audio, midi_in, synth, world)
}

#[test]
fn a_synth_wired_to_the_speakers_plays() {
    let (mut audio, midi_in, synth, world) = synth_patch();
    audio.submit(&world).unwrap();
    audio.midi_sender().send(note(0, 60));
    assert!(peak(&mut audio, 0, 20) > 0.01);

    // Unwired from the speakers: silence.
    let mut unplugged = AudioWorld::new();
    unplugged.connect(midi_in, synth);
    audio.submit(&unplugged).unwrap();
    assert!(peak(&mut audio, 20, 5) < 1e-6);
}

#[test]
fn rewiring_keeps_notes_sounding() {
    let (mut audio, midi_in, synth, world) = synth_patch();
    audio.submit(&world).unwrap();
    audio.midi_sender().send(note(0, 60));
    peak(&mut audio, 0, 5);

    // Put a filter in between: a rebuild. The synth's voice survives it.
    let filter = audio.create_node(&NodeDesc::LowPass).unwrap();
    let mut filtered = AudioWorld::new();
    filtered.connect(midi_in, synth);
    filtered.connect(synth, filter);
    filtered.connect(filter, NodeId::SPEAKERS);
    audio.submit(&filtered).unwrap();
    assert!(peak(&mut audio, 5, 5) > 0.01, "the same note, through the filter");
}

#[test]
fn params_reach_nodes_and_persist() {
    let (mut audio, midi_in, synth, _) = synth_patch();
    let gain = audio.create_node(&NodeDesc::Gain).unwrap();
    let volume = audio.param::<f32>(gain, "db").unwrap();
    let mut world = AudioWorld::new();
    world.connect(midi_in, synth);
    world.connect(synth, gain);
    world.connect(gain, NodeId::SPEAKERS);
    world.set_param(gain, volume, -60.0);
    audio.submit(&world).unwrap();
    audio.midi_sender().send(note(0, 60));
    peak(&mut audio, 0, 20); // let the gain glide down
    assert!(peak(&mut audio, 20, 5) < 1e-3, "-60 dB silences it");

    // A world that doesn't mention the parameter keeps its value.
    let mut wiring_only = world.clone();
    wiring_only.clear();
    wiring_only.connect(midi_in, synth);
    wiring_only.connect(synth, gain);
    wiring_only.connect(gain, NodeId::SPEAKERS);
    audio.submit(&wiring_only).unwrap();
    assert!(peak(&mut audio, 25, 5) < 1e-3);
}

#[test]
fn a_bad_world_keeps_the_previous_wiring_playing() {
    let (mut audio, midi_in, synth, _) = synth_patch();
    let (a, b) = (audio.create_node(&NodeDesc::Gain).unwrap(), audio.create_node(&NodeDesc::Delay).unwrap());
    let mut good = AudioWorld::new();
    good.connect(midi_in, synth);
    good.connect(synth, a);
    good.connect(a, b);
    good.connect(b, NodeId::SPEAKERS);
    audio.submit(&good).unwrap();
    audio.midi_sender().send(note(0, 60));
    assert!(peak(&mut audio, 0, 10) > 0.01);

    // A cycle: refused, and refused again while the world is the same.
    let mut cycle = good.clone();
    cycle.connect(b, a);
    for _ in 0..2 {
        let err = audio.submit(&cycle).unwrap_err();
        assert!(matches!(&err, AudioError::Patch(why) if why.contains("cycle")), "{err}");
    }
    assert!(peak(&mut audio, 10, 5) > 0.01, "the good wiring still plays");

    // MIDI into an audio input.
    let mut mismatch = good.clone();
    mismatch.connect(midi_in, b);
    let err = audio.submit(&mismatch).unwrap_err();
    assert!(
        matches!(err, AudioError::SignalMismatch { from_signal: Signal::Midi, to_signal: Signal::Audio, .. }),
        "{err}"
    );

    // Back to good: fine again.
    audio.submit(&good).unwrap();
}

#[test]
fn destroyed_nodes_are_skipped() {
    let (mut audio, _, synth, world) = synth_patch();
    audio.submit(&world).unwrap();
    audio.midi_sender().send(note(0, 60));
    assert!(peak(&mut audio, 0, 10) > 0.01);

    audio.destroy_node(synth);
    audio.submit(&world).unwrap(); // still names the synth
    assert!(peak(&mut audio, 10, 5) < 1e-6);
    assert!(audio.node_params(synth).is_none());
}

#[test]
fn updating_a_node_changes_its_configuration() {
    let (mut audio, midi_in, _, world) = synth_patch();
    audio.update_node(midi_in, &NodeDesc::MidiIn { port: None, channel: Channel::new(1) }).unwrap();
    audio.submit(&world).unwrap();
    audio.midi_sender().send(note(0, 60));
    assert!(peak(&mut audio, 0, 10) < 1e-6, "channel 1 only now");
    audio.midi_sender().send(note(1, 64));
    assert!(peak(&mut audio, 10, 10) > 0.01);
}

#[test]
fn named_handles_are_checked() {
    let mut audio = BlockAudioEngine::new(CONFIG);
    let delay = audio.create_node(&NodeDesc::Delay).unwrap();
    assert!(audio.param::<f32>(delay, "feedback").is_ok());
    let err = audio.param::<f32>(delay, "cutof").unwrap_err();
    assert!(matches!(&err, AudioError::UnknownParam { available, .. } if available == "time, feedback, mix"), "{err}");

    assert_eq!(audio.output(delay, "out").unwrap().index, 0);
    assert!(matches!(audio.input(delay, "midi"), Err(AudioError::UnknownPin { .. })));
    assert!(matches!(audio.input(NodeId::SPEAKERS, "input"), Ok(pin) if pin.node == NodeId::SPEAKERS));

    audio.destroy_node(delay);
    assert!(matches!(audio.param::<f32>(delay, "mix"), Err(AudioError::UnknownNode(_))));
}
