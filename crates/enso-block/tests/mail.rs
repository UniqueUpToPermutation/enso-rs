//! Mailboxes and the engine (build-order step 2 of docs/design/blocks.md).

use enso_block::{
    Block, BuildError, Child, Delivery, Engine, EngineError, Inbox, Io, LinkError, Linker, Outbox, Ports,
    Workstream,
};

// ---- small blocks -------------------------------------------------------------------

/// Sends `step × 10 + k` for k in 0..per_step, every step.
#[derive(Ports, Default)]
struct Emitter {
    per_step: u32,
    #[outbox]
    out: Outbox<u32>,
}

impl Block for Emitter {
    fn process(&mut self, io: &Io) {
        for k in 0..self.per_step {
            io.send(self.out, io.step() as u32 * 10 + k);
        }
    }
}

/// Collects what it receives, by step.
#[derive(Ports, Default)]
struct Collector {
    /// (step, message)
    seen: Vec<(u64, u32)>,
    take: Option<usize>,
    #[inbox(name = "in", capacity = 8)]
    input: Inbox<u32>,
    /// Republishes how many it has seen.
    #[outbox]
    count: Outbox<u32>,
}

impl Block for Collector {
    fn process(&mut self, io: &Io) {
        let step = io.step();
        let messages = io.receive(self.input);
        let taken: Vec<u32> = match self.take {
            Some(n) => messages.take(n).collect(),
            None => messages.collect(),
        };
        self.seen.extend(taken.into_iter().map(|m| (step, m)));
        io.send(self.count, self.seen.len() as u32);
    }
}

/// An inbox with `Latest` delivery that records what it gets.
#[derive(Ports, Default)]
struct Gauge {
    #[inbox(latest)]
    level: Inbox<u32>,
    #[outbox]
    seen: Outbox<u32>,
}

impl Block for Gauge {
    fn process(&mut self, io: &Io) {
        for level in io.receive(self.level) {
            io.send(self.seen, level);
        }
    }
}

// ---- within one workstream ----------------------------------------------------------

#[derive(Ports, Default)]
struct Pair {
    #[child(init = Emitter { per_step: 1, ..Default::default() })]
    emitter: Child<Emitter>,
    #[child]
    collector: Child<Collector>,
}

impl Block for Pair {
    fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
        l.connect_mail(l[self.emitter].out, l[self.collector].input)
    }
}

#[test]
fn messages_arrive_at_the_next_step_and_dont_order_execution() {
    let mut engine = Engine::new();
    engine.add("main", Workstream::build(Pair::default()).unwrap()).unwrap();
    let counts = engine.listen::<u32>("main:collector/count", Delivery::Queue(16)).unwrap();

    engine.step_all();
    engine.step_all();
    engine.step_all();
    // Step 0 sees nothing (the step began before anything was sent);
    // step 1 sees step 0's message; step 2 sees step 1's.
    assert_eq!(counts.drain().collect::<Vec<_>>(), [0, 1, 2]);
}

#[test]
fn queues_drop_the_oldest_when_full() {
    #[derive(Ports, Default)]
    struct Flood {
        #[child(init = Emitter { per_step: 12, ..Default::default() })]
        emitter: Child<Emitter>,
        #[child]
        collector: Child<Collector>,
    }
    impl Block for Flood {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect_mail(l[self.emitter].out, l[self.collector].input)
        }
    }
    let mut ws = Workstream::build(Flood::default()).unwrap();
    ws.step(); // 12 messages into a queue of 8: the 4 oldest are pushed out
    assert_eq!(ws.dropped("collector/in"), Ok(4));
    // Next step the emitter runs first and sends 12 more before the
    // collector reads: all 8 waiting messages and 4 new ones are pushed out.
    ws.step();
    assert_eq!(ws.dropped("collector/in"), Ok(16));
}

#[test]
fn unread_messages_wait_for_later_steps() {
    let mut engine = Engine::new();
    #[derive(Ports, Default)]
    struct Slow {
        #[child(init = Emitter { per_step: 3, ..Default::default() })]
        emitter: Child<Emitter>,
        #[child(init = Collector { take: Some(1), ..Default::default() })]
        collector: Child<Collector>,
    }
    impl Block for Slow {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect_mail(l[self.emitter].out, l[self.collector].input)
        }
    }
    engine.add("main", Workstream::build(Slow::default()).unwrap()).unwrap();
    let counts = engine.listen::<u32>("main:collector/count", Delivery::Queue(16)).unwrap();
    for _ in 0..4 {
        engine.step_all();
    }
    // One message per step, oldest first, none lost (the queue holds 8).
    assert_eq!(counts.drain().collect::<Vec<_>>(), [0, 1, 2, 3]);
}

#[test]
fn latest_keeps_only_the_newest() {
    let mut engine = Engine::new();
    engine.add("gauge", Workstream::build(Gauge::default()).unwrap()).unwrap();
    let level = engine.sender::<u32>("gauge:level").unwrap();
    let seen = engine.listen::<u32>("gauge:seen", Delivery::Queue(16)).unwrap();

    for value in [3, 7, 9] {
        level.send(value);
    }
    engine.step_all();
    assert_eq!(seen.drain().collect::<Vec<_>>(), [9]);
    assert_eq!(level.dropped(), 0, "replacing a Latest value isn't a loss");
}

#[test]
fn outboxes_fan_out_and_inboxes_fan_in() {
    #[derive(Ports, Default)]
    struct Web {
        #[child(init = Emitter { per_step: 1, ..Default::default() })]
        a: Child<Emitter>,
        #[child(init = Emitter { per_step: 1, ..Default::default() })]
        b: Child<Emitter>,
        #[child]
        left: Child<Collector>,
        #[child]
        right: Child<Collector>,
    }
    impl Block for Web {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            // a → left and right; b → left (by path).
            l.connect_mail(l[self.a].out, l[self.left].input)?;
            l.connect_mail(l[self.a].out, l[self.right].input)?;
            l.connect_paths("b/out", "left/in")
        }
    }
    let mut engine = Engine::new();
    engine.add("main", Workstream::build(Web::default()).unwrap()).unwrap();
    let left = engine.listen::<u32>("main:left/count", Delivery::Latest).unwrap();
    let right = engine.listen::<u32>("main:right/count", Delivery::Latest).unwrap();
    engine.step_all();
    engine.step_all();
    assert_eq!((left.try_recv(), right.try_recv()), (Some(2), Some(1)));
}

#[test]
fn a_parent_cant_wire_its_own_mailboxes() {
    #[derive(Ports, Default)]
    struct Selfish {
        #[child]
        emitter: Child<Emitter>,
        #[inbox]
        mine: Inbox<u32>,
    }
    impl Block for Selfish {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect_mail(l[self.emitter].out, self.mine)
        }
    }
    let err = Workstream::build(Selfish::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::NotVisible(ref p), .. } if p == "mine"), "{err}");
}

#[test]
fn ports_and_mailboxes_dont_mix() {
    #[derive(Ports, Default)]
    struct Mixed {
        #[child]
        emitter: Child<Emitter>,
        #[child]
        number: Child<Number>,
    }
    #[derive(Ports, Default)]
    struct Number {
        #[input]
        value: enso_block::In<u32>,
    }
    impl Block for Number {}
    impl Block for Mixed {
        fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
            l.connect_paths("emitter/out", "number/value")
        }
    }
    let err = Workstream::build(Mixed::default()).err().unwrap();
    assert!(matches!(err, BuildError::Link { source: LinkError::WrongDirection(..), .. }), "{err}");
}

// ---- across workstreams -------------------------------------------------------------

#[test]
fn the_engine_links_workstreams_by_path() {
    let mut engine = Engine::new();
    engine.add("source", Workstream::build(Emitter { per_step: 2, ..Default::default() }).unwrap()).unwrap();
    engine.add("sink", Workstream::build(Collector::default()).unwrap()).unwrap();
    engine.connect_mail("source:out", "sink:/in").unwrap(); // a leading '/' is fine
    let counts = engine.listen::<u32>("sink:count", Delivery::Latest).unwrap();

    // `step_all` steps "source" then "sink": the sink's step begins after
    // the source has sent, so each round's 2 messages arrive that round.
    engine.step_all();
    engine.step_all();
    assert_eq!(counts.try_recv(), Some(4));
}

#[test]
fn engine_errors() {
    let mut engine = Engine::new();
    engine.add("a", Workstream::build(Emitter::default()).unwrap()).unwrap();
    engine.add("b", Workstream::build(Collector::default()).unwrap()).unwrap();
    assert_eq!(
        engine.add("a", Workstream::build(Emitter::default()).unwrap()),
        Err(EngineError::DuplicateWorkstream("a".into()))
    );
    assert_eq!(engine.connect_mail("nope:out", "b:in"), Err(EngineError::UnknownWorkstream("nope".into())));
    assert_eq!(engine.connect_mail("a-out", "b:in"), Err(EngineError::BadPath("a-out".into())));
    assert_eq!(engine.connect_mail("b:in", "a:out"), Err(EngineError::NotAnOutbox("b:in".into())));
    assert_eq!(engine.connect_mail("a:out", "b:count"), Err(EngineError::NotAnInbox("b:count".into())));
    assert!(matches!(engine.connect_mail("a:missing", "b:in"), Err(EngineError::Access { .. })));
    assert!(matches!(engine.sender::<String>("b:in"), Err(EngineError::Access { .. })), "wrong message type");
}

#[test]
fn type_mismatch_between_workstreams() {
    #[derive(Ports, Default)]
    struct TextSink {
        #[inbox(name = "in")]
        input: Inbox<String>,
    }
    impl Block for TextSink {}
    let mut engine = Engine::new();
    engine.add("a", Workstream::build(Emitter::default()).unwrap()).unwrap();
    engine.add("b", Workstream::build(TextSink::default()).unwrap()).unwrap();
    assert!(matches!(engine.connect_mail("a:out", "b:in"), Err(EngineError::TypeMismatch { .. })));
}

#[test]
fn realtime_workstreams_send_only_messages_that_own_no_memory() {
    #[derive(Ports, Default)]
    struct Chatty {
        #[outbox]
        log: Outbox<String>,
    }
    impl Block for Chatty {}
    let err = Workstream::build_realtime(Chatty::default()).err().unwrap();
    assert!(matches!(err, BuildError::NotRealtimeSafe { ref path, .. } if path == "log"), "{err}");

    // Plain numbers are fine, and so is *receiving* owned messages.
    assert!(Workstream::build_realtime(Emitter::default()).is_ok());
    #[derive(Ports, Default)]
    struct Reader {
        #[inbox]
        text: Inbox<String>,
    }
    impl Block for Reader {}
    assert!(Workstream::build_realtime(Reader::default()).is_ok());
}

#[test]
fn workstreams_on_their_own_threads() {
    let mut engine = Engine::new();
    engine.add("producer", Workstream::build(Emitter { per_step: 1, ..Default::default() }).unwrap()).unwrap();
    engine.add("consumer", Workstream::build(Collector::default()).unwrap()).unwrap();
    engine.connect_mail("producer:out", "consumer:in").unwrap();
    let counts = engine.listen::<u32>("consumer:count", Delivery::Latest).unwrap();

    let mut producer = engine.take("producer").unwrap();
    let mut consumer = engine.take("consumer").unwrap();
    // Workstreams are `Send`: each moves to its own thread.
    let a = std::thread::spawn(move || {
        for _ in 0..50 {
            producer.step();
            std::thread::yield_now();
        }
    });
    let b = std::thread::spawn(move || {
        for _ in 0..400 {
            consumer.step();
            std::thread::yield_now();
        }
        consumer
    });
    a.join().unwrap();
    let mut consumer = b.join().unwrap();
    consumer.step(); // collect anything that arrived after its last step
    // Up to 8 may be lost if the consumer fell behind (drop-oldest), never more.
    let received = std::iter::from_fn(|| counts.try_recv()).last().unwrap();
    let dropped = consumer.dropped("in").unwrap() as u32;
    assert_eq!(received + dropped, 50, "every message is either received or counted as dropped");
}
