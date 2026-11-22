//! egui panels for enso.
//!
//! * [`PatchEditor`]: the audio patch as a node graph (egui-snarl), applied
//!   to any [`AudioEngine`](enso_audio::AudioEngine): it creates and
//!   destroys nodes and submits an `AudioWorld` describing the wiring and
//!   knob values.

mod patch_editor;

pub use patch_editor::{MidiPort, PatchEditor, PatchNode};
