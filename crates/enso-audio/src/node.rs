//! Nodes (the audio engine's resources), their pins and their parameters.
//!
//! Everything here is plain data (IDs, numbers, static strings), so it can
//! be built on any thread, stored in components, or saved in a preset, like
//! `enso_visual`'s IDs.

use std::{
    borrow::Cow,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};

use enso_core::{Channel, PortId};

// ---- IDs ----------------------------------------------------------------------------

/// A node of the audio graph, from
/// [`AudioEngine::create_node`](crate::AudioEngine::create_node).
///
/// IDs are never reused, so an ID whose node was destroyed simply stops
/// sounding; it can never point at a newer node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u64);

impl NodeId {
    /// The speakers: always there, never created or destroyed. Connect
    /// sound to it to hear it; everything connected to it is mixed.
    pub const SPEAKERS: NodeId = NodeId(0);

    /// A fresh ID, never returned before. [`AudioEngine`](crate::AudioEngine)
    /// implementations use this to name the nodes they create.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

// ---- what a node is ---------------------------------------------------------------------

/// What flows through a pin. Wires only connect pins of the same signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    Midi,
    Audio,
}

/// The kinds of node, with their configuration. Plain data: changing the
/// configuration of a live node is
/// [`AudioEngine::update_node`](crate::AudioEngine::update_node).
///
/// Planned: VST3 instruments and effects, and a MIDI sequencer (it will be a
/// node too, so it runs on the audio thread with no added latency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeDesc {
    /// Live MIDI (everything sent through the engine's
    /// [`MidiSender`](crate::MidiSender)), optionally only from one device
    /// or on one channel.
    MidiIn { port: Option<PortId>, channel: Option<Channel> },
    /// Shifts notes by its `semitones` parameter.
    Transpose,
    /// A small polyphonic synthesizer: MIDI in, sound out.
    TestSynth,
    /// Volume in decibels.
    Gain,
    /// A resonant low-pass filter.
    LowPass,
    /// An echo.
    Delay,
}

/// A pin of a node, for listing (e.g. to draw a node editor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinInfo {
    pub name: Cow<'static, str>,
    pub signal: Signal,
}

/// The value types parameters can have. Only `f32` for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    F32,
}

/// A parameter of a node, for listing (e.g. to build a control panel).
#[derive(Debug, Clone, PartialEq)]
pub struct ParamInfo {
    /// What [`AudioEngine::param`](crate::AudioEngine::param) looks up.
    pub name: Cow<'static, str>,
    /// For display.
    pub label: Cow<'static, str>,
    pub kind: ParamKind,
    pub min: f32,
    pub max: f32,
    /// The value of a new node (before it's first set).
    pub default: f32,
    /// Better shown on a logarithmic scale (frequencies, times).
    pub logarithmic: bool,
    /// For display, e.g. `" Hz"`.
    pub unit: Cow<'static, str>,
}

const fn pin(name: &'static str, signal: Signal) -> PinInfo {
    PinInfo { name: Cow::Borrowed(name), signal }
}

/// A parameter shown on a linear scale.
const fn param(name: &'static str, label: &'static str, range: (f32, f32), default: f32, unit: &'static str) -> ParamInfo {
    shaped(name, label, range, default, unit, false)
}

const fn shaped(
    name: &'static str,
    label: &'static str,
    range: (f32, f32),
    default: f32,
    unit: &'static str,
    logarithmic: bool,
) -> ParamInfo {
    ParamInfo {
        name: Cow::Borrowed(name),
        label: Cow::Borrowed(label),
        kind: ParamKind::F32,
        min: range.0,
        max: range.1,
        default,
        logarithmic,
        unit: Cow::Borrowed(unit),
    }
}

/// A parameter shown on a logarithmic scale.
const fn logarithmic(
    name: &'static str,
    label: &'static str,
    range: (f32, f32),
    default: f32,
    unit: &'static str,
) -> ParamInfo {
    shaped(name, label, range, default, unit, true)
}

// Pin and parameter names are the field names of the blocks behind them.
const MIDI_OUT: &[PinInfo] = &[pin("out", Signal::Midi)];
const MIDI_IN: &[PinInfo] = &[pin("input", Signal::Midi)];
const SYNTH_IN: &[PinInfo] = &[pin("midi", Signal::Midi)];
const AUDIO_IN: &[PinInfo] = &[pin("input", Signal::Audio)];
const AUDIO_OUT: &[PinInfo] = &[pin("out", Signal::Audio)];

/// The inputs of [`NodeId::SPEAKERS`].
pub const SPEAKER_INPUTS: &[PinInfo] = AUDIO_IN;

use crate::blocks::{Delay, Gain, LowPass};

const TRANSPOSE_PARAMS: &[ParamInfo] = &[param("semitones", "shift", (-24.0, 24.0), 0.0, " st")];
const GAIN_PARAMS: &[ParamInfo] = &[param("db", "volume", (-60.0, 12.0), Gain::DEFAULT_DB, " dB")];
const LOW_PASS_PARAMS: &[ParamInfo] = &[
    logarithmic("cutoff", "cutoff", (20.0, 20_000.0), LowPass::DEFAULT_CUTOFF, " Hz"),
    param("resonance", "resonance", (0.0, 1.0), LowPass::DEFAULT_RESONANCE, ""),
];
const DELAY_PARAMS: &[ParamInfo] = &[
    logarithmic("time", "time", (0.01, Delay::MAX_SECONDS), Delay::DEFAULT_TIME, " s"),
    param("feedback", "feedback", (0.0, 0.95), Delay::DEFAULT_FEEDBACK, ""),
    param("mix", "mix", (0.0, 1.0), Delay::DEFAULT_MIX, ""),
];

impl NodeDesc {
    pub fn title(&self) -> &'static str {
        match self {
            NodeDesc::MidiIn { .. } => "MIDI in",
            NodeDesc::Transpose => "Transpose",
            NodeDesc::TestSynth => "Test synth",
            NodeDesc::Gain => "Gain",
            NodeDesc::LowPass => "Low-pass filter",
            NodeDesc::Delay => "Delay",
        }
    }

    /// Input pins; the first is what connecting the bare `NodeId` uses.
    pub fn inputs(&self) -> &'static [PinInfo] {
        match self {
            NodeDesc::MidiIn { .. } => &[],
            NodeDesc::Transpose => MIDI_IN,
            NodeDesc::TestSynth => SYNTH_IN,
            NodeDesc::Gain | NodeDesc::LowPass | NodeDesc::Delay => AUDIO_IN,
        }
    }

    /// Output pins; the first is what connecting the bare `NodeId` uses.
    pub fn outputs(&self) -> &'static [PinInfo] {
        match self {
            NodeDesc::MidiIn { .. } | NodeDesc::Transpose => MIDI_OUT,
            NodeDesc::TestSynth | NodeDesc::Gain | NodeDesc::LowPass | NodeDesc::Delay => AUDIO_OUT,
        }
    }

    pub fn params(&self) -> &'static [ParamInfo] {
        match self {
            NodeDesc::Transpose => TRANSPOSE_PARAMS,
            NodeDesc::Gain => GAIN_PARAMS,
            NodeDesc::LowPass => LOW_PASS_PARAMS,
            NodeDesc::Delay => DELAY_PARAMS,
            NodeDesc::MidiIn { .. } | NodeDesc::TestSynth => &[],
        }
    }
}

// ---- pins ---------------------------------------------------------------------------

/// An output pin: a node and the index of one of its outputs.
/// A bare [`NodeId`] converts to its first output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputPin {
    pub node: NodeId,
    pub index: u16,
}

/// An input pin: a node and the index of one of its inputs.
/// A bare [`NodeId`] converts to its first input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputPin {
    pub node: NodeId,
    pub index: u16,
}

impl From<NodeId> for OutputPin {
    fn from(node: NodeId) -> Self {
        Self { node, index: 0 }
    }
}

impl From<NodeId> for InputPin {
    fn from(node: NodeId) -> Self {
        Self { node, index: 0 }
    }
}

// ---- parameters -----------------------------------------------------------------------

/// A parameter's name, hashed: what [`AudioWorld`](crate::AudioWorld)
/// stores, so worlds stay small and allocation-free to fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamKey(pub(crate) enso_block::Key);

impl ParamKey {
    pub fn of(name: &str) -> Self {
        Self(enso_block::Key::named(name))
    }
}

/// A parameter value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamValue {
    F32(f32),
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for f32 {}
}

/// Rust types that parameters can hold. Sealed: implemented for `f32`.
pub trait ParamType: sealed::Sealed + Copy {
    const KIND: ParamKind;
    fn into_value(self) -> ParamValue;
}

impl ParamType for f32 {
    const KIND: ParamKind = ParamKind::F32;
    fn into_value(self) -> ParamValue {
        ParamValue::F32(self)
    }
}

/// A handle to a named node parameter of type `T`.
///
/// Get a checked one from [`AudioEngine::param`](crate::AudioEngine::param)
/// (which verifies the node has it, with this type), or an unchecked one
/// with [`Param::new`]. Handles refer to the parameter by name, not by node,
/// so one handle serves every node of a kind.
pub struct Param<T> {
    key: ParamKey,
    _type: PhantomData<fn() -> T>,
}

// Manual impls: derive would require `T: Clone` etc.
impl<T> Clone for Param<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Param<T> {}
impl<T> std::fmt::Debug for Param<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Param").field(&self.key).finish()
    }
}

impl<T: ParamType> Param<T> {
    /// A handle without checking any node. Prefer
    /// [`AudioEngine::param`](crate::AudioEngine::param).
    pub fn new(name: &str) -> Self {
        Self { key: ParamKey::of(name), _type: PhantomData }
    }

    pub fn key(self) -> ParamKey {
        self.key
    }
}
