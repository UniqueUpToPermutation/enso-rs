//! Swapping while running (build-order step 3): rebuilt trees replace
//! running ones between steps, keeping what can be kept.

use enso_block::{
    AccessError, Block, BuildError, Child, Delivery, Engine, EngineError, In, Inbox, Io, LinkError, Linker, Out,
    Outbox, Ports, Registrar, Workstream,
};

// ---- blocks -------------------------------------------------------------------------

/// Counts its steps: state that should survive rebuilds.
#[derive(Ports, Default)]
struct Counter {
    #[state]
    count: u32,
    #[output]
    out: Out<u32>,
}

impl Block for Counter {
    fn process(&mut self, io: &Io) {
        self.count += 1;
        *io.write(self.out) = self.count;
    }
}

/// Adds `amount` (a knob) to its input.
#[derive(Ports, Default)]
struct Offset {
    #[input(name = "in")]
    input: In<u32>,
    #[input(default = 100)]
    amount: In<u32>,
    #[output]
    out: Out<u32>,
}

impl Block for Offset {
    fn process(&mut self, io: &Io) {
        *io.write(self.out) = *io.read(self.input) + *io.read(self.amount);
    }
}

/// Counts its steps *without* marking the count `#[state]`, and has an
/// extra output when configured to: a rebuild applies the new configuration
/// and starts the count over.
#[derive(Ports)]
struct Shape {
    extra: bool,
    count: u32,
    #[output]
    out: Out<u32>,
    more: Option<Out<u32>>,
}

impl Block for Shape {
    fn register(&mut self, r: &mut Registrar) {
        self.more = self.extra.then(|| r.output("more", 0));
    }
    fn process(&mut self, io: &Io) {
        self.count += 1;
        *io.write(self.out) = self.count;
    }
}

/// The tree being rebuilt: a counter, optionally followed by an offset.
#[derive(Ports)]
struct Patch {
    with_offset: bool,
    shape_extra: bool,
    #[child]
    counter: Child<Counter>,
    #[child(init = Shape { extra: self.shape_extra, count: 0, out: Out::default(), more: None })]
    shape: Child<Shape>,
    offset: Option<Child<Offset>>,
}

impl Patch {
    fn new(with_offset: bool) -> Self {
        Self { with_offset, shape_extra: false, counter: Child::default(), shape: Child::default(), offset: None }
    }
}

impl Block for Patch {
    fn register(&mut self, r: &mut Registrar) {
        self.offset = self.with_offset.then(|| r.child("offset", Offset::default()));
    }
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        if let Some(offset) = self.offset {
            l.connect(l[self.counter].out, l[offset].input)?;
        }
        Ok(())
    }
}

/// Handles of a `Patch`, read once and kept across rebuilds.
struct Handles {
    count: Out<u32>,
    shape: Out<u32>,
}

fn handles(ws: &Workstream) -> Handles {
    let root = ws.root::<Patch>().unwrap();
    Handles { count: ws.get(root.counter).unwrap().out, shape: ws.get(root.shape).unwrap().out }
}

// ---- standalone workstreams ---------------------------------------------------------

#[test]
fn block_state_and_handles_survive_a_rebuild() {
    let mut ws = Workstream::build(Patch::new(false)).unwrap();
    let h = handles(&ws);
    for _ in 0..3 {
        ws.step();
    }
    assert_eq!(*ws.read(h.count).unwrap(), 3);

    // Add a block: the counter keeps running, with its state.
    ws.rebuild(Patch::new(true)).unwrap();
    ws.step();
    assert_eq!(*ws.read(h.count).unwrap(), 4, "the same counter instance carried on");
    let offset = ws.get(ws.root::<Patch>().unwrap().offset.unwrap()).unwrap().out;
    assert_eq!(*ws.read(offset).unwrap(), 104, "the new block is wired in");
}

#[test]
fn configuration_applies_and_only_state_fields_carry_over() {
    let mut ws = Workstream::build(Patch::new(false)).unwrap();
    let h = handles(&ws);
    for _ in 0..5 {
        ws.step();
    }
    let changed = Patch { shape_extra: true, ..Patch::new(false) };
    ws.rebuild(changed).unwrap();
    ws.step();
    assert_eq!(*ws.read(h.count).unwrap(), 6, "#[state] count carried over");
    assert_eq!(*ws.read(h.shape).unwrap(), 1, "a plain field starts fresh");
    // The new configuration took effect: the extra output exists now.
    let shape = ws.get(ws.root::<Patch>().unwrap().shape).unwrap();
    assert!(shape.extra && shape.more.is_some_and(|more| ws.read(more).is_ok()));
}

#[test]
fn custom_carry_over() {
    /// Keeps a history it moves over by hand (e.g. buffers that need care).
    #[derive(Ports, Default)]
    struct Recorder {
        history: Vec<u64>,
        #[output]
        out: Out<usize>,
    }
    impl Block for Recorder {
        fn carry_over(&mut self, previous: &mut dyn std::any::Any) {
            if let Some(previous) = previous.downcast_mut::<Self>() {
                std::mem::swap(&mut self.history, &mut previous.history);
            }
        }
        fn process(&mut self, io: &Io) {
            if self.history.len() < self.history.capacity() {
                self.history.push(io.step());
            }
            *io.write(self.out) = self.history.len();
        }
    }
    let recorder = || Recorder { history: Vec::with_capacity(16), ..Default::default() };
    let mut ws = Workstream::build(recorder()).unwrap();
    let out = ws.root::<Recorder>().unwrap().out;
    ws.step();
    ws.step();
    ws.rebuild(recorder()).unwrap();
    ws.step();
    assert_eq!(*ws.read(out).unwrap(), 3);
}

#[test]
fn removed_interfaces_are_gone_and_kept_values_carry_over() {
    let mut ws = Workstream::build(Patch::new(true)).unwrap();
    let offset = ws.get(ws.root::<Patch>().unwrap().offset.unwrap()).unwrap();
    let (amount, out) = (offset.amount, offset.out);
    ws.set(amount, 7).unwrap();
    ws.step();
    assert_eq!(*ws.read(out).unwrap(), 1 + 7);

    // Rebuild with the same structure: the knob keeps its value.
    ws.rebuild(Patch::new(true)).unwrap();
    ws.step();
    assert_eq!(*ws.read(out).unwrap(), 2 + 7, "knob setting survived");

    // Remove the offset: its handles now report it gone.
    ws.rebuild(Patch::new(false)).unwrap();
    assert_eq!(ws.read(out).err(), Some(AccessError::Gone));
    assert_eq!(ws.set(amount, 1).err(), Some(AccessError::Gone));
}

#[test]
fn a_failed_rebuild_changes_nothing() {
    #[derive(Ports, Default)]
    struct Loop {
        #[child]
        a: Child<Offset>,
        #[child]
        b: Child<Offset>,
    }
    impl Block for Loop {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect(l[self.a].out, l[self.b].input)?;
            l.connect(l[self.b].out, l[self.a].input)
        }
    }
    let mut ws = Workstream::build(Patch::new(false)).unwrap();
    let h = handles(&ws);
    ws.step();
    assert!(matches!(ws.rebuild(Loop::default()), Err(BuildError::Cycle(_))));
    ws.step();
    assert_eq!(*ws.read(h.count).unwrap(), 2, "still the old graph");
}

#[test]
fn a_rebuild_gives_crashed_blocks_a_fresh_start() {
    #[derive(Ports, Default)]
    struct Flaky {
        crashes: bool,
        #[output]
        out: Out<u32>,
    }
    impl Block for Flaky {
        fn process(&mut self, io: &Io) {
            assert!(!self.crashes, "bug");
            *io.write(self.out) = 1;
        }
    }
    #[derive(Ports)]
    struct Root {
        crashes: bool,
        #[child(init = Flaky { crashes: self.crashes, ..Default::default() })]
        flaky: Child<Flaky>,
    }
    impl Block for Root {}

    let mut ws = Workstream::build(Root { crashes: true, flaky: Child::default() }).unwrap();
    ws.step();
    assert_eq!(ws.disabled().collect::<Vec<_>>(), ["flaky"]);
    ws.rebuild(Root { crashes: false, flaky: Child::default() }).unwrap();
    ws.step();
    assert_eq!(ws.disabled().count(), 0);
    let out = ws.get(ws.root::<Root>().unwrap().flaky).unwrap().out;
    assert_eq!(*ws.read(out).unwrap(), 1);
}

// ---- through the engine -------------------------------------------------------------

/// Receives numbers and republishes their running total.
#[derive(Ports, Default)]
struct Totaliser {
    #[state]
    total: u32,
    #[inbox(name = "in")]
    input: Inbox<u32>,
    #[outbox]
    total_out: Outbox<u32>,
}

impl Block for Totaliser {
    fn process(&mut self, io: &Io) {
        for n in io.receive(self.input) {
            self.total += n;
        }
        io.send(self.total_out, self.total);
    }
}

#[test]
fn mailboxes_and_engine_links_survive_rebuilds() {
    let mut engine = Engine::new();
    let ws = engine.add("totals", Workstream::build(Totaliser::default()).unwrap());
    let root = engine.root::<Totaliser>(ws).unwrap();
    let (input, total_out) = (root.input, root.total_out);
    let sender = engine.sender(input).unwrap();
    let totals = engine.listen(total_out, Delivery::Latest).unwrap();

    sender.send(5);
    engine.step_all();
    assert_eq!(totals.try_recv(), Some(5));

    // Rebuild: the sender (made before) and the listener (an engine link)
    // keep working, and the block keeps its total.
    engine.rebuild(ws, Totaliser::default()).unwrap();
    sender.send(2);
    engine.step_all();
    assert_eq!(totals.try_recv(), Some(7));
}

#[test]
fn workstreams_controlled_by_an_engine_rebuild_through_it() {
    let mut engine = Engine::new();
    let id = engine.add("w", Workstream::build(Patch::new(false)).unwrap());
    let ws = engine.workstream_mut(id).unwrap();
    assert!(matches!(ws.rebuild(Patch::new(true)), Err(BuildError::Controlled)));
}

#[test]
fn rebuild_and_set_while_running_on_another_thread() {
    let mut engine = Engine::new();
    let id = engine.add("patch", Workstream::build(Patch::new(true)).unwrap());
    let root = engine.root::<Patch>(id).unwrap();
    let count = engine.get(root.counter).unwrap().out;
    let offset = engine.get(root.offset.unwrap()).unwrap();
    let (amount, out) = (offset.amount, offset.out);

    let mut runner = engine.take(id).unwrap();
    assert!(matches!(engine.root::<Patch>(id), Err(EngineError::Taken)));

    // The stepping thread hands its workstream back when told to stop.
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        while stop_rx.try_recv().is_err() {
            runner.step();
            std::thread::yield_now();
        }
        runner.step(); // apply anything sent just before stopping
        runner
    });

    // While it runs: change a knob, rebuild (same structure), free garbage.
    engine.set(amount, 1000).unwrap();
    for _ in 0..20 {
        engine.rebuild(id, Patch::new(true)).unwrap();
        engine.collect_garbage();
        std::thread::yield_now();
    }
    stop_tx.send(()).unwrap();
    let runner = thread.join().unwrap();
    engine.collect_garbage();

    let steps = runner.steps() as u32;
    assert_eq!(*runner.read(count).unwrap(), steps, "the counter never restarted across 20 swaps");
    assert_eq!(*runner.read(out).unwrap(), steps + 1000, "the knob set from here arrived");
}
