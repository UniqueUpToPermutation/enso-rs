//! [`TestSynth`]: a small polyphonic synth, to hear the engine before
//! hosting plugins.

use std::f32::consts::TAU;

use enso_core::{Channel, MidiMessage, midi::key_frequency};

use crate::{AudioConfig, BlockEvent, Node, Process};

const VOICES: usize = 16;
/// Per-voice level at full velocity; leaves headroom for chords.
const VOICE_GAIN: f32 = 0.15;
/// Pitch bend range, in semitones.
const BEND_RANGE: f32 = 2.0;

/// A soft organ-ish tone (a sine plus two quiet harmonics) with an
/// attack/decay/sustain/release envelope. Responds to note on/off, velocity,
/// the sustain pedal (CC 64) and pitch bend. 16 voices; when all are busy,
/// the oldest is stolen.
pub struct TestSynth {
    voices: [Voice; VOICES],
    sample_rate: f32,
    /// Counts note-ons, to find the oldest voice.
    clock: u64,
    sustain: [bool; 16],
    bend: [f32; 16],
    env: Envelope,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    channel: Channel,
    key: u8,
    started: u64,
    stage: Stage,
    level: f32,
    velocity: f32,
    /// 0..1, the fundamental's phase.
    phase: f32,
    /// Key released while the sustain pedal was down.
    held: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Off,
    Attack,
    /// Decay, then sustain (the level settles at `SUSTAIN`).
    Decay,
    Release,
}

/// Per-sample envelope coefficients.
struct Envelope {
    attack_step: f32,
    decay_coef: f32,
    release_coef: f32,
}

const SUSTAIN: f32 = 0.55;

impl Envelope {
    fn new(sample_rate: f32) -> Self {
        // One-pole smoothing: covers ~63% of the remaining distance per time constant.
        let coef = |seconds: f32| (-1.0 / (seconds * sample_rate)).exp();
        Self { attack_step: 1.0 / (0.004 * sample_rate), decay_coef: coef(0.35), release_coef: coef(0.25) }
    }
}

impl TestSynth {
    pub fn new(config: AudioConfig) -> Self {
        let sample_rate = config.sample_rate as f32;
        Self {
            voices: [Voice::default(); VOICES],
            sample_rate,
            clock: 0,
            sustain: [false; 16],
            bend: [1.0; 16],
            env: Envelope::new(sample_rate),
        }
    }

    fn handle(&mut self, event: &BlockEvent) {
        match event.message {
            MidiMessage::NoteOn { channel, key, velocity } => self.note_on(channel, key, velocity),
            MidiMessage::NoteOff { channel, key, .. } => {
                let pedal = self.sustain[usize::from(channel.index())];
                for voice in self.voices.iter_mut().filter(|v| v.sounding(channel, key)) {
                    if pedal {
                        voice.held = true;
                    } else {
                        voice.stage = Stage::Release;
                    }
                }
            }
            MidiMessage::ControlChange { channel, controller: 64, value } => {
                let down = value >= 64;
                self.sustain[usize::from(channel.index())] = down;
                if !down {
                    for voice in self.voices.iter_mut().filter(|v| v.held && v.channel == channel) {
                        voice.held = false;
                        voice.stage = Stage::Release;
                    }
                }
            }
            MidiMessage::PitchBend { channel, value } => {
                let semitones = f32::from(value) / 8192.0 * BEND_RANGE;
                self.bend[usize::from(channel.index())] = 2f32.powf(semitones / 12.0);
            }
            _ => {}
        }
    }

    fn note_on(&mut self, channel: Channel, key: u8, velocity: u8) {
        // Re-strike a voice already playing this key, else a free one, else steal the oldest.
        let index = self
            .voices
            .iter()
            .position(|v| v.stage != Stage::Off && v.channel == channel && v.key == key)
            .or_else(|| self.voices.iter().position(|v| v.stage == Stage::Off))
            .unwrap_or_else(|| {
                (0..VOICES).min_by_key(|&i| self.voices[i].started).expect("VOICES > 0")
            });
        self.clock += 1;
        let voice = &mut self.voices[index];
        let restrike = voice.stage != Stage::Off && voice.key == key;
        *voice = Voice {
            channel,
            key,
            started: self.clock,
            stage: Stage::Attack,
            // Keep level and phase when re-striking: no click.
            level: if restrike { voice.level } else { 0.0 },
            phase: if restrike { voice.phase } else { 0.0 },
            velocity: (f32::from(velocity) / 127.0).powf(1.5),
            held: false,
        };
    }
}

impl Voice {
    fn sounding(&self, channel: Channel, key: u8) -> bool {
        matches!(self.stage, Stage::Attack | Stage::Decay) && self.channel == channel && self.key == key
    }
}

impl Node for TestSynth {
    fn process(&mut self, p: Process<'_>) {
        let [left, right] = p.output;
        let mut events = p.events.iter().peekable();
        for frame in 0..p.frames {
            // Sample-accurate: apply each event at its own frame.
            while let Some(event) = events.next_if(|e| e.offset as usize <= frame) {
                self.handle(event);
            }

            let mut sample = 0.0;
            for voice in self.voices.iter_mut().filter(|v| v.stage != Stage::Off) {
                match voice.stage {
                    Stage::Attack => {
                        voice.level += self.env.attack_step;
                        if voice.level >= 1.0 {
                            voice.level = 1.0;
                            voice.stage = Stage::Decay;
                        }
                    }
                    Stage::Decay => voice.level = SUSTAIN + (voice.level - SUSTAIN) * self.env.decay_coef,
                    Stage::Release => {
                        voice.level *= self.env.release_coef;
                        if voice.level < 1e-4 {
                            voice.stage = Stage::Off;
                            continue;
                        }
                    }
                    Stage::Off => {}
                }

                let frequency = key_frequency(voice.key) * self.bend[usize::from(voice.channel.index())];
                voice.phase = (voice.phase + frequency / self.sample_rate).fract();
                let x = voice.phase * TAU;
                let tone = x.sin() + 0.3 * (2.0 * x).sin() + 0.12 * (3.0 * x).sin();
                sample += tone * voice.level * voice.velocity * VOICE_GAIN;
            }
            left[frame] = sample;
            right[frame] = sample;
        }
    }
}
