//! [`Engine`]: the workstreams of a process, the mailbox links between
//! them, and changes to them while they run.

use std::any::TypeId;

use crate::{
    AccessError, Block, BuildError, Child, Delivery, In, Inbox, InterfaceId, InterfaceKind, MailReceiver,
    MailSender, Message, Outbox, PortHandle, Workstream, WorkstreamId,
    control::Control,
    graph::Layout,
    mail::{Mailbox, receiver},
};

/// Engine failures. Paths in messages are display labels.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("no workstream {0:?} in this engine")]
    UnknownWorkstream(WorkstreamId),
    #[error("the handle's workstream isn't in this engine")]
    ForeignHandle,
    #[error("no workstream in this engine exposes {0:?}")]
    UnknownInterface(InterfaceId),
    #[error("the workstream has been taken out to be stepped elsewhere; get handles before `take`")]
    Taken,
    #[error(transparent)]
    Access(#[from] AccessError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("{0:?} must be an outbox")]
    NotAnOutbox(String),
    #[error("{0:?} must be an inbox")]
    NotAnInbox(String),
    #[error("{from:?} sends {from_type}, but {to:?} receives {to_type}")]
    TypeMismatch { from: String, from_type: &'static str, to: String, to_type: &'static str },
    #[error(
        "{path:?} sends {type_name} from a real-time workstream; dropping it may free memory \
         (use a message type that owns no memory)"
    )]
    NotRealtimeSafe { path: String, type_name: &'static str },
}

struct Entry {
    label: String,
    /// The control side: always here.
    control: Control,
    /// The stepping side, until it's taken to its own thread.
    runner: Option<Workstream>,
}

/// The workstreams of a process, the mailbox links between them, and
/// changes to them while they run.
///
/// ```ignore
/// let mut engine = Engine::new();
/// let audio = engine.add("audio", Workstream::build_realtime(AudioRoot::default())?);
/// let render = engine.add("render", Workstream::build(RenderRoot::default())?);
///
/// let meter = engine.get(engine.root::<AudioRoot>(audio)?.meter)?;
/// let visuals = engine.get(engine.root::<RenderRoot>(render)?.visuals)?;
/// engine.connect_mail(meter.level, visuals.level)?;      // typed handles
/// let midi = engine.sender(meter.notes)?;                 // for the MIDI thread
/// let audio_runner = engine.take(audio).unwrap();         // move to its thread
///
/// // Later, while it runs:
/// engine.rebuild(audio, AudioRoot::with_more_voices())?;  // swapped in at its next step
/// engine.set(meter.gain, 0.5)?;                           // a knob, from here
/// engine.collect_garbage();                               // free what it swapped out
/// ```
///
/// The engine keeps each workstream's **control side** even after the
/// stepping side is taken to another thread: rebuilding, connecting
/// mailboxes and setting knobs all work at any time. Changes travel through
/// a lock-free queue and apply at the start of the workstream's next step;
/// what they replace comes back to be freed by
/// [`collect_garbage`](Self::collect_garbage) (called by every method).
///
/// Handles know their workstream, so connecting needs nothing else, and they
/// stay valid across rebuilds. Data (patch files, the editor) uses
/// [`InterfaceId`]s instead (`*_ids` methods). Only mailboxes connect
/// workstreams (decision 5).
#[derive(Default)]
pub struct Engine {
    entries: Vec<Entry>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a built workstream (the engine takes over its control side).
    /// `label` is for display only.
    pub fn add(&mut self, label: &str, mut workstream: Workstream) -> WorkstreamId {
        let control = workstream.take_control().expect("a fresh workstream has its control side");
        let id = WorkstreamId(control.layout.tree);
        self.entries.push(Entry { label: label.to_owned(), control, runner: Some(workstream) });
        id
    }

    /// Replace a workstream's tree with one built from `root`, while it runs.
    /// Built here; swapped in at the workstream's next step. Blocks that
    /// still exist with the same type and interfaces keep running with their
    /// state; knobs, outputs, inbox mailboxes and engine-made links carry
    /// over; handles stay valid.
    pub fn rebuild(&mut self, id: WorkstreamId, root: impl Block) -> Result<(), EngineError> {
        self.collect_garbage();
        let entry = self.entry_mut(id)?;
        entry.control.rebuild(Box::new(root))?;
        Ok(())
    }

    /// Set a knob (an unconnected input), from here, while the workstream
    /// runs anywhere. Applies at its next step.
    pub fn set<T: Send + 'static>(&mut self, input: In<T>, value: T) -> Result<(), EngineError> {
        self.collect_garbage();
        let (index, port) = self.locate(&input)?;
        let control = &mut self.entries[index].control;
        control.layout.check_type::<T>(port)?;
        let slot = control.layout.knob_slot(port)?;
        control.set(slot, Box::new(value))?;
        Ok(())
    }

    /// Like [`set`](Self::set), by ID: for tools working from data (the
    /// patch editor).
    pub fn set_id<T: Send + 'static>(&mut self, input: InterfaceId, value: T) -> Result<(), EngineError> {
        self.collect_garbage();
        let (index, port) = self.find_id(input)?;
        let control = &mut self.entries[index].control;
        control.layout.check_type::<T>(port)?;
        let slot = control.layout.knob_slot(port)?;
        control.set(slot, Box::new(value))?;
        Ok(())
    }

    /// Link an outbox to an inbox, in the same workstream or different
    /// ones. Both must be exposed by their roots.
    pub fn connect_mail<M: Message>(&mut self, from: Outbox<M>, to: Inbox<M>) -> Result<(), EngineError> {
        self.collect_garbage();
        let (from_ws, from_port) = self.locate(&from)?;
        let (to_ws, to_port) = self.locate(&to)?;
        self.connect_ports(from_ws, from_port, to_ws, to_port)
    }

    /// Like [`connect_mail`](Self::connect_mail), by ID: for wiring that
    /// comes from data. Kinds and types are checked here.
    pub fn connect_mail_ids(&mut self, from: InterfaceId, to: InterfaceId) -> Result<(), EngineError> {
        self.collect_garbage();
        let (from_ws, from_port) = self.find_id(from)?;
        let (to_ws, to_port) = self.find_id(to)?;
        let (source, target) = (self.layout(from_ws), self.layout(to_ws));
        let (out, inb) = (source.port(from_port), target.port(to_port));
        if out.kind != InterfaceKind::Outbox {
            return Err(EngineError::NotAnOutbox(out.path.clone()));
        }
        if inb.kind != InterfaceKind::Inbox {
            return Err(EngineError::NotAnInbox(inb.path.clone()));
        }
        if out.type_id != inb.type_id {
            return Err(EngineError::TypeMismatch {
                from: out.path.clone(),
                from_type: out.type_name,
                to: inb.path.clone(),
                to_type: inb.type_name,
            });
        }
        self.connect_ports(from_ws, from_port, to_ws, to_port)
    }

    /// A handle for sending into an inbox from outside any block, e.g. from
    /// a MIDI callback thread.
    pub fn sender<M: Message>(&self, inbox: Inbox<M>) -> Result<MailSender<M>, EngineError> {
        let (ws, port) = self.locate(&inbox)?;
        self.sender_port(ws, port)
    }

    /// Like [`sender`](Self::sender), by ID.
    pub fn sender_id<M: Message>(&self, inbox: InterfaceId) -> Result<MailSender<M>, EngineError> {
        let (ws, port) = self.find_id(inbox)?;
        let info = self.layout(ws).port(port);
        if info.kind != InterfaceKind::Inbox {
            return Err(EngineError::NotAnInbox(info.path.clone()));
        }
        self.sender_port(ws, port)
    }

    /// Receive what an outbox sends, outside any block (a test, the host's
    /// loop). `delivery` sets this receiver's own queue.
    pub fn listen<M: Message>(&mut self, outbox: Outbox<M>, delivery: Delivery) -> Result<MailReceiver<M>, EngineError> {
        self.collect_garbage();
        let (ws, port) = self.locate(&outbox)?;
        self.listen_port(ws, port, delivery)
    }

    /// Like [`listen`](Self::listen), by ID.
    pub fn listen_id<M: Message>(&mut self, outbox: InterfaceId, delivery: Delivery) -> Result<MailReceiver<M>, EngineError> {
        self.collect_garbage();
        let (ws, port) = self.find_id(outbox)?;
        let info = self.layout(ws).port(port);
        if info.kind != InterfaceKind::Outbox {
            return Err(EngineError::NotAnOutbox(info.path.clone()));
        }
        if info.type_id != TypeId::of::<M>() {
            return Err(wrong_type(self.layout(ws), port));
        }
        self.listen_port(ws, port, delivery)
    }

    /// A workstream's root block, as its own type: where reaching handles
    /// from outside starts. Only while the engine still holds it (get
    /// handles before [`take`](Self::take); they stay valid).
    pub fn root<R: Block>(&self, id: WorkstreamId) -> Result<&R, EngineError> {
        let runner = self.entry(id)?.runner.as_ref().ok_or(EngineError::Taken)?;
        runner.root::<R>().ok_or(EngineError::Access(AccessError::ForeignHandle))
    }

    /// A block anywhere in the engine, as its own type (while the engine
    /// holds its workstream).
    pub fn get<B: Block>(&self, child: Child<B>) -> Result<&B, EngineError> {
        let entry = self.entries.iter().find(|e| e.control.layout.tree == child.tree).ok_or(EngineError::ForeignHandle)?;
        let runner = entry.runner.as_ref().ok_or(EngineError::Taken)?;
        runner.get(child).ok_or(EngineError::ForeignHandle)
    }

    /// The stepping side, while the engine holds it.
    pub fn workstream(&self, id: WorkstreamId) -> Option<&Workstream> {
        self.entry(id).ok()?.runner.as_ref()
    }

    pub fn workstream_mut(&mut self, id: WorkstreamId) -> Option<&mut Workstream> {
        self.entry_mut(id).ok()?.runner.as_mut()
    }

    /// Take the stepping side out, to move it to the thread that steps it.
    /// The engine keeps the control side: rebuilds, knob settings and new
    /// links still reach it.
    pub fn take(&mut self, id: WorkstreamId) -> Option<Workstream> {
        self.entry_mut(id).ok()?.runner.take()
    }

    /// Step every workstream the engine still holds, in the order they were
    /// added (for hosts that run everything on one thread, and tests), then
    /// free what they swapped out.
    pub fn step_all(&mut self) {
        for entry in &mut self.entries {
            if let Some(runner) = &mut entry.runner {
                runner.step();
            }
        }
        self.collect_garbage();
    }

    /// Free what workstreams have swapped out. Every method calls this;
    /// call it regularly when the engine is otherwise idle (e.g. once per
    /// UI frame).
    pub fn collect_garbage(&mut self) {
        for entry in &mut self.entries {
            entry.control.collect_garbage();
        }
    }

    /// The workstreams, with their display labels.
    pub fn workstreams(&self) -> impl Iterator<Item = (WorkstreamId, &str)> {
        self.entries.iter().map(|e| (WorkstreamId(e.control.layout.tree), e.label.as_str()))
    }

    // ---- helpers -------------------------------------------------------------------

    fn entry(&self, id: WorkstreamId) -> Result<&Entry, EngineError> {
        self.entries.iter().find(|e| e.control.layout.tree == id.0).ok_or(EngineError::UnknownWorkstream(id))
    }

    fn entry_mut(&mut self, id: WorkstreamId) -> Result<&mut Entry, EngineError> {
        self.entries.iter_mut().find(|e| e.control.layout.tree == id.0).ok_or(EngineError::UnknownWorkstream(id))
    }

    fn layout(&self, index: usize) -> &Layout {
        &self.entries[index].control.layout
    }

    /// A handle → (entry index, port), checking it's exposed.
    fn locate(&self, handle: &impl PortHandle) -> Result<(usize, usize), EngineError> {
        let index = self
            .entries
            .iter()
            .position(|e| e.control.layout.tree == handle.port_tree())
            .ok_or(EngineError::ForeignHandle)?;
        Ok((index, self.layout(index).handle(handle)?))
    }

    fn find_id(&self, id: InterfaceId) -> Result<(usize, usize), EngineError> {
        self.entries
            .iter()
            .enumerate()
            .find_map(|(i, e)| e.control.layout.by_id(id).ok().map(|port| (i, port)))
            .ok_or(EngineError::UnknownInterface(id))
    }

    fn connect_ports(&mut self, from_ws: usize, from_port: usize, to_ws: usize, to_port: usize) -> Result<(), EngineError> {
        self.check_realtime(from_ws, from_port)?;
        let mailbox = self.layout(to_ws).port(to_port).mailbox.clone().expect("inboxes have mailboxes");
        self.entries[from_ws].control.add_target(from_port, mailbox)?;
        Ok(())
    }

    fn sender_port<M: Message>(&self, ws: usize, port: usize) -> Result<MailSender<M>, EngineError> {
        let layout = self.layout(ws);
        let mailbox = layout.port(port).mailbox.clone().and_then(|m| m.downcast::<Mailbox<M>>().ok());
        let mailbox = mailbox.ok_or_else(|| wrong_type(layout, port))?;
        Ok(MailSender { mailbox })
    }

    fn listen_port<M: Message>(&mut self, ws: usize, port: usize, delivery: Delivery) -> Result<MailReceiver<M>, EngineError> {
        self.check_realtime(ws, port)?;
        let (receiver, mailbox) = receiver::<M>(delivery);
        self.entries[ws].control.add_target(port, mailbox)?;
        Ok(receiver)
    }

    fn check_realtime(&self, ws: usize, port: usize) -> Result<(), EngineError> {
        let layout = self.layout(ws);
        let info = layout.port(port);
        if layout.realtime && info.mail.is_some_and(|f| f.drops) {
            return Err(EngineError::NotRealtimeSafe { path: info.path.clone(), type_name: info.type_name });
        }
        Ok(())
    }
}

fn wrong_type(layout: &Layout, port: usize) -> EngineError {
    let info = layout.port(port);
    EngineError::Access(AccessError::WrongType { path: info.path.clone(), actual: info.type_name })
}
