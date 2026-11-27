//! Which address of which device a round dials, in what order: what worked
//! first, a device that answered nowhere left quiet for a while, and never this
//! node's own listener or another house's private address.

use std::time::Instant;

use itsanas_crypto::DeviceId;

use super::{Candidate, Contact, PRIVATE_PROBE_EVERY, Quiet, quiet_for};
use crate::coordinator::is_private_address;

impl Contact {
    /// Record that a sync with `device` at `address` completed.
    ///
    /// Only for a device the coordinator listed: a device found on the LAN or
    /// typed into the configuration has its own route and is not this book's
    /// business.
    ///
    /// `now_unix` is this machine's clock, and is only ever compared with
    /// other readings of it. Not even that is trusted to go forward: a
    /// Raspberry Pi with no real-time clock reads 1970 until NTP answers, and
    /// a success recorded then would rank below a stale address recorded
    /// yesterday, costing a connect timeout every round. So a success is
    /// recorded as later than every success already in the book, whatever the
    /// clock says.
    pub fn worked(&mut self, device: DeviceId, address: &str, now_unix: u64) {
        let latest = self
            .book
            .values()
            .flatten()
            .filter_map(|candidate| candidate.worked)
            .max();
        let at = latest.map_or(now_unix, |latest| now_unix.max(latest.saturating_add(1)));
        self.quiet.remove(&device);
        if let Some(candidates) = self.book.get_mut(&device)
            && let Some(candidate) = candidates.iter_mut().find(|c| c.address == address)
        {
            candidate.worked = Some(at);
            candidate.proven = true;
            self.changed = true;
        }
    }

    /// Record that `device` did not answer at `address` this round: a private
    /// address that had answered goes back to being dialled only as
    /// [`Self::dialable`] allows a machine not heard here, and the device
    /// waits before its next dial ([`QUIET_AFTER_MISS`]) unless it answers
    /// at another address this round.
    ///
    /// [`QUIET_AFTER_MISS`]: super::QUIET_AFTER_MISS
    pub fn failed(&mut self, device: DeviceId, address: &str, now: Instant) {
        let Some(candidate) = self
            .book
            .get_mut(&device)
            .and_then(|candidates| candidates.iter_mut().find(|c| c.address == address))
        else {
            return;
        };
        candidate.proven = false;
        let round = self.rounds;
        let quiet = self.quiet.entry(device).or_insert(Quiet {
            misses: 0,
            round,
            until: now,
            news: false,
        });
        quiet.news = false;
        if quiet.misses == 0 || quiet.round != round {
            quiet.misses = quiet.misses.saturating_add(1);
            quiet.round = round;
            quiet.until = now + quiet_for(quiet.misses);
        }
    }

    /// Every address known for the account's other devices, in dialling order:
    /// for each device, the one that last worked first, then the ones that can
    /// work from anywhere before the private ones.
    #[must_use]
    pub fn candidates(&self) -> Vec<(DeviceId, String)> {
        self.ordered()
            .into_iter()
            .map(|(device, candidate)| (device, candidate.address))
            .collect()
    }

    /// Every candidate, in [`Self::candidates`]'s order.
    fn ordered(&self) -> Vec<(DeviceId, Candidate)> {
        let mut out = Vec::new();
        for (device, candidates) in &self.book {
            let mut ordered = candidates.clone();
            sort_best_first(&mut ordered);
            out.extend(ordered.into_iter().map(|candidate| (*device, candidate)));
        }
        out
    }

    /// [`Self::candidates`], less the addresses on a private network (RFC 1918)
    /// of a machine that is plausibly not on this one: `heard_here` says
    /// whether local discovery heard the device, and `None` -- discovery not
    /// running here -- keeps every address, as before.
    ///
    /// The hand test of 2026-10-22: two houses both numbered 192.168.1.x, and
    /// the laptop in one and a Mac in the other dialled each other's
    /// 192.168.1.x address from the coordinator at every round, each dial a
    /// connect timeout. A private address is somebody's own network; the
    /// coordinator, or a peer relaying it, says where a machine is on *its*
    /// network, not that this machine shares it. The evidence that it does is
    /// a beacon heard here. (Both machines having announced from the same
    /// public address would be evidence too, but the coordinator does not
    /// record the address it sees a publication come from.)
    ///
    /// Not hearing a machine is weak evidence, though (red team,
    /// 2026-10-23): another VLAN, a VPN numbered 10.x, a Wi-Fi that filters
    /// multicast, and the first minute of discovery all hide a machine this
    /// one can reach. So that gate has three doors: `heard_here` answers `None`
    /// while discovery warms up (`Neighbourhood::heard_here`); one round in
    /// [`PRIVATE_PROBE_EVERY`] -- the first, then every sixth -- dials every
    /// private address anyway; and an address that answered
    /// ([`Self::worked`]) is dialled at every round until it fails
    /// ([`Self::failed`]). Another house's 192.168.1.x thus costs one connect
    /// timeout every sixth round, not one every round.
    ///
    /// A machine left with no address to dial is missed like any other
    /// ([`Self::round_reached`]): the coordinator is asked again, at the
    /// backed-off rate, in case it moved somewhere this machine can reach.
    ///
    /// And never a device still in its wait after answering nowhere
    /// ([`QUIET_AFTER_MISS`], [`Self::failed`]) at `now`, unless discovery
    /// hears it here: a machine that is off costs a dial every half hour at
    /// most, not one at every round.
    ///
    /// Never an address `is_me` says is this node ([`is_this_listener`]):
    /// whatever device the book names there, this node is what answers. Hand
    /// test 2026-10-22: a USB key copied from the laptop had enrolled at the
    /// laptop's own `192.168.1.142:9797`, and the laptop's node dialled
    /// itself for the key at every round, 376 times in one log.
    ///
    /// [`QUIET_AFTER_MISS`]: super::QUIET_AFTER_MISS
    #[must_use]
    pub fn dialable(
        &self,
        heard_here: impl Fn(&DeviceId) -> Option<bool>,
        is_me: impl Fn(&str) -> bool,
        now: Instant,
    ) -> Vec<(DeviceId, String)> {
        let probe = self.rounds.is_multiple_of(PRIVATE_PROBE_EVERY);
        self.ordered()
            .into_iter()
            .filter(|(_, candidate)| !is_me(&candidate.address))
            .filter(|(device, _)| {
                heard_here(device) == Some(true)
                    || self
                        .quiet
                        .get(device)
                        .is_none_or(|quiet| quiet.news || now >= quiet.until)
            })
            .filter(|(device, candidate)| {
                probe
                    || candidate.proven
                    || !only_rfc1918(&candidate.address)
                    || heard_here(device) != Some(false)
            })
            .map(|(device, candidate)| (device, candidate.address))
            .collect()
    }
}

/// Worked most recently first, then never-worked public before never-worked
/// private. Stable, so ties keep the order they arrived in.
/// Whether every entry of `address` (one address, or an `AddressList`) is an
/// IPv4 address of a private network, RFC 1918. A name, a public or an IPv6
/// address, a `relay:` -- anything else -- is not.
/// Whether `address` reaches this node: the address it publishes itself
/// (`published`, compared as written), or a list whose every entry is one of
/// this machine's addresses (`here`) at this node's own `port`. The same
/// machine at another port is another node -- a second account on the same
/// laptop -- and is dialled. A name is never this node by this test: it would
/// take a resolution, and a stale name costs what any unreachable one does.
#[must_use]
pub fn is_this_listener(
    address: &str,
    port: u16,
    here: &itsanas_discover::LocalNets,
    published: &[&str],
) -> bool {
    if published.contains(&address) {
        return true;
    }
    let entries: Vec<std::net::SocketAddr> = address
        .split(',')
        .map(|entry| entry.trim().parse())
        .collect::<std::result::Result<_, _>>()
        .unwrap_or_default();
    !entries.is_empty()
        && entries
            .iter()
            .all(|entry| entry.port() == port && here.holds(entry.ip()))
}

fn only_rfc1918(address: &str) -> bool {
    let entries: Vec<std::net::SocketAddr> = address
        .split(',')
        .map(|entry| entry.trim().parse())
        .collect::<std::result::Result<_, _>>()
        .unwrap_or_default();
    !entries.is_empty()
        && entries
            .iter()
            .all(|entry| matches!(entry.ip(), std::net::IpAddr::V4(v4) if v4.is_private()))
}

pub(super) fn sort_best_first(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.worked
            .cmp(&a.worked)
            .then_with(|| is_private_address(&a.address).cmp(&is_private_address(&b.address)))
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use crate::contact::tests::{HOME, ROUND, device, round_against};
    use crate::contact::{QUIET_AFTER_MISS, QUIET_AT_MOST};

    use super::*;

    /// The hand test of 2026-10-22: the laptop's house and the Mac's are both
    /// 192.168.1.x, and the laptop dialled the Mac's 192.168.1.x address from
    /// the coordinator at every round, timing out each time. A private address
    /// of a machine discovery did not hear is dialled only at a probe round,
    /// one in [`PRIVATE_PROBE_EVERY`]; a public address, a list with one, a
    /// name and a relay always are; and a node whose discovery is off, or
    /// warming up, dials as it always did.
    /// Hand test 2026-10-22, the laptop's `daemon.log`: 376 times, one per
    /// round over four runs, `192.168.1.142:9797: unreachable (tls: expected
    /// to reach device 14dd30325b23 but 36f669090138 answered)`. The USB key
    /// copied from the laptop had enrolled at the laptop's own address, and
    /// the laptop's node, listening there, dialled itself for the key. An
    /// address that is this node -- one of this machine's addresses at this
    /// node's own port, or what it publishes -- is never dialled, whatever
    /// device the book names. The same machine at another port is the next
    /// account on the laptop, and is.
    #[test]
    fn red_team_an_address_that_is_this_node_is_never_dialled_for_another_device() {
        let (key, second_account, pi) = (device(7), device(8), device(9));
        let mut contact = Contact::new();
        contact.read(
            &[
                (key, "192.168.1.142:9797".to_owned()),
                (key, "127.0.0.1:9797".to_owned()),
                (key, "86.238.26.80:9797".to_owned()),
                (key, "192.168.1.142:9797,[::1]:9797".to_owned()),
                (second_account, "192.168.1.142:9798".to_owned()),
                (pi, "192.168.1.11:9797".to_owned()),
                (pi, "192.168.1.11:9797,192.168.1.142:9797".to_owned()),
            ],
            &[],
            &[],
            Instant::now(),
        );
        let ip = |text: &str| text.parse::<std::net::IpAddr>().unwrap();
        let laptop = itsanas_discover::LocalNets::from_interfaces([(
            "Wi-Fi".to_owned(),
            ip("192.168.1.142"),
            ip("255.255.255.0"),
        )]);
        let published = ["86.238.26.80:9797"];
        let dialled: Vec<String> = contact
            .dialable(
                |_| Some(true),
                |address| is_this_listener(address, 9797, &laptop, &published),
                Instant::now(),
            )
            .into_iter()
            .map(|(_, address)| address)
            .collect();

        for itself in [
            "192.168.1.142:9797",
            "127.0.0.1:9797",
            "86.238.26.80:9797",
            "192.168.1.142:9797,[::1]:9797",
        ] {
            assert!(
                !dialled.iter().any(|address| address == itself),
                "{itself} is this node, and it was dialled for another device: the node \
                 dials itself at every round and is refused by its own pinning"
            );
        }
        for other in [
            "192.168.1.142:9798",
            "192.168.1.11:9797",
            "192.168.1.11:9797,192.168.1.142:9797",
        ] {
            assert!(
                dialled.iter().any(|address| address == other),
                "{other} is another node (another account on this machine, or a list with \
                 another machine in it) and was not dialled: {dialled:?}"
            );
        }
    }

    #[test]
    fn red_team_a_private_address_of_another_house_is_not_dialled() {
        let (mac, pi) = (device(5), device(6));
        let mut contact = Contact::new();
        contact.read(
            &[
                (mac, "192.168.1.20:9797".to_owned()),
                (mac, "10.0.0.20:9797".to_owned()),
                (mac, "172.16.4.20:9797".to_owned()),
                (pi, "192.168.1.30:9797".to_owned()),
                (pi, "203.0.113.30:9797".to_owned()),
                (pi, "192.168.1.30:9797,[2001:db8::30]:9797".to_owned()),
                (pi, "pi.example.net:9797".to_owned()),
            ],
            &[],
            &[],
            Instant::now(),
        );
        let private = [
            "192.168.1.20:9797",
            "10.0.0.20:9797",
            "172.16.4.20:9797",
            "192.168.1.30:9797",
        ];
        let start = Instant::now();
        let at = |round: u32| start + ROUND * round;
        let addresses = |contact: &Contact, heard: Option<bool>, now: Instant| -> Vec<String> {
            contact
                .dialable(|_| heard, |_| false, now)
                .into_iter()
                .map(|(_, address)| address)
                .collect()
        };

        // Two hours of five-minute rounds, nobody answering at a private
        // address: the other house is dialled at the probe rounds only. The
        // Pi answers at its public address, so it is never in the wait for
        // a machine that is off (`QUIET_AFTER_MISS`).
        let mut probed = Vec::new();
        for round in 0..24u32 {
            let elsewhere = addresses(&contact, Some(false), at(round));
            if elsewhere
                .iter()
                .any(|address| private.contains(&address.as_str()))
            {
                probed.push(round);
            }
            for kept in [
                "203.0.113.30:9797",
                "192.168.1.30:9797,[2001:db8::30]:9797",
                "pi.example.net:9797",
            ] {
                assert!(
                    elsewhere.iter().any(|address| address == kept),
                    "round {round}: {kept} was not dialled"
                );
            }
            for address in private
                .iter()
                .filter(|address| elsewhere.iter().any(|dialled| dialled == *address))
            {
                let device = if address.ends_with(".30:9797") {
                    pi
                } else {
                    mac
                };
                contact.failed(device, address, at(round));
            }
            contact.worked(pi, "203.0.113.30:9797", 1_000);
            contact.round_reached(&BTreeSet::from([pi]), at(round));
        }
        assert_eq!(
            probed,
            [0, 6, 12, 18],
            "a private address of a machine discovery never heard here was dialled at \
             rounds {probed:?} of 24, not one in {PRIVATE_PROBE_EVERY}"
        );
        assert_eq!(
            addresses(&contact, Some(true), at(24)).len(),
            7,
            "a machine heard on this LAN lost an address"
        );
        assert_eq!(
            addresses(&contact, None, at(24)).len(),
            7,
            "with discovery off or warming up, an address was dropped"
        );
    }

    /// The other side of the gate (red team, 2026-10-23): a NAS at 10.0.2.5
    /// on another VLAN of the same house, or behind a VPN, is never heard by
    /// discovery and is reachable all the same. It is dialled while discovery
    /// warms up, at every probe round after that, and -- once a probe has
    /// reached it -- at every round, until it fails.
    #[test]
    fn red_team_a_machine_on_another_vlan_is_still_dialled() {
        let nas = device(7);
        let address = "10.0.2.5:9797";
        let mut contact = Contact::new();
        let start = Instant::now();
        contact.read(&[(nas, address.to_owned())], &[], &[], start);
        // Past the first round, which is a probe round of its own.
        contact.round_reached(&BTreeSet::new(), start);
        let at = |round: u32| start + ROUND * round;
        let dialled = |contact: &Contact, heard: Option<bool>, now: Instant| {
            contact
                .dialable(|_| heard, |_| false, now)
                .iter()
                .any(|(device, at)| *device == nas && at == address)
        };

        assert!(
            dialled(&contact, None, at(1)),
            "the VLAN's NAS was not dialled while discovery warmed up"
        );

        // Discovery warmed up and never hears it; nothing answers.
        let mut probes = Vec::new();
        for round in 1..13u32 {
            if dialled(&contact, Some(false), at(round)) {
                probes.push(round);
                contact.failed(nas, address, at(round));
            }
            contact.round_reached(&BTreeSet::new(), start + ROUND * round);
        }
        assert_eq!(
            probes,
            [6, 12],
            "the VLAN's NAS was dialled at rounds {probes:?}, not every \
             {PRIVATE_PROBE_EVERY}th"
        );

        // The probe of round 12 answered: dialled at every round from then on.
        contact.worked(nas, address, 1_000);
        for round in 13..20u32 {
            assert!(
                dialled(&contact, Some(false), at(round)),
                "round {round}: the NAS answered and was not dialled again"
            );
            contact.round_reached(&BTreeSet::from([nas]), start + ROUND * round);
        }
        // Until it fails: then back to the probes.
        contact.failed(nas, address, at(20));
        contact.round_reached(&BTreeSet::new(), at(20));
        assert!(
            !dialled(&contact, Some(false), at(21)),
            "an address that stopped answering was still dialled at every round"
        );
    }

    /// One day of rounds against a machine of the account that answers
    /// nowhere, dialled as the daemon dials it: the rounds at which
    /// [`Contact::dialable`] offered it, each followed by a failure.
    fn dials_to_a_machine_that_is_off(every: Duration, rounds: u32) -> Vec<u32> {
        let laptop = device(2);
        let address = "86.238.26.80:9811,[2001:db8::2]:9811";
        let mut contact = Contact::new();
        let start = Instant::now();
        contact.read(&[(laptop, address.to_owned())], &[], &[], start);
        let mut dialled = Vec::new();
        for round in 0..rounds {
            let now = start + every * round;
            if contact
                .dialable(|_| Some(false), |_| false, now)
                .iter()
                .any(|(device, _)| *device == laptop)
            {
                dialled.push(round);
                contact.failed(laptop, address, now);
            }
            contact.round_reached(&BTreeSet::new(), now);
        }
        dialled
    }

    /// Fleet run 40478381966: the runner dialled the laptop's `test` node,
    /// shut in Nicolas's house, at every one of its 30 s rounds -- 31 connect
    /// timeouts of five seconds and 31 refusals from the relay it had once
    /// parked on, in twenty minutes. At the default five minutes that is 288
    /// timeouts a day for every machine of the account that is off, and most
    /// of them are, most of the time. A machine that answered nowhere waits
    /// [`QUIET_AFTER_MISS`], doubled at each miss, before it is dialled
    /// again, up to [`QUIET_AT_MOST`]: about fifty dials a day whatever the
    /// round, and never more than half an hour without one, so every hour of
    /// its ring is still an hour tried (`placing::unreached`).
    #[test]
    fn red_team_a_machine_that_stays_off_is_dialled_about_fifty_times_a_day_not_288() {
        let day = Duration::from_secs(24 * 3600);
        for every in [Duration::from_secs(30), ROUND] {
            let rounds = u32::try_from(day.as_secs() / every.as_secs()).expect("rounds");
            let dialled = dials_to_a_machine_that_is_off(every, rounds);
            assert!(
                dialled.len() <= 60,
                "at a round every {every:?}, a machine that stays off for a day was dialled \
                 {} times: a connect timeout at nearly every round",
                dialled.len()
            );
            let longest = dialled
                .windows(2)
                .map(|pair| every * (pair[1] - pair[0]))
                .max()
                .expect("dialled more than once");
            assert!(
                longest <= QUIET_AT_MOST + every,
                "at a round every {every:?}, {longest:?} passed without a dial: an hour of the \
                 machine's ring went untried, and a machine back on is found that much later"
            );
        }
    }

    /// A machine dialled at three addresses in one round missed once, not
    /// three times: counted per address, a laptop with a LAN address, a public
    /// one and a relay waited four minutes after its first miss instead of
    /// one, and reached the half hour three times faster.
    #[test]
    fn a_machine_that_fails_at_three_addresses_in_one_round_missed_once() {
        let laptop = device(2);
        let addresses = ["192.168.1.2:9811", "86.238.26.80:9811", "203.0.113.2:9811"];
        let start = Instant::now();
        let mut contact = Contact::new();
        contact.read(
            &addresses.map(|address| (laptop, address.to_owned())),
            &[],
            &[],
            start,
        );
        for address in addresses {
            contact.failed(laptop, address, start);
        }
        contact.round_reached(&BTreeSet::new(), start);
        assert!(
            contact
                .dialable(|_| None, |_| false, start + QUIET_AFTER_MISS)
                .iter()
                .any(|(device, _)| *device == laptop),
            "one round's failures at three addresses counted as three misses"
        );
    }

    /// The way back from the wait above: whatever says the machine is on
    /// again -- reached at another address or on the LAN, heard by discovery,
    /// or given an address the book did not hold -- has it dialled at the
    /// next round, not half an hour later.
    #[test]
    fn red_team_a_machine_back_on_is_dialled_at_once() {
        let laptop = device(2);
        let address = "86.238.26.80:9811";
        let start = Instant::now();
        let off_for_an_hour = || {
            let mut contact = Contact::new();
            contact.read(&[(laptop, address.to_owned())], &[], &[], start);
            for round in 0..12u32 {
                let now = start + ROUND * round;
                if !contact.dialable(|_| Some(false), |_| false, now).is_empty() {
                    contact.failed(laptop, address, now);
                }
                contact.round_reached(&BTreeSet::new(), now);
            }
            contact
        };
        let next = start + ROUND * 12;
        let offered = |contact: &Contact, heard: Option<bool>| {
            contact
                .dialable(|_| heard, |_| false, next)
                .iter()
                .any(|(device, _)| *device == laptop)
        };

        let contact = off_for_an_hour();
        assert!(
            !offered(&contact, Some(false)),
            "the premise: an hour off, the laptop is in its wait"
        );
        assert!(
            offered(&contact, Some(true)),
            "discovery hears the laptop on this LAN and it was not dialled"
        );

        let mut contact = off_for_an_hour();
        contact.round_reached(&BTreeSet::from([laptop]), next);
        assert!(
            offered(&contact, Some(false)),
            "the laptop answered this round, on the LAN or as a configured peer, and its \
             book address was still left in the wait"
        );

        let mut contact = off_for_an_hour();
        contact.worked(laptop, address, 1_000);
        assert!(
            offered(&contact, Some(false)),
            "the laptop answered at its address and was left in the wait"
        );

        let mut contact = off_for_an_hour();
        contact.read(
            &[
                (laptop, address.to_owned()),
                (laptop, "203.0.113.2:9811".to_owned()),
            ],
            &[],
            &[],
            next,
        );
        assert!(
            offered(&contact, Some(false)),
            "the coordinator gave the laptop an address the book did not hold -- it moved -- \
             and it was not dialled there"
        );

        let mut contact = off_for_an_hour();
        contact.read(&[(laptop, address.to_owned())], &[], &[], next);
        assert!(
            !offered(&contact, Some(false)),
            "a read that repeats the address the laptop is off at is no news, and it emptied \
             the wait: a coordinator read every few minutes would dial it every round"
        );
    }

    /// The fall-back to two minutes is somebody else's word (red team,
    /// 2026-10-23): a machine of the account answering one round in two, and
    /// a coordinator listing a new port at every read, each put the wait back
    /// to [`REREAD_AFTER_MISS`] and held this node at a read every two minutes
    /// -- 720 a day at a round every 30 s. Floored to one fall-back per
    /// [`RESET_FLOOR`], the doubling wins: never more than 432 a day at
    /// 30 s rounds (three reads per ten minutes: the fall-back's, two and
    /// six minutes after it), and no more than the 288 rounds of a day at
    /// the default five minutes.
    #[test]
    fn red_team_a_machine_toggling_every_round_cannot_hold_the_wait_at_two_minutes() {
        let day = Duration::from_secs(24 * 3600);
        let laptop = device(2);
        for (every, most) in [(Duration::from_secs(30), 432), (ROUND, 288)] {
            let mut contact = Contact::new();
            let start = Instant::now();
            contact.published(Some(HOME), start);
            contact.read(&[(laptop, "192.168.1.2:9000".to_owned())], &[], &[], start);
            let rounds = u32::try_from(day.as_secs() / every.as_secs()).expect("rounds");
            let reads = (1..rounds)
                .filter(|&round| {
                    // An answer one round in two at its address, and a new
                    // port beside it at every read.
                    let home = "192.168.1.2:9000";
                    let listed = [
                        (laptop, home.to_owned()),
                        (laptop, format!("192.168.1.2:{}", 9000 + round)),
                    ];
                    let live: &[&str] = if round % 2 == 0 { &[home] } else { &[] };
                    round_against(&mut contact, &listed, true, live, start + every * round).0
                })
                .count();
            assert!(
                reads <= most,
                "at a round every {every:?}, a machine toggling every round cost {reads} reads \
                 in a day; the floor allows at most {most}"
            );
        }
    }
}
