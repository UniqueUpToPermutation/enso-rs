//! [`BlockAudioEngine`]: the [`AudioEngine`] implementation, on blocks.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use enso_block::{BlockId, BuildError, Engine, EngineError, InterfaceId, Key, Workstream, WorkstreamId};
use enso_core::{Clock, MidiEvent, Timestamp};

use crate::{
    AudioConfig, AudioEngine, AudioStream, AudioWorld, DeviceError, NodeDesc, NodeId, OutputDevice, ParamInfo,
    ParamKey, ParamKind, ParamValue, PinInfo, SPEAKER_INPUTS, Signal, Stats,
    driver::{AudioDriver, StatsHandle},
    patch::{Endpoint, PatchRoot, PatchSpec},
};

/// What can go wrong talking to an [`AudioEngine`]. Nothing here is fatal:
/// whatever was playing keeps playing.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no node {0:?}")]
    UnknownNode(NodeId),
    #[error("{node:?} has no parameter {name:?} (available: {available})")]
    UnknownParam { node: NodeId, name: String, available: String },
    #[error("{node:?}'s parameter {name:?} is {found:?}, not {expected:?}")]
    ParamKindMismatch { node: NodeId, name: String, expected: ParamKind, found: ParamKind },
    #[error("{node:?} has no {direction} called {name:?} (available: {available})")]
    UnknownPin { node: NodeId, direction: &'static str, name: String, available: String },
    #[error("{node:?} has no {direction} number {index}")]
    NoSuchPin { node: NodeId, direction: &'static str, index: u16 },
    #[error("can't wire {from:?} ({from_signal:?}) into {to:?} ({to_signal:?})")]
    SignalMismatch { from: NodeId, from_signal: Signal, to: NodeId, to_signal: Signal },
    /// The world's wiring can't be built (e.g. a cycle, or two wires into
    /// one audio input). The previous wiring keeps playing.
    #[error("the patch can't be built: {0}")]
    Patch(String),
    /// The audio thread hasn't picked up earlier changes yet (or isn't
    /// running). Submit again later.
    #[error("the audio thread hasn't caught up with earlier changes yet")]
    Busy,
    #[error("the engine is already playing")]
    AlreadyStarted,
    #[error(transparent)]
    Device(#[from] DeviceError),
}

/// Sends live MIDI to an [`AudioEngine`], from any thread (MIDI input
/// threads, the UI). Clone it freely. Events are played at their
/// timestamps: stamp them with the clock the engine was started with.
#[derive(Clone)]
pub struct MidiSender(Arc<dyn Fn(MidiEvent) + Send + Sync>);

impl MidiSender {
    /// A sender calling `send` for every event (engines build theirs
    /// with this; a test fake can record).
    pub fn new(send: impl Fn(MidiEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(send))
    }

    /// Never blocks. If the engine's queue is full, the oldest waiting
    /// event is dropped.
    pub fn send(&self, event: MidiEvent) {
        (self.0)(event)
    }
}

impl std::fmt::Debug for MidiSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MidiSender")
    }
}

struct Node {
    /// The UUID of the node's block: stable for the node's lifetime, so
    /// the block keeps its state (voices, echoes) across rewiring.
    block: BlockId,
    desc: NodeDesc,
}

/// The audio engine: nodes are blocks in a real-time `enso_block`
/// workstream, which the sound card's callback steps.
///
/// * [`submit`](AudioEngine::submit) turns a changed set of nodes or wires
///   into a rebuild of the block tree, swapped in on the audio thread
///   between two steps (nodes keep their state); parameter changes are sent
///   to the blocks directly. Nothing here ever waits for the audio thread.
/// * [`start`](Self::start) plays it on a sound card; or, without one,
///   [`render`](Self::render) computes it offline.
pub struct BlockAudioEngine {
    config: AudioConfig,
    engine: Engine,
    workstream: WorkstreamId,
    /// Until `start` moves it to the audio thread.
    driver: Option<AudioDriver>,
    stats: StatsHandle,
    midi: MidiSender,
    nodes: BTreeMap<NodeId, Node>,
    /// The spec the workstream was last rebuilt from.
    applied: PatchSpec,
    /// The last spec that failed to build, and why: not retried until the
    /// world changes.
    failed: Option<(PatchSpec, String)>,
    /// Parameter values the blocks have been sent.
    sent: HashMap<(NodeId, ParamKey), ParamValue>,
}

impl BlockAudioEngine {
    /// An engine with no nodes. For a sound card, use its
    /// [`OutputDevice::audio_config`].
    pub fn new(config: AudioConfig) -> Self {
        let applied = PatchSpec::default();
        let workstream =
            Workstream::build_realtime(PatchRoot::new(config, applied.clone())).expect("an empty patch always builds");
        let mut engine = Engine::new();
        let id = engine.add("audio", workstream);
        let root = engine.root::<PatchRoot>(id).expect("just added");
        let inbox = engine.get(root.source).expect("built").input;
        let output = engine.get(root.output).expect("built").input;
        let mail = engine.sender(inbox).expect("the root exposes its MIDI source");
        let driver = AudioDriver::new(engine.take(id).expect("just added"), output, config);
        Self {
            config,
            stats: driver.stats(),
            engine,
            workstream: id,
            driver: Some(driver),
            midi: MidiSender::new(move |event| mail.send(event)),
            nodes: BTreeMap::new(),
            applied,
            failed: None,
            sent: HashMap::new(),
        }
    }

    /// Play on `device`: the audio computation moves to its thread. Sound
    /// stops when the returned stream is dropped. `clock` must be the clock
    /// MIDI events are stamped with. Can be done once; if the device fails
    /// to start (other than a sample-rate mismatch, checked first), the
    /// engine can't be started again.
    pub fn start(&mut self, device: OutputDevice, clock: Clock) -> Result<AudioStream, AudioError> {
        let device_rate = device.audio_config().sample_rate;
        if device_rate != self.config.sample_rate {
            return Err(DeviceError::SampleRateMismatch { driver: self.config.sample_rate, device: device_rate }.into());
        }
        let driver = self.driver.take().ok_or(AudioError::AlreadyStarted)?;
        Ok(device.start(driver, clock)?)
    }

    /// Compute the stretch of time ending at `now` into `output`
    /// (interleaved, `channels` samples per frame), without a sound card:
    /// for tests, offline bouncing, or keeping a device-less engine
    /// running. Silence once [`start`](Self::start)ed. Never allocates.
    pub fn render(&mut self, output: &mut [f32], channels: usize, now: Timestamp) {
        match &mut self.driver {
            Some(driver) => driver.render(output, channels, now),
            None => output.fill(0.0),
        }
    }

    /// Free what the audio thread has finished with. [`submit`] does this;
    /// call it when rendering offline without submitting.
    ///
    /// [`submit`]: AudioEngine::submit
    pub fn collect_garbage(&mut self) {
        self.engine.collect_garbage();
    }

    /// The block tree the world describes.
    fn spec(&self, world: &AudioWorld) -> Result<PatchSpec, AudioError> {
        let nodes = self.nodes.values().map(|n| (n.block, n.desc)).collect();
        let mut wires = Vec::with_capacity(world.wires().len());
        for &(from, to) in world.wires() {
            // Wires to or from destroyed nodes are skipped, like draws of
            // destroyed meshes.
            let Some(source) = self.nodes.get(&from.node) else { continue };
            let Some(out) = source.desc.outputs().get(usize::from(from.index)) else {
                return Err(AudioError::NoSuchPin { node: from.node, direction: "output", index: from.index });
            };
            let (end, input) = if to.node == NodeId::SPEAKERS {
                (Endpoint::Speakers, SPEAKER_INPUTS.get(usize::from(to.index)))
            } else {
                let Some(target) = self.nodes.get(&to.node) else { continue };
                let input = target.desc.inputs().get(usize::from(to.index));
                (Endpoint::Pin(target.block, input.map_or(Key::new(0), |i| Key::named(&i.name))), input)
            };
            let Some(input) = input else {
                return Err(AudioError::NoSuchPin { node: to.node, direction: "input", index: to.index });
            };
            if out.signal != input.signal {
                return Err(AudioError::SignalMismatch {
                    from: from.node,
                    from_signal: out.signal,
                    to: to.node,
                    to_signal: input.signal,
                });
            }
            wires.push((Endpoint::Pin(source.block, Key::named(&out.name)), end));
        }
        Ok(PatchSpec { nodes, wires })
    }

    /// Rebuild the block tree if the world's nodes or wires changed.
    fn apply_wiring(&mut self, world: &AudioWorld) -> Result<(), AudioError> {
        let spec = self.spec(world)?;
        if spec == self.applied {
            return Ok(());
        }
        if let Some((failed, why)) = &self.failed
            && *failed == spec
        {
            return Err(AudioError::Patch(why.clone()));
        }
        match self.engine.rebuild(self.workstream, PatchRoot::new(self.config, spec.clone())) {
            Ok(()) => {
                self.applied = spec;
                self.failed = None;
                Ok(())
            }
            Err(EngineError::Build(BuildError::Busy)) => Err(AudioError::Busy),
            Err(err) => {
                let why = err.to_string();
                self.failed = Some((spec, why.clone()));
                Err(AudioError::Patch(why))
            }
        }
    }

    /// Send parameter values that changed since they were last sent.
    fn apply_params(&mut self, world: &AudioWorld) {
        for setting in world.params() {
            let Some(node) = self.nodes.get(&setting.node) else { continue };
            let slot = (setting.node, setting.param);
            if self.sent.get(&slot) == Some(&setting.value) {
                continue;
            }
            let input = InterfaceId { block: node.block, key: setting.param.0 };
            let result = match setting.value {
                ParamValue::F32(value) => self.engine.set_id(input, value),
            };
            // On error (the node's block isn't built yet because its rebuild
            // failed or is waiting, or the queue is full), try again at the
            // next submit.
            if result.is_ok() {
                self.sent.insert(slot, setting.value);
            }
        }
    }
}

impl AudioEngine for BlockAudioEngine {
    fn config(&self) -> AudioConfig {
        self.config
    }

    fn create_node(&mut self, desc: &NodeDesc) -> Result<NodeId, AudioError> {
        let id = NodeId::next();
        self.nodes.insert(id, Node { block: BlockId::random(), desc: *desc });
        Ok(id)
    }

    fn update_node(&mut self, id: NodeId, desc: &NodeDesc) -> Result<(), AudioError> {
        let node = self.nodes.get_mut(&id).ok_or(AudioError::UnknownNode(id))?;
        node.desc = *desc;
        Ok(())
    }

    fn destroy_node(&mut self, id: NodeId) {
        if self.nodes.remove(&id).is_some() {
            self.sent.retain(|(node, _), _| *node != id);
        }
    }

    fn node_inputs(&self, node: NodeId) -> Option<&[PinInfo]> {
        if node == NodeId::SPEAKERS {
            return Some(SPEAKER_INPUTS);
        }
        self.nodes.get(&node).map(|n| n.desc.inputs())
    }

    fn node_outputs(&self, node: NodeId) -> Option<&[PinInfo]> {
        if node == NodeId::SPEAKERS {
            return Some(&[]);
        }
        self.nodes.get(&node).map(|n| n.desc.outputs())
    }

    fn node_params(&self, node: NodeId) -> Option<&[ParamInfo]> {
        if node == NodeId::SPEAKERS {
            return Some(&[]);
        }
        self.nodes.get(&node).map(|n| n.desc.params())
    }

    fn midi_sender(&self) -> MidiSender {
        self.midi.clone()
    }

    fn submit(&mut self, world: &AudioWorld) -> Result<(), AudioError> {
        let wiring = self.apply_wiring(world);
        self.apply_params(world);
        self.engine.collect_garbage();
        wiring
    }

    fn stats(&self) -> Stats {
        self.stats.read()
    }
}
