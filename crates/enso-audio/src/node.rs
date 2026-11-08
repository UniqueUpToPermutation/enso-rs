//! [`Node`]: one unit of sound processing (a synth, an effect, later a
//! hosted plugin or a MIDI processor).

use enso_core::{MidiMessage, PortId};

/// Every audio connection is stereo: two planar channels (left, right).
/// Planar (one slice per channel) rather than interleaved, because that's
/// what DSP code and plugin APIs (VST3, CLAP) work with.
pub const CHANNELS: usize = 2;

/// A MIDI message placed inside the current block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEvent {
    /// Frame within the block at which it happens, `0..frames`.
    pub offset: u32,
    /// The input it came from (for events from other nodes, the port of
    /// the event they were derived from, or `PortId::default()`).
    pub port: PortId,
    pub message: MidiMessage,
}

/// A node's MIDI output for one block. Fixed capacity: [`push`](Self::push)
/// never allocates, and drops the event if the buffer is full.
#[derive(Debug)]
pub struct EventOutput {
    events: Vec<BlockEvent>,
}

impl EventOutput {
    /// Events per node per block. Generous: a block is ~10 ms.
    pub const CAPACITY: usize = 512;

    pub(crate) fn new() -> Self {
        Self { events: Vec::with_capacity(Self::CAPACITY) }
    }

    /// Emit an event. Keep offsets non-decreasing. Returns `false` (and
    /// drops the event) when full.
    pub fn push(&mut self, event: BlockEvent) -> bool {
        if self.events.len() < self.events.capacity() {
            self.events.push(event);
            true
        } else {
            false
        }
    }

    pub(crate) fn clear(&mut self) {
        self.events.clear();
    }

    pub(crate) fn as_slice(&self) -> &[BlockEvent] {
        &self.events
    }
}

/// Everything a node gets for one block.
pub struct Process<'a> {
    pub sample_rate: f32,
    /// Frames in this block (`1..=max_block`). All slices have this length.
    pub frames: usize,
    /// The sum of everything connected to this node's audio input.
    pub input: [&'a [f32]; CHANNELS],
    /// Zeroed before the call; the node adds or writes its sound here.
    pub output: [&'a mut [f32]; CHANNELS],
    /// MIDI for this block, sorted by `offset`.
    pub events: &'a [BlockEvent],
    /// MIDI this node emits, for nodes connected with [`MidiFrom::Node`](crate::MidiFrom::Node).
    pub events_out: &'a mut EventOutput,
}

/// A unit of processing, run on the audio thread.
///
/// Build it (and allocate everything it needs) on the control thread, then
/// hand it to [`AudioEngine::add_node`](crate::AudioEngine::add_node). In
/// `process`: **no allocation, no locks, no I/O, no logging**. A panic is
/// caught: the node is switched off and the rest keeps playing.
pub trait Node: Send + 'static {
    fn process(&mut self, process: Process<'_>);
}
