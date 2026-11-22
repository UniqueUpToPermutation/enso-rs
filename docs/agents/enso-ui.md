# enso-ui and the enso app

## enso-ui

A node-graph editor (egui-snarl, vendored; see `third_party/README.md`)
for the audio patch, written against the `enso_audio::AudioEngine` trait
only (no blocks; enso-ui doesn't depend on `enso-block`).

- `patch_editor.rs`: `PatchNode { desc: Option<NodeDesc> (None = an
  "Output" node = `NodeId::SPEAKERS`), id: Option<NodeId>, applied, knobs:
  Vec<(ParamInfo, f32)> }`. Pins, titles and knob ranges come from
  `NodeDesc`.
  - `show(ui, ports)` draws the graph through a `Viewer` (typed pins; only
    same-signal wires connect; add/remove menus; knob sliders and MIDI
    pickers that edit the node's `desc` in place).
  - `sync(&mut impl AudioEngine)`: creates engine nodes for new graph
    nodes, `update_node` when a desc changed (MIDI port/channel), destroys
    engine nodes whose graph node is gone (`created` list), then refills an
    `AudioWorld` (every wire by pin index, every knob value) and submits
    it. The engine diffs: unchanged frames cost a comparison. Errors (a
    cycle) show via `error()`; `Busy` is ignored (transient).
- Rewiring doesn't cut sounding notes or delay tails (nodes keep their
  state). Tests (`patch_editor::tests`) render offline through
  `BlockAudioEngine::render` and check exactly that, plus cycles and knobs.

Adding a node kind: see [enso-audio.md](enso-audio.md#adding-a-node-kind),
then add it to `MENU`.

## crates/enso: the app

`cargo run -p enso [-- <output name>]`. eframe 0.37 (wgpu backend, the
same wgpu 30 as the renderer). At startup:

1. Open the audio device; `BlockAudioEngine::new(device.audio_config())`;
   `editor.sync` once; `audio.start(device, clock)`.
2. Connect every MIDI input to `audio.midi_sender()` (wrapped to implement
   enso-io's `MidiSink`).

Each frame: `editor.show`, `editor.sync(&mut audio)`. With no audio
device, `audio.render` into a scratch buffer applies changes silently. The
status bar shows the device, buffer, load (`audio.stats()`), MIDI inputs, crashed blocks, and editor errors.

Not yet: saving/loading patches (the graph maps to plain `NodeDesc`s and wires), the visual renderer in the window, MIDI learn.
