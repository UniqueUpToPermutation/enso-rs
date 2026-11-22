//! The audio thread must never allocate: allocation can take a lock inside
//! the allocator and stall for an unbounded time. This test installs an
//! allocator that **aborts** on any allocation inside `assert_no_alloc`, and
//! drives the engine through everything it does live: MIDI, chunking, every
//! node kind, parameter changes, and rewiring swapped in mid-performance.
//!
//! (It's its own test binary because the global allocator is per binary.)

use std::time::Duration;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use enso_audio::{AudioConfig, AudioEngine, AudioWorld, BlockAudioEngine, NodeDesc, NodeId};
use enso_core::{Channel, MidiEvent, MidiMessage, PortId, Timestamp};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const CONFIG: AudioConfig = AudioConfig { sample_rate: 48_000, max_block: 128 };

#[test]
fn the_audio_path_never_allocates() {
    let mut audio = BlockAudioEngine::new(CONFIG);
    let midi_in = audio.create_node(&NodeDesc::MidiIn { port: None, channel: None }).unwrap();
    let transpose = audio.create_node(&NodeDesc::Transpose).unwrap();
    let low_pass = audio.create_node(&NodeDesc::LowPass).unwrap();
    let delay = audio.create_node(&NodeDesc::Delay).unwrap();
    let gain = audio.create_node(&NodeDesc::Gain).unwrap();
    let mut synths: Vec<NodeId> = vec![audio.create_node(&NodeDesc::TestSynth).unwrap()];
    let cutoff = audio.param::<f32>(low_pass, "cutoff").unwrap();
    let shift = audio.param::<f32>(transpose, "semitones").unwrap();
    let delay_time = audio.param::<f32>(delay, "time").unwrap();
    let midi = audio.midi_sender();

    let channel = Channel::new(0).unwrap();
    // 480 frames: not a multiple of max_block, like a WASAPI period.
    let mut out = vec![0.0f32; 480 * 2];
    let period = Duration::from_secs_f64(480.0 / 48_000.0);
    let mut now = Timestamp::ZERO + Duration::from_secs(1);
    let mut world = AudioWorld::new();

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
        // Every 60 buffers, a different number of synths: a rebuild.
        if i % 60 == 30 {
            let wanted = 1 + (i as usize / 60) % 3;
            while synths.len() < wanted {
                synths.push(audio.create_node(&NodeDesc::TestSynth).unwrap());
            }
            while synths.len() > wanted {
                audio.destroy_node(synths.pop().unwrap());
            }
        }
        // midi in → transpose → synth(s) → low-pass → delay → gain → speakers.
        world.clear();
        world.connect(midi_in, transpose);
        for &synth in &synths {
            world.connect(transpose, synth);
            world.connect(synth, low_pass);
        }
        world.connect(low_pass, delay);
        world.connect(delay, gain);
        world.connect(gain, NodeId::SPEAKERS);
        world.set_param(low_pass, cutoff, 200.0 + 50.0 * (i % 100) as f32);
        world.set_param(transpose, shift, (i % 13) as f32 - 6.0);
        world.set_param(delay, delay_time, 0.05 + 0.005 * (i % 50) as f32);
        audio.submit(&world).unwrap();

        assert_no_alloc(|| audio.render(&mut out, 2, now));
        now = now + period;
    }

    let stats = audio.stats();
    assert_eq!(stats.callbacks, 300);
    assert_eq!(stats.disabled_blocks, 0);
    assert!(out.iter().any(|s| *s != 0.0), "it made sound");
}
