//! Building a block tree: registration, loading, interface dictionaries,
//! linking. Everything here runs before the workstream starts stepping.

use std::{
    any::{Any, TypeId},
    ops::Index,
};

use crate::{
    Block, BlockError, Child, In, Merge, MergeMode, Out, PortHandle,
    merge::{ClearFn, MergeFn, clear_erased, merge_erased},
};

/// Which way data flows through a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    In,
    Out,
}

/// One entry of a block's public dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    /// Relative to the block whose dictionary this is: `filter/cutoff`.
    pub path: String,
    pub direction: Direction,
    /// The Rust type carried, for display (`f32`, `enso_block::Events<..>`).
    pub type_name: &'static str,
    pub merge: MergeMode,
    pub(crate) port: u32,
    pub(crate) type_id: TypeId,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LinkError {
    #[error("{0:?} isn't exposed by any child (it may be private, or the parent's own port)")]
    NotVisible(String),
    #[error("{0:?} isn't an interface of any child")]
    UnknownPath(String),
    #[error("{from:?} carries {from_type}, but {to:?} expects {to_type}")]
    TypeMismatch { from: String, from_type: &'static str, to: String, to_type: &'static str },
    #[error("{0:?} must be an output and {1:?} an input")]
    WrongDirection(String, String),
    #[error("{0:?} accepts only one link (its merge mode is Single)")]
    AlreadyLinked(String),
    #[error("a handle wasn't registered (missing assignment in `register`?)")]
    Unregistered,
}

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
}

// ---- build state -------------------------------------------------------------------

pub(crate) struct BlockEntry {
    pub block: Option<Box<dyn Block>>,
    pub name: String,
    /// From the root: "" for the root, "voice/osc" below it.
    pub path: String,
    pub children: Vec<u32>,
    pub ports: Vec<u32>,
    /// Ports (own or descendants') this block keeps from floating up.
    pub hidden: Vec<u32>,
    pub aliases: Vec<(String, u32)>,
    pub dictionary: Vec<Interface>,
}

pub(crate) struct PortEntry {
    pub owner: u32,
    pub name: String,
    pub direction: Direction,
    pub type_id: TypeId,
    pub type_name: &'static str,
    pub merge: MergeMode,
    /// For merging inputs.
    pub clear: Option<ClearFn>,
    pub merge_fn: Option<MergeFn>,
    /// An output's initial value, or an input's default. Moved into the
    /// workstream's storage when compiled.
    pub value: Option<Box<dyn Any + Send>>,
}

#[derive(Default)]
pub(crate) struct Build {
    pub blocks: Vec<BlockEntry>,
    pub ports: Vec<PortEntry>,
    /// (output port, input port)
    pub links: Vec<(u32, u32)>,
    pub errors: Vec<BuildError>,
}

impl Build {
    /// Run every stage up to linking.
    pub fn new(root: Box<dyn Block>) -> Result<Self, BuildError> {
        let mut build = Build::default();
        build.blocks.push(BlockEntry::new(String::new(), String::new()));
        let mut root = root;
        root.register(&mut Registrar { build: &mut build, block: 0 });
        build.blocks[0].block = Some(root);
        build.check()?;

        // Load: children before parents (they're registered after them).
        for id in (0..build.blocks.len()).rev() {
            let path = build.blocks[id].path.clone();
            let block = build.blocks[id].block.as_mut().expect("present outside link");
            block
                .load(&mut LoadContext { path: &path })
                .map_err(|source| BuildError::Load { path, source })?;
        }

        // Dictionaries: children first, so parents can include them.
        for id in (0..build.blocks.len()).rev() {
            build.make_dictionary(id as u32);
        }
        build.check()?;

        for id in 0..build.blocks.len() {
            let mut block = build.blocks[id].block.take().expect("present");
            let result = block.link(&mut Linker { build: &mut build, block: id as u32 });
            build.blocks[id].block = Some(block);
            if let Err(source) = result {
                return Err(BuildError::Link { path: build.blocks[id].path.clone(), source });
            }
        }
        Ok(build)
    }

    fn check(&mut self) -> Result<(), BuildError> {
        if self.errors.is_empty() { Ok(()) } else { Err(self.errors.remove(0)) }
    }

    pub fn port_path(&self, port: u32) -> String {
        let p = &self.ports[port as usize];
        let owner = &self.blocks[p.owner as usize].path;
        if owner.is_empty() { p.name.clone() } else { format!("{owner}/{}", p.name) }
    }

    fn make_dictionary(&mut self, id: u32) {
        let entry = &self.blocks[id as usize];
        let hidden = |port: u32| entry.hidden.contains(&port);
        let mut dictionary: Vec<Interface> = Vec::new();
        for &port in entry.ports.iter().filter(|&&p| !hidden(p)) {
            // A block's own port appears in its dictionary under its name.
            dictionary.push(self.interface(self.ports[port as usize].name.clone(), port));
        }
        for &child in &entry.children {
            let child = &self.blocks[child as usize];
            for interface in child.dictionary.iter().filter(|i| !hidden(i.port)) {
                dictionary.push(Interface { path: format!("{}/{}", child.name, interface.path), ..interface.clone() });
            }
        }
        let mut errors = Vec::new();
        for (alias, port) in &entry.aliases {
            if dictionary.iter().any(|i| &i.path == alias) {
                errors.push(format!("alias {alias:?} clashes with an existing interface"));
            } else if dictionary.iter().any(|i| i.port == *port) {
                dictionary.push(Interface { path: alias.clone(), ..self.interface(String::new(), *port) });
            } else {
                errors.push(format!("alias {alias:?} points at an interface this block doesn't expose"));
            }
        }
        let path = entry.path.clone();
        self.errors.extend(errors.into_iter().map(|message| BuildError::Register { path: path.clone(), message }));
        self.blocks[id as usize].dictionary = dictionary;
    }

    fn interface(&self, path: String, port: u32) -> Interface {
        let p = &self.ports[port as usize];
        Interface { path, direction: p.direction, type_name: p.type_name, merge: p.merge, port, type_id: p.type_id }
    }
}

impl BlockEntry {
    fn new(name: String, path: String) -> Self {
        Self {
            block: None,
            name,
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

/// Handed to [`Block::register`]: declares the block's ports and children.
pub struct Registrar<'a> {
    build: &'a mut Build,
    block: u32,
}

impl Registrar<'_> {
    /// This block's path from the root (`""` for the root).
    pub fn path(&self) -> &str {
        &self.build.blocks[self.block as usize].path
    }

    /// An input that accepts one link. `default` is its value while
    /// unconnected: an unconnected input is a knob.
    pub fn input<T: Send + 'static>(&mut self, name: &str, default: T) -> In<T> {
        In::new(self.port(name, Direction::In, MergeMode::Single, None, Box::new(default), port_type::<T>()))
    }

    /// An input that merges several links with `mode` (`Sum`, `Append`).
    /// `default` is its value while nothing is linked; with links, it is
    /// cleared and the sources are merged in every step.
    pub fn input_merged<T: Merge>(&mut self, name: &str, default: T, mode: MergeMode) -> In<T> {
        if !T::supports(mode) {
            self.error(format!("input {name:?}: {} doesn't support {mode:?}", std::any::type_name::<T>()));
        }
        let fns = Some((clear_erased::<T> as ClearFn, merge_erased::<T> as MergeFn));
        In::new(self.port(name, Direction::In, mode, fns, Box::new(default), port_type::<T>()))
    }

    /// An output, holding `initial` until the block first writes it.
    pub fn output<T: Send + 'static>(&mut self, name: &str, initial: T) -> Out<T> {
        Out::new(self.port(name, Direction::Out, MergeMode::Single, None, Box::new(initial), port_type::<T>()))
    }

    /// Add `block` as a child called `name`. It registers right away; its
    /// non-private interfaces float up into this block's dictionary as
    /// `name/...`.
    pub fn child<B: Block>(&mut self, name: &str, mut block: B) -> Child<B> {
        self.check_name(name);
        let parent = &self.build.blocks[self.block as usize];
        let path = if parent.path.is_empty() { name.to_owned() } else { format!("{}/{name}", parent.path) };
        let id = self.build.blocks.len() as u32;
        self.build.blocks.push(BlockEntry::new(name.to_owned(), path));
        self.build.blocks[self.block as usize].children.push(id);
        block.register(&mut Registrar { build: self.build, block: id });
        self.build.blocks[id as usize].block = Some(Box::new(block));
        Child::new(id)
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
        self.build.blocks[self.block as usize].hidden.push(port);
    }

    /// Also expose `port` (own, or exposed by a child) under `name`: a
    /// second name for the same interface, not a link.
    pub fn alias(&mut self, name: &str, port: impl PortHandle) {
        self.check_name(name);
        let port = port.port_index();
        self.build.blocks[self.block as usize].aliases.push((name.to_owned(), port));
    }

    fn port(
        &mut self,
        name: &str,
        direction: Direction,
        merge: MergeMode,
        fns: Option<(ClearFn, MergeFn)>,
        value: Box<dyn Any + Send>,
        (type_id, type_name): (TypeId, &'static str),
    ) -> u32 {
        self.check_name(name);
        let index = self.build.ports.len() as u32;
        self.build.ports.push(PortEntry {
            owner: self.block,
            name: name.to_owned(),
            direction,
            type_id,
            type_name,
            merge,
            clear: fns.map(|f| f.0),
            merge_fn: fns.map(|f| f.1),
            value: Some(value),
        });
        self.build.blocks[self.block as usize].ports.push(index);
        index
    }

    /// Names are path segments: unique within the block, no `/`.
    fn check_name(&mut self, name: &str) {
        let entry = &self.build.blocks[self.block as usize];
        let taken = entry.ports.iter().any(|&p| self.build.ports[p as usize].name == name)
            || entry.children.iter().any(|&c| self.build.blocks[c as usize].name == name)
            || entry.aliases.iter().any(|(a, _)| a == name);
        if name.is_empty() || name.contains('/') {
            self.error(format!("invalid name {name:?} (must be non-empty, without '/')"));
        } else if taken {
            self.error(format!("name {name:?} is used twice"));
        }
    }

    fn error(&mut self, message: String) {
        let path = self.path().to_owned();
        self.build.errors.push(BuildError::Register { path, message });
    }
}

fn port_type<T: 'static>() -> (TypeId, &'static str) {
    (TypeId::of::<T>(), std::any::type_name::<T>())
}

// ---- loading -----------------------------------------------------------------------

/// Handed to [`Block::load`].
pub struct LoadContext<'a> {
    path: &'a str,
}

impl LoadContext<'_> {
    /// The loading block's path from the root.
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
        if !from.is_registered() || !to.is_registered() {
            return Err(LinkError::Unregistered);
        }
        self.link(from.index, to.index)
    }

    /// Link by path, relative to this block (`"osc/out"`, `"filter/input"`),
    /// for wiring that comes from data rather than code. Types are
    /// checked when linking.
    pub fn connect_paths(&mut self, from: &str, to: &str) -> Result<(), LinkError> {
        let from = self.lookup(from)?;
        let to = self.lookup(to)?;
        self.link(from, to)
    }

    /// The interfaces this block's children expose, with paths relative to
    /// this block (what `connect_paths` accepts).
    pub fn available(&self) -> Vec<Interface> {
        let entry = &self.build.blocks[self.block as usize];
        entry
            .children
            .iter()
            .flat_map(|&c| {
                let child = &self.build.blocks[c as usize];
                child.dictionary.iter().map(|i| Interface { path: format!("{}/{}", child.name, i.path), ..i.clone() })
            })
            .collect()
    }

    fn lookup(&self, path: &str) -> Result<u32, LinkError> {
        self.available()
            .into_iter()
            .find(|i| i.path == path)
            .map(|i| i.port)
            .ok_or_else(|| LinkError::UnknownPath(path.to_owned()))
    }

    fn visible(&self, port: u32) -> bool {
        let entry = &self.build.blocks[self.block as usize];
        entry.children.iter().any(|&c| self.build.blocks[c as usize].dictionary.iter().any(|i| i.port == port))
    }

    fn link(&mut self, from: u32, to: u32) -> Result<(), LinkError> {
        let b = &*self.build;
        let (f, t) = (&b.ports[from as usize], &b.ports[to as usize]);
        if f.direction != Direction::Out || t.direction != Direction::In {
            return Err(LinkError::WrongDirection(b.port_path(from), b.port_path(to)));
        }
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
    let block = build.blocks.get(child.index as usize).and_then(|e| e.block.as_deref());
    block
        .and_then(|b| (b as &dyn Any).downcast_ref::<B>())
        .unwrap_or_else(|| panic!("{child:?} isn't a registered child of this type"))
}
