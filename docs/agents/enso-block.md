# enso-block

Implementation of [design/blocks.md](../design/blocks.md), build-order
steps 1–3: blocks, ports, interfaces, linking, a single-threaded
`Workstream`, `#[derive(Ports)]`, mailboxes, the `Engine`, and rebuilding
while running. **Not yet:** delay blocks, the ECS, moving `enso-audio`
onto blocks.

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
   Block *indices* (internal) follow registration order, so a parent comes
   before its children. Block *identity* is the `BlockId` (see Identity).
2. **Load:** in reverse ID order (children before parents).
3. **Dictionaries:** in reverse ID order. A block's dictionary holds its
   own ports, its children's dictionaries (display paths prefixed with the
   child's label), minus ports it marked `private`, plus its aliases. The root's dictionary is
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
- Linking: `Linker::connect_mail(outbox, inbox)` or `connect_ids`
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
- `Engine` (`engine.rs`): workstreams by `WorkstreamId` (labels are for
  display). `connect_mail(outbox, inbox)` takes handles (they carry their
  tree tag, so the engine finds the workstreams itself);
  `connect_mail_ids` takes `InterfaceId`s. `sender(inbox)` → `MailSender`
  (`Clone`, for e.g. MIDI threads); `listen(outbox, delivery)` →
  `MailReceiver` (for the host and tests); `_id` variants of both.
  `root::<R>(id)` / `get(child)` reach handles. `take(id)` moves a workstream out to its thread (it's
  `Send`); `step_all()` steps all of them in order on one thread.
  Connections are made before the workstreams start; changing them while
  running is step 3.

## Swapping while running (step 3)

- **Control / runner split** (`control.rs`, like `enso-audio`'s
  engine/processor). `Workstream` is the runner (graph + target table +
  queue ends). Its `Control` (layout, target lists, engine-made links, queue
  ends) stays with it until `Engine::add` takes it over; the engine keeps
  it even after `take`.
- `Command`s (control → runner, `rtrb`): `Swap(Box<Swap>)`,
  `Targets(Box<TargetTable>)`, `Set { slot, value }`. They're applied at
  the **start of `step`** by moving pointers only, and only while the
  garbage queue has room. What they replace comes back as `Garbage` and is
  freed by `Control::collect_garbage` (engine methods call it; standalone
  non-real-time workstreams call it after each step).
- **Stable indices** (`build.rs`): `Build::new(.., previous: Some(&Layout))`
  gives each block and interface whose ID existed before the same index;
  new ones get fresh indices; indices are **never reused**. So
  `build.blocks` and `build.ports` are sparse (`Option`), with
  `build.order` holding registration order. Handles from before a rebuild
  stay valid; a handle to a removed interface gives `AccessError::Gone`.
- **What carries over** (`Swap`):
  - **The fresh instance always runs,** so configuration and child handles
    come from the new build.
  - `carry_blocks` (same `BlockId` and Rust type): the runner calls
    `fresh.carry_state(old)` (derived: swaps `#[state]` fields), then
    `fresh.carry_over(old)` (optional hand-written hook). The old instance
    goes back as garbage. A crashed (switched-off) block starts fresh.
  - `keep_slots`: values of outputs and knobs whose port still exists
    with the same ID, kind and type (the slot boxes are swapped).
  - **Inbox mailboxes** are reused (`compile(.., previous)`), so
    `MailSender`s and other workstreams' links keep working.
  - **Engine-made links** live in `Control::external` by outbox
    `InterfaceId` and are re-added when compiling.
- `Layout` (`graph.rs`, `Arc`-shared): everything both sides need to check
  handles and IDs (`handle`, `typed`, `by_id`, `knob_slot`), plus the
  per-port slot, mailbox and `MailFns`. Engine methods use the control's
  layout, so they work while the runner is on another thread.
- `tests/swap.rs` covers each rule (including rebuilding and setting
  knobs while a thread steps the runner). `tests/no_alloc.rs` proves that
  stepping, including applying swaps, knob settings and new links, never
  allocates on a real-time workstream.

## Identity (decisions 23–25)

- `ids.rs`: `BlockId` (a UUID), `Key` (u64, unique per block across ports,
  mailboxes and aliases), `InterfaceId { block, key }`, `Spec { key,
  label }` (what every `Registrar` method takes; `"name".into()` gives
  `Key::named("name")` plus that label), `WorkstreamId`.
- Root ID: `BuildOptions::id`, or random. Child ID: `parent.child(key)` =
  UUID v5 of the key in the parent's namespace, so it's deterministic.
  `Registrar::child_with_id` is for children created from data (IDs from a
  file). Duplicate child IDs are a registration error.
- `Key::named` is FNV-1a and `const`. The derive keys by the **field** name
  (not the label) or by `id = N`.
- Handles carry `tree` (a global counter per build). The `Linker` refuses
  foreign handles (`LinkError::ForeignHandle`), and so does `Workstream`
  (`AccessError::ForeignHandle`). From outside, a port must be **exposed by
  the root** (`AccessError::NotExposed`).
- Outside access: `ws.root::<R>()` and `ws.get(child)` reach handles;
  `ws.read(port)`, `ws.set(input, v)` and `ws.dropped(inbox)` take
  handles; `read_id`/`set_id` take `InterfaceId`s. `Linker::id()` and
  `id_of(child)` build IDs for `connect_ids`.
- Labels are free text, may repeat, and are never looked up. They only
  build display paths (`Workstream::disabled`, error messages,
  `Interface::path`).

## API notes

- `Registrar::get(child)` reads a child's handles during `register`, to
  pass them to `private`/`alias`. In `link`, `&l[block]` reaches any block
  of the tree (e.g. a grandchild through a child's `Child` field).
- `Linker::connect` / `connect_mail` are typed (checked by the compiler);
  `connect_ids` checks kind and `TypeId` at link time.
- **Step context:** `Workstream::step_with(&ctx)` hands every block a
  `&dyn Any`, read with `io.context::<C>()` (`None` from plain `step()`).
  That's how audio blocks get `AudioContext` without the platform knowing
  about audio.
- `Workstream::peek(port)` → `Option<Ref<T>>`: like `read` but never
  allocates, even on failure (for reading results on real-time threads).
- `Engine::set_id(InterfaceId, value)`: knobs by ID (the patch editor).
- `Events::push` never allocates: it returns `false` and drops the event
  at capacity.
- `#[derive(Ports)]` emits `::enso_block::...` paths, so a crate using it
  needs `enso-block` as a **direct** dependency.
  `Linker::available()` lists what can be linked.

Tests: `tests/blocks.rs` (one per design rule, small arithmetic blocks)
and `tests/mail.rs` (mailboxes, engine, threads), all by handle or ID.
