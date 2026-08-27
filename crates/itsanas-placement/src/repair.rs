//! Noticing that a chunk is running out of copies, and deciding what to do.
//!
//! Placement says where a chunk *should* live. Reality says where it *does*.
//! Repair is the difference, and it is the part that has to work without
//! anybody watching: the failure it exists to catch is a slow one. Nobody
//! notices a chunk quietly dropping from three replicas to one. They notice
//! when it drops to zero, and by then it is too late.
//!
//! # Planning is separated from doing on purpose
//!
//! Everything here is a pure function of "what the swarm looks like" and "what I
//! observed". No sockets, no store, no clock. That means the interesting cases —
//! a chunk with no copies left, a swarm too small to meet its own floor, a peer
//! that lied about holding something — are ordinary unit tests rather than
//! things you hope are right because the integration test happened to pass.
//!
//! # What this deliberately does not do
//!
//! It never plans a *deletion*. An over-replicated chunk is wasted space; a
//! wrongly deleted one is gone. Reclaiming excess replicas needs certainty that
//! the other copies exist, which needs storage challenges against every holder,
//! and until that runs on a schedule the safe asymmetry is to only ever add.

use std::collections::{BTreeMap, BTreeSet};

use itsanas_crypto::{ChunkId, DeviceId, UserId};

use crate::nodeset::NodeSet;

/// How many replicas a chunk should have before it is considered safe.
///
/// Three is the smallest number where losing one machine is not an emergency
/// and losing two at once is required to lose data.
pub const DEFAULT_REPLICATION_FLOOR: usize = 3;

/// What this node believes about where chunks currently live.
///
/// Built from what peers reported, so it is a belief rather than a fact: a peer
/// can claim to hold something it discarded. Storage challenges are what turn a
/// claim into evidence; this structure is what decides whether to bother.
#[derive(Clone, Debug, Default)]
pub struct Census {
    holders: BTreeMap<ChunkId, BTreeSet<DeviceId>>,
}

impl Census {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `device` reports holding `chunk`.
    pub fn observe(&mut self, chunk: ChunkId, device: DeviceId) {
        self.holders.entry(chunk).or_default().insert(device);
    }

    /// Record that a chunk exists, whether or not anyone is known to hold it.
    ///
    /// Needed so a chunk that *nobody* reported can still be planned for. Left
    /// out of the census entirely, it would be invisible to repair, which is
    /// the exact case that loses data.
    pub fn note(&mut self, chunk: ChunkId) {
        self.holders.entry(chunk).or_default();
    }

    /// Who is believed to hold `chunk`.
    #[must_use]
    pub fn holders(&self, chunk: &ChunkId) -> BTreeSet<DeviceId> {
        self.holders.get(chunk).cloned().unwrap_or_default()
    }

    /// How many copies a chunk is believed to have.
    #[must_use]
    pub fn count(&self, chunk: &ChunkId) -> usize {
        self.holders.get(chunk).map_or(0, BTreeSet::len)
    }

    /// Every chunk the census knows about.
    pub fn chunks(&self) -> impl Iterator<Item = &ChunkId> {
        self.holders.keys()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.holders.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.holders.is_empty()
    }
}

/// One chunk that should be sent to one node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Push {
    pub chunk: ChunkId,
    pub to: DeviceId,
}

/// A chunk that cannot reach its floor with the swarm as it stands.
///
/// Not an error — an alert. The operator can add capacity, or accept the risk,
/// but they cannot do either if nothing tells them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtRisk {
    pub chunk: ChunkId,
    /// How many copies are believed to exist.
    pub held_by: usize,
    /// How many there should be.
    pub floor: usize,
}

/// What repair intends to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepairPlan {
    /// Chunks to send, sorted so two nodes planning the same repair agree.
    pub pushes: Vec<Push>,
    /// Chunks that will still be short afterwards.
    pub at_risk: Vec<AtRisk>,
}

impl RepairPlan {
    /// Whether anything needs doing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pushes.is_empty() && self.at_risk.is_empty()
    }

    /// Whether any chunk is one failure away from being lost.
    ///
    /// The condition worth waking somebody for, as distinct from the ordinary
    /// business of a node having gone away for the evening.
    #[must_use]
    pub fn has_critical(&self) -> bool {
        self.at_risk.iter().any(|risk| risk.held_by <= 1)
    }
}

/// Work out what to send where.
///
/// `available` is the set of nodes currently reachable. A node that is merely
/// asleep is not a reason to re-place a chunk — it will come back, and moving
/// data because someone shut a laptop would mean the network churns constantly.
/// So placement is computed over the whole swarm, and only the *sending* is
/// restricted to what can be reached now.
#[must_use]
pub fn plan(
    swarm: &NodeSet,
    owner: UserId,
    census: &Census,
    floor: usize,
    available: &BTreeSet<DeviceId>,
) -> RepairPlan {
    let mut plan = RepairPlan::default();

    for chunk in census.chunks() {
        let holders = census.holders(chunk);
        let targets = swarm.replicas_for(owner, chunk, floor);

        for target in &targets {
            if holders.contains(target) {
                continue;
            }
            if !available.contains(target) {
                // Asleep, not gone. Nothing to do this round.
                continue;
            }
            plan.pushes.push(Push {
                chunk: *chunk,
                to: *target,
            });
        }

        // After this round, a chunk is safe if enough distinct nodes will hold
        // it: those that already do, plus those about to be sent it.
        let reachable_targets = targets
            .iter()
            .filter(|target| available.contains(*target) || holders.contains(*target))
            .count();

        let eventual = holders
            .iter()
            .filter(|holder| swarm.nodes().iter().any(|node| node.device == **holder))
            .count()
            .max(reachable_targets);

        if eventual < floor {
            plan.at_risk.push(AtRisk {
                chunk: *chunk,
                held_by: holders.len(),
                floor,
            });
        }
    }

    // Sorted so that two nodes computing the same plan produce the same list,
    // which is what lets a test compare them and an operator diff two logs.
    plan.pushes.sort_unstable();
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodeset::StorageNode;

    fn device(index: u16) -> DeviceId {
        let mut bytes = [0u8; 32];
        bytes[..2].copy_from_slice(&index.to_le_bytes());
        let filler = blake3::hash(&index.to_le_bytes());
        bytes[2..].copy_from_slice(&filler.as_bytes()[..30]);
        DeviceId::from_bytes(bytes)
    }

    fn user(index: u8) -> UserId {
        UserId::from_bytes([index; 32])
    }

    fn chunk(index: u32) -> ChunkId {
        ChunkId::from_bytes(*blake3::hash(&index.to_le_bytes()).as_bytes())
    }

    fn swarm(count: u16) -> NodeSet {
        NodeSet::new(
            (0..count)
                .map(|index| StorageNode {
                    device: device(index),
                    owner: user(u8::try_from(index).unwrap_or(0) + 50),
                    capacity_bytes: 100 * 1024 * 1024 * 1024,
                })
                .collect(),
        )
        .unwrap()
    }

    fn everyone(swarm: &NodeSet) -> BTreeSet<DeviceId> {
        swarm.nodes().iter().map(|node| node.device).collect()
    }

    const FLOOR: usize = 3;

    #[test]
    fn a_fully_replicated_chunk_needs_nothing() {
        let swarm = swarm(8);
        let owner = user(1);
        let mut census = Census::new();

        for target in swarm.replicas_for(owner, &chunk(1), FLOOR) {
            census.observe(chunk(1), target);
        }

        let plan = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));
        assert!(plan.is_empty(), "a healthy chunk generated work: {plan:?}");
    }

    #[test]
    fn one_missing_replica_produces_exactly_one_push_to_the_right_node() {
        let swarm = swarm(8);
        let owner = user(1);
        let targets = swarm.replicas_for(owner, &chunk(1), FLOOR);

        let mut census = Census::new();
        census.observe(chunk(1), targets[0]);
        census.observe(chunk(1), targets[1]);

        let plan = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));

        assert_eq!(
            plan.pushes,
            vec![Push {
                chunk: chunk(1),
                to: targets[2],
            }],
            "repair did not send the chunk to the one node that should hold it"
        );
        assert!(plan.at_risk.is_empty());
    }

    #[test]
    fn repair_never_sends_a_chunk_to_a_node_that_should_not_hold_it() {
        // Otherwise repair slowly spreads every chunk to every node, and the
        // pledged-capacity accounting stops meaning anything.
        let swarm = swarm(12);
        let owner = user(1);
        let census = {
            let mut census = Census::new();
            for index in 0..50 {
                census.note(chunk(index));
            }
            census
        };

        let plan = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));

        for push in &plan.pushes {
            let allowed = swarm.replicas_for(owner, &push.chunk, FLOOR);
            assert!(
                allowed.contains(&push.to),
                "repair planned to send {:?} to a node outside its replica set",
                push.chunk
            );
        }
        assert_eq!(plan.pushes.len(), 50 * FLOOR);
    }

    #[test]
    fn a_chunk_nobody_holds_is_planned_for_rather_than_overlooked() {
        // The case that loses data. A chunk missing from the census entirely
        // would be invisible to repair.
        let swarm = swarm(6);
        let owner = user(1);

        let mut census = Census::new();
        census.note(chunk(7));

        let plan = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));
        assert_eq!(plan.pushes.len(), FLOOR);
        assert!(plan.at_risk.is_empty(), "it is recoverable, so not at risk");
    }

    #[test]
    fn a_swarm_too_small_to_meet_the_floor_raises_an_alert() {
        // Silence here would mean a user believing they have three replicas
        // when the network can only ever give them two.
        let swarm = swarm(2);
        let owner = user(1);

        let mut census = Census::new();
        census.note(chunk(1));

        let plan = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));

        assert_eq!(plan.pushes.len(), 2, "it should still use what exists");
        assert_eq!(
            plan.at_risk,
            vec![AtRisk {
                chunk: chunk(1),
                held_by: 0,
                floor: FLOOR,
            }]
        );
    }

    #[test]
    fn a_chunk_with_a_single_copy_left_is_flagged_as_critical() {
        // The difference between "a node is having an evening off" and "one
        // more failure and this is gone".
        let swarm = swarm(1);
        let owner = user(1);

        let mut census = Census::new();
        census.observe(chunk(1), swarm.nodes()[0].device);

        let plan = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));
        assert!(
            plan.has_critical(),
            "a chunk one failure from being lost did not raise a critical alert"
        );
    }

    #[test]
    fn an_offline_node_is_not_a_reason_to_move_data() {
        // A node that is merely asleep will come back. Re-placing its chunks
        // would mean the network churns every time somebody shuts a laptop.
        let swarm = swarm(8);
        let owner = user(1);
        let targets = swarm.replicas_for(owner, &chunk(1), FLOOR);

        let mut census = Census::new();
        census.observe(chunk(1), targets[0]);
        census.observe(chunk(1), targets[1]);

        // The third target is asleep.
        let mut available = everyone(&swarm);
        available.remove(&targets[2]);

        let plan = plan(&swarm, owner, &census, FLOOR, &available);

        assert!(
            plan.pushes.is_empty(),
            "repair tried to move data because a node was temporarily offline: \
             {plan:?}"
        );
        assert_eq!(
            plan.at_risk,
            vec![AtRisk {
                chunk: chunk(1),
                held_by: 2,
                floor: FLOOR,
            }],
            "the shortfall should still be reported while the node is away"
        );
    }

    #[test]
    fn repair_never_plans_a_deletion() {
        // An over-replicated chunk is wasted space; a wrongly deleted one is
        // gone. The plan type has no deletion variant, and this test exists so
        // that adding one is a deliberate act rather than a convenience.
        let swarm = swarm(8);
        let owner = user(1);

        let mut census = Census::new();
        // Every node claims to hold it, far beyond the floor.
        for node in swarm.nodes() {
            census.observe(chunk(1), node.device);
        }

        let plan = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));
        assert!(
            plan.is_empty(),
            "an over-replicated chunk produced work: {plan:?}"
        );
    }

    #[test]
    fn a_plan_is_deterministic_and_ordered() {
        // Two nodes planning the same repair must produce comparable output, or
        // an operator cannot diff two logs and a test cannot assert equality.
        let swarm = swarm(10);
        let owner = user(1);

        let mut census = Census::new();
        for index in 0..30 {
            census.note(chunk(index));
        }

        let first = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));
        let second = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));

        assert_eq!(first, second);
        assert!(
            first.pushes.windows(2).all(|pair| pair[0] <= pair[1]),
            "the push list is not sorted"
        );
    }

    #[test]
    fn a_holder_that_has_left_the_swarm_does_not_count_towards_the_floor() {
        // A machine that was decommissioned still appears in an old census.
        // Counting it would mean believing in a replica that no longer exists.
        let swarm = swarm(4);
        let owner = user(1);
        let departed = device(9999);

        let mut census = Census::new();
        census.observe(chunk(1), departed);
        for target in swarm.replicas_for(owner, &chunk(1), FLOOR).iter().take(2) {
            census.observe(chunk(1), *target);
        }

        let plan = plan(&swarm, owner, &census, FLOOR, &everyone(&swarm));

        assert!(
            plan.pushes.iter().all(|push| push.to != departed),
            "repair tried to send data to a node that has left the swarm"
        );
        assert_eq!(
            plan.pushes.len(),
            1,
            "the third replica should be replaced: {plan:?}"
        );
    }

    #[test]
    fn an_empty_census_produces_an_empty_plan() {
        let swarm = swarm(5);
        let plan = plan(&swarm, user(1), &Census::new(), FLOOR, &everyone(&swarm));
        assert!(plan.is_empty());
        assert!(!plan.has_critical());
    }

    #[test]
    fn nothing_is_planned_when_no_node_is_reachable() {
        // Everything is offline. There is nothing to do but say so.
        let swarm = swarm(6);
        let owner = user(1);

        let mut census = Census::new();
        census.note(chunk(1));

        let plan = plan(&swarm, owner, &census, FLOOR, &BTreeSet::new());

        assert!(plan.pushes.is_empty());
        assert!(
            plan.has_critical(),
            "zero copies and nowhere to send is critical"
        );
    }

    #[test]
    fn the_census_counts_distinct_holders_not_repeated_claims() {
        // A peer answering the same question twice must not inflate the
        // replica count, or repair concludes a chunk is safe when it is not.
        let mut census = Census::new();
        census.observe(chunk(1), device(1));
        census.observe(chunk(1), device(1));
        census.observe(chunk(1), device(2));

        assert_eq!(census.count(&chunk(1)), 2);
    }
}
