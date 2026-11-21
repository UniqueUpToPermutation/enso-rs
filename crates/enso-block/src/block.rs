//! The [`Block`] trait and the handles blocks keep: [`In`], [`Out`],
//! [`Child`].

use std::{any::Any, fmt, marker::PhantomData};

use crate::{Io, Linker, LoadContext, Registrar};

/// Anything a block's `load` can fail with. A boxed error, so blocks can
/// use `?` on any error type.
pub type BlockError = Box<dyn std::error::Error + Send + Sync>;

/// A unit of computation. See `docs/design/blocks.md`.
///
/// The lifecycle, in order (all but `process` run before the workstream
/// starts, and may allocate and be slow):
/// 1. **construct:** with its structural configuration (`Mixer::new(8)`);
/// 2. [`register`](Self::register): declare ports and children;
/// 3. [`load`](Self::load): load resources;
/// 4. [`link`](Self::link): connect children to each other;
/// 5. [`process`](Self::process): the core routine, once per step.
///
/// `Any` is a supertrait so the [`Linker`] can hand a parent its children
/// back as their concrete types (`&l[self.osc]`).
pub trait Block: Any + Send {
    /// Declare ports, children, private flags and aliases. Store the
    /// returned handles in `self`.
    fn register(&mut self, r: &mut Registrar<'_>);

    /// Load resources (files, buffers). Children have already loaded.
    fn load(&mut self, cx: &mut LoadContext<'_>) -> Result<(), BlockError> {
        let _ = cx;
        Ok(())
    }

    /// Connect children's interfaces to each other. Never to `self`'s own
    /// ports (a parent doesn't take part in its children's wiring; give the
    /// job to a child instead).
    fn link(&mut self, l: &mut Linker<'_>) -> Result<(), crate::LinkError> {
        let _ = l;
        Ok(())
    }

    /// The core routine: read inputs, write outputs. Runs once per step,
    /// after every block it depends on. Follow the rules of the thread
    /// it runs on (the audio thread: no allocation, locks or I/O).
    fn process(&mut self, io: &Io<'_>) {
        let _ = io;
    }
}

/// Marks a handle field as not registered yet: what `Default` gives.
const UNSET: u32 = u32::MAX;

macro_rules! handle {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        pub struct $name<T> {
            pub(crate) index: u32,
            _type: PhantomData<fn() -> T>,
        }

        impl<T> $name<T> {
            pub(crate) fn new(index: u32) -> Self {
                Self { index, _type: PhantomData }
            }

            /// True once registration assigned this handle.
            pub fn is_registered(&self) -> bool {
                self.index != UNSET
            }
        }

        // Written by hand: `#[derive(Clone, Copy)]` would require `T: Copy`,
        // but a handle is just an index whatever `T` is.
        impl<T> Clone for $name<T> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl<T> Copy for $name<T> {}

        impl<T> PartialEq for $name<T> {
            fn eq(&self, other: &Self) -> bool {
                self.index == other.index
            }
        }

        impl<T> Eq for $name<T> {}

        /// An unregistered handle, so block structs can `#[derive(Default)]`
        /// and fill their handles in `register`.
        impl<T> Default for $name<T> {
            fn default() -> Self {
                Self::new(UNSET)
            }
        }

        impl<T> fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}<{}>({})", stringify!($name), std::any::type_name::<T>(), self.index)
            }
        }
    };
}

handle!(
    /// An input port carrying `T`. A small `Copy` handle: the data lives in
    /// the workstream and is read with [`Io::read`].
    In
);
handle!(
    /// An output port carrying `T`, written with [`Io::write`].
    Out
);
handle!(
    /// A child block of type `T`, as returned by [`Registrar::child`]. In
    /// `link`, `&l[child]` gives the child itself, to read its handles.
    Child
);

/// Either kind of port, for [`Registrar::private`] and [`Registrar::alias`].
pub trait PortHandle {
    #[doc(hidden)]
    fn port_index(&self) -> u32;
}

impl<T> PortHandle for In<T> {
    fn port_index(&self) -> u32 {
        self.index
    }
}

impl<T> PortHandle for Out<T> {
    fn port_index(&self) -> u32 {
        self.index
    }
}
