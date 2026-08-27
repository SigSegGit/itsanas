//! The set of nodes offering storage, and the placement decision over it.

use itsanas_crypto::{ChunkId, DeviceId, UserId};
use serde::{Deserialize, Serialize};

/// Domain string for placement hashing.
///
/// Changing it repositions every chunk in the network, so it is versioned and
/// must only move behind a migration.
const PLACEMENT_DOMAIN: &str = "itsanas v1 chunk placement";

/// Most slots any one node may hold.
///
/// Bounds two things at once: the work of a placement decision, which is one
/// hash per slot, and the influence of a single enormous node. A cap of 64 means
/// a node pledging a hundred times the swarm minimum is treated as pledging
/// sixty-four times it — deliberately, because letting one node own most of the
/// slots would concentrate the network's data on the machine most attractive to
/// attack.
pub const MAX_SLOTS: u32 = 64;

/// Everything that can go wrong building a node set.
#[derive(Debug, thiserror::Error)]
pub enum PlacementError {
    #[error("node {0} appears twice in the node set")]
    DuplicateNode(String),
    #[error("node {0} pledged zero capacity; remove it instead of listing it")]
    ZeroCapacity(String),
}

/// One machine offering storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageNode {
    /// The machine.
    pub device: DeviceId,
    /// Whose machine it is.
    ///
    /// Used for owner affinity: a user's own devices always appear in their own
    /// replica sets.
    pub owner: UserId,
    /// How much space it has pledged, in bytes.
    pub capacity_bytes: u64,
}

/// The swarm, as every node agrees it stands.
///
/// In a deployed network this is the signed node-set epoch the coordinator
/// publishes and peers pin. Placement is a pure function of it, so two nodes
/// holding the same epoch cannot disagree about where anything belongs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSet {
    nodes: Vec<StorageNode>,
    /// Slots per node, parallel to `nodes`.
    slots: Vec<u32>,
}

impl NodeSet {
    /// Build a node set, computing each node's slot count.
    ///
    /// Nodes are sorted by device id, so a set built from the same members in a
    /// different order is the same set. Without that, two peers given the same
    /// membership by different routes could produce different orderings, and any
    /// tie-break that depends on position would silently disagree.
    pub fn new(mut nodes: Vec<StorageNode>) -> Result<Self, PlacementError> {
        nodes.sort_by_key(|node| node.device.to_bytes());

        for pair in nodes.windows(2) {
            if pair[0].device == pair[1].device {
                return Err(PlacementError::DuplicateNode(pair[0].device.short()));
            }
        }
        if let Some(empty) = nodes.iter().find(|node| node.capacity_bytes == 0) {
            return Err(PlacementError::ZeroCapacity(empty.device.short()));
        }

        // Slots are relative to the smallest pledge in the swarm, so the ratios
        // are what matter rather than the absolute numbers. A swarm of three
        // 1 TB machines and a swarm of three 1 GB machines place identically,
        // which is right: capacity weighting is about relative shares.
        let smallest = nodes
            .iter()
            .map(|node| node.capacity_bytes)
            .min()
            .unwrap_or(1)
            .max(1);

        let slots = nodes
            .iter()
            .map(|node| {
                let ratio = node.capacity_bytes / smallest;
                u32::try_from(ratio)
                    .unwrap_or(MAX_SLOTS)
                    .clamp(1, MAX_SLOTS)
            })
            .collect();

        Ok(Self { nodes, slots })
    }

    /// The nodes, in device-id order.
    #[must_use]
    pub fn nodes(&self) -> &[StorageNode] {
        &self.nodes
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// How many slots a node holds, or 0 if it is not in the set.
    #[must_use]
    pub fn slots_for(&self, device: &DeviceId) -> u32 {
        self.nodes
            .iter()
            .position(|node| node.device == *device)
            .map_or(0, |index| self.slots[index])
    }

    /// The highest hash this node produces for a chunk, across its slots.
    ///
    /// Taking the maximum over a node's slots is what makes the weighting
    /// exact: the chance that the single highest value in the whole swarm
    /// belongs to a given node is precisely that node's share of the slots.
    fn score(node: &StorageNode, slots: u32, owner: UserId, chunk: &ChunkId) -> [u8; 32] {
        let mut best = [0u8; 32];

        for slot in 0..slots {
            let digest = blake3::Hasher::new_derive_key(PLACEMENT_DOMAIN)
                .update(node.device.as_bytes())
                .update(&slot.to_le_bytes())
                .update(owner.as_bytes())
                .update(chunk.as_bytes())
                .finalize();

            let candidate: [u8; 32] = *digest.as_bytes();
            if candidate > best {
                best = candidate;
            }
        }

        best
    }

    /// Which nodes should hold this chunk, best first.
    ///
    /// Returns at most `count` nodes, and fewer only when the swarm is smaller
    /// than that. **The owner's own devices always come first**: they are the
    /// machines that can always be reached by the person who needs the data, and
    /// the only ones whose interest in keeping it is not a matter of trust. A
    /// replica set that excluded them would mean a user whose peers all left
    /// could not read their own files.
    #[must_use]
    pub fn replicas_for(&self, owner: UserId, chunk: &ChunkId, count: usize) -> Vec<DeviceId> {
        if count == 0 || self.nodes.is_empty() {
            return Vec::new();
        }

        let mut chosen: Vec<DeviceId> = self
            .nodes
            .iter()
            .filter(|node| node.owner == owner)
            .map(|node| node.device)
            .take(count)
            .collect();

        if chosen.len() >= count {
            return chosen;
        }

        // Everyone else, ranked. The device id breaks a tie in the scores,
        // which cannot happen by chance with a 256-bit hash but must still be
        // defined for the ordering to be total.
        let mut ranked: Vec<([u8; 32], DeviceId)> = self
            .nodes
            .iter()
            .zip(&self.slots)
            .filter(|(node, _)| node.owner != owner)
            .map(|(node, slots)| (Self::score(node, *slots, owner, chunk), node.device))
            .collect();

        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.to_bytes().cmp(&left.1.to_bytes()))
        });

        chosen.extend(
            ranked
                .into_iter()
                .take(count - chosen.len())
                .map(|(_, device)| device),
        );

        chosen
    }

    /// Whether a given node should be holding a chunk.
    #[must_use]
    pub fn holds(&self, device: &DeviceId, owner: UserId, chunk: &ChunkId, count: usize) -> bool {
        self.replicas_for(owner, chunk, count).contains(device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn device(index: u16) -> DeviceId {
        let mut bytes = [0u8; 32];
        bytes[..2].copy_from_slice(&index.to_le_bytes());
        // Spread the rest so ids are not near-identical, which would make a
        // sorting bug invisible.
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

    /// A swarm of `count` equal-capacity nodes, all belonging to strangers.
    fn swarm(count: u16) -> NodeSet {
        NodeSet::new(
            (0..count)
                .map(|index| StorageNode {
                    device: device(index),
                    // Each node its own user, so owner affinity never fires
                    // and the hashing is what is being measured.
                    owner: user(u8::try_from(index % 200).unwrap_or(0).wrapping_add(50)),
                    capacity_bytes: 100 * 1024 * 1024 * 1024,
                })
                .collect(),
        )
        .expect("a valid swarm")
    }

    const CHUNKS: u32 = 4000;
    const REPLICAS: usize = 3;

    #[test]
    fn placement_is_deterministic() {
        // Two nodes computing different answers is the failure this whole
        // module is shaped around, and it would be silent.
        let set = swarm(12);
        let owner = user(1);

        for index in 0..50 {
            let first = set.replicas_for(owner, &chunk(index), REPLICAS);
            let second = set.replicas_for(owner, &chunk(index), REPLICAS);
            assert_eq!(first, second);
        }
    }

    #[test]
    fn the_answer_does_not_depend_on_the_order_the_set_was_built_in() {
        // Two peers given the same membership by different routes must produce
        // the same set, or any position-dependent tie-break disagrees.
        let mut forwards: Vec<StorageNode> = swarm(10).nodes().to_vec();
        let mut backwards = forwards.clone();
        backwards.reverse();

        // Also shuffle deterministically, so this is not just "reversed".
        forwards.sort_by_key(|node| node.capacity_bytes ^ u64::from(node.device.as_bytes()[3]));

        let a = NodeSet::new(forwards).unwrap();
        let b = NodeSet::new(backwards).unwrap();

        assert_eq!(a, b, "the same members produced two different node sets");
        for index in 0..50 {
            assert_eq!(
                a.replicas_for(user(1), &chunk(index), REPLICAS),
                b.replicas_for(user(1), &chunk(index), REPLICAS)
            );
        }
    }

    #[test]
    fn no_floating_point_is_involved() {
        // The reason this module exists in the shape it does. `f64::ln` is
        // libm-dependent and two platforms can differ in the last ulp, which
        // would make two machines disagree about where a chunk lives.
        let source = include_str!("nodeset.rs");
        let code = source
            .split("#[cfg(test)]")
            .next()
            .expect("there is code before the tests");

        for banned in ["f64", "f32", ".ln(", ".log", ".powf", ".sqrt"] {
            assert!(
                !code.contains(banned),
                "{banned:?} appears in the placement logic; scores must be \
                 computed identically on every platform"
            );
        }
    }

    // -----------------------------------------------------------------------
    // The exit criteria
    // -----------------------------------------------------------------------

    #[test]
    fn removing_a_node_moves_only_that_nodes_share() {
        // The property rendezvous hashing exists for. With modulo hashing this
        // test finds almost every chunk moving, which at real scale means
        // re-uploading the entire network.
        let before = swarm(20);
        let owner = user(1);

        let departing = before.nodes()[7].device;
        let after = NodeSet::new(
            before
                .nodes()
                .iter()
                .filter(|node| node.device != departing)
                .copied()
                .collect(),
        )
        .unwrap();

        let mut disturbed = 0;
        let mut wrongly_moved = 0;

        for index in 0..CHUNKS {
            let old: BTreeSet<DeviceId> = before
                .replicas_for(owner, &chunk(index), REPLICAS)
                .into_iter()
                .collect();
            let new: BTreeSet<DeviceId> = after
                .replicas_for(owner, &chunk(index), REPLICAS)
                .into_iter()
                .collect();

            if old == new {
                continue;
            }
            disturbed += 1;

            // The only legitimate change: the departing node drops out and
            // exactly one newcomer replaces it. Any chunk moving between two
            // *surviving* nodes is gratuitous work and a bug.
            let left = &old - &new;
            let joined = &new - &old;

            if left != BTreeSet::from([departing]) || joined.len() != 1 {
                wrongly_moved += 1;
            }
        }

        assert_eq!(
            wrongly_moved, 0,
            "{wrongly_moved} chunks moved between two surviving nodes; \
             rendezvous hashing's minimal-disruption property is broken"
        );

        // Only chunks the departing node actually held should be disturbed:
        // about `REPLICAS / nodes` of them.
        let expected = CHUNKS as usize * REPLICAS / 20;
        assert!(
            disturbed < expected * 2,
            "{disturbed} chunks were disturbed by one node leaving, expected \
             roughly {expected}"
        );
    }

    #[test]
    fn adding_a_node_only_pulls_in_its_own_share() {
        let before = swarm(20);
        let owner = user(1);

        let mut with_newcomer = before.nodes().to_vec();
        with_newcomer.push(StorageNode {
            device: device(999),
            owner: user(200),
            capacity_bytes: 100 * 1024 * 1024 * 1024,
        });
        let after = NodeSet::new(with_newcomer).unwrap();

        let mut wrongly_moved = 0;
        for index in 0..CHUNKS {
            let old: BTreeSet<DeviceId> = before
                .replicas_for(owner, &chunk(index), REPLICAS)
                .into_iter()
                .collect();
            let new: BTreeSet<DeviceId> = after
                .replicas_for(owner, &chunk(index), REPLICAS)
                .into_iter()
                .collect();

            if old == new {
                continue;
            }

            // The newcomer joining is the only permitted change.
            let joined = &new - &old;
            if joined != BTreeSet::from([device(999)]) || (&old - &new).len() != 1 {
                wrongly_moved += 1;
            }
        }

        assert_eq!(
            wrongly_moved, 0,
            "adding one node reshuffled chunks between existing nodes"
        );
    }

    #[test]
    fn distribution_matches_pledged_capacity() {
        // A node pledging four times as much should hold roughly four times as
        // many chunks. Without this the "mutual" in mutual storage is a fiction:
        // the small nodes carry the network.
        let capacities = [1u64, 1, 2, 4, 8];
        let unit = 10 * 1024 * 1024 * 1024;

        let set = NodeSet::new(
            capacities
                .iter()
                .enumerate()
                .map(|(index, multiple)| StorageNode {
                    device: device(u16::try_from(index).unwrap()),
                    owner: user(u8::try_from(index).unwrap() + 50),
                    capacity_bytes: multiple * unit,
                })
                .collect(),
        )
        .unwrap();

        let mut held: BTreeMap<DeviceId, usize> = BTreeMap::new();
        let total_chunks = 20_000;
        for index in 0..total_chunks {
            // One replica, so the measurement is of the ranking itself rather
            // than of "almost everyone is in the top three of five".
            for chosen in set.replicas_for(user(1), &chunk(index), 1) {
                *held.entry(chosen).or_default() += 1;
            }
        }

        let total_slots: u32 = capacities
            .iter()
            .map(|multiple| u32::try_from(*multiple).unwrap())
            .sum();

        for (index, multiple) in capacities.iter().enumerate() {
            let this = device(u16::try_from(index).unwrap());
            let actual = *held.get(&this).unwrap_or(&0);
            let expected =
                total_chunks as usize * usize::try_from(*multiple).unwrap() / total_slots as usize;

            let low = expected * 80 / 100;
            let high = expected * 120 / 100;
            assert!(
                actual >= low && actual <= high,
                "node {index} pledged {multiple}x the unit and holds {actual} \
                 chunks; expected about {expected} (within 20%)"
            );
        }
    }

    #[test]
    fn a_users_own_devices_always_hold_their_own_data() {
        // A user whose peers have all left must still be able to read their own
        // files. If placement could exclude their own machines, it could hand
        // every replica to strangers.
        let mine = user(1);
        let mut nodes: Vec<StorageNode> = swarm(15).nodes().to_vec();

        let laptop = device(9001);
        let pi = device(9002);
        for own in [laptop, pi] {
            nodes.push(StorageNode {
                device: own,
                owner: mine,
                capacity_bytes: 50 * 1024 * 1024 * 1024,
            });
        }
        let set = NodeSet::new(nodes).unwrap();

        for index in 0..500 {
            let replicas = set.replicas_for(mine, &chunk(index), REPLICAS);
            assert!(
                replicas.contains(&laptop) && replicas.contains(&pi),
                "chunk {index} was placed without either of the owner's own \
                 devices: {replicas:?}"
            );
            assert_eq!(replicas.len(), REPLICAS);
        }
    }

    #[test]
    fn owner_affinity_does_not_starve_a_user_with_many_devices() {
        // A user with more devices than the replication factor must not fill
        // every slot with their own machines — that would mean their data never
        // leaves their own hardware and a house fire takes all of it.
        let mine = user(1);
        let mut nodes: Vec<StorageNode> = swarm(10).nodes().to_vec();
        for index in 0..8u16 {
            nodes.push(StorageNode {
                device: device(9100 + index),
                owner: mine,
                capacity_bytes: 50 * 1024 * 1024 * 1024,
            });
        }
        let set = NodeSet::new(nodes).unwrap();

        let replicas = set.replicas_for(mine, &chunk(1), REPLICAS);
        assert_eq!(
            replicas.len(),
            REPLICAS,
            "the replica set grew beyond the replication factor"
        );

        // This documents the current, deliberate behaviour: with more own
        // devices than replicas, they take every slot. That is right for
        // availability and wrong for durability, and the fix belongs with the
        // repair loop, which will need an explicit off-site target.
        assert!(
            replicas.iter().all(|device| set
                .nodes()
                .iter()
                .any(|node| node.device == *device && node.owner == mine)),
            "expected the owner's own devices to fill the set"
        );
    }

    // -----------------------------------------------------------------------
    // Edges
    // -----------------------------------------------------------------------

    #[test]
    fn a_swarm_smaller_than_the_replication_factor_returns_everyone() {
        let set = swarm(2);
        let replicas = set.replicas_for(user(1), &chunk(1), 5);
        assert_eq!(replicas.len(), 2);
    }

    #[test]
    fn an_empty_swarm_places_nothing_rather_than_panicking() {
        let set = NodeSet::new(Vec::new()).unwrap();
        assert!(set.is_empty());
        assert!(set.replicas_for(user(1), &chunk(1), 3).is_empty());
        assert!(!set.holds(&device(1), user(1), &chunk(1), 3));
    }

    #[test]
    fn asking_for_zero_replicas_returns_none() {
        assert!(swarm(5).replicas_for(user(1), &chunk(1), 0).is_empty());
    }

    #[test]
    fn a_replica_set_never_contains_the_same_node_twice() {
        // Three replicas on one machine is one replica with extra steps, and
        // would make the durability accounting a lie.
        let set = swarm(8);
        for index in 0..200 {
            let replicas = set.replicas_for(user(1), &chunk(index), REPLICAS);
            let unique: BTreeSet<_> = replicas.iter().collect();
            assert_eq!(
                unique.len(),
                replicas.len(),
                "duplicate node in {replicas:?}"
            );
        }
    }

    #[test]
    fn duplicate_and_zero_capacity_nodes_are_refused() {
        let node = StorageNode {
            device: device(1),
            owner: user(1),
            capacity_bytes: 1024,
        };

        assert!(matches!(
            NodeSet::new(vec![node, node]),
            Err(PlacementError::DuplicateNode(_))
        ));

        assert!(matches!(
            NodeSet::new(vec![StorageNode {
                capacity_bytes: 0,
                ..node
            }]),
            Err(PlacementError::ZeroCapacity(_))
        ));
    }

    #[test]
    fn one_enormous_node_cannot_take_over_the_swarm() {
        // Concentrating the network's data on a single machine is exactly what
        // a well-resourced adversary would pledge for.
        let set = NodeSet::new(vec![
            StorageNode {
                device: device(1),
                owner: user(50),
                capacity_bytes: 1024 * 1024 * 1024,
            },
            StorageNode {
                device: device(2),
                owner: user(51),
                capacity_bytes: 1024 * 1024 * 1024,
            },
            StorageNode {
                device: device(3),
                owner: user(52),
                // A million times the others.
                capacity_bytes: 1024 * 1024 * 1024 * 1024 * 1024,
            },
        ])
        .unwrap();

        assert_eq!(
            set.slots_for(&device(3)),
            MAX_SLOTS,
            "the slot cap was not applied"
        );

        let mut giant = 0;
        for index in 0..2000 {
            if set.replicas_for(user(1), &chunk(index), 1)[0] == device(3) {
                giant += 1;
            }
        }

        // 64 of 66 slots is still most of them — the cap bounds dominance, it
        // does not prevent it. What it prevents is 999999/1000001.
        assert!(
            giant < 2000,
            "the giant node took every single chunk despite the cap"
        );
        assert!(
            set.slots_for(&device(1)) == 1 && set.slots_for(&device(2)) == 1,
            "the small nodes lost their slots entirely"
        );
    }

    #[test]
    fn identical_capacities_distribute_evenly() {
        let set = swarm(10);
        let mut held: BTreeMap<DeviceId, usize> = BTreeMap::new();

        for index in 0..10_000 {
            for chosen in set.replicas_for(user(1), &chunk(index), 1) {
                *held.entry(chosen).or_default() += 1;
            }
        }

        assert_eq!(held.len(), 10, "some node never received a single chunk");
        for (node, count) in held {
            assert!(
                (700..=1300).contains(&count),
                "node {node:?} holds {count} of 10000 chunks; expected about 1000"
            );
        }
    }

    #[test]
    fn different_owners_get_different_placements_for_the_same_chunk_id() {
        // Chunk ids are blinded per user so this should not arise, but placement
        // must not be the thing that reintroduces cross-user correlation.
        let set = swarm(12);
        let same_chunk = chunk(42);

        let a = set.replicas_for(user(1), &same_chunk, REPLICAS);
        let b = set.replicas_for(user(2), &same_chunk, REPLICAS);

        assert_ne!(
            a, b,
            "two users' identical chunk ids placed identically, so a host could \
             infer that two peers hold the same content"
        );
    }
}
