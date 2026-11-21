//! [`Workstream`]: runs one block tree, step after step.

use std::{
    any::{Any, TypeId},
    cell::{Ref, RefCell, RefMut},
    panic::{AssertUnwindSafe, catch_unwind},
};

use crate::{
    Block, BuildError, In, Interface, MergeMode, Out,
    build::{Build, Direction},
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
pub struct Workstream {
    blocks: Vec<Option<Box<dyn Block>>>,
    paths: Vec<String>,
    /// Block indices in execution order.
    order: Vec<u32>,
    /// Per block: merges to run before it.
    merges: Vec<Vec<MergeStep>>,
    slots: Vec<RefCell<Box<dyn Any + Send>>>,
    /// Per port: the slot it reads or writes.
    port_slot: Vec<u32>,
    port_owner: Vec<u32>,
    port_type: Vec<(TypeId, &'static str)>,
    /// Per port: true if it's an input holding its own value with nothing
    /// linked (a knob that [`set`](Self::set) may change).
    knob: Vec<bool>,
    interfaces: Vec<Interface>,
    steps: u64,
    disabled: Vec<u32>,
}

impl Workstream {
    /// Register, load and link `root` and its children, then compile them
    /// into a runnable graph.
    pub fn build(root: impl Block) -> Result<Self, BuildError> {
        Self::compile(Build::new(Box::new(root))?)
    }

    fn compile(mut build: Build) -> Result<Self, BuildError> {
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
            if port.direction == Direction::Out {
                port_slot[i] = new_slot(port.value.take().expect("value"));
            }
        }
        // Inputs: a single link reads the source's slot directly (no copy);
        // otherwise the input keeps its own value.
        let mut merges: Vec<Vec<MergeStep>> = (0..build.blocks.len()).map(|_| Vec::new()).collect();
        for (i, port) in build.ports.iter_mut().enumerate() {
            if port.direction != Direction::In {
                continue;
            }
            let sources: Vec<usize> =
                build.links.iter().filter(|&&(_, to)| to as usize == i).map(|&(from, _)| port_slot[from as usize] as usize).collect();
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

        let order = execution_order(&build)?;
        let interfaces = std::mem::take(&mut build.blocks[0].dictionary);
        Ok(Self {
            paths: build.blocks.iter().map(|b| b.path.clone()).collect(),
            blocks: build.blocks.into_iter().map(|b| b.block).collect(),
            order,
            merges,
            slots,
            port_slot,
            port_owner: build.ports.iter().map(|p| p.owner).collect(),
            port_type: build.ports.iter().map(|p| (p.type_id, p.type_name)).collect(),
            knob,
            interfaces,
            steps: 0,
            disabled: Vec::new(),
        })
    }

    /// Run every block once, in dependency order. A block that panics is
    /// switched off (see [`disabled`](Self::disabled)); the rest carry on.
    pub fn step(&mut self) {
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

    /// The root block's dictionary: everything it exposes, by path.
    pub fn interfaces(&self) -> &[Interface] {
        &self.interfaces
    }

    /// The current value of the interface at `path` (an output, or what an
    /// input sees).
    pub fn read<T: 'static>(&self, path: &str) -> Result<Ref<'_, T>, AccessError> {
        let port = self.port::<T>(path)?;
        let slot = &self.slots[self.port_slot[port] as usize];
        Ok(Ref::map(slot.borrow(), |v| (**v).downcast_ref::<T>().expect("type checked")))
    }

    /// Turn a knob: set the value of an unconnected input. Call between
    /// steps.
    pub fn set<T: Send + 'static>(&mut self, path: &str, value: T) -> Result<(), AccessError> {
        let port = self.port::<T>(path)?;
        if !self.knob[port] {
            return Err(AccessError::NotAKnob(path.to_owned()));
        }
        *self.slots[self.port_slot[port] as usize].borrow_mut() = Box::new(value);
        Ok(())
    }

    /// Paths of blocks switched off because they panicked.
    pub fn disabled(&self) -> impl Iterator<Item = &str> {
        self.disabled.iter().map(|&b| self.paths[b as usize].as_str())
    }

    fn port<T: 'static>(&self, path: &str) -> Result<usize, AccessError> {
        let interface =
            self.interfaces.iter().find(|i| i.path == path).ok_or_else(|| AccessError::Unknown(path.to_owned()))?;
        let port = interface.port as usize;
        let (type_id, type_name) = self.port_type[port];
        if type_id != TypeId::of::<T>() {
            return Err(AccessError::WrongType { path: path.to_owned(), actual: type_name });
        }
        Ok(port)
    }
}

/// Topological order of all blocks: a block runs after every block whose
/// outputs it reads. Kahn's algorithm, taking blocks in registration order
/// whenever there's a choice, so the order is deterministic.
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

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AccessError {
    #[error("no interface {0:?} on the root block")]
    Unknown(String),
    #[error("{path:?} carries {actual}")]
    WrongType { path: String, actual: &'static str },
    #[error("{0:?} isn't an unconnected input")]
    NotAKnob(String),
}

/// A block's access to its ports during [`Block::process`].
pub struct Io<'a> {
    block: u32,
    slots: &'a [RefCell<Box<dyn Any + Send>>],
    port_slot: &'a [u32],
    port_owner: &'a [u32],
    step: u64,
}

impl Io<'_> {
    /// Read an input: the linked output's value, the merged inputs, or the
    /// knob's value.
    pub fn read<T: 'static>(&self, input: In<T>) -> Ref<'_, T> {
        let slot = self.slot(input.index);
        Ref::map(slot.borrow(), |v| (**v).downcast_ref::<T>().expect("port type"))
    }

    /// Write an output. Downstream blocks read it later in this step.
    pub fn write<T: 'static>(&self, output: Out<T>) -> RefMut<'_, T> {
        let slot = self.slot(output.index);
        RefMut::map(slot.borrow_mut(), |v| (**v).downcast_mut::<T>().expect("port type"))
    }

    /// Which step this is (0, 1, 2, ...).
    pub fn step(&self) -> u64 {
        self.step
    }

    /// Panics (and so switches the block off) on another block's port: a
    /// bug in the block, never user input.
    fn slot(&self, port: u32) -> &RefCell<Box<dyn Any + Send>> {
        let port = port as usize;
        assert!(
            self.port_owner.get(port) == Some(&self.block),
            "a block may only use its own ports (port {port})"
        );
        &self.slots[self.port_slot[port] as usize]
    }
}
