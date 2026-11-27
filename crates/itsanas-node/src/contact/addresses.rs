//! What the coordinator lists for the account's devices, taken into the book:
//! bounded, signed once signed, and never displacing an address that worked.

use std::collections::BTreeSet;
use std::time::Instant;

use super::{Candidate, Contact, MAX_ADDRESSES, MAX_DEVICES, sort_best_first};
use itsanas_coord::claim::{ClaimedPresence, SignedClaim, SignedPresence};
use itsanas_crypto::DeviceId;

impl Contact {
    /// Take in the coordinator's list of the account's other devices.
    ///
    /// `found` is every `(device, address)` the read kept; `presences` are the
    /// signed ones among them (none when an older coordinator answered
    /// unsigned). An address with its signature in `presences` carries it
    /// into the book, which is what lets it be written to disk. `claimed` are
    /// those that came with this account's claim, already checked
    /// ([`crate::coordinator::verified_claimed`]); none when the coordinator
    /// is older than `ClaimedPeers`.
    ///
    /// A device no longer listed is dropped: it was withdrawn, or has been
    /// silent past the presence's lifetime, and in both cases this node has no
    /// address for it worth dialling. For the others, the listed address joins
    /// the set of candidates. It replaces nothing and takes no rank from being
    /// new: the one that last worked is still dialled first.
    pub fn read(
        &mut self,
        found: &[(DeviceId, String)],
        presences: &[SignedPresence],
        claimed: &[ClaimedPresence],
        now: Instant,
    ) {
        self.read_at = Some(now);
        self.missed = false;
        // News for a machine that is missing -- an address the book did not
        // hold for it -- is what re-reading is for: the next miss may be the
        // same story again, so it is asked about soon, not in an hour.
        let news = found.iter().any(|(device, address)| {
            self.missing.contains(device)
                && self
                    .book
                    .get(device)
                    .is_none_or(|held| !held.iter().any(|candidate| candidate.address == *address))
        });
        if news {
            self.fall_back(now);
        }
        let listed: BTreeSet<DeviceId> = found.iter().map(|(device, _)| *device).collect();
        let before = self.book.len();
        self.book.retain(|device, _| listed.contains(device));
        self.changed |= self.book.len() != before;
        self.quiet.retain(|device, _| listed.contains(device));

        for (device, address) in found {
            let presence = presences
                .iter()
                .find(|signed| {
                    signed.presence.device == *device && signed.presence.address == *address
                })
                .cloned();
            let claim = claimed
                .iter()
                .find(|row| presence.as_ref() == Some(&row.presence))
                .map(|row| row.claim.clone());
            self.insert(*device, address, presence, claim, None);
        }
    }

    /// Add one candidate, within [`MAX_DEVICES`] and [`MAX_ADDRESSES`]. The one
    /// door into the book, for the wire and for the file alike.
    pub(super) fn insert(
        &mut self,
        device: DeviceId,
        address: &str,
        presence: Option<SignedPresence>,
        claim: Option<SignedClaim>,
        worked: Option<u64>,
    ) {
        if !self.book.contains_key(&device) && self.book.len() >= MAX_DEVICES {
            return;
        }
        let candidates = self.book.entry(device).or_default();
        if let Some(known) = candidates.iter_mut().find(|known| known.address == address) {
            // Signed at last, or signed again later: keep the latest proof.
            // The device's own date, compared only with its own earlier
            // ones. Kept old, the presence this node relays would look stale
            // to the receiver, who could not tell it from a replay.
            if let Some(presence) = presence
                && known
                    .presence
                    .as_ref()
                    .is_none_or(|held| held.presence.at_unix < presence.presence.at_unix)
            {
                known.presence = Some(presence);
                self.changed = true;
            }
            // A claim at last, or a later one: the later is the owner's
            // current word on the device.
            if let Some(claim) = claim
                && known.presence.is_some()
                && known
                    .claim
                    .as_ref()
                    .is_none_or(|held| held.claim.issued_unix < claim.claim.issued_unix)
            {
                known.claim = Some(claim);
                self.changed = true;
            }
            return;
        }
        // An address the book did not hold: the machine may have moved, and
        // is dialled there at the next round ([`QUIET_AFTER_MISS`]) -- once:
        // the wait it was in goes on doubling if that fails too.
        if let Some(quiet) = self.quiet.get_mut(&device) {
            quiet.news = true;
        }
        let candidates = self.book.entry(device).or_default();
        candidates.push(Candidate {
            address: address.to_owned(),
            worked,
            // Never a claim without the presence it vouches for.
            claim: presence.as_ref().and(claim),
            presence,
            proven: false,
        });
        if candidates.len() > MAX_ADDRESSES {
            sort_best_first(candidates);
            candidates.truncate(MAX_ADDRESSES);
        }
        self.changed = true;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::contact::PUBLISH_EVERY;
    use crate::contact::tests::{HOME, ROUND, device, listing};

    use super::*;

    /// Rodin on #322: the wait ends on an address the book did not hold, so
    /// a coordinator listing a new port at every read could have this node
    /// dial a machine that is off at every read. Reads are backed off and
    /// floored on their own (`REREAD_AFTER_MISS`, `RESET_FLOOR`), so that
    /// buys at most one dial a read: over a day of 30 s rounds, a few hundred,
    /// never the 2880 of a dial a round.
    #[test]
    fn red_team_a_coordinator_listing_a_new_port_every_read_buys_a_dial_a_read_at_most() {
        let laptop = device(2);
        let every = Duration::from_secs(30);
        let day = Duration::from_secs(24 * 3600);
        let start = Instant::now();
        let mut contact = Contact::new();
        contact.published(Some(HOME), start);
        let (mut reads, mut dials) = (0u32, 0u32);
        let rounds = u32::try_from(day.as_secs() / every.as_secs()).expect("rounds");
        for round in 0..rounds {
            let now = start + every * round;
            let due = contact.due(Some(HOME), now);
            contact.asked(&due, now);
            if due.publish {
                contact.published(Some(HOME), now);
            }
            if due.read {
                reads += 1;
                let moved = format!("203.0.113.2:{}", 10_000 + round);
                contact.read(&[(laptop, moved)], &[], &[], now);
            }
            let dialled: Vec<String> = contact
                .dialable(|_| Some(false), |_| false, now)
                .into_iter()
                .filter(|(device, _)| *device == laptop)
                .map(|(_, address)| address)
                .collect();
            if !dialled.is_empty() {
                dials += 1;
            }
            for address in &dialled {
                contact.failed(laptop, address, now);
            }
            contact.round_reached(&BTreeSet::new(), now);
        }
        assert!(
            dials <= reads + 60,
            "a coordinator handing out a new port bought {dials} dials of a machine that is \
             off with {reads} reads: more than one a read plus the wait's own"
        );
        assert!(
            dials <= 500,
            "a coordinator handing out a new port at every read had this node dial a \
             machine that is off {dials} times in a day of 30 s rounds"
        );
    }

    /// The coordinator chooses which presences it lists -- stale ones, or
    /// unsigned ones from an older coordinator -- so it decides what arrives
    /// in this table. It must not decide its size.
    #[test]
    fn red_team_a_coordinator_cannot_grow_the_address_book_without_bound() {
        let mut contact = Contact::new();
        let now = Instant::now();
        let flood: Vec<(DeviceId, String)> = (0..=u16::MAX)
            .map(|n| {
                let mut id = [0u8; 32];
                id[..2].copy_from_slice(&n.to_be_bytes());
                (
                    DeviceId::from_bytes(id),
                    format!("10.0.{}.{}:9797", n >> 8, n & 0xff),
                )
            })
            .collect();
        contact.read(&flood, &[], &[], now);
        assert_eq!(
            contact.book.len(),
            MAX_DEVICES,
            "one answer filled the table"
        );

        let one = flood[0].0;
        let addresses: Vec<(DeviceId, String)> = (0..1000)
            .map(|n| (one, format!("10.1.{}.{}:9797", n >> 8, n & 0xff)))
            .collect();
        contact.read(&addresses, &[], &[], now);
        assert!(
            contact.book.values().all(|c| c.len() <= MAX_ADDRESSES),
            "one device was given a thousand addresses and kept them"
        );
    }

    /// A relayed or replayed address is an opinion; an address that answered
    /// is evidence. A list of stale addresses for the Pi -- from a coordinator
    /// that lies, or one that is only out of date -- must not push the address
    /// that works down the dialling order, where a round's connect timeouts
    /// would be spent on the stale ones first.
    #[test]
    fn red_team_addresses_that_never_worked_do_not_displace_one_that_did() {
        let mut contact = Contact::new();
        let start = Instant::now();
        let pi = device(2);
        contact.read(&[(pi, "192.168.1.20:9797".to_owned())], &[], &[], start);
        contact.worked(pi, "192.168.1.20:9797", 1_000);

        let stale: Vec<(DeviceId, String)> = (0..20)
            .map(|n| (pi, format!("203.0.113.{n}:9797")))
            .collect();
        contact.read(&stale, &[], &[], start + ROUND);

        let order = contact.candidates();
        assert_eq!(
            order.first().map(|(_, address)| address.as_str()),
            Some("192.168.1.20:9797"),
            "the address that answered was pushed down by addresses that never did"
        );
        assert!(order.len() <= MAX_ADDRESSES, "the replay was kept in full");
    }

    /// A coordinator that pretends to be older than `SignedPeers` hangs up on
    /// it, and a client that then asks `Peers` reads a list the coordinator can
    /// forge at will: the signature check would be advisory against the one
    /// party it exists to check. Once a coordinator has signed in front of this
    /// process, that fallback is closed for good.
    #[test]
    fn red_team_a_coordinator_that_has_signed_cannot_talk_this_node_down_to_an_unsigned_list() {
        let mut contact = Contact::new();
        let start = Instant::now();
        assert!(
            contact.due(Some(HOME), start).accept_unsigned,
            "a coordinator older than signed lists must still be readable"
        );

        contact.published(Some(HOME), start);
        contact.read(&listing(&[2]), &[], &[], start);
        contact.signed();
        for round in 1..=24u32 {
            assert!(
                !contact
                    .due(Some(HOME), start + PUBLISH_EVERY * round)
                    .accept_unsigned,
                "hour {round}: a coordinator that signed may now forge the list by hanging up"
            );
        }
    }

    #[test]
    fn a_device_no_longer_listed_is_forgotten() {
        let mut contact = Contact::new();
        let now = Instant::now();
        contact.read(&listing(&[2, 3]), &[], &[], now);
        contact.read(&listing(&[2]), &[], &[], now);
        let devices: BTreeSet<DeviceId> =
            contact.candidates().into_iter().map(|(d, _)| d).collect();
        assert_eq!(
            devices,
            [device(2)].into(),
            "a withdrawn device kept its place and would be dialled every round"
        );
    }
}
