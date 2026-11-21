# enso-block

Implementation of [design/blocks.md](../design/blocks.md), build-order step 1:
blocks, ports, interfaces, linking, and a single-threaded `Workstream`.
**Not yet:** mailboxes, the engine (several workstreams), swapping while
running, the ECS.

## Files

- `block.rs`: the `Block` trait (`register`, `load`, `link`, `process`;
  only `register` is required). `In<T>`/`Out<T>`/`Child<B>` are `Copy`
  index handles. `Default` gives an unregistered handle, so block structs
  can `#[derive(Default)]`.
- `merge.rs`: `MergeMode {Single, Append, Sum}`, the `Merge` trait (sum for
  numbers, append for `Vec`), `Events<T: Stamped>` (appends sorted by
  timestamp, stably, by insertion: no allocation within capacity).
  `Stamped` is implemented for `enso_core::MidiEvent`.
- `build.rs`: build state, `Registrar`, `LoadContext`, `Linker`,
  interface dictionaries, `LinkError`/`BuildError`.
- `workstream.rs`: `compile` (storage slots, merge steps, execution order),
  `Workstream::step`, `Io`, and the control API (`read`, `set`).

## How a build runs (`Workstream::build(root)`)

1. **Register:** `root.register`. `Registrar::child` registers the child
   immediately (recursively), then stores it in the flat block table.
   Block IDs follow registration order, so a parent's ID is lower than
   its children's.
2. **Load:** in reverse ID order (children before parents).
3. **Dictionaries:** in reverse ID order. A block's dictionary holds its
   own ports, its children's dictionaries prefixed `child/`, minus ports
   it marked `private`, plus its aliases. The root's dictionary is
   `Workstream::interfaces()`.
4. **Link:** each block is taken out of the table, `link` runs with a
   `Linker` that can see its children (`&l[child]` downcasts through
   `Any`, which is why `Block: Any`), and the block is put back. A port may
   be linked only if it's in **a child's dictionary**. That one check
   enforces decision 8 (a parent's own ports aren't in its children's
   dictionaries), decision 14 (descendants' floated ports are) and
   `private`.
5. **Compile:**
   - every output gets a storage slot;
   - a `Single` input with one link **shares the source's slot** (no copy);
   - an unlinked input gets its own slot holding its default: a **knob**,
     settable with `Workstream::set`;
   - a merging input with links gets its own slot plus a `MergeStep`
     (clear, then merge each source) that runs before its block.

   Execution order is Kahn's algorithm over the links' owners, picking the
   lowest ID when there's a choice (deterministic). A cycle →
   `BuildError::Cycle` (delay blocks aren't implemented yet).

## Running

- Slots are `RefCell<Box<dyn Any + Send>>`. `Io::read` and `Io::write`
  return `Ref`/`RefMut` guards, so a block can hold reads and writes of
  *different* ports at once without `unsafe`. Borrow flags are plain
  counters: no locks, no allocation. When thread pools arrive, this is the
  part to replace with something `Sync`.
- `Io` refuses ports of other blocks (panic → block disabled).
- A panicking `process` is caught: the block is set to `None` and listed by
  `Workstream::disabled()`; the rest keep running. (Its `Box` is dropped
  on the workstream's thread. Revisit for real-time workstreams.)
- Composites without a routine are still visited (a no-op call). That's
  cheap; skip them if profiles ever say otherwise.

## `#[derive(Ports)]` (crate `enso-block-derive`)

`Block: Ports`. Registration runs `register_ports` (generated) and then
`Block::register` (optional, hand-written). Every block derives `Ports`,
even with no annotated fields, because `Ports` is a supertrait.

- `#[input(name, default, merge)]` on `In<T>`, `#[output(name, initial)]`
  on `Out<T>`, `#[child(name, init)]` on `Child<B>`, `#[private]` next to
  a port. Missing values use `Default::default()`. The expressions run
  inside `register_ports`, so they may use `self` (`default =
  self.initial`).
- Fields register in declaration order. Fields without attributes are
  plain state.
- What a derive can't express (ports that depend on configuration, private
  flags or aliases on *children's* ports via `Registrar::get`) goes in
  `Block::register`, which runs after the derived part.
- Generated code names `::enso_block::...`, so the derive only works
  outside the `enso-block` crate (tests and doctests are outside).
- The macro checks the handle type of each field (`#[output]` on an `In`
  is an error) and reports **all** bad fields at once, with spans. It uses
  syn 2 (also in the tree: syn 3, pulled in by other crates).

## API notes

- `Registrar::get(child)` reads a child's handles during `register`, to
  pass them to `private`/`alias`.
- `Linker::connect` (typed: checked by the compiler) and
  `Linker::connect_paths` (by path: checked by `TypeId` at link time, for
  data-defined composites). `Linker::available()` lists what can be linked.
- Names are path segments: non-empty, no `/`, unique within a block
  across ports, children and aliases.

Tests: `tests/blocks.rs`, one per design rule, using small arithmetic
blocks.
