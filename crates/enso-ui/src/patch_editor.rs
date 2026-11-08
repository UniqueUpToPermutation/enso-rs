//! [`PatchEditor`]: the audio patch as a node graph you can rewire with
//! the mouse, applied to the engine live.
//!
//! The graph (egui-snarl's `Snarl<PatchNode>`) is the source of truth for
//! the *layout*; [`PatchEditor::sync`] turns it into engine nodes and an
//! `enso_audio::Patch` after every frame, sending a new patch only when
//! the wiring actually changed.

use std::ops::RangeInclusive;

use egui::{Color32, Ui, pos2};
use egui_snarl::{
    InPin, InPinId, NodeId as GraphNodeId, OutPin, OutPinId, Snarl,
    ui::{PinInfo, SnarlPin, SnarlStyle, SnarlViewer, SnarlWidget},
};
use enso_audio::{
    AudioEngine, MidiFilter, MidiFrom, NodeId, Patch, SharedParam,
    nodes::{Delay, Gain, LowPass, TestSynth, Transpose},
};
use enso_core::{Channel, PortId};

/// What flows through a pin. Wires only connect pins of the same kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Midi,
    Audio,
}

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

/// The kinds of node the editor offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// MIDI from the outside world, optionally one device or channel.
    MidiIn { port: Option<PortId>, channel: Option<Channel> },
    /// The speakers.
    Output,
    TestSynth,
    Transpose,
    Gain,
    LowPass,
    Delay,
}

impl NodeKind {
    /// Offered in the "add node" menu, in this order.
    const MENU: [NodeKind; 7] = [
        NodeKind::MidiIn { port: None, channel: None },
        NodeKind::Transpose,
        NodeKind::TestSynth,
        NodeKind::LowPass,
        NodeKind::Delay,
        NodeKind::Gain,
        NodeKind::Output,
    ];

    fn title(self) -> &'static str {
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

    fn inputs(self) -> &'static [Signal] {
        match self {
            NodeKind::MidiIn { .. } => &[],
            NodeKind::Transpose | NodeKind::TestSynth => &[Signal::Midi],
            NodeKind::Gain | NodeKind::LowPass | NodeKind::Delay | NodeKind::Output => &[Signal::Audio],
        }
    }

    fn outputs(self) -> &'static [Signal] {
        match self {
            NodeKind::MidiIn { .. } | NodeKind::Transpose => &[Signal::Midi],
            NodeKind::TestSynth | NodeKind::Gain | NodeKind::LowPass | NodeKind::Delay => &[Signal::Audio],
            NodeKind::Output => &[],
        }
    }
}

/// A slider bound to a value the audio node reads.
struct Knob {
    name: &'static str,
    range: RangeInclusive<f32>,
    logarithmic: bool,
    suffix: &'static str,
    param: SharedParam,
}

impl Knob {
    fn new(name: &'static str, range: RangeInclusive<f32>, value: f32, suffix: &'static str) -> Self {
        Self { name, range, logarithmic: false, suffix, param: SharedParam::new(value) }
    }

    fn logarithmic(mut self) -> Self {
        self.logarithmic = true;
        self
    }

    fn show(&self, ui: &mut Ui) {
        let mut value = self.param.get();
        let slider = egui::Slider::new(&mut value, self.range.clone())
            .logarithmic(self.logarithmic)
            .suffix(self.suffix)
            .text(self.name);
        if ui.add(slider).changed() {
            self.param.set(value);
        }
    }
}

/// One node of the editor's graph.
pub struct PatchNode {
    kind: NodeKind,
    knobs: Vec<Knob>,
    /// The engine node, once created ([`PatchEditor::sync`]). MIDI in and
    /// Output have none: they become patch endpoints instead.
    engine: Option<NodeId>,
}

impl PatchNode {
    pub fn new(kind: NodeKind) -> Self {
        let knobs = match kind {
            NodeKind::Transpose => vec![Knob::new("shift", -24.0..=24.0, 0.0, " st")],
            NodeKind::Gain => vec![Knob::new("volume", -60.0..=12.0, -6.0, " dB")],
            NodeKind::LowPass => vec![
                Knob::new("cutoff", 20.0..=20_000.0, 2_500.0, " Hz").logarithmic(),
                Knob::new("resonance", 0.0..=1.0, 0.3, ""),
            ],
            NodeKind::Delay => vec![
                Knob::new("time", 0.01..=Delay::MAX_SECONDS, 0.35, " s").logarithmic(),
                Knob::new("feedback", 0.0..=0.95, 0.4, ""),
                Knob::new("mix", 0.0..=1.0, 0.3, ""),
            ],
            NodeKind::MidiIn { .. } | NodeKind::Output | NodeKind::TestSynth => Vec::new(),
        };
        Self { kind, knobs, engine: None }
    }

    pub fn kind(&self) -> NodeKind {
        self.kind
    }

    /// Create this node's engine node, if it has one.
    fn instantiate(&self, engine: &mut AudioEngine) -> Option<Result<NodeId, enso_audio::EngineError>> {
        let config = engine.config();
        let knob = |i: usize| self.knobs[i].param.clone();
        Some(match self.kind {
            NodeKind::MidiIn { .. } | NodeKind::Output => return None,
            NodeKind::TestSynth => engine.add_node(TestSynth::new(config)),
            NodeKind::Transpose => engine.add_node(Transpose::new(knob(0))),
            NodeKind::Gain => engine.add_node(Gain::new(config, knob(0))),
            NodeKind::LowPass => engine.add_node(LowPass::new(config, knob(0), knob(1))),
            NodeKind::Delay => engine.add_node(Delay::new(config, knob(0), knob(1), knob(2))),
        })
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
    /// Engine nodes created so far, to notice deleted graph nodes.
    known: Vec<NodeId>,
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
        Self { graph: Snarl::new(), known: Vec::new(), error: None, style }
    }

    /// A playable starting point:
    /// MIDI in → test synth → low-pass → delay → gain → output.
    pub fn with_starter_patch() -> Self {
        let mut editor = Self::new();
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
            let node = editor.graph.insert_node(pos2(40.0 + 230.0 * i as f32, 120.0 + 40.0 * (i % 2) as f32), PatchNode::new(kind));
            if let Some(previous) = previous {
                editor.graph.connect(OutPinId { node: previous, output: 0 }, InPinId { node, input: 0 });
            }
            previous = Some(node);
        }
        editor
    }

    /// Why the last change couldn't be applied (e.g. a cycle), if it couldn't.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Draw the graph. Right-click the background to add nodes, right-click
    /// a node to remove it, drag between pins to wire.
    pub fn show(&mut self, ui: &mut Ui, ports: &[MidiPort]) {
        let mut viewer = Viewer { ports };
        SnarlWidget::new().id_salt("patch-editor").style(self.style).show(&mut self.graph, &mut viewer, ui);
    }

    /// Bring the engine in line with the graph: create engine nodes for new
    /// graph nodes, remove those whose graph node was deleted, and send the
    /// wiring as a new patch if it changed.
    pub fn sync(&mut self, engine: &mut AudioEngine) {
        // New nodes.
        for node in self.graph.nodes_mut() {
            if node.engine.is_none() {
                match node.instantiate(engine) {
                    Some(Ok(id)) => {
                        node.engine = Some(id);
                        self.known.push(id);
                    }
                    Some(Err(err)) => self.error = Some(format!("couldn't add {}: {err}", node.kind.title())),
                    None => {}
                }
            }
        }

        // Deleted nodes.
        let alive: Vec<NodeId> = self.graph.nodes().filter_map(|n| n.engine).collect();
        let (keep, gone): (Vec<_>, Vec<_>) = self.known.iter().partition(|id| alive.contains(id));
        self.known = keep;
        for id in gone {
            if let Err(err) = engine.remove_node(id) {
                log::warn!("removing node: {err}");
            }
        }

        // Wiring.
        let patch = self.patch();
        if &patch != engine.patch() {
            match engine.set_patch(patch) {
                Ok(()) => self.error = None,
                Err(err) => self.error = Some(err.to_string()),
            }
        }
    }

    /// The engine patch the graph's wires describe.
    fn patch(&self) -> Patch {
        let mut patch = Patch::new();
        for (out, input) in self.graph.wires() {
            let (Some(from), Some(to)) = (self.graph.get_node(out.node), self.graph.get_node(input.node)) else {
                continue;
            };
            match from.kind.outputs().get(out.output) {
                Some(Signal::Midi) => {
                    let source = match from.kind {
                        NodeKind::MidiIn { port, channel } => MidiFrom::Input(MidiFilter { port, channel }),
                        _ => match from.engine {
                            Some(id) => MidiFrom::Node(id),
                            None => continue,
                        },
                    };
                    if let Some(to) = to.engine {
                        patch.midi(source, to);
                    }
                }
                Some(Signal::Audio) => {
                    let Some(from) = from.engine else { continue };
                    match (to.kind, to.engine) {
                        (NodeKind::Output, _) => {
                            patch.output(from);
                        }
                        (_, Some(to)) => {
                            patch.audio(from, to);
                        }
                        _ => {}
                    }
                }
                None => {}
            }
        }
        patch
    }
}

/// Tells egui-snarl how to draw and edit `PatchNode`s.
struct Viewer<'a> {
    ports: &'a [MidiPort],
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
        let signal = snarl[pin.id.node].kind.inputs()[pin.id.input];
        ui.label(signal.name());
        signal.pin()
    }

    fn show_output(&mut self, pin: &OutPin, ui: &mut Ui, snarl: &mut Snarl<PatchNode>) -> impl SnarlPin + 'static {
        let signal = snarl[pin.id.node].kind.outputs()[pin.id.output];
        ui.label(signal.name());
        signal.pin()
    }

    /// Only same-kind pins connect.
    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<PatchNode>) {
        let out = snarl[from.id.node].kind.outputs()[from.id.output];
        let into = snarl[to.id.node].kind.inputs()[to.id.input];
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
        ui.vertical(|ui| {
            if let NodeKind::MidiIn { port, channel } = &mut patch_node.kind {
                midi_filter_ui(ui, node, self.ports, port, channel);
            }
            for knob in &patch_node.knobs {
                knob.show(ui);
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
            // The engine node goes at the next `sync`.
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

    use enso_audio::{AudioConfig, Processor};
    use enso_core::{MidiEvent, MidiMessage, Timestamp};

    use super::*;

    fn engine() -> (AudioEngine, Processor) {
        AudioEngine::new(AudioConfig::new(48_000))
    }

    /// Renders `buffers` 10 ms buffers; returns the peak level.
    fn peak(processor: &mut Processor, first: u64, buffers: u64) -> f32 {
        let mut out = vec![0.0; 480 * 2];
        let mut peak = 0.0f32;
        for i in first..first + buffers {
            let now = Timestamp::ZERO + Duration::from_millis(1000 + 10 * i);
            processor.process(&mut out, 2, now);
            peak = out.iter().fold(peak, |p, s| p.max(s.abs()));
        }
        peak
    }

    #[test]
    fn starter_patch_plays() {
        let (mut engine, mut processor) = engine();
        let mut editor = PatchEditor::with_starter_patch();
        editor.sync(&mut engine);
        assert_eq!(editor.error(), None);
        // Synth, filter, delay, gain: MIDI in and Output are endpoints.
        assert_eq!(engine.patch().nodes().len(), 4);

        let message = MidiMessage::NoteOn { channel: Channel::new(0).unwrap(), key: 60, velocity: 100 };
        let time = Timestamp::ZERO + Duration::from_millis(1000);
        engine.midi_queue().push(MidiEvent { time, port: PortId(3), message }).unwrap();
        assert!(peak(&mut processor, 0, 20) > 0.01);
    }

    #[test]
    fn deleting_and_rewiring_reach_the_engine() {
        let (mut engine, mut processor) = engine();
        let mut editor = PatchEditor::with_starter_patch();
        editor.sync(&mut engine);
        let before = engine.patch().clone();

        // Unchanged graph: no new patch is sent.
        editor.sync(&mut engine);
        assert_eq!(engine.patch(), &before);

        // Remove the delay: its engine node goes, and the chain breaks.
        let delay = editor.graph.node_ids().find(|(_, n)| n.kind == NodeKind::Delay).unwrap().0;
        editor.graph.remove_node(delay);
        editor.sync(&mut engine);
        // synth → filter, and gain → output: three nodes, two islands.
        assert_eq!(engine.patch().nodes().len(), 3);
        assert_eq!(editor.known.len(), 3);
        peak(&mut processor, 0, 2); // the audio thread applies it

        // Wire a cycle: rejected, reported, and the old patch stays.
        let gain = editor.graph.node_ids().find(|(_, n)| n.kind == NodeKind::Gain).unwrap().0;
        let filter = editor.graph.node_ids().find(|(_, n)| n.kind == NodeKind::LowPass).unwrap().0;
        editor.graph.connect(OutPinId { node: filter, output: 0 }, InPinId { node: gain, input: 0 });
        editor.graph.connect(OutPinId { node: gain, output: 0 }, InPinId { node: filter, input: 0 });
        let previous = engine.patch().clone();
        editor.sync(&mut engine);
        assert!(editor.error().is_some_and(|e| e.contains("cycle")));
        assert_eq!(engine.patch(), &previous);
    }

    #[test]
    fn midi_in_filters_become_patch_inputs() {
        let (mut engine, _processor) = engine();
        let mut editor = PatchEditor::new();
        let channel = Channel::new(2);
        let input = editor.graph.insert_node(pos2(0.0, 0.0), PatchNode::new(NodeKind::MidiIn { port: Some(PortId(1)), channel }));
        let synth = editor.graph.insert_node(pos2(0.0, 0.0), PatchNode::new(NodeKind::TestSynth));
        editor.graph.connect(OutPinId { node: input, output: 0 }, InPinId { node: synth, input: 0 });
        editor.sync(&mut engine);
        let synth_id = editor.graph[synth].engine.unwrap();
        let mut expected = Patch::new();
        expected.midi(MidiFrom::Input(MidiFilter { port: Some(PortId(1)), channel }), synth_id);
        assert_eq!(engine.patch(), &expected);
    }
}
