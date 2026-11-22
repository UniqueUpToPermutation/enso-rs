//! # enso-audio
//!
//! The audio engine.
//!
//! ```text
//!   application (sim tick, patch editor)          audio thread (sound card callback)
//!   ────────────────────────────────────          ──────────────────────────────────
//!   create_node / update / destroy ──┐
//!   AudioWorld (wires, params) ──────┼─▶ AudioEngine::submit ──▶ swapped in between two steps
//!                                    │                           (nodes keep their state)
//!   MIDI threads ──▶ MidiSender ─────┴───────────────────────▶ played at its exact frame
//! ```
//!
//! * [`AudioEngine`] is the interface: node creation (returning IDs), named
//!   parameters, live MIDI, and `submit`. [`BlockAudioEngine`] is the
//!   implementation; [`OutputDevice`] picks the sound card it plays on.
//! * **Nodes** ([`NodeDesc`]) are the resources: a synth, a filter, a
//!   "MIDI in". Created up front, referred to by `Copy` [`NodeId`]s.
//! * **Per tick**, the application describes the wiring and parameter
//!   values in an [`AudioWorld`]: plain data. The engine works out what
//!   changed. Wiring is a free graph: any output into any input of the same
//!   [`Signal`], ending at [`NodeId::SPEAKERS`].
//! * **Named parameters**: [`AudioEngine::param`] gives a checked
//!   [`Param`] handle, set per tick with [`AudioWorld::set_param`].
//! * **Live MIDI** doesn't wait for a tick: a [`MidiSender`] posts it
//!   straight to the audio thread, which plays it at the frame matching its
//!   timestamp.
//!
//! Underneath, nodes are blocks of the block platform (`enso-block`;
//! `docs/design/blocks.md`) in a real-time workstream. That's private, like
//! `enso-visual`'s subrenderers.
//!
//! Audio is computed in small steps on a real-time thread that the sound
//! card wakes every few milliseconds. If a step is late you hear a click, so
//! that thread **never allocates, locks, blocks or logs**.

mod blocks;
mod buffer;
mod context;
mod device;
mod driver;
mod engine;
mod node;
mod patch;
mod world;

#[cfg(test)]
mod tests;

pub use context::AudioConfig;
// Internal names, used by the blocks (`crate::AudioBuf`).
pub(crate) use buffer::{AudioBuf, CHANNELS};
pub(crate) use context::{AudioContext, BlockEvent, MidiEvents, midi_events};
pub use device::{AudioStream, DeviceError, OutputDevice, output_device_names};
pub use driver::Stats;
pub use engine::{AudioError, BlockAudioEngine, MidiSender};
pub use node::{
    InputPin, NodeDesc, NodeId, OutputPin, Param, ParamInfo, ParamKey, ParamKind, ParamType, ParamValue, PinInfo,
    SPEAKER_INPUTS, Signal,
};
pub use world::{AudioWorld, ParamSetting};

/// The audio engine interface.
///
/// * **Nodes** are created up front and referred to by `Copy` IDs. IDs are
///   never reused: wires and parameters naming a destroyed node are
///   skipped. Creating, updating or destroying a node takes effect at the
///   next [`submit`](Self::submit).
/// * **Each tick**, [`submit`](Self::submit) is given an [`AudioWorld`]:
///   how nodes are wired, and parameter values.
/// * **Live MIDI** goes through [`midi_sender`](Self::midi_sender), not
///   through ticks, to keep latency minimal.
///
/// Code that only describes patches (the patch editor, a simulation,
/// tests) can be written against this trait (`fn setup(a: &mut impl
/// AudioEngine)`) instead of a concrete engine. Bring it into scope to call
/// its methods: `use enso_audio::AudioEngine;`.
pub trait AudioEngine {
    /// Sample rate and step size, fixed for the engine's lifetime.
    fn config(&self) -> AudioConfig;

    // ---- nodes ----

    fn create_node(&mut self, desc: &NodeDesc) -> Result<NodeId, AudioError>;

    /// Change a node's configuration in place, keeping its ID, wires and
    /// parameters (and its state, if it stays the same kind).
    fn update_node(&mut self, id: NodeId, desc: &NodeDesc) -> Result<(), AudioError>;

    /// Its sound stops at the next submit. Unknown IDs are ignored.
    fn destroy_node(&mut self, id: NodeId);

    /// The node's input pins (what [`AudioWorld::connect`] can wire into),
    /// e.g. to draw it. `None` if the node doesn't exist.
    fn node_inputs(&self, node: NodeId) -> Option<&[PinInfo]>;

    /// The node's output pins. `None` if the node doesn't exist.
    fn node_outputs(&self, node: NodeId) -> Option<&[PinInfo]>;

    /// The node's parameters (what [`AudioWorld::set_param`] can set), e.g.
    /// to build a control panel. `None` if the node doesn't exist.
    fn node_params(&self, node: NodeId) -> Option<&[ParamInfo]>;

    // ---- named handles ----

    /// A handle to the parameter `name` of `node`, checked to exist with
    /// type `T`.
    fn param<T: ParamType>(&self, node: NodeId, name: &str) -> Result<Param<T>, AudioError> {
        let params = self.node_params(node).ok_or(AudioError::UnknownNode(node))?;
        match params.iter().find(|p| p.name == name) {
            Some(p) if p.kind == T::KIND => Ok(Param::new(name)),
            Some(p) => {
                Err(AudioError::ParamKindMismatch { node, name: name.to_owned(), expected: T::KIND, found: p.kind })
            }
            None => Err(AudioError::UnknownParam { node, name: name.to_owned(), available: names(params, |p| &p.name) }),
        }
    }

    /// The input pin `name` of `node` (for nodes with several inputs; a
    /// bare `NodeId` means its first).
    fn input(&self, node: NodeId, name: &str) -> Result<InputPin, AudioError> {
        let pins = self.node_inputs(node).ok_or(AudioError::UnknownNode(node))?;
        find_pin(pins, node, name, "input").map(|index| InputPin { node, index })
    }

    /// The output pin `name` of `node`.
    fn output(&self, node: NodeId, name: &str) -> Result<OutputPin, AudioError> {
        let pins = self.node_outputs(node).ok_or(AudioError::UnknownNode(node))?;
        find_pin(pins, node, name, "output").map(|index| OutputPin { node, index })
    }

    // ---- live input ----

    /// Where live MIDI goes (to every "MIDI in" node whose filter matches).
    fn midi_sender(&self) -> MidiSender;

    // ---- the tick ----

    /// Make the graph match `world`. Changes reach the audio thread between
    /// two of its steps; this never waits for it.
    ///
    /// If the wiring can't be applied (a cycle, mismatched signals), the
    /// previous wiring keeps playing and the error is returned again at each
    /// submit until the world changes. Parameters are applied either way.
    fn submit(&mut self, world: &AudioWorld) -> Result<(), AudioError>;

    /// What the audio thread has been doing (load, buffer size). Resets
    /// the peak load.
    fn stats(&self) -> Stats;
}

fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> String {
    items.iter().map(name).collect::<Vec<_>>().join(", ")
}

fn find_pin(pins: &[PinInfo], node: NodeId, name: &str, direction: &'static str) -> Result<u16, AudioError> {
    match pins.iter().position(|p| p.name == name) {
        Some(index) => Ok(index as u16),
        None => Err(AudioError::UnknownPin {
            node,
            direction,
            name: name.to_owned(),
            available: names(pins, |p| &p.name),
        }),
    }
}
