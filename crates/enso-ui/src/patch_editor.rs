//! [`PatchEditor`]: the audio patch as a node graph you can rewire with
//! the mouse, applied to the running audio workstream.
//!
//! The graph (egui-snarl's `Snarl<PatchNode>`) is the source of truth.
//! [`PatchEditor::sync`] turns it into a [`PatchSpec`] after every frame and,
//! when that changed, rebuilds the audio workstream from it
//! (`Engine::rebuild`). Blocks that survive keep their state, so rewiring
//! doesn't cut notes or echoes. Knob changes don't rebuild: they're sent
//! with `Engine::set_id`.

use egui::{Color32, Ui, pos2};
use egui_snarl::{
    InPin, InPinId, NodeId as GraphNodeId, OutPin, OutPinId, Snarl,
    ui::{PinInfo, SnarlPin, SnarlStyle, SnarlViewer, SnarlWidget},
};
use enso_audio::{
    AudioConfig,
    enso_block::{BlockId, Engine, InterfaceId, WorkstreamId},
};
use enso_core::{Channel, PortId};

use crate::patch::{Endpoint, KnobSpec, NodeKind, PatchRoot, PatchSpec, Signal};

impl Signal {
    fn name(self) -> &'static str {
        match self {
            Signal::Midi => "midi",
            Signal::Audio => "audio",
        }
    }

    fn pin(self) -> PinInfo {
        match self {
            Signal::Midi => PinInfo::square().with_fill(Color32::from_rgb(235, 165, 60)),
            Signal::Audio => PinInfo::circle().with_fill(Color32::from_rgb(90, 170, 240)),
        }
    }
}

/// One node of the editor's graph.
pub struct PatchNode {
    kind: NodeKind,
    /// The UUID of the node's block: stable for the node's lifetime, so the
    /// block keeps its state across rebuilds.
    id: BlockId,
    /// Current knob values (what the sliders show).
    knobs: Vec<(KnobSpec, f32)>,
}

impl PatchNode {
    pub fn new(kind: NodeKind) -> Self {
        Self { kind, id: BlockId::random(), knobs: kind.knobs().into_iter().map(|k| (k, k.default)).collect() }
    }

    pub fn kind(&self) -> NodeKind {
        self.kind
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
    config: AudioConfig,
    graph: Snarl<PatchNode>,
    /// The spec last sent to the engine (or attempted: a failing spec isn't
    /// retried every frame).
    attempted: Option<PatchSpec>,
    /// Knob changes made in the UI, sent at the next `sync`.
    knob_changes: Vec<(InterfaceId, f32)>,
    error: Option<String>,
    style: SnarlStyle,
}

impl PatchEditor {
    /// An empty graph.
    pub fn new(config: AudioConfig) -> Self {
        let style = SnarlStyle { collapsible: Some(true), ..SnarlStyle::new() };
        Self { config, graph: Snarl::new(), attempted: None, knob_changes: Vec::new(), error: None, style }
    }

    /// A playable starting point:
    /// MIDI in → test synth → low-pass → delay → gain → output.
    pub fn with_starter_patch(config: AudioConfig) -> Self {
        let mut editor = Self::new(config);
        let chain = [
            NodeKind::MidiIn { port: None, channel: None },
            NodeKind::TestSynth,
            NodeKind::LowPass,
            NodeKind::Delay,
            NodeKind::Gain,
            NodeKind::Output,
        ];
        let mut previous: Option<GraphNodeId> = None;
        for (i, kind) in chain.into_iter().enumerate() {
            let at = pos2(40.0 + 230.0 * i as f32, 120.0 + 40.0 * (i % 2) as f32);
            let node = editor.graph.insert_node(at, PatchNode::new(kind));
            if let Some(previous) = previous {
                editor.graph.connect(OutPinId { node: previous, output: 0 }, InPinId { node, input: 0 });
            }
            previous = Some(node);
        }
        editor
    }

    /// The root block for the graph as it is now: build the audio
    /// workstream from this at startup.
    pub fn root(&mut self) -> PatchRoot {
        let spec = self.spec();
        self.attempted = Some(spec.clone());
        PatchRoot::new(self.config, spec)
    }

    /// Why the last change couldn't be applied (e.g. a cycle), if it couldn't.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Draw the graph. Right-click the background to add nodes, right-click
    /// a node to remove it, drag between pins to wire.
    pub fn show(&mut self, ui: &mut Ui, ports: &[MidiPort]) {
        let mut viewer = Viewer { ports, knob_changes: &mut self.knob_changes };
        SnarlWidget::new().id_salt("patch-editor").style(self.style).show(&mut self.graph, &mut viewer, ui);
    }

    /// Apply the graph to the audio workstream `audio`: rebuild it if the
    /// wiring or the set of nodes changed, and send knob changes.
    pub fn sync(&mut self, engine: &mut Engine, audio: WorkstreamId) {
        let spec = self.spec();
        if self.attempted.as_ref() != Some(&spec) {
            self.attempted = Some(spec.clone());
            match engine.rebuild(audio, PatchRoot::new(self.config, spec)) {
                Ok(()) => self.error = None,
                Err(err) => self.error = Some(err.to_string()),
            }
        }
        for (id, value) in self.knob_changes.drain(..) {
            // A knob of a node whose rebuild failed isn't there yet: ignore.
            if let Err(err) = engine.set_id(id, value) {
                log::debug!("knob not applied: {err}");
            }
        }
    }

    /// What the graph describes.
    fn spec(&self) -> PatchSpec {
        let nodes = self.graph.nodes().filter(|n| n.kind != NodeKind::Output).map(|n| (n.id, n.kind)).collect();
        let wires = self
            .graph
            .wires()
            .filter_map(|(out, input)| {
                let (from, to) = (self.graph.get_node(out.node)?, self.graph.get_node(input.node)?);
                let from = Endpoint::Pin(from.id, from.kind.outputs().get(out.output)?.key);
                let to = match to.kind {
                    NodeKind::Output => Endpoint::Output,
                    kind => Endpoint::Pin(to.id, kind.inputs().get(input.input)?.key),
                };
                Some((from, to))
            })
            .collect();
        PatchSpec { nodes, wires }
    }
}

/// Tells egui-snarl how to draw and edit `PatchNode`s.
struct Viewer<'a> {
    ports: &'a [MidiPort],
    knob_changes: &'a mut Vec<(InterfaceId, f32)>,
}

impl SnarlViewer<PatchNode> for Viewer<'_> {
    fn title(&mut self, node: &PatchNode) -> String {
        node.kind.title().to_owned()
    }

    fn inputs(&mut self, node: &PatchNode) -> usize {
        node.kind.inputs().len()
    }

    fn outputs(&mut self, node: &PatchNode) -> usize {
        node.kind.outputs().len()
    }

    fn show_input(&mut self, pin: &InPin, ui: &mut Ui, snarl: &mut Snarl<PatchNode>) -> impl SnarlPin + 'static {
        let signal = snarl[pin.id.node].kind.inputs()[pin.id.input].signal;
        ui.label(signal.name());
        signal.pin()
    }

    fn show_output(&mut self, pin: &OutPin, ui: &mut Ui, snarl: &mut Snarl<PatchNode>) -> impl SnarlPin + 'static {
        let signal = snarl[pin.id.node].kind.outputs()[pin.id.output].signal;
        ui.label(signal.name());
        signal.pin()
    }

    /// Only same-kind pins connect.
    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<PatchNode>) {
        let out = snarl[from.id.node].kind.outputs()[from.id.output].signal;
        let into = snarl[to.id.node].kind.inputs()[to.id.input].signal;
        if out == into {
            snarl.connect(from.id, to.id);
        }
    }

    fn has_body(&mut self, node: &PatchNode) -> bool {
        !node.knobs.is_empty() || matches!(node.kind, NodeKind::MidiIn { .. })
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
        let block = patch_node.id;
        ui.vertical(|ui| {
            if let NodeKind::MidiIn { port, channel } = &mut patch_node.kind {
                midi_filter_ui(ui, node, self.ports, port, channel);
            }
            for (spec, value) in &mut patch_node.knobs {
                let slider = egui::Slider::new(value, spec.range.0..=spec.range.1)
                    .logarithmic(spec.logarithmic)
                    .suffix(spec.suffix)
                    .text(spec.label);
                if ui.add(slider).changed() {
                    self.knob_changes.push((InterfaceId { block, key: spec.key }, *value));
                }
            }
        });
    }

    fn has_graph_menu(&mut self, _pos: egui::Pos2, _snarl: &mut Snarl<PatchNode>) -> bool {
        true
    }

    fn show_graph_menu(&mut self, pos: egui::Pos2, ui: &mut Ui, snarl: &mut Snarl<PatchNode>) {
        ui.label("Add node");
        for kind in NodeKind::MENU {
            if ui.button(kind.title()).clicked() {
                snarl.insert_node(pos, PatchNode::new(kind));
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
            // Its block goes at the next `sync` (the rebuild leaves it out).
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

    use enso_audio::{AudioDriver, enso_block::Workstream};
    use enso_core::{MidiEvent, MidiMessage, Timestamp};

    use super::*;

    const CONFIG: AudioConfig = AudioConfig { sample_rate: 48_000, max_block: 512 };

    /// Build the editor's patch into an engine and an offline driver.
    fn run(editor: &mut PatchEditor) -> (Engine, WorkstreamId, AudioDriver, enso_audio::enso_block::MailSender<MidiEvent>) {
        let mut engine = Engine::new();
        let id = engine.add("audio", Workstream::build_realtime(editor.root()).unwrap());
        let root = engine.root::<PatchRoot>(id).unwrap();
        let midi = engine.sender(engine.get(root.source).unwrap().input).unwrap();
        let output = engine.get(root.output).unwrap().input;
        let driver = AudioDriver::new(engine.take(id).unwrap(), output, CONFIG);
        (engine, id, driver, midi)
    }

    /// Renders `buffers` 10 ms buffers from `first`; returns the peak level.
    fn peak(driver: &mut AudioDriver, first: u64, buffers: u64) -> f32 {
        let mut out = vec![0.0; 480 * 2];
        let mut peak = 0.0f32;
        for i in first..first + buffers {
            driver.render(&mut out, 2, Timestamp::ZERO + Duration::from_millis(1000 + 10 * i));
            peak = out.iter().fold(peak, |p, s| p.max(s.abs()));
        }
        peak
    }

    fn note(key: u8) -> MidiEvent {
        let message = MidiMessage::NoteOn { channel: Channel::new(0).unwrap(), key, velocity: 100 };
        MidiEvent { time: Timestamp::ZERO + Duration::from_millis(1000), port: PortId(3), message }
    }

    #[test]
    fn starter_patch_plays() {
        let mut editor = PatchEditor::with_starter_patch(CONFIG);
        let (_engine, _, mut driver, midi) = run(&mut editor);
        midi.send(note(60));
        assert!(peak(&mut driver, 0, 20) > 0.01);
    }

    #[test]
    fn rewiring_rebuilds_and_keeps_sounding() {
        let mut editor = PatchEditor::with_starter_patch(CONFIG);
        let (mut engine, id, mut driver, midi) = run(&mut editor);
        midi.send(note(60));
        peak(&mut driver, 0, 5);

        // Remove the delay: a rebuild; the synth's note keeps sounding,
        // but the chain is broken (filter no longer reaches the output).
        let delay = editor.graph.node_ids().find(|(_, n)| n.kind == NodeKind::Delay).unwrap().0;
        editor.graph.remove_node(delay);
        editor.sync(&mut engine, id);
        assert_eq!(editor.error(), None);
        assert!(peak(&mut driver, 5, 5) < 1e-3, "the gain lost its input");

        // Wire filter → gain directly: sound again, from the same note.
        let find = |kind| editor.graph.node_ids().find(|(_, n)| n.kind == kind).unwrap().0;
        let (filter, gain) = (find(NodeKind::LowPass), find(NodeKind::Gain));
        editor.graph.connect(OutPinId { node: filter, output: 0 }, InPinId { node: gain, input: 0 });
        editor.sync(&mut engine, id);
        assert!(peak(&mut driver, 10, 5) > 0.01, "the synth's voice survived two rebuilds");

        // A cycle: rejected, reported, and the old patch keeps playing.
        editor.graph.connect(OutPinId { node: gain, output: 0 }, InPinId { node: filter, input: 0 });
        editor.sync(&mut engine, id);
        assert!(editor.error().is_some_and(|e| e.contains("cycle")), "{:?}", editor.error());
        assert!(peak(&mut driver, 15, 5) > 0.01);
    }

    #[test]
    fn knobs_reach_their_blocks() {
        let mut editor = PatchEditor::with_starter_patch(CONFIG);
        let (mut engine, id, mut driver, midi) = run(&mut editor);
        let gain = editor.graph.nodes().find(|n| n.kind == NodeKind::Gain).unwrap();
        let volume = InterfaceId { block: gain.id, key: gain.knobs[0].0.key };
        editor.knob_changes.push((volume, -60.0));
        editor.sync(&mut engine, id);
        midi.send(note(60));
        peak(&mut driver, 0, 20); // let the gain glide down
        assert!(peak(&mut driver, 20, 5) < 1e-3, "-60 dB silences the patch");
    }
}
