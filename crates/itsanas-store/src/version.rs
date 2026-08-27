//! Version vectors: how ITSaNAS knows which edit came after which.
//!
//! Wall-clock timestamps cannot order edits made on different machines. The Pi
//! and the laptop drift, NTP steps, a VM resumes with a stale clock — and the
//! consequence of getting it wrong is not a cosmetic glitch, it is silently
//! discarding somebody's work. So ordering here never depends on time.
//!
//! Instead every version of a file carries a map from device to the highest
//! sequence number that device had contributed when the version was written.
//! Comparing two of those maps componentwise answers the only question that
//! matters: **did one of these edits happen with knowledge of the other, or were
//! they made independently?**
//!
//! ```text
//! A = {laptop: 5, pi: 2}      B = {laptop: 5, pi: 3}      → A happened before B
//! A = {laptop: 7, pi: 2}      B = {laptop: 5, pi: 3}      → concurrent, conflict
//! ```
//!
//! Concurrent is not an error. It is the normal outcome of two people editing
//! while apart, and the sync engine materialises both rather than picking one.
//!
//! # Why this type lives in the store crate
//!
//! It is part of the operation-log *format* — it is serialised into segments
//! that go on the wire. The format belongs with the code that defines it. The
//! *algorithms* that use it to merge divergent histories live in
//! `itsanas-sync`.

use std::collections::BTreeMap;

use itsanas_crypto::DeviceId;
use serde::{Deserialize, Serialize};

/// How two versions relate causally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CausalOrder {
    /// The same set of observations.
    Equal,
    /// The left version happened strictly before the right one.
    Before,
    /// The left version happened strictly after the right one.
    After,
    /// Neither knew about the other. This is a conflict.
    Concurrent,
}

/// A map from device to the highest sequence number observed from it.
///
/// An absent device is treated as sequence 0, so a vector never has to
/// enumerate devices it has not heard from — which matters once a user has more
/// devices than the two or three they started with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionVector(BTreeMap<DeviceId, u64>);

impl VersionVector {
    /// An empty vector: knows about nothing.
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// The highest sequence observed from `device`, or 0.
    #[must_use]
    pub fn get(&self, device: &DeviceId) -> u64 {
        self.0.get(device).copied().unwrap_or(0)
    }

    /// Record that `device` has reached `sequence`.
    ///
    /// Never moves a counter backwards: a peer replaying an older segment must
    /// not be able to make this device forget what it already knows.
    pub fn observe(&mut self, device: DeviceId, sequence: u64) {
        let slot = self.0.entry(device).or_insert(0);
        *slot = (*slot).max(sequence);
    }

    /// Take the componentwise maximum with `other`.
    pub fn merge(&mut self, other: &Self) {
        for (device, sequence) in &other.0 {
            self.observe(*device, *sequence);
        }
    }

    /// Whether this vector knows everything `other` knows.
    ///
    /// Reflexive: a vector always dominates itself.
    #[must_use]
    pub fn dominates(&self, other: &Self) -> bool {
        other.0.iter().all(|(device, sequence)| {
            // Only `other`'s entries need checking. Any device present here and
            // absent there is at sequence 0 there, which this trivially meets.
            self.get(device) >= *sequence
        })
    }

    /// Compare two versions causally.
    #[must_use]
    pub fn compare(&self, other: &Self) -> CausalOrder {
        match (self.dominates(other), other.dominates(self)) {
            (true, true) => CausalOrder::Equal,
            (true, false) => CausalOrder::After,
            (false, true) => CausalOrder::Before,
            (false, false) => CausalOrder::Concurrent,
        }
    }

    /// Devices this vector has heard from, in a deterministic order.
    pub fn devices(&self) -> impl Iterator<Item = (&DeviceId, &u64)> {
        self.0.iter()
    }

    /// How many devices this vector has heard from.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The vector produced by `device` writing at `sequence`, on top of this
    /// one.
    ///
    /// This is what a device stamps onto a file it is about to write: everything
    /// it knew, plus its own new entry.
    #[must_use]
    pub fn advanced(&self, device: DeviceId, sequence: u64) -> Self {
        let mut next = self.clone();
        next.observe(device, sequence);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    fn vector(entries: &[(u8, u64)]) -> VersionVector {
        let mut v = VersionVector::new();
        for (id, sequence) in entries {
            v.observe(device(*id), *sequence);
        }
        v
    }

    const LAPTOP: u8 = 1;
    const PI: u8 = 2;
    const VM: u8 = 3;

    #[test]
    fn an_empty_vector_reads_every_device_as_zero() {
        let empty = VersionVector::new();
        assert_eq!(empty.get(&device(LAPTOP)), 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn a_vector_equals_itself() {
        let v = vector(&[(LAPTOP, 5), (PI, 3)]);
        assert_eq!(v.compare(&v), CausalOrder::Equal);
        assert!(v.dominates(&v), "dominance must be reflexive");
    }

    #[test]
    fn an_empty_vector_precedes_every_non_empty_one() {
        let empty = VersionVector::new();
        let some = vector(&[(LAPTOP, 1)]);

        assert_eq!(empty.compare(&some), CausalOrder::Before);
        assert_eq!(some.compare(&empty), CausalOrder::After);
        assert_eq!(empty.compare(&VersionVector::new()), CausalOrder::Equal);
    }

    #[test]
    fn a_later_edit_on_one_device_happens_after() {
        let before = vector(&[(LAPTOP, 5), (PI, 2)]);
        let after = vector(&[(LAPTOP, 5), (PI, 3)]);

        assert_eq!(before.compare(&after), CausalOrder::Before);
        assert_eq!(after.compare(&before), CausalOrder::After);
    }

    #[test]
    fn independent_edits_are_concurrent() {
        // The case the whole type exists for: the laptop moved ahead on its own
        // counter while the Pi moved ahead on its own, and neither saw the
        // other. Picking a winner here would silently destroy work.
        let laptop = vector(&[(LAPTOP, 7), (PI, 2)]);
        let pi = vector(&[(LAPTOP, 5), (PI, 3)]);

        assert_eq!(laptop.compare(&pi), CausalOrder::Concurrent);
        assert_eq!(pi.compare(&laptop), CausalOrder::Concurrent);
        assert!(!laptop.dominates(&pi));
        assert!(!pi.dominates(&laptop));
    }

    #[test]
    fn a_device_absent_from_one_side_is_treated_as_zero_not_as_unknown() {
        // A vector that has never heard from the VM must still be comparable
        // with one that has. Treating absence as "unknown" rather than zero
        // would make almost everything spuriously concurrent.
        let without = vector(&[(LAPTOP, 5)]);
        let with = vector(&[(LAPTOP, 5), (VM, 1)]);

        assert_eq!(without.compare(&with), CausalOrder::Before);
        assert_eq!(with.compare(&without), CausalOrder::After);
    }

    #[test]
    fn observing_never_moves_a_counter_backwards() {
        // A peer replaying an old segment must not make this device forget.
        let mut v = vector(&[(LAPTOP, 10)]);
        v.observe(device(LAPTOP), 3);
        assert_eq!(v.get(&device(LAPTOP)), 10);

        v.observe(device(LAPTOP), 11);
        assert_eq!(v.get(&device(LAPTOP)), 11);
    }

    #[test]
    fn merging_takes_the_componentwise_maximum() {
        let mut a = vector(&[(LAPTOP, 7), (PI, 2)]);
        let b = vector(&[(LAPTOP, 5), (PI, 3), (VM, 9)]);

        a.merge(&b);

        assert_eq!(a.get(&device(LAPTOP)), 7);
        assert_eq!(a.get(&device(PI)), 3);
        assert_eq!(a.get(&device(VM)), 9);
    }

    #[test]
    fn merging_two_concurrent_vectors_dominates_both() {
        // This is what makes convergence possible: after both sides exchange
        // everything, their merged views are identical and dominate the inputs.
        let a = vector(&[(LAPTOP, 7), (PI, 2)]);
        let b = vector(&[(LAPTOP, 5), (PI, 3)]);
        assert_eq!(a.compare(&b), CausalOrder::Concurrent);

        let mut merged_from_a = a.clone();
        merged_from_a.merge(&b);
        let mut merged_from_b = b.clone();
        merged_from_b.merge(&a);

        assert_eq!(
            merged_from_a, merged_from_b,
            "merge is not commutative, so two devices would disagree"
        );
        assert!(merged_from_a.dominates(&a));
        assert!(merged_from_a.dominates(&b));
        assert_eq!(merged_from_a.compare(&a), CausalOrder::After);
    }

    #[test]
    fn merging_is_idempotent() {
        let a = vector(&[(LAPTOP, 7), (PI, 2)]);
        let b = vector(&[(LAPTOP, 5), (PI, 3)]);

        let mut once = a.clone();
        once.merge(&b);
        let mut twice = once.clone();
        twice.merge(&b);

        assert_eq!(
            once, twice,
            "applying the same update twice changed the result"
        );
    }

    #[test]
    fn advancing_produces_a_strict_successor() {
        let base = vector(&[(LAPTOP, 5), (PI, 3)]);
        let next = base.advanced(device(LAPTOP), 6);

        assert_eq!(base.compare(&next), CausalOrder::Before);
        assert_eq!(next.get(&device(LAPTOP)), 6);
        assert_eq!(next.get(&device(PI)), 3);
    }

    #[test]
    fn a_three_device_chain_orders_transitively() {
        let first = vector(&[(LAPTOP, 1)]);
        let second = first.advanced(device(PI), 1);
        let third = second.advanced(device(VM), 1);

        assert_eq!(first.compare(&second), CausalOrder::Before);
        assert_eq!(second.compare(&third), CausalOrder::Before);
        assert_eq!(
            first.compare(&third),
            CausalOrder::Before,
            "causal ordering is not transitive"
        );
    }

    #[test]
    fn serialisation_round_trips() {
        let original = vector(&[(LAPTOP, 7), (PI, 2), (VM, 99)]);
        let encoded = postcard::to_stdvec(&original).unwrap();
        let decoded: VersionVector = postcard::from_bytes(&encoded).unwrap();

        assert_eq!(decoded, original);
        assert_eq!(decoded.compare(&original), CausalOrder::Equal);
    }
}
