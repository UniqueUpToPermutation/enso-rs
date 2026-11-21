//! egui panels for enso.
//!
//! * [`PatchEditor`]: the audio patch as a node graph (egui-snarl), applied
//!   to the running audio workstream by rebuilding it (`enso_block::Engine`).
//! * [`patch`]: the patch as data ([`PatchSpec`]) and the block tree it
//!   describes ([`PatchRoot`]), independent of the GUI.

pub mod patch;
mod patch_editor;

pub use patch::{NodeKind, PatchRoot, PatchSpec};
pub use patch_editor::{MidiPort, PatchEditor, PatchNode};
