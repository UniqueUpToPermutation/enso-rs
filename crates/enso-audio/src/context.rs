//! What audio blocks know about each step, and MIDI placed within it.

use std::time::Duration;

use enso_block::{Events, Stamped};
use enso_core::{MidiMessage, PortId, Timestamp};

/// Fixed for the lifetime of an audio workstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfig {
    /// Frames (samples per channel) per second, e.g. 48 000.
    pub sample_rate: u32,
    /// The most frames one step processes. Device buffers larger than this
    /// are processed in several steps, so every buffer can be allocated up
    /// front at this size.
    pub max_block: usize,
}

impl AudioConfig {
    /// 512 frames: ~10 ms at 48 kHz, the size of a typical WASAPI period.
    pub const DEFAULT_MAX_BLOCK: usize = 512;

    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate, max_block: Self::DEFAULT_MAX_BLOCK }
    }
}

/// Handed to every block of an audio workstream each step
/// (`io.context::<AudioContext>()`).
///
/// **Timing:** a step stands for the stretch of time `start..end`. A MIDI
/// event stamped in that stretch plays at its exact frame. The driver makes
/// each device buffer stand for the time just *before* the callback, so the
/// engine sounds one buffer late but jitter-free (see
/// [`AudioDriver`](crate::AudioDriver)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioContext {
    pub sample_rate: f32,
    /// Frames in this step (`1..=max_block`).
    pub frames: usize,
    pub start: Timestamp,
    pub end: Timestamp,
}

impl AudioContext {
    /// The frame at which `time` falls in this step, rounded to the nearest
    /// frame. Late events (before `start`) play at frame 0; `None` for
    /// events at or after `end` (they belong to a later step).
    pub fn frame_of(&self, time: Timestamp) -> Option<usize> {
        if time >= self.end {
            return None;
        }
        let since = time.duration_since(self.start).as_secs_f64();
        Some(((since * f64::from(self.sample_rate)).round() as usize).min(self.frames.saturating_sub(1)))
    }

    /// The time of frame `frame` of this step.
    #[allow(dead_code)] // part of the block-author API; tests use it
    pub fn time_of(&self, frame: usize) -> Timestamp {
        self.start + Duration::from_secs_f64(frame as f64 / f64::from(self.sample_rate))
    }
}

/// A MIDI message placed in a step, as it flows between audio blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEvent {
    /// When it happens (keeps merged streams in order).
    pub time: Timestamp,
    /// The frame within the step, `0..frames`.
    pub offset: u32,
    /// The input device it came from.
    pub port: PortId,
    pub message: MidiMessage,
}

impl Stamped for BlockEvent {
    fn timestamp(&self) -> Timestamp {
        self.time
    }
}

/// One step's MIDI, in time order: what MIDI ports carry.
pub type MidiEvents = Events<BlockEvent>;

/// Events one port can carry per step (a step is ~10 ms).
pub const EVENT_CAPACITY: usize = 512;

/// An empty event list with room for [`EVENT_CAPACITY`] events, allocated
/// here, off the audio thread. Pushing beyond it drops events instead of
/// allocating.
pub fn midi_events() -> MidiEvents {
    Events::with_capacity(EVENT_CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_of_times() {
        let start = Timestamp::from_secs_f64(1.0);
        let ctx = AudioContext {
            sample_rate: 1000.0,
            frames: 10,
            start,
            end: start + Duration::from_millis(10),
        };
        assert_eq!(ctx.frame_of(start), Some(0));
        assert_eq!(ctx.frame_of(start + Duration::from_micros(3_400)), Some(3));
        assert_eq!(ctx.frame_of(start + Duration::from_micros(3_600)), Some(4), "rounds to nearest");
        assert_eq!(ctx.frame_of(start - Duration::from_millis(5)), Some(0), "late: at once");
        assert_eq!(ctx.frame_of(ctx.end), None, "next step's");
        assert_eq!(ctx.time_of(5), start + Duration::from_millis(5));
    }
}
