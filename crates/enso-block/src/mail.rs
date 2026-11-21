//! Mailboxes: asynchronous, never-blocking messages between blocks, within
//! a workstream or across workstreams (threads).
//!
//! Every [`Inbox`](crate::Inbox) owns one [`Mailbox`]: a bounded lock-free
//! queue (`crossbeam_queue::ArrayQueue`). Senders use `force_push`: when the
//! queue is full, the **oldest** message is pushed out (decision 5) and
//! freed by the sender. `Latest` delivery is the same queue with room for
//! one message, so a new value always replaces the unread one.

use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
};

use crossbeam_queue::ArrayQueue;

/// What can travel through a mailbox: cloneable (one outbox may feed
/// several inboxes) and sendable to other threads. Implemented for every
/// such type.
pub trait Message: Clone + Send + 'static {}
impl<T: Clone + Send + 'static> Message for T {}

/// Messages a **real-time** workstream may send (decision 13): dropping
/// them runs no code, so an evicted message never frees memory on a
/// real-time thread. Implemented for every `Copy` message.
///
/// The engine checks the underlying property, `!std::mem::needs_drop::<M>()`,
/// when it connects a real-time workstream's outbox. That also admits
/// non-`Copy` types that own no memory.
pub trait RealtimeMessage: Message + Copy {}
impl<T: Message + Copy> RealtimeMessage for T {}

/// How an inbox's mailbox delivers. Chosen by the inbox's owner at
/// registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Delivery {
    /// Every message, in arrival order. Holds up to this many; when full,
    /// the oldest is dropped (and counted).
    Queue(usize),
    /// Only the newest message (e.g. a level meter, a world snapshot).
    Latest,
}

impl Default for Delivery {
    fn default() -> Self {
        Delivery::Queue(64)
    }
}

/// One inbox's queue, shared (`Arc`) by everything that sends to it.
pub(crate) struct Mailbox<M> {
    queue: ArrayQueue<M>,
    latest: bool,
    dropped: AtomicU64,
}

impl<M> Mailbox<M> {
    fn new(delivery: Delivery) -> Self {
        let (capacity, latest) = match delivery {
            Delivery::Queue(capacity) => (capacity.max(1), false),
            Delivery::Latest => (1, true),
        };
        Self { queue: ArrayQueue::new(capacity), latest, dropped: AtomicU64::new(0) }
    }

    /// Never blocks; a full queue pushes its oldest message out.
    pub(crate) fn post(&self, message: M) {
        if let Some(evicted) = self.queue.force_push(message) {
            if !self.latest {
                self.dropped.fetch_add(1, Relaxed);
            }
            drop(evicted); // freed here, on the sender's thread
        }
    }

    pub(crate) fn take(&self) -> Option<M> {
        self.queue.pop()
    }

    pub(crate) fn pending(&self) -> usize {
        self.queue.len()
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Relaxed)
    }
}

/// A type-erased mailbox, as the workstream stores it.
pub(crate) type AnyMailbox = Arc<dyn Any + Send + Sync>;

/// Type-specific operations, captured when a mailbox interface is
/// registered (generic code, stored as plain function pointers).
#[derive(Clone, Copy)]
pub(crate) struct MailFns {
    pub make_mailbox: fn(Delivery) -> AnyMailbox,
    pub pending: fn(&(dyn Any + Send + Sync)) -> usize,
    pub dropped: fn(&(dyn Any + Send + Sync)) -> u64,
    /// An outbox's (empty) list of target mailboxes.
    pub make_targets: fn() -> Box<dyn Any + Send + Sync>,
    pub add_target: fn(&mut (dyn Any + Send + Sync), AnyMailbox) -> bool,
    /// `std::mem::needs_drop::<M>()`: whether evicting a message runs code.
    pub drops: bool,
}

pub(crate) fn mail_fns<M: Message>() -> MailFns {
    MailFns {
        make_mailbox: |delivery| Arc::new(Mailbox::<M>::new(delivery)),
        pending: |mailbox| mailbox.downcast_ref::<Mailbox<M>>().map_or(0, Mailbox::pending),
        dropped: |mailbox| mailbox.downcast_ref::<Mailbox<M>>().map_or(0, Mailbox::dropped),
        make_targets: || Box::new(Vec::<Arc<Mailbox<M>>>::new()),
        add_target: |targets, mailbox| {
            match (targets.downcast_mut::<Vec<Arc<Mailbox<M>>>>(), mailbox.downcast::<Mailbox<M>>()) {
                (Some(targets), Ok(mailbox)) => {
                    targets.push(mailbox);
                    true
                }
                _ => false,
            }
        },
        drops: std::mem::needs_drop::<M>(),
    }
}

/// Sends into a workstream's inbox from outside any block, e.g. from a
/// MIDI callback thread. Clone it freely; it never blocks.
pub struct MailSender<M> {
    pub(crate) mailbox: Arc<Mailbox<M>>,
}

impl<M> Clone for MailSender<M> {
    fn clone(&self) -> Self {
        Self { mailbox: self.mailbox.clone() }
    }
}

impl<M> MailSender<M> {
    pub fn send(&self, message: M) {
        self.mailbox.post(message);
    }

    /// Messages lost because the inbox was full.
    pub fn dropped(&self) -> u64 {
        self.mailbox.dropped()
    }
}

/// Receives what a workstream's outbox sends, outside any block (a test,
/// the host's main loop).
pub struct MailReceiver<M> {
    pub(crate) mailbox: Arc<Mailbox<M>>,
}

impl<M> MailReceiver<M> {
    /// The oldest waiting message, if any. Never blocks.
    pub fn try_recv(&self) -> Option<M> {
        self.mailbox.take()
    }

    /// Everything waiting right now.
    pub fn drain(&self) -> impl Iterator<Item = M> + '_ {
        std::iter::from_fn(|| self.mailbox.take())
    }

    /// Messages lost because this receiver's queue was full.
    pub fn dropped(&self) -> u64 {
        self.mailbox.dropped()
    }
}

/// A receiver created by the engine for an outbox.
pub(crate) fn receiver<M: Message>(delivery: Delivery) -> (MailReceiver<M>, AnyMailbox) {
    let mailbox = Arc::new(Mailbox::<M>::new(delivery));
    (MailReceiver { mailbox: mailbox.clone() }, mailbox)
}
