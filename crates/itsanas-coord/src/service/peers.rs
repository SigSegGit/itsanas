//! Whose addresses a caller is handed: an account's own devices, and other
//! accounts' machines offering space.

use itsanas_crypto::{DeviceId, UserId};

use super::{CoordService, PRESENCE_TTL};
use crate::{
    claim::{ClaimedPresence, Presence, SignedPresence},
    error::Result,
    protocol::{MAX_HOSTS_RETURNED, MAX_PEERS_RETURNED},
};

/// How many addresses one [`Request::Hosts`] reads: bounded whatever the size
/// of the network, and most of them are expected to offer something.
const HOSTS_SCANNED: usize = 256;

impl CoordService<'_> {
    /// [`Self::peers_of`] without the signatures, for `Peers`, which predates
    /// them.
    pub(super) fn bare_peers_of(&self, user: UserId, now: u64) -> Result<Vec<Presence>> {
        Ok(self
            .peers_of(user, now)?
            .into_iter()
            .map(|signed| signed.presence)
            .collect())
    }

    /// Live, reachable devices for `user`, most recently confirmed first.
    ///
    /// Ordered and expired by **this coordinator's** record of when it last
    /// heard from each device, never by the `at_unix` inside the presence. That
    /// number is the announcing device's opinion of the time: a Raspberry Pi
    /// with no real-time clock reports 1970 and would sort last forever, and
    /// anybody who wanted to sort first could simply say so. The same mistake
    /// was made and removed in `itsanas-discover`; it does not get to come back
    /// here.
    ///
    /// Each with the signature it was announced under, so `SignedPeers` can
    /// hand it on and the reader can check it.
    pub(super) fn peers_of(&self, user: UserId, now: u64) -> Result<Vec<SignedPresence>> {
        Ok(self
            .claimed_peers_of(user, now)?
            .into_iter()
            .map(|claimed| claimed.presence)
            .collect())
    }

    /// As [`Self::peers_of`], each with the live claim it was listed under.
    ///
    /// The claim is the one the list is built from, so a row cannot carry a
    /// claim other than the one that put it there.
    pub(super) fn claimed_peers_of(&self, user: UserId, now: u64) -> Result<Vec<ClaimedPresence>> {
        let mut out = Vec::new();
        for claim in self.directory.live_claims_of(user)? {
            let device = claim.claim.device;
            let Some(presence) = self.directory.presence_of(device)? else {
                continue;
            };
            let seen = self.directory.last_seen(device)?.unwrap_or(0);
            if now.saturating_sub(seen) > PRESENCE_TTL {
                continue;
            }
            out.push((seen, ClaimedPresence { presence, claim }));
        }

        // Then by device id, so two clients asking at the same moment get the
        // same list rather than a shuffling one.
        out.sort_by(|a, b| {
            b.0.cmp(&a.0).then(
                a.1.presence
                    .presence
                    .device
                    .cmp(&b.1.presence.presence.device),
            )
        });
        out.truncate(MAX_PEERS_RETURNED);
        Ok(out.into_iter().map(|(_, claimed)| claimed).collect())
    }

    /// What [`Request::Hosts`] tells `caller`: machines of other accounts that
    /// offer space and were heard lately. Nothing to a device with no claim.
    pub(super) fn hosts_for(&self, caller: DeviceId, now: u64) -> Result<Vec<ClaimedPresence>> {
        let Some(mine) = self.directory.claim_for(caller)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for presence in self.directory.presences_after(caller, HOSTS_SCANNED)? {
            let device = presence.presence.device;
            let Some(claim) = self.directory.claim_for(device)? else {
                continue;
            };
            let seen = self.directory.last_seen(device)?.unwrap_or(0);
            if claim.claim.revoked
                || claim.claim.pledged_bytes == 0
                || claim.claim.owner == mine.claim.owner
                || now.saturating_sub(seen) > PRESENCE_TTL
                || self.directory.is_suspended(claim.claim.owner)?
            {
                continue;
            }
            out.push(ClaimedPresence { presence, claim });
            if out.len() == MAX_HOSTS_RETURNED {
                break;
            }
        }
        Ok(out)
    }
}
