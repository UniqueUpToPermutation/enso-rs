//! [`AudioDriver`]: turns device buffers into workstream steps.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use enso_block::{In, Workstream};
use enso_core::Timestamp;

use crate::{AudioBuf, AudioConfig, AudioContext, CHANNELS};

/// Written by the audio thread, read anywhere. Atomics are the one kind of
/// sharing that never blocks either side.
#[derive(Default)]
struct SharedStats {
    callbacks: AtomicU64,
    frames: AtomicU32,
    max_frames: AtomicU32,
    /// f32 bits. Non-negative floats order like their bit patterns, so
    /// `fetch_max` on the bits is a float max.
    load: AtomicU32,
    peak_load: AtomicU32,
    disabled_blocks: AtomicU32,
}

/// What the audio thread has been doing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    /// Device buffers processed so far.
    pub callbacks: u64,
    /// Frames in the latest buffer, and the most seen in one buffer.
    pub frames: u32,
    pub max_frames: u32,
    /// Time spent computing the latest buffer, as a fraction of the time it
    /// lasts. Must stay well below 1.0, or you hear dropouts.
    pub load: f32,
    /// Highest `load` since the previous [`StatsHandle::read`].
    pub peak_load: f32,
    /// Blocks switched off because they panicked.
    pub disabled_blocks: u32,
}

/// Reads an [`AudioDriver`]'s statistics from any thread. Clone it freely.
#[derive(Clone)]
pub struct StatsHandle(Arc<SharedStats>);

impl StatsHandle {
    /// The latest statistics (resets `peak_load`).
    pub fn read(&self) -> Stats {
        let s = &self.0;
        Stats {
            callbacks: s.callbacks.load(Relaxed),
            frames: s.frames.load(Relaxed),
            max_frames: s.max_frames.load(Relaxed),
            load: f32::from_bits(s.load.load(Relaxed)),
            peak_load: f32::from_bits(s.peak_load.swap(0, Relaxed)),
            disabled_blocks: s.disabled_blocks.load(Relaxed),
        }
    }
}

/// Runs an audio [`Workstream`] from device buffers.
///
/// Each device buffer is cut into steps of at most `max_block` frames.
/// Every step gets an [`AudioContext`] (the time it stands for); then the
/// sound arriving at the `output` input (an
/// [`AudioOut`](crate::blocks::AudioOut) exposed by the root) is copied to
/// the device, made speaker-safe (NaN/infinity → silence, clipped to ±1).
///
/// **Timing:** a buffer stands for the `frames / sample_rate` seconds just
/// *before* `now`, so MIDI lands at its exact frame and the engine sounds
/// one buffer late without jitter, rather than quantising rhythm to buffer
/// boundaries.
///
/// Everything here runs on the audio thread: no allocation (enforced by
/// `tests/no_alloc.rs`). Rebuilds, knob settings and mailbox links arrive
/// through the workstream's queue (from the `Engine` that controls it).
pub struct AudioDriver {
    workstream: Workstream,
    output: In<AudioBuf>,
    config: AudioConfig,
    stats: Arc<SharedStats>,
}

impl AudioDriver {
    /// `output`: the input of the root's [`AudioOut`](crate::blocks::AudioOut).
    pub fn new(workstream: Workstream, output: In<AudioBuf>, config: AudioConfig) -> Self {
        Self { workstream, output, config, stats: Arc::new(SharedStats::default()) }
    }

    pub fn config(&self) -> AudioConfig {
        self.config
    }

    /// For reading statistics from other threads.
    pub fn stats(&self) -> StatsHandle {
        StatsHandle(self.stats.clone())
    }

    pub fn workstream(&self) -> &Workstream {
        &self.workstream
    }

    pub fn workstream_mut(&mut self) -> &mut Workstream {
        &mut self.workstream
    }

    /// Fill `output` (interleaved, `channels` samples per frame, as sound
    /// cards use) with the stretch of time ending at `now`. Stereo goes to
    /// the first two channels, extra channels are silent, mono gets the
    /// mix. Also how tests render offline, with chosen timestamps.
    pub fn render(&mut self, output: &mut [f32], channels: usize, now: Timestamp) {
        let started = Instant::now();
        let channels = channels.max(1);
        let frames = output.len() / channels;
        let rate = f64::from(self.config.sample_rate);
        let period = Duration::from_secs_f64(frames as f64 / rate);
        let window_start = now - period;

        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.config.max_block);
            let start = window_start + Duration::from_secs_f64(done as f64 / rate);
            let context = AudioContext {
                sample_rate: self.config.sample_rate as f32,
                frames: n,
                start,
                end: start + Duration::from_secs_f64(n as f64 / rate),
            };
            self.workstream.step_with(&context);
            self.copy_out(&mut output[done * channels..(done + n) * channels], channels, n);
            done += n;
        }
        // A trailing partial frame (malformed buffer) gets silence.
        output[frames * channels..].fill(0.0);

        let stats = &self.stats;
        let load = if frames > 0 { started.elapsed().as_secs_f32() / period.as_secs_f32() } else { 0.0 };
        stats.callbacks.fetch_add(1, Relaxed);
        stats.frames.store(frames as u32, Relaxed);
        stats.max_frames.fetch_max(frames as u32, Relaxed);
        stats.load.store(load.to_bits(), Relaxed);
        stats.peak_load.fetch_max(load.to_bits(), Relaxed);
        stats.disabled_blocks.store(self.workstream.disabled().count() as u32, Relaxed);
    }

    /// The output's sound → `out` (interleaved). Silence if the output is
    /// missing (e.g. removed by a rebuild).
    fn copy_out(&self, out: &mut [f32], channels: usize, n: usize) {
        let Some(sound) = self.workstream.peek(self.output) else {
            out.fill(0.0);
            return;
        };
        let (left, right) = (sound.channel(0), sound.channel(1));
        for (f, frame) in out.chunks_exact_mut(channels).take(n).enumerate() {
            let (l, r) = (left[f], right[f]);
            if channels == 1 {
                frame[0] = speaker_safe(0.5 * (l + r));
            } else {
                frame[0] = speaker_safe(l);
                frame[1] = speaker_safe(r);
                frame[CHANNELS..].fill(0.0);
            }
        }
    }
}

/// Silence for NaN/infinity, hard clip to -1..=1.
fn speaker_safe(sample: f32) -> f32 {
    if sample.is_finite() { sample.clamp(-1.0, 1.0) } else { 0.0 }
}
