//! [`AudioBuf`]: one step's worth of stereo audio, the value carried by
//! audio ports.

use enso_block::{Merge, MergeMode};

use crate::AudioConfig;

/// Every audio connection is stereo: two planar channels (left, right).
/// Planar (one slice per channel) rather than interleaved, because that's
/// what DSP code and plugin APIs (VST3, CLAP) work with.
pub const CHANNELS: usize = 2;

/// A block of stereo audio, `max_block` frames long, of which the first
/// [`frames`](Self::frames) are this step's sound.
///
/// Invariant: samples past `frames` are zero, so reading a buffer that was
/// written with fewer frames (or never) gives silence, not stale sound.
///
/// Audio inputs that should mix several sources are declared with
/// `merge = Sum` (decision 15: audio inputs default to a single link).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuf {
    channels: [Vec<f32>; CHANNELS],
    frames: usize,
}

impl AudioBuf {
    /// Silence, with room for `config.max_block` frames (allocated here,
    /// off the audio thread).
    pub fn new(config: AudioConfig) -> Self {
        Self { channels: [vec![0.0; config.max_block], vec![0.0; config.max_block]], frames: 0 }
    }

    /// Frames written this step.
    #[allow(dead_code)] // part of the block-author API; tests use it
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// One channel, full length (silent past [`frames`](Self::frames)).
    pub fn channel(&self, channel: usize) -> &[f32] {
        &self.channels[channel]
    }

    /// `[left, right]` at frame `i` (silence past the end).
    pub fn frame(&self, i: usize) -> [f32; CHANNELS] {
        [self.channels[0].get(i).copied().unwrap_or(0.0), self.channels[1].get(i).copied().unwrap_or(0.0)]
    }

    /// Start writing `frames` frames: returns `[left, right]`, each exactly
    /// `frames` long, for the caller to fill completely. (Their contents are
    /// whatever was there before.) Never allocates; `frames` is clamped to
    /// the buffer's size.
    pub fn write(&mut self, frames: usize) -> [&mut [f32]; CHANNELS] {
        let frames = frames.min(self.channels[0].len());
        if frames < self.frames {
            for channel in &mut self.channels {
                channel[frames..self.frames].fill(0.0);
            }
        }
        self.frames = frames;
        let [left, right] = &mut self.channels;
        [&mut left[..frames], &mut right[..frames]]
    }
}

/// Audio sums (for inputs declared `merge = Sum`).
impl Merge for AudioBuf {
    fn supports(mode: MergeMode) -> bool {
        matches!(mode, MergeMode::Single | MergeMode::Sum)
    }

    fn clear(&mut self) {
        let frames = self.frames;
        for channel in &mut self.channels {
            channel[..frames].fill(0.0);
        }
        self.frames = 0;
    }

    fn merge(&mut self, other: &Self, _mode: MergeMode) {
        let frames = other.frames.min(self.channels[0].len());
        for (into, from) in self.channels.iter_mut().zip(&other.channels) {
            for (a, b) in into[..frames].iter_mut().zip(&from[..frames]) {
                *a += b;
            }
        }
        self.frames = self.frames.max(frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AudioConfig {
        AudioConfig { sample_rate: 48_000, max_block: 8 }
    }

    #[test]
    fn shorter_writes_leave_silence_behind() {
        let mut buf = AudioBuf::new(config());
        for channel in buf.write(6) {
            channel.fill(1.0);
        }
        buf.write(2)[0].fill(0.5);
        assert_eq!(buf.frames(), 2);
        assert_eq!(buf.channel(0), &[0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn sums() {
        let (mut a, mut b) = (AudioBuf::new(config()), AudioBuf::new(config()));
        a.write(2)[0].fill(1.0);
        b.write(4)[0].fill(0.25);
        a.clear();
        a.merge(&b, MergeMode::Sum);
        a.merge(&b, MergeMode::Sum);
        assert_eq!(a.frames(), 4);
        assert_eq!(&a.channel(0)[..5], &[0.5, 0.5, 0.5, 0.5, 0.0]);
    }
}
