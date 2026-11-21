//! [`Engine`]: the workstreams of a process and the mailbox links between
//! them.

use crate::{
    AccessError, Delivery, InterfaceKind, MailReceiver, MailSender, Message, Workstream,
    mail::receiver,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("a workstream called {0:?} already exists")]
    DuplicateWorkstream(String),
    #[error("no workstream called {0:?}")]
    UnknownWorkstream(String),
    #[error("{0:?} isn't a mailbox path (expected \"workstream:path\")")]
    BadPath(String),
    #[error("{path:?}: {source}")]
    Access { path: String, source: AccessError },
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

/// The workstreams of a process, by name, and the mailbox links between
/// them.
///
/// ```ignore
/// let mut engine = Engine::new();
/// engine.add("audio", Workstream::build_realtime(audio_root)?)?;
/// engine.add("render", Workstream::build(render_root)?)?;
/// engine.connect_mail("audio:meter/level", "render:visuals/level")?;
/// let midi_in = engine.sender::<MidiEvent>("audio:midi")?;   // for the MIDI thread
/// let audio = engine.take("audio").unwrap();                  // move to its thread
/// ```
///
/// Only mailboxes connect workstreams: they have no ports in common, so an
/// `In`/`Out` link across workstreams can't even be expressed (decision 5).
///
/// Paths are `workstream:path`, where `path` is an interface of the
/// workstream's root block (`audio:instrument/level`; a leading `/` after
/// the colon is allowed).
#[derive(Default)]
pub struct Engine {
    workstreams: Vec<(String, Workstream)>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a built workstream under `name`.
    pub fn add(&mut self, name: &str, workstream: Workstream) -> Result<(), EngineError> {
        if self.index(name).is_ok() {
            return Err(EngineError::DuplicateWorkstream(name.to_owned()));
        }
        self.workstreams.push((name.to_owned(), workstream));
        Ok(())
    }

    /// Link the outbox at `from` to the inbox at `to` (e.g.
    /// `"audio:meter/level"` → `"render:visuals/level"`). The workstreams may
    /// be the same or different.
    pub fn connect_mail(&mut self, from: &str, to: &str) -> Result<(), EngineError> {
        let (from_ws, from_port) = self.resolve(from)?;
        let (to_ws, to_port) = self.resolve(to)?;
        let source = &self.workstreams[from_ws].1;
        let target = &self.workstreams[to_ws].1;
        if source.kind(from_port) != InterfaceKind::Outbox {
            return Err(EngineError::NotAnOutbox(from.to_owned()));
        }
        if target.kind(to_port) != InterfaceKind::Inbox {
            return Err(EngineError::NotAnInbox(to.to_owned()));
        }
        let ((from_id, from_type), (to_id, to_type)) = (source.type_of(from_port), target.type_of(to_port));
        if from_id != to_id {
            return Err(EngineError::TypeMismatch { from: from.into(), from_type, to: to.into(), to_type });
        }
        self.check_realtime(from_ws, from_port, from)?;
        let mailbox = target.mailbox_of(to_port).expect("inboxes have mailboxes");
        let added = self.workstreams[from_ws].1.add_target(from_port, mailbox);
        debug_assert!(added, "types were checked");
        Ok(())
    }

    /// A handle for sending into the inbox at `path` from outside any
    /// block, e.g. from a MIDI callback thread.
    pub fn sender<M: Message>(&self, path: &str) -> Result<MailSender<M>, EngineError> {
        let (ws, port) = self.resolve(path)?;
        let workstream = &self.workstreams[ws].1;
        if workstream.kind(port) != InterfaceKind::Inbox {
            return Err(EngineError::NotAnInbox(path.to_owned()));
        }
        let mailbox = workstream.typed_mailbox::<M>(port).ok_or_else(|| self.type_error(path, ws, port))?;
        Ok(MailSender { mailbox })
    }

    /// Receive what the outbox at `path` sends, outside any block (a test,
    /// the host's loop). `delivery` sets this receiver's own queue.
    pub fn listen<M: Message>(&mut self, path: &str, delivery: Delivery) -> Result<MailReceiver<M>, EngineError> {
        let (ws, port) = self.resolve(path)?;
        let workstream = &self.workstreams[ws].1;
        if workstream.kind(port) != InterfaceKind::Outbox {
            return Err(EngineError::NotAnOutbox(path.to_owned()));
        }
        if workstream.type_of(port).0 != std::any::TypeId::of::<M>() {
            return Err(self.type_error(path, ws, port));
        }
        self.check_realtime(ws, port, path)?;
        let (receiver, mailbox) = receiver::<M>(delivery);
        self.workstreams[ws].1.add_target(port, mailbox);
        Ok(receiver)
    }

    pub fn workstream(&self, name: &str) -> Option<&Workstream> {
        self.workstreams.iter().find(|(n, _)| n == name).map(|(_, w)| w)
    }

    pub fn workstream_mut(&mut self, name: &str) -> Option<&mut Workstream> {
        self.workstreams.iter_mut().find(|(n, _)| n == name).map(|(_, w)| w)
    }

    /// Remove a workstream, typically to move it to the thread that steps
    /// it. Its mailbox links stay intact (they're shared queues).
    pub fn take(&mut self, name: &str) -> Option<Workstream> {
        let index = self.index(name).ok()?;
        Some(self.workstreams.remove(index).1)
    }

    /// Step every workstream once, in the order they were added. For hosts
    /// that run everything on one thread (and for tests).
    pub fn step_all(&mut self) {
        for (_, workstream) in &mut self.workstreams {
            workstream.step();
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.workstreams.iter().map(|(n, _)| n.as_str())
    }

    fn index(&self, name: &str) -> Result<usize, EngineError> {
        self.workstreams
            .iter()
            .position(|(n, _)| n == name)
            .ok_or_else(|| EngineError::UnknownWorkstream(name.to_owned()))
    }

    /// `"ws:path"` → (workstream index, interface index).
    fn resolve(&self, full: &str) -> Result<(usize, usize), EngineError> {
        let (ws, path) = full.split_once(':').ok_or_else(|| EngineError::BadPath(full.to_owned()))?;
        let index = self.index(ws)?;
        let port = self.workstreams[index]
            .1
            .find(path)
            .map_err(|source| EngineError::Access { path: full.to_owned(), source })?;
        Ok((index, port))
    }

    fn check_realtime(&self, ws: usize, port: usize, path: &str) -> Result<(), EngineError> {
        let workstream = &self.workstreams[ws].1;
        if workstream.is_realtime() && workstream.drops(port) {
            return Err(EngineError::NotRealtimeSafe { path: path.to_owned(), type_name: workstream.type_of(port).1 });
        }
        Ok(())
    }

    fn type_error(&self, path: &str, ws: usize, port: usize) -> EngineError {
        EngineError::Access {
            path: path.to_owned(),
            source: AccessError::WrongType { path: path.to_owned(), actual: self.workstreams[ws].1.type_of(port).1 },
        }
    }
}
