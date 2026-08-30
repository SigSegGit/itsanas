//! Which devices are known to hold each chunk.
//!
//! # Why the owner records this instead of deriving it
//!
//! The original design had a coordinator publish a signed **node-set epoch** so
//! that every peer computed the same rendezvous placement "without an agreement
//! protocol". That phrasing hid the problem: requiring every peer to hold the
//! same membership list *is* an agreement protocol, run by decree.
//!
//! It is also unnecessary here, and noticing why is the point. A global content
//! store — IPFS, a DHT — has to answer "who holds this block?" for an arbitrary
//! asker, and that needs agreement about the keyspace. **ITSaNAS never asks that
//! question.** Every chunk belongs to exactly one user, who already keeps an
//! operation log of their own chunks. An owner who knows what they stored can
//! simply write down where they put it.
//!
//! So this table is the replacement for the node-set epoch, and it is strictly
//! smaller: no consensus, no signed membership, no epoch to pin, and nothing
//! that stops working when the coordinator does. [`docs/DESIGN.md`] §8 carries
//! the full argument.
//!
//! # The ledger is other people; this device is counted separately
//!
//! The table holds **remote** acknowledgements only. This device's own copy is
//! not in it, because it is not an acknowledgement — it is a file on this disk,
//! and the authority on whether it is there is the disk.
//!
//! That leaves one place where the two have to be added up, and it is named so
//! that nobody has to remember: [`Index::under_replicated`] takes a target that
//! **counts this device**, so a target of three asks for two remote holders. A
//! separate [`Index::remote_holders`] returns the ledger as it is.
//!
//! The alternative — an implicit `+1` scattered across call sites — is exactly
//! the unwritten rule that produces a repair loop quietly targeting one replica
//! too few, and being wrong in a direction nothing reports.
//!
//! # What a record means, and what it does not
//!
//! A record means: this device sent the chunk to that device, and that device
//! said it accepted it. It is evidence, not proof. A host that accepted a chunk
//! and then deleted it still has a record here until a storage challenge fails
//! and [`Index::forget_holder`] is called.
//!
//! That is the honest position and it is why challenges exist. A ledger of
//! acknowledgements is worth having anyway: without one there is nothing to
//! challenge, because nothing knows who to ask.
//!
//! [`docs/DESIGN.md`]: https://github.com/SigSeg/itsanas/blob/main/docs/DESIGN.md

use itsanas_crypto::{ChunkId, DeviceId, ID_LEN};

#[allow(unused_imports)] // Referenced only from documentation links.
use crate::index::Index;

/// Bytes in a holder key: the chunk, then the device.
pub const HOLDER_KEY_LEN: usize = ID_LEN * 2;

/// The composite key for one (chunk, device) pair.
///
/// Chunk first so that every holder of one chunk is a contiguous range, which
/// is what makes "who holds this?" a range scan rather than a full table walk.
#[must_use]
pub fn key(chunk: &ChunkId, device: &DeviceId) -> [u8; HOLDER_KEY_LEN] {
    let mut out = [0u8; HOLDER_KEY_LEN];
    out[..ID_LEN].copy_from_slice(chunk.as_bytes());
    out[ID_LEN..].copy_from_slice(device.as_bytes());
    out
}

/// The lowest key that can belong to `chunk`.
#[must_use]
pub fn range_start(chunk: &ChunkId) -> [u8; HOLDER_KEY_LEN] {
    key(chunk, &DeviceId::from_bytes([0x00; ID_LEN]))
}

/// The highest key that can belong to `chunk`.
#[must_use]
pub fn range_end(chunk: &ChunkId) -> [u8; HOLDER_KEY_LEN] {
    key(chunk, &DeviceId::from_bytes([0xff; ID_LEN]))
}

/// The same pair, keyed the other way round: device first.
///
/// Two orderings of one fact, written in the same transaction, because the two
/// questions asked of it have opposite shapes. "Who holds this chunk?" is a
/// range scan under the chunk; "what does this peer hold, least recently
/// confirmed?" is a range scan under the device. With one ordering the other
/// question is a full table scan — which at a terabyte is fourteen million rows
/// walked every audit round, on a Raspberry Pi.
///
/// Denormalised state usually drifts, and that is the objection this project
/// answers by deriving instead. It cannot drift here: both keys are written and
/// removed inside the same redb transaction, so there is no window in which one
/// exists without the other.
#[must_use]
pub fn by_device(device: &DeviceId, chunk: &ChunkId) -> [u8; HOLDER_KEY_LEN] {
    let mut out = [0u8; HOLDER_KEY_LEN];
    out[..ID_LEN].copy_from_slice(device.as_bytes());
    out[ID_LEN..].copy_from_slice(chunk.as_bytes());
    out
}

/// The lowest device-first key that can belong to `device`.
#[must_use]
pub fn device_range_start(device: &DeviceId) -> [u8; HOLDER_KEY_LEN] {
    by_device(device, &ChunkId::from_bytes([0x00; ID_LEN]))
}

/// The highest device-first key that can belong to `device`.
#[must_use]
pub fn device_range_end(device: &DeviceId) -> [u8; HOLDER_KEY_LEN] {
    by_device(device, &ChunkId::from_bytes([0xff; ID_LEN]))
}

/// Split a device-first key back into its two halves.
#[must_use]
pub fn split_by_device(bytes: &[u8]) -> Option<(DeviceId, ChunkId)> {
    if bytes.len() != HOLDER_KEY_LEN {
        return None;
    }
    let mut device = [0u8; ID_LEN];
    device.copy_from_slice(&bytes[..ID_LEN]);
    let mut chunk = [0u8; ID_LEN];
    chunk.copy_from_slice(&bytes[ID_LEN..]);
    Some((DeviceId::from_bytes(device), ChunkId::from_bytes(chunk)))
}

/// Split a stored key back into its two halves.
///
/// Returns `None` for a key of the wrong length, which can only mean the table
/// was written by something other than this module.
#[must_use]
pub fn split(bytes: &[u8]) -> Option<(ChunkId, DeviceId)> {
    if bytes.len() != HOLDER_KEY_LEN {
        return None;
    }
    let mut chunk = [0u8; ID_LEN];
    chunk.copy_from_slice(&bytes[..ID_LEN]);
    let mut device = [0u8; ID_LEN];
    device.copy_from_slice(&bytes[ID_LEN..]);
    Some((ChunkId::from_bytes(chunk), DeviceId::from_bytes(device)))
}

/// One device known to hold a chunk, and when that was last confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holder {
    /// The device that acknowledged holding it.
    pub device: DeviceId,
    /// This node's clock when the acknowledgement arrived.
    ///
    /// Our own clock, not the peer's: a host's opinion of the time is not
    /// evidence, and this is used to decide which acknowledgements are stale
    /// enough to be worth re-checking.
    pub confirmed_unix: u64,
}

/// A chunk that fewer devices hold than it should.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtRisk {
    /// The chunk.
    pub chunk: ChunkId,
    /// How many devices hold it, **this one included**.
    ///
    /// One means it exists nowhere else, which is the condition worth an alert
    /// rather than a background repair.
    pub held_by: usize,
    /// How many should hold it, on the same counting.
    pub target: usize,
}

impl AtRisk {
    /// How many more copies are needed.
    #[must_use]
    pub const fn shortfall(&self) -> usize {
        self.target.saturating_sub(self.held_by)
    }

    /// Whether this chunk exists nowhere but here.
    ///
    /// The condition worth waking somebody for: one disk failure from gone.
    #[must_use]
    pub const fn only_copy(&self) -> bool {
        self.held_by <= 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; ID_LEN])
    }

    fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; ID_LEN])
    }

    #[test]
    fn a_key_round_trips_through_its_two_halves() {
        let (c, d) = split(&key(&chunk(3), &device(9))).unwrap();
        assert_eq!(c, chunk(3));
        assert_eq!(d, device(9));
    }

    #[test]
    fn every_holder_of_one_chunk_sorts_together() {
        // The whole reason the chunk comes first. If the device sorted first,
        // answering "who holds this chunk?" would walk the entire table, which
        // on a Raspberry Pi with a million chunks is the difference between a
        // repair pass that finishes and one that does not.
        let start = range_start(&chunk(5));
        let end = range_end(&chunk(5));

        for byte in 0..=255u8 {
            let k = key(&chunk(5), &device(byte));
            assert!(
                k >= start && k <= end,
                "device {byte} fell outside the range"
            );
        }

        assert!(key(&chunk(4), &device(255)) < start);
        assert!(key(&chunk(6), &device(0)) > end);
    }

    #[test]
    fn everything_one_device_holds_sorts_together_in_the_other_ordering() {
        // The reason the second ordering exists. Without it, "what does this
        // peer hold?" walks every row for every peer on every audit round —
        // fourteen million of them at a terabyte, on a Raspberry Pi.
        let start = device_range_start(&device(5));
        let end = device_range_end(&device(5));

        for byte in 0..=255u8 {
            let k = by_device(&device(5), &chunk(byte));
            assert!(
                k >= start && k <= end,
                "chunk {byte} fell outside the range"
            );
        }

        assert!(by_device(&device(4), &chunk(255)) < start);
        assert!(by_device(&device(6), &chunk(0)) > end);
    }

    #[test]
    fn the_two_orderings_describe_the_same_pair() {
        let (c, d) = split(&key(&chunk(3), &device(9))).unwrap();
        let (d2, c2) = split_by_device(&by_device(&device(9), &chunk(3))).unwrap();
        assert_eq!((c, d), (c2, d2));
    }

    #[test]
    fn a_key_of_the_wrong_length_is_refused_rather_than_guessed_at() {
        assert!(split(&[]).is_none());
        assert!(split(&[0u8; HOLDER_KEY_LEN - 1]).is_none());
        assert!(split(&[0u8; HOLDER_KEY_LEN + 1]).is_none());
    }

    #[test]
    fn a_chunk_held_only_here_is_flagged_as_the_only_copy() {
        // This is the alert condition: everything else is a shortfall to work
        // through in the background, and this one is a disk failure away from
        // data loss.
        let alone = AtRisk {
            chunk: chunk(1),
            held_by: 1,
            target: 3,
        };
        assert!(alone.only_copy());
        assert_eq!(alone.shortfall(), 2);

        let paired = AtRisk {
            chunk: chunk(1),
            held_by: 2,
            target: 3,
        };
        assert!(!paired.only_copy());
        assert_eq!(paired.shortfall(), 1);
    }

    #[test]
    fn a_chunk_held_more_widely_than_its_target_has_no_shortfall() {
        // Saturating rather than wrapping: extra copies are a good thing, and
        // an underflow here would ask the repair loop for four billion pushes.
        let plenty = AtRisk {
            chunk: chunk(1),
            held_by: 5,
            target: 3,
        };
        assert_eq!(plenty.shortfall(), 0);
    }
}
