//! [`AudioEngine`]: the control-thread half of the engine.

use std::sync::{Arc, atomic::Ordering::Relaxed};

use crossbeam_queue::ArrayQueue;
use enso_core::MidiEvent;

use crate::{
    AudioConfig, Node, Patch, Processor,
    processor::{Command, Garbage, MAX_NODES, SharedStats},
};

/// MIDI into the engine: a lock-free queue that any number of threads can
/// push to, and the audio thread pops from. enso-io's `MidiInputs` accepts
/// it as a sink, and you can push events yourself (stamped in the future to
/// schedule them).
pub type MidiQueue = Arc<ArrayQueue<MidiEvent>>;

const COMMAND_CAPACITY: usize = 256;
const MIDI_CAPACITY: usize = 1024;

/// A node in an engine. Stays valid until [`AudioEngine::remove_node`]; a
/// stale ID is rejected rather than reaching whichever node reused its slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    slot: u32,
    generation: u32,
}

impl NodeId {
    pub(crate) fn slot(self) -> usize {
        self.slot as usize
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("the engine holds the maximum number of nodes")]
    TooManyNodes,
    #[error("the audio thread isn't keeping up with commands (is the stream running?)")]
    QueueFull,
    #[error("{0:?} isn't (or is no longer) in this engine")]
    UnknownNode(NodeId),
    #[error("the patch has a cycle: a node would feed its own input")]
    Cycle,
}

/// What the audio thread has been doing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    /// Buffers processed so far.
    pub callbacks: u64,
    /// Frames in the latest buffer, and the most seen in one buffer.
    pub frames: u32,
    pub max_frames: u32,
    /// Time spent computing the latest buffer, as a fraction of the time it
    /// lasts. Must stay well below 1.0, or you hear dropouts.
    pub load: f32,
    /// Highest `load` since the previous [`AudioEngine::stats`] call.
    pub peak_load: f32,
    /// Nodes switched off because they panicked.
    pub node_panics: u32,
    /// MIDI events dropped because the engine was flooded.
    pub midi_dropped: u32,
}

/// The control side of the engine: owns the patch, creates and removes
/// nodes, and frees what the audio thread is done with. All methods are
/// quick and never wait for the audio thread.
pub struct AudioEngine {
    config: AudioConfig,
    commands: rtrb::Producer<Command>,
    garbage: rtrb::Consumer<Garbage>,
    midi: MidiQueue,
    stats: Arc<SharedStats>,
    /// Per slot: generation, and whether a node lives there.
    slots: Vec<(u32, bool)>,
    patch: Patch,
}

impl AudioEngine {
    /// Create an engine and its audio-thread half. Start the [`Processor`]
    /// on a device ([`OutputDevice::start`](crate::OutputDevice::start)),
    /// or drive it yourself for offline rendering.
    pub fn new(config: AudioConfig) -> (Self, Processor) {
        let (commands, command_rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        // Every command returns at most one piece of garbage; extra room
        // for panicking nodes.
        let (garbage_tx, garbage) = rtrb::RingBuffer::new(COMMAND_CAPACITY + 64);
        let midi = Arc::new(ArrayQueue::new(MIDI_CAPACITY));
        let stats = Arc::new(SharedStats::default());
        let processor = Processor::new(config, command_rx, garbage_tx, midi.clone(), stats.clone());
        let engine = Self {
            config,
            commands,
            garbage,
            midi,
            stats,
            slots: vec![(0, false); MAX_NODES],
            patch: Patch::new(),
        };
        (engine, processor)
    }

    pub fn config(&self) -> AudioConfig {
        self.config
    }

    /// Where MIDI enters the engine. Clone it freely.
    pub fn midi_queue(&self) -> MidiQueue {
        self.midi.clone()
    }

    /// Hand `node` to the audio thread. It runs once a patch mentions it.
    pub fn add_node(&mut self, node: impl Node) -> Result<NodeId, EngineError> {
        self.collect_garbage();
        let slot = self.slots.iter().position(|&(_, used)| !used).ok_or(EngineError::TooManyNodes)?;
        self.send(Command::Insert { slot, node: Box::new(node) })?;
        let (generation, used) = &mut self.slots[slot];
        *used = true;
        Ok(NodeId { slot: slot as u32, generation: *generation })
    }

    /// Disconnect `id` from the patch and remove it. It's freed here, on
    /// this thread, once the audio thread lets go of it.
    pub fn remove_node(&mut self, id: NodeId) -> Result<(), EngineError> {
        self.check(id)?;
        if self.patch.nodes().contains(&id) {
            let mut patch = self.patch.clone();
            patch.disconnect(id);
            self.set_patch(patch)?;
        }
        self.send(Command::Remove { slot: id.slot() })?;
        let (generation, used) = &mut self.slots[id.slot()];
        *used = false;
        *generation = generation.wrapping_add(1);
        Ok(())
    }

    /// Rewire: compile `patch` here and swap it in on the audio thread
    /// between two buffers. Nodes keep their state across the swap.
    pub fn set_patch(&mut self, patch: Patch) -> Result<(), EngineError> {
        self.collect_garbage();
        for id in patch.nodes() {
            self.check(id)?;
        }
        let schedule = patch.compile(self.config)?;
        self.send(Command::Schedule(Box::new(schedule)))?;
        self.patch = patch;
        Ok(())
    }

    /// The patch most recently set.
    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    /// Read the audio thread's statistics (and reset `peak_load`). Also
    /// frees what the audio thread sent back, so calling this regularly
    /// (e.g. once per frame) keeps the engine tidy.
    pub fn stats(&mut self) -> Stats {
        self.collect_garbage();
        let s = &self.stats;
        Stats {
            callbacks: s.callbacks.load(Relaxed),
            frames: s.frames.load(Relaxed),
            max_frames: s.max_frames.load(Relaxed),
            load: f32::from_bits(s.load.load(Relaxed)),
            peak_load: f32::from_bits(s.peak_load.swap(0, Relaxed)),
            node_panics: s.node_panics.load(Relaxed),
            midi_dropped: s.midi_dropped.load(Relaxed),
        }
    }

    /// Free whatever the audio thread is done with (old schedules, removed
    /// nodes). Every method calls this; call it yourself if you go a long
    /// time without calling any. If garbage piles up, the audio thread
    /// postpones commands until there's room, so nothing breaks.
    pub fn collect_garbage(&mut self) {
        while let Ok(garbage) = self.garbage.pop() {
            drop(garbage);
        }
    }

    fn check(&self, id: NodeId) -> Result<(), EngineError> {
        match self.slots.get(id.slot()) {
            Some(&(generation, true)) if generation == id.generation => Ok(()),
            _ => Err(EngineError::UnknownNode(id)),
        }
    }

    fn send(&mut self, command: Command) -> Result<(), EngineError> {
        self.commands.push(command).map_err(|_| EngineError::QueueFull)
    }
}
