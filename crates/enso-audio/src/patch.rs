//! The block tree behind [`BlockAudioEngine`](crate::BlockAudioEngine):
//! [`PatchRoot`], a *data-defined* composite (design §6) built from a
//! [`PatchSpec`] and rebuilt whenever the spec changes.

use enso_block::{Block, BlockId, Child, InterfaceId, Key, LinkError, Linker, Ports, Registrar};

use crate::{
    AudioConfig, NodeDesc,
    blocks::{AudioOut, Delay, Gain, LowPass, MidiFilter, MidiSource, TestSynth, Transpose},
};

/// One end of a wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// An interface of a node's block.
    Pin(BlockId, Key),
    /// The root's [`AudioOut`].
    Speakers,
}

/// Which blocks exist (by UUID) and which interfaces are wired.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct PatchSpec {
    pub nodes: Vec<(BlockId, NodeDesc)>,
    pub wires: Vec<(Endpoint, Endpoint)>,
}

/// A [`MidiSource`] (where live MIDI enters), an [`AudioOut`] (what the
/// device plays), and the spec's nodes, wired as the spec says.
#[derive(Ports)]
pub(crate) struct PatchRoot {
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
        for &(id, desc) in &self.spec.nodes {
            let label = desc.title();
            match desc {
                NodeDesc::MidiIn { port, channel } => {
                    self.filters.push(r.child_with_id(id, label, MidiFilter::new(port, channel)));
                }
                NodeDesc::TestSynth => drop(r.child_with_id(id, label, TestSynth::new(config))),
                NodeDesc::Transpose => drop(r.child_with_id(id, label, Transpose::new())),
                NodeDesc::Gain => drop(r.child_with_id(id, label, Gain::new(config))),
                NodeDesc::LowPass => drop(r.child_with_id(id, label, LowPass::new(config))),
                NodeDesc::Delay => drop(r.child_with_id(id, label, Delay::new(config))),
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
            Endpoint::Speakers => speakers,
        };
        for &(from, to) in &self.spec.wires {
            l.connect_ids(resolve(from), resolve(to))?;
        }
        Ok(())
    }
}
