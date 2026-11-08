//! [`Processor`]: the audio-thread half of the engine.
//!
//! Everything here runs inside the sound card's callback. Rules: no
//! allocation, no locks, no blocking, no logging. Every buffer was allocated
//! by the control thread ([`AudioEngine`](crate::AudioEngine)), and
//! everything that must be freed is sent back to it. `tests/no_alloc.rs`
//! enforces this.

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use enso_core::{MidiEvent, Timestamp};

use crate::{
    AudioConfig, CHANNELS, MidiQueue,
    node::{BlockEvent, Node, Process},
    patch::{MidiSource, Schedule, input_matches},
};

/// Node table size: the most nodes an engine can hold at once.
pub(crate) const MAX_NODES: usize = 1024;
/// MIDI events waiting for their time to come.
const PENDING_CAPACITY: usize = 4096;

/// Control thread → audio thread.
pub(crate) enum Command {
    Insert { slot: usize, node: Box<dyn Node> },
    Remove { slot: usize },
    Schedule(Box<Schedule>),
}

/// Audio thread → control thread: things to free off the audio thread.
/// Never read, only dropped (hence `dead_code`).
#[allow(dead_code)]
pub(crate) enum Garbage {
    Node(Box<dyn Node>),
    Schedule(Box<Schedule>),
    /// The payload of a node's panic.
    Panic(Box<dyn Any + Send>),
}

/// Written by the audio thread, read by the engine. Atomics are the one
/// kind of sharing that never blocks either side.
#[derive(Default)]
pub(crate) struct SharedStats {
    pub callbacks: AtomicU64,
    pub frames: AtomicU32,
    pub max_frames: AtomicU32,
    /// f32 bits. Non-negative floats order like their bit patterns, so
    /// `fetch_max` on the bits is a float max.
    pub load: AtomicU32,
    pub peak_load: AtomicU32,
    pub node_panics: AtomicU32,
    pub midi_dropped: AtomicU32,
}

/// The audio thread's half of the engine. Move it into the audio callback
/// (see [`OutputDevice::start`](crate::OutputDevice::start)), or call
/// [`process`](Self::process) yourself to render offline.
pub struct Processor {
    config: AudioConfig,
    nodes: Vec<Option<Box<dyn Node>>>,
    schedule: Box<Schedule>,
    commands: rtrb::Consumer<Command>,
    garbage: rtrb::Producer<Garbage>,
    midi: MidiQueue,
    /// Received MIDI not yet played, sorted by time.
    pending: Vec<MidiEvent>,
    /// External MIDI falling in the current block.
    block_events: Vec<BlockEvent>,
    stats: Arc<SharedStats>,
}

impl Processor {
    pub(crate) fn new(
        config: AudioConfig,
        commands: rtrb::Consumer<Command>,
        garbage: rtrb::Producer<Garbage>,
        midi: MidiQueue,
        stats: Arc<SharedStats>,
    ) -> Self {
        Self {
            config,
            nodes: (0..MAX_NODES).map(|_| None).collect(),
            schedule: Box::new(Schedule::empty(config)),
            commands,
            garbage,
            midi,
            pending: Vec::with_capacity(PENDING_CAPACITY),
            block_events: Vec::with_capacity(PENDING_CAPACITY),
            stats,
        }
    }

    pub fn config(&self) -> AudioConfig {
        self.config
    }

    /// Fill `output` with the next stretch of sound: interleaved frames of
    /// `channels` samples (the layout sound cards use). Stereo goes to the
    /// first two channels, extra channels are silent, mono gets the mix.
    ///
    /// `now` is the time this buffer is being computed (the device callback
    /// passes `clock.now()`). **Timing:** the buffer stands for the
    /// `frames / sample_rate` seconds just *before* `now`. A MIDI event
    /// lands at its exact position in that window, so the whole engine
    /// sounds one buffer late but jitter-free. That's better than playing
    /// every event at the start of the next buffer, which would quantise
    /// rhythm to the buffer size. Events stamped later than `now` wait.
    ///
    /// Output is made safe for speakers: NaN and infinity become silence,
    /// and samples are clipped to -1..=1.
    pub fn process(&mut self, output: &mut [f32], channels: usize, now: Timestamp) {
        let started = Instant::now();
        let channels = channels.max(1);
        let frames = output.len() / channels;
        let sample_rate = f64::from(self.config.sample_rate);

        self.apply_commands();
        self.receive_midi();

        let period = Duration::from_secs_f64(frames as f64 / sample_rate);
        let window_start = now - period;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.config.max_block);
            self.take_block_events(window_start, done, n, sample_rate);
            self.run(n);
            self.write_output(&mut output[done * channels..(done + n) * channels], channels, n);
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
    }

    /// Apply the engine's commands, as long as there's room to send back
    /// what they replace (otherwise wait for the next buffer).
    fn apply_commands(&mut self) {
        while self.garbage.slots() > 0 {
            let Ok(command) = self.commands.pop() else { break };
            let replaced = match command {
                Command::Insert { slot, node } => match self.nodes.get_mut(slot) {
                    Some(entry) => entry.replace(node).map(Garbage::Node),
                    None => Some(Garbage::Node(node)),
                },
                Command::Remove { slot } => self.nodes.get_mut(slot).and_then(Option::take).map(Garbage::Node),
                Command::Schedule(schedule) => {
                    Some(Garbage::Schedule(std::mem::replace(&mut self.schedule, schedule)))
                }
            };
            if let Some(garbage) = replaced {
                // Can't fail: we checked for a free slot.
                let _ = self.garbage.push(garbage);
            }
        }
    }

    /// Move newly arrived MIDI into `pending`, keeping it sorted by time.
    fn receive_midi(&mut self) {
        while let Some(event) = self.midi.pop() {
            if self.pending.len() == self.pending.capacity() {
                self.stats.midi_dropped.fetch_add(1, Relaxed);
                continue;
            }
            // Usually the newest: scan from the end.
            let at = self.pending.iter().rposition(|e| e.time <= event.time).map_or(0, |i| i + 1);
            self.pending.insert(at, event); // within capacity: no allocation
        }
    }

    /// Collect the pending events that fall in frames `start..start + n` of
    /// the window beginning at `window_start`.
    fn take_block_events(&mut self, window_start: Timestamp, start: usize, n: usize, sample_rate: f64) {
        self.block_events.clear();
        let end = start + n;
        let mut taken = 0;
        for event in &self.pending {
            let since = event.time.as_nanos() as i128 - window_start.as_nanos() as i128;
            // Late events (from before the window) play at once. Rounded to
            // the nearest frame: nanosecond stamps rarely fall exactly on one.
            let frame = if since <= 0 { 0 } else { (since as f64 * 1e-9 * sample_rate).round() as usize };
            if frame >= end {
                break; // sorted: everything after is later still
            }
            if self.block_events.len() < self.block_events.capacity() {
                self.block_events.push(BlockEvent {
                    offset: frame.saturating_sub(start) as u32,
                    port: event.port,
                    message: event.message,
                });
            }
            taken += 1;
        }
        self.pending.drain(..taken);
    }

    /// Run every step of the schedule for `n` frames.
    fn run(&mut self, n: usize) {
        let max_block = self.config.max_block;
        let block = CHANNELS * max_block;
        let sample_rate = self.config.sample_rate as f32;
        let Schedule { steps, outputs, events_out, mix, events_in, .. } = &mut *self.schedule;

        for (i, step) in steps.iter().enumerate() {
            // Steps only read from earlier steps (topological order), so
            // split the buffers into "earlier" (read) and "mine" (write).
            let (earlier, rest) = outputs.split_at_mut(i * block);
            let out = &mut rest[..block];
            let (earlier_events, rest_events) = events_out.split_at_mut(i);
            let my_events = &mut rest_events[0];

            // Sum the audio inputs.
            for c in 0..CHANNELS {
                mix[c * max_block..c * max_block + n].fill(0.0);
            }
            for &source in &step.audio_from {
                let source = &earlier[source * block..(source + 1) * block];
                for c in 0..CHANNELS {
                    let range = c * max_block..c * max_block + n;
                    for (m, s) in mix[range.clone()].iter_mut().zip(&source[range]) {
                        *m += s;
                    }
                }
            }

            // Merge the MIDI inputs, in time order.
            events_in.clear();
            for source in &step.midi_from {
                let events: &mut dyn Iterator<Item = &BlockEvent> = match source {
                    MidiSource::Input(filter) => &mut self.block_events.iter().filter(|e| input_matches(filter, e)),
                    MidiSource::Step(j) => &mut earlier_events[*j].as_slice().iter(),
                };
                for &event in events {
                    if events_in.len() < events_in.capacity() {
                        events_in.push(event);
                    }
                }
            }
            sort_by_offset(events_in);

            my_events.clear();
            for c in 0..CHANNELS {
                out[c * max_block..c * max_block + n].fill(0.0);
            }
            let Some(node) = self.nodes.get_mut(step.slot).and_then(Option::as_mut) else { continue };

            let (left, right) = out.split_at_mut(max_block);
            let process = Process {
                sample_rate,
                frames: n,
                input: [&mix[..n], &mix[max_block..max_block + n]],
                output: [&mut left[..n], &mut right[..n]],
                events: events_in,
                events_out: my_events,
            };
            // A bug in one node must not stop the show: catch the panic,
            // silence and retire the node. (Panicking allocates; that's
            // acceptable on this already-broken path.)
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| node.process(process))) {
                self.stats.node_panics.fetch_add(1, Relaxed);
                out.fill(0.0);
                my_events.clear();
                for garbage in [self.nodes[step.slot].take().map(Garbage::Node), Some(Garbage::Panic(payload))]
                    .into_iter()
                    .flatten()
                {
                    if let Err(rtrb::PushError::Full(garbage)) = self.garbage.push(garbage) {
                        // No room to send it back: leaking beats freeing here.
                        std::mem::forget(garbage);
                    }
                }
            }
        }
    }

    /// Sum the steps routed to the device into `out` (interleaved).
    fn write_output(&self, out: &mut [f32], channels: usize, n: usize) {
        let max_block = self.config.max_block;
        let block = CHANNELS * max_block;
        let schedule = &*self.schedule;
        for (f, frame) in out.chunks_exact_mut(channels).take(n).enumerate() {
            let (mut left, mut right) = (0.0, 0.0);
            for &step in &schedule.to_device {
                left += schedule.outputs[step * block + f];
                right += schedule.outputs[step * block + max_block + f];
            }
            if channels == 1 {
                frame[0] = speaker_safe(0.5 * (left + right));
            } else {
                frame[0] = speaker_safe(left);
                frame[1] = speaker_safe(right);
                frame[2..].fill(0.0);
            }
        }
    }
}

/// Silence for NaN/infinity, hard clip to -1..=1.
fn speaker_safe(sample: f32) -> f32 {
    if sample.is_finite() { sample.clamp(-1.0, 1.0) } else { 0.0 }
}

/// Stable insertion sort: events at the same offset keep their order (a
/// note-off then note-on of the same key must stay that way). The standard
/// library's stable sort may allocate; this never does, and blocks hold few
/// events.
fn sort_by_offset(events: &mut [BlockEvent]) {
    for i in 1..events.len() {
        let mut j = i;
        while j > 0 && events[j - 1].offset > events[j].offset {
            events.swap(j - 1, j);
            j -= 1;
        }
    }
}
