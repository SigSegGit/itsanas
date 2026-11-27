//! Taking chunks a peer wants hosted, within this node's pledge.

use itsanas_store::{Store, Vault};

use crate::{error::Result, service::Pledge, transport::PeerClient};

/// What one round of hosting for a peer did./// What one round of hosting for a peer did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostReport {
    /// How many chunks the peer asked this node to hold.
    pub wanted: usize,
    /// How many were taken into this node's vault.
    pub taken: usize,
    /// How many were already held, so nothing was transferred.
    pub already_held: usize,
    pub bytes_taken: u64,
    /// Whether the pledge, not the peer, is what limited this round.
    pub pledge_full: bool,
    /// Why the per-account bound (§8 1c) stopped this round taking more, if
    /// it did.
    pub bounded: Option<String>,
}

impl HostReport {
    #[must_use]
    pub const fn changed_anything(&self) -> bool {
        self.taken > 0
    }
}

/// Take on some of a peer's data, over the connection this node dialled.
///
/// # The reason this exists
///
/// Every other exchange here runs one way. `push` offers this node's work and
/// the peer stores it; `pull` fetches what the peer holds and merges it. Both
/// are initiated by whoever dialled, and in both of them the *dialled* side is
/// the one that ends up hosting. So a member behind a router they do not
/// control could have their data held by others and could hold nothing in
/// return -- which in a system built on mutual storage is not an inconvenience,
/// it is being unable to keep your half of the bargain.
///
/// Nothing about networks required that. It was a question nobody asked. This
/// asks it: *have you anything you would like me to hold?* One reachable side
/// per pair is now enough for hosting to go both ways, and the reachable side
/// can be the one machine somebody has a port forwarded to.
///
/// # What bounds it
///
/// The peer names what it wants, and this node decides what it takes. The
/// pledge is checked here rather than trusted to the peer: a peer that asked
/// for a terabyte gets what this node offered the network and not a byte more.
///
/// # Errors
///
/// If the peer refuses, or the vault cannot be written.
/// How many chunks one round offers to take on.
///
/// A page rather than everything: a round that took an unbounded amount would
/// make one sync unpredictable in length, and the next round is never far away.
const PER_ROUND: u32 = 32;

pub fn host_for(vault: &Vault, client: &mut PeerClient, pledge: Pledge) -> Result<HostReport> {
    host_for_bounded(vault, client, pledge, None)
}

/// [`host_for`], with the per-account bound a host applies to what it is
/// offered (§8 1c) applied to what it pulls as well.
///
/// Found by the redteam agent: the bound sat on `StoreChunk` and
/// `StoreSegment`, and this path -- the peer naming the owner and the chunks,
/// this node fetching them into its vault -- reached `put_chunk` checked only
/// against this node's own pledge. A client refused on push offered the same
/// chunks here instead. The peer is the device this connection proved, and it
/// is held to the claim it presented when it last dialled this node; one that
/// never did is not hosted for.
///
/// # Errors
///
/// As [`host_for`].
pub fn host_for_bounded(
    vault: &Vault,
    client: &mut PeerClient,
    pledge: Pledge,
    bound: Option<(&dyn crate::Owners, &Store)>,
) -> Result<HostReport> {
    let mut report = HostReport::default();

    // The same total the host's `StoreChunk` check reads, so both paths
    // refuse at the same byte, without walking the vault each round.
    let held = vault.held_bytes()?;
    if held >= pledge.bytes {
        report.pledge_full = true;
        return Ok(report);
    }

    let (owner, wanted) = client.want_hosted(PER_ROUND)?;
    report.wanted = wanted.len();

    let mut room = pledge.bytes.saturating_sub(held);
    let mut taken = Vec::new();

    for address in wanted {
        if vault.has_chunk(owner, &address)? {
            report.already_held += 1;
            // Still worth telling them: a holder they have forgotten about is
            // a copy they think they do not have.
            taken.push(address);
            continue;
        }

        let Some(sealed) = client.chunk(owner, address)? else {
            // The peer asked for this to be held and then would not hand it
            // over. Not an error -- it may have been collected between the two
            // messages -- and not this node's problem to solve.
            continue;
        };

        let size = sealed.len() as u64;
        if size > room {
            report.pledge_full = true;
            break;
        }
        if let Some((owners, store)) = bound
            && let Err(why) = owners.admits(
                client.peer_device(),
                owner,
                size,
                store,
                vault,
                pledge.bytes,
            )
        {
            report.bounded = Some(why);
            break;
        }

        if vault.put_chunk(owner, &address, &sealed)? {
            room = room.saturating_sub(size);
            report.taken += 1;
            report.bytes_taken = report.bytes_taken.saturating_add(size);
        }
        taken.push(address);
    }

    if !taken.is_empty() {
        // The owner keeps the placement ledger, so it has to be told. It is a
        // claim, and the owner's storage challenges are what turn it into
        // evidence -- the same treatment a chunk pushed the other way gets.
        client.hosted(taken)?;
    }

    Ok(report)
}
