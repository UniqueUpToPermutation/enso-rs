//! [`AudioWorld`]: what the audio graph should be, as plain data.

use crate::{InputPin, NodeId, OutputPin, Param, ParamKey, ParamType, ParamValue};

/// One parameter setting in an [`AudioWorld`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSetting {
    pub node: NodeId,
    pub param: ParamKey,
    pub value: ParamValue,
}

/// How the nodes are wired and what their parameters are, described each
/// tick by the application (simulation, patch editor) and handed to
/// [`AudioEngine::submit`](crate::AudioEngine::submit). The audio-side
/// counterpart of `enso_visual::RenderWorld`.
///
/// **Plain data:** node IDs and numbers only, so it can be built on any
/// thread and kept between ticks (call [`clear`](Self::clear) and refill it
/// to reuse its memory).
///
/// * **Wiring is the whole truth:** a wire that's missing from a submitted
///   world is removed. Wires naming destroyed nodes are skipped.
/// * **Parameters persist:** a parameter keeps its last submitted value
///   until a world sets it again (sound must not jump because a tick forgot
///   one). Values glide inside the engine, so setting one every tick is fine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioWorld {
    wires: Vec<(OutputPin, InputPin)>,
    params: Vec<ParamSetting>,
}

impl AudioWorld {
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty it, keeping its memory.
    pub fn clear(&mut self) {
        self.wires.clear();
        self.params.clear();
    }

    /// Wire `from` into `to`: MIDI to MIDI, audio to audio. A `NodeId`
    /// means its first pin; [`NodeId::SPEAKERS`] is the sound you hear.
    /// An input takes any number of wires: audio is mixed, MIDI merged in
    /// time order.
    pub fn connect(&mut self, from: impl Into<OutputPin>, to: impl Into<InputPin>) {
        let wire = (from.into(), to.into());
        if !self.wires.contains(&wire) {
            self.wires.push(wire);
        }
    }

    /// Set a parameter of `node`. Setting it again in the same world
    /// replaces the value.
    pub fn set_param<T: ParamType>(&mut self, node: NodeId, param: Param<T>, value: T) {
        let setting = ParamSetting { node, param: param.key(), value: value.into_value() };
        match self.params.iter_mut().find(|p| p.node == node && p.param == setting.param) {
            Some(existing) => *existing = setting,
            None => self.params.push(setting),
        }
    }

    pub fn wires(&self) -> &[(OutputPin, InputPin)] {
        &self.wires
    }

    pub fn params(&self) -> &[ParamSetting] {
        &self.params
    }
}
