//! The control half of a workstream (step 3: swapping while running).
//!
//! A workstream's graph is owned by whoever steps it, often a real-time
//! thread. Everything that changes it is prepared here, on the control
//! side, and sent as a [`Command`] through a lock-free queue. The stepping
//! side applies commands at the start of a step by moving pointers only;
//! what it replaces comes back as [`Garbage`], to be freed here. The same
//! split as `enso-audio`'s engine and processor.

use std::{any::Any, collections::HashMap, sync::Arc};

use crate::{
    AccessError, Block, BuildError, InterfaceId,
    build::Build,
    graph::{Graph, Layout, TargetTable, compile},
    mail::AnyMailbox,
};

const QUEUE: usize = 64;

/// Control side → stepping side.
pub(crate) enum Command {
    Swap(Box<Swap>),
    Targets(Box<TargetTable>),
    Set { slot: u32, value: Box<dyn Any + Send> },
}

/// A rebuilt graph, with what to carry over from the running one.
pub(crate) struct Swap {
    pub graph: Box<Graph>,
    pub targets: Box<TargetTable>,
    /// Block indices where the fresh instance takes over the running one's
    /// state (`#[state]` fields, `Block::carry_over`): same ID and type.
    /// Stable indices make them the same in both graphs.
    pub carry_blocks: Vec<u32>,
    /// (old slot, new slot): values that carry over (outputs and knobs).
    pub keep_slots: Vec<(u32, u32)>,
}

/// Stepping side → control side: things to free off the stepping thread.
/// Never read, only dropped.
#[allow(dead_code)]
pub(crate) enum Garbage {
    Swap(Box<Swap>),
    Targets(Box<TargetTable>),
    Value(Box<dyn Any + Send>),
}

/// The stepping side's ends of the queues.
pub(crate) struct RunnerQueues {
    pub commands: rtrb::Consumer<Command>,
    pub garbage: rtrb::Producer<Garbage>,
}

pub(crate) struct Control {
    /// The layout of the most recent graph sent (or built).
    pub layout: Arc<Layout>,
    /// Per outbox port: every target (inside the tree and engine-made).
    targets: Vec<Vec<AnyMailbox>>,
    /// Targets the engine added, by outbox ID, so they survive rebuilds.
    external: HashMap<InterfaceId, Vec<AnyMailbox>>,
    commands: rtrb::Producer<Command>,
    garbage: rtrb::Consumer<Garbage>,
}

impl Control {
    /// Build and compile `root`; returns the control side, the graph and its
    /// targets, and the stepping side's queue ends.
    pub fn new(
        root: Box<dyn Block>,
        root_id: crate::BlockId,
        realtime: bool,
    ) -> Result<(Self, Box<Graph>, Box<TargetTable>, RunnerQueues), BuildError> {
        let tree = crate::ids::next_tree();
        let build = Build::new(root, root_id, tree, None)?;
        let (graph, targets) = compile(build, root_id, realtime, None, &HashMap::new())?;
        let table = TargetTable::new(&graph.layout, &targets);
        let (commands, command_rx) = rtrb::RingBuffer::new(QUEUE);
        // Each command returns at most one piece of garbage.
        let (garbage_tx, garbage) = rtrb::RingBuffer::new(QUEUE);
        let control = Self { layout: graph.layout.clone(), targets, external: HashMap::new(), commands, garbage };
        Ok((control, Box::new(graph), Box::new(table), RunnerQueues { commands: command_rx, garbage: garbage_tx }))
    }

    /// Build `root` again (same root ID, same tree), compile it with stable
    /// indices, and send it to the stepping side, which swaps it in at the
    /// start of its next step.
    pub fn rebuild(&mut self, root: Box<dyn Block>) -> Result<(), BuildError> {
        self.collect_garbage();
        if self.commands.slots() == 0 {
            return Err(BuildError::Busy);
        }
        let old = self.layout.clone();
        let build = Build::new(root, old.root_id, old.tree, Some(&old))?;
        let (graph, targets) = compile(build, old.root_id, old.realtime, Some(&old), &self.external)?;
        let new = graph.layout.clone();
        let table = TargetTable::new(&new, &targets);
        let swap = Swap {
            carry_blocks: carry_blocks(&old, &new),
            keep_slots: keep_slots(&old, &new),
            graph: Box::new(graph),
            targets: Box::new(table),
        };
        self.send(Command::Swap(Box::new(swap))).map_err(|_| BuildError::Busy)?;
        self.layout = new;
        self.targets = targets;
        Ok(())
    }

    /// Add `mailbox` to the targets of the outbox `port` (a link made by the
    /// engine). Remembered by the outbox's ID across rebuilds.
    pub fn add_target(&mut self, port: usize, mailbox: AnyMailbox) -> Result<(), AccessError> {
        self.collect_garbage();
        let id = self.layout.port(port).id;
        self.external.entry(id).or_default().push(mailbox.clone());
        self.targets[port].push(mailbox);
        let table = TargetTable::new(&self.layout, &self.targets);
        self.send(Command::Targets(Box::new(table))).map_err(|_| AccessError::Busy)
    }

    /// Replace the value of the knob in `slot`.
    pub fn set(&mut self, slot: usize, value: Box<dyn Any + Send>) -> Result<(), AccessError> {
        self.collect_garbage();
        self.send(Command::Set { slot: slot as u32, value }).map_err(|_| AccessError::Busy)
    }

    /// Free what the stepping side has sent back.
    pub fn collect_garbage(&mut self) {
        while let Ok(garbage) = self.garbage.pop() {
            drop(garbage);
        }
    }

    fn send(&mut self, command: Command) -> Result<(), ()> {
        self.commands.push(command).map_err(|_| ())
    }
}

/// Blocks whose state can carry over: same ID (hence same index) and same
/// type. (The fresh instance always runs, so its configuration applies; the
/// block itself decides what state survives, even if its ports changed.)
fn carry_blocks(old: &Layout, new: &Layout) -> Vec<u32> {
    new.blocks
        .iter()
        .enumerate()
        .filter_map(|(i, block)| {
            let (block, before) = (block.as_ref()?, old.blocks.get(i)?.as_ref()?);
            (block.id == before.id && block.type_id == before.type_id).then_some(i as u32)
        })
        .collect()
}

/// Values that carry over: outputs (their last value) and knobs (their
/// setting), where the port still exists with the same type and still
/// owns its slot.
fn keep_slots(old: &Layout, new: &Layout) -> Vec<(u32, u32)> {
    new.ports
        .iter()
        .enumerate()
        .filter_map(|(i, port)| {
            let (port, before) = (port.as_ref()?, old.ports.get(i)?.as_ref()?);
            let same = port.id == before.id && port.type_id == before.type_id && port.kind == before.kind;
            let both_own = port.owns_slot && before.owns_slot && port.knob == before.knob;
            (same && both_own).then_some((before.slot?, port.slot?))
        })
        .collect()
}
