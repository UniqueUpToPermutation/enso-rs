//! Plain data types shared by every enso crate.
//!
//! Nothing here touches a device, a thread or the GPU: these are the values
//! that flow *between* the crates (`enso-io` produces MIDI events,
//! `enso-audio` and the simulation consume them). Keeping them in one small
//! dependency-free crate means the producers and consumers agree on the types
//! without depending on each other.

pub mod midi;
pub mod time;

pub use midi::{Channel, MidiEvent, MidiMessage, PortId};
pub use time::{Clock, Timestamp};
