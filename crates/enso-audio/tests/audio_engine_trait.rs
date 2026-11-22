//! The `AudioEngine` trait can be implemented without a sound card or
//! blocks, so code that builds patches can be written once against the
//! trait and tested anywhere.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use enso_audio::{
    AudioConfig, AudioEngine, AudioError, AudioWorld, MidiSender, NodeDesc, NodeId, ParamInfo, PinInfo,
    SPEAKER_INPUTS, Stats,
};
use enso_core::{Channel, MidiEvent, MidiMessage, PortId, Timestamp};

/// A fake engine that records what it was asked to do.
#[derive(Default)]
struct Recording {
    nodes: HashMap<NodeId, NodeDesc>,
    /// Wire count of each submitted world.
    submits: Vec<usize>,
    midi: Arc<Mutex<Vec<MidiEvent>>>,
}

impl AudioEngine for Recording {
    fn config(&self) -> AudioConfig {
        AudioConfig::new(48_000)
    }
    fn create_node(&mut self, desc: &NodeDesc) -> Result<NodeId, AudioError> {
        let id = NodeId::next();
        self.nodes.insert(id, *desc);
        Ok(id)
    }
    fn update_node(&mut self, id: NodeId, desc: &NodeDesc) -> Result<(), AudioError> {
        *self.nodes.get_mut(&id).ok_or(AudioError::UnknownNode(id))? = *desc;
        Ok(())
    }
    fn destroy_node(&mut self, id: NodeId) {
        self.nodes.remove(&id);
    }
    fn node_inputs(&self, node: NodeId) -> Option<&[PinInfo]> {
        if node == NodeId::SPEAKERS {
            return Some(SPEAKER_INPUTS);
        }
        self.nodes.get(&node).map(|d| d.inputs())
    }
    fn node_outputs(&self, node: NodeId) -> Option<&[PinInfo]> {
        self.nodes.get(&node).map(|d| d.outputs())
    }
    fn node_params(&self, node: NodeId) -> Option<&[ParamInfo]> {
        self.nodes.get(&node).map(|d| d.params())
    }
    fn midi_sender(&self) -> MidiSender {
        let midi = self.midi.clone();
        MidiSender::new(move |event| midi.lock().unwrap().push(event))
    }
    fn submit(&mut self, world: &AudioWorld) -> Result<(), AudioError> {
        self.submits.push(world.wires().len());
        Ok(())
    }
    fn stats(&self) -> Stats {
        Stats::default()
    }
}

/// Patch-building code written against the trait.
fn build_patch(audio: &mut impl AudioEngine, world: &mut AudioWorld) -> Result<NodeId, AudioError> {
    let midi_in = audio.create_node(&NodeDesc::MidiIn { port: None, channel: None })?;
    let synth = audio.create_node(&NodeDesc::TestSynth)?;
    let filter = audio.create_node(&NodeDesc::LowPass)?;
    let cutoff = audio.param::<f32>(filter, "cutoff")?;
    world.connect(midi_in, audio.input(synth, "midi")?);
    world.connect(synth, filter);
    world.connect(audio.output(filter, "out")?, NodeId::SPEAKERS);
    world.set_param(filter, cutoff, 1200.0);
    audio.submit(world)?;
    Ok(filter)
}

#[test]
fn patches_can_be_built_against_the_trait() {
    let mut fake = Recording::default();
    let mut world = AudioWorld::new();
    let filter = build_patch(&mut fake, &mut world).unwrap();

    assert_eq!(fake.nodes.len(), 3);
    assert_eq!(fake.submits, [3]);
    assert_eq!(world.params().len(), 1);
    assert_eq!(world.params()[0].node, filter);

    // Default methods check names against the fake's lists.
    assert!(matches!(fake.param::<f32>(filter, "nope"), Err(AudioError::UnknownParam { .. })));
    assert!(matches!(fake.param::<f32>(NodeId::next(), "cutoff"), Err(AudioError::UnknownNode(_))));

    let sender = fake.midi_sender();
    let message = MidiMessage::NoteOn { channel: Channel::new(0).unwrap(), key: 60, velocity: 1 };
    sender.send(MidiEvent { time: Timestamp::ZERO, port: PortId(0), message });
    assert_eq!(fake.midi.lock().unwrap().len(), 1);
}

#[test]
fn worlds_are_plain_data() {
    // Send + Sync + Clone + comparable: buildable on a simulation thread.
    fn assert_plain<T: Send + Sync + Clone + PartialEq + 'static>() {}
    assert_plain::<AudioWorld>();
    assert_plain::<NodeId>();
    assert_plain::<NodeDesc>();
}
