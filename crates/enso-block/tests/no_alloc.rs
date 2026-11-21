//! A real-time workstream must never allocate or free while stepping,
//! including when it swaps in a rebuilt graph, a knob value or new mailbox
//! links (step 3). This binary installs an allocator that aborts on any
//! allocation inside `assert_no_alloc`.

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use enso_block::{Block, Child, Delivery, Engine, In, Inbox, Io, LinkError, Linker, Out, Outbox, Ports, Workstream};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

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

#[derive(Ports, Default)]
struct Meter {
    #[input(name = "in")]
    input: In<u32>,
    #[input(default = 1)]
    gain: In<u32>,
    #[inbox(capacity = 16)]
    notes: Inbox<u32>,
    #[outbox]
    level: Outbox<u32>,
    #[state]
    peak: u32,
}

impl Block for Meter {
    fn process(&mut self, io: &Io) {
        for note in io.receive(self.notes) {
            self.peak = self.peak.max(note);
        }
        io.send(self.level, *io.read(self.input) * *io.read(self.gain) + self.peak);
    }
}

#[derive(Ports)]
struct Patch {
    meters: usize,
    #[child]
    counter: Child<Counter>,
    #[child]
    meter: Child<Meter>,
    extra: Vec<Child<Meter>>,
}

impl Patch {
    fn new(meters: usize) -> Self {
        Self { meters, counter: Child::default(), meter: Child::default(), extra: Vec::new() }
    }
}

impl Block for Patch {
    fn register(&mut self, r: &mut enso_block::Registrar) {
        self.extra = (0..self.meters).map(|i| r.child(format!("extra{i}"), Meter::default())).collect();
    }
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.counter].out, l[self.meter].input)?;
        for &extra in &self.extra {
            l.connect_mail(l[self.meter].level, l[extra].notes)?;
        }
        Ok(())
    }
}

#[test]
fn stepping_and_swapping_never_allocate() {
    let mut engine = Engine::new();
    let id = engine.add("audio", Workstream::build_realtime(Patch::new(1)).unwrap());
    let root = engine.root::<Patch>(id).unwrap();
    let meter = engine.get(root.meter).unwrap();
    let (notes, level, gain) = (meter.notes, meter.level, meter.gain);
    let sender = engine.sender(notes).unwrap();
    let mut runner = engine.take(id).unwrap();

    for i in 0..200u32 {
        // Control-side work (may allocate: it's not the stepping thread).
        match i % 50 {
            10 => engine.rebuild(id, Patch::new(2 + (i as usize / 50) % 3)).unwrap(),
            20 => engine.set(gain, i).unwrap(),
            30 => drop(engine.listen(level, Delivery::Latest).unwrap()),
            _ => {}
        }
        sender.send(i);
        // The stepping side: applies whatever arrived, then runs.
        assert_no_alloc(|| runner.step());
        engine.collect_garbage();
    }
    assert_eq!(runner.steps(), 200);
    assert_eq!(runner.disabled().count(), 0);
    let count = runner.get(runner.root::<Patch>().unwrap().counter).unwrap().out;
    assert_eq!(*runner.read(count).unwrap(), 200, "the counter's state survived four rebuilds");
}
