//! Application mode: the machines a machine in application mode dials first,
//! and every minute (HANDOVER §8 00).
//!
//! A machine in application mode backs up a program's data: small files,
//! written one at a time, each worth having on two other machines within
//! minutes. Two machines that both run in this mode are each other's
//! *privileged hosts*: each asks the other whether it runs in application mode
//! ([`itsanas_net::PeerClient::app_mode`]) and, if it does, dials it right
//! after the account's own machines in every full round, and alone in a light
//! round every [`crate::control::APP_INTERVAL`].
//!
//! What this module refuses to do:
//!
//! - **Remember a stranger.** Only a peer that did what a real host does this
//!   session, or a machine of this account, is noted (the caller decides; see
//!   `daemon::sync_once`). A device key costs nothing to mint, and a peer
//!   noted here is dialled every minute.
//! - **Grow.** At most [`MAX_APP_PEERS`] are kept; a new one displaces the one
//!   heard from longest ago.
//! - **Keep a peer that stopped saying so.** A peer that answers "off" leaves
//!   at once, and one not heard from in [`FORGET_AFTER`] leaves at the next
//!   look: a machine that went away is not dialled every minute for ever.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use itsanas_crypto::DeviceId;

/// Most privileged hosts a machine dials in a light round. Two copies
/// elsewhere is the goal (`SAFE_COPIES`); eight is room for the ones that are
/// off, and a bound on what a light round can cost.
pub const MAX_APP_PEERS: usize = 8;

/// How long a privileged host is kept without being heard from again. An hour
/// is twelve full rounds at the default interval: a machine that answered
/// none of them is not one to dial every minute.
pub const FORGET_AFTER: Duration = Duration::from_secs(3600);

/// Where a privileged host was last reached, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPeer {
    /// The address that reached it.
    pub address: String,
    /// The relay it was reached through, if any: its device and address.
    pub through: Option<(DeviceId, String)>,
    /// When it last said it runs in application mode.
    pub heard: Instant,
}

/// The privileged hosts this machine knows, bounded.
#[derive(Debug, Default)]
pub struct AppPeers {
    peers: BTreeMap<DeviceId, AppPeer>,
}

impl AppPeers {
    /// `device`, reached at `address` (through `through`), answered `on` at
    /// `now`.
    pub fn heard(
        &mut self,
        device: DeviceId,
        address: &str,
        through: Option<&(DeviceId, String)>,
        on: bool,
        now: Instant,
    ) {
        if !on {
            self.peers.remove(&device);
            return;
        }
        if !self.peers.contains_key(&device)
            && self.peers.len() >= MAX_APP_PEERS
            && let Some(oldest) = self
                .peers
                .iter()
                .min_by_key(|(_, peer)| peer.heard)
                .map(|(device, _)| *device)
        {
            self.peers.remove(&oldest);
        }
        self.peers.insert(
            device,
            AppPeer {
                address: address.to_owned(),
                through: through.cloned(),
                heard: now,
            },
        );
    }

    /// Whether `device` is one of them.
    #[must_use]
    pub fn knows(&self, device: &DeviceId) -> bool {
        self.peers.contains_key(device)
    }

    /// The privileged hosts to dial at `now`, the one heard from last first.
    /// Forgets the ones past [`FORGET_AFTER`].
    pub fn due(&mut self, now: Instant) -> Vec<(DeviceId, AppPeer)> {
        self.peers
            .retain(|_, peer| now.saturating_duration_since(peer.heard) < FORGET_AFTER);
        let mut due: Vec<(DeviceId, AppPeer)> = self
            .peers
            .iter()
            .map(|(device, peer)| (*device, peer.clone()))
            .collect();
        due.sort_by_key(|(_, peer)| std::cmp::Reverse(peer.heard));
        due
    }

    /// Whether a light round would dial anybody at `now`.
    #[must_use]
    pub fn any(&self, now: Instant) -> bool {
        self.peers
            .values()
            .any(|peer| now.saturating_duration_since(peer.heard) < FORGET_AFTER)
    }
}

#[cfg(test)]
mod tests {
    use itsanas_crypto::{DeviceKeys, SecretBytes};

    use super::*;

    fn device(seed: u8) -> DeviceId {
        DeviceKeys::from_seed(&SecretBytes::new([seed; 32])).device_id()
    }

    #[test]
    fn red_team_privileged_hosts_are_bounded_and_the_oldest_gives_way() {
        // Each one is dialled every minute. A list that grew with every peer
        // that ever said "on" would turn a light round into a full one, and a
        // flood of answers into a dial storm.
        let start = Instant::now();
        let mut peers = AppPeers::default();
        for seed in 0..=u8::try_from(MAX_APP_PEERS).expect("small") {
            let at = start + Duration::from_secs(u64::from(seed));
            peers.heard(device(seed), "198.51.100.1:7000", None, true, at);
        }
        let due = peers.due(start + Duration::from_secs(60));
        assert_eq!(due.len(), MAX_APP_PEERS, "the list grew past its bound");
        assert!(
            due.iter().all(|(known, _)| *known != device(0)),
            "the newest one displaced someone other than the one heard from longest ago"
        );
        assert_eq!(
            due.first().map(|(known, _)| *known),
            Some(device(u8::try_from(MAX_APP_PEERS).expect("small"))),
            "the one heard from last is not dialled first"
        );
    }

    #[test]
    fn red_team_a_peer_that_says_off_or_goes_quiet_is_no_longer_dialled_every_minute() {
        let start = Instant::now();
        let mut peers = AppPeers::default();
        peers.heard(device(1), "198.51.100.1:7000", None, true, start);
        peers.heard(device(2), "198.51.100.2:7000", None, true, start);
        assert!(peers.any(start));

        // Switched off: gone at once.
        peers.heard(device(1), "198.51.100.1:7000", None, false, start);
        let due = peers.due(start);
        assert_eq!(due.len(), 1, "a peer that said off is still dialled");

        // Quiet for the whole window: gone at the next look.
        let later = start + FORGET_AFTER;
        assert!(!peers.any(later), "a peer silent for an hour still counts");
        assert!(
            peers.due(later).is_empty(),
            "a peer silent for an hour is still dialled every minute"
        );
    }

    #[test]
    fn a_peer_reached_through_a_relay_is_dialled_through_it_again() {
        let start = Instant::now();
        let relay = (device(9), "203.0.113.9:7000".to_owned());
        let mut peers = AppPeers::default();
        peers.heard(device(3), "10.0.0.3:7000", Some(&relay), true, start);
        let due = peers.due(start);
        assert_eq!(due[0].1.through.as_ref(), Some(&relay));
        assert_eq!(due[0].1.address, "10.0.0.3:7000");
    }
}
