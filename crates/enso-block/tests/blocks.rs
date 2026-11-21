//! The block platform's rules, exercised with small arithmetic blocks.
//! Each test names the design decision (docs/design/blocks.md) it covers.

use enso_block::{
    AccessError, Block, BlockError, BuildError, Child, Events, In, Io, LinkError, Linker, LoadContext, MergeMode,
    Out, Ports, Registrar, Stamped, Workstream,
};
use enso_core::Timestamp;

// ---- small blocks -------------------------------------------------------------------

/// Outputs `value` (its `value` input is a knob). The default comes from
/// `self`: the derive's code runs inside a method.
#[derive(Ports, Default)]
struct Constant {
    initial: f32,
    #[input(default = self.initial)]
    value: In<f32>,
    #[output]
    out: Out<f32>,
}

impl Constant {
    fn new(value: f32) -> Self {
        Self { initial: value, ..Default::default() }
    }
}

impl Block for Constant {
    fn process(&mut self, io: &Io) {
        *io.write(self.out) = *io.read(self.value);
    }
}

/// out = a + b (both knobs when unconnected).
#[derive(Ports, Default)]
struct Add {
    #[input]
    a: In<f32>,
    #[input]
    b: In<f32>,
    #[output]
    out: Out<f32>,
}

impl Block for Add {
    fn process(&mut self, io: &Io) {
        *io.write(self.out) = *io.read(self.a) + *io.read(self.b);
    }
}

/// out = in * factor.
#[derive(Ports, Default)]
struct Scale {
    factor: f32,
    #[input(name = "in")]
    input: In<f32>,
    #[output]
    out: Out<f32>,
}

impl Block for Scale {
    fn process(&mut self, io: &Io) {
        *io.write(self.out) = *io.read(self.input) * self.factor;
    }
}

/// Sums any number of links into one input (`Sum` merge).
#[derive(Ports, Default)]
struct Bus {
    #[input(name = "in", merge = Sum)]
    input: In<f32>,
    #[output]
    out: Out<f32>,
}

impl Block for Bus {
    fn process(&mut self, io: &Io) {
        *io.write(self.out) = *io.read(self.input);
    }
}

/// A mixer whose number of inputs is configuration (decision 9): the
/// fixed output is derived, the variable inputs registered by hand.
#[derive(Ports)]
struct Mixer {
    inputs: Vec<In<f32>>,
    count: usize,
    #[output]
    out: Out<f32>,
}

impl Mixer {
    fn new(count: usize) -> Self {
        Self { inputs: Vec::new(), count, out: Out::default() }
    }
}

impl Block for Mixer {
    fn register(&mut self, r: &mut Registrar) {
        self.inputs = (0..self.count).map(|i| r.input(&format!("in{i}"), 0.0)).collect();
    }
    fn process(&mut self, io: &Io) {
        *io.write(self.out) = self.inputs.iter().map(|&i| *io.read(i)).sum();
    }
}

/// Keeps state across steps.
#[derive(Ports, Default)]
struct Counter {
    count: f32,
    #[output]
    out: Out<f32>,
}

impl Block for Counter {
    fn process(&mut self, io: &Io) {
        self.count += 1.0;
        *io.write(self.out) = self.count;
    }
}

// ---- basics -------------------------------------------------------------------------

#[test]
fn a_leaf_runs() {
    let mut ws = Workstream::build(Constant::new(4.0)).unwrap();
    ws.step();
    assert_eq!(*ws.read::<f32>("out").unwrap(), 4.0);
}

#[derive(Ports, Default)]
struct TwoAdds {
    first: Child<Add>,
    second: Child<Add>,
    source: Child<Constant>,
}

impl Block for TwoAdds {
    fn register(&mut self, r: &mut Registrar) {
        // Registered in the *reverse* of data-flow order on purpose.
        self.second = r.child("second", Add::default());
        self.first = r.child("first", Add::default());
        self.source = r.child("source", Constant::new(1.0));
    }
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.source].out, l[self.first].a)?;
        l.connect(l[self.first].out, l[self.second].a)
    }
}

#[test]
fn ports_order_execution_within_one_step() {
    let mut ws = Workstream::build(TwoAdds::default()).unwrap();
    ws.set("first/b", 10.0f32).unwrap();
    ws.set("second/b", 100.0f32).unwrap();
    ws.step();
    // One step is enough: source, then first, then second.
    assert_eq!(*ws.read::<f32>("second/out").unwrap(), 111.0);
}

#[test]
fn unconnected_inputs_are_knobs_connected_ones_are_not() {
    let mut ws = Workstream::build(TwoAdds::default()).unwrap();
    assert_eq!(ws.set("second/a", 5.0f32), Err(AccessError::NotAKnob("second/a".into())));
    assert_eq!(
        ws.set("second/b", 5.0f64),
        Err(AccessError::WrongType { path: "second/b".into(), actual: "f32" })
    );
    assert_eq!(ws.set("nope", 1.0f32), Err(AccessError::Unknown("nope".into())));
}

#[test]
fn state_survives_steps() {
    let mut ws = Workstream::build(Counter::default()).unwrap();
    for _ in 0..3 {
        ws.step();
    }
    assert_eq!(*ws.read::<f32>("out").unwrap(), 3.0);
    assert_eq!(ws.steps(), 3);
}

// ---- interfaces: floating up, private, aliases --------------------------------------

/// A composite with no ports of its own (decision 8): its surface is what
/// its children float up.
#[derive(Ports, Default)]
struct Chain {
    #[child(init = Constant::new(3.0))]
    source: Child<Constant>,
    #[child(init = Scale { factor: 10.0, ..Default::default() })]
    scale: Child<Scale>,
}

impl Block for Chain {
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.source].out, l[self.scale].input)
    }
}

#[test]
fn children_float_up_by_path() {
    let mut ws = Workstream::build(Chain::default()).unwrap();
    let mut paths: Vec<_> = ws.interfaces().iter().map(|i| i.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["scale/in", "scale/out", "source/out", "source/value"]);
    ws.step();
    assert_eq!(*ws.read::<f32>("scale/out").unwrap(), 30.0);
    // A knob deep inside, set by path.
    ws.set("source/value", 4.0f32).unwrap();
    ws.step();
    assert_eq!(*ws.read::<f32>("scale/out").unwrap(), 40.0);
}

#[test]
fn private_hides_and_alias_renames() {
    let ws = Workstream::build(PrivateAlias::default()).unwrap();
    let mut paths: Vec<_> = ws.interfaces().iter().map(|i| i.path.as_str()).collect();
    paths.sort();
    // `scale/in` is private; `out` is a second name for `scale/out`.
    assert_eq!(paths, ["out", "scale/out", "source/out", "source/value"]);
}

#[derive(Ports, Default)]
struct PrivateAlias {
    source: Child<Constant>,
    scale: Child<Scale>,
}

impl Block for PrivateAlias {
    fn register(&mut self, r: &mut Registrar) {
        self.source = r.child("source", Constant::new(1.0));
        self.scale = r.child("scale", Scale { factor: 3.0, ..Default::default() });
        let scale = r.get(self.scale);
        let (input, output) = (scale.input, scale.out);
        r.private(input);
        r.alias("out", output);
    }
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect(l[self.source].out, l[self.scale].input)
    }
}

#[test]
fn a_parent_can_link_what_its_children_float_up_from_their_children() {
    // Decision 14: the root links `chain/scale/out` (a grandchild's port).
    #[derive(Ports, Default)]
    struct Root {
        chain: Child<PrivateAlias>,
        add: Child<Add>,
    }
    impl Block for Root {
        fn register(&mut self, r: &mut Registrar) {
            self.chain = r.child("chain", PrivateAlias::default());
            self.add = r.child("add", Add::default());
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect_paths("chain/out", "add/a")?; // via the alias
            l.connect_paths("chain/source/out", "add/b") // a grandchild, by its own path
        }
    }
    let mut ws = Workstream::build(Root::default()).unwrap();
    ws.step();
    assert_eq!(*ws.read::<f32>("add/out").unwrap(), 3.0 + 1.0);
}

#[test]
fn private_interfaces_cant_be_linked_from_above() {
    #[derive(Ports, Default)]
    struct Root {
        chain: Child<PrivateAlias>,
        source: Child<Constant>,
    }
    impl Block for Root {
        fn register(&mut self, r: &mut Registrar) {
            self.chain = r.child("chain", PrivateAlias::default());
            self.source = r.child("source", Constant::new(1.0));
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            // `scale/in` is private inside `chain`.
            l.connect_paths("source/out", "chain/scale/in")
        }
    }
    let err = Workstream::build(Root::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::UnknownPath(ref p), .. } if p == "chain/scale/in"), "{err}");
}

#[test]
fn a_parent_cant_link_its_own_ports() {
    // Decision 8: parents wire children to each other, never to themselves.
    #[derive(Ports, Default)]
    struct Selfish {
        source: Child<Constant>,
        input: In<f32>,
    }
    impl Block for Selfish {
        fn register(&mut self, r: &mut Registrar) {
            self.source = r.child("source", Constant::new(1.0));
            self.input = r.input("in", 0.0);
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect(l[self.source].out, self.input)
        }
    }
    let err = Workstream::build(Selfish::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::NotVisible(ref p), .. } if p == "in"), "{err}");
}

// ---- link checking ------------------------------------------------------------------

#[test]
fn types_are_checked_when_linking_by_path() {
    #[derive(Ports, Default)]
    struct Mismatch {
        count: Child<Counter>,
        events: Child<EventSink>,
    }
    impl Block for Mismatch {
        fn register(&mut self, r: &mut Registrar) {
            self.count = r.child("count", Counter::default());
            self.events = r.child("events", EventSink::default());
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect_paths("count/out", "events/in")
        }
    }
    let err = Workstream::build(Mismatch::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::TypeMismatch { .. }, .. }), "{err}");
}

#[test]
fn single_inputs_take_one_link() {
    #[derive(Ports, Default)]
    struct Twice {
        a: Child<Constant>,
        b: Child<Constant>,
        scale: Child<Scale>,
    }
    impl Block for Twice {
        fn register(&mut self, r: &mut Registrar) {
            self.a = r.child("a", Constant::new(1.0));
            self.b = r.child("b", Constant::new(2.0));
            self.scale = r.child("scale", Scale::default());
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect(l[self.a].out, l[self.scale].input)?;
            l.connect(l[self.b].out, l[self.scale].input)
        }
    }
    let err = Workstream::build(Twice::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::AlreadyLinked(ref p), .. } if p == "scale/in"), "{err}");
}

#[test]
fn cycles_are_rejected() {
    #[derive(Ports, Default)]
    struct Loop {
        a: Child<Scale>,
        b: Child<Scale>,
    }
    impl Block for Loop {
        fn register(&mut self, r: &mut Registrar) {
            self.a = r.child("a", Scale::default());
            self.b = r.child("b", Scale::default());
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect(l[self.a].out, l[self.b].input)?;
            l.connect(l[self.b].out, l[self.a].input)
        }
    }
    let err = Workstream::build(Loop::default()).err().unwrap();
    assert!(matches!(err, BuildError::Cycle(ref blocks) if blocks.contains(&"a".to_owned())), "{err}");
}

#[test]
fn registration_errors_are_reported() {
    #[derive(Ports, Default)]
    struct Clash {
        a: Out<f32>,
        b: Out<f32>,
    }
    impl Block for Clash {
        fn register(&mut self, r: &mut Registrar) {
            self.a = r.output("x", 0.0);
            self.b = r.output("x", 0.0);
        }
    }
    let err = Workstream::build(Clash::default()).err().unwrap();
    assert!(err.to_string().contains("used twice"), "{err}");
}

// ---- merging ------------------------------------------------------------------------

#[test]
fn sum_inputs_add_their_links() {
    #[derive(Ports, Default)]
    struct Summing {
        parts: Vec<Child<Constant>>,
        bus: Child<Bus>,
    }
    impl Block for Summing {
        fn register(&mut self, r: &mut Registrar) {
            self.parts = (1..=3).map(|i| r.child(&format!("c{i}"), Constant::new(i as f32))).collect();
            self.bus = r.child("bus", Bus::default());
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            for &part in &self.parts {
                l.connect(l[part].out, l[self.bus].input)?;
            }
            Ok(())
        }
    }
    let mut ws = Workstream::build(Summing::default()).unwrap();
    ws.step();
    assert_eq!(*ws.read::<f32>("bus/out").unwrap(), 6.0);
    ws.step(); // cleared between steps, not accumulated
    assert_eq!(*ws.read::<f32>("bus/out").unwrap(), 6.0);
}

#[derive(Debug, Clone, PartialEq)]
struct Note(u64);

impl Stamped for Note {
    fn timestamp(&self) -> Timestamp {
        Timestamp::from_nanos(self.0)
    }
}

/// Emits fixed notes.
#[derive(Ports, Default)]
struct EventSource {
    notes: Vec<u64>,
    #[output(initial = Events::with_capacity(16))]
    out: Out<Events<Note>>,
}

impl Block for EventSource {
    fn process(&mut self, io: &Io) {
        let mut out = io.write(self.out);
        out.clear();
        for &n in &self.notes {
            out.push(Note(n));
        }
    }
}

#[derive(Ports, Default)]
struct EventSink {
    #[input(name = "in", default = Events::with_capacity(64), merge = Append)]
    input: In<Events<Note>>,
}

impl Block for EventSink {}

#[test]
fn append_inputs_merge_by_timestamp() {
    // Decision 19: Append sorts by time, stably (equal times keep link order).
    #[derive(Ports, Default)]
    struct Streams {
        a: Child<EventSource>,
        b: Child<EventSource>,
        sink: Child<EventSink>,
    }
    impl Block for Streams {
        fn register(&mut self, r: &mut Registrar) {
            self.a = r.child("a", EventSource { notes: vec![10, 30], ..Default::default() });
            self.b = r.child("b", EventSource { notes: vec![5, 20, 30], ..Default::default() });
            self.sink = r.child("sink", EventSink::default());
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect(l[self.a].out, l[self.sink].input)?;
            l.connect(l[self.b].out, l[self.sink].input)
        }
    }
    let mut ws = Workstream::build(Streams::default()).unwrap();
    ws.step();
    let merged = ws.read::<Events<Note>>("sink/in").unwrap();
    let times: Vec<u64> = merged.as_slice().iter().map(|n| n.0).collect();
    assert_eq!(times, [5, 10, 20, 30, 30]);
}

#[test]
fn merge_modes_must_suit_the_type() {
    #[derive(Ports)]
    struct BadMerge;
    impl Block for BadMerge {
        fn register(&mut self, r: &mut Registrar) {
            // Lists append; they can't be summed.
            let _ = r.input_merged("in", Vec::<u8>::new(), MergeMode::Sum);
        }
    }
    let err = Workstream::build(BadMerge).err().unwrap();
    assert!(err.to_string().contains("doesn't support Sum"), "{err}");
}

// ---- configuration, loading, failures -----------------------------------------------

#[test]
fn ports_can_depend_on_configuration() {
    #[derive(Ports, Default)]
    struct Mix {
        mixer: Option<Child<Mixer>>,
        parts: Vec<Child<Constant>>,
    }
    impl Block for Mix {
        fn register(&mut self, r: &mut Registrar) {
            self.parts = (0..4).map(|i| r.child(&format!("c{i}"), Constant::new(1.0 + i as f32))).collect();
            self.mixer = Some(r.child("mixer", Mixer::new(4)));
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            let mixer = self.mixer.unwrap();
            for (i, &part) in self.parts.iter().enumerate() {
                l.connect(l[part].out, l[mixer].inputs[i])?;
            }
            Ok(())
        }
    }
    let mut ws = Workstream::build(Mix::default()).unwrap();
    ws.step();
    assert_eq!(*ws.read::<f32>("mixer/out").unwrap(), 1.0 + 2.0 + 3.0 + 4.0);
    assert!(ws.interfaces().iter().any(|i| i.path == "mixer/in3"));
}

#[test]
fn load_failures_name_the_block() {
    #[derive(Ports)]
    struct Missing;
    impl Block for Missing {
        fn register(&mut self, _: &mut Registrar) {}
        fn load(&mut self, cx: &mut LoadContext) -> Result<(), BlockError> {
            Err(format!("no file for {}", cx.path()).into())
        }
    }
    #[derive(Ports, Default)]
    struct Root {
        inner: Child<MissingHolder>,
    }
    #[derive(Ports, Default)]
    struct MissingHolder;
    impl Block for MissingHolder {
        fn register(&mut self, r: &mut Registrar) {
            r.child("missing", Missing);
        }
    }
    impl Block for Root {
        fn register(&mut self, r: &mut Registrar) {
            self.inner = r.child("holder", MissingHolder);
        }
    }
    let err = Workstream::build(Root::default()).err().unwrap();
    assert_eq!(err.to_string(), "\"holder/missing\": failed to load: no file for holder/missing");
}

#[test]
fn a_panicking_block_is_switched_off() {
    #[derive(Ports)]
    struct Faulty {
        out: Out<f32>,
    }
    impl Block for Faulty {
        fn register(&mut self, r: &mut Registrar) {
            self.out = r.output("out", 0.0);
        }
        fn process(&mut self, _: &Io) {
            panic!("bug");
        }
    }
    #[derive(Ports, Default)]
    struct Root {
        faulty: Option<Child<Faulty>>,
        counter: Child<Counter>,
    }
    impl Block for Root {
        fn register(&mut self, r: &mut Registrar) {
            self.faulty = Some(r.child("faulty", Faulty { out: Out::default() }));
            self.counter = r.child("counter", Counter::default());
        }
    }
    let mut ws = Workstream::build(Root::default()).unwrap();
    ws.step();
    ws.step();
    assert_eq!(ws.disabled().collect::<Vec<_>>(), ["faulty"]);
    assert_eq!(*ws.read::<f32>("counter/out").unwrap(), 2.0);
}

// ---- #[derive(Ports)] ---------------------------------------------------------------

#[test]
fn derive_registers_in_field_order_with_names_and_private() {
    #[derive(Ports, Default)]
    struct Annotated {
        #[input(default = 2.0)]
        gain: In<f32>,
        #[private]
        #[input(name = "secret")]
        hidden: In<f32>,
        #[output(name = "result", initial = -1.0)]
        out: Out<f32>,
        #[child(name = "inner", init = Constant::new(7.0))]
        constant: Child<Constant>,
        steps_seen: u32,
    }
    impl Block for Annotated {
        fn process(&mut self, io: &Io) {
            self.steps_seen += 1;
            *io.write(self.out) = *io.read(self.gain) * *io.read(self.hidden);
        }
    }

    let mut ws = Workstream::build(Annotated::default()).unwrap();
    let paths: Vec<_> = ws.interfaces().iter().map(|i| i.path.as_str()).collect();
    // Field order; `secret` is private; the child floats up as `inner/...`.
    assert_eq!(paths, ["gain", "result", "inner/value", "inner/out"]);
    assert_eq!(*ws.read::<f32>("result").unwrap(), -1.0, "initial value before the first step");
    ws.step();
    assert_eq!(*ws.read::<f32>("result").unwrap(), 0.0, "2.0 × the private knob's default (0.0)");
    ws.step();
    assert_eq!(*ws.read::<f32>("inner/out").unwrap(), 7.0);
}

#[test]
fn derive_works_on_generic_blocks() {
    /// Passes any summable value through.
    #[derive(Ports, Default)]
    struct Through<T: enso_block::Merge + Default + Copy> {
        #[input(merge = Sum)]
        input: In<T>,
        #[output]
        out: Out<T>,
    }
    impl<T: enso_block::Merge + Default + Copy> Block for Through<T> {
        fn process(&mut self, io: &Io) {
            *io.write(self.out) = *io.read(self.input);
        }
    }
    let mut ws = Workstream::build(Through::<f64>::default()).unwrap();
    ws.set("input", 2.5f64).unwrap();
    ws.step();
    assert_eq!(*ws.read::<f64>("out").unwrap(), 2.5);
}
