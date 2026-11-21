//! [`Engine`]: the workstreams of a process and the mailbox links between
//! them.

use std::any::TypeId;

use crate::{
    AccessError, Block, Child, Delivery, Inbox, InterfaceId, InterfaceKind, MailReceiver, MailSender, Message,
    Outbox, PortHandle, Workstream, WorkstreamId,
    mail::receiver,
};

/// Engine failures. Paths in messages are display labels.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("no workstream {0:?} in this engine")]
    UnknownWorkstream(WorkstreamId),
    #[error("the handle's workstream isn't in this engine")]
    ForeignHandle,
    #[error("no workstream in this engine exposes {0:?}")]
    UnknownInterface(InterfaceId),
    #[error(transparent)]
    Access(#[from] AccessError),
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

/// The workstreams of a process, and the mailbox links between them.
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
/// let audio = engine.take(audio).unwrap();                // move to its thread
/// ```
///
/// Handles know their workstream, so connecting needs nothing else. Data
/// (patch files, the editor) uses [`InterfaceId`]s instead
/// (`connect_mail_ids`). Only mailboxes connect workstreams: the engine
/// has no way to link `In`/`Out` across them (decision 5).
#[derive(Default)]
pub struct Engine {
    /// (label, workstream)
    workstreams: Vec<(String, Workstream)>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a built workstream. `label` is for display only.
    pub fn add(&mut self, label: &str, workstream: Workstream) -> WorkstreamId {
        let id = WorkstreamId(workstream.tree());
        self.workstreams.push((label.to_owned(), workstream));
        id
    }

    /// Link an outbox to an inbox, in the same workstream or different
    /// ones. Both must be exposed by their roots.
    pub fn connect_mail<M: Message>(&mut self, from: Outbox<M>, to: Inbox<M>) -> Result<(), EngineError> {
        let (from_ws, from_port) = self.locate(&from)?;
        let (to_ws, to_port) = self.locate(&to)?;
        self.connect_ports(from_ws, from_port, to_ws, to_port)
    }

    /// Like [`connect_mail`](Self::connect_mail), by ID: for wiring that
    /// comes from data. Kinds and types are checked here.
    pub fn connect_mail_ids(&mut self, from: InterfaceId, to: InterfaceId) -> Result<(), EngineError> {
        let (from_ws, from_port) = self.find_id(from)?;
        let (to_ws, to_port) = self.find_id(to)?;
        let source = &self.workstreams[from_ws].1;
        let target = &self.workstreams[to_ws].1;
        if source.kind(from_port) != InterfaceKind::Outbox {
            return Err(EngineError::NotAnOutbox(source.path_of(from_port).to_owned()));
        }
        if target.kind(to_port) != InterfaceKind::Inbox {
            return Err(EngineError::NotAnInbox(target.path_of(to_port).to_owned()));
        }
        let ((from_id, from_type), (to_id, to_type)) = (source.type_of(from_port), target.type_of(to_port));
        if from_id != to_id {
            return Err(EngineError::TypeMismatch {
                from: source.path_of(from_port).to_owned(),
                from_type,
                to: target.path_of(to_port).to_owned(),
                to_type,
            });
        }
        self.connect_ports(from_ws, from_port, to_ws, to_port)
    }

    /// A handle for sending into an inbox from outside any block, e.g. from
    /// a MIDI callback thread.
    pub fn sender<M: Message>(&self, inbox: Inbox<M>) -> Result<MailSender<M>, EngineError> {
        let (ws, port) = self.locate(&inbox)?;
        let mailbox = self.workstreams[ws].1.typed_mailbox::<M>(port).expect("typed handle, inbox");
        Ok(MailSender { mailbox })
    }

    /// Like [`sender`](Self::sender), by ID.
    pub fn sender_id<M: Message>(&self, inbox: InterfaceId) -> Result<MailSender<M>, EngineError> {
        let (ws, port) = self.find_id(inbox)?;
        let workstream = &self.workstreams[ws].1;
        if workstream.kind(port) != InterfaceKind::Inbox {
            return Err(EngineError::NotAnInbox(workstream.path_of(port).to_owned()));
        }
        let mailbox = workstream.typed_mailbox::<M>(port).ok_or_else(|| wrong_type(workstream, port))?;
        Ok(MailSender { mailbox })
    }

    /// Receive what an outbox sends, outside any block (a test, the host's
    /// loop). `delivery` sets this receiver's own queue.
    pub fn listen<M: Message>(&mut self, outbox: Outbox<M>, delivery: Delivery) -> Result<MailReceiver<M>, EngineError> {
        let (ws, port) = self.locate(&outbox)?;
        self.listen_port(ws, port, delivery)
    }

    /// Like [`listen`](Self::listen), by ID.
    pub fn listen_id<M: Message>(&mut self, outbox: InterfaceId, delivery: Delivery) -> Result<MailReceiver<M>, EngineError> {
        let (ws, port) = self.find_id(outbox)?;
        let workstream = &self.workstreams[ws].1;
        if workstream.kind(port) != InterfaceKind::Outbox {
            return Err(EngineError::NotAnOutbox(workstream.path_of(port).to_owned()));
        }
        if workstream.type_of(port).0 != TypeId::of::<M>() {
            return Err(wrong_type(workstream, port));
        }
        self.listen_port(ws, port, delivery)
    }

    /// A workstream's root block, as its own type: where reaching handles
    /// from outside starts.
    pub fn root<R: Block>(&self, id: WorkstreamId) -> Result<&R, EngineError> {
        let workstream = self.workstream(id).ok_or(EngineError::UnknownWorkstream(id))?;
        workstream.root::<R>().ok_or(EngineError::Access(AccessError::ForeignHandle))
    }

    /// A block anywhere in the engine, as its own type.
    pub fn get<B: Block>(&self, child: Child<B>) -> Result<&B, EngineError> {
        let workstream = self.workstreams.iter().find(|(_, w)| w.tree() == child.tree).ok_or(EngineError::ForeignHandle)?;
        workstream.1.get(child).ok_or(EngineError::ForeignHandle)
    }

    pub fn workstream(&self, id: WorkstreamId) -> Option<&Workstream> {
        self.workstreams.iter().find(|(_, w)| w.tree() == id.0).map(|(_, w)| w)
    }

    pub fn workstream_mut(&mut self, id: WorkstreamId) -> Option<&mut Workstream> {
        self.workstreams.iter_mut().find(|(_, w)| w.tree() == id.0).map(|(_, w)| w)
    }

    /// Remove a workstream, typically to move it to the thread that steps
    /// it. Its mailbox links stay intact (they're shared queues).
    pub fn take(&mut self, id: WorkstreamId) -> Option<Workstream> {
        let index = self.workstreams.iter().position(|(_, w)| w.tree() == id.0)?;
        Some(self.workstreams.remove(index).1)
    }

    /// Step every workstream once, in the order they were added. For hosts
    /// that run everything on one thread (and for tests).
    pub fn step_all(&mut self) {
        for (_, workstream) in &mut self.workstreams {
            workstream.step();
        }
    }

    /// The workstreams, with their display labels.
    pub fn workstreams(&self) -> impl Iterator<Item = (WorkstreamId, &str)> {
        self.workstreams.iter().map(|(label, w)| (WorkstreamId(w.tree()), label.as_str()))
    }

    // ---- helpers -------------------------------------------------------------------

    /// A handle → (workstream index, port), checking it's exposed.
    fn locate(&self, handle: &impl PortHandle) -> Result<(usize, usize), EngineError> {
        let ws = self.workstreams.iter().position(|(_, w)| w.tree() == handle.port_tree()).ok_or(EngineError::ForeignHandle)?;
        Ok((ws, self.workstreams[ws].1.handle(handle)?))
    }

    fn find_id(&self, id: InterfaceId) -> Result<(usize, usize), EngineError> {
        self.workstreams
            .iter()
            .enumerate()
            .find_map(|(i, (_, w))| w.by_id(id).ok().map(|port| (i, port)))
            .ok_or(EngineError::UnknownInterface(id))
    }

    fn connect_ports(&mut self, from_ws: usize, from_port: usize, to_ws: usize, to_port: usize) -> Result<(), EngineError> {
        self.check_realtime(from_ws, from_port)?;
        let mailbox = self.workstreams[to_ws].1.mailbox_of(to_port).expect("inboxes have mailboxes");
        let added = self.workstreams[from_ws].1.add_target(from_port, mailbox);
        debug_assert!(added, "types were checked");
        Ok(())
    }

    fn listen_port<M: Message>(&mut self, ws: usize, port: usize, delivery: Delivery) -> Result<MailReceiver<M>, EngineError> {
        self.check_realtime(ws, port)?;
        let (receiver, mailbox) = receiver::<M>(delivery);
        self.workstreams[ws].1.add_target(port, mailbox);
        Ok(receiver)
    }

    fn check_realtime(&self, ws: usize, port: usize) -> Result<(), EngineError> {
        let workstream = &self.workstreams[ws].1;
        if workstream.is_realtime() && workstream.drops(port) {
            return Err(EngineError::NotRealtimeSafe {
                path: workstream.path_of(port).to_owned(),
                type_name: workstream.type_of(port).1,
            });
        }
        Ok(())
    }
}

fn wrong_type(workstream: &Workstream, port: usize) -> EngineError {
    EngineError::Access(AccessError::WrongType {
        path: workstream.path_of(port).to_owned(),
        actual: workstream.type_of(port).1,
    })
}
