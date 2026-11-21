//! enso's audio engine, built on the block platform (`enso-block`; design:
//! `docs/design/blocks.md`).
//!
//! Audio is computed in small steps on a real-time thread that the sound
//! card wakes every few milliseconds. If a step is late you hear a click, so
//! that thread **never allocates, locks, blocks or logs**. The block
//! platform provides that split:
//!
//! * The patch is a tree of [`blocks`] in a real-time `Workstream`, built
//!   and rebuilt by an `enso_block::Engine` on your (control) thread:
//!   rewiring = `engine.rebuild(..)`; knobs = `engine.set(..)`; MIDI enters
//!   through a [`blocks::MidiSource`]'s inbox (`engine.sender(..)`).
//! * An [`AudioDriver`] steps that workstream from device buffers, handing
//!   every block an [`AudioContext`] (sample rate, frames, time window), and
//!   copies the [`blocks::AudioOut`]'s sound to the device.
//! * [`OutputDevice`] / [`AudioStream`] connect a driver to a sound card via
//!   cpal. `AudioDriver::render` can also be called directly, without a sound
//!   card: that's how the tests render offline.
//!
//! Audio ports carry [`AudioBuf`]s, MIDI ports carry [`MidiEvents`]
//! (events placed at frames).

mod buffer;
pub mod blocks;
mod context;
mod device;
mod driver;

pub use buffer::{AudioBuf, CHANNELS};
pub use context::{AudioConfig, AudioContext, BlockEvent, EVENT_CAPACITY, MidiEvents, midi_events};
pub use device::{AudioStream, DeviceError, OutputDevice, output_device_names};
pub use driver::{AudioDriver, Stats, StatsHandle};
pub use enso_block;
