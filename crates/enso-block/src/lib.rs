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
//! use enso_block::{Block, Child, In, Io, LinkError, Linker, Out, Ports, Workstream};
//!
//! #[derive(Ports, Default)]
//! struct Constant {
//!     value: f32,
//!     #[output(initial = self.value)]
//!     out: Out<f32>,
//! }
//! impl Block for Constant {}
//!
//! #[derive(Ports, Default)]
//! struct Double {
//!     #[input(name = "in")]
//!     input: In<f32>,
//!     #[output]
//!     out: Out<f32>,
//! }
//! impl Block for Double {
//!     fn process(&mut self, io: &Io) {
//!         *io.write(self.out) = 2.0 * *io.read(self.input);
//!     }
//! }
//!
//! #[derive(Ports, Default)]
//! struct Patch {
//!     #[child(init = Constant { value: 21.0, ..Default::default() })]
//!     source: Child<Constant>,
//!     #[child]
//!     double: Child<Double>,
//! }
//! impl Block for Patch {
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
mod engine;
mod mail;
mod merge;
mod workstream;

pub use block::{Block, BlockError, Child, In, Inbox, Out, Outbox, PortHandle, Ports};
pub use engine::{Engine, EngineError};
pub use mail::{Delivery, MailReceiver, MailSender, Message, RealtimeMessage};
/// `#[derive(Ports)]`; see the [`Ports`](trait@Ports) trait. A derive macro
/// and a trait may share a name: they live in different namespaces (like
/// serde's `Serialize`).
pub use enso_block_derive::Ports;
pub use build::{BuildError, Interface, InterfaceKind, LinkError, Linker, LoadContext, Registrar};
pub use merge::{Events, Merge, MergeMode, Stamped};
pub use workstream::{AccessError, Io, Receive, Workstream};
