//! The hosts this node may place on, frozen for one round, and their order
//! for each chunk.
//!
//! The order is rendezvous hashing, the same blake3 score as
//! [`crate::nodeset`], so every own device that sees the same roster ranks
//! every chunk the same way and two of them do not spread one chunk over two
//! disjoint sets of hosts.
//!
//! What this module refuses to do:
//!
//! - **Weight a host it has not proven.** A host gets more than one slot only
//!   when it has passed an audit *and* has a week of measured history. Weight
//!   for uptime alone is what an always-on Sybil farm would buy: a rack of
//!   fresh identities that answer every hour and keep nothing.
//! - **Read pledged capacity.** Another account's pledge is not known here; a
//!   host that is full says so and becomes ineligible instead.
//! - **Change during a round.** The roster is built once; a host that appears
//!   mid-round waits for the next one, so one round's decisions agree.

use crate::liveness::lost_after_hours;
use crate::nodeset::score;
use crate::target::Account;
use crate::uptime::Uptime;
use itsanas_crypto::{ChunkId, DeviceId, UserId};

/// Most slots an availability class gives: quartiles 1..=4.
const CLASSES: u64 = 4;

/// One candidate host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub device: DeviceId,
    pub account: Account,
    pub uptime: Option<Uptime>,
    /// May be offered new chunks (reachable, not full, not draining).
    pub eligible: bool,
    /// Passed an audit, not paused.
    pub proven: bool,
    /// Seen within the store's `LIVE_FOR`.
    pub recent: bool,
}

/// The round's hosts, sorted by device, with their slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roster {
    hosts: Vec<Host>,
    /// Parallel to `hosts`.
    slots: Vec<u32>,
}

/// One slot, or the host's availability quartile when it has earned weight.
fn slots_for(host: &Host, now_hour: u64) -> u32 {
    if !host.proven {
        return 1;
    }
    let Some(uptime) = host.uptime.as_ref() else {
        return 1;
    };
    match uptime.availability(now_hour, lost_after_hours(Some(uptime))) {
        // 0..=250 → 1, ..=500 → 2, ..=750 → 3, above → 4.
        Some(per_mille) => {
            let class = (u64::from(per_mille) * CLASSES)
                .div_ceil(1000)
                .clamp(1, CLASSES);
            u32::try_from(class).unwrap_or(1)
        }
        None => 1,
    }
}

impl Roster {
    /// Build the roster; a device listed twice keeps its first entry.
    #[must_use]
    pub fn new(mut hosts: Vec<Host>, now_hour: u64) -> Self {
        hosts.sort_by_key(|host| host.device);
        hosts.dedup_by(|later, earlier| later.device == earlier.device);
        let slots = hosts.iter().map(|host| slots_for(host, now_hour)).collect();
        Self { hosts, slots }
    }

    #[must_use]
    pub fn get(&self, device: &DeviceId) -> Option<&Host> {
        self.hosts
            .binary_search_by(|host| host.device.cmp(device))
            .ok()
            .map(|index| &self.hosts[index])
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Slots a host was given (0 when absent).
    #[must_use]
    pub fn slots(&self, device: &DeviceId) -> u32 {
        self.hosts
            .binary_search_by(|host| host.device.cmp(device))
            .map_or(0, |index| self.slots[index])
    }

    /// Eligible hosts in rendezvous order for (owner, chunk), best first.
    #[must_use]
    pub fn ranked(&self, owner: UserId, chunk: &ChunkId) -> Vec<&Host> {
        self.sorted(owner, chunk, true)
    }

    /// Every host in rendezvous order, eligible or not: release needs the
    /// lowest-ranked *holder*, and a holder that is full is still a holder.
    #[must_use]
    pub fn order(&self, owner: UserId, chunk: &ChunkId) -> Vec<&Host> {
        self.sorted(owner, chunk, false)
    }

    fn sorted(&self, owner: UserId, chunk: &ChunkId, eligible_only: bool) -> Vec<&Host> {
        let mut scored: Vec<([u8; 32], &Host)> = self
            .hosts
            .iter()
            .zip(&self.slots)
            .filter(|(host, _)| !eligible_only || host.eligible)
            .map(|(host, slots)| (score(&host.device, *slots, owner, chunk), host))
            .collect();
        // The device id breaks a tie, as in NodeSet, so the order is total.
        scored.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.device.cmp(&left.1.device))
        });
        scored.into_iter().map(|(_, host)| host).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{chunk, device, steady, user};

    const NOW: u64 = 10_000;
    const CHUNKS: u32 = 4000;

    fn host(index: u16, uptime: Option<Uptime>, proven: bool) -> Host {
        Host {
            device: device(index),
            account: Account::Known(user(u8::try_from(index).unwrap())),
            uptime,
            eligible: true,
            proven,
            recent: true,
        }
    }

    /// How often each device comes first over many chunks.
    fn firsts(roster: &Roster) -> Vec<u32> {
        let mut counts = vec![0u32; roster.len()];
        for index in 0..CHUNKS {
            let best = roster.ranked(user(200), &chunk(index))[0].device;
            let position = roster
                .hosts
                .iter()
                .position(|host| host.device == best)
                .unwrap();
            counts[position] += 1;
        }
        counts
    }

    #[test]
    fn red_team_ranking_weight_goes_only_to_proven_hosts_with_a_week_of_history() {
        let young = crate::fixtures::ring(NOW - 50, 50, |_| true);
        let roster = Roster::new(
            vec![
                host(1, Some(steady(NOW, 1000, 0)), true),
                host(2, Some(steady(NOW, 1000, 0)), false),
                host(3, Some(young), true),
                host(4, None, true),
            ],
            NOW,
        );
        assert_eq!(roster.slots(&device(1)), 4, "proven and measured");
        assert_eq!(roster.slots(&device(2)), 1, "always on but never audited");
        assert_eq!(roster.slots(&device(3)), 1, "audited but under a week old");
        assert_eq!(roster.slots(&device(4)), 1, "audited, never measured");

        // Over many chunks the weighted host comes first about 4 times in 7;
        // the others share the rest evenly.
        let position = |index| {
            roster
                .hosts
                .iter()
                .position(|h| h.device == device(index))
                .unwrap()
        };
        let counts = firsts(&roster);
        let weighted = counts[position(1)];
        assert!((2100..2500).contains(&weighted), "{counts:?}");
        for index in [2, 3, 4] {
            assert!((450..700).contains(&counts[position(index)]), "{counts:?}");
        }
    }

    #[test]
    fn availability_quartiles_set_the_slots() {
        let roster = Roster::new(
            vec![
                host(1, Some(steady(NOW, 200, 0)), true),
                host(2, Some(steady(NOW, 500, 0)), true),
                host(3, Some(steady(NOW, 700, 0)), true),
                host(4, Some(steady(NOW, 900, 0)), true),
            ],
            NOW,
        );
        let slots: Vec<u32> = (1..=4).map(|index| roster.slots(&device(index))).collect();
        assert_eq!(slots, vec![1, 2, 3, 4]);
    }

    #[test]
    fn ineligible_hosts_are_ordered_but_not_ranked() {
        let mut full = host(2, None, true);
        full.eligible = false;
        let roster = Roster::new(vec![host(1, None, true), full, host(3, None, true)], NOW);
        for index in 0..50 {
            let ranked = roster.ranked(user(200), &chunk(index));
            assert_eq!(ranked.len(), 2);
            assert!(ranked.iter().all(|h| h.device != device(2)));
            let order = roster.order(user(200), &chunk(index));
            assert_eq!(order.len(), 3);
            let without: Vec<&DeviceId> = order
                .iter()
                .map(|h| &h.device)
                .filter(|d| **d != device(2))
                .collect();
            let ranked_ids: Vec<&DeviceId> = ranked.iter().map(|h| &h.device).collect();
            assert_eq!(without, ranked_ids, "the same order, minus the full host");
        }
    }

    #[test]
    fn the_order_does_not_depend_on_how_the_roster_was_built() {
        let forwards: Vec<Host> = (0..10).map(|index| host(index, None, true)).collect();
        let mut backwards = forwards.clone();
        backwards.reverse();
        backwards.push(host(3, None, false));
        let a = Roster::new(forwards, NOW);
        let b = Roster::new(backwards, NOW);
        assert_eq!(a.len(), 10, "a duplicate device is kept once");
        assert_eq!(b.len(), 10);
        for index in 0..100 {
            let left: Vec<DeviceId> = a
                .ranked(user(1), &chunk(index))
                .iter()
                .map(|h| h.device)
                .collect();
            let right: Vec<DeviceId> = b
                .ranked(user(1), &chunk(index))
                .iter()
                .map(|h| h.device)
                .collect();
            assert_eq!(left, right);
        }
        assert!(a.get(&device(3)).is_some());
        assert!(a.get(&device(42)).is_none());
        assert!(!a.is_empty());
    }
}
