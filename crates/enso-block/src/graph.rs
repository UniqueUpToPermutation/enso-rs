//! Compiling a built tree into what a workstream runs.
//!
//! - [`Graph`]: what the stepping thread owns: block instances, execution
//!   order, storage slots, mailboxes. Swapped wholesale on a rebuild.
//! - [`Layout`]: an immutable description of a graph (identities, types,
//!   paths, where every port's value lives), shared (`Arc`) by the stepping
//!   side and the control side, so both can check handles and IDs.

use std::{
    any::{Any, TypeId},
    cell::{Cell, RefCell},
    collections::HashMap,
    sync::Arc,
};

use crate::{
    AccessError, Block, BlockId, BuildError, Interface, InterfaceId, InterfaceKind, MergeMode, PortHandle,
    build::Build,
    mail::{AnyMailbox, MailFns},
    merge::{ClearFn, MergeFn},
};

/// One input's fan-in, done before its block runs.
pub(crate) struct MergeStep {
    pub into: usize,
    pub sources: Vec<usize>,
    pub mode: MergeMode,
    pub clear: ClearFn,
    pub merge: MergeFn,
}

pub(crate) struct BlockInfo {
    pub id: BlockId,
    pub type_id: TypeId,
    pub path: String,
}

pub(crate) struct PortInfo {
    pub id: InterfaceId,
    pub kind: InterfaceKind,
    pub type_id: TypeId,
    pub type_name: &'static str,
    pub path: String,
    /// The storage slot this port reads or writes (ports only).
    pub slot: Option<u32>,
    /// An output (owns its slot) or an unconnected input holding its own
    /// value (a knob). Values of such slots survive rebuilds.
    pub owns_slot: bool,
    pub knob: bool,
    pub mail: Option<MailFns>,
    /// Inboxes: the mailbox, reused across rebuilds so senders stay valid.
    pub mailbox: Option<AnyMailbox>,
}

/// An immutable description of a compiled graph.
pub(crate) struct Layout {
    pub tree: u32,
    pub root_id: BlockId,
    pub realtime: bool,
    /// Indexed by block index (sparse: holes are blocks of earlier builds).
    pub blocks: Vec<Option<BlockInfo>>,
    /// Indexed by port index (sparse).
    pub ports: Vec<Option<PortInfo>>,
    pub block_index: HashMap<BlockId, u32>,
    pub port_index: HashMap<InterfaceId, u32>,
    /// The root's dictionary.
    pub interfaces: Vec<Interface>,
    /// Ports reachable from outside (in the root's dictionary).
    pub exposed: Vec<bool>,
}

impl Layout {
    pub fn port(&self, port: usize) -> &PortInfo {
        self.ports[port].as_ref().expect("live port")
    }

    /// A handle's port, if it's from this tree, still exists, and is
    /// exposed by the root.
    pub fn handle(&self, handle: &impl PortHandle) -> Result<usize, AccessError> {
        if handle.port_tree() == u32::MAX {
            return Err(AccessError::Unregistered);
        }
        if handle.port_tree() != self.tree {
            return Err(AccessError::ForeignHandle);
        }
        let port = handle.port_index() as usize;
        match self.ports.get(port) {
            Some(Some(_)) if self.exposed[port] => Ok(port),
            Some(Some(info)) => Err(AccessError::NotExposed(info.path.clone())),
            _ => Err(AccessError::Gone),
        }
    }

    /// Like [`handle`](Self::handle), also checking the value type (a
    /// handle from before a rebuild may name a port whose type changed).
    pub fn typed<T: 'static>(&self, handle: &impl PortHandle) -> Result<usize, AccessError> {
        let port = self.handle(handle)?;
        self.check_type::<T>(port)?;
        Ok(port)
    }

    pub fn by_id(&self, id: InterfaceId) -> Result<usize, AccessError> {
        self.interfaces
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.port as usize)
            .ok_or(AccessError::UnknownInterface(id))
    }

    pub fn check_type<T: 'static>(&self, port: usize) -> Result<(), AccessError> {
        let info = self.port(port);
        if info.type_id != TypeId::of::<T>() {
            return Err(AccessError::WrongType { path: info.path.clone(), actual: info.type_name });
        }
        Ok(())
    }

    /// The slot of a knob (an unconnected input), for setting it.
    pub fn knob_slot(&self, port: usize) -> Result<usize, AccessError> {
        let info = self.port(port);
        match (info.knob, info.slot) {
            (true, Some(slot)) => Ok(slot as usize),
            _ => Err(AccessError::NotAKnob(info.path.clone())),
        }
    }
}

/// Per outbox port: its targets, as the type-specific `Vec<Arc<Mailbox<M>>>`.
pub(crate) struct TargetTable(pub Vec<Option<Box<dyn Any + Send + Sync>>>);

impl TargetTable {
    /// Typed target lists from type-erased ones.
    pub fn new(layout: &Layout, lists: &[Vec<AnyMailbox>]) -> Self {
        let table = layout
            .ports
            .iter()
            .enumerate()
            .map(|(port, info)| {
                let info = info.as_ref()?;
                let fns = info.mail.filter(|_| info.kind == InterfaceKind::Outbox)?;
                let mut targets = (fns.make_targets)();
                for mailbox in lists.get(port).into_iter().flatten() {
                    (fns.add_target)(&mut *targets, mailbox.clone());
                }
                Some(targets)
            })
            .collect();
        Self(table)
    }
}

/// What a workstream's stepping thread owns.
pub(crate) struct Graph {
    pub blocks: Vec<Option<Box<dyn Block>>>,
    /// Block indices in execution order.
    pub order: Vec<u32>,
    /// Per block: merges to run before it.
    pub merges: Vec<Vec<MergeStep>>,
    pub slots: Vec<RefCell<Box<dyn Any + Send>>>,
    /// Per port: the slot it reads or writes (`u32::MAX` for none).
    pub port_slot: Vec<u32>,
    /// Per port: its block (`u32::MAX` for holes).
    pub port_owner: Vec<u32>,
    /// Per port: the inbox's mailbox.
    pub mailbox: Vec<Option<AnyMailbox>>,
    /// Per port (inboxes): messages this step may still take.
    pub budget: Vec<Cell<usize>>,
    pub inboxes: Vec<u32>,
    pub layout: Arc<Layout>,
}

/// Compile a built tree. Inbox mailboxes of `previous` are reused for
/// inboxes that still exist; `external` targets (links made by the engine)
/// are added to outbox target lists by ID. Returns the graph and each
/// outbox's target list (type-erased).
pub(crate) fn compile(
    mut build: Build,
    root_id: BlockId,
    realtime: bool,
    previous: Option<&Layout>,
    external: &HashMap<InterfaceId, Vec<AnyMailbox>>,
) -> Result<(Graph, Vec<Vec<AnyMailbox>>), BuildError> {
    let port_count = build.ports.len();
    let block_count = build.blocks.len();
    let mut slots = Vec::new();
    let mut port_slot = vec![u32::MAX; port_count];
    let mut owns_slot = vec![false; port_count];
    let mut knob = vec![false; port_count];
    let mut new_slot = |value: Box<dyn Any + Send>| {
        slots.push(RefCell::new(value));
        (slots.len() - 1) as u32
    };

    // Outputs first: every output owns a slot.
    for (i, port) in build.ports.iter_mut().enumerate() {
        if let Some(port) = port.as_mut().filter(|p| p.kind == InterfaceKind::Out) {
            port_slot[i] = new_slot(port.value.take().expect("value"));
            owns_slot[i] = true;
        }
    }
    // Inputs: a single link reads the source's slot directly (no copy);
    // otherwise the input keeps its own value.
    let mut merges: Vec<Vec<MergeStep>> = (0..block_count).map(|_| Vec::new()).collect();
    for i in 0..port_count {
        let Some(port) = build.ports[i].as_ref().filter(|p| p.kind == InterfaceKind::In) else { continue };
        let sources: Vec<usize> = build
            .links
            .iter()
            .filter(|&&(_, to)| to as usize == i)
            .map(|&(from, _)| port_slot[from as usize] as usize)
            .collect();
        let (merge, owner, clear, merge_fn) = (port.merge, port.owner, port.clear, port.merge_fn);
        match (merge, sources.as_slice()) {
            (MergeMode::Single, [source]) => port_slot[i] = *source as u32,
            (_, []) => {
                let value = build.ports[i].as_mut().and_then(|p| p.value.take()).expect("value");
                port_slot[i] = new_slot(value);
                owns_slot[i] = true;
                knob[i] = true;
            }
            (mode, _) => {
                let value = build.ports[i].as_mut().and_then(|p| p.value.take()).expect("value");
                let into = new_slot(value);
                port_slot[i] = into;
                merges[owner as usize].push(MergeStep {
                    into: into as usize,
                    sources,
                    mode,
                    clear: clear.expect("merging inputs have merge functions"),
                    merge: merge_fn.expect("merging inputs have merge functions"),
                });
            }
        }
    }

    // Mailboxes: one per inbox (the previous one if the inbox survived).
    let mut mailbox: Vec<Option<AnyMailbox>> = vec![None; port_count];
    let mut inboxes = Vec::new();
    for (i, port) in build.live_ports() {
        let Some(fns) = port.mail else { continue };
        match port.kind {
            InterfaceKind::Inbox => {
                let reused = previous
                    .and_then(|p| p.ports.get(i as usize)?.as_ref())
                    .filter(|old| old.kind == InterfaceKind::Inbox && old.type_id == port.type_id)
                    .and_then(|old| old.mailbox.clone());
                mailbox[i as usize] = Some(reused.unwrap_or_else(|| (fns.make_mailbox)(port.delivery)));
                inboxes.push(i);
            }
            InterfaceKind::Outbox if realtime && fns.drops => {
                return Err(BuildError::NotRealtimeSafe { path: build.port_path(i), type_name: port.type_name });
            }
            _ => {}
        }
    }

    // Outbox targets: links inside the tree, plus those the engine made.
    let mut targets: Vec<Vec<AnyMailbox>> = vec![Vec::new(); port_count];
    for &(from, to) in &build.mail_links {
        targets[from as usize].push(mailbox[to as usize].clone().expect("inbox"));
    }
    let root_index = build.order[0];
    let root_block = build.block(root_index).id;
    for (i, port) in build.live_ports() {
        if port.kind == InterfaceKind::Outbox {
            let id = InterfaceId { block: build.block(port.owner).id, key: port.key };
            targets[i as usize].extend(external.get(&id).into_iter().flatten().cloned());
        }
    }

    let order = execution_order(&build)?;
    let interfaces = std::mem::take(&mut build.block_mut(root_index).dictionary);
    let mut exposed = vec![false; port_count];
    for interface in &interfaces {
        exposed[interface.port as usize] = true;
    }

    let layout = Layout {
        tree: build.tree,
        root_id: root_block,
        realtime,
        blocks: build
            .blocks
            .iter()
            .map(|b| {
                let b = b.as_ref()?;
                Some(BlockInfo { id: b.id, type_id: b.type_id, path: b.path.clone() })
            })
            .collect(),
        ports: (0..port_count)
            .map(|i| {
                let p = build.ports[i].as_ref()?;
                Some(PortInfo {
                    id: InterfaceId { block: build.block(p.owner).id, key: p.key },
                    kind: p.kind,
                    type_id: p.type_id,
                    type_name: p.type_name,
                    path: build.port_path(i as u32),
                    slot: (port_slot[i] != u32::MAX).then_some(port_slot[i]),
                    owns_slot: owns_slot[i],
                    knob: knob[i],
                    mail: p.mail,
                    mailbox: mailbox[i].clone(),
                })
            })
            .collect(),
        block_index: build.order.iter().map(|&b| (build.block(b).id, b)).collect(),
        port_index: build
            .live_ports()
            .map(|(i, p)| (InterfaceId { block: build.block(p.owner).id, key: p.key }, i))
            .collect(),
        interfaces,
        exposed,
    };
    debug_assert_eq!(layout.root_id, root_id);

    let graph = Graph {
        port_owner: (0..port_count).map(|i| build.ports[i].as_ref().map_or(u32::MAX, |p| p.owner)).collect(),
        blocks: build.blocks.into_iter().map(|b| b.and_then(|b| b.block)).collect(),
        order,
        merges,
        slots,
        port_slot,
        mailbox,
        budget: (0..port_count).map(|_| Cell::new(0)).collect(),
        inboxes,
        layout: Arc::new(layout),
    };
    Ok((graph, targets))
}

/// Topological order of all blocks: a block runs after every block whose
/// outputs it reads. Kahn's algorithm, taking blocks in registration order
/// whenever there's a choice, so the order is deterministic. (Mail links
/// don't order anything: messages arrive by the next step.)
fn execution_order(build: &Build) -> Result<Vec<u32>, BuildError> {
    let order = &build.order;
    let position: HashMap<u32, usize> = order.iter().enumerate().map(|(i, &b)| (b, i)).collect();
    let owner = |port: u32| position[&build.port(port).owner];
    let edges: Vec<(usize, usize)> = build.links.iter().map(|&(from, to)| (owner(from), owner(to))).collect();
    let mut incoming = vec![0usize; order.len()];
    for &(_, to) in &edges {
        incoming[to] += 1;
    }
    let mut done = vec![false; order.len()];
    let mut result = Vec::with_capacity(order.len());
    while result.len() < order.len() {
        let Some(next) = (0..order.len()).find(|&b| !done[b] && incoming[b] == 0) else {
            let stuck = (0..order.len()).filter(|&b| !done[b]).map(|b| build.block(order[b]).path.clone()).collect();
            return Err(BuildError::Cycle(stuck));
        };
        done[next] = true;
        result.push(order[next]);
        for &(from, to) in &edges {
            if from == next {
                incoming[to] -= 1;
            }
        }
    }
    Ok(result)
}
