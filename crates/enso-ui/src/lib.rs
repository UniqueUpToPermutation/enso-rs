//! egui panels for enso.
//!
//! * [`PatchEditor`]: the audio patch as a node graph (egui-snarl), applied
//!   to an [`enso_audio::AudioEngine`] live.

mod patch_editor;

pub use patch_editor::{MidiPort, NodeKind, PatchEditor, PatchNode};
