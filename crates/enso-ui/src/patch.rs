//! A patch as data, and the block tree it describes.
//!
//! [`PatchSpec`] is what the editor's graph means: which blocks exist
//! (each with a UUID) and which interfaces are wired, by ID.
//! [`PatchRoot`] is the root block built from it: a *data-defined*
//! composite (design §6), rebuilt whenever the spec changes.

use enso_audio::{
    AudioConfig,
    blocks::{AudioOut, Delay, Gain, LowPass, MidiFilter, MidiSource, TestSynth, Transpose},
    enso_block::{Block, BlockId, Child, InterfaceId, Key, LinkError, Linker, Ports, Registrar},
};
use enso_core::{Channel, PortId};

/// What flows through a pin. Wires only connect pins of the same kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Midi,
    Audio,
}

/// The kinds of node a patch can contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// MIDI from the outside world, optionally one device or channel
    /// (a [`MidiFilter`] fed by the patch's [`MidiSource`]).
    MidiIn { port: Option<PortId>, channel: Option<Channel> },
    /// The speakers (the patch's [`AudioOut`]; no block of its own).
    Output,
    TestSynth,
    Transpose,
    Gain,
    LowPass,
    Delay,
}

/// A pin: its signal, and the key of the block interface it stands for.
#[derive(Debug, Clone, Copy)]
pub struct Pin {
    pub signal: Signal,
    pub key: Key,
}

const fn pin(signal: Signal, field: &str) -> Pin {
    // Interface keys are the hashes of the blocks' field names (decision 24).
    Pin { signal, key: Key::named(field) }
}

/// A knob: an unconnected `In<f32>` of the block, shown as a slider.
#[derive(Debug, Clone, Copy)]
pub struct KnobSpec {
    pub label: &'static str,
    pub key: Key,
    pub range: (f32, f32),
    pub default: f32,
    pub logarithmic: bool,
    pub suffix: &'static str,
}

const fn knob(label: &'static str, field: &str, range: (f32, f32), default: f32, suffix: &'static str) -> KnobSpec {
    KnobSpec { label, key: Key::named(field), range, default, logarithmic: false, suffix }
}

impl NodeKind {
    /// Offered in the "add node" menu, in this order.
    pub const MENU: [NodeKind; 7] = [
        NodeKind::MidiIn { port: None, channel: None },
        NodeKind::Transpose,
        NodeKind::TestSynth,
        NodeKind::LowPass,
        NodeKind::Delay,
        NodeKind::Gain,
        NodeKind::Output,
    ];

    pub fn title(self) -> &'static str {
        match self {
            NodeKind::MidiIn { .. } => "MIDI in",
            NodeKind::Output => "Output",
            NodeKind::TestSynth => "Test synth",
            NodeKind::Transpose => "Transpose",
            NodeKind::Gain => "Gain",
            NodeKind::LowPass => "Low-pass filter",
            NodeKind::Delay => "Delay",
        }
    }

    pub fn inputs(self) -> &'static [Pin] {
        const MIDI_IN: &[Pin] = &[pin(Signal::Midi, "input")];
        const SYNTH_IN: &[Pin] = &[pin(Signal::Midi, "midi")];
        const AUDIO_IN: &[Pin] = &[pin(Signal::Audio, "input")];
        match self {
            NodeKind::MidiIn { .. } => &[],
            NodeKind::Transpose => MIDI_IN,
            NodeKind::TestSynth => SYNTH_IN,
            NodeKind::Gain | NodeKind::LowPass | NodeKind::Delay | NodeKind::Output => AUDIO_IN,
        }
    }

    pub fn outputs(self) -> &'static [Pin] {
        const MIDI_OUT: &[Pin] = &[pin(Signal::Midi, "out")];
        const AUDIO_OUT: &[Pin] = &[pin(Signal::Audio, "out")];
        match self {
            NodeKind::MidiIn { .. } | NodeKind::Transpose => MIDI_OUT,
            NodeKind::TestSynth | NodeKind::Gain | NodeKind::LowPass | NodeKind::Delay => AUDIO_OUT,
            NodeKind::Output => &[],
        }
    }

    /// Defaults match the blocks' own, so a new node sounds the same before
    /// and after its knobs are first touched.
    pub fn knobs(self) -> Vec<KnobSpec> {
        match self {
            NodeKind::Transpose => vec![knob("shift", "semitones", (-24.0, 24.0), 0.0, " st")],
            NodeKind::Gain => vec![knob("volume", "db", (-60.0, 12.0), Gain::DEFAULT_DB, " dB")],
            NodeKind::LowPass => vec![
                KnobSpec {
                    logarithmic: true,
                    ..knob("cutoff", "cutoff", (20.0, 20_000.0), LowPass::DEFAULT_CUTOFF, " Hz")
                },
                knob("resonance", "resonance", (0.0, 1.0), LowPass::DEFAULT_RESONANCE, ""),
            ],
            NodeKind::Delay => vec![
                KnobSpec {
                    logarithmic: true,
                    ..knob("time", "time", (0.01, Delay::MAX_SECONDS), Delay::DEFAULT_TIME, " s")
                },
                knob("feedback", "feedback", (0.0, 0.95), Delay::DEFAULT_FEEDBACK, ""),
                knob("mix", "mix", (0.0, 1.0), Delay::DEFAULT_MIX, ""),
            ],
            NodeKind::MidiIn { .. } | NodeKind::Output | NodeKind::TestSynth => Vec::new(),
        }
    }
}

/// One end of a wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// An interface of a node's block.
    Pin(BlockId, Key),
    /// The speakers: the patch's [`AudioOut`].
    Output,
}

/// What a patch is: blocks (by UUID) and wires between their interfaces.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PatchSpec {
    /// Every node that has a block (all but Output nodes).
    pub nodes: Vec<(BlockId, NodeKind)>,
    pub wires: Vec<(Endpoint, Endpoint)>,
}

/// The root block of an audio patch: a [`MidiSource`] (where MIDI enters),
/// an [`AudioOut`] (what the device plays), and the spec's nodes, wired as
/// the spec says.
#[derive(Ports)]
pub struct PatchRoot {
    config: AudioConfig,
    spec: PatchSpec,
    #[child]
    pub source: Child<MidiSource>,
    #[child(init = AudioOut::new(self.config))]
    pub output: Child<AudioOut>,
    /// "MIDI in" nodes: fed from `source`.
    filters: Vec<Child<MidiFilter>>,
}

impl PatchRoot {
    pub fn new(config: AudioConfig, spec: PatchSpec) -> Self {
        Self { config, spec, source: Child::default(), output: Child::default(), filters: Vec::new() }
    }
}

impl Block for PatchRoot {
    fn register(&mut self, r: &mut Registrar) {
        let config = self.config;
        for &(id, kind) in &self.spec.nodes {
            let label = kind.title();
            match kind {
                NodeKind::MidiIn { port, channel } => {
                    self.filters.push(r.child_with_id(id, label, MidiFilter::new(port, channel)));
                }
                NodeKind::TestSynth => drop(r.child_with_id(id, label, TestSynth::new(config))),
                NodeKind::Transpose => drop(r.child_with_id(id, label, Transpose::new())),
                NodeKind::Gain => drop(r.child_with_id(id, label, Gain::new(config))),
                NodeKind::LowPass => drop(r.child_with_id(id, label, LowPass::new(config))),
                NodeKind::Delay => drop(r.child_with_id(id, label, Delay::new(config))),
                NodeKind::Output => {}
            }
        }
    }

    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        for &filter in &self.filters {
            l.connect(l[self.source].out, l[filter].input)?;
        }
        let speakers = InterfaceId { block: l.id_of(self.output), key: Key::named("input") };
        let resolve = |end: Endpoint| match end {
            Endpoint::Pin(block, key) => InterfaceId { block, key },
            Endpoint::Output => speakers,
        };
        for &(from, to) in &self.spec.wires {
            l.connect_ids(resolve(from), resolve(to))?;
        }
        Ok(())
    }
}
