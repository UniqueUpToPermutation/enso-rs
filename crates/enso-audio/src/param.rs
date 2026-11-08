//! Knobs that the control thread turns while the audio thread listens.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering::Relaxed},
};

/// One `f32` shared between threads, lock-free: an atomic holding the
/// float's bits. Clone it: the UI keeps one copy and calls [`set`](Self::set),
/// the node keeps another and calls [`get`](Self::get) once per block.
///
/// This is the simplest real-time-safe way to share a value: no locks, no
/// queue, latest value wins. (A general parameter system with automation and
/// MIDI learn will build on top of this.)
#[derive(Debug, Clone)]
pub struct SharedParam(Arc<AtomicU32>);

impl SharedParam {
    pub fn new(value: f32) -> Self {
        Self(Arc::new(AtomicU32::new(value.to_bits())))
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }

    pub fn set(&self, value: f32) {
        self.0.store(value.to_bits(), Relaxed);
    }
}

/// Glides towards a target instead of jumping. A parameter that jumps from
/// one block to the next makes an audible click ("zipper noise").
#[derive(Debug, Clone, Copy)]
pub struct Smoothed {
    value: f32,
    coef: f32,
}

impl Smoothed {
    /// Starts at `value`; covers ~63% of a change every `seconds`.
    pub fn new(value: f32, seconds: f32, sample_rate: f32) -> Self {
        Self { value, coef: (-1.0 / (seconds * sample_rate).max(1.0)).exp() }
    }

    /// Advance one sample towards `target`.
    pub fn next(&mut self, target: f32) -> f32 {
        self.value = target + (self.value - target) * self.coef;
        self.value
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}
