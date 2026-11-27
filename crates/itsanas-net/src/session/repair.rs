//! Finding chunks this device lost and fetching them back from a holder.

use itsanas_crypto::ChunkId;
use itsanas_store::Store;

use crate::{
    error::{NetError, Result},
    transport::PeerClient,
};

/// How many live chunks one repair round samples for local loss.
///
/// A bounded walk from a fresh random start, because at a terabyte there are
/// fourteen million and stat-ing all of them every round would cost far more
/// than the loss it is looking for.
///
/// # What this actually guarantees, which is less than it sounds
///
/// Each round examines 2048 chunks out of `N`, so the chance a given chunk is
/// looked at is `2048/N`, and reaching a nine-in-ten chance of having examined
/// one particular chunk takes:
///
/// | live chunks | rounds | at the five-minute service beat |
/// | --- | --- | --- |
/// | 2 000 | 1 | five minutes |
/// | 100 000 (~10 GB) | 111 | nine hours |
/// | 14 000 000 (1 TB) | 15 739 | **fifty-five days** |
///
/// An earlier version of this comment said "covers a household store in a
/// handful of rounds", which is wrong by a factor of twenty on the middle row
/// and by three orders of magnitude on the row it cites as the reason for the
/// bound in the first place.
///
/// Fifty-five days is defensible on its own terms — a block silently lost is
/// not an emergency until somebody reads the file — but only because it is not
/// the only detector. `doctor` finds every loss in one pass and now writes what
/// it finds where repair will drain it first. The sampling scan is the
/// background sweep for losses nobody has noticed yet.
pub const REPAIR_SCAN_PER_ROUND: usize = 2_048;

/// How many losses are considered for each one that can be asked about.
///
/// A loss is only worth raising with a peer the ledger records as holding it,
/// so most of the window is skipped. Eight times the fetch budget means a round
/// still finds work when seven in eight of the losses in view belong to peers
/// that are not this one.
const LOSS_WINDOW: usize = 8;

/// How many lost chunks one repair round tries to fetch back.
///
/// Small, because the peer is doing this for free and a node that has just lost
/// a disk would otherwise open with a demand for its entire store. Losing a
/// whole disk is a restore, not a repair; this is for the handful of blocks a
/// filesystem quietly drops.
pub const REPAIR_FETCH_PER_ROUND: usize = 32;

/// What one repair round found and fixed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// Chunks this node's files need and whose bytes are not on this disk.
    ///
    /// Everything known, not only what this round sampled.
    pub lost: usize,
    /// Chunks asked of this peer, because its own record says it holds them.
    pub asked: usize,
    /// Chunks that came back, opened, and re-addressed to what was asked for.
    pub restored: usize,
    /// Chunks that came back as something else.
    ///
    /// Not a transport error. A peer that answers a repair request with bytes
    /// that do not open is either broken or trying to turn a recoverable loss
    /// into a permanent one, and either way its record for that chunk is
    /// withdrawn.
    pub forged: usize,
    /// Losses this peer is not recorded as holding, so it was not told about.
    pub not_asked: usize,
}

impl RepairReport {
    /// Whether this round did anything worth printing.
    #[must_use]
    pub const fn changed_anything(&self) -> bool {
        self.restored > 0 || self.forged > 0
    }
}

/// Fetch back chunks this node has lost, from a peer that already holds them.
///
/// # Why this is not the same as pushing
///
/// `push` restores *replication*: it offers a peer everything the peer lacks.
/// It cannot restore anything to this disk, and a chunk missing from this disk
/// is the one failure the placement ledger was built to survive. A file becomes
/// unreadable, `doctor` reports it, and until this existed the only cure was a
/// human noticing.
///
/// A disk drops a block, a filesystem loses an inode after a power cut, a
/// backup restore comes back partial. The chunk is still on three other
/// machines and its address is still in the index; nothing was reaching for it.
///
/// # A repair request is a disclosure, and that decides who may be asked
///
/// Asking a peer "do you have chunk X?" tells it that this node does not. The
/// ids are blinded, so it learns nothing about the *content* — but it learns
/// which chunks now exist only on hosts, which is precisely the list to delete
/// if you want to destroy somebody's data. A healing mechanism that publishes
/// the map of the wounds.
///
/// The first version asked every peer it connected to about everything it had
/// lost, including strangers the local-discovery loop had dialled for the first
/// time that round.
///
/// So a peer is asked **only about chunks the ledger already records it as
/// holding**, and a chunk no peer is recorded as holding is asked of nobody:
/// nobody has it, so the question buys nothing and costs the answer.
///
/// What that rule buys, stated honestly rather than generously. It does not
/// reduce the disclosure to nothing. The peer knew "I was given X"; it now
/// learns "the owner no longer has X", which is new, and if the ledger names it
/// as the only holder it has just learned it is the last copy. That is the
/// short list of what it could destroy, and the rule does not remove it.
///
/// What the rule does is cut the audience from *every peer this node dials* —
/// strangers the local-discovery loop met thirty seconds ago included — down
/// to the peers already holding the chunk, which is the smallest audience that
/// can repair it at all. Anything smaller repairs nothing.
///
/// One channel is left open and is worth naming rather than discovering later:
/// **silence**. A host holding a thousand chunks that is never asked about any
/// of them learns the owner has lost none of them. Cheap to observe, hard to
/// close without asking for chunks that are not missing, and the cost of that
/// cover traffic is the bandwidth this whole subsystem exists to conserve.
///
/// # What comes back is verified before it is written
///
/// See [`Store::accept_chunk`]. Accepting unverified bytes would set
/// `has_chunk`, stop the scan looking, and turn a recoverable loss into a
/// permanent one.
pub fn repair(store: &Store, client: &mut PeerClient, scan: usize) -> Result<RepairReport> {
    let owner = store.owner();
    let peer = client.peer_device();
    let mut report = RepairReport::default();

    // Sample a slice of the live set, which records anything newly missing.
    // Its return value is deliberately ignored: what matters is the queue it
    // maintains, which also holds everything `doctor` found in one pass.
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw)
        .map_err(|error| NetError::Refused(format!("could not draw a repair cursor: {error}")))?;
    let _ = store.missing_locally(&ChunkId::from_bytes(raw), scan)?;

    // A bounded slice from a moving start, not the whole queue from the top.
    // A node that lost a disk has millions of these, and a run of losses no
    // reachable peer holds would otherwise sit at the front of the key order
    // for ever and starve everything behind it.
    let losses = store.known_losses_from(
        &ChunkId::from_bytes(raw),
        REPAIR_FETCH_PER_ROUND * LOSS_WINDOW,
    )?;
    report.lost = usize::try_from(store.loss_count()?).unwrap_or(usize::MAX);

    for address in losses {
        if report.asked == REPAIR_FETCH_PER_ROUND {
            break;
        }

        // The disclosure test, and the only one: has this peer already told
        // this node it holds this chunk?
        let holds = store
            .remote_holders(&address)?
            .iter()
            .any(|holder| holder.device == peer);
        if !holds {
            report.not_asked += 1;
            continue;
        }

        report.asked += 1;
        let Some(sealed) = client.chunk(owner, address)? else {
            continue;
        };
        if store.accept_chunk(&address, &sealed)? {
            report.restored += 1;
        } else {
            report.forged += 1;
            // The peer said it held this and produced something else. Whether
            // that is corruption or malice, the record is not evidence of
            // anything any more.
            store.forget_holder(&address, &peer)?;
        }
    }

    Ok(report)
}
