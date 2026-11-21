//! Building a block tree: registration, loading, interface dictionaries,
//! linking. Everything here runs before the workstream steps (off the
//! stepping thread when rebuilding).
//!
//! Identity is by IDs (design §0): every block has a [`BlockId`], every
//! interface a [`Key`] within its block. Labels only build display paths.
//!
//! **Indices are stable across rebuilds:** when a tree is rebuilt, a block
//! or interface that existed before gets the same internal index as before
//! (looked up by its ID in the previous [`Layout`]); new ones get fresh
//! indices, and indices are never reused. That's what lets a running block
//! keep its handles (and its state) when the graph around it is swapped.

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    ops::Index,
};

use crate::{
    Block, BlockError, BlockId, Child, Delivery, In, Inbox, InterfaceId, Key, Merge, MergeMode, Message, Out,
    Outbox, PortHandle, Spec,
    graph::Layout,
    mail::{MailFns, mail_fns},
    merge::{ClearFn, MergeFn, clear_erased, merge_erased},
};

/// What an interface is: a port (same-step data flow) or a mailbox end
/// (messages, across steps and workstreams).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceKind {
    In,
    Out,
    Inbox,
    Outbox,
}

impl InterfaceKind {
    pub fn is_mailbox(self) -> bool {
        matches!(self, InterfaceKind::Inbox | InterfaceKind::Outbox)
    }
}

/// One entry of a block's public dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    /// What identifies it, in data and across runs.
    pub id: InterfaceId,
    /// For display only: labels from this block down, e.g. `filter/cutoff`.
    pub path: String,
    pub kind: InterfaceKind,
    /// The Rust type carried, for display (`f32`, `enso_block::Events<..>`).
    pub type_name: &'static str,
    pub merge: MergeMode,
    pub(crate) port: u32,
    pub(crate) type_id: TypeId,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LinkError {
    #[error("{0:?} isn't exposed by any child (it may be private, or the parent's own interface)")]
    NotVisible(String),
    #[error("no child exposes {0:?}")]
    UnknownInterface(InterfaceId),
    #[error("{from:?} carries {from_type}, but {to:?} expects {to_type}")]
    TypeMismatch { from: String, from_type: &'static str, to: String, to_type: &'static str },
    #[error("{0:?} → {1:?}: connect an output to an input, or an outbox to an inbox")]
    WrongDirection(String, String),
    #[error("{0:?} accepts only one link (its merge mode is Single)")]
    AlreadyLinked(String),
    #[error("a handle wasn't registered (missing assignment in `register`?)")]
    Unregistered,
    #[error("a handle belongs to a different block tree")]
    ForeignHandle,
}

/// Build failures. Paths in messages are display labels.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("{path:?}: {message}")]
    Register { path: String, message: String },
    #[error("{path:?}: failed to load: {source}")]
    Load { path: String, source: BlockError },
    #[error("{path:?}: {source}")]
    Link { path: String, source: LinkError },
    #[error("cycle: {} depend on each other (feedback needs a delay block)", .0.join(", "))]
    Cycle(Vec<String>),
    #[error(
        "{path:?} sends {type_name} from a real-time workstream; dropping it may free memory \
         (use a message type that owns no memory)"
    )]
    NotRealtimeSafe { path: String, type_name: &'static str },
    #[error("this workstream is controlled by an engine; rebuild it through the engine")]
    Controlled,
    #[error("the workstream isn't keeping up with changes (is it being stepped?)")]
    Busy,
}

// ---- build state -------------------------------------------------------------------

pub(crate) struct BlockEntry {
    pub block: Option<Box<dyn Block>>,
    pub id: BlockId,
    /// The block's Rust type (to decide whether a running instance can be
    /// kept across a rebuild).
    pub type_id: TypeId,
    pub label: String,
    /// Labels from the root, for display: "" for the root, "voice/osc" below.
    pub path: String,
    pub children: Vec<u32>,
    pub ports: Vec<u32>,
    /// Ports (own or descendants') this block keeps from floating up.
    pub hidden: Vec<u32>,
    /// (key, label, port)
    pub aliases: Vec<(Key, String, u32)>,
    pub dictionary: Vec<Interface>,
}

/// A registered interface: a port or a mailbox end. (Called a "port"
/// internally either way: they share handles, keys and visibility.)
pub(crate) struct PortEntry {
    pub owner: u32,
    pub key: Key,
    pub label: String,
    pub kind: InterfaceKind,
    pub type_id: TypeId,
    pub type_name: &'static str,
    pub merge: MergeMode,
    /// For merging inputs.
    pub clear: Option<ClearFn>,
    pub merge_fn: Option<MergeFn>,
    /// An output's initial value, or an input's default. Moved into the
    /// workstream's storage when compiled. `None` for mailboxes.
    pub value: Option<Box<dyn Any + Send>>,
    /// For mailbox ends.
    pub mail: Option<MailFns>,
    pub delivery: Delivery,
}

pub(crate) struct Build {
    /// Tags this tree's handles (the same across rebuilds).
    pub tree: u32,
    /// Sparse: indexed by block index; holes are blocks of earlier builds.
    pub blocks: Vec<Option<BlockEntry>>,
    /// Sparse: indexed by port index.
    pub ports: Vec<Option<PortEntry>>,
    /// Block indices in registration order (parents before children).
    pub order: Vec<u32>,
    /// (output port, input port)
    pub links: Vec<(u32, u32)>,
    /// (outbox, inbox) within this tree.
    pub mail_links: Vec<(u32, u32)>,
    pub errors: Vec<BuildError>,
    /// Indices from the previous build, by identity.
    previous_blocks: HashMap<BlockId, u32>,
    previous_ports: HashMap<InterfaceId, u32>,
}

impl Build {
    /// Run every stage up to linking. `root_id` identifies the root block;
    /// every other ID derives from it. With `previous`, existing blocks and
    /// interfaces keep their indices.
    pub fn new(
        root: Box<dyn Block>,
        root_id: BlockId,
        tree: u32,
        previous: Option<&Layout>,
    ) -> Result<Self, BuildError> {
        let mut build = Build {
            tree,
            blocks: Vec::new(),
            ports: Vec::new(),
            order: Vec::new(),
            links: Vec::new(),
            mail_links: Vec::new(),
            errors: Vec::new(),
            previous_blocks: previous.map(|p| p.block_index.clone()).unwrap_or_default(),
            previous_ports: previous.map(|p| p.port_index.clone()).unwrap_or_default(),
        };
        // Never reuse an index of the previous build (a kept block may still
        // hold a handle to it).
        let (block_floor, port_floor) = previous.map_or((0, 0), |p| (p.blocks.len(), p.ports.len()));
        build.blocks.resize_with(block_floor, || None);
        build.ports.resize_with(port_floor, || None);

        let root_index = build.alloc_block(root_id);
        build.place_block(root_index, BlockEntry::new(root_id, String::new(), String::new()));
        let mut root = root;
        let mut r = Registrar { build: &mut build, block: root_index };
        root.register_ports(&mut r);
        root.register(&mut r);
        let type_id = (*root).type_id();
        let entry = build.block_mut(root_index);
        entry.type_id = type_id;
        entry.block = Some(root);
        build.check()?;

        // Load: children before parents.
        for i in (0..build.order.len()).rev() {
            let index = build.order[i];
            let entry = build.block_mut(index);
            let (path, block_id) = (entry.path.clone(), entry.id);
            let block = entry.block.as_mut().expect("present outside link");
            block
                .load(&mut LoadContext { path: &path, id: block_id })
                .map_err(|source| BuildError::Load { path, source })?;
        }

        // Dictionaries: children first, so parents can include them.
        for i in (0..build.order.len()).rev() {
            let index = build.order[i];
            build.make_dictionary(index);
        }
        build.check()?;

        for i in 0..build.order.len() {
            let index = build.order[i];
            let mut block = build.block_mut(index).block.take().expect("present");
            let result = block.link(&mut Linker { build: &mut build, block: index });
            build.block_mut(index).block = Some(block);
            if let Err(source) = result {
                return Err(BuildError::Link { path: build.block(index).path.clone(), source });
            }
        }
        Ok(build)
    }

    fn check(&mut self) -> Result<(), BuildError> {
        if self.errors.is_empty() { Ok(()) } else { Err(self.errors.remove(0)) }
    }

    pub fn block(&self, index: u32) -> &BlockEntry {
        self.blocks[index as usize].as_ref().expect("live block")
    }

    pub fn block_mut(&mut self, index: u32) -> &mut BlockEntry {
        self.blocks[index as usize].as_mut().expect("live block")
    }

    pub fn port(&self, index: u32) -> &PortEntry {
        self.ports[index as usize].as_ref().expect("live port")
    }

    /// Live ports with their indices.
    pub fn live_ports(&self) -> impl Iterator<Item = (u32, &PortEntry)> {
        self.ports.iter().enumerate().filter_map(|(i, p)| Some((i as u32, p.as_ref()?)))
    }

    /// The index for block `id`: its previous index, or a fresh one.
    fn alloc_block(&mut self, id: BlockId) -> u32 {
        let fresh = self.blocks.len() as u32;
        let index = match self.previous_blocks.get(&id) {
            Some(&index) if self.blocks.get(index as usize).is_some_and(Option::is_none) => index,
            _ => fresh,
        };
        if index == fresh {
            self.blocks.push(None);
        }
        index
    }

    fn place_block(&mut self, index: u32, entry: BlockEntry) {
        self.blocks[index as usize] = Some(entry);
        self.order.push(index);
    }

    /// The index for interface `id`: its previous index, or a fresh one.
    fn alloc_port(&mut self, id: InterfaceId) -> u32 {
        let fresh = self.ports.len() as u32;
        let index = match self.previous_ports.get(&id) {
            Some(&index) if self.ports.get(index as usize).is_some_and(Option::is_none) => index,
            _ => fresh,
        };
        if index == fresh {
            self.ports.push(None);
        }
        index
    }

    /// For display: the port's labels from the root.
    pub fn port_path(&self, port: u32) -> String {
        let p = self.port(port);
        join(&self.block(p.owner).path, &p.label)
    }

    fn make_dictionary(&mut self, index: u32) {
        let entry = self.block(index);
        let hidden = |port: u32| entry.hidden.contains(&port);
        let mut dictionary: Vec<Interface> = Vec::new();
        for &port in entry.ports.iter().filter(|&&p| !hidden(p)) {
            let p = self.port(port);
            dictionary.push(self.interface(InterfaceId { block: entry.id, key: p.key }, p.label.clone(), port));
        }
        for &child in &entry.children {
            let child = self.block(child);
            for interface in child.dictionary.iter().filter(|i| !hidden(i.port)) {
                dictionary.push(Interface { path: join(&child.label, &interface.path), ..interface.clone() });
            }
        }
        let mut errors = Vec::new();
        for (key, label, port) in &entry.aliases {
            if dictionary.iter().any(|i| i.port == *port) {
                let id = InterfaceId { block: entry.id, key: *key };
                dictionary.push(self.interface(id, label.clone(), *port));
            } else {
                errors.push(format!("alias {label:?} points at an interface this block doesn't expose"));
            }
        }
        let path = entry.path.clone();
        self.errors.extend(errors.into_iter().map(|message| BuildError::Register { path: path.clone(), message }));
        self.block_mut(index).dictionary = dictionary;
    }

    fn interface(&self, id: InterfaceId, path: String, port: u32) -> Interface {
        let p = self.port(port);
        Interface { id, path, kind: p.kind, type_name: p.type_name, merge: p.merge, port, type_id: p.type_id }
    }
}

/// `a/b`, or just `b` when `a` is empty (the root).
pub(crate) fn join(a: &str, b: &str) -> String {
    if a.is_empty() { b.to_owned() } else { format!("{a}/{b}") }
}

impl BlockEntry {
    fn new(id: BlockId, label: String, path: String) -> Self {
        Self {
            block: None,
            id,
            type_id: TypeId::of::<()>(),
            label,
            path,
            children: Vec::new(),
            ports: Vec::new(),
            hidden: Vec::new(),
            aliases: Vec::new(),
            dictionary: Vec::new(),
        }
    }
}

// ---- registration ------------------------------------------------------------------

/// Handed to [`Block::register`]: declares the block's interfaces and
/// children. Each takes a [`Spec`] (key + label); `"name".into()` keys by
/// the name's hash and labels with the name.
pub struct Registrar<'a> {
    build: &'a mut Build,
    block: u32,
}

impl Registrar<'_> {
    /// This block's identity.
    pub fn id(&self) -> BlockId {
        self.build.block(self.block).id
    }

    /// For display: this block's labels from the root (`""` for the root).
    pub fn path(&self) -> &str {
        &self.build.block(self.block).path
    }

    /// An input that accepts one link. `default` is its value while
    /// unconnected: an unconnected input is a knob.
    pub fn input<T: Send + 'static>(&mut self, spec: impl Into<Spec>, default: T) -> In<T> {
        let entry = PortEntry { value: Some(Box::new(default)), ..PortEntry::new::<T>(InterfaceKind::In) };
        In::new(self.add(spec.into(), entry), self.build.tree)
    }

    /// An input that merges several links with `mode` (`Sum`, `Append`).
    /// `default` is its value while nothing is linked; with links, it is
    /// cleared and the sources are merged in every step.
    pub fn input_merged<T: Merge>(&mut self, spec: impl Into<Spec>, default: T, mode: MergeMode) -> In<T> {
        let spec = spec.into();
        if !T::supports(mode) {
            self.error(format!("input {:?}: {} doesn't support {mode:?}", spec.label, std::any::type_name::<T>()));
        }
        let entry = PortEntry {
            value: Some(Box::new(default)),
            merge: mode,
            clear: Some(clear_erased::<T> as ClearFn),
            merge_fn: Some(merge_erased::<T> as MergeFn),
            ..PortEntry::new::<T>(InterfaceKind::In)
        };
        In::new(self.add(spec, entry), self.build.tree)
    }

    /// An output, holding `initial` until the block first writes it.
    pub fn output<T: Send + 'static>(&mut self, spec: impl Into<Spec>, initial: T) -> Out<T> {
        let entry = PortEntry { value: Some(Box::new(initial)), ..PortEntry::new::<T>(InterfaceKind::Out) };
        Out::new(self.add(spec.into(), entry), self.build.tree)
    }

    /// An inbox: receives messages from any number of outboxes, in this
    /// workstream or others. `delivery`: keep every message (up to a
    /// capacity, dropping the oldest) or only the latest.
    pub fn inbox<M: Message>(&mut self, spec: impl Into<Spec>, delivery: Delivery) -> Inbox<M> {
        let entry = PortEntry { mail: Some(mail_fns::<M>()), delivery, ..PortEntry::new::<M>(InterfaceKind::Inbox) };
        Inbox::new(self.add(spec.into(), entry), self.build.tree)
    }

    /// An outbox: sends messages to every inbox linked to it (none: they're
    /// dropped). Never blocks.
    pub fn outbox<M: Message>(&mut self, spec: impl Into<Spec>) -> Outbox<M> {
        let entry = PortEntry { mail: Some(mail_fns::<M>()), ..PortEntry::new::<M>(InterfaceKind::Outbox) };
        Outbox::new(self.add(spec.into(), entry), self.build.tree)
    }

    /// Add `block` as a child. Its ID is derived from this block's ID and
    /// `spec.key`, so it's the same on every build. It registers right
    /// away; its non-private interfaces float up into this block's
    /// dictionary.
    pub fn child<B: Block>(&mut self, spec: impl Into<Spec>, block: B) -> Child<B> {
        let spec = spec.into();
        let id = self.id().child(spec.key);
        self.child_with_id(id, spec.label, block)
    }

    /// Add `block` as a child with an explicit ID: for children created
    /// from data (a patch file), whose IDs are stored with them.
    pub fn child_with_id<B: Block>(
        &mut self,
        id: BlockId,
        label: impl Into<std::borrow::Cow<'static, str>>,
        mut block: B,
    ) -> Child<B> {
        let label = label.into().into_owned();
        let taken = self.build.blocks.iter().flatten().any(|b| b.id == id);
        if taken {
            self.error(format!("two blocks have the same ID {id} (the second is labelled {label:?})"));
        }
        let path = join(self.path(), &label);
        // A duplicate gets a fresh index so it can't overwrite the first.
        let index = if taken {
            self.build.blocks.push(None);
            self.build.blocks.len() as u32 - 1
        } else {
            self.build.alloc_block(id)
        };
        self.build.place_block(index, BlockEntry::new(id, label, path));
        self.build.block_mut(self.block).children.push(index);
        let mut r = Registrar { build: self.build, block: index };
        block.register_ports(&mut r);
        block.register(&mut r);
        let entry = self.build.block_mut(index);
        entry.type_id = TypeId::of::<B>();
        entry.block = Some(Box::new(block));
        Child::new(index, self.build.tree)
    }

    /// A child registered earlier, as its own type, to read its handles
    /// (e.g. to mark one private or alias it). Panics if the handle isn't
    /// a child of this type, a mistake in the block's code.
    pub fn get<B: Block>(&self, child: Child<B>) -> &B {
        child_of(self.build, child)
    }

    /// Keep `port` (this block's own, or one a child exposes) out of this
    /// block's dictionary. This block can still link it; its parent can't
    /// see it.
    pub fn private(&mut self, port: impl PortHandle) {
        let port = port.port_index();
        self.build.block_mut(self.block).hidden.push(port);
    }

    /// Also expose `port` (own, or exposed by a child) as an interface of
    /// this block, under its own key and label: a second identity for the
    /// same interface, not a link.
    pub fn alias(&mut self, spec: impl Into<Spec>, port: impl PortHandle) {
        let spec = spec.into();
        self.check_key(spec.key, &spec.label);
        let port = port.port_index();
        self.build.block_mut(self.block).aliases.push((spec.key, spec.label.into_owned(), port));
    }

    fn add(&mut self, spec: Spec, entry: PortEntry) -> u32 {
        let clash = self.check_key(spec.key, &spec.label);
        let index = if clash {
            self.build.ports.push(None);
            self.build.ports.len() as u32 - 1
        } else {
            self.build.alloc_port(InterfaceId { block: self.id(), key: spec.key })
        };
        self.build.ports[index as usize] =
            Some(PortEntry { owner: self.block, key: spec.key, label: spec.label.into_owned(), ..entry });
        self.build.block_mut(self.block).ports.push(index);
        index
    }

    /// Keys identify interfaces within a block, so they must be unique
    /// (across ports, mailboxes and aliases). Labels may repeat. Returns
    /// whether `key` clashes.
    fn check_key(&mut self, key: Key, label: &str) -> bool {
        let entry = self.build.block(self.block);
        let clash = entry
            .ports
            .iter()
            .map(|&p| (self.build.port(p).key, self.build.port(p).label.clone()))
            .chain(entry.aliases.iter().map(|(k, l, _)| (*k, l.clone())))
            .find(|(k, _)| *k == key);
        if let Some((_, other)) = &clash {
            self.error(format!("{label:?} and {other:?} have the same key {key:?} (give one an explicit id)"));
        }
        clash.is_some()
    }

    fn error(&mut self, message: String) {
        let path = self.path().to_owned();
        self.build.errors.push(BuildError::Register { path, message });
    }
}

impl PortEntry {
    /// An entry of `kind` carrying `T`, everything else empty; owner, key
    /// and label are filled in by `Registrar::add`.
    fn new<T: 'static>(kind: InterfaceKind) -> Self {
        Self {
            owner: u32::MAX,
            key: Key::new(0),
            label: String::new(),
            kind,
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
            merge: MergeMode::Single,
            clear: None,
            merge_fn: None,
            value: None,
            mail: None,
            delivery: Delivery::default(),
        }
    }
}

// ---- loading -----------------------------------------------------------------------

/// Handed to [`Block::load`].
pub struct LoadContext<'a> {
    path: &'a str,
    id: BlockId,
}

impl LoadContext<'_> {
    /// The loading block's identity.
    pub fn id(&self) -> BlockId {
        self.id
    }

    /// For display: the loading block's labels from the root.
    pub fn path(&self) -> &str {
        self.path
    }
}

// ---- linking -----------------------------------------------------------------------

/// Handed to [`Block::link`]: connects the block's children to each other.
///
/// `&l[self.child]` gives a child back as its own type, to read its handles:
/// `l.connect(l[self.osc].out, l[self.filter].input)`.
pub struct Linker<'a> {
    build: &'a mut Build,
    block: u32,
}

impl Linker<'_> {
    /// Link an output to an input. Both must be exposed by children of this
    /// block (at any depth, unless something marked them private).
    pub fn connect<T: 'static>(&mut self, from: Out<T>, to: In<T>) -> Result<(), LinkError> {
        let (from, to) = (self.local(&from)?, self.local(&to)?);
        self.link(from, to)
    }

    /// Link an outbox to an inbox (both exposed by children). Several
    /// outboxes may feed one inbox, and one outbox may feed several inboxes.
    pub fn connect_mail<M: 'static>(&mut self, from: Outbox<M>, to: Inbox<M>) -> Result<(), LinkError> {
        let (from, to) = (self.local(&from)?, self.local(&to)?);
        self.link(from, to)
    }

    /// Link by ID, for wiring that comes from data (a patch file, the
    /// editor): output → input or outbox → inbox. Types are checked when
    /// linking.
    pub fn connect_ids(&mut self, from: InterfaceId, to: InterfaceId) -> Result<(), LinkError> {
        let from = self.lookup(from)?;
        let to = self.lookup(to)?;
        self.link(from, to)
    }

    /// The linking block's identity.
    pub fn id(&self) -> BlockId {
        self.build.block(self.block).id
    }

    /// A child's identity (to build [`InterfaceId`]s for `connect_ids`).
    pub fn id_of<B: Block>(&self, child: Child<B>) -> BlockId {
        child_of(self.build, child);
        self.build.block(child.index).id
    }

    /// The interfaces this block's children expose (what `connect_ids`
    /// accepts), with display paths relative to this block.
    pub fn available(&self) -> Vec<Interface> {
        let entry = self.build.block(self.block);
        entry
            .children
            .iter()
            .flat_map(|&c| {
                let child = self.build.block(c);
                child.dictionary.iter().map(|i| Interface { path: join(&child.label, &i.path), ..i.clone() })
            })
            .collect()
    }

    fn local(&self, handle: &impl PortHandle) -> Result<u32, LinkError> {
        if handle.port_tree() == u32::MAX {
            Err(LinkError::Unregistered)
        } else if handle.port_tree() != self.build.tree {
            Err(LinkError::ForeignHandle)
        } else if self.build.ports.get(handle.port_index() as usize).is_none_or(Option::is_none) {
            // A handle of an earlier build whose interface no longer exists.
            Err(LinkError::Unregistered)
        } else {
            Ok(handle.port_index())
        }
    }

    fn lookup(&self, id: InterfaceId) -> Result<u32, LinkError> {
        let entry = self.build.block(self.block);
        entry
            .children
            .iter()
            .flat_map(|&c| &self.build.block(c).dictionary)
            .find(|i| i.id == id)
            .map(|i| i.port)
            .ok_or(LinkError::UnknownInterface(id))
    }

    fn visible(&self, port: u32) -> bool {
        let entry = self.build.block(self.block);
        entry.children.iter().any(|&c| self.build.block(c).dictionary.iter().any(|i| i.port == port))
    }

    fn link(&mut self, from: u32, to: u32) -> Result<(), LinkError> {
        let b = &*self.build;
        let (f, t) = (b.port(from), b.port(to));
        let mail = match (f.kind, t.kind) {
            (InterfaceKind::Out, InterfaceKind::In) => false,
            (InterfaceKind::Outbox, InterfaceKind::Inbox) => true,
            _ => return Err(LinkError::WrongDirection(b.port_path(from), b.port_path(to))),
        };
        for port in [from, to] {
            if !self.visible(port) {
                return Err(LinkError::NotVisible(b.port_path(port)));
            }
        }
        if f.type_id != t.type_id {
            return Err(LinkError::TypeMismatch {
                from: b.port_path(from),
                from_type: f.type_name,
                to: b.port_path(to),
                to_type: t.type_name,
            });
        }
        if mail {
            // Mailboxes fan in and out freely; linking the same pair twice
            // would only deliver every message twice.
            if !b.mail_links.contains(&(from, to)) {
                self.build.mail_links.push((from, to));
            }
            return Ok(());
        }
        if t.merge == MergeMode::Single && b.links.iter().any(|&(_, i)| i == to) {
            return Err(LinkError::AlreadyLinked(b.port_path(to)));
        }
        self.build.links.push((from, to));
        Ok(())
    }
}

impl<B: Block> Index<Child<B>> for Linker<'_> {
    type Output = B;

    /// The child as its own type. Panics if the handle isn't a child of
    /// this type, a mistake in the block's code caught while building.
    fn index(&self, child: Child<B>) -> &B {
        child_of(self.build, child)
    }
}

fn child_of<B: Block>(build: &Build, child: Child<B>) -> &B {
    let block = (child.tree == build.tree)
        .then(|| build.blocks.get(child.index as usize)?.as_ref()?.block.as_deref())
        .flatten();
    block
        .and_then(|b| (b as &dyn Any).downcast_ref::<B>())
        .unwrap_or_else(|| panic!("{child:?} isn't a registered child of this type in this tree"))
}
