//! A full sync round against one peer.
//!
//! This is where the pieces meet: the merge rules from [`itsanas_sync`], the
//! local state from [`itsanas_store`], and a peer on the other end of a socket.
//!
//! A round is deliberately two independent halves:
//!
//! * **Push** — offer this device's segments and any chunks the peer lacks.
//! * **Pull** — fetch what the peer has from *other* devices and merge it.
//!
//! Either half can fail without poisoning the other, and neither is required
//! for the other to be useful. A device with nothing new still pulls; a device
//! whose peer is a pure host still pushes.
//!
//! # Why the pull half writes to the vault as well as the store
//!
//! Segments fetched from a peer are put in this node's vault before being
//! applied. That is not bookkeeping — it is what makes relaying work. Once the
//! laptop holds the Pi's segments, the laptop can serve them to the VM, and the
//! Pi never has to be online at the same time as the VM. It also gives the pull
//! a natural resume point: "everything after what my vault already holds",
//! which costs nothing to track and is correct after a crash.

use std::cell::RefCell;
use std::collections::BTreeSet;

use itsanas_crypto::{ChunkId, UserId};
use itsanas_store::{SegmentEnvelope, Store, Vault};
use itsanas_sync::{ChunkSource, SyncReport, apply_segments};

use crate::{
    error::{NetError, Result},
    protocol::{MAX_HAVE_BATCH, MAX_SEGMENTS_PER_REQUEST},
    transport::PeerClient,
};

/// What one push half did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PushReport {
    pub segments_offered: usize,
    pub segments_accepted: usize,
    pub chunks_offered: usize,
    pub chunks_accepted: usize,
    pub bytes_sent: u64,
    /// Chunks this peer is now known to hold, whether just sent or already had.
    ///
    /// Counted separately from `chunks_accepted` because the two answer
    /// different questions: how much work this round did, and how much of this
    /// node's data now exists somewhere other than this disk.
    pub holders_recorded: usize,
}

/// What a whole round did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoundReport {
    pub push: PushReport,
    pub pull: SyncReport,
}

impl RoundReport {
    /// Whether this round moved anything at all.
    #[must_use]
    pub const fn changed_anything(&self) -> bool {
        self.push.segments_accepted > 0
            || self.push.chunks_accepted > 0
            || self.pull.changed_anything()
    }
}

/// Fetches chunks from a peer on demand.
///
/// [`ChunkSource`] takes `&self` because the merge engine holds it immutably
/// while walking operations; the socket underneath needs `&mut`. The `RefCell`
/// bridges the two. It cannot deadlock: the engine never calls back into itself
/// while a fetch is outstanding, so the borrow is only ever held across one
/// request.
struct RemoteChunks<'a> {
    client: RefCell<&'a mut PeerClient>,
}

impl ChunkSource for RemoteChunks<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> itsanas_sync::Result<Option<Vec<u8>>> {
        self.client
            .borrow_mut()
            .chunk(owner, *address)
            // A peer that fails mid-fetch is a transport problem, not a merge
            // problem. Reporting it as "absent" would let a broken connection
            // masquerade as a device being asleep, and the operation would be
            // deferred forever instead of surfacing the fault.
            .map_err(|error| itsanas_sync::SyncError::Source(error.to_string()))
    }
}

/// Offer this device's segments and any chunks the peer lacks.
pub fn push(store: &Store, client: &mut PeerClient) -> Result<PushReport> {
    let owner = store.owner();
    let peer = client.peer_device();
    let mut report = PushReport::default();

    for envelope in store.segments()? {
        report.segments_offered += 1;
        if client.store_segment(&envelope)? {
            report.segments_accepted += 1;
            report.bytes_sent = report
                .bytes_sent
                .saturating_add(envelope.sealed_body.len() as u64);
        }
    }

    // Ask before sending. Re-uploading a hundred thousand chunks every round
    // because we never asked is the difference between a usable system and one
    // that saturates the link forever.
    let addresses = store.blobs().addresses()?;
    for batch in addresses.chunks(MAX_HAVE_BATCH) {
        let missing = client.missing_chunks(owner, batch.to_vec())?;
        let wanted: BTreeSet<ChunkId> = missing.iter().copied().collect();

        // What the peer did *not* ask for, it already has. That answer costs
        // nothing extra — it is the same round trip that decides what to send —
        // and it is what makes the placement ledger converge on every sync
        // rather than only recording chunks this node happened to upload. A
        // node restored from its recovery phrase learns where its data lives by
        // asking, instead of re-uploading everything to find out.
        let mut confirmed: Vec<ChunkId> = batch
            .iter()
            .filter(|address| !wanted.contains(address))
            .copied()
            .collect();

        for address in missing {
            let Some(sealed) = store.blobs().get(&address)? else {
                // Collected between listing and sending. Not an error.
                continue;
            };

            report.chunks_offered += 1;
            let len = sealed.len() as u64;
            if client.store_chunk(owner, address, sealed)? {
                report.chunks_accepted += 1;
                report.bytes_sent = report.bytes_sent.saturating_add(len);
                confirmed.push(address);
            }
        }

        report.holders_recorded += confirmed.len();
        store.record_holders(&confirmed, &peer)?;
    }

    Ok(report)
}

/// Fetch what the peer has from this user's *other* devices, and merge it.
pub fn pull(store: &Store, vault: &Vault, client: &mut PeerClient) -> Result<SyncReport> {
    let owner = store.owner();
    let mine = store.device_id();

    let heads = client.heads(owner)?;
    let mut fetched: Vec<SegmentEnvelope> = Vec::new();

    for head in heads {
        if head.device == mine {
            continue;
        }

        // Resume from whatever this node's vault already holds for that device.
        let local_head = vault
            .heads_for(owner)?
            .into_iter()
            .find(|(device, _, _)| *device == head.device)
            .map(|(_, head, _)| head);

        if local_head == Some(head.head) {
            // Already current with this device. Nothing to ask for.
            continue;
        }

        let segments = client.segments(owner, head.device, local_head, MAX_SEGMENTS_PER_REQUEST)?;

        for envelope in &segments {
            // Retained so this node can relay them onwards, and so the next
            // pull has a resume point. put_segment verifies the signature and
            // refuses a chain with a hole.
            vault.put_segment(envelope)?;
        }

        fetched.extend(segments);
    }

    if fetched.is_empty() {
        return Ok(SyncReport::default());
    }

    let source = RemoteChunks {
        client: RefCell::new(client),
    };
    let (report, _) = apply_segments(store, &fetched, &source)
        .map_err(|error| NetError::Refused(error.to_string()))?;

    Ok(report)
}

/// Push then pull.
pub fn round(store: &Store, vault: &Vault, client: &mut PeerClient) -> Result<RoundReport> {
    Ok(RoundReport {
        push: push(store, client)?,
        pull: pull(store, vault, client)?,
    })
}

/// Apply this user's own segments that peers have pushed into the vault.
///
/// A push puts segments and chunks into the *receiving* node's vault, where
/// they sit ready to be relayed. Nothing else picks them up: only [`pull`]
/// applies segments to the local store, and a node that never dials anybody
/// never pulls. Without this, a node that only ever accepts connections stays
/// permanently ignorant of the very data it is holding.
///
/// That is not a corner case. Any device behind NAT can push and cannot be
/// dialled, so for its peers this is the *only* way its work arrives.
///
/// Chunks come from the vault, not the network: a peer that pushed a segment
/// pushed the chunks with it, so nothing here needs anyone to be online.
pub fn drain_vault(store: &Store, vault: &Vault) -> Result<SyncReport> {
    let owner = store.owner();
    let mine = store.device_id();

    let mut segments = Vec::new();
    for (device, _, _) in vault.heads_for(owner)? {
        if device == mine {
            continue;
        }
        segments.extend(vault.segments_for(
            owner,
            device,
            None,
            usize::from(MAX_SEGMENTS_PER_REQUEST),
        )?);
    }

    if segments.is_empty() {
        return Ok(SyncReport::default());
    }

    let source = VaultChunks { vault, owner };
    let (report, _) = apply_segments(store, &segments, &source)
        .map_err(|error| NetError::Refused(error.to_string()))?;

    Ok(report)
}

/// Serves chunks out of the local vault.
struct VaultChunks<'a> {
    vault: &'a Vault,
    owner: UserId,
}

impl ChunkSource for VaultChunks<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> itsanas_sync::Result<Option<Vec<u8>>> {
        if owner != self.owner {
            return Ok(None);
        }
        self.vault
            .get_chunk(owner, address)
            .map_err(|error| itsanas_sync::SyncError::Source(error.to_string()))
    }
}
