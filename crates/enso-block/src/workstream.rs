//! [`Workstream`]: runs one block tree, step after step.

use std::{
    any::{Any, TypeId},
    cell::{Cell, Ref, RefCell, RefMut},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use crate::{
    Block, BlockId, BuildError, Child, In, Inbox, Interface, InterfaceId, InterfaceKind, MergeMode, Message, Out,
    Outbox, PortHandle,
    build::Build,
    mail::{AnyMailbox, MailFns, Mailbox},
    merge::{ClearFn, MergeFn},
};

/// One input's fan-in, done before its block runs.
struct MergeStep {
    into: usize,
    sources: Vec<usize>,
    mode: MergeMode,
    clear: ClearFn,
    merge: MergeFn,
}

/// Runs one block tree. Single-threaded: [`step`](Self::step) runs every
/// block once, in dependency order.
///
/// Port values live here, one storage slot per output (and per input that
/// keeps its own value), each in a `RefCell`. That's what lets
/// [`Io::read`] and [`Io::write`] hand out references to different slots at
/// once. `RefCell` checks borrows with a plain counter: no locks, no
/// allocation, fine on the audio thread.
///
/// A workstream is `Send`: build it anywhere, then move it to the thread
/// that steps it.
pub struct Workstream {
    blocks: Vec<Option<Box<dyn Block>>>,
    paths: Vec<String>,
    /// Block indices in execution order.
    order: Vec<u32>,
    /// Per block: merges to run before it.
    merges: Vec<Vec<MergeStep>>,
    slots: Vec<RefCell<Box<dyn Any + Send>>>,

    // Per interface ("port" index):
    port_slot: Vec<u32>,
    port_owner: Vec<u32>,
    port_kind: Vec<InterfaceKind>,
    port_type: Vec<(TypeId, &'static str)>,
    /// True for an input holding its own value with nothing linked: a knob
    /// that [`set`](Self::set) may change.
    knob: Vec<bool>,
    mail_fns: Vec<Option<MailFns>>,
    /// Inboxes: their mailbox.
    mailbox: Vec<Option<AnyMailbox>>,
    /// Outboxes: their targets (`Vec<Arc<Mailbox<M>>>`, type-erased).
    targets: Vec<Option<Box<dyn Any + Send + Sync>>>,
    /// Inboxes: messages this step may still take (those that had arrived
    /// when the step began).
    budget: Vec<Cell<usize>>,
    inboxes: Vec<u32>,

    /// For display: each port's labels from the root.
    port_path: Vec<String>,
    /// Which ports the root exposes (others can't be reached from outside).
    exposed: Vec<bool>,
    interfaces: Vec<Interface>,
    tree: u32,
    id: BlockId,
    realtime: bool,
    steps: u64,
    disabled: Vec<u32>,
}

/// How to build a [`Workstream`]; see [`Workstream::build_with`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    /// The root block's ID. Every other block's ID derives from it, so give
    /// the same ID on every run to keep saved references valid. Default:
    /// random.
    pub id: Option<BlockId>,
    /// Stepped on a real-time thread (see [`Workstream::build_realtime`]).
    pub realtime: bool,
}

impl Workstream {
    /// Register, load and link `root` and its children, then compile them
    /// into a runnable graph. The root gets a random [`BlockId`].
    pub fn build(root: impl Block) -> Result<Self, BuildError> {
        Self::build_with(root, BuildOptions::default())
    }

    /// Like [`build`](Self::build), for a workstream stepped on a real-time
    /// thread (audio). Its outboxes may only carry messages that own no
    /// memory (decision 13), so sending, or evicting from a full queue,
    /// never frees memory on that thread. Checked here and whenever the
    /// engine connects one of its outboxes.
    pub fn build_realtime(root: impl Block) -> Result<Self, BuildError> {
        Self::build_with(root, BuildOptions { realtime: true, ..Default::default() })
    }

    pub fn build_with(root: impl Block, options: BuildOptions) -> Result<Self, BuildError> {
        let id = options.id.unwrap_or_else(BlockId::random);
        Self::compile(Build::new(Box::new(root), id)?, options.realtime)
    }

    fn compile(mut build: Build, realtime: bool) -> Result<Self, BuildError> {
        let port_count = build.ports.len();
        let mut slots = Vec::new();
        let mut port_slot = vec![u32::MAX; port_count];
        let mut knob = vec![false; port_count];
        let mut new_slot = |value: Box<dyn Any + Send>| {
            slots.push(RefCell::new(value));
            (slots.len() - 1) as u32
        };

        // Outputs first: every output owns a slot.
        for (i, port) in build.ports.iter_mut().enumerate() {
            if port.kind == InterfaceKind::Out {
                port_slot[i] = new_slot(port.value.take().expect("value"));
            }
        }
        // Inputs: a single link reads the source's slot directly (no copy);
        // otherwise the input keeps its own value.
        let mut merges: Vec<Vec<MergeStep>> = (0..build.blocks.len()).map(|_| Vec::new()).collect();
        for (i, port) in build.ports.iter_mut().enumerate() {
            if port.kind != InterfaceKind::In {
                continue;
            }
            let sources: Vec<usize> = build
                .links
                .iter()
                .filter(|&&(_, to)| to as usize == i)
                .map(|&(from, _)| port_slot[from as usize] as usize)
                .collect();
            match (port.merge, sources.as_slice()) {
                (MergeMode::Single, [source]) => port_slot[i] = *source as u32,
                (_, []) => {
                    port_slot[i] = new_slot(port.value.take().expect("value"));
                    knob[i] = true;
                }
                (mode, _) => {
                    let into = new_slot(port.value.take().expect("value"));
                    port_slot[i] = into;
                    merges[port.owner as usize].push(MergeStep {
                        into: into as usize,
                        sources,
                        mode,
                        clear: port.clear.expect("merging inputs have merge functions"),
                        merge: port.merge_fn.expect("merging inputs have merge functions"),
                    });
                }
            }
        }

        // Mailboxes: one per inbox; outboxes get the inboxes linked to them.
        let mut mailbox: Vec<Option<AnyMailbox>> = vec![None; port_count];
        let mut targets: Vec<Option<Box<dyn Any + Send + Sync>>> = (0..port_count).map(|_| None).collect();
        let mut inboxes = Vec::new();
        for (i, port) in build.ports.iter().enumerate() {
            let Some(fns) = port.mail else { continue };
            match port.kind {
                InterfaceKind::Inbox => {
                    mailbox[i] = Some((fns.make_mailbox)(port.delivery));
                    inboxes.push(i as u32);
                }
                InterfaceKind::Outbox => {
                    if realtime && fns.drops {
                        return Err(BuildError::NotRealtimeSafe { path: build.port_path(i as u32), type_name: port.type_name });
                    }
                    targets[i] = Some((fns.make_targets)());
                }
                _ => {}
            }
        }
        for &(from, to) in &build.mail_links {
            let fns = build.ports[from as usize].mail.expect("outbox");
            let target = mailbox[to as usize].clone().expect("inbox");
            let list = targets[from as usize].as_deref_mut().expect("outbox");
            (fns.add_target)(list, target);
        }

        let order = execution_order(&build)?;
        let interfaces = std::mem::take(&mut build.blocks[0].dictionary);
        let mut exposed = vec![false; port_count];
        for interface in &interfaces {
            exposed[interface.port as usize] = true;
        }
        Ok(Self {
            port_path: (0..port_count as u32).map(|p| build.port_path(p)).collect(),
            exposed,
            tree: build.tree,
            id: build.blocks[0].id,
            paths: build.blocks.iter().map(|b| b.path.clone()).collect(),
            blocks: build.blocks.into_iter().map(|b| b.block).collect(),
            order,
            merges,
            slots,
            port_slot,
            port_owner: build.ports.iter().map(|p| p.owner).collect(),
            port_kind: build.ports.iter().map(|p| p.kind).collect(),
            port_type: build.ports.iter().map(|p| (p.type_id, p.type_name)).collect(),
            knob,
            mail_fns: build.ports.iter().map(|p| p.mail).collect(),
            mailbox,
            targets,
            budget: (0..port_count).map(|_| Cell::new(0)).collect(),
            inboxes,
            interfaces,
            realtime,
            steps: 0,
            disabled: Vec::new(),
        })
    }

    /// Run every block once, in dependency order. Inboxes deliver what had
    /// arrived when the step began; later messages wait for the next step. A
    /// block that panics is switched off (see [`disabled`](Self::disabled));
    /// the rest carry on.
    pub fn step(&mut self) {
        for &inbox in &self.inboxes {
            let inbox = inbox as usize;
            let (Some(fns), Some(mailbox)) = (&self.mail_fns[inbox], &self.mailbox[inbox]) else { continue };
            self.budget[inbox].set((fns.pending)(&**mailbox));
        }

        for &block in &self.order {
            let block = block as usize;
            for merge in &self.merges[block] {
                let mut into = self.slots[merge.into].borrow_mut();
                (merge.clear)(&mut **into);
                for &source in &merge.sources {
                    (merge.merge)(&mut **into, &**self.slots[source].borrow(), merge.mode);
                }
            }

            let Some(routine) = self.blocks[block].as_mut() else { continue };
            let io = Io {
                block: block as u32,
                slots: &self.slots,
                port_slot: &self.port_slot,
                port_owner: &self.port_owner,
                mailbox: &self.mailbox,
                targets: &self.targets,
                budget: &self.budget,
                step: self.steps,
            };
            if catch_unwind(AssertUnwindSafe(|| routine.process(&io))).is_err() {
                self.blocks[block] = None;
                self.disabled.push(block as u32);
            }
        }
        self.steps += 1;
    }

    /// Steps run so far.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Whether this workstream was built with [`build_realtime`](Self::build_realtime).
    pub fn is_realtime(&self) -> bool {
        self.realtime
    }

    /// The root block's identity.
    pub fn id(&self) -> BlockId {
        self.id
    }

    /// The root block's dictionary: everything it exposes, with IDs.
    pub fn interfaces(&self) -> &[Interface] {
        &self.interfaces
    }

    /// The root block, as its own type: the starting point for reaching
    /// handles from outside (`ws.get(ws.root::<Patch>()?.voice)`).
    pub fn root<R: Block>(&self) -> Option<&R> {
        self.blocks.first()?.as_deref().and_then(|b| (b as &dyn Any).downcast_ref::<R>())
    }

    /// A block of this tree, as its own type, to read its handles. `None`
    /// for a handle of another tree or a block that was switched off.
    pub fn get<B: Block>(&self, child: Child<B>) -> Option<&B> {
        if child.tree != self.tree {
            return None;
        }
        self.blocks.get(child.index as usize)?.as_deref().and_then(|b| (b as &dyn Any).downcast_ref::<B>())
    }

    /// The current value of a port (an output, or what an input sees). It
    /// must be exposed by the root.
    pub fn read<T: 'static>(&self, port: impl ValuePort<T>) -> Result<Ref<'_, T>, AccessError> {
        let port = self.handle(&port)?;
        Ok(self.value(port))
    }

    /// Like [`read`](Self::read), by ID (for tools working from data).
    pub fn read_id<T: 'static>(&self, id: InterfaceId) -> Result<Ref<'_, T>, AccessError> {
        let port = self.by_id(id)?;
        if self.port_kind[port].is_mailbox() {
            return Err(AccessError::NotAPort(self.port_path[port].clone()));
        }
        self.check_type::<T>(port)?;
        Ok(self.value(port))
    }

    /// Turn a knob: set the value of an unconnected input (exposed by the
    /// root). Call between steps.
    pub fn set<T: Send + 'static>(&mut self, input: In<T>, value: T) -> Result<(), AccessError> {
        let port = self.handle(&input)?;
        self.set_port(port, value)
    }

    /// Like [`set`](Self::set), by ID.
    pub fn set_id<T: Send + 'static>(&mut self, id: InterfaceId, value: T) -> Result<(), AccessError> {
        let port = self.by_id(id)?;
        self.check_type::<T>(port)?;
        self.set_port(port, value)
    }

    /// Messages an inbox (exposed by the root) lost to a full queue.
    pub fn dropped<M>(&self, inbox: Inbox<M>) -> Result<u64, AccessError> {
        let port = self.handle(&inbox)?;
        match (&self.mail_fns[port], &self.mailbox[port]) {
            (Some(fns), Some(mailbox)) => Ok((fns.dropped)(&**mailbox)),
            _ => Err(AccessError::NotAnInbox(self.port_path[port].clone())),
        }
    }

    /// Display paths of blocks switched off because they panicked.
    pub fn disabled(&self) -> impl Iterator<Item = &str> {
        self.disabled.iter().map(|&b| self.paths[b as usize].as_str())
    }

    fn value<T: 'static>(&self, port: usize) -> Ref<'_, T> {
        let slot = &self.slots[self.port_slot[port] as usize];
        Ref::map(slot.borrow(), |v| (**v).downcast_ref::<T>().expect("type checked"))
    }

    fn set_port<T: Send + 'static>(&mut self, port: usize, value: T) -> Result<(), AccessError> {
        if !self.knob[port] {
            return Err(AccessError::NotAKnob(self.port_path[port].clone()));
        }
        *self.slots[self.port_slot[port] as usize].borrow_mut() = Box::new(value);
        Ok(())
    }

    fn check_type<T: 'static>(&self, port: usize) -> Result<(), AccessError> {
        let (type_id, type_name) = self.port_type[port];
        if type_id != TypeId::of::<T>() {
            return Err(AccessError::WrongType { path: self.port_path[port].clone(), actual: type_name });
        }
        Ok(())
    }

    // ---- for the engine ------------------------------------------------------------

    pub(crate) fn tree(&self) -> u32 {
        self.tree
    }

    /// A handle's port, if it's from this tree and exposed by the root.
    /// (Typed handles need no type check: the compiler did it.)
    pub(crate) fn handle(&self, handle: &impl PortHandle) -> Result<usize, AccessError> {
        if handle.port_tree() == u32::MAX {
            return Err(AccessError::Unregistered);
        }
        if handle.port_tree() != self.tree {
            return Err(AccessError::ForeignHandle);
        }
        let port = handle.port_index() as usize;
        if !self.exposed.get(port).copied().unwrap_or(false) {
            return Err(AccessError::NotExposed(self.port_path.get(port).cloned().unwrap_or_default()));
        }
        Ok(port)
    }

    pub(crate) fn by_id(&self, id: InterfaceId) -> Result<usize, AccessError> {
        self.interfaces
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.port as usize)
            .ok_or(AccessError::UnknownInterface(id))
    }

    pub(crate) fn path_of(&self, port: usize) -> &str {
        &self.port_path[port]
    }

    pub(crate) fn kind(&self, port: usize) -> InterfaceKind {
        self.port_kind[port]
    }

    pub(crate) fn type_of(&self, port: usize) -> (TypeId, &'static str) {
        self.port_type[port]
    }

    pub(crate) fn mailbox_of(&self, port: usize) -> Option<AnyMailbox> {
        self.mailbox[port].clone()
    }

    /// Whether dropping the outbox's message type runs code.
    pub(crate) fn drops(&self, port: usize) -> bool {
        self.mail_fns[port].is_some_and(|f| f.drops)
    }

    /// Connect the outbox `port` to a mailbox (of the same message type).
    pub(crate) fn add_target(&mut self, port: usize, mailbox: AnyMailbox) -> bool {
        match (&self.mail_fns[port], self.targets[port].as_deref_mut()) {
            (Some(fns), Some(targets)) => (fns.add_target)(targets, mailbox),
            _ => false,
        }
    }

    /// The typed mailbox of the inbox `port`.
    pub(crate) fn typed_mailbox<M: Message>(&self, port: usize) -> Option<Arc<Mailbox<M>>> {
        self.mailbox[port].clone()?.downcast::<Mailbox<M>>().ok()
    }
}

/// Topological order of all blocks: a block runs after every block whose
/// outputs it reads. Kahn's algorithm, taking blocks in registration order
/// whenever there's a choice, so the order is deterministic. (Mail links
/// don't order anything: messages arrive by the next step.)
fn execution_order(build: &Build) -> Result<Vec<u32>, BuildError> {
    let count = build.blocks.len();
    let owner = |port: u32| build.ports[port as usize].owner as usize;
    let edges: Vec<(usize, usize)> = build.links.iter().map(|&(from, to)| (owner(from), owner(to))).collect();
    let mut incoming = vec![0usize; count];
    for &(_, to) in &edges {
        incoming[to] += 1;
    }
    let mut done = vec![false; count];
    let mut order = Vec::with_capacity(count);
    while order.len() < count {
        let Some(next) = (0..count).find(|&b| !done[b] && incoming[b] == 0) else {
            let stuck = (0..count).filter(|&b| !done[b]).map(|b| build.blocks[b].path.clone()).collect();
            return Err(BuildError::Cycle(stuck));
        };
        done[next] = true;
        order.push(next as u32);
        for &(from, to) in &edges {
            if from == next {
                incoming[to] -= 1;
            }
        }
    }
    Ok(order)
}

/// Handles whose value can be read from outside: [`In`] and [`Out`].
pub trait ValuePort<T>: PortHandle {}
impl<T> ValuePort<T> for In<T> {}
impl<T> ValuePort<T> for Out<T> {}

/// Reaching a workstream from outside failed. Paths are display labels.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AccessError {
    #[error("the root block exposes no interface {0:?}")]
    UnknownInterface(InterfaceId),
    #[error("a handle wasn't registered")]
    Unregistered,
    #[error("the handle belongs to a different workstream")]
    ForeignHandle,
    #[error("{0:?} isn't exposed by the root block (private somewhere on the way up?)")]
    NotExposed(String),
    #[error("{path:?} carries {actual}")]
    WrongType { path: String, actual: &'static str },
    #[error("{0:?} isn't an unconnected input")]
    NotAKnob(String),
    #[error("{0:?} is a mailbox, not a port")]
    NotAPort(String),
    #[error("{0:?} isn't an inbox")]
    NotAnInbox(String),
}

/// A block's access to its ports and mailboxes during [`Block::process`].
pub struct Io<'a> {
    block: u32,
    slots: &'a [RefCell<Box<dyn Any + Send>>],
    port_slot: &'a [u32],
    port_owner: &'a [u32],
    mailbox: &'a [Option<AnyMailbox>],
    targets: &'a [Option<Box<dyn Any + Send + Sync>>],
    budget: &'a [Cell<usize>],
    step: u64,
}

impl Io<'_> {
    /// Read an input: the linked output's value, the merged inputs, or the
    /// knob's value.
    pub fn read<T: 'static>(&self, input: In<T>) -> Ref<'_, T> {
        let slot = &self.slots[self.port_slot[self.own(input.index)] as usize];
        Ref::map(slot.borrow(), |v| (**v).downcast_ref::<T>().expect("port type"))
    }

    /// Write an output. Downstream blocks read it later in this step.
    pub fn write<T: 'static>(&self, output: Out<T>) -> RefMut<'_, T> {
        let slot = &self.slots[self.port_slot[self.own(output.index)] as usize];
        RefMut::map(slot.borrow_mut(), |v| (**v).downcast_mut::<T>().expect("port type"))
    }

    /// Take the messages that had arrived in `inbox` when this step began,
    /// oldest first. They're yours now: whatever you don't keep is freed when
    /// you drop it (on this thread; on a real-time workstream, receive only
    /// messages that own no memory, or pass them on to be freed elsewhere).
    /// Messages you don't take stay for the next step.
    pub fn receive<M: 'static>(&self, inbox: Inbox<M>) -> Receive<'_, M> {
        let port = self.own(inbox.index);
        let mailbox = self.mailbox[port].as_deref().and_then(|m| m.downcast_ref::<Mailbox<M>>());
        Receive { mailbox, budget: &self.budget[port] }
    }

    /// Send `message` to every inbox linked to `outbox` (cloned for all but
    /// the last). Never blocks: a full queue drops its oldest message.
    pub fn send<M: Clone + 'static>(&self, outbox: Outbox<M>, message: M) {
        let port = self.own(outbox.index);
        let Some(targets) = self.targets[port].as_deref().and_then(|t| t.downcast_ref::<Vec<Arc<Mailbox<M>>>>())
        else {
            return;
        };
        if let Some((last, rest)) = targets.split_last() {
            for target in rest {
                target.post(message.clone());
            }
            last.post(message);
        }
    }

    /// Which step this is (0, 1, 2, ...).
    pub fn step(&self) -> u64 {
        self.step
    }

    /// Panics (and so switches the block off) on another block's
    /// interface: a bug in the block, never user input.
    fn own(&self, port: u32) -> usize {
        let port = port as usize;
        assert!(
            self.port_owner.get(port) == Some(&self.block),
            "a block may only use its own ports and mailboxes (port {port})"
        );
        port
    }
}

/// The messages an inbox delivers this step: see [`Io::receive`].
pub struct Receive<'a, M> {
    mailbox: Option<&'a Mailbox<M>>,
    budget: &'a Cell<usize>,
}

impl<M> Iterator for Receive<'_, M> {
    type Item = M;

    fn next(&mut self) -> Option<M> {
        let left = self.budget.get();
        if left == 0 {
            return None;
        }
        let message = self.mailbox?.take()?;
        self.budget.set(left - 1);
        Some(message)
    }
}
