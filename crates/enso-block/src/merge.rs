//! Fan-in: what an input does when several outputs are linked to it.

use std::any::Any;

use enso_core::{MidiEvent, Timestamp};

/// How an input combines several links. Declared per input at
/// registration (it may come from configuration).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MergeMode {
    /// At most one link; a second is a link error. The default, and the only
    /// mode for types without [`Merge`].
    #[default]
    Single,
    /// Concatenate the sources' messages; timestamped types sort by time
    /// (stably: equal times keep link order).
    Append,
    /// Add the signals together.
    Sum,
}

/// Port types that can merge several sources into one input.
///
/// Merging runs on the workstream's thread before the block runs, so on a
/// real-time thread it must not allocate: reserve capacity in the input's
/// default value (e.g. `Vec::with_capacity`).
pub trait Merge: Send + 'static {
    /// Whether `mode` makes sense for this type (`Single` always does).
    fn supports(mode: MergeMode) -> bool;
    /// Empty the value before sources are merged in.
    fn clear(&mut self);
    /// Merge `other` in.
    fn merge(&mut self, other: &Self, mode: MergeMode);
}

macro_rules! summable {
    ($($t:ty),*) => {$(
        impl Merge for $t {
            fn supports(mode: MergeMode) -> bool {
                matches!(mode, MergeMode::Single | MergeMode::Sum)
            }
            fn clear(&mut self) {
                *self = Default::default();
            }
            fn merge(&mut self, other: &Self, _mode: MergeMode) {
                *self += *other;
            }
        }
    )*};
}
summable!(f32, f64, i32, i64);

/// Plain lists append in link order. (Allocates if the input's capacity
/// runs out; give its default value room.)
impl<T: Clone + Send + 'static> Merge for Vec<T> {
    fn supports(mode: MergeMode) -> bool {
        matches!(mode, MergeMode::Single | MergeMode::Append)
    }
    fn clear(&mut self) {
        Vec::clear(self);
    }
    fn merge(&mut self, other: &Self, _mode: MergeMode) {
        self.extend_from_slice(other);
    }
}

/// Something that happens at a point in time.
pub trait Stamped {
    fn timestamp(&self) -> Timestamp;
}

impl Stamped for MidiEvent {
    fn timestamp(&self) -> Timestamp {
        self.time
    }
}

/// A list of timestamped messages, kept in time order. Appending merges
/// sort stably by timestamp without allocating (as long as capacity lasts),
/// as decision 19 requires.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Events<T> {
    items: Vec<T>,
}

impl<T> Events<T> {
    /// Room for `capacity` events before pushing allocates.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { items: Vec::with_capacity(capacity) }
    }

    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Room left before pushing would allocate.
    pub fn spare_capacity(&self) -> usize {
        self.items.capacity() - self.items.len()
    }
}

impl<T: Stamped> Events<T> {
    /// Insert in time order (after events with the same timestamp). Never
    /// allocates: when the list is at capacity, the event is dropped and
    /// `false` returned (so it's safe on a real-time thread).
    pub fn push(&mut self, event: T) -> bool {
        if self.items.len() == self.items.capacity() {
            return false;
        }
        let at = self.items.partition_point(|e| e.timestamp() <= event.timestamp());
        self.items.insert(at, event);
        true
    }
}

impl<T: Stamped + Clone + Send + 'static> Merge for Events<T> {
    fn supports(mode: MergeMode) -> bool {
        matches!(mode, MergeMode::Single | MergeMode::Append)
    }
    fn clear(&mut self) {
        self.items.clear();
    }
    fn merge(&mut self, other: &Self, _mode: MergeMode) {
        for event in &other.items {
            let _ = self.push(event.clone());
        }
    }
}

// ---- type-erased forms, stored per input by the workstream ------------------------

pub(crate) type ClearFn = fn(&mut dyn Any);
pub(crate) type MergeFn = fn(&mut dyn Any, &dyn Any, MergeMode);

pub(crate) fn clear_erased<T: Merge>(value: &mut dyn Any) {
    if let Some(value) = value.downcast_mut::<T>() {
        value.clear();
    }
}

pub(crate) fn merge_erased<T: Merge>(into: &mut dyn Any, from: &dyn Any, mode: MergeMode) {
    if let (Some(into), Some(from)) = (into.downcast_mut::<T>(), from.downcast_ref::<T>()) {
        into.merge(from, mode);
    }
}
