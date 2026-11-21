//! [`Workstream`]: runs one block tree, step after step, and swaps in
//! rebuilt graphs between steps.

use std::{
    any::Any,
    cell::{Cell, Ref, RefCell, RefMut},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use crate::{
    Block, BlockId, BuildError, Child, In, Inbox, Interface, InterfaceId, Message, Out, Outbox, PortHandle,
    control::{Command, Control, Garbage, RunnerQueues},
    graph::{Graph, TargetTable},
    mail::{AnyMailbox, Mailbox},
};

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

/// Runs one block tree. Single-threaded: [`step`](Self::step) runs every
/// block once, in dependency order.
///
/// Port values live here, one storage slot per output (and per input that
/// keeps its own value), each in a `RefCell`. That's what lets
/// [`Io::read`] and [`Io::write`] hand out references to different slots at
/// once. `RefCell` checks borrows with a plain counter: no locks, no
/// allocation, fine on the audio thread.
///
/// **Changing it while it runs** (step 3): [`rebuild`](Self::rebuild) (or
/// `Engine::rebuild` once an engine controls it) builds the new tree
/// elsewhere and hands it over; the next `step` swaps it in by moving
/// pointers. Blocks that still exist with the same type and interfaces keep
/// running, with their state; knob values, output values and inbox
/// mailboxes carry over; handles stay valid.
///
/// A workstream is `Send`: build it anywhere, then move it to the thread
/// that steps it.
pub struct Workstream {
    graph: Box<Graph>,
    targets: Box<TargetTable>,
    queues: RunnerQueues,
    /// The control side, until an [`Engine`](crate::Engine) takes it over.
    control: Option<Control>,
    steps: u64,
    disabled: Vec<u32>,
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
    /// engine connects one of its outboxes. Rebuild it through an
    /// [`Engine`](crate::Engine), so old graphs are freed on the control
    /// thread.
    pub fn build_realtime(root: impl Block) -> Result<Self, BuildError> {
        Self::build_with(root, BuildOptions { realtime: true, ..Default::default() })
    }

    pub fn build_with(root: impl Block, options: BuildOptions) -> Result<Self, BuildError> {
        let id = options.id.unwrap_or_else(BlockId::random);
        let (control, graph, targets, queues) = Control::new(Box::new(root), id, options.realtime)?;
        Ok(Self { graph, targets, queues, control: Some(control), steps: 0, disabled: Vec::new() })
    }

    /// Replace the tree with one built from `root`, keeping what can be
    /// kept (see the type's docs). Takes effect immediately. Only for
    /// workstreams not controlled by an engine; otherwise use
    /// `Engine::rebuild`.
    pub fn rebuild(&mut self, root: impl Block) -> Result<(), BuildError> {
        let control = self.control.as_mut().ok_or(BuildError::Controlled)?;
        control.rebuild(Box::new(root))?;
        self.apply_commands();
        self.collect_garbage();
        Ok(())
    }

    /// Run every block once, in dependency order. First applies pending
    /// changes (rebuilds, knob settings, new mailbox links). Inboxes then
    /// deliver what had arrived when the step began; later messages wait for
    /// the next step. A block that panics is switched off (see
    /// [`disabled`](Self::disabled)); the rest carry on.
    pub fn step(&mut self) {
        self.step_with(&());
    }

    /// Like [`step`](Self::step), handing every block `context`: whatever
    /// the host knows about this step (for audio: sample rate, frame count,
    /// the step's time window). Blocks read it with [`Io::context`]. Keeps
    /// the platform domain-agnostic: the workstream never looks inside.
    pub fn step_with(&mut self, context: &dyn Any) {
        self.apply_commands();

        let graph = &*self.graph;
        for &inbox in &graph.inboxes {
            let inbox = inbox as usize;
            let (Some(fns), Some(mailbox)) = (graph.layout.port(inbox).mail, &graph.mailbox[inbox]) else { continue };
            graph.budget[inbox].set((fns.pending)(&**mailbox));
        }

        let graph = &mut *self.graph;
        for &block in &graph.order {
            let block = block as usize;
            for merge in &graph.merges[block] {
                let mut into = graph.slots[merge.into].borrow_mut();
                (merge.clear)(&mut **into);
                for &source in &merge.sources {
                    (merge.merge)(&mut **into, &**graph.slots[source].borrow(), merge.mode);
                }
            }

            let Some(routine) = graph.blocks[block].as_mut() else { continue };
            let io = Io {
                block: block as u32,
                slots: &graph.slots,
                port_slot: &graph.port_slot,
                port_owner: &graph.port_owner,
                mailbox: &graph.mailbox,
                targets: &self.targets.0,
                budget: &graph.budget,
                step: self.steps,
                context,
            };
            if catch_unwind(AssertUnwindSafe(|| routine.process(&io))).is_err() {
                graph.blocks[block] = None;
                self.disabled.push(block as u32);
            }
        }
        self.steps += 1;

        // Free swapped-out graphs here, unless this thread is real-time
        // (an engine frees those on its own thread).
        if !self.graph.layout.realtime {
            self.collect_garbage();
        }
    }

    /// Apply what the control side sent, while there's room to send back
    /// what it replaces. Moves pointers only: no allocation, no freeing.
    fn apply_commands(&mut self) {
        while self.queues.garbage.slots() > 0 {
            let Ok(command) = self.queues.commands.pop() else { break };
            let garbage = match command {
                Command::Swap(mut swap) => {
                    // Fresh instances take over the running ones' state
                    // (`#[state]` fields, then `Block::carry_over`). A
                    // switched-off (crashed) block has none: it starts fresh.
                    for &block in &swap.carry_blocks {
                        let block = block as usize;
                        let running = self.graph.blocks.get_mut(block).and_then(Option::as_mut);
                        let fresh = swap.graph.blocks.get_mut(block).and_then(Option::as_mut);
                        if let (Some(running), Some(fresh)) = (running, fresh) {
                            let previous: &mut dyn Any = &mut **running;
                            fresh.carry_state(previous);
                            fresh.carry_over(previous);
                        }
                    }
                    for &(old, new) in &swap.keep_slots {
                        let (old, new) = (&self.graph.slots[old as usize], &swap.graph.slots[new as usize]);
                        std::mem::swap(&mut *old.borrow_mut(), &mut *new.borrow_mut());
                    }
                    std::mem::swap(&mut self.graph, &mut swap.graph);
                    std::mem::swap(&mut self.targets, &mut swap.targets);
                    self.disabled.clear();
                    Garbage::Swap(swap)
                }
                Command::Targets(mut targets) => {
                    std::mem::swap(&mut self.targets, &mut targets);
                    Garbage::Targets(targets)
                }
                Command::Set { slot, mut value } => {
                    if let Some(slot) = self.graph.slots.get(slot as usize) {
                        std::mem::swap(&mut *slot.borrow_mut(), &mut value);
                    }
                    Garbage::Value(value)
                }
            };
            // Can't fail: we checked for room.
            let _ = self.queues.garbage.push(garbage);
        }
    }

    /// Free what was swapped out (when not controlled by an engine).
    pub fn collect_garbage(&mut self) {
        if let Some(control) = &mut self.control {
            control.collect_garbage();
        }
    }

    /// Steps run so far.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Whether this workstream was built with [`build_realtime`](Self::build_realtime).
    pub fn is_realtime(&self) -> bool {
        self.graph.layout.realtime
    }

    /// The root block's identity.
    pub fn id(&self) -> BlockId {
        self.graph.layout.root_id
    }

    /// The root block's dictionary: everything it exposes, with IDs.
    pub fn interfaces(&self) -> &[Interface] {
        &self.graph.layout.interfaces
    }

    /// The root block, as its own type: the starting point for reaching
    /// handles from outside (`ws.get(ws.root::<Patch>()?.voice)`).
    pub fn root<R: Block>(&self) -> Option<&R> {
        let root = *self.graph.layout.block_index.get(&self.graph.layout.root_id)?;
        self.graph.blocks.get(root as usize)?.as_deref().and_then(|b| (b as &dyn Any).downcast_ref::<R>())
    }

    /// A block of this tree, as its own type, to read its handles. `None`
    /// for a handle of another tree or a block that was switched off.
    pub fn get<B: Block>(&self, child: Child<B>) -> Option<&B> {
        if child.tree != self.graph.layout.tree {
            return None;
        }
        self.graph.blocks.get(child.index as usize)?.as_deref().and_then(|b| (b as &dyn Any).downcast_ref::<B>())
    }

    /// The current value of a port (an output, or what an input sees). It
    /// must be exposed by the root.
    pub fn read<T: 'static>(&self, port: impl ValuePort<T>) -> Result<Ref<'_, T>, AccessError> {
        let port = self.graph.layout.typed::<T>(&port)?;
        Ok(self.value(port))
    }

    /// Like [`read`](Self::read), but never allocates, even when it fails
    /// (`None`: foreign, removed, unexposed, or of another type). For hosts
    /// reading results on a real-time thread, e.g. the audio output.
    pub fn peek<T: 'static>(&self, port: impl ValuePort<T>) -> Option<Ref<'_, T>> {
        let layout = &self.graph.layout;
        let index = port.port_index() as usize;
        let info = layout.ports.get(index)?.as_ref()?;
        let ok = port.port_tree() == layout.tree && layout.exposed[index] && info.type_id == std::any::TypeId::of::<T>();
        let slot = self.graph.slots.get(*self.graph.port_slot.get(index)? as usize).filter(|_| ok)?;
        Some(Ref::map(slot.borrow(), |v| (**v).downcast_ref::<T>().expect("type checked")))
    }

    /// Like [`read`](Self::read), by ID (for tools working from data).
    pub fn read_id<T: 'static>(&self, id: InterfaceId) -> Result<Ref<'_, T>, AccessError> {
        let layout = &self.graph.layout;
        let port = layout.by_id(id)?;
        if layout.port(port).kind.is_mailbox() {
            return Err(AccessError::NotAPort(layout.port(port).path.clone()));
        }
        layout.check_type::<T>(port)?;
        Ok(self.value(port))
    }

    /// Turn a knob: set the value of an unconnected input (exposed by the
    /// root). Call between steps, on the thread that steps it (from
    /// elsewhere, use `Engine::set`).
    pub fn set<T: Send + 'static>(&mut self, input: In<T>, value: T) -> Result<(), AccessError> {
        let port = self.graph.layout.typed::<T>(&input)?;
        self.set_port(port, value)
    }

    /// Like [`set`](Self::set), by ID.
    pub fn set_id<T: Send + 'static>(&mut self, id: InterfaceId, value: T) -> Result<(), AccessError> {
        let port = self.graph.layout.by_id(id)?;
        self.graph.layout.check_type::<T>(port)?;
        self.set_port(port, value)
    }

    /// Messages an inbox (exposed by the root) lost to a full queue.
    pub fn dropped<M>(&self, inbox: Inbox<M>) -> Result<u64, AccessError> {
        let layout = &self.graph.layout;
        let port = layout.handle(&inbox)?;
        match (layout.port(port).mail, &self.graph.mailbox[port]) {
            (Some(fns), Some(mailbox)) => Ok((fns.dropped)(&**mailbox)),
            _ => Err(AccessError::NotAnInbox(layout.port(port).path.clone())),
        }
    }

    /// Display paths of blocks switched off because they panicked (since
    /// the last rebuild, which gives them a fresh start).
    pub fn disabled(&self) -> impl Iterator<Item = &str> {
        let layout = &self.graph.layout;
        self.disabled.iter().filter_map(|&b| Some(layout.blocks.get(b as usize)?.as_ref()?.path.as_str()))
    }

    fn value<T: 'static>(&self, port: usize) -> Ref<'_, T> {
        let slot = &self.graph.slots[self.graph.port_slot[port] as usize];
        Ref::map(slot.borrow(), |v| (**v).downcast_ref::<T>().expect("type checked"))
    }

    fn set_port<T: Send + 'static>(&mut self, port: usize, value: T) -> Result<(), AccessError> {
        let slot = self.graph.layout.knob_slot(port)?;
        *self.graph.slots[slot].borrow_mut() = Box::new(value);
        Ok(())
    }

    // ---- for the engine ------------------------------------------------------------

    /// Hand the control side over (to an engine).
    pub(crate) fn take_control(&mut self) -> Option<Control> {
        self.control.take()
    }
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
    #[error("the handle's interface no longer exists (removed by a rebuild)")]
    Gone,
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
    #[error("the workstream isn't keeping up with changes (is it being stepped?)")]
    Busy,
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
    context: &'a dyn Any,
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
    pub fn send<M: Message>(&self, outbox: Outbox<M>, message: M) {
        let port = self.own(outbox.index);
        let targets = self.targets.get(port).and_then(Option::as_deref);
        let Some(targets) = targets.and_then(|t| t.downcast_ref::<Vec<Arc<Mailbox<M>>>>()) else {
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

    /// What the host handed to [`Workstream::step_with`] for this step, if
    /// it's a `C`. Audio blocks read an `AudioContext` here.
    pub fn context<C: 'static>(&self) -> Option<&C> {
        self.context.downcast_ref::<C>()
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
