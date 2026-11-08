//! Built-in nodes.

mod test_synth;

pub use test_synth::TestSynth;

use crate::{Node, Process};

/// A closure as a node, for tests and quick experiments:
/// `FnNode(|p: Process| p.output[0].fill(0.1))`. Same rules as any node:
/// no allocation inside.
pub struct FnNode<F>(pub F);

impl<F: FnMut(Process<'_>) + Send + 'static> Node for FnNode<F> {
    fn process(&mut self, process: Process<'_>) {
        (self.0)(process)
    }
}
