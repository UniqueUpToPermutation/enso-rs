//! enso's audio engine.
//!
//! Audio is computed in small blocks on a real-time thread that the sound
//! card wakes every few milliseconds. If a block is late you hear a click,
//! so that thread **never allocates, locks, blocks or logs**. Everything is
//! split along that line:
//!
//! * [`AudioEngine`] lives on your (control) thread. It creates nodes,
//!   compiles [`Patch`]es (who connects to whom) into a ready-to-run
//!   schedule, and sends both to the audio thread through a lock-free queue.
//!   Old schedules and removed nodes are sent *back* and freed here, since
//!   freeing memory isn't allowed over there either.
//! * [`Processor`] lives on the audio thread. It applies those commands
//!   between blocks, turns timestamped MIDI into sample offsets, runs the
//!   nodes, and writes the device's buffer. [`Processor::process`] can also
//!   be called directly, without a sound card: that's how the tests render
//!   audio offline.
//! * [`OutputDevice`] / [`AudioStream`] connect a `Processor` to a sound card
//!   via cpal.
//!
//! ```no_run
//! use enso_audio::{AudioEngine, MidiFrom, OutputDevice, Patch, nodes::TestSynth};
//! use enso_core::Clock;
//!
//! let clock = Clock::new();
//! let device = OutputDevice::open_default()?;
//! let (mut engine, processor) = AudioEngine::new(device.audio_config());
//! let _stream = device.start(processor, clock)?; // sound until dropped
//!
//! let synth = engine.add_node(TestSynth::new(engine.config()))?;
//! let mut patch = Patch::new();
//! patch.midi(MidiFrom::inputs(), synth).output(synth);
//! engine.set_patch(patch)?; // rewire any time, while it plays
//! // Feed MIDI: hand `engine.midi_queue()` to enso-io's `MidiInputs`.
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod device;
mod engine;
mod node;
pub mod nodes;
mod param;
mod patch;
mod processor;

pub use device::{AudioStream, DeviceError, OutputDevice, output_device_names};
pub use engine::{AudioEngine, EngineError, MidiQueue, NodeId, Stats};
pub use node::{BlockEvent, CHANNELS, EventOutput, Node, Process};
pub use param::{SharedParam, Smoothed};
pub use patch::{MidiFilter, MidiFrom, Patch};
pub use processor::Processor;

/// Fixed for the lifetime of an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfig {
    /// Frames (samples per channel) per second, e.g. 48 000.
    pub sample_rate: u32,
    /// The most frames a node is asked to process at once. Device buffers
    /// larger than this are processed in several chunks, so every buffer
    /// can be allocated up front at this size.
    pub max_block: usize,
}

impl AudioConfig {
    /// 512 frames: ~10 ms at 48 kHz, the size of a typical WASAPI period.
    pub const DEFAULT_MAX_BLOCK: usize = 512;

    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate, max_block: Self::DEFAULT_MAX_BLOCK }
    }
}
