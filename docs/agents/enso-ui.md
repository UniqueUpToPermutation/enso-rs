# enso-ui and the enso app

## enso-ui: `PatchEditor`

A node-graph editor (egui-snarl, vendored; see `third_party/README.md`)
for the audio engine's patch.

- `Snarl<PatchNode>` holds the layout and wiring. A `PatchNode` has a
  `NodeKind`, its knobs (each a `Knob` bound to an `enso_audio::SharedParam`),
  and `engine: Option<NodeId>`.
- `show(ui, ports)` draws the graph through a `Viewer` that implements
  `SnarlViewer`:
  - Pins are typed (`Signal::Midi` squares, `Signal::Audio` circles), and
    `connect` refuses mismatched ones.
  - Right-click the background for the add-node menu (`NodeKind::MENU`),
    and a node for "Remove".
  - Bodies hold the knob sliders and, for MIDI in nodes, the
    device/channel pickers.
- `sync(engine)` runs after `show`, every frame:
  1. It creates engine nodes for graph nodes that don't have one yet.
  2. It removes engine nodes whose graph node is gone (`known` vs. alive),
     so removal works however the graph node was deleted.
  3. It builds a `Patch` from the wires and sends it **only if it
     differs** from `engine.patch()`. *MIDI in* becomes
     `MidiFrom::Input(filter)`, and *Output* becomes `patch.output(..)`;
     neither has an engine node. Errors such as cycles go to `error()`,
     and the old patch keeps playing.
- Knob values reach the audio thread directly through `SharedParam`
  atomics (lock-free, latest value wins). Nodes smooth them (`Smoothed`).

### Adding a node kind

1. Add the node in `enso-audio/src/nodes/`, with its knobs as `SharedParam`s.
2. Add it to `enso-audio/tests/no_alloc.rs`.
3. In `patch_editor.rs`: add a `NodeKind` variant, then fill in `MENU`,
   `title`, `inputs`, `outputs`, the knobs in `PatchNode::new`, and
   `instantiate`.

Tests (`patch_editor::tests`) build graphs in code, sync them into an
offline engine, and render to check sound, deletion and cycles.

## crates/enso: the app

`cargo run -p enso [-- <output name>]`. eframe 0.37 (wgpu backend, the
same wgpu 30 as the renderer). It opens the audio output, connects every
MIDI input to the engine's queue, and shows the editor with a starter patch
(MIDI in → test synth → low-pass → delay → gain → output). A status bar
shows the device, buffer size, load, MIDI inputs and errors. If there's no
audio device, the editor still runs with a detached engine.

Not yet: saving/loading patches (snarl has a `serde` feature), the visual
renderer in the window, MIDI learn.
