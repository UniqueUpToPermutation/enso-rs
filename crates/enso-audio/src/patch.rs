//! [`Patch`]: the wiring between nodes, and its compiled form, [`Schedule`].

use enso_core::{Channel, PortId};

use crate::{
    AudioConfig, CHANNELS, EngineError, NodeId,
    node::{BlockEvent, EventOutput},
};

/// Which MIDI from the outside world reaches a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MidiFilter {
    /// Only this input device (`None`: any).
    pub port: Option<PortId>,
    /// Only this channel (`None`: any).
    pub channel: Option<Channel>,
}

impl MidiFilter {
    pub fn matches(&self, port: PortId, channel: Channel) -> bool {
        self.port.is_none_or(|p| p == port) && self.channel.is_none_or(|c| c == channel)
    }
}

/// Where a node's MIDI comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MidiFrom {
    /// MIDI inputs (keyboards, controllers) passing the filter.
    Input(MidiFilter),
    /// Events another node emits.
    Node(NodeId),
}

impl MidiFrom {
    /// Every MIDI input, every channel.
    pub fn inputs() -> Self {
        Self::Input(MidiFilter::default())
    }

    /// One input device, every channel.
    pub fn port(port: PortId) -> Self {
        Self::Input(MidiFilter { port: Some(port), channel: None })
    }

    /// Every input device, one channel.
    pub fn channel(channel: Channel) -> Self {
        Self::Input(MidiFilter { port: None, channel: Some(channel) })
    }
}

/// The wiring of the engine: which audio and MIDI flows between nodes and to
/// the speakers. Plain data; build it anywhere, then hand it to
/// [`AudioEngine::set_patch`](crate::AudioEngine::set_patch), which swaps it
/// in between two blocks while sound keeps playing.
///
/// Only nodes mentioned in the patch run. Several connections into one
/// input are summed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Patch {
    pub(crate) audio: Vec<(NodeId, NodeId)>,
    pub(crate) output: Vec<NodeId>,
    pub(crate) midi: Vec<(MidiFrom, NodeId)>,
}

impl Patch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Send `from`'s audio into `to`'s input.
    pub fn audio(&mut self, from: NodeId, to: NodeId) -> &mut Self {
        self.audio.push((from, to));
        self
    }

    /// Send `from`'s audio to the speakers.
    pub fn output(&mut self, from: NodeId) -> &mut Self {
        self.output.push(from);
        self
    }

    /// Send MIDI into `to`.
    pub fn midi(&mut self, from: MidiFrom, to: NodeId) -> &mut Self {
        self.midi.push((from, to));
        self
    }

    /// Remove every connection to or from `node`.
    pub fn disconnect(&mut self, node: NodeId) -> &mut Self {
        self.audio.retain(|&(a, b)| a != node && b != node);
        self.output.retain(|&a| a != node);
        self.midi.retain(|&(from, to)| from != MidiFrom::Node(node) && to != node);
        self
    }

    /// Every node mentioned, in order of first mention.
    pub fn nodes(&self) -> Vec<NodeId> {
        let mut nodes = Vec::new();
        let mut add = |n: NodeId| {
            if !nodes.contains(&n) {
                nodes.push(n);
            }
        };
        for &(a, b) in &self.audio {
            add(a);
            add(b);
        }
        for &a in &self.output {
            add(a);
        }
        for &(from, to) in &self.midi {
            if let MidiFrom::Node(n) = from {
                add(n);
            }
            add(to);
        }
        nodes
    }

    /// Order the nodes so every node runs after its sources, and allocate
    /// every buffer the audio thread will need. Runs on the control thread.
    pub(crate) fn compile(&self, config: AudioConfig) -> Result<Schedule, EngineError> {
        let nodes = self.nodes();
        let index = |n: NodeId| nodes.iter().position(|&m| m == n).expect("collected above");

        // Edges source -> destination, for both audio and MIDI.
        let mut edges: Vec<(usize, usize)> = self.audio.iter().map(|&(a, b)| (index(a), index(b))).collect();
        edges.extend(self.midi.iter().filter_map(|&(from, to)| match from {
            MidiFrom::Node(n) => Some((index(n), index(to))),
            MidiFrom::Input(_) => None,
        }));

        // Kahn's algorithm: repeatedly take a node with no unscheduled
        // sources. Scanning in mention order keeps the result deterministic.
        let mut incoming = vec![0usize; nodes.len()];
        for &(_, to) in &edges {
            incoming[to] += 1;
        }
        let mut order = Vec::with_capacity(nodes.len());
        let mut done = vec![false; nodes.len()];
        while order.len() < nodes.len() {
            let Some(next) = (0..nodes.len()).find(|&i| !done[i] && incoming[i] == 0) else {
                return Err(EngineError::Cycle);
            };
            done[next] = true;
            order.push(next);
            for &(from, to) in &edges {
                if from == next {
                    incoming[to] -= 1;
                }
            }
        }

        // `step_of[node index]` = its position in the run order.
        let mut step_of = vec![0; nodes.len()];
        for (step, &node) in order.iter().enumerate() {
            step_of[node] = step;
        }
        let steps = order
            .iter()
            .map(|&node| {
                let id = nodes[node];
                Step {
                    slot: id.slot(),
                    audio_from: self.audio.iter().filter(|&&(_, to)| to == id).map(|&(from, _)| step_of[index(from)]).collect(),
                    midi_from: self
                        .midi
                        .iter()
                        .filter(|&&(_, to)| to == id)
                        .map(|&(from, _)| match from {
                            MidiFrom::Input(filter) => MidiSource::Input(filter),
                            MidiFrom::Node(n) => MidiSource::Step(step_of[index(n)]),
                        })
                        .collect(),
                }
            })
            .collect::<Vec<_>>();
        let to_device = self.output.iter().map(|&n| step_of[index(n)]).collect();
        Ok(Schedule::new(config, steps, to_device))
    }
}

/// A compiled [`Patch`]: run order plus every buffer, allocated in advance.
/// Built on the control thread, then owned by the audio thread.
pub(crate) struct Schedule {
    pub steps: Vec<Step>,
    /// Steps whose output goes to the speakers.
    pub to_device: Vec<usize>,
    /// Each step's audio output: `CHANNELS * max_block` floats per step.
    pub outputs: Vec<f32>,
    /// Each step's MIDI output.
    pub events_out: Vec<EventOutput>,
    /// Scratch: the summed audio input of the step being run.
    pub mix: Vec<f32>,
    /// Scratch: the merged MIDI input of the step being run.
    pub events_in: Vec<BlockEvent>,
}

pub(crate) struct Step {
    /// Which node (slot in the processor's node table) this step runs.
    pub slot: usize,
    /// Earlier steps whose audio is summed into this one's input.
    pub audio_from: Vec<usize>,
    pub midi_from: Vec<MidiSource>,
}

pub(crate) enum MidiSource {
    Input(MidiFilter),
    /// An earlier step's MIDI output.
    Step(usize),
}

impl Schedule {
    pub fn new(config: AudioConfig, steps: Vec<Step>, to_device: Vec<usize>) -> Self {
        let block = CHANNELS * config.max_block;
        Self {
            outputs: vec![0.0; steps.len() * block],
            events_out: steps.iter().map(|_| EventOutput::new()).collect(),
            mix: vec![0.0; block],
            // Room for every source's events at once.
            events_in: Vec::with_capacity(EventOutput::CAPACITY * 4),
            steps,
            to_device,
        }
    }

    pub fn empty(config: AudioConfig) -> Self {
        Self::new(config, Vec::new(), Vec::new())
    }
}

/// True if `event` passes `filter`.
pub(crate) fn input_matches(filter: &MidiFilter, event: &BlockEvent) -> bool {
    filter.matches(event.port, event.message.channel())
}
