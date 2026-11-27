//! Relays and introduced hosts: which machine to park on, the presences of the
//! account a relay may hand out and take in, and the board a serving node
//! answers from.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use itsanas_coord::claim::{ClaimedPresence, MAX_CLOCK_SKEW};
use itsanas_crypto::{DeviceId, UserId};

use super::{Contact, HOST_RELAY_FOR, MAX_DEVICES, quiet_for};
use crate::coordinator::is_private_address;

impl Contact {
    /// The machine of the account to park on when this one cannot be
    /// reached (§8 0v): the first, in dialling order, with an address that
    /// works from anywhere and is not itself a relay. Own account only in
    /// this first version: a host of another account relays too (its
    /// `relays_for`), but this node does not choose one yet.
    #[must_use]
    pub fn relay_choice(&self) -> Option<(DeviceId, String)> {
        self.relay_choices().into_iter().next()
    }

    /// Every machine [`Self::relay_choice`] could pick, best first: a
    /// first choice that refuses does not leave this node unreachable while
    /// another would take the park.
    ///
    /// The account's own machines first; then hosts of other accounts the
    /// last round reached ([`Self::hosts_reached`]), which relay for an
    /// account whose claim they confirmed, under a lower cap.
    #[must_use]
    pub fn relay_choices(&self) -> Vec<(DeviceId, String)> {
        let mut choices: Vec<_> = self.candidates();
        for (device, address, _) in &self.hosts {
            if !choices.iter().any(|(known, _)| known == device) {
                choices.push((*device, address.clone()));
            }
        }
        choices.retain(|(_, address)| {
            !address.starts_with(RELAY_PREFIX) && !is_private_address(address)
        });
        choices
    }

    /// The hosts of other accounts this round reached and that proved
    /// themselves, at `now`; kept [`HOST_RELAY_FOR`] after they last did.
    pub fn hosts_reached(&mut self, hosts: Vec<(DeviceId, String)>, now: Instant) {
        for (device, address) in hosts {
            self.hosts.retain(|(known, _, _)| *known != device);
            self.hosts.push((device, address, now));
        }
        self.hosts
            .retain(|(_, _, seen)| now.saturating_duration_since(*seen) < HOST_RELAY_FOR);
    }

    /// The hosts of other accounts in `introduced` this round may dial: not
    /// one that answered nowhere less than its wait ago ([`Self::host_missed`]).
    /// The coordinator hands an address out for a week after its machine last
    /// published (`PRESENCE_TTL`), and each dial to a machine that is off costs
    /// a connect timeout, at every round until then.
    #[must_use]
    pub fn introduced_due(
        &self,
        introduced: Vec<(DeviceId, String)>,
        now: Instant,
    ) -> Vec<(DeviceId, String)> {
        introduced
            .into_iter()
            .filter(|(device, _)| {
                self.host_quiet
                    .get(device)
                    .is_none_or(|(_, until)| now >= *until)
            })
            .collect()
    }

    /// Record that a host of another account the coordinator named did not
    /// answer: it waits [`QUIET_AFTER_MISS`] before its next dial, doubled at
    /// each miss in a row up to [`QUIET_AT_MOST`], as the account's own
    /// machines do. Any answer from it ends the wait ([`Self::round_reached`]).
    ///
    /// [`QUIET_AFTER_MISS`]: super::QUIET_AFTER_MISS
    /// [`QUIET_AT_MOST`]: super::QUIET_AT_MOST
    pub fn host_missed(&mut self, device: DeviceId, now: Instant) {
        if !self.host_quiet.contains_key(&device) && self.host_quiet.len() >= MAX_DEVICES {
            self.host_quiet.retain(|_, (_, until)| *until > now);
            if self.host_quiet.len() >= MAX_DEVICES {
                return;
            }
        }
        let (misses, until) = self.host_quiet.entry(device).or_insert((0, now));
        *misses = misses.saturating_add(1);
        *until = now + quiet_for(*misses);
    }

    /// The relay behind an address `relay:<device>@<address>`, and where to
    /// dial it: its own address from this book when the relay is a machine
    /// of the account, else the one carried in the address -- a host of
    /// another account is in no book here. Either way the relay's key is
    /// pinned on the dial (`connect_relayed`), so an address that leads
    /// elsewhere reaches nobody.
    #[must_use]
    pub fn through(&self, address: &str) -> Option<(DeviceId, String)> {
        let rest = address.strip_prefix(RELAY_PREFIX)?;
        let (id, carried) = rest
            .split_once('@')
            .map_or((rest, None), |(id, at)| (id, Some(at)));
        let relay: DeviceId = id.parse().ok()?;
        self.candidates()
            .into_iter()
            .find(|(device, address)| *device == relay && !address.starts_with(RELAY_PREFIX))
            .or_else(|| {
                carried
                    .filter(|at| !at.is_empty() && !at.starts_with(RELAY_PREFIX))
                    .map(|at| (relay, at.to_owned()))
            })
    }

    /// What this node may hand a peer of its account: every address it holds
    /// with both the device's signature and the owner's claim.
    ///
    /// An unsigned address is on an old coordinator's word and an unclaimed
    /// one proves where a machine is but not whose; neither leaves this node.
    #[must_use]
    pub fn relayable(&self) -> Vec<ClaimedPresence> {
        self.book
            .values()
            .flatten()
            .filter_map(|candidate| {
                Some(ClaimedPresence {
                    presence: candidate.presence.clone()?,
                    claim: candidate.claim.clone()?,
                })
            })
            .collect()
    }

    /// Take in the rows a machine of this account relayed
    /// ([`itsanas_net::Request::Presences`]), keeping only what the book can
    /// check for itself.
    ///
    /// A row is kept only if all of these hold, and each one is an attack it
    /// refuses:
    ///
    /// * it decodes, and both signatures check out for `owner`
    ///   ([`ClaimedPresence::verify_for`]): a relay cannot forge an address,
    ///   nor pass off another account's genuine machine as one of this one's;
    /// * its device is one the book already holds a claim for, from the
    ///   coordinator or the file. **A relay refreshes addresses; it never
    ///   introduces a machine.** A device the owner withdrew is dropped by the
    ///   next coordinator read ([`Self::read`]), and from then on no relay
    ///   can bring it back with a claim signed before the withdrawal, which
    ///   `verify_for` cannot date. New machines are learnt from the
    ///   coordinator, as before;
    /// * its claim is not older than the one the book holds for that device
    ///   (the owner's clock against itself);
    /// * its presence is not older than the latest the book holds for that
    ///   device (the device's clock against itself): an address the machine
    ///   has since left cannot be replayed into the book.
    ///
    /// A kept row joins as a candidate that has never worked: it is dialled
    /// after every address that has, and [`MAX_ADDRESSES`] drops never-worked
    /// ones first, so a relay cannot push out an address that answers. `me`
    /// is this machine, which the book never lists.
    ///
    /// Dates more than [`MAX_CLOCK_SKEW`] past `now` count for nothing: a row
    /// dated so is refused, and a presence already held with such a date is
    /// not "the latest". Otherwise one device whose clock read 2099 once --
    /// signed genuinely, since it is that device's own clock -- became the
    /// newest presence for good, and every later relayed address for it was
    /// refused as older. The coordinator's read, which has no date filter,
    /// still reaches the machine; the relay no longer goes blind.
    ///
    /// [`MAX_ADDRESSES`]: super::MAX_ADDRESSES
    pub fn relayed(&mut self, rows: &[Vec<u8>], owner: UserId, me: DeviceId, now: u64) -> Relayed {
        let horizon = now.saturating_add(MAX_CLOCK_SKEW);
        let mut outcome = Relayed::default();
        for row in rows {
            let Ok(row) = postcard::from_bytes::<ClaimedPresence>(row) else {
                outcome.forged += 1;
                continue;
            };
            let device = row.presence.presence.device;
            if device == me {
                continue;
            }
            if row.verify_for(owner).is_err() || row.presence.presence.at_unix > horizon {
                outcome.forged += 1;
                continue;
            }
            // Signed by the device itself: whoever relays it cannot make it
            // longer, and a shorter one changes nothing kept.
            let longest = self.relayed_heads.entry(device).or_default();
            *longest = (*longest).max(row.presence.presence.head);
            let Some(held) = self.book.get(&device) else {
                outcome.unknown += 1;
                continue;
            };
            let latest_claim = held
                .iter()
                .filter_map(|candidate| candidate.claim.as_ref())
                .map(|claim| claim.claim.issued_unix)
                .max();
            let latest_presence = held
                .iter()
                .filter_map(|candidate| candidate.presence.as_ref())
                .map(|presence| presence.presence.at_unix)
                .filter(|at| *at <= horizon)
                .max();
            let fresh = latest_claim.is_some_and(|at| row.claim.claim.issued_unix >= at)
                && latest_presence.is_none_or(|at| row.presence.presence.at_unix >= at);
            if !fresh {
                outcome.stale += 1;
                continue;
            }
            let address = row.presence.presence.address.clone();
            self.insert(device, &address, Some(row.presence), Some(row.claim), None);
            outcome.kept += 1;
        }
        outcome
    }

    /// The chain lengths [`Self::relayed`] gathered since the last call.
    pub fn take_relayed_heads(&mut self) -> BTreeMap<DeviceId, u64> {
        std::mem::take(&mut self.relayed_heads)
    }

    /// What this node answers a machine of its account that asks where the
    /// others are. See [`Board`].
    #[must_use]
    pub fn board(&self) -> Board {
        let members = self
            .book
            .iter()
            .filter(|(_, candidates)| candidates.iter().any(|c| c.claim.is_some()))
            .map(|(device, _)| *device)
            .collect();
        let rows = self
            .relayable()
            .into_iter()
            .filter_map(|row| {
                let device = row.presence.presence.device;
                postcard::to_stdvec(&row).ok().map(|bytes| (device, bytes))
            })
            .collect();
        Board {
            members,
            rows,
            relays: self.relay_choices(),
        }
    }
}

/// What [`Contact::relayed`] did with a peer's answer.
///
/// The refusals in three counts, because they do not mean the same thing
/// and the first fleet run (2026-10-23) printed all three as one warning,
/// every round: a sibling relaying a withdrawn machine, or an address older
/// than one this node heard first-hand, is ordinary; a row that fails its
/// signatures is the one worth a line in the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Relayed {
    /// Rows that joined or refreshed the book.
    pub kept: usize,
    /// Undecodable, failing the device's or the owner's signature (forged,
    /// or another account's), or dated past the clock skew.
    pub forged: usize,
    /// Genuine, for a machine the book does not hold: withdrawn, or not
    /// listed to this node by the coordinator (yet).
    pub unknown: usize,
    /// Genuine, and older than the claim or presence the book holds.
    pub stale: usize,
}

/// The book as the listener answers from it: who may ask, and what they get.
///
/// A copy, because the book belongs to the daemon's round and the listener
/// answers on other threads; the daemon replaces it after each round
/// ([`SharedBoard::replace`]).
///
/// **Who may ask:** a device the book holds with this account's claim -- a
/// machine the coordinator listed as one of this account's, checked by the
/// owner's signature. Not a device that merely says it belongs: the request
/// carries nothing, and the caller is the device TLS proved. A machine of the
/// account the coordinator has not listed here yet is refused until it is;
/// it is on the coordinator's list and can read it there.
#[derive(Clone, Debug, Default)]
pub struct Board {
    members: BTreeSet<DeviceId>,
    rows: Vec<(DeviceId, Vec<u8>)>,
    /// [`Contact::relay_choices`], for the thread that parks.
    relays: Vec<(DeviceId, String)>,
}

/// How an address names a relay instead of a socket (§8 0v):
/// `relay:<device id>`, the device a machine nobody can reach has parked on.
pub const RELAY_PREFIX: &str = "relay:";

/// A [`Board`] shared between the round that writes it and the listener that
/// answers from it.
#[derive(Debug, Default)]
pub struct SharedBoard(std::sync::Mutex<Board>);

impl SharedBoard {
    /// Replace what the listener answers with.
    pub fn replace(&self, board: Board) {
        // A poisoned lock means a listener thread panicked while reading; the
        // board is a plain value, whole either way.
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = board;
    }
}

impl SharedBoard {
    /// The machines to park on, best first, as the last round saw them.
    #[must_use]
    pub fn relays(&self) -> Vec<(DeviceId, String)> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relays
            .clone()
    }
}

impl itsanas_net::Relay for SharedBoard {
    fn presences_for(&self, caller: DeviceId) -> Option<Vec<Vec<u8>>> {
        let board = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !board.members.contains(&caller) {
            return None;
        }
        // Not the caller's own rows: it knows where it is.
        Some(
            board
                .rows
                .iter()
                .filter(|(device, _)| *device != caller)
                .map(|(_, row)| row.clone())
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use itsanas_coord::claim::{NodeClaim, Presence};
    use itsanas_crypto::{DeviceKeys, SecretBytes, UserKeys};

    use crate::contact::tests::{HOME, device, found, me, owner};

    use super::*;

    #[test]
    fn red_team_a_relay_the_account_knows_is_dialled_at_the_books_address() {
        let mut contact = Contact::new();
        let relay = device(3);
        let listed = vec![
            (device(2), format!("{RELAY_PREFIX}{relay}@203.0.113.5:9797")),
            (relay, "203.0.113.5:9797".to_owned()),
            (device(1), "192.168.1.4:9797".to_owned()),
            (
                device(5),
                format!("{RELAY_PREFIX}{relay}@198.51.100.9:9797"),
            ),
        ];
        contact.read(&listed, &[], &[], Instant::now());
        assert_eq!(
            contact.through(&format!("{RELAY_PREFIX}{relay}")),
            Some((relay, "203.0.113.5:9797".to_owned()))
        );
        assert_eq!(
            contact.through(&format!("{RELAY_PREFIX}{relay}@198.51.100.9:9797")),
            Some((relay, "203.0.113.5:9797".to_owned())),
            "the address carried in a presence won over the book's for a relay of \
             the account"
        );
        assert_eq!(
            contact.through(&format!("{RELAY_PREFIX}{}", device(9))),
            None
        );
        // A host of another account is in no book: the carried address is
        // all there is, and the relay's key is pinned on the dial.
        assert_eq!(
            contact.through(&format!("{RELAY_PREFIX}{}@198.51.100.9:9797", device(9))),
            Some((device(9), "198.51.100.9:9797".to_owned()))
        );
        assert_eq!(
            contact.relay_choice(),
            Some((relay, "203.0.113.5:9797".to_owned())),
            "a machine parked on a private address, or on another relay"
        );
    }

    #[test]
    fn red_team_a_host_of_another_account_relays_only_after_the_accounts_own_machines() {
        let mut contact = Contact::new();
        let own = device(3);
        let host = device(1);
        contact.read(
            &[(own, "203.0.113.5:9797".to_owned())],
            &[],
            &[],
            Instant::now(),
        );
        let now = Instant::now();
        contact.hosts_reached(
            vec![
                (host, "198.51.100.7:9797".to_owned()),
                (device(2), "192.168.1.2:9797".to_owned()),
            ],
            now,
        );
        assert_eq!(
            contact.relay_choices(),
            vec![
                (own, "203.0.113.5:9797".to_owned()),
                (host, "198.51.100.7:9797".to_owned())
            ],
            "a stranger's host was put before the account's own machine, or a host \
             on a private address was offered as a relay"
        );
        // A quiet round proves nothing either way: the host stays.
        contact.hosts_reached(Vec::new(), now + Duration::from_secs(300));
        assert_eq!(
            contact.relay_choices().len(),
            2,
            "a round that pushed nothing dropped the relay: the park would flap"
        );
        contact.hosts_reached(Vec::new(), now + HOST_RELAY_FOR);
        assert_eq!(
            contact.relay_choices().len(),
            1,
            "a host unseen for a day stayed"
        );
    }

    /// A host of another account the coordinator still names after its
    /// machine went off (it keeps the address a week, `PRESENCE_TTL`) was
    /// dialled at every round: 2880 connect timeouts a day at 30 s rounds. It
    /// now waits after each miss, from a minute doubling to the half hour.
    #[test]
    fn red_team_an_introduced_host_that_is_off_is_not_dialled_every_round() {
        let host = device(9);
        let named = vec![(host, "203.0.113.9:9797".to_owned())];
        let every = Duration::from_secs(30);
        let start = Instant::now();
        let end = start + Duration::from_secs(24 * 3600);
        let mut contact = Contact::new();
        let mut dials = 0u32;
        let mut now = start;
        while now < end {
            if !contact.introduced_due(named.clone(), now).is_empty() {
                dials += 1;
                contact.host_missed(host, now);
            }
            contact.round_reached(&BTreeSet::new(), now);
            now += every;
        }
        assert!(
            dials <= 60,
            "a host that is off was dialled {dials} times in a day: a connect timeout at nearly every round"
        );
        assert!(
            dials >= 48,
            "a host that is off was dialled {dials} times in a day: less than every half hour, so one back on waits longer than QUIET_AT_MOST"
        );
    }

    /// A host that answers is dialled again at the next round: the wait is for
    /// a machine that is off, not one that was.
    #[test]
    fn an_introduced_host_that_answers_is_due_again_at_the_next_round() {
        let host = device(9);
        let named = vec![(host, "203.0.113.9:9797".to_owned())];
        let start = Instant::now();
        let minute = Duration::from_secs(60);
        let mut contact = Contact::new();
        for missed in 0..20 {
            contact.host_missed(host, start + minute * missed);
        }
        assert!(
            contact
                .introduced_due(named.clone(), start + minute * 20)
                .is_empty(),
            "a host that missed twenty times in a row was due a minute later"
        );
        contact.round_reached(&BTreeSet::from([host]), start + minute * 20);
        assert_eq!(
            contact.introduced_due(named.clone(), start + minute * 20 + Duration::from_secs(30)),
            named,
            "a host that answered still waited out the half hour its misses had earned"
        );
    }

    /// Device `seed` at `address` on its own clock `at`, with `account`'s
    /// claim issued at `issued`.
    fn dated(account: &UserKeys, seed: u8, address: &str, at: u64, issued: u64) -> ClaimedPresence {
        let keys = DeviceKeys::from_seed(&SecretBytes::new([seed; 32]));
        ClaimedPresence {
            presence: Presence {
                device: keys.device_id(),
                address: address.to_owned(),
                at_unix: at,
                head: 0,
            }
            .sign(&keys),
            claim: NodeClaim {
                owner: account.user_id(),
                device: keys.device_id(),
                pledged_bytes: 0,
                issued_unix: issued,
                revoked: false,
            }
            .sign(account),
        }
    }

    /// HANDOVER §9, "Tail truncation", with no coordinator: a sibling
    /// relaying where the account's machines are hands on how long each one
    /// signed its log, and only a row that verifies counts. Sabotage: keep no
    /// relayed head.
    #[test]
    fn red_team_a_relayed_presence_says_how_long_its_devices_log_is() {
        let account = owner(1);
        let pi = dated(&account, 2, HOME, 100, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));
        let keys = DeviceKeys::from_seed(&SecretBytes::new([2; 32]));
        let mut longer = pi.clone();
        longer.presence = Presence {
            device: keys.device_id(),
            address: HOME.to_owned(),
            at_unix: 200,
            head: 7,
        }
        .sign(&keys);
        let mut forged = longer.clone();
        forged.presence.presence.head = 99;
        contact.relayed(&encoded(&[forged, longer]), me(), this_machine(), NOW);
        assert_eq!(
            contact.take_relayed_heads().get(&keys.device_id()),
            Some(&7),
            "a sibling's relayed presence said nothing of how long the Pi's log is, or a forged length counted"
        );
        assert!(
            contact.take_relayed_heads().is_empty(),
            "the heads were handed over twice"
        );
    }

    fn encoded(rows: &[ClaimedPresence]) -> Vec<Vec<u8>> {
        rows.iter()
            .map(|row| postcard::to_stdvec(row).expect("encode"))
            .collect()
    }

    /// A book that got `rows` from the coordinator.
    fn book_of(rows: &[ClaimedPresence]) -> Contact {
        let presences: Vec<_> = rows.iter().map(|row| row.presence.clone()).collect();
        let mut contact = Contact::new();
        contact.read(&found(&presences), &presences, rows, Instant::now());
        contact
    }

    fn this_machine() -> DeviceId {
        device(0xEE)
    }

    const AWAY: &str = "203.0.113.7:9797";

    /// "Now" for the relay tests: far past every date they sign.
    const NOW: u64 = 1_000_000;

    /// The point of the step: the Pi moved while the coordinator was down,
    /// the laptop reached it since, and tells this machine. The new address
    /// joins behind the one that worked, and is relayed onward.
    #[test]
    fn a_newer_address_relayed_by_a_machine_of_the_account_joins_the_book() {
        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));
        contact.worked(pi.presence.presence.device, HOME, 1000);

        let moved = dated(&owner(1), 2, AWAY, 200, 10);
        let outcome = contact.relayed(
            &encoded(std::slice::from_ref(&moved)),
            me(),
            this_machine(),
            NOW,
        );

        assert_eq!(
            outcome,
            Relayed {
                kept: 1,
                ..Relayed::default()
            }
        );
        let addresses: Vec<_> = contact.candidates().into_iter().map(|(_, a)| a).collect();
        assert_eq!(
            addresses,
            vec![HOME.to_owned(), AWAY.to_owned()],
            "a relayed address must join behind the one that worked, never ahead of it"
        );
        assert!(
            contact.relayable().contains(&moved),
            "a kept row is not passed on"
        );
    }

    /// Sabotage: skip `verify_for` in `relayed`.
    #[test]
    fn red_team_a_peer_cannot_hand_out_a_presence_it_forged() {
        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));
        let mut forged = dated(&owner(1), 2, AWAY, 200, 10);
        forged.presence.presence.address = "198.51.100.66:9797".to_owned();

        let outcome = contact.relayed(&encoded(&[forged]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                forged: 1,
                ..Relayed::default()
            }
        );
        assert_eq!(
            contact.candidates(),
            vec![(pi.presence.presence.device, HOME.to_owned())],
            "an address the device never signed reached the book: the relay chooses where this \
             machine dials"
        );
    }

    /// A relay holds genuine presences of other accounts' machines, each
    /// signed by its device and claimed by its owner. None of them is ours.
    /// Sabotage: skip `verify_for` in `relayed`.
    #[test]
    fn red_team_a_relay_cannot_pass_off_another_accounts_machine_as_ours() {
        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));
        // The same device, claimed by somebody else at a newer date: it would
        // pass every date check, and only the owner's signature refuses it.
        let theirs = dated(&owner(2), 2, AWAY, 200, 20);

        let outcome = contact.relayed(&encoded(&[theirs]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                forged: 1,
                ..Relayed::default()
            }
        );
        assert_eq!(
            contact.candidates().len(),
            1,
            "another account's claim was taken as ours"
        );
    }

    /// The owner withdrew the Pi; the coordinator stopped listing it, and the
    /// book dropped it. A relay still holds its old, unrevoked claim, which
    /// `verify_for` cannot date. Sabotage: let `relayed` take a device the
    /// book does not hold.
    #[test]
    fn red_team_a_relay_cannot_bring_back_a_machine_the_owner_withdrew() {
        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let laptop = dated(&owner(1), 3, "192.168.1.30:9797", 100, 10);
        let mut contact = book_of(&[pi.clone(), laptop.clone()]);
        // The next read lists the laptop only: the Pi was withdrawn.
        contact.read(
            &found(std::slice::from_ref(&laptop.presence)),
            std::slice::from_ref(&laptop.presence),
            std::slice::from_ref(&laptop),
            Instant::now(),
        );

        let replayed = dated(&owner(1), 2, AWAY, 300, 10);
        let outcome = contact.relayed(&encoded(&[replayed]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                unknown: 1,
                ..Relayed::default()
            }
        );
        assert!(
            contact
                .candidates()
                .iter()
                .all(|(d, _)| *d != pi.presence.presence.device),
            "a withdrawn machine is back in the book on a relay's word, and would be dialled \
             and relayed onward"
        );
    }

    /// A clock far ahead once must not blind the relay for good: the Pi's
    /// presence dated 2099 sits in the book, and a relayed presence dated now
    /// is kept all the same; and a row dated past the skew is refused.
    /// Sabotage: drop the horizon filter on the held presences.
    #[test]
    fn red_team_a_presence_from_a_clock_far_ahead_does_not_blind_the_relay() {
        const Y2099: u64 = 4_070_908_800;
        let pi = dated(&owner(1), 2, AWAY, Y2099, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));

        let now_here = dated(&owner(1), 2, HOME, NOW - 5, 10);
        let outcome = contact.relayed(&encoded(&[now_here]), me(), this_machine(), NOW);
        assert_eq!(
            outcome,
            Relayed {
                kept: 1,
                ..Relayed::default()
            },
            "a presence dated 2099 in the book refused every honest one after it: the \
             relay stays blind to that machine for good"
        );

        let ahead = dated(&owner(1), 2, AWAY, NOW + MAX_CLOCK_SKEW + 1, 10);
        let outcome = contact.relayed(&encoded(&[ahead]), me(), this_machine(), NOW);
        assert_eq!(outcome.forged, 1, "a row dated past the skew was kept");
    }

    /// The Pi left `AWAY` for `HOME`. A relay replays its genuine, older
    /// presence at `AWAY`. Sabotage: drop the presence-date check.
    #[test]
    fn red_team_a_peer_cannot_strand_a_machine_at_an_address_it_left() {
        let pi = dated(&owner(1), 2, HOME, 200, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));

        let stale = dated(&owner(1), 2, AWAY, 100, 10);
        let outcome = contact.relayed(&encoded(&[stale]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                stale: 1,
                ..Relayed::default()
            }
        );
        assert_eq!(
            contact.candidates(),
            vec![(pi.presence.presence.device, HOME.to_owned())],
            "an address the machine left was replayed into the book: a connect timeout every \
             round, and relayed onward"
        );
    }

    /// The owner re-issued the Pi's claim; a relay offers the older one.
    /// Sabotage: drop the claim-date check.
    #[test]
    fn red_team_a_claim_older_than_the_one_held_is_refused() {
        let pi = dated(&owner(1), 2, HOME, 100, 20);
        let mut contact = book_of(std::slice::from_ref(&pi));

        let older = dated(&owner(1), 2, AWAY, 200, 10);
        let outcome = contact.relayed(&encoded(&[older]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                stale: 1,
                ..Relayed::default()
            },
            "a claim the owner has replaced was taken back in"
        );
    }

    /// Sabotage: drop the `members` check in `SharedBoard::presences_for`.
    #[test]
    fn red_team_a_stranger_asking_for_the_accounts_presences_is_refused() {
        use itsanas_net::Relay;

        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let laptop = dated(&owner(1), 3, "192.168.1.30:9797", 100, 10);
        let shared = SharedBoard::default();
        shared.replace(book_of(&[pi.clone(), laptop.clone()]).board());

        assert_eq!(
            shared.presences_for(device(0x77)),
            None,
            "a device that is not one of this account's machines was told where they are"
        );
        assert_eq!(
            shared.presences_for(laptop.presence.presence.device),
            Some(encoded(&[pi])),
            "a machine of the account should get the others, and not itself"
        );
    }
}
