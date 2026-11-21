# Design: blocks

**Status:** agreed. Build-order step 1 (§12) is implemented in
`crates/enso-block` (see [enso-block.md](../agents/enso-block.md)).

**Owner's brief (2026-11-21):** The user defines *blocks*. Blocks have
inputs (`In<T>`) and outputs (`Out<T>`). Attaching inputs to outputs builds
computations. Blocks publish a dictionary of *interfaces*, and children's
interfaces float up to their parents unless a flag says otherwise.
Initialisation has three stages: **registration**, **resource loading** and
**linkage**. A block may have a **core routine**, which is optional for
composites. An execution engine runs the computation.

## Decisions so far (owner)

1. Blocks are **domain-agnostic**: the same system for audio, rendering, the
   game simulation, UI, the content server, and so on.
2. **Terminology:** the **engine** is the overarching structure. It contains
   **workstreams**, one per thread, and each owns its own tree of blocks.
3. **Blocks have no say in how they are executed.** A workstream is given a
   single root block and runs the whole computation from start to end.
   Start **single-threaded**; thread pools come later, inside the workstream.
4. `In`/`Out` are synchronization primitives (execution order within a
   workstream). Blocks in different workstreams talk through **mailboxes**.
   An `In`/`Out` link across workstreams is an **error**.
5. A full mailbox queue **drops the oldest** message.
6. Everything runs **in one process** for now; messages arrive in
   **arrival order**.
7. **The owner of a mailbox is responsible for freeing memory** in the
   messages it receives.
8. **A parent links its children to each other, never to itself.** If a
   parent needs to take part, it creates a child for that job and links the
   child.
9. **Registration may depend on configuration** (e.g. a variable number of
   ports).
10. **Fan-in needs merge semantics:** message streams append; signals (e.g.
    audio) may be summed, depending on configuration.
11. **Blocks encode the simulation side of the ECS.** Each frame, the
    simulation workstream *stages* changes, which are merged into the ECS at
    the end of the update.
12. The **content server is synchronous at first**; later it becomes its own
    workstream.
13. Mailboxes sent **from real-time workstreams carry only non-allocating
    message types**, so evicting the oldest never frees memory on a
    real-time thread.
14. A parent may **link anything its children expose**, including
    interfaces floated up from deeper descendants.
15. **Audio inputs default to the `Single` merge mode**; summing is opt-in
    (or done by mixer blocks).
16. **ECS conflicts are last-wins** for now (to be expanded later).
17. **We write our own ECS.**
18. **Blocks declare their ECS queries at registration.**
19. **`Append` merges sort by timestamp** (stable: equal timestamps keep
    link order).
20. **Blocks read the ECS as of the start of the step.** A same-step
    dependency is always expressed as a port.
21. **Entities spawned in a step get placeholder IDs**, usable within that
    step and resolved when staged changes are merged.
22. **A `RealtimeMessage` marker trait** enforces decision 13 when mailboxes
    are connected.
    *As implemented (agent, pending owner review):* the check uses
    `!std::mem::needs_drop::<M>()`, the exact property decision 13 needs
    (dropping the message runs no code). It's checked for **every** outbox
    of a real-time workstream at build time, connected or not, because
    sending to an unconnected outbox also drops the message on the sender's
    thread. `RealtimeMessage` still exists, implemented for `Copy` messages,
    for typed APIs.
23. **Things are identified by IDs, never by names.** Names stay, as
    optional, non-unique **labels** for display and error messages.
24. **Port keys** identify an interface within its block. By default they're
    derived from the field name; `id = N` pins one so the field can be
    renamed without breaking saved data.
25. **Block instances are identified by full UUIDs.**
26. *(Agent, pending owner review)* **Rebuilding separates configuration from
    state.** A rebuild always runs the *fresh* block instances, so new
    configuration takes effect. State survives only where declared: fields
    marked `#[state]` (moved over by the derive) and an optional
    `Block::carry_over` hook, for blocks with the same `BlockId` and Rust
    type. Knob settings, output values, inbox mailboxes and engine-made
    links carry over automatically; handles stay valid (stable internal
    indices). Why: keeping the old instance (the first implementation)
    also kept its old configuration, so a rebuild's changes didn't apply.

## 0. Identity

| Thing | Identified in code by | Identified in data (files, editor) by |
|---|---|---|
| Block instance | `Child<B>` handle | `BlockId` (a UUID) |
| Interface (port, mailbox end) | `In<T>`/`Out<T>`/`Inbox<M>`/`Outbox<M>` handle | `InterfaceId = (BlockId, Key)` |
| Workstream | `WorkstreamId` (from `Engine::add`) | (future: an ID in the show file) |

- **Handles** are small typed indices tagged with the tree they belong to,
  so using a handle from another workstream is detected. The compiler checks
  their types: `Outbox<f32>` can't be connected to `Inbox<String>`.
- **Keys** (`Key`, 64-bit) are unique within a block: by default a stable
  hash of the field name, or `#[input(id = 7)]`.
- **`BlockId`s are deterministic for blocks built from code:** a child's
  UUID is derived (UUID v5) from its parent's UUID and its key, so the same
  tree gets the same IDs on every run and saved references stay valid.
  Children created from data carry the UUIDs stored with them. A root's
  UUID is given when building (`Workstream::build_with_id`), or random.
- **Labels** (`name = "cutoff"`, default: the field name) appear in error
  messages and the UI as paths like `voice/filter/cutoff`. They're never
  looked up and needn't be unique.

The audio engine (`enso-audio`) will be retrofitted first.

Sections marked **Proposal** are the agent's suggestions; **Open** marks
questions for the owner.

---

## 1. Concepts

| Term | Meaning |
|---|---|
| **Block** | A unit of computation: interfaces, maybe children, maybe a core routine. Knows nothing about threads. |
| **Leaf block** | A block with a core routine and its own ports. |
| **Composite block** | A block made of children, which it links to each other. Its outside surface is the interfaces its children float up. |
| **Engine** | The whole runtime in the process: its workstreams and the mailbox links between them. |
| **Workstream** | A thread (later possibly with a pool) that owns one root block and runs its whole tree, step after step. |
| **Step** | One run of a workstream's computation: one audio buffer, one frame, one tick. |
| **Port** | `In<T>` / `Out<T>`: synchronous data flow within one workstream and one step. Defines execution order. |
| **Mailbox** | `Inbox<M>` / `Outbox<M>`: asynchronous messages, across steps and workstreams. |
| **Interface** | A named, public item in a block's dictionary: a port or a mailbox. |
| **Path** | A display string built from labels (`voice/filter/cutoff`) for errors and the UI. Never used for identity (§0). |

## 2. Two kinds of connection

|  | **Ports** (`Out<T>` → `In<T>`) | **Mailboxes** (`Outbox<M>` → `Inbox<M>`) |
|---|---|---|
| Where | Same workstream | Anywhere, usually across workstreams |
| When | Same step: producer runs before consumer | Delivered at the receiver's next step |
| Meaning | "B needs A's result **now**": an ordering constraint | "Tell B something": no ordering, no waiting |
| Cost | A buffer the workstream allocates and lends | A bounded lock-free queue or a snapshot slot |
| Blocking | Never (the order is computed in advance) | Never: a full queue drops its oldest message |
| Across workstreams | **Error** | Yes, that's what they're for |
| Examples | Oscillator → filter → mixer; transform → skinning → draw list | MIDI → audio; audio level → visuals; sim → renderer; UI → "load preset" |

Why two mechanisms: within one workstream, waiting for an input is free,
because the workstream sorts its graph once and runs it in order. Across
threads, waiting means blocking. The audio thread must never block, and the
renderer shouldn't stall on the simulation. Making the difference visible in
the types (`In` vs `Inbox`) means the compiler and the reader always know
which is which.

## 3. The engine and its workstreams

```rust
let mut engine = Engine::new();
let audio = engine.add("audio", Workstream::build_realtime(AudioRoot::default())?)?;
let render = engine.add("render", Workstream::build(RenderRoot::default())?)?;

// Handles reach into a built tree like the Linker does (§0):
let meter = engine.get(engine.root::<AudioRoot>(audio)?.meter)?;
let visuals = engine.get(engine.root::<RenderRoot>(render)?.visuals)?;
engine.connect_mail(meter.level, visuals.level)?;   // typed: checked by the compiler
```

- **Cross-workstream wiring is the engine's job.** Workstream roots have no
  common parent, so the engine links their mailboxes: by handle in code,
  or by `InterfaceId` for wiring that comes from data. Both ends must be
  exposed by their roots. A show can therefore be one file of IDs.
- **Each workstream runs its tree, step by step.** Single-threaded at
  first. Each step: drain inboxes → run core routines in port order →
  publish outboxes. Later, inside the workstream only: a thread pool runs
  blocks whose inputs are ready (port links are exactly the dependencies).
  Blocks don't change.
- **What triggers a step depends on the workstream.** The audio device
  callback steps `audio`, the render loop steps `render` once per frame, a
  fixed tick steps `sim`. The workstream type is the same in each case.
- **The rules come with the thread.** Blocks stepped in the audio callback
  must be real-time safe (no allocation, locks, I/O or blocking). Blocks
  don't declare where they run, so that's the block author's job, as it is
  for today's `Node`. **Open (later):** blocks declare *capabilities*
  ("real-time safe", "does I/O") so a wrong tree fails when compiled.

## 4. Mailboxes

```rust
pub struct Inbox<M>  { /* handle */ }
pub struct Outbox<M> { /* handle */ }

fn process(&mut self, io: &mut Io) {
    for note in io.receive(self.notes) { ... }      // drained at step start
    io.send(self.level, self.meter.rms());          // never blocks
}
```

Messages are `Send + 'static` and carry a `Timestamp` from the shared
`Clock`, so receivers can place them precisely (audio to the sample).

| Policy | Semantics | Use | Implementation |
|---|---|---|---|
| **Queue** | Every message, in arrival order; when full, **the oldest is dropped** and counted | Notes, commands, requests | Bounded lock-free queue: `crossbeam_queue::ArrayQueue::force_push` replaces the oldest without locking |
| **Latest** | Only the newest value | Meter levels → visuals; sim state → render | Triple buffer / atomic snapshot: never full |
| **Request / reply** | Ask now, answer later | "Load texture X" → content server (once it's a workstream) | A queue there, a reply mailbox back, matched by request ID |

**Freeing memory (decision 7):** once delivered, a message belongs to the
block that owns the inbox, which is responsible for freeing what it holds.
For a block in a real-time workstream this means one of:
- the message type owns no heap memory (`Copy` data, fixed-size arrays),
  so dropping it costs nothing (the common case: MIDI, parameter values);
- or the block keeps it and passes it on for disposal through an outbox to
  a non-real-time workstream (e.g. a sample buffer it replaced).

Proposal: a `Disposal` helper block in the standard library, so this is
one link and not something each block writes.

**Dropped by a full queue:** dropping the oldest happens on the *sender's*
side (`force_push` hands back the evicted message), so the sender frees it.
Decision 13 makes that safe: mailboxes sent *from* real-time workstreams
carry only non-allocating message types (levels, notes, telemetry). It is
enforced by a marker trait, `RealtimeMessage` (decision 22), implemented
for `Copy` types. The engine requires it when it connects an outbox that
belongs to a workstream marked real-time. Messages that aren't
`RealtimeMessage` can still be sent *into* real-time workstreams; the
receiving block then follows the freeing rule above.

## 5. Interfaces, floating up, and linkage

Every block has a dictionary `Path → InterfaceInfo { kind (In / Out / Inbox /
Outbox), type, policy, merge, default, range, docs }`.

- A child's interfaces appear on its parent under the child's name:
  `filter/cutoff` becomes `voice/filter/cutoff`.
- A **private** flag stops an interface floating up.
- **Parents link children to each other, never to themselves
  (decision 8).** A composite therefore needs no ports of its own: its
  surface *is* whatever its children float up. A parent with its own core
  routine still never connects that routine to its children. If it needs
  to take part, that functionality becomes a child.
- Commands are inboxes; telemetry is outboxes with the `Latest` policy. The
  UI can reach any block in any workstream by `InterfaceId`.

```rust
struct Voice {
    osc: Child<Osc>,          // floats up: osc/midi (Inbox), osc/pitch, ...
    filter: Child<LowPass>,   // floats up: filter/cutoff, filter/out, ...
}

impl Block for Voice {
    fn register(&mut self, r: &mut Registrar) {
        self.osc = r.child("osc", Osc::default());
        self.filter = r.child("filter", LowPass::default());
        r.private(self.filter.input());   // wired inside; hide it from outside
    }

    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.osc].out, l[self.filter].input)
    }
    // No core routine. From outside, the voice is played through
    // `voice/osc/midi` and heard at `voice/filter/out`.
}
```

**Proposal: re-export under a nicer name** (`voice/out` →
`voice/filter/out`) as an alias only, with no link and no pass-through. That
keeps decision 8 intact while giving composites a clean surface.

A parent may link **anything its children expose**, including interfaces
floated up from deeper descendants, e.g. `a/env/out` (decision 14). They're
part of the child's public dictionary; `private` is how a child keeps
something internal.

**Proposal: an unconnected `In<T>` is a knob.** It has a default; the
workstream feeds it a constant that can be changed (from the UI, through a
mailbox). Linking something to it turns it into modulation. One mechanism
covers parameters, knobs, MIDI learn and automation.

## 6. Configuration and registration

Registration may depend on configuration (decision 9), so configuration
comes first:

1. **Construct** the block with its *structural* configuration (port
   counts, mode, which children), e.g. `Mixer::new(MixerConfig { inputs: 8 })`
   or from a patch file.
2. **Register** declares interfaces and children from that configuration.
3. **Load** resources (files, samples, plugins, buffers).

Changing structural configuration later = re-register and re-load that
block, then relink its parent. Its interfaces (and therefore links) may
change. Knob values aren't structural: they're inputs.

## 7. Merging (fan-in)

Decision 10: several outputs into one input need defined semantics. Each
`In<T>` declares a **merge mode** at registration (possibly from
configuration); each port type says which modes it supports:

| Mode | Meaning | Typical types |
|---|---|---|
| **Single** | At most one link (more is a link error) | Default for values (`f32`, `Vec3`, transforms) **and for audio** (decision 15) |
| **Append** | Concatenate all sources' messages | Message/event streams (MIDI events) |
| **Sum** | Add the signals | Audio, when the input is configured to mix |

`Append` sorts the merged messages **by timestamp, stably** (decision 19):
equal timestamps keep link order. That's what sample-accurate audio needs,
and what today's engine does. (Mailboxes, by contrast, deliver in arrival
order: decision 6.)

## 8. Simulation and the ECS

Decision 11: blocks encode the simulation side of the ECS.

```
 sim step:   read ECS (snapshot) ──▶ blocks compute ──▶ staged changes ──▶ merge into ECS at end of step
                                                                              │
                                                     publish (Latest) ────────┴──▶ render: RenderWorld
```

- During a step, blocks **read** the world as it was at the start of the
  step, and **write** only staged changes (spawn, despawn, set component,
  ...), through an output or a staging interface.
- At the end of the step, the workstream merges the staged changes into the
  ECS. Every block sees the same consistent world, the order of blocks
  doesn't change what they read, and parallel execution later is safe.
- After merging, the sim publishes what the renderer needs (a
  `RenderWorld`) through a `Latest` mailbox. That's exactly the
  `prev`/`curr` + interpolation model `enso-visual` already has.

- **Conflicts are last-wins** (decision 16): if two blocks stage the same
  component in one step, the change from the block that ran later wins.
  "Later" is the workstream's execution order, which is deterministic for a
  given graph. Per-component merge rules can come later.
- **Our own ECS** (decision 17), not bevy_ecs. It only needs what blocks
  use: entities, component storage, queries, and applying staged changes.
- **Queries are declared at registration** (decision 18), like ports:

```rust
fn register(&mut self, r: &mut Registrar) {
    self.movers = r.query::<(&Position, &Velocity)>();   // reads
    self.writes = r.stage::<Position>();                  // staged writes
}

fn process(&mut self, io: &mut Io) {
    for (entity, (pos, vel)) in io.query(self.movers) {
        io.stage(self.writes, entity, Position(pos.0 + vel.0 * io.dt()));
    }
}
```

  The workstream then knows, before running anything, which components
  each block reads and writes. That allows checking ("nobody writes
  `Health`, but three blocks read it" is fine; flagging two writers is
  possible), visualising data flow in the editor, and, later, scheduling
  non-overlapping blocks in parallel.

- **The ECS is always the start-of-step snapshot** (decision 20). If B
  needs what A computed this step, A sends it through a port to B. That's
  one rule with no hidden ordering, and it's what makes block order
  irrelevant to ECS reads.
- **Spawning returns a placeholder ID** (decision 21) that the block can use
  in further staged changes (add components, link to other entities) and
  pass through ports within the same step. When the staged changes are
  merged, placeholders are replaced by real entity IDs. A placeholder
  can't be queried before the merge: it isn't in the snapshot.

## 9. Lifecycle

```
 construct ─▶ register ─▶ load ─▶ link ─▶ compile ─────▶ workstream: swap in, step, step, ...
 (structural   (interfaces, (resources) (siblings  (flatten aliases, order,
  config)       children)               wired)      allocate; engine: mailboxes)
◀──────────── off the stepping thread: may allocate, may be slow ────────────▶
```

- **Load** is synchronous for now (decision 12). Proposal: once the content
  server is its own workstream, loading becomes a request/reply. A block then
  joins its workstream's graph when its resources arrive, so a heavy plugin
  never interrupts audio.
- **Compile:** resolve aliases; check types and merge modes; order
  topologically (cycles only through blocks marked as a delay); allocate port
  buffers. The engine creates cross-workstream mailboxes.
- **Swap:** between two steps the workstream swaps in the newly compiled
  graph, and blocks that survive keep their state. Workstreams swap
  independently, so messages for a block that's gone are dropped (and
  freed off-thread).

## 10. Retrofitting enso-audio

| Today | Becomes |
|---|---|
| `Processor` + `Schedule` | The `audio` workstream, stepped by the device callback |
| `AudioEngine` (control side) + command/garbage queues | The engine's control API (relink, compile, swap), shared by all workstreams |
| `Node::process(Process)` | Leaf `Block` with a core routine |
| `Process { input, output }` | `In<Audio>` / `Out<Audio>` (merge: `Sum` where configured) |
| `events` / `events_out` | `In<Events>` (merge: `Append`) / `Out<Events>` |
| `MidiQueue` | An `Inbox<MidiEvent>`, `Queue` policy (MIDI events are `Copy`: nothing to free) |
| `SharedParam` knobs | Unconnected `In<f32>`, set through a mailbox |
| `Stats` atomics | Telemetry outboxes (`Latest`) |
| `Patch` / `patch.output(..)` | A data-defined composite linking siblings; output is a sink block |
| `PatchEditor` | Editor for data-defined composites, showing interfaces (by `InterfaceId`, labelled with their paths) |
| (future) VST3 plugin | A leaf whose ports come from the plugin at register (decision 9); loaded in load |

Proposal: a new `enso-block` crate (block traits, registrar, linker,
compiler, mailboxes, single-threaded workstream, engine). Then `enso-audio`
moves onto it piece by piece, with its tests (offline rendering, no-alloc)
green throughout.

## 11. Open questions

No blocking questions remain for a first implementation. Deferred:

1. Block capabilities ("real-time safe", "does I/O") checked at compile
   time (§3).
2. Per-component ECS merge rules beyond last-wins (§8).
3. Asynchronous loading through a content-server workstream (§9).
4. Thread pools inside workstreams (§3).
5. Timestamp-ordered mailbox delivery, e.g. for deterministic replay.

## 12. Suggested build order

1. ✅ **`enso-block` core:** `Block` trait, `Registrar`, `Linker`,
   `In`/`Out` handles, the interface dictionary with floating up and
   `private`, aliases, merge modes. Compile a tree into a flat ordered
   graph. Single-threaded `Workstream::step`. Tests with small
   arithmetic blocks.
   ✅ Plus `#[derive(Ports)]` (owner's request): ports and children
   declared as annotated fields; `Block::register` became optional and
   runs after the derived part.
2. ✅ **Mailboxes and the engine:** `Inbox`/`Outbox`, Queue (drop-oldest) and
   Latest policies, `RealtimeMessage`, `Engine` with several workstreams and
   `connect_mail` (by handle; by `InterfaceId` after decision 23).
3. ✅ **Swap while running:** relink → compile → swap between steps, keeping
   block state. That's the existing `enso-audio` command/garbage mechanism,
   generalised (decision 26).
4. **Retrofit `enso-audio`** onto it (§10). The offline-rendering and
   no-alloc tests must pass at every step.
5. **The ECS and the sim workstream** (§8), feeding `RenderWorld` to the
   renderer through a `Latest` mailbox.
6. The patch editor becomes a block-tree editor.
