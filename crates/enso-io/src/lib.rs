//! enso's connection to the outside world.
//!
//! * [`midi`]: MIDI inputs (timestamped, hot-pluggable) and outputs.
//!
//! Asset loading (images, samples) is planned here too; see
//! `docs/agents/architecture.md`.

pub mod midi;

pub use enso_core;
