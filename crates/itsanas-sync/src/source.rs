//! Where a device gets chunks it does not have.
//!
//! The sync engine decides *what* should exist; this trait decides *where the
//! bytes come from*. Keeping the two apart is what lets the convergence tests
//! run without a network: the simulation implements this over an in-memory bag
//! of sealed chunks, and M4 will implement it over QUIC, with no change to the
//! merge logic in between.
//!
//! Every implementation deals exclusively in sealed bytes. A chunk source never
//! holds a key and never sees a plaintext, which is precisely what makes it
//! safe for the thing on the other end to be a stranger's machine.

use std::cell::RefCell;
use std::collections::BTreeSet;

use itsanas_crypto::{ChunkId, UserId};

use crate::error::Result;

/// Somewhere sealed chunks can be fetched from.
pub trait ChunkSource {
    /// Fetch the sealed bytes for `address`, belonging to `owner`.
    ///
    /// `Ok(None)` means "nowhere reachable has this right now", which is an
    /// ordinary and expected state — the device holding it is probably asleep.
    /// Reserve `Err` for a host that misbehaved.
    fn fetch(&self, owner: UserId, address: &ChunkId) -> Result<Option<Vec<u8>>>;
}

/// A source that has nothing.
///
/// Useful for exercising the deferred path: applying an operation whose chunks
/// are unavailable must leave local state untouched and ask to be retried,
/// rather than materialising a file whose content cannot be read.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmptySource;

/// `inner`, asked at most once for each address in one application of
/// segments ([`crate::apply_replaying`]): whatever the first ask brought, a
/// second cannot bring more. Bytes that did not open under their address, or
/// nothing at all, are not asked for again for the next file that shares the
/// chunk; a host serving noise for a chunk many files share used to cost one
/// chunk-sized download per file per round (ROADMAP, the integrity pass's
/// residues).
pub(crate) struct AskedOnce<'a> {
    inner: &'a dyn ChunkSource,
    asked: RefCell<BTreeSet<ChunkId>>,
}

impl<'a> AskedOnce<'a> {
    pub(crate) fn new(inner: &'a dyn ChunkSource) -> Self {
        Self {
            inner,
            asked: RefCell::new(BTreeSet::new()),
        }
    }
}

impl ChunkSource for AskedOnce<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> Result<Option<Vec<u8>>> {
        if !self.asked.borrow_mut().insert(*address) {
            return Ok(None);
        }
        self.inner.fetch(owner, address)
    }
}

impl ChunkSource for EmptySource {
    fn fetch(&self, _owner: UserId, _address: &ChunkId) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

impl<T: ChunkSource + ?Sized> ChunkSource for &T {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> Result<Option<Vec<u8>>> {
        (**self).fetch(owner, address)
    }
}
