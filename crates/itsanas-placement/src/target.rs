//! When a chunk has enough copies, and where the next ones should go.
//!
//! A chunk is **satisfied** when it is both *durable* (copies in at least
//! [`Goal::accounts`] other accounts, so one person deleting everything or one
//! house burning loses nothing) and *available* (the chance that no copy is
//! reachable at a given hour is at most [`Goal::miss_ppm`]).
//!
//! What this module refuses to do:
//!
//! - **Count this account's own devices for durability.** They are one house:
//!   right for availability, wrong for durability. They count for availability
//!   as **one** unit, since they tend to be off together (one router, one
//!   power cut).
//! - **Count what is not proven.** A holder that never passed an audit, or is
//!   paused or draining, satisfies nothing: otherwise a host that accepts
//!   chunks and discards them would make every chunk look safe.
//! - **Count throwaway devices as accounts.** Every device of an unknown
//!   account is the *same* [`Account::Unknown`]: ten fresh identities are one
//!   stranger, not ten.
//! - **Count one network as many.** Proven holders last seen from one
//!   [`Network`] count for availability as one unit, as good as its best
//!   member: one router, one power cut. For durability, accounts seen from
//!   one network are one household (Nicolas, 2026-10-19). A holder with no
//!   known network is a unit of its own.
//! - **Search without end.** [`Goal::cap`] remote copies is where the answer
//!   becomes "short" instead of a thirtieth copy.

use crate::liveness::lost_after_hours;
use crate::network::Network;
use crate::uptime::{Unit, Uptime, miss_ppm};
use itsanas_crypto::{DeviceId, UserId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a chunk should reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Goal {
    /// Distinct other accounts holding proven copies.
    pub accounts: usize,
    /// Highest acceptable P(no copy reachable), in ppm.
    pub miss_ppm: u32,
    /// Most remote copies the search will ask for.
    pub cap: usize,
}

/// Two other accounts, 99 % reachable, twelve remote copies at most: with hosts
/// up 30 % of the time, 99 % takes thirteen, so twelve is where it stops.
pub const GOAL: Goal = Goal {
    accounts: 2,
    miss_ppm: 10_000,
    cap: 12,
};

/// Whose a holder is, as far as this node knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Account {
    /// An account a claim names.
    Known(UserId),
    /// Every device whose account is not known: all of them are one account.
    Unknown,
}

/// One device holding (or about to hold) a copy.
#[derive(Clone, Copy, Debug)]
pub struct Holding<'a> {
    pub device: DeviceId,
    pub account: Account,
    /// One of this account's own devices (never this node itself: the caller
    /// leaves it out, because a copy here is not a copy anywhere else).
    pub own: bool,
    pub uptime: Option<&'a Uptime>,
    /// Passed an audit, not paused, not draining.
    pub proven: bool,
    /// Seen within the store's `LIVE_FOR`.
    pub recent: bool,
    /// Where this node last saw it from, if lately and directly.
    pub network: Option<Network>,
}

/// How a chunk stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub durable: bool,
    pub available: bool,
    pub miss_ppm: u32,
    /// Independent households among proven off-account holders: distinct
    /// accounts, merged when they were seen from one network.
    pub accounts: usize,
    /// Off-account holders, proven or not.
    pub remote: usize,
    pub capped: bool,
}

impl Verdict {
    #[must_use]
    pub const fn satisfied(&self) -> bool {
        self.durable && self.available
    }
}

/// Assess a chunk from its holdings (this node excluded).
///
/// durable = ≥ `goal.accounts` distinct accounts among proven off-account
/// holders, at least one of them recent. available = `miss_ppm` over one unit
/// per proven off-account holder with no known network, one unit per network
/// among the others (its best member), and one union unit for every own device ≤
/// `goal.miss_ppm`. capped = remote ≥ `goal.cap`.
#[must_use]
pub fn assess(holdings: &[Holding<'_>], goal: &Goal, now_hour: u64) -> Verdict {
    let proven: Vec<&Holding<'_>> = holdings
        .iter()
        .filter(|holding| !holding.own && holding.proven)
        .collect();
    let accounts = households(&proven);
    let durable = accounts >= goal.accounts && proven.iter().any(|holding| holding.recent);

    let own_rings: Vec<&Uptime> = holdings
        .iter()
        .filter(|holding| holding.own)
        .filter_map(|holding| holding.uptime)
        .collect();
    let any_own = holdings.iter().any(|holding| holding.own);
    let own_union = Uptime::union(&own_rings);

    let mut alone: Vec<Option<&Uptime>> = Vec::new();
    let mut by_network: BTreeMap<Network, Vec<Option<&Uptime>>> = BTreeMap::new();
    for holding in &proven {
        match holding.network {
            Some(network) => by_network.entry(network).or_default().push(holding.uptime),
            None => alone.push(holding.uptime),
        }
    }
    // A network is as good as its best member, no better: it is assumed to go
    // dark at once. Not the union of its members, as for own devices: the
    // union of two hosts behind one router is up whenever either is, which
    // is exactly the independence a shared network is evidence against.
    let unions: Vec<Option<&Uptime>> = by_network
        .values()
        .map(|members| {
            members.iter().flatten().copied().max_by_key(|ring| {
                ring.availability(now_hour, lost_after_hours(Some(ring)))
                    .unwrap_or(0)
            })
        })
        .collect();
    let mut units: Vec<Unit<'_>> = alone
        .iter()
        .map(|uptime| Unit {
            uptime: *uptime,
            lost_after_hours: lost_after_hours(*uptime),
        })
        .collect();
    units.extend(unions.iter().map(|best| Unit {
        uptime: *best,
        lost_after_hours: lost_after_hours(*best),
    }));
    if any_own {
        units.push(Unit {
            uptime: own_union.as_ref(),
            lost_after_hours: lost_after_hours(own_union.as_ref()),
        });
    }
    let miss = miss_ppm(&units, now_hour);

    let remote = holdings.iter().filter(|holding| !holding.own).count();
    Verdict {
        durable,
        available: miss <= goal.miss_ppm,
        miss_ppm: miss,
        accounts,
        remote,
        capped: remote >= goal.cap,
    }
}

/// Union-find root, halving the path on the way.
fn root(parent: &mut [usize], mut at: usize) -> usize {
    while parent[at] != at {
        parent[at] = parent[parent[at]];
        at = parent[at];
    }
    at
}

/// How many independent households hold proven copies: holders are one
/// household when they share an account **or** a network (Nicolas,
/// 2026-10-19: two accounts behind one box burn, flood and get burgled
/// together). Union-find over a handful of holders; quadratic is fine at the
/// copy cap of 12.
fn households(proven: &[&Holding<'_>]) -> usize {
    let mut parent: Vec<usize> = (0..proven.len()).collect();
    for i in 0..proven.len() {
        for j in i + 1..proven.len() {
            let (a, b) = (proven[i], proven[j]);
            let shared = a.account == b.account || (a.network.is_some() && a.network == b.network);
            if shared {
                let (ra, rb) = (root(&mut parent, i), root(&mut parent, j));
                parent[ra] = rb;
            }
        }
    }
    (0..proven.len())
        .filter(|&i| root(&mut parent, i) == i)
        .count()
}

/// Which hosts to add, and whether that would be enough.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub hosts: Vec<DeviceId>,
    pub satisfied: bool,
}

/// Greedy over `ranked` (eligible hosts not holding the chunk, best first), in
/// four passes: a new account on a new network, a new account, a new network,
/// then any. Each added host counts as a proven, recent holder with its own
/// uptime. Stops when satisfied or when remote copies reach `goal.cap`.
///
/// Accounts first, because two copies with one stranger are one decision
/// away from zero; networks next, because two copies behind one router are
/// one power cut away from none reachable. A host with no known network is
/// always "new": nothing says it shares one.
#[must_use]
pub fn want(holdings: &[Holding<'_>], ranked: &[Holding<'_>], goal: &Goal, now_hour: u64) -> Want {
    let mut current: Vec<Holding<'_>> = holdings.to_vec();
    let mut hosts = Vec::new();
    let mut verdict = assess(&current, goal, now_hour);

    for (new_account, new_network) in [(true, true), (true, false), (false, true), (false, false)] {
        for candidate in ranked {
            if verdict.satisfied() || verdict.capped {
                return Want {
                    hosts,
                    satisfied: verdict.satisfied(),
                };
            }
            if current
                .iter()
                .any(|holding| holding.device == candidate.device)
            {
                continue;
            }
            // A new account behind a router that already holds a copy is
            // not a new household.
            let represented = current.iter().any(|holding| {
                !holding.own
                    && holding.proven
                    && (holding.account == candidate.account
                        || (candidate.network.is_some() && holding.network == candidate.network))
            });
            let network_held = candidate.network.is_some()
                && current.iter().any(|holding| {
                    !holding.own && holding.proven && holding.network == candidate.network
                });
            if (new_account && represented) || (new_network && network_held) {
                continue;
            }
            current.push(Holding {
                own: false,
                proven: true,
                recent: true,
                ..*candidate
            });
            hosts.push(candidate.device);
            verdict = assess(&current, goal, now_hour);
        }
    }

    Want {
        hosts,
        satisfied: verdict.satisfied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{device, steady, user};

    const NOW: u64 = 10_000;

    fn holding(index: u16, account: Account, uptime: Option<&Uptime>) -> Holding<'_> {
        Holding {
            device: device(index),
            account,
            own: false,
            uptime,
            proven: true,
            recent: true,
            network: None,
        }
    }

    fn known(index: u8) -> Account {
        Account::Known(user(index))
    }

    #[test]
    fn two_proven_reliable_accounts_satisfy_the_goal() {
        let a = steady(NOW, 950, 0);
        let b = steady(NOW, 950, 50);
        let verdict = assess(
            &[
                holding(1, known(1), Some(&a)),
                holding(2, known(2), Some(&b)),
            ],
            &GOAL,
            NOW,
        );
        assert!(verdict.satisfied(), "{verdict:?}");
        assert_eq!(verdict.accounts, 2);
    }

    #[test]
    fn own_devices_count_as_one_unit_and_never_for_durability() {
        // Five own devices up all the time: available, never durable.
        let rings: Vec<Uptime> = (0..5).map(|_| steady(NOW, 1000, 0)).collect();
        let own: Vec<Holding<'_>> = rings
            .iter()
            .enumerate()
            .map(|(index, ring)| Holding {
                own: true,
                ..holding(u16::try_from(index).unwrap(), known(9), Some(ring))
            })
            .collect();
        let verdict = assess(&own, &GOAL, NOW);
        assert!(!verdict.durable);
        assert_eq!(verdict.accounts, 0);
        assert_eq!(verdict.remote, 0);
        assert!(verdict.available);

        // Two own devices off at the same hours are one unit: 50 % miss, not
        // 25 % as two independent units would give.
        let half_a = steady(NOW, 500, 0);
        let half_b = steady(NOW, 500, 50);
        let pair = [
            Holding {
                own: true,
                ..holding(1, known(9), Some(&half_a))
            },
            Holding {
                own: true,
                ..holding(2, known(9), Some(&half_b))
            },
        ];
        assert_eq!(
            assess(&pair, &GOAL, NOW).miss_ppm,
            0,
            "the union is always up"
        );
        let same = [
            Holding {
                own: true,
                ..holding(1, known(9), Some(&half_a))
            },
            Holding {
                own: true,
                ..holding(2, known(9), Some(&half_a))
            },
        ];
        assert_eq!(assess(&same, &GOAL, NOW).miss_ppm, 500_000);
    }

    #[test]
    fn red_team_two_throwaway_devices_of_unknown_accounts_are_one_account() {
        let a = steady(NOW, 1000, 0);
        let b = steady(NOW, 1000, 0);
        let verdict = assess(
            &[
                holding(1, Account::Unknown, Some(&a)),
                holding(2, Account::Unknown, Some(&b)),
            ],
            &GOAL,
            NOW,
        );
        assert_eq!(verdict.accounts, 1);
        assert!(!verdict.durable);
    }

    #[test]
    fn red_team_an_unproven_holder_satisfies_nothing() {
        let rings: Vec<Uptime> = (0..6).map(|_| steady(NOW, 1000, 0)).collect();
        let unproven: Vec<Holding<'_>> = rings
            .iter()
            .enumerate()
            .map(|(index, ring)| Holding {
                proven: false,
                ..holding(
                    u16::try_from(index).unwrap(),
                    known(u8::try_from(index).unwrap()),
                    Some(ring),
                )
            })
            .collect();
        let verdict = assess(&unproven, &GOAL, NOW);
        assert!(!verdict.durable);
        assert!(!verdict.available);
        assert_eq!(verdict.miss_ppm, 1_000_000);
        assert_eq!(verdict.remote, 6, "they still count towards the cap");
    }

    #[test]
    fn durability_needs_one_holder_seen_recently() {
        let a = steady(NOW, 990, 0);
        let b = steady(NOW, 990, 50);
        let stale = [
            Holding {
                recent: false,
                ..holding(1, known(1), Some(&a))
            },
            Holding {
                recent: false,
                ..holding(2, known(2), Some(&b))
            },
        ];
        assert!(!assess(&stale, &GOAL, NOW).durable);
    }

    #[test]
    fn the_cap_stops_the_search() {
        // Hosts up 10 % of the time, all at the same hours: no number of them
        // reaches 99 %, so the search must stop at the cap, not run the list.
        let rings: Vec<Uptime> = (0..40).map(|_| steady(NOW, 100, 0)).collect();
        let ranked: Vec<Holding<'_>> = rings
            .iter()
            .enumerate()
            .map(|(index, ring)| {
                holding(
                    u16::try_from(index).unwrap(),
                    known(u8::try_from(index).unwrap()),
                    Some(ring),
                )
            })
            .collect();
        let wanted = want(&[], &ranked, &GOAL, NOW);
        assert_eq!(wanted.hosts.len(), GOAL.cap);
        assert!(!wanted.satisfied);

        // A chunk already at the cap wants nobody.
        let full = want(&ranked[..GOAL.cap], &ranked[GOAL.cap..], &GOAL, NOW);
        assert_eq!(full.hosts, [] as [itsanas_crypto::DeviceId; 0]);
    }

    #[test]
    fn distinct_accounts_come_first() {
        // The three best-ranked hosts belong to one account; a worse-ranked
        // host of a second account must be chosen before the second and third.
        let rings: Vec<Uptime> = (0..4).map(|phase| steady(NOW, 990, phase * 25)).collect();
        let ranked = [
            holding(1, known(1), Some(&rings[0])),
            holding(2, known(1), Some(&rings[1])),
            holding(3, known(1), Some(&rings[2])),
            holding(4, known(2), Some(&rings[3])),
        ];
        let wanted = want(&[], &ranked, &GOAL, NOW);
        assert_eq!(wanted.hosts, vec![device(1), device(4)]);
        assert!(wanted.satisfied);
    }

    fn on<'a>(network: &str, holding: Holding<'a>) -> Holding<'a> {
        Holding {
            network: crate::Network::of(network.parse().unwrap()),
            ..holding
        }
    }

    #[test]
    fn red_team_two_holders_behind_one_router_count_as_one_for_availability() {
        // Two accounts, each up 95 % of the hours, down at different ones:
        // as independent holders, 0.25 % miss and satisfied. Seen from one
        // /24 they are one unit as good as the better of them, 5 % miss: one
        // power cut takes both, and the chunk needs a third copy elsewhere.
        let a = steady(NOW, 950, 0);
        let b = steady(NOW, 950, 50);
        let apart = [
            on("203.0.113.7", holding(1, known(1), Some(&a))),
            on("198.51.100.7", holding(2, known(2), Some(&b))),
        ];
        assert!(assess(&apart, &GOAL, NOW).satisfied(), "two networks");
        let together = [
            on("203.0.113.7", holding(1, known(1), Some(&a))),
            on("203.0.113.99", holding(2, known(2), Some(&b))),
        ];
        let verdict = assess(&together, &GOAL, NOW);
        assert!(
            !verdict.durable,
            "two accounts behind one router read as two households: {verdict:?}"
        );
        assert_eq!(verdict.accounts, 1);
        assert!(
            !verdict.available,
            "two holders on one network read as independent: {verdict:?}"
        );
        assert_eq!(verdict.miss_ppm, 50_000);
        // No network known is no evidence of sharing one.
        let unknown = [
            holding(1, known(1), Some(&a)),
            holding(2, known(2), Some(&b)),
        ];
        assert!(assess(&unknown, &GOAL, NOW).satisfied());
    }

    #[test]
    fn red_team_a_second_copy_goes_to_another_network_before_the_same_one() {
        // Host 2 is ranked before host 3, a new account either way; only
        // host 3 is off the network the first copy is on.
        let rings: Vec<Uptime> = (0..3).map(|phase| steady(NOW, 990, phase * 25)).collect();
        let held = [on("203.0.113.1", holding(1, known(1), Some(&rings[0])))];
        let ranked = [
            on("203.0.113.2", holding(2, known(2), Some(&rings[1]))),
            on("198.51.100.3", holding(3, known(3), Some(&rings[2]))),
        ];
        let wanted = want(&held, &ranked, &GOAL, NOW);
        assert_eq!(wanted.hosts, vec![device(3)]);
        assert!(wanted.satisfied);
    }

    #[test]
    fn red_team_a_new_account_behind_the_same_router_is_not_a_new_household() {
        // Host 2 is a new network but the same account as the copy held;
        // host 3 is a new account on the same network. Neither is a new
        // household; the new network is at least a second power supply.
        let rings: Vec<Uptime> = (0..3).map(|phase| steady(NOW, 990, phase * 25)).collect();
        let held = [on("203.0.113.1", holding(1, known(1), Some(&rings[0])))];
        let ranked = [
            on("198.51.100.2", holding(2, known(1), Some(&rings[1]))),
            on("203.0.113.3", holding(3, known(3), Some(&rings[2]))),
        ];
        let wanted = want(&held, &ranked, &GOAL, NOW);
        assert_eq!(wanted.hosts.first(), Some(&device(2)), "{wanted:?}");
        assert!(!wanted.satisfied, "one household is not two: {wanted:?}");
    }

    #[test]
    fn a_satisfied_chunk_wants_nobody() {
        let a = steady(NOW, 990, 0);
        let b = steady(NOW, 990, 50);
        let held = [
            holding(1, known(1), Some(&a)),
            holding(2, known(2), Some(&b)),
        ];
        let c = steady(NOW, 990, 25);
        let wanted = want(&held, &[holding(3, known(3), Some(&c))], &GOAL, NOW);
        assert_eq!(
            wanted,
            Want {
                hosts: Vec::new(),
                satisfied: true
            }
        );
    }
}
