//! The block compute platform. Design: `docs/design/blocks.md`.
//!
//! A [`Block`] declares ports ([`In`], [`Out`]) and children in
//! [`register`](Block::register), loads resources in [`load`](Block::load),
//! wires its children together in [`link`](Block::link), and transforms
//! inputs into outputs in [`process`](Block::process). A [`Workstream`]
//! builds a tree of blocks and runs it, step after step, every block after
//! the blocks it depends on.
//!
//! ```
//! use enso_block::{Block, In, Io, Linker, LinkError, Out, Registrar, Workstream, Child};
//!
//! #[derive(Default)]
//! struct Constant { value: f32, out: Out<f32> }
//! impl Block for Constant {
//!     fn register(&mut self, r: &mut Registrar) { self.out = r.output("out", self.value); }
//! }
//!
//! #[derive(Default)]
//! struct Double { input: In<f32>, out: Out<f32> }
//! impl Block for Double {
//!     fn register(&mut self, r: &mut Registrar) {
//!         self.input = r.input("in", 0.0);
//!         self.out = r.output("out", 0.0);
//!     }
//!     fn process(&mut self, io: &Io) {
//!         *io.write(self.out) = 2.0 * *io.read(self.input);
//!     }
//! }
//!
//! #[derive(Default)]
//! struct Patch { source: Child<Constant>, double: Child<Double> }
//! impl Block for Patch {
//!     fn register(&mut self, r: &mut Registrar) {
//!         self.source = r.child("source", Constant { value: 21.0, ..Default::default() });
//!         self.double = r.child("double", Double::default());
//!     }
//!     fn link(&mut self, l: &mut Linker) -> Result<(), LinkError> {
//!         l.connect(l[self.source].out, l[self.double].input)
//!     }
//! }
//!
//! let mut ws = Workstream::build(Patch::default()).unwrap();
//! ws.step();
//! assert_eq!(*ws.read::<f32>("double/out").unwrap(), 42.0);
//! ```

mod block;
mod build;
mod merge;
mod workstream;

pub use block::{Block, BlockError, Child, In, Out, PortHandle};
pub use build::{BuildError, Direction, Interface, LinkError, Linker, LoadContext, Registrar};
pub use merge::{Events, Merge, MergeMode, Stamped};
pub use workstream::{AccessError, Io, Workstream};
