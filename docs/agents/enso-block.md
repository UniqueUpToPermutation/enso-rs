# enso-block

Implementation of [design/blocks.md](../design/blocks.md), build-order
steps 1–2: blocks, ports, interfaces, linking, a single-threaded
`Workstream`, `#[derive(Ports)]`, mailboxes and the `Engine`.
**Not yet:** swapping while running, delay blocks, the ECS.

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

## Mailboxes and the engine (step 2)

- `mail.rs`: `Mailbox<M>` wraps a `crossbeam_queue::ArrayQueue`. Senders
  use `force_push`, which evicts the **oldest** message when full; the
  evicted one is freed on the sender's thread and counted (`dropped`).
  `Delivery::Latest` is the same queue with capacity 1 (replacing isn't
  counted). `Delivery::Queue(n)` defaults to 64.
- Interfaces now have an `InterfaceKind`: `In`, `Out`, `Inbox`, `Outbox`.
  Mailbox ends are stored as "ports" internally (same handles, names,
  visibility and `private`), with `MailFns` (type-specific function
  pointers) instead of values.
- Linking: `Linker::connect_mail(outbox, inbox)` or `connect_paths`
  (which accepts either kind of pair). The visibility rules are the same as
  for ports. Fan-in and fan-out are unlimited; a duplicate pair is ignored.
  Mail links **don't affect execution order**.
- Compile: each inbox gets an `Arc<Mailbox<M>>`; each outbox gets a
  `Vec<Arc<Mailbox<M>>>` of targets (type-erased as `Box<dyn Any>`).
- Step: first, each inbox's **budget** is set to the number of messages
  waiting. `Io::receive` yields at most that many, so messages sent during
  the step (even by blocks earlier in the same step) wait for the next one.
  Unread messages stay queued. `Io::send` clones the message for all
  targets but the last; with no targets, the message is simply dropped.
- `Workstream::build_realtime`: **every** outbox's type must satisfy
  `!needs_drop` (not just connected ones: sending to an unconnected outbox
  also drops on the sending thread). The engine checks again when
  connecting.
- `Engine` (`engine.rs`): named workstreams; `connect_mail("ws:path",
  "ws:path")`; `sender::<M>(inbox)` → `MailSender` (`Clone`, for e.g. MIDI
  threads); `listen::<M>(outbox, delivery)` → `MailReceiver` (for the host
  and tests); `take(name)` moves a workstream out to its thread (it's
  `Send`); `step_all()` steps all of them in order on one thread.
  Connections are made before the workstreams start; changing them while
  running is step 3.

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
