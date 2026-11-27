//! Whom a round dials and in what order: the configured peers, the machines
//! the coordinator listed or introduced, and the contact book's addresses.

use std::{
    collections::BTreeSet,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use itsanas_crypto::DeviceId;
use itsanas_net::PeerClient;
use itsanas_node::contact::Contact;
use itsanas_policy::Scope as PolicyScope;

use super::{pledge::local, reachability::this_listener, round::sync_once, shutdown::halted};
use crate::{coordinator, discovery::Neighbourhood, node::Node};

/// Dial the hosts of other accounts the coordinator names, for a machine with
/// nobody typed in and nobody on its LAN: what makes offering space reach anyone.
/// Says whether one of them did what only a real host does.
pub(super) fn dial_introduced(
    node: &Node,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    contact: &mut Contact,
    reached: &mut BTreeSet<DeviceId>,
    hosts: &mut Vec<(DeviceId, String)>,
    scope: PolicyScope,
) -> bool {
    let mut in_touch = false;
    for (device, address) in contact.introduced_due(introduced(node), Instant::now()) {
        if halted(node, shutdown) {
            break;
        }
        if reached.contains(&device) {
            continue;
        }
        let through = contact.through(&address);
        // A relay answers a caller it knows: this presents to it first, so the
        // splice works from the next round on.
        if let Some((relay, at)) = through
            .as_ref()
            .filter(|(relay, _)| !reached.contains(relay))
            && let Some(outcome) = sync_once(node, at, None, Some(*relay), false, false, scope)
        {
            reached.insert(outcome.device);
        }
        let outcome = sync_once(
            node,
            &address,
            through.as_ref(),
            Some(device),
            false,
            false,
            scope,
        );
        if let Some(outcome) = outcome {
            reached.insert(outcome.device);
            in_touch |= outcome.earned_trust;
            if outcome.earned_trust {
                neighbourhood.confirm(outcome.device);
                hosts.push((outcome.device, address));
            }
        } else {
            // Off, or gone: the coordinator keeps its address a week.
            contact.host_missed(device, Instant::now());
        }
    }
    in_touch
}

/// How long the coordinator's list of hosts is kept: it costs a connection, and
/// the same caller is told the same few.
pub(super) const INTRODUCED_FOR: Duration = Duration::from_secs(3600);

/// The hosts the coordinator names, asked at most once per [`INTRODUCED_FOR`]. A
/// coordinator that cannot be reached leaves the last list in use.
pub(super) fn introduced(node: &Node) -> Vec<(DeviceId, String)> {
    if node.config.coordinator.is_none() {
        return Vec::new();
    }
    let mut kept = local(node)
        .introduced
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let due = kept
        .as_ref()
        .is_none_or(|(asked, _)| asked.elapsed() >= INTRODUCED_FOR);
    if due {
        let list = coordinator::hosts(node).unwrap_or_else(|error| {
            println!("coordinator: could not ask for hosts ({error})");
            kept.take().map(|(_, list)| list).unwrap_or_default()
        });
        *kept = Some((Instant::now(), list));
    }
    kept.as_ref()
        .map(|(_, list)| list.clone())
        .unwrap_or_default()
}

/// Whether a machine found on the LAN that answered counts as a contact for
/// `isolated` (`safety::reached_words`): one of the account's (its beacon
/// carries a tag only the account's key makes), one confirmed before, or one
/// that just did what only a real host does. Not any stranger that
/// authenticated: its identity cost nothing to mint, and a laptop on a café
/// LAN, one stranger answering a round, would read green all week while
/// nothing of its account synced -- the same reason the listener's witness
/// is not used.
pub(super) const fn counts_as_contact(mine: bool, confirmed: bool, earned_trust: bool) -> bool {
    mine || confirmed || earned_trust
}

/// Sync with every peer in the configuration, and say which answered.
///
/// In the same order as everything else a round dials: the ones that can
/// answer from where this machine stands, first. A configured peer is usually
/// a LAN address somebody typed at home, and a round that starts with those
/// spends its first connect timeouts on them from anywhere else.
pub(super) fn dial_configured(
    node: &Node,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    scope: PolicyScope,
    hosts: &mut Vec<(DeviceId, String)>,
) -> BTreeSet<DeviceId> {
    let mut reached = BTreeSet::new();
    let mut configured = node.config.peers.clone();
    coordinator::reachable_first_addresses(&mut configured);
    for peer in &configured {
        if halted(node, shutdown) {
            break;
        }
        if let Some(outcome) = sync_once(node, peer, None, None, true, false, scope) {
            if outcome.earned_trust {
                hosts.push((outcome.device, peer.clone()));
            }
            neighbourhood.confirm(outcome.device);
            reached.insert(outcome.device);
        }
    }
    reached
}

/// Dial the account's devices at every address the coordinator gave for them,
/// best first, until each answers once.
pub(super) fn dial_listed(
    node: &Node,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    contact: &mut Contact,
    reached: &mut BTreeSet<DeviceId>,
    scope: PolicyScope,
    bound: std::net::SocketAddr,
) {
    // Never this node itself, whatever device the book names there.
    let is_me = this_listener(
        node.config.announce.clone(),
        coordinator::found_address(),
        bound,
        itsanas_discover::LocalNets::of_this_machine(),
    );
    let site = Dialling {
        node,
        shutdown,
        scope,
    };
    dial_book(
        &site,
        contact,
        neighbourhood,
        reached,
        is_me,
        node.store.owner(),
        node.store.device_id(),
    );
    // How long each sibling signed its log, as the others relayed it: what
    // tells a listing it is short with no coordinator (`Store::behind`).
    for (device, head) in contact.take_relayed_heads() {
        if device != node.store.device_id() {
            let _ = node.store.note_known_head(&device, head);
        }
    }
}

/// What [`dial_book`] needs from the world, so what a round learns from the
/// account's machines can be driven by a test without a socket or a peer.
pub(super) trait DialSite {
    /// Whether to stop dialling ([`fn@halted`]).
    fn halted(&self) -> bool;
    /// One round with `expect` at `address`, or through the relay `through`;
    /// `None` if it did not answer. `ask_presences` asks it what it knows of
    /// the account's other machines (`Outcome::relayed`): [`dial_book`]
    /// decides it, not the adapter, so that a test sees it asked.
    fn sync(
        &self,
        address: &str,
        through: Option<&(DeviceId, String)>,
        expect: DeviceId,
        ask_presences: bool,
    ) -> Option<Outcome>;
    /// This machine's clock, as [`Contact::worked`] and [`Contact::relayed`]
    /// read it.
    fn now_unix(&self) -> u64;
}

/// The daemon's [`DialSite`]: its node, pinned on the device it dials.
pub(super) struct Dialling<'a> {
    node: &'a Node,
    shutdown: &'a AtomicBool,
    scope: PolicyScope,
}

impl DialSite for Dialling<'_> {
    fn halted(&self) -> bool {
        halted(self.node, self.shutdown)
    }

    fn sync(
        &self,
        address: &str,
        through: Option<&(DeviceId, String)>,
        expect: DeviceId,
        ask_presences: bool,
    ) -> Option<Outcome> {
        sync_once(
            self.node,
            address,
            through,
            Some(expect),
            true,
            ask_presences,
            self.scope,
        )
    }

    fn now_unix(&self) -> u64 {
        itsanas_discover::now_unix()
    }
}

/// [`dial_listed`]'s decisions: who is dialled where, and what an answer
/// teaches the book and the neighbourhood. `owner` and `me` are this
/// machine's account and device, for [`Contact::relayed`].
pub(super) fn dial_book(
    site: &impl DialSite,
    contact: &mut Contact,
    neighbourhood: &Neighbourhood,
    reached: &mut BTreeSet<DeviceId>,
    is_me: impl Fn(&str) -> bool,
    owner: itsanas_crypto::UserId,
    me: DeviceId,
) {
    // A private address only of a machine heard on this LAN, or one that
    // answered, or at a probe round: two houses on 192.168.1.x are not one
    // network, and another VLAN or a VPN may be (`Contact::dialable`). A
    // machine that answered nowhere waits a little longer at each miss, up
    // to half an hour (`QUIET_AFTER_MISS`): most of an account is off.
    let now = Instant::now();
    for (device, address) in contact.dialable(|device| neighbourhood.heard_here(device), is_me, now)
    {
        if site.halted() || reached.contains(&device) {
            continue;
        }
        // A machine parked on a relay (§8 0v) is dialled through it, at the
        // relay's own address from this book.
        let through = contact.through(&address);
        if address.starts_with(itsanas_node::contact::RELAY_PREFIX) && through.is_none() {
            continue;
        }
        // Every machine of the account is asked what it knows of the others:
        // that is the presence relay, and without it a machine that moved
        // while the coordinator was down stays unsynced until it is back.
        let Some(outcome) = site.sync(&address, through.as_ref(), device, true) else {
            contact.failed(device, &address, Instant::now());
            continue;
        };
        reached.insert(outcome.device);
        contact.worked(device, &address, site.now_unix());
        // What this machine of the account knows of the others. Checked
        // row by row (`Contact::relayed`); what it adds is dialled from the
        // next round, behind every address that has worked.
        if let Some(rows) = &outcome.relayed {
            let relayed = contact.relayed(rows, owner, me, site.now_unix());
            // Only what fails its signatures is worth a line: a withdrawn
            // machine or an address older than one heard first-hand is what
            // siblings relay every round (`Relayed`), and printing those
            // drowned the one case that means somebody is lying.
            if relayed.forged > 0 {
                eprintln!(
                    "itsanas: {address} relayed {} address(es) of this account's machines \
                     that fail their signatures (forged, another account's, or dated in \
                     the future); ignored",
                    relayed.forged
                );
            }
        }
        if outcome.earned_trust {
            neighbourhood.confirm(outcome.device);
        }
    }
}

/// What one round against one peer established.
#[derive(Clone, Debug)]
pub(super) struct Outcome {
    /// Which device actually answered.
    pub(super) device: DeviceId,
    /// Whether it did something only a real host does.
    ///
    /// Deliberately separate from "it answered". A device key is a free
    /// keypair, so authenticating identifies a peer and vouches for nothing.
    pub(super) earned_trust: bool,
    /// What the peer said of where the account's machines are
    /// ([`PeerClient::presences`]), unchecked. Asked only of a book device;
    /// `None` if it could not tell.
    pub(super) relayed: Option<Vec<Vec<u8>>>,
}

/// Dial `peer`, or `expect` through the relay `through` (§8 0v).
pub(super) fn dial(
    node: &Node,
    peer: &str,
    through: Option<&(DeviceId, String)>,
    expect: Option<DeviceId>,
) -> itsanas_net::Result<PeerClient> {
    let budget = Some(itsanas_net::transport::PEER_SESSION_BUDGET);
    match (through, expect) {
        (Some((relay, address)), Some(target)) => PeerClient::connect_relayed(
            itsanas_net::AddressList(address),
            *relay,
            target,
            &node.device,
            node.store.owner(),
            budget,
        ),
        _ if peer.starts_with(itsanas_node::contact::RELAY_PREFIX) => Err(
            itsanas_net::NetError::Refused("no known relay for this address".to_owned()),
        ),
        _ => PeerClient::connect_within(
            itsanas_net::AddressList(peer),
            &node.device,
            node.store.owner(),
            expect,
            budget,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::id;
    use super::*;
    use std::sync::atomic::Ordering;

    /// The adapter `dial_book` is tested through, run for real: a round with
    /// the machine expected at an address happens, one with another device
    /// expected there does not, and a pause on disk or a shutdown halts it.
    /// Sabotage: `sync_once` handed `None` for the device expected.
    #[test]
    fn red_team_the_dialling_adapter_is_pinned_on_the_device_and_halts_on_a_pause() {
        crate::testing::with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("tempdir");
            let (host, phrase) =
                crate::testing::phone(&dir.path().join("host"), coordinator, "camille", None);
            let (me, _) = crate::testing::phone(
                &dir.path().join("me"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let shutdown = AtomicBool::new(false);
            let site = Dialling {
                node: &me,
                shutdown: &shutdown,
                scope: PolicyScope::Everything,
            };
            crate::testing::serving(&host, |at| {
                let at = at.to_string();
                assert!(
                    site.sync(&at, None, host.store.device_id(), false)
                        .is_some(),
                    "the machine expected at its address was not synced with"
                );
                assert!(
                    site.sync(&at, None, me.store.device_id(), false).is_none(),
                    "a round ran with whatever answered at an address that names another device: whoever holds the address takes the round"
                );
            });
            assert!(
                !site.halted(),
                "a machine neither paused nor stopping halted its dials"
            );
            crate::control::Control {
                paused_since: Some(itsanas_discover::now_unix()),
                ..crate::control::Control::default()
            }
            .write(&me.home)
            .expect("pause");
            assert!(
                site.halted(),
                "a pause written on disk did not halt the dials under way"
            );
            crate::control::Control::default()
                .write(&me.home)
                .expect("resume");
            shutdown.store(true, Ordering::Relaxed);
            assert!(site.halted(), "a shutdown did not halt the dials under way");
        });
    }

    /// Who found on the LAN counts as a contact for `isolated`. A stranger
    /// that only authenticated does not: its key cost nothing, and a laptop
    /// on a café LAN would read green all week with nothing of its account
    /// synced. Sabotage: count every machine that answered.
    #[test]
    fn red_team_a_stranger_that_only_answered_is_no_contact() {
        assert!(
            !counts_as_contact(false, false, false),
            "a stranger that only authenticated counts as a contact: a café LAN keeps a \
             cut-off laptop green"
        );
        assert!(
            counts_as_contact(true, false, false),
            "the account's own machine"
        );
        assert!(
            counts_as_contact(false, true, false),
            "a host confirmed before"
        );
        assert!(
            counts_as_contact(false, false, true),
            "a host that kept our data"
        );
    }

    /// A scripted world for [`dial_book`]: the addresses that answer, with
    /// whether the answer earned trust and the rows it relayed, and every
    /// dial written down with the relay it went through.
    #[derive(Default)]
    struct FakeDials {
        answers: std::cell::RefCell<std::collections::BTreeMap<String, Answer>>,
        dials: std::cell::RefCell<Vec<Dial>>,
        /// Paused or shut down once this many dials are made.
        halt_after: std::cell::Cell<Option<usize>>,
    }

    /// What an address says when dialled: earned trust, relayed rows.
    type Answer = (bool, Option<Vec<Vec<u8>>>);

    /// One dial: the device expected, the address, the relay it went through.
    type Dial = (DeviceId, String, Option<(DeviceId, String)>);

    impl FakeDials {
        fn answering(answers: &[(&str, Answer)]) -> Self {
            let site = Self::default();
            site.answer(answers);
            site
        }

        /// From now on only `answers` answer.
        fn answer(&self, answers: &[(&str, Answer)]) {
            *self.answers.borrow_mut() = answers
                .iter()
                .map(|(address, answer)| ((*address).to_owned(), answer.clone()))
                .collect();
        }

        /// Every dial so far, forgotten.
        fn dialled(&self) -> Vec<(DeviceId, String)> {
            self.dialled_through()
                .into_iter()
                .map(|(device, address, _)| (device, address))
                .collect()
        }

        /// [`Self::dialled`], with the relay each dial went through.
        fn dialled_through(&self) -> Vec<Dial> {
            std::mem::take(&mut *self.dials.borrow_mut())
        }
    }

    impl DialSite for FakeDials {
        fn halted(&self) -> bool {
            self.halt_after
                .get()
                .is_some_and(|after| self.dials.borrow().len() >= after)
        }

        fn sync(
            &self,
            address: &str,
            through: Option<&(DeviceId, String)>,
            expect: DeviceId,
            ask_presences: bool,
        ) -> Option<Outcome> {
            self.dials
                .borrow_mut()
                .push((expect, address.to_owned(), through.cloned()));
            let (earned_trust, relayed) = self.answers.borrow().get(address).cloned()?;
            Some(Outcome {
                device: expect,
                earned_trust,
                // A peer that is not asked for presences sends none.
                relayed: relayed.filter(|_| ask_presences),
            })
        }

        fn now_unix(&self) -> u64 {
            RELAY_NOW
        }
    }

    /// "Now" for the relay tests: far past every date they sign.
    const RELAY_NOW: u64 = 1_000_000;

    /// The account these tests' node belongs to.
    fn account() -> itsanas_crypto::UserKeys {
        itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([1; 32]))
    }

    /// This machine, which the book never lists.
    const ME: u8 = 0xEE;

    /// Device `seed` at `address` on its own clock `at`, with the account's
    /// claim on it.
    fn claimed(seed: u8, address: &str, at: u64) -> itsanas_coord::claim::ClaimedPresence {
        let keys =
            itsanas_crypto::DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new([seed; 32]));
        itsanas_coord::claim::ClaimedPresence {
            presence: itsanas_coord::claim::Presence {
                device: keys.device_id(),
                address: address.to_owned(),
                at_unix: at,
                head: 0,
            }
            .sign(&keys),
            claim: itsanas_coord::claim::NodeClaim {
                owner: account().user_id(),
                device: keys.device_id(),
                pledged_bytes: 0,
                issued_unix: 10,
                revoked: false,
            }
            .sign(&account()),
        }
    }

    fn device_of(row: &itsanas_coord::claim::ClaimedPresence) -> DeviceId {
        row.presence.presence.device
    }

    /// A book the coordinator gave `rows`, signed and claimed.
    fn book_of(rows: &[itsanas_coord::claim::ClaimedPresence]) -> Contact {
        let presences: Vec<_> = rows.iter().map(|row| row.presence.clone()).collect();
        let found: Vec<_> = presences
            .iter()
            .map(|signed| (signed.presence.device, signed.presence.address.clone()))
            .collect();
        let mut contact = Contact::new();
        contact.read(&found, &presences, rows, Instant::now());
        contact
    }

    /// A book the coordinator gave `listed`, on its word alone.
    fn listed(listed: &[(DeviceId, String)]) -> Contact {
        let mut contact = Contact::new();
        contact.read(listed, &[], &[], Instant::now());
        contact
    }

    /// One round of the daemon over the book, ended as the daemon ends it.
    fn dial_round(site: &FakeDials, contact: &mut Contact, neighbourhood: &Neighbourhood) {
        let mut reached = BTreeSet::new();
        dial_book(
            site,
            contact,
            neighbourhood,
            &mut reached,
            |_| false,
            account().user_id(),
            id(ME),
        );
        contact.round_reached(&reached, Instant::now());
    }

    #[test]
    fn red_team_a_relay_address_with_no_known_relay_is_never_dialled() {
        // A `relay:` address is a machine parked behind another one; with no
        // address for that relay in the book, dialling it is a connect to a
        // string that is no address -- a timeout at every round.
        let mut contact = listed(&[
            (
                id(2),
                format!("{}{}", itsanas_node::contact::RELAY_PREFIX, id(9)),
            ),
            (id(3), "203.0.113.3:9797".to_owned()),
        ]);
        let site = FakeDials::default();
        dial_round(&site, &mut contact, &Neighbourhood::new());
        assert_eq!(
            site.dialled(),
            vec![(id(3), "203.0.113.3:9797".to_owned())],
            "a machine behind a relay nobody knows was dialled: a timeout spent every round"
        );
    }

    #[test]
    fn red_team_a_machine_a_sibling_relays_is_dialled_next_round_and_a_forged_one_never() {
        // The Pi moved while the coordinator was down; the laptop reached it
        // since and says where. A row that fails its signatures says where
        // somebody wants this machine to dial instead, and must not be heard.
        let laptop = claimed(2, "203.0.113.2:9797", 100);
        let pi = claimed(3, "203.0.113.3:9797", 100);
        let mac = claimed(4, "203.0.113.4:9797", 100);
        let mut contact = book_of(&[laptop, pi.clone(), mac]);
        let moved = claimed(3, "198.51.100.3:9797", 200);
        let mut forged = claimed(4, "198.51.100.4:9797", 200);
        forged.presence.presence.address = "198.51.100.66:9797".to_owned();
        let rows = [moved, forged]
            .iter()
            .map(|row| postcard::to_stdvec(row).expect("encode"))
            .collect();
        let neighbourhood = Neighbourhood::new();

        let site = FakeDials::answering(&[
            ("203.0.113.2:9797", (true, Some(rows))),
            ("203.0.113.3:9797", (true, None)),
            ("203.0.113.4:9797", (true, None)),
        ]);
        dial_round(&site, &mut contact, &neighbourhood);
        let first = site.dialled();
        // The Pi and the Mac have gone quiet at the addresses that worked.
        site.answer(&[
            ("203.0.113.2:9797", (true, None)),
            ("198.51.100.3:9797", (true, None)),
        ]);
        dial_round(&site, &mut contact, &neighbourhood);
        let second = site.dialled();

        assert!(
            second.contains(&(device_of(&pi), "198.51.100.3:9797".to_owned())),
            "the address a sibling relayed was never dialled: a machine that moved stays \
             unsynced until the coordinator is back ({second:?})"
        );
        assert!(
            !first
                .iter()
                .chain(&second)
                .any(|(_, address)| address == "198.51.100.66:9797"),
            "an address the Mac never signed was dialled: a sibling chooses where this machine \
             sends the account's data ({first:?}, {second:?})"
        );
    }

    #[test]
    fn red_team_a_machine_that_did_not_answer_waits_before_it_is_dialled_again() {
        // Most of an account is off most of the time: a laptop in a bag
        // dialled at every round is a connect timeout every round.
        let mut contact = listed(&[(id(2), "203.0.113.2:9797".to_owned())]);
        let site = FakeDials::default();
        let neighbourhood = Neighbourhood::new();
        dial_round(&site, &mut contact, &neighbourhood);
        assert_eq!(
            site.dialled().len(),
            1,
            "the machine was not dialled at all"
        );
        dial_round(&site, &mut contact, &neighbourhood);
        assert_eq!(
            site.dialled(),
            vec![],
            "a machine that just missed was dialled again at once: a timeout every round for \
             every machine that is off (QUIET_AFTER_MISS)"
        );
    }

    #[test]
    fn red_team_answering_is_not_trust() {
        // A device key is a free keypair: a machine that only answered could
        // be anybody's, and confirmed it would hold a neighbourhood place a
        // flood of strangers cannot take back.
        let mut contact = listed(&[
            (id(2), "203.0.113.2:9797".to_owned()),
            (id(3), "203.0.113.3:9797".to_owned()),
        ]);
        let site = FakeDials::answering(&[
            ("203.0.113.2:9797", (false, None)),
            ("203.0.113.3:9797", (true, None)),
        ]);
        let neighbourhood = Neighbourhood::new();
        dial_round(&site, &mut contact, &neighbourhood);
        assert!(
            neighbourhood.is_confirmed(&id(3)),
            "a machine that earned trust was not confirmed"
        );
        assert!(
            !neighbourhood.is_confirmed(&id(2)),
            "a machine that only answered was confirmed: a minted key keeps its place against \
             the account's real machines"
        );
    }

    #[test]
    fn a_device_reached_by_its_first_address_is_not_dialled_at_its_second() {
        // One sync a round per machine: a second one at its other address
        // is the same work again, and a timeout if that address is stale.
        let mut contact = listed(&[
            (id(2), "203.0.113.2:9797".to_owned()),
            (id(2), "198.51.100.2:9797".to_owned()),
        ]);
        let site = FakeDials::answering(&[
            ("203.0.113.2:9797", (true, None)),
            ("198.51.100.2:9797", (true, None)),
        ]);
        dial_round(&site, &mut contact, &Neighbourhood::new());
        assert_eq!(
            site.dialled().len(),
            1,
            "a machine already synced this round was dialled again at another address"
        );
    }

    #[test]
    fn a_machine_parked_on_a_relay_is_dialled_through_that_relay() {
        // A machine behind NAT parks on a relay (§8 0v) and the book lists
        // it as `relay:<relay>`: dialled at that string it is a connect
        // failure every round, never a sync. The relay is reached at its own
        // address from this book when it is a machine of the account, else
        // at the one the address carries (a host of another account).
        let prefix = itsanas_node::contact::RELAY_PREFIX;
        let mut contact = listed(&[
            (id(9), "203.0.113.9:9797".to_owned()),
            (id(2), format!("{prefix}{}", id(9))),
            (id(3), format!("{prefix}{}@198.51.100.8:9797", id(8))),
        ]);
        let site = FakeDials::default();
        dial_round(&site, &mut contact, &Neighbourhood::new());
        let dials = site.dialled_through();
        assert!(
            dials.contains(&(
                id(2),
                format!("{prefix}{}", id(9)),
                Some((id(9), "203.0.113.9:9797".to_owned()))
            )),
            "a machine parked on a relay of the account was not dialled through it: it stays \
             unsynced for as long as it sits behind its NAT ({dials:?})"
        );
        assert!(
            dials.contains(&(
                id(3),
                format!("{prefix}{}@198.51.100.8:9797", id(8)),
                Some((id(8), "198.51.100.8:9797".to_owned()))
            )),
            "a machine parked on another account's host was not dialled through the address \
             its relay address carries: it stays unsynced ({dials:?})"
        );
    }

    #[test]
    fn an_address_that_answered_is_dialled_first_the_next_round() {
        // A stale address listed first costs a connect timeout every round
        // before the one that works, unless what answered goes first.
        let mut contact = listed(&[
            (id(2), "203.0.113.2:9797".to_owned()),
            (id(2), "198.51.100.2:9797".to_owned()),
        ]);
        let neighbourhood = Neighbourhood::new();
        let site = FakeDials::answering(&[("198.51.100.2:9797", (true, None))]);
        dial_round(&site, &mut contact, &neighbourhood);
        assert_eq!(
            site.dialled(),
            vec![
                (id(2), "203.0.113.2:9797".to_owned()),
                (id(2), "198.51.100.2:9797".to_owned())
            ],
            "the first round did not try the book's addresses in its order"
        );
        site.answer(&[
            ("203.0.113.2:9797", (true, None)),
            ("198.51.100.2:9797", (true, None)),
        ]);
        dial_round(&site, &mut contact, &neighbourhood);
        assert_eq!(
            site.dialled(),
            vec![(id(2), "198.51.100.2:9797".to_owned())],
            "the address that answered was not dialled first the next round: a stale address \
             costs a connect timeout every round before the one that works"
        );
    }

    #[test]
    fn a_private_address_that_answered_is_dialled_between_probe_rounds() {
        // A machine over a VPN or on another VLAN is not heard by discovery
        // here, so its 10.x address is dialled only on probe rounds -- one
        // in PRIVATE_PROBE_EVERY -- unless it has answered: then it syncs
        // every round, not once every six.
        let mut contact = listed(&[(id(2), "10.0.0.2:9797".to_owned())]);
        let neighbourhood = Neighbourhood::new();
        // Discovery has listened long enough that not hearing the machine
        // counts (`Neighbourhood::heard_here`).
        neighbourhood.listening_since(
            Instant::now()
                .checked_sub(crate::discovery::WARM_UP + Duration::from_secs(1))
                .expect("an instant before the warm-up"),
        );
        let site = FakeDials::answering(&[("10.0.0.2:9797", (true, None))]);
        dial_round(&site, &mut contact, &neighbourhood);
        assert_eq!(
            site.dialled().len(),
            1,
            "the first round, a probe round, did not dial the private address"
        );
        dial_round(&site, &mut contact, &neighbourhood);
        assert_eq!(
            site.dialled(),
            vec![(id(2), "10.0.0.2:9797".to_owned())],
            "a private address that answered was not dialled off a probe round: a machine \
             reached over a VPN syncs once every six rounds"
        );
    }

    #[test]
    fn a_pause_stops_the_book_dials_after_the_machine_under_way() {
        // `itsanas pause` is the person taking the bandwidth back, and a
        // shutdown waits for the round: each further machine is a session
        // more of both.
        let mut contact = listed(&[
            (id(2), "203.0.113.2:9797".to_owned()),
            (id(3), "203.0.113.3:9797".to_owned()),
            (id(4), "203.0.113.4:9797".to_owned()),
        ]);
        let site = FakeDials::answering(&[
            ("203.0.113.2:9797", (true, None)),
            ("203.0.113.3:9797", (true, None)),
            ("203.0.113.4:9797", (true, None)),
        ]);
        site.halt_after.set(Some(1));
        dial_round(&site, &mut contact, &Neighbourhood::new());
        assert_eq!(
            site.dialled().len(),
            1,
            "machines were still dialled after a pause or a shutdown: the bandwidth paused \
             for is used anyway, and a shutdown waits a session per machine"
        );
    }
}
