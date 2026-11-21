//! Identity (design §0): what blocks and interfaces *are*, as opposed to
//! what they're *called*.
//!
//! - [`BlockId`]: a UUID per block instance.
//! - [`Key`]: identifies an interface (or a child) within its block.
//! - [`InterfaceId`]: `(BlockId, Key)`, an interface anywhere.
//! - [`Spec`]: what registration takes: a key plus a display label.
//!
//! Labels are for people (error messages, the UI) and are never looked up.

use std::{
    borrow::Cow,
    fmt,
    sync::atomic::{AtomicU32, Ordering},
};

use uuid::Uuid;

/// A block instance's identity: a UUID.
///
/// Blocks built from code get **deterministic** IDs: a child's ID is derived
/// from its parent's ID and its key ([`child`](Self::child)), so the same
/// tree has the same IDs on every run, and anything saved against them stays
/// valid. Blocks created from data carry the IDs stored with them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(Uuid);

impl BlockId {
    /// A fresh random ID (UUID v4).
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub const fn uuid(self) -> Uuid {
        self.0
    }

    /// The ID of this block's child registered under `key`: a UUID v5
    /// (a SHA-1 hash) of the key in this block's namespace.
    pub fn child(self, key: Key) -> Self {
        Self(Uuid::new_v5(&self.0, &key.0.to_le_bytes()))
    }
}

impl fmt::Debug for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlockId({})", self.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Identifies an interface or a child within its block. Unique per block.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(u64);

impl Key {
    /// An explicit key (`#[input(id = 7)]`): survives renaming the field.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The default key of a field called `name`: a stable 64-bit hash
    /// (FNV-1a) of the name. `const`, so derived keys cost nothing at run
    /// time. Renaming the field changes the key; pin it with `Key::new` to
    /// rename freely.
    pub const fn named(name: &str) -> Self {
        let bytes = name.as_bytes();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            i += 1;
        }
        Self(hash)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({:#x})", self.0)
    }
}

/// An interface anywhere: the block that declares it, and its key there.
/// How data (patch files, the editor) refers to ports and mailboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InterfaceId {
    pub block: BlockId,
    pub key: Key,
}

/// What registering a port, mailbox or child takes: an identifying key and
/// a display label.
///
/// `"cutoff".into()` gives `Key::named("cutoff")` labelled `cutoff`, the
/// same as the derive gives a field called `cutoff`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub key: Key,
    pub label: Cow<'static, str>,
}

impl Spec {
    pub fn new(key: Key, label: impl Into<Cow<'static, str>>) -> Self {
        Self { key, label: label.into() }
    }
}

impl From<&str> for Spec {
    fn from(name: &str) -> Self {
        Self { key: Key::named(name), label: Cow::Owned(name.to_owned()) }
    }
}

impl From<String> for Spec {
    fn from(name: String) -> Self {
        Self { key: Key::named(&name), label: Cow::Owned(name) }
    }
}

/// Identifies a workstream within an [`Engine`](crate::Engine) (returned by
/// [`Engine::add`](crate::Engine::add)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkstreamId(pub(crate) u32);

/// Every build gets a fresh tree number; handles carry it, so a handle used
/// with the wrong workstream is caught.
pub(crate) fn next_tree() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_from_names_are_stable() {
        // FNV-1a test vectors.
        assert_eq!(Key::named("").get(), 0xcbf2_9ce4_8422_2325);
        assert_eq!(Key::named("a").get(), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(Key::named("cutoff"), Key::named("resonance"));
    }

    #[test]
    fn child_ids_are_deterministic() {
        let parent = BlockId::from_uuid(Uuid::from_u128(42));
        assert_eq!(parent.child(Key::named("osc")), parent.child(Key::named("osc")));
        assert_ne!(parent.child(Key::named("osc")), parent.child(Key::named("filter")));
        assert_ne!(parent.child(Key::named("osc")), BlockId::random().child(Key::named("osc")));
    }
}
