# enso-ui and the enso app

## enso-ui

A node-graph editor (egui-snarl, vendored; see `third_party/README.md`)
for the audio patch, applied to a running block workstream.

- `patch.rs` (no GUI): `NodeKind` (the menu), each kind's pins
  (`Signal` + the block interface's `Key`, i.e. `Key::named(field name)`)
  and knobs (`KnobSpec` with the block's `DEFAULT_*`); `PatchSpec` (nodes as
  `(BlockId, NodeKind)`, wires as `(Endpoint, Endpoint)`); `PatchRoot`, the
  **data-defined composite**:
  - Fixed children: `MidiSource` (key "source") and `AudioOut` (key
    "output").
  - One child per node via `child_with_id(node UUID, ..)`. "MIDI in" nodes
    are `MidiFilter`s fed from the source; "Output" nodes have no block
    (`Endpoint::Output` resolves to the `AudioOut` input).
  - Wires are linked with `connect_ids`.
- `patch_editor.rs`: `PatchNode { kind, id: BlockId (random UUID, stable for
  the node's life), knobs }`.
  - `show(ui, ports)` draws the graph through a `Viewer` (typed pins; only
    same-signal wires connect; add/remove menus; knob sliders and MIDI
    pickers). Slider changes are queued as `(InterfaceId, f32)`.
  - `sync(engine, audio)`: builds the `PatchSpec`; if it differs from the
    last **attempted** one, `engine.rebuild(audio, PatchRoot)`. A failure
    (e.g. a cycle) is shown via `error()`, the old patch keeps playing, and
    the same failing spec isn't retried every frame. It then sends queued
    knob changes with `engine.set_id`.
  - `root()` gives the initial `PatchRoot` (build the workstream from it).
- Because blocks keep `#[state]` across rebuilds, rewiring doesn't cut
  sounding notes or delay tails. Tests (`patch_editor::tests`) render
  offline through an `AudioDriver` and check exactly that, plus cycles and
  knobs.

### Adding a node kind

1. Add the block in `enso-audio/src/blocks/` (knobs as `In<f32>` with
   `DEFAULT_*` consts, state as `#[state]`) and to
   `enso-audio/tests/no_alloc.rs`.
2. In `patch.rs`: a `NodeKind` variant, then `MENU`, `title`, `inputs`,
   `outputs` (keys = the block's field names), `knobs`, and a
   `PatchRoot::register` arm.

## crates/enso: the app

`cargo run -p enso [-- <output name>]`. eframe 0.37 (wgpu backend, the
same wgpu 30 as the renderer). At startup:

1. Open the audio device; build a real-time workstream from
   `editor.root()`; add it to an `Engine`.
2. Get the `MidiSource` inbox and `AudioOut` input handles; take the runner
   into an `AudioDriver` and start the device.
3. Connect every MIDI input to the inbox through a cloneable
   `MailSender` wrapper.

Each frame: `editor.show`, `editor.sync`, `engine.collect_garbage`. With no
audio device, the workstream stays in the engine and `engine.step_all()`
applies changes silently. The status bar shows the device, buffer, load
(`StatsHandle`), MIDI inputs, crashed blocks, and editor errors.

Not yet: saving/loading patches (a `PatchSpec` is already plain data with
UUIDs), the visual renderer in the window, MIDI learn.
