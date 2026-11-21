//! The block platform's rules, exercised with small arithmetic blocks.
//! Each test names the design decision (docs/design/blocks.md) it covers.
//! Everything is reached by handle or ID; labels only appear in display
//! paths and error messages (decision 23).

use enso_block::{
    AccessError, Block, BlockError, BlockId, BuildError, BuildOptions, Child, Events, In, InterfaceId, Io, Key,
    LinkError, Linker, LoadContext, MergeMode, Out, Ports, Registrar, Spec, Stamped, Workstream,
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
        // `format!(..)` → a `Spec` keyed by the name's hash.
        self.inputs = (0..self.count).map(|i| r.input(format!("in{i}"), 0.0)).collect();
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
    let out = ws.root::<Constant>().unwrap().out;
    assert_eq!(*ws.read(out).unwrap(), 4.0);
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

/// The handles of `TwoAdds`'s children, copied out (they're `Copy`).
fn two_adds(ws: &Workstream) -> (In<f32>, In<f32>, In<f32>, Out<f32>) {
    let root = ws.root::<TwoAdds>().unwrap();
    let (first, second) = (ws.get(root.first).unwrap(), ws.get(root.second).unwrap());
    (first.b, second.a, second.b, second.out)
}

#[test]
fn ports_order_execution_within_one_step() {
    let mut ws = Workstream::build(TwoAdds::default()).unwrap();
    let (first_b, _, second_b, out) = two_adds(&ws);
    ws.set(first_b, 10.0).unwrap();
    ws.set(second_b, 100.0).unwrap();
    ws.step();
    // One step is enough: source, then first, then second.
    assert_eq!(*ws.read(out).unwrap(), 111.0);
}

#[test]
fn unconnected_inputs_are_knobs_connected_ones_are_not() {
    let mut ws = Workstream::build(TwoAdds::default()).unwrap();
    let (_, second_a, _, _) = two_adds(&ws);
    assert_eq!(ws.set(second_a, 5.0), Err(AccessError::NotAKnob("second/a".into())));
}

#[test]
fn state_survives_steps() {
    let mut ws = Workstream::build(Counter::default()).unwrap();
    for _ in 0..3 {
        ws.step();
    }
    let out = ws.root::<Counter>().unwrap().out;
    assert_eq!(*ws.read(out).unwrap(), 3.0);
    assert_eq!(ws.steps(), 3);
}

// ---- identity -----------------------------------------------------------------------

#[test]
fn ids_are_the_same_on_every_build() {
    // Decision 25: give the root the same ID, get the same IDs everywhere.
    let root = BlockId::random();
    let options = BuildOptions { id: Some(root), ..Default::default() };
    let a = Workstream::build_with(TwoAdds::default(), options).unwrap();
    let b = Workstream::build_with(TwoAdds::default(), options).unwrap();
    let ids = |ws: &Workstream| ws.interfaces().iter().map(|i| i.id).collect::<Vec<_>>();
    assert_eq!(ids(&a), ids(&b));
    assert_eq!(a.id(), root);
    // A child's ID derives from its parent's and its key.
    let second = InterfaceId { block: root.child(Key::named("second")), key: Key::named("out") };
    assert!(ids(&a).contains(&second));
    // A random root gives different IDs.
    assert_ne!(ids(&a), ids(&Workstream::build(TwoAdds::default()).unwrap()));
}

#[test]
fn labels_may_repeat_keys_may_not() {
    // Decision 23: two inputs both labelled "level", told apart by key.
    #[derive(Ports, Default)]
    struct SameLabels {
        #[input(name = "level")]
        left: In<f32>,
        #[input(name = "level")]
        right: In<f32>,
    }
    impl Block for SameLabels {}
    let ws = Workstream::build(SameLabels::default()).unwrap();
    let labels: Vec<_> = ws.interfaces().iter().map(|i| i.path.as_str()).collect();
    assert_eq!(labels, ["level", "level"]);

    #[derive(Ports, Default)]
    struct SameKeys {
        a: Out<f32>,
        b: Out<f32>,
    }
    impl Block for SameKeys {
        fn register(&mut self, r: &mut Registrar) {
            self.a = r.output(Spec::new(Key::new(1), "a"), 0.0);
            self.b = r.output(Spec::new(Key::new(1), "b"), 0.0);
        }
    }
    let err = Workstream::build(SameKeys::default()).err().unwrap();
    assert!(err.to_string().contains("same key"), "{err}");
}

#[test]
fn explicit_ids_pin_keys() {
    // Decision 24: by default the key is the field name's hash; `id = N`
    // pins it, so the field (or label) can be renamed freely.
    #[derive(Ports, Default)]
    struct Pinned {
        #[input(id = 7, name = "brightness")]
        renamed_field: In<f32>,
        #[input]
        cutoff: In<f32>,
    }
    impl Block for Pinned {}
    let ws = Workstream::build(Pinned::default()).unwrap();
    let keys: Vec<Key> = ws.interfaces().iter().map(|i| i.id.key).collect();
    assert_eq!(keys, [Key::new(7), Key::named("cutoff")]);
}

#[test]
fn data_driven_links_use_interface_ids() {
    // What a patch file does: link by (block ID, key), no handles, no names.
    #[derive(Ports, Default)]
    struct FromData {
        #[child(init = Constant::new(2.0))]
        source: Child<Constant>,
        #[child(init = Scale { factor: 5.0, ..Default::default() })]
        scale: Child<Scale>,
    }
    impl Block for FromData {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            let from = InterfaceId { block: l.id_of(self.source), key: Key::named("out") };
            let to = InterfaceId { block: l.id_of(self.scale), key: Key::named("input") };
            l.connect_ids(from, to)
        }
    }
    let mut ws = Workstream::build(FromData::default()).unwrap();
    ws.step();
    let scale = ws.root::<FromData>().unwrap().scale;
    let out_id = InterfaceId { block: ws.id().child(Key::named("scale")), key: Key::named("out") };
    assert_eq!(*ws.read_id::<f32>(out_id).unwrap(), 10.0);
    assert_eq!(*ws.read(ws.get(scale).unwrap().out).unwrap(), 10.0);
}

#[test]
fn handles_from_another_workstream_are_refused() {
    let a = Workstream::build(Counter::default()).unwrap();
    let mut b = Workstream::build(Constant::new(1.0)).unwrap();
    let a_out = a.root::<Counter>().unwrap().out;
    assert_eq!(b.read(a_out).err(), Some(AccessError::ForeignHandle));
    let b_value = b.root::<Constant>().unwrap().value;
    assert!(b.set(b_value, 3.0).is_ok());
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
fn children_float_up() {
    let mut ws = Workstream::build(Chain::default()).unwrap();
    let mut paths: Vec<_> = ws.interfaces().iter().map(|i| i.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["scale/in", "scale/out", "source/out", "source/value"]);
    let root = ws.root::<Chain>().unwrap();
    let (value, out) = (ws.get(root.source).unwrap().value, ws.get(root.scale).unwrap().out);
    ws.step();
    assert_eq!(*ws.read(out).unwrap(), 30.0);
    // A knob deep inside, by handle.
    ws.set(value, 4.0).unwrap();
    ws.step();
    assert_eq!(*ws.read(out).unwrap(), 40.0);
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
fn private_hides_and_alias_adds_an_identity() {
    let ws = Workstream::build(PrivateAlias::default()).unwrap();
    let mut paths: Vec<_> = ws.interfaces().iter().map(|i| i.path.as_str()).collect();
    paths.sort();
    // `scale/in` is private; `out` is a second identity for `scale/out`.
    assert_eq!(paths, ["out", "scale/out", "source/out", "source/value"]);
    let alias = InterfaceId { block: ws.id(), key: Key::named("out") };
    assert!(ws.interfaces().iter().any(|i| i.id == alias));

    // A private interface can't be reached from outside either.
    let scale = ws.root::<PrivateAlias>().unwrap().scale;
    let input = ws.get(scale).unwrap().input;
    assert_eq!(ws.read(input).err(), Some(AccessError::NotExposed("scale/in".into())));
}

#[test]
fn a_parent_can_link_what_its_children_float_up_from_their_children() {
    // Decision 14: the root links a grandchild's port and an alias.
    #[derive(Ports, Default)]
    struct Root {
        #[child]
        chain: Child<PrivateAlias>,
        #[child]
        add: Child<Add>,
    }
    impl Block for Root {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            let chain = &l[self.chain];
            let (alias_out, source) = (
                InterfaceId { block: l.id_of(self.chain), key: Key::named("out") },
                chain.source,
            );
            let add = InterfaceId { block: l.id_of(self.add), key: Key::named("a") };
            l.connect_ids(alias_out, add)?; // via the alias
            // A grandchild, by handle: `l[child]` reaches through the chain.
            let source_out = l[source].out;
            l.connect(source_out, l[self.add].b)
        }
    }
    let mut ws = Workstream::build(Root::default()).unwrap();
    ws.step();
    let add = ws.root::<Root>().unwrap().add;
    assert_eq!(*ws.read(ws.get(add).unwrap().out).unwrap(), 3.0 + 1.0);
}

#[test]
fn private_interfaces_cant_be_linked_from_above() {
    #[derive(Ports, Default)]
    struct Root {
        #[child]
        chain: Child<PrivateAlias>,
        #[child(init = Constant::new(1.0))]
        source: Child<Constant>,
    }
    impl Block for Root {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            // `scale/in` is private inside `chain`.
            let scale = l[self.chain].scale;
            let hidden = l[scale].input;
            l.connect(l[self.source].out, hidden)
        }
    }
    let err = Workstream::build(Root::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::NotVisible(ref p), .. } if p == "chain/scale/in"), "{err}");
}

#[test]
fn a_parent_cant_link_its_own_ports() {
    // Decision 8: parents wire children to each other, never to themselves.
    #[derive(Ports, Default)]
    struct Selfish {
        #[child(init = Constant::new(1.0))]
        source: Child<Constant>,
        #[input(name = "in")]
        input: In<f32>,
    }
    impl Block for Selfish {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect(l[self.source].out, self.input)
        }
    }
    let err = Workstream::build(Selfish::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::NotVisible(ref p), .. } if p == "in"), "{err}");
}

// ---- link checking ------------------------------------------------------------------

#[test]
fn types_are_checked_when_linking_by_id() {
    #[derive(Ports, Default)]
    struct Mismatch {
        #[child]
        count: Child<Counter>,
        #[child]
        events: Child<EventSink>,
    }
    impl Block for Mismatch {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            let from = InterfaceId { block: l.id_of(self.count), key: Key::named("out") };
            let to = InterfaceId { block: l.id_of(self.events), key: Key::named("input") };
            l.connect_ids(from, to)
        }
    }
    let err = Workstream::build(Mismatch::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::TypeMismatch { .. }, .. }), "{err}");
}

#[test]
fn unknown_ids_are_reported() {
    #[derive(Ports, Default)]
    struct Lost {
        #[child]
        count: Child<Counter>,
    }
    impl Block for Lost {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            let id = InterfaceId { block: BlockId::random(), key: Key::named("out") };
            l.connect_ids(id, id)
        }
    }
    let err = Workstream::build(Lost::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::UnknownInterface(_), .. }), "{err}");
}

#[test]
fn single_inputs_take_one_link() {
    #[derive(Ports, Default)]
    struct Twice {
        #[child(init = Constant::new(1.0))]
        a: Child<Constant>,
        #[child(init = Constant::new(2.0))]
        b: Child<Constant>,
        #[child]
        scale: Child<Scale>,
    }
    impl Block for Twice {
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
        #[child]
        a: Child<Scale>,
        #[child]
        b: Child<Scale>,
    }
    impl Block for Loop {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect(l[self.a].out, l[self.b].input)?;
            l.connect(l[self.b].out, l[self.a].input)
        }
    }
    let err = Workstream::build(Loop::default()).err().unwrap();
    assert!(matches!(err, BuildError::Cycle(ref blocks) if blocks.contains(&"a".to_owned())), "{err}");
}

// ---- merging ------------------------------------------------------------------------

#[test]
fn sum_inputs_add_their_links() {
    #[derive(Ports, Default)]
    struct Summing {
        parts: Vec<Child<Constant>>,
        #[child]
        bus: Child<Bus>,
    }
    impl Block for Summing {
        fn register(&mut self, r: &mut Registrar) {
            self.parts = (1..=3).map(|i| r.child(format!("c{i}"), Constant::new(i as f32))).collect();
        }
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            for &part in &self.parts {
                l.connect(l[part].out, l[self.bus].input)?;
            }
            Ok(())
        }
    }
    let mut ws = Workstream::build(Summing::default()).unwrap();
    let out = ws.get(ws.root::<Summing>().unwrap().bus).unwrap().out;
    ws.step();
    assert_eq!(*ws.read(out).unwrap(), 6.0);
    ws.step(); // cleared between steps, not accumulated
    assert_eq!(*ws.read(out).unwrap(), 6.0);
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
        #[child(init = EventSource { notes: vec![10, 30], ..Default::default() })]
        a: Child<EventSource>,
        #[child(init = EventSource { notes: vec![5, 20, 30], ..Default::default() })]
        b: Child<EventSource>,
        #[child]
        sink: Child<EventSink>,
    }
    impl Block for Streams {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect(l[self.a].out, l[self.sink].input)?;
            l.connect(l[self.b].out, l[self.sink].input)
        }
    }
    let mut ws = Workstream::build(Streams::default()).unwrap();
    let input = ws.get(ws.root::<Streams>().unwrap().sink).unwrap().input;
    ws.step();
    let merged = ws.read(input).unwrap();
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
            self.parts = (0..4).map(|i| r.child(format!("c{i}"), Constant::new(1.0 + i as f32))).collect();
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
    let out = ws.get(ws.root::<Mix>().unwrap().mixer.unwrap()).unwrap().out;
    ws.step();
    assert_eq!(*ws.read(out).unwrap(), 1.0 + 2.0 + 3.0 + 4.0);
    assert_eq!(ws.interfaces().iter().filter(|i| i.path.starts_with("mixer/in")).count(), 4);
}

#[test]
fn load_failures_name_the_block() {
    #[derive(Ports)]
    struct Missing;
    impl Block for Missing {
        fn load(&mut self, cx: &mut LoadContext) -> Result<(), BlockError> {
            Err(format!("no file for {}", cx.path()).into())
        }
    }
    #[derive(Ports, Default)]
    struct MissingHolder;
    impl Block for MissingHolder {
        fn register(&mut self, r: &mut Registrar) {
            r.child("missing", Missing);
        }
    }
    #[derive(Ports, Default)]
    struct Root {
        #[child(name = "holder")]
        inner: Child<MissingHolder>,
    }
    impl Block for Root {}
    let err = Workstream::build(Root::default()).err().unwrap();
    assert_eq!(err.to_string(), "\"holder/missing\": failed to load: no file for holder/missing");
}

#[test]
fn a_panicking_block_is_switched_off() {
    #[derive(Ports, Default)]
    struct Faulty {
        #[output]
        out: Out<f32>,
    }
    impl Block for Faulty {
        fn process(&mut self, _: &Io) {
            panic!("bug");
        }
    }
    #[derive(Ports, Default)]
    struct Root {
        #[child]
        faulty: Child<Faulty>,
        #[child]
        counter: Child<Counter>,
    }
    impl Block for Root {}
    let mut ws = Workstream::build(Root::default()).unwrap();
    let out = ws.get(ws.root::<Root>().unwrap().counter).unwrap().out;
    ws.step();
    ws.step();
    assert_eq!(ws.disabled().collect::<Vec<_>>(), ["faulty"]);
    assert_eq!(*ws.read(out).unwrap(), 2.0);
}

// ---- #[derive(Ports)] ---------------------------------------------------------------

#[test]
fn derive_registers_in_field_order_with_labels_and_private() {
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
    let root = ws.root::<Annotated>().unwrap();
    let (out, inner_out) = (root.out, ws.get(root.constant).unwrap().out);
    assert_eq!(*ws.read(out).unwrap(), -1.0, "initial value before the first step");
    ws.step();
    assert_eq!(*ws.read(out).unwrap(), 0.0, "2.0 × the private knob's default (0.0)");
    ws.step();
    assert_eq!(*ws.read(inner_out).unwrap(), 7.0);
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
    let root = ws.root::<Through<f64>>().unwrap();
    let (input, out) = (root.input, root.out);
    ws.set(input, 2.5).unwrap();
    ws.step();
    assert_eq!(*ws.read(out).unwrap(), 2.5);
}
