//! [`PatchEditor`]: the audio patch as a node graph you can rewire with
//! the mouse, applied to an [`AudioEngine`].
//!
//! The graph (egui-snarl's `Snarl<PatchNode>`) is the source of truth.
//! [`PatchEditor::sync`] creates, updates and destroys the engine's nodes to
//! match it, then submits an [`AudioWorld`] with its wires and knob values.
//! The engine works out what changed: rewiring rebuilds the audio graph
//! (nodes keep their state, so notes and echoes aren't cut), knob changes
//! are just sent.

use egui::{Color32, Ui, pos2};
use egui_snarl::{
    InPin, InPinId, NodeId as GraphNodeId, OutPin, OutPinId, Snarl,
    ui::{PinInfo as SnarlPinInfo, SnarlPin, SnarlStyle, SnarlViewer, SnarlWidget},
};
use enso_audio::{
    AudioEngine, AudioError, AudioWorld, InputPin, NodeDesc, NodeId, OutputPin, Param, ParamInfo, PinInfo,
    SPEAKER_INPUTS, Signal,
};
use enso_core::{Channel, PortId};

/// Offered in the "add node" menu, in this order (`None`: the speakers).
const MENU: [Option<NodeDesc>; 7] = [
    Some(NodeDesc::MidiIn { port: None, channel: None }),
    Some(NodeDesc::Transpose),
    Some(NodeDesc::TestSynth),
    Some(NodeDesc::LowPass),
    Some(NodeDesc::Delay),
    Some(NodeDesc::Gain),
    None,
];

fn signal_name(signal: Signal) -> &'static str {
    match signal {
        Signal::Midi => "midi",
        Signal::Audio => "audio",
    }
}

fn signal_pin(signal: Signal) -> SnarlPinInfo {
    match signal {
        Signal::Midi => SnarlPinInfo::square().with_fill(Color32::from_rgb(235, 165, 60)),
        Signal::Audio => SnarlPinInfo::circle().with_fill(Color32::from_rgb(90, 170, 240)),
    }
}

/// One node of the editor's graph.
pub struct PatchNode {
    /// `None`: the speakers (an "Output" node; there may be several, all
    /// standing for [`NodeId::SPEAKERS`]).
    desc: Option<NodeDesc>,
    /// The engine's node, once [`PatchEditor::sync`] has created it.
    id: Option<NodeId>,
    /// The configuration the engine has, to notice changes (e.g. the MIDI
    /// channel picked in the node).
    applied: Option<NodeDesc>,
    /// Current knob values (what the sliders show).
    knobs: Vec<(ParamInfo, f32)>,
}

impl PatchNode {
    pub fn new(desc: Option<NodeDesc>) -> Self {
        let knobs = desc.map_or(&[][..], |d| d.params()).iter().map(|p| (p.clone(), p.default)).collect();
        Self { desc, id: None, applied: None, knobs }
    }

    /// `None` for the speakers.
    pub fn desc(&self) -> Option<NodeDesc> {
        self.desc
    }

    fn title(&self) -> &'static str {
        self.desc.map_or("Output", |d| d.title())
    }

    fn inputs(&self) -> &'static [PinInfo] {
        self.desc.map_or(SPEAKER_INPUTS, |d| d.inputs())
    }

    fn outputs(&self) -> &'static [PinInfo] {
        self.desc.map_or(&[], |d| d.outputs())
    }
}

/// A MIDI input the editor can offer in "MIDI in" nodes.
#[derive(Debug, Clone)]
pub struct MidiPort {
    pub id: PortId,
    pub name: String,
}

/// The patch editor: show it every frame, then [`sync`](Self::sync) it.
pub struct PatchEditor {
    graph: Snarl<PatchNode>,
    /// Engine nodes this editor created (to destroy those whose graph
    /// node was removed).
    created: Vec<NodeId>,
    /// Refilled at every sync (keeps its memory).
    world: AudioWorld,
    error: Option<String>,
    style: SnarlStyle,
}

impl Default for PatchEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl PatchEditor {
    /// An empty graph.
    pub fn new() -> Self {
        let style = SnarlStyle { collapsible: Some(true), ..SnarlStyle::new() };
        Self { graph: Snarl::new(), created: Vec::new(), world: AudioWorld::new(), error: None, style }
    }

    /// A playable starting point:
    /// MIDI in → test synth → low-pass → delay → gain → output.
    pub fn with_starter_patch() -> Self {
        let mut editor = Self::new();
        let chain = [
            Some(NodeDesc::MidiIn { port: None, channel: None }),
            Some(NodeDesc::TestSynth),
            Some(NodeDesc::LowPass),
            Some(NodeDesc::Delay),
            Some(NodeDesc::Gain),
            None,
        ];
        let mut previous: Option<GraphNodeId> = None;
        for (i, desc) in chain.into_iter().enumerate() {
            let at = pos2(40.0 + 230.0 * i as f32, 120.0 + 40.0 * (i % 2) as f32);
            let node = editor.graph.insert_node(at, PatchNode::new(desc));
            if let Some(previous) = previous {
                editor.graph.connect(OutPinId { node: previous, output: 0 }, InPinId { node, input: 0 });
            }
            previous = Some(node);
        }
        editor
    }

    /// Why the graph couldn't be applied (e.g. a cycle), if it couldn't.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Draw the graph. Right-click the background to add nodes, right-click
    /// a node to remove it, drag between pins to wire.
    pub fn show(&mut self, ui: &mut Ui, ports: &[MidiPort]) {
        let mut viewer = Viewer { ports };
        SnarlWidget::new().id_salt("patch-editor").style(self.style).show(&mut self.graph, &mut viewer, ui);
    }

    /// Apply the graph to `audio`: create, update and destroy nodes, then
    /// submit the wiring and knob values. Call it every frame; unchanged
    /// graphs cost little.
    pub fn sync(&mut self, audio: &mut impl AudioEngine) {
        let mut problem = None;

        // Nodes.
        let mut live = Vec::new();
        for node in self.graph.nodes_mut() {
            let Some(desc) = node.desc else {
                node.id = Some(NodeId::SPEAKERS);
                continue;
            };
            match node.id {
                None => match audio.create_node(&desc) {
                    Ok(id) => {
                        node.id = Some(id);
                        node.applied = Some(desc);
                        self.created.push(id);
                    }
                    Err(err) => problem = Some(err),
                },
                Some(id) if node.applied != Some(desc) => {
                    node.applied = Some(desc);
                    if let Err(err) = audio.update_node(id, &desc) {
                        problem = Some(err);
                    }
                }
                Some(_) => {}
            }
            live.extend(node.id);
        }
        self.created.retain(|id| {
            let keep = live.contains(id);
            if !keep {
                audio.destroy_node(*id);
            }
            keep
        });

        // Wires and knobs.
        self.world.clear();
        for (out, input) in self.graph.wires() {
            let (Some(from), Some(to)) = (self.graph.get_node(out.node), self.graph.get_node(input.node)) else {
                continue;
            };
            let (Some(from), Some(to)) = (from.id, to.id) else { continue };
            self.world.connect(
                OutputPin { node: from, index: out.output as u16 },
                InputPin { node: to, index: input.input as u16 },
            );
        }
        for node in self.graph.nodes() {
            let Some(id) = node.id else { continue };
            for (info, value) in &node.knobs {
                self.world.set_param(id, Param::<f32>::new(&info.name), *value);
            }
        }

        match audio.submit(&self.world) {
            Ok(()) => {}
            // Transient: the audio thread will catch up; keep what's shown.
            Err(AudioError::Busy) => return,
            Err(err) => problem = Some(err),
        }
        self.error = problem.map(|err| err.to_string());
    }
}

/// Tells egui-snarl how to draw and edit `PatchNode`s.
struct Viewer<'a> {
    ports: &'a [MidiPort],
}

impl SnarlViewer<PatchNode> for Viewer<'_> {
    fn title(&mut self, node: &PatchNode) -> String {
        node.title().to_owned()
    }

    fn inputs(&mut self, node: &PatchNode) -> usize {
        node.inputs().len()
    }

    fn outputs(&mut self, node: &PatchNode) -> usize {
        node.outputs().len()
    }

    fn show_input(&mut self, pin: &InPin, ui: &mut Ui, snarl: &mut Snarl<PatchNode>) -> impl SnarlPin + 'static {
        let signal = snarl[pin.id.node].inputs()[pin.id.input].signal;
        ui.label(signal_name(signal));
        signal_pin(signal)
    }

    fn show_output(&mut self, pin: &OutPin, ui: &mut Ui, snarl: &mut Snarl<PatchNode>) -> impl SnarlPin + 'static {
        let signal = snarl[pin.id.node].outputs()[pin.id.output].signal;
        ui.label(signal_name(signal));
        signal_pin(signal)
    }

    /// Only same-signal pins connect.
    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<PatchNode>) {
        let out = snarl[from.id.node].outputs()[from.id.output].signal;
        let into = snarl[to.id.node].inputs()[to.id.input].signal;
        if out == into {
            snarl.connect(from.id, to.id);
        }
    }

    fn has_body(&mut self, node: &PatchNode) -> bool {
        !node.knobs.is_empty() || matches!(node.desc, Some(NodeDesc::MidiIn { .. }))
    }

    fn show_body(
        &mut self,
        node: GraphNodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<PatchNode>,
    ) {
        let patch_node = &mut snarl[node];
        ui.vertical(|ui| {
            if let Some(NodeDesc::MidiIn { port, channel }) = &mut patch_node.desc {
                midi_filter_ui(ui, node, self.ports, port, channel);
            }
            for (info, value) in &mut patch_node.knobs {
                let slider = egui::Slider::new(value, info.min..=info.max)
                    .logarithmic(info.logarithmic)
                    .suffix(info.unit.as_ref())
                    .text(info.label.as_ref());
                ui.add(slider);
            }
        });
    }

    fn has_graph_menu(&mut self, _pos: egui::Pos2, _snarl: &mut Snarl<PatchNode>) -> bool {
        true
    }

    fn show_graph_menu(&mut self, pos: egui::Pos2, ui: &mut Ui, snarl: &mut Snarl<PatchNode>) {
        ui.label("Add node");
        for desc in MENU {
            let node = PatchNode::new(desc);
            if ui.button(node.title()).clicked() {
                snarl.insert_node(pos, node);
                ui.close();
            }
        }
    }

    fn has_node_menu(&mut self, _node: &PatchNode) -> bool {
        true
    }

    fn show_node_menu(
        &mut self,
        node: GraphNodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<PatchNode>,
    ) {
        if ui.button("Remove").clicked() {
            // Its engine node is destroyed at the next `sync`.
            snarl.remove_node(node);
            ui.close();
        }
    }
}

/// Device and channel pickers of a "MIDI in" node.
fn midi_filter_ui(
    ui: &mut Ui,
    node: GraphNodeId,
    ports: &[MidiPort],
    port: &mut Option<PortId>,
    channel: &mut Option<Channel>,
) {
    let port_name = match port {
        None => "any device".to_owned(),
        Some(id) => ports.iter().find(|p| p.id == *id).map_or("(unplugged)".to_owned(), |p| p.name.clone()),
    };
    egui::ComboBox::from_id_salt(("midi-port", node)).selected_text(port_name).show_ui(ui, |ui| {
        ui.selectable_value(port, None, "any device");
        for p in ports {
            ui.selectable_value(port, Some(p.id), &p.name);
        }
    });

    let channel_name = channel.map_or("any channel".to_owned(), |c| format!("channel {}", c.number()));
    egui::ComboBox::from_id_salt(("midi-channel", node)).selected_text(channel_name).show_ui(ui, |ui| {
        ui.selectable_value(channel, None, "any channel");
        for index in 0..16 {
            let c = Channel::new(index);
            ui.selectable_value(channel, c, format!("channel {}", index + 1));
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use enso_audio::{AudioConfig, BlockAudioEngine};
    use enso_core::{MidiEvent, MidiMessage, Timestamp};

    use super::*;

    const CONFIG: AudioConfig = AudioConfig { sample_rate: 48_000, max_block: 512 };

    /// Renders `buffers` 10 ms buffers from `first`; returns the peak level.
    fn peak(audio: &mut BlockAudioEngine, first: u64, buffers: u64) -> f32 {
        let mut out = vec![0.0; 480 * 2];
        let mut peak = 0.0f32;
        for i in first..first + buffers {
            audio.render(&mut out, 2, Timestamp::ZERO + Duration::from_millis(1000 + 10 * i));
            audio.collect_garbage();
            peak = out.iter().fold(peak, |p, s| p.max(s.abs()));
        }
        peak
    }

    fn note(key: u8) -> MidiEvent {
        let message = MidiMessage::NoteOn { channel: Channel::new(0).unwrap(), key, velocity: 100 };
        MidiEvent { time: Timestamp::ZERO + Duration::from_millis(1000), port: PortId(3), message }
    }

    fn find(editor: &PatchEditor, desc: Option<NodeDesc>) -> GraphNodeId {
        editor.graph.node_ids().find(|(_, n)| n.desc == desc).unwrap().0
    }

    #[test]
    fn starter_patch_plays() {
        let mut editor = PatchEditor::with_starter_patch();
        let mut audio = BlockAudioEngine::new(CONFIG);
        editor.sync(&mut audio);
        assert_eq!(editor.error(), None);
        audio.midi_sender().send(note(60));
        assert!(peak(&mut audio, 0, 20) > 0.01);
    }

    #[test]
    fn rewiring_keeps_sounding() {
        let mut editor = PatchEditor::with_starter_patch();
        let mut audio = BlockAudioEngine::new(CONFIG);
        editor.sync(&mut audio);
        audio.midi_sender().send(note(60));
        peak(&mut audio, 0, 5);

        // Remove the delay: its node is destroyed and the chain is broken.
        editor.graph.remove_node(find(&editor, Some(NodeDesc::Delay)));
        editor.sync(&mut audio);
        assert_eq!(editor.error(), None);
        assert_eq!(editor.created.len(), 4);
        assert!(peak(&mut audio, 5, 5) < 1e-3, "the gain lost its input");

        // Wire filter → gain directly: sound again, from the same note.
        let (filter, gain) = (find(&editor, Some(NodeDesc::LowPass)), find(&editor, Some(NodeDesc::Gain)));
        editor.graph.connect(OutPinId { node: filter, output: 0 }, InPinId { node: gain, input: 0 });
        editor.sync(&mut audio);
        assert!(peak(&mut audio, 10, 5) > 0.01, "the synth's voice survived two rebuilds");

        // A cycle: rejected, reported, and the old patch keeps playing.
        editor.graph.connect(OutPinId { node: gain, output: 0 }, InPinId { node: filter, input: 0 });
        editor.sync(&mut audio);
        assert!(editor.error().is_some_and(|e| e.contains("cycle")), "{:?}", editor.error());
        assert!(peak(&mut audio, 15, 5) > 0.01);
    }

    #[test]
    fn knobs_reach_their_nodes() {
        let mut editor = PatchEditor::with_starter_patch();
        let mut audio = BlockAudioEngine::new(CONFIG);
        let gain = find(&editor, Some(NodeDesc::Gain));
        editor.graph[gain].knobs[0].1 = -60.0;
        editor.sync(&mut audio);
        audio.midi_sender().send(note(60));
        peak(&mut audio, 0, 20); // let the gain glide down
        assert!(peak(&mut audio, 20, 5) < 1e-3, "-60 dB silences the patch");
    }
}
