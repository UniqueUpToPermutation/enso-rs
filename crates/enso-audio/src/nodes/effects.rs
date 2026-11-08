//! Simple effects and MIDI processors, controlled through [`SharedParam`]s.

use enso_core::MidiMessage;

use crate::{AudioConfig, BlockEvent, CHANNELS, Node, Process, SharedParam, param::Smoothed};

fn db_to_gain(db: f32) -> f32 {
    if db <= -60.0 { 0.0 } else { 10f32.powf(db / 20.0) }
}

// ---- gain ---------------------------------------------------------------------------

/// Volume, in decibels (-60 dB and below is silence).
pub struct Gain {
    db: SharedParam,
    gain: Smoothed,
}

impl Gain {
    pub fn new(config: AudioConfig, db: SharedParam) -> Self {
        let gain = Smoothed::new(db_to_gain(db.get()), 0.01, config.sample_rate as f32);
        Self { db, gain }
    }
}

impl Node for Gain {
    fn process(&mut self, p: Process<'_>) {
        let target = db_to_gain(self.db.get());
        let [left, right] = p.output;
        for f in 0..p.frames {
            let g = self.gain.next(target);
            left[f] = p.input[0][f] * g;
            right[f] = p.input[1][f] * g;
        }
    }
}

// ---- filter -------------------------------------------------------------------------

/// Resonant low-pass filter (a "topology-preserving transform" state
/// variable filter: stable however fast the cutoff moves).
pub struct LowPass {
    cutoff: SharedParam,
    resonance: SharedParam,
    smooth_cutoff: Smoothed,
    sample_rate: f32,
    /// Two integrator states per channel.
    state: [[f32; 2]; CHANNELS],
}

impl LowPass {
    /// `cutoff` in Hz; `resonance` 0..1.
    pub fn new(config: AudioConfig, cutoff: SharedParam, resonance: SharedParam) -> Self {
        let sample_rate = config.sample_rate as f32;
        Self {
            smooth_cutoff: Smoothed::new(cutoff.get(), 0.02, sample_rate),
            cutoff,
            resonance,
            sample_rate,
            state: [[0.0; 2]; CHANNELS],
        }
    }
}

impl Node for LowPass {
    fn process(&mut self, p: Process<'_>) {
        let target = self.cutoff.get().clamp(20.0, 0.45 * self.sample_rate);
        // Damping: 2 (none) down to 0.05 (screaming).
        let k = 2.0 - 1.95 * self.resonance.get().clamp(0.0, 1.0);
        let [left, right] = p.output;
        for f in 0..p.frames {
            let cutoff = self.smooth_cutoff.next(target);
            let g = (std::f32::consts::PI * cutoff / self.sample_rate).tan();
            let a1 = 1.0 / (1.0 + g * (g + k));
            for (c, out) in [(0, &mut *left), (1, &mut *right)] {
                let [ic1, ic2] = &mut self.state[c];
                let v3 = p.input[c][f] - *ic2;
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
/// `feedback` and `mix` 0..1.
pub struct Delay {
    time: SharedParam,
    feedback: SharedParam,
    mix: SharedParam,
    smooth_time: Smoothed,
    sample_rate: f32,
    /// One ring buffer per channel, allocated here, never resized.
    lines: [Vec<f32>; CHANNELS],
    write: usize,
}

impl Delay {
    pub const MAX_SECONDS: f32 = 2.0;

    pub fn new(config: AudioConfig, time: SharedParam, feedback: SharedParam, mix: SharedParam) -> Self {
        let sample_rate = config.sample_rate as f32;
        let len = (Self::MAX_SECONDS * sample_rate) as usize + 2;
        Self {
            // Slow smoothing: changing the time then sweeps the pitch like tape.
            smooth_time: Smoothed::new(time.get(), 0.15, sample_rate),
            time,
            feedback,
            mix,
            sample_rate,
            lines: [vec![0.0; len], vec![0.0; len]],
            write: 0,
        }
    }
}

impl Node for Delay {
    fn process(&mut self, p: Process<'_>) {
        let target = self.time.get().clamp(0.001, Self::MAX_SECONDS);
        let feedback = self.feedback.get().clamp(0.0, 0.95);
        let mix = self.mix.get().clamp(0.0, 1.0);
        let len = self.lines[0].len();
        let [left, right] = p.output;
        for f in 0..p.frames {
            let delay = self.smooth_time.next(target) * self.sample_rate;
            // Read between two samples (linear interpolation) so the time can glide.
            let read = (self.write as f32 - delay).rem_euclid(len as f32);
            let (i, frac) = (read as usize % len, read.fract());
            for (c, out) in [(0, &mut *left), (1, &mut *right)] {
                let line = &mut self.lines[c];
                let delayed = line[i] + (line[(i + 1) % len] - line[i]) * frac;
                let dry = p.input[c][f];
                line[self.write] = dry + delayed * feedback;
                out[f] = dry * (1.0 - mix) + delayed * mix;
            }
            self.write = (self.write + 1) % len;
        }
    }
}

// ---- transpose (MIDI) ---------------------------------------------------------------

/// Shifts notes by `semitones` (rounded). Remembers each sounding note's
/// shift, so a note-off releases the note that was actually started even if
/// the knob moved in between.
pub struct Transpose {
    semitones: SharedParam,
    /// Per channel and key: the shifted key now sounding, if any.
    sounding: [[Option<u8>; 128]; 16],
}

impl Transpose {
    pub fn new(semitones: SharedParam) -> Self {
        Self { semitones, sounding: [[None; 128]; 16] }
    }
}

impl Node for Transpose {
    fn process(&mut self, p: Process<'_>) {
        let shift = self.semitones.get().round() as i32;
        for event in p.events {
            let message = match event.message {
                MidiMessage::NoteOn { channel, key, velocity } => {
                    let Ok(shifted) = u8::try_from(i32::from(key) + shift) else { continue };
                    if shifted > 127 {
                        continue;
                    }
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
            p.events_out.push(BlockEvent { message, ..*event });
        }
    }
}
