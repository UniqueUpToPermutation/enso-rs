//! Simple effects and MIDI processors. Their settings are knobs:
//! unconnected `In<f32>` inputs, set from outside with `Engine::set`, and
//! smoothed here so changes never click.

use enso_block::{Block, In, Io, Out, Ports};
use enso_core::MidiMessage;

use crate::{AudioBuf, AudioConfig, AudioContext, CHANNELS, MidiEvents, blocks::Smoothed, midi_events};

fn db_to_gain(db: f32) -> f32 {
    if db <= -60.0 { 0.0 } else { 10f32.powf(db / 20.0) }
}

// ---- gain ---------------------------------------------------------------------------

/// Volume, in decibels (-60 dB and below is silence).
#[derive(Ports)]
pub struct Gain {
    config: AudioConfig,
    #[input(name = "in", default = AudioBuf::new(self.config), merge = Sum)]
    pub input: In<AudioBuf>,
    #[input(default = Self::DEFAULT_DB)]
    pub db: In<f32>,
    #[output(initial = AudioBuf::new(self.config))]
    pub out: Out<AudioBuf>,
    #[state]
    gain: Smoothed,
}

impl Gain {
    pub const DEFAULT_DB: f32 = -6.0;

    pub fn new(config: AudioConfig) -> Self {
        let gain = Smoothed::new(db_to_gain(Self::DEFAULT_DB), 0.01, config.sample_rate as f32);
        Self { config, input: In::default(), db: In::default(), out: Out::default(), gain }
    }
}

impl Block for Gain {
    fn process(&mut self, io: &Io) {
        let Some(ctx) = io.context::<AudioContext>() else { return };
        let target = db_to_gain(*io.read(self.db));
        let input = io.read(self.input);
        let mut out = io.write(self.out);
        let [left, right] = out.write(ctx.frames);
        for f in 0..left.len() {
            let g = self.gain.next(target);
            let [l, r] = input.frame(f);
            left[f] = l * g;
            right[f] = r * g;
        }
    }
}

// ---- filter -------------------------------------------------------------------------

/// Resonant low-pass filter (a "topology-preserving transform" state
/// variable filter: stable however fast the cutoff moves). `cutoff` in Hz,
/// `resonance` 0..1.
#[derive(Ports)]
pub struct LowPass {
    config: AudioConfig,
    #[input(name = "in", default = AudioBuf::new(self.config), merge = Sum)]
    pub input: In<AudioBuf>,
    #[input(default = Self::DEFAULT_CUTOFF)]
    pub cutoff: In<f32>,
    #[input(default = Self::DEFAULT_RESONANCE)]
    pub resonance: In<f32>,
    #[output(initial = AudioBuf::new(self.config))]
    pub out: Out<AudioBuf>,
    #[state]
    smooth_cutoff: Smoothed,
    /// Two integrator states per channel.
    #[state]
    state: [[f32; 2]; CHANNELS],
}

impl LowPass {
    pub const DEFAULT_CUTOFF: f32 = 2_500.0;
    pub const DEFAULT_RESONANCE: f32 = 0.3;

    pub fn new(config: AudioConfig) -> Self {
        Self {
            config,
            input: In::default(),
            cutoff: In::default(),
            resonance: In::default(),
            out: Out::default(),
            smooth_cutoff: Smoothed::new(Self::DEFAULT_CUTOFF, 0.02, config.sample_rate as f32),
            state: [[0.0; 2]; CHANNELS],
        }
    }
}

impl Block for LowPass {
    fn process(&mut self, io: &Io) {
        let Some(ctx) = io.context::<AudioContext>() else { return };
        let sample_rate = ctx.sample_rate;
        let target = io.read(self.cutoff).clamp(20.0, 0.45 * sample_rate);
        // Damping: 2 (none) down to 0.05 (screaming).
        let k = 2.0 - 1.95 * io.read(self.resonance).clamp(0.0, 1.0);
        let input = io.read(self.input);
        let mut out = io.write(self.out);
        let [left, right] = out.write(ctx.frames);
        for f in 0..left.len() {
            let cutoff = self.smooth_cutoff.next(target);
            let g = (std::f32::consts::PI * cutoff / sample_rate).tan();
            let a1 = 1.0 / (1.0 + g * (g + k));
            let x = input.frame(f);
            for (c, out) in [(0, &mut *left), (1, &mut *right)] {
                let [ic1, ic2] = &mut self.state[c];
                let v3 = x[c] - *ic2;
                let v1 = a1 * *ic1 + g * a1 * v3;
                let v2 = *ic2 + g * v1;
                *ic1 = 2.0 * v1 - *ic1;
                *ic2 = 2.0 * v2 - *ic2;
                out[f] = v2;
            }
        }
    }
}

// ---- delay --------------------------------------------------------------------------

/// Stereo echo. `time` in seconds (up to [`Delay::MAX_SECONDS`]),
/// `feedback` and `mix` 0..1. The echo buffer survives rebuilds, so
/// rewiring around a delay doesn't cut its tail.
#[derive(Ports)]
pub struct Delay {
    config: AudioConfig,
    #[input(name = "in", default = AudioBuf::new(self.config), merge = Sum)]
    pub input: In<AudioBuf>,
    #[input(default = Self::DEFAULT_TIME)]
    pub time: In<f32>,
    #[input(default = Self::DEFAULT_FEEDBACK)]
    pub feedback: In<f32>,
    #[input(default = Self::DEFAULT_MIX)]
    pub mix: In<f32>,
    #[output(initial = AudioBuf::new(self.config))]
    pub out: Out<AudioBuf>,
    #[state]
    smooth_time: Smoothed,
    /// One ring buffer per channel, allocated in `new`, never resized.
    #[state]
    lines: [Vec<f32>; CHANNELS],
    #[state]
    write: usize,
}

impl Delay {
    pub const MAX_SECONDS: f32 = 2.0;
    pub const DEFAULT_TIME: f32 = 0.35;
    pub const DEFAULT_FEEDBACK: f32 = 0.4;
    pub const DEFAULT_MIX: f32 = 0.3;

    pub fn new(config: AudioConfig) -> Self {
        let sample_rate = config.sample_rate as f32;
        let len = (Self::MAX_SECONDS * sample_rate) as usize + 2;
        Self {
            config,
            input: In::default(),
            time: In::default(),
            feedback: In::default(),
            mix: In::default(),
            out: Out::default(),
            // Slow smoothing: changing the time then sweeps the pitch like tape.
            smooth_time: Smoothed::new(Self::DEFAULT_TIME, 0.15, sample_rate),
            lines: [vec![0.0; len], vec![0.0; len]],
            write: 0,
        }
    }
}

impl Block for Delay {
    fn process(&mut self, io: &Io) {
        let Some(ctx) = io.context::<AudioContext>() else { return };
        let sample_rate = ctx.sample_rate;
        let target = io.read(self.time).clamp(0.001, Self::MAX_SECONDS);
        let feedback = io.read(self.feedback).clamp(0.0, 0.95);
        let mix = io.read(self.mix).clamp(0.0, 1.0);
        let input = io.read(self.input);
        let mut out = io.write(self.out);
        let [left, right] = out.write(ctx.frames);
        let len = self.lines[0].len();
        for f in 0..left.len() {
            let delay = self.smooth_time.next(target) * sample_rate;
            // Read between two samples (linear interpolation) so the time can glide.
            let read = (self.write as f32 - delay).rem_euclid(len as f32);
            let (i, frac) = (read as usize % len, read.fract());
            let x = input.frame(f);
            for (c, out) in [(0, &mut *left), (1, &mut *right)] {
                let line = &mut self.lines[c];
                let delayed = line[i] + (line[(i + 1) % len] - line[i]) * frac;
                line[self.write] = x[c] + delayed * feedback;
                out[f] = x[c] * (1.0 - mix) + delayed * mix;
            }
            self.write = (self.write + 1) % len;
        }
    }
}

// ---- transpose (MIDI) ---------------------------------------------------------------

/// Shifts notes by `semitones` (rounded). Remembers each sounding note's
/// shift, so a note-off releases the note that was actually started even if
/// the knob moved in between (and across rebuilds).
#[derive(Ports)]
pub struct Transpose {
    #[input(name = "in", default = midi_events(), merge = Append)]
    pub input: In<MidiEvents>,
    #[input(default = 0.0)]
    pub semitones: In<f32>,
    #[output(initial = midi_events())]
    pub out: Out<MidiEvents>,
    /// Per channel and key: the shifted key now sounding, if any.
    #[state]
    sounding: [[Option<u8>; 128]; 16],
}

impl Transpose {
    pub fn new() -> Self {
        Self { input: In::default(), semitones: In::default(), out: Out::default(), sounding: [[None; 128]; 16] }
    }
}

impl Default for Transpose {
    fn default() -> Self {
        Self::new()
    }
}

impl Block for Transpose {
    fn process(&mut self, io: &Io) {
        let shift = io.read(self.semitones).round() as i32;
        let input = io.read(self.input);
        let mut out = io.write(self.out);
        out.clear();
        for event in input.as_slice() {
            let message = match event.message {
                MidiMessage::NoteOn { channel, key, velocity } => {
                    let Some(shifted) = u8::try_from(i32::from(key) + shift).ok().filter(|k| *k <= 127) else {
                        continue;
                    };
                    self.sounding[usize::from(channel.index())][usize::from(key)] = Some(shifted);
                    MidiMessage::NoteOn { channel, key: shifted, velocity }
                }
                MidiMessage::NoteOff { channel, key, velocity } => {
                    let entry = &mut self.sounding[usize::from(channel.index())][usize::from(key)];
                    let Some(shifted) = entry.take() else { continue };
                    MidiMessage::NoteOff { channel, key: shifted, velocity }
                }
                other => other,
            };
            out.push(crate::BlockEvent { message, ..*event });
        }
    }
}
