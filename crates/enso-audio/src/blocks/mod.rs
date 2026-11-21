//! The audio blocks. Each is an `enso_block::Block` that reads its step's
//! timing from `io.context::<AudioContext>()`.
//!
//! - [`MidiSource`] → [`MidiFilter`] / [`Transpose`]: MIDI in and around.
//! - [`TestSynth`]: MIDI → audio.
//! - [`LowPass`], [`Delay`], [`Gain`]: audio → audio, with knobs.
//! - [`AudioOut`]: where the device listens.

mod effects;
mod midi;
mod synth;

pub use effects::{Delay, Gain, LowPass, Transpose};
pub use midi::{MidiFilter, MidiSource};
pub use synth::TestSynth;

use enso_block::{Block, In, Ports};

use crate::{AudioBuf, AudioConfig};

/// The sink the device listens to: everything linked to its input is
/// summed. Expose it from the root and hand its input to the
/// [`AudioDriver`](crate::AudioDriver).
#[derive(Ports)]
pub struct AudioOut {
    config: AudioConfig,
    #[input(name = "in", default = AudioBuf::new(self.config), merge = Sum)]
    pub input: In<AudioBuf>,
}

impl AudioOut {
    pub fn new(config: AudioConfig) -> Self {
        Self { config, input: In::default() }
    }
}

impl Block for AudioOut {}

/// Glides towards a target instead of jumping. A parameter that jumps from
/// one step to the next makes an audible click ("zipper noise").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Smoothed {
    value: f32,
    coef: f32,
}

impl Smoothed {
    /// Starts at `value`; covers ~63% of a change every `seconds`.
    pub fn new(value: f32, seconds: f32, sample_rate: f32) -> Self {
        Self { value, coef: (-1.0 / (seconds * sample_rate).max(1.0)).exp() }
    }

    /// Advance one sample towards `target`.
    pub fn next(&mut self, target: f32) -> f32 {
        self.value = target + (self.value - target) * self.coef;
        self.value
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}
