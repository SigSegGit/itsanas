//! Segments: each device's signed log chain, stored only when it continues
//! the chain held, and served in order from a resume point.

use itsanas_crypto::{DeviceId, ObjectId, UserId};
use redb::ReadableTable;

use super::{
    CHAIN_BYTES, CHAIN_KEY_LEN, CHAIN_LENGTHS, HEADS, SEGMENTS, Vault, chain_key, segment_key,
};
use crate::{
    error::{Result, StoreError},
    oplog::SegmentEnvelope,
};

impl Vault {
    // -------------------------------------------------------------- segments

    /// Accept a segment for storage, after verifying its signature.
    ///
    /// Returns whether it was newly stored. Rejects a segment that does not
    /// continue the chain this vault already holds for that device, so a host
    /// cannot be induced to store a chain with a hole in it and then serve that
    /// hole to a peer as though it were complete.
    pub fn put_segment(&self, envelope: &SegmentEnvelope) -> Result<bool> {
        // Before anything is written. A host that stored unverified envelopes
        // would be a convenient way to attribute garbage to someone's device.
        envelope.verify_signature()?;

        let encoded = envelope.encode()?;
        let key = chain_key(envelope.owner, envelope.device);

        let txn = self.db.begin_write()?;
        let stored;

        {
            let mut segments = txn.open_table(SEGMENTS)?;
            let mut lengths = txn.open_table(CHAIN_LENGTHS)?;
            let mut chain_bytes = txn.open_table(CHAIN_BYTES)?;
            let mut heads = txn.open_table(HEADS)?;

            let length = lengths.get(key.as_slice())?.map_or(0, |v| v.value());
            let head = match heads.get(key.as_slice())? {
                Some(value) => Some(ObjectId::from_slice(value.value())?),
                None => None,
            };

            if head == Some(envelope.segment_id) {
                // Already the tip. Re-offering is normal, not an error.
                stored = false;
            } else if envelope.previous == head {
                segments.insert(
                    segment_key(envelope.owner, envelope.device, length).as_slice(),
                    encoded.as_slice(),
                )?;
                lengths.insert(key.as_slice(), length + 1)?;
                let so_far = chain_bytes.get(key.as_slice())?.map_or(0, |v| v.value());
                chain_bytes.insert(key.as_slice(), so_far.saturating_add(encoded.len() as u64))?;
                heads.insert(key.as_slice(), envelope.segment_id.as_bytes().as_slice())?;
                stored = true;
            } else {
                return Err(StoreError::SegmentChainBroken {
                    segment: envelope.segment_id.short(),
                    expected: head.map_or_else(|| "none".to_owned(), |id| id.short()),
                    found: envelope
                        .previous
                        .map_or_else(|| "none".to_owned(), |id| id.short()),
                });
            }
        }

        txn.commit()?;
        Ok(stored)
    }

    /// Segments held for one device's chain, oldest first.
    ///
    /// `after` resumes from just past a segment the caller already has; `limit`
    /// caps the response so one request cannot ask for an unbounded assembly.
    pub fn segments_for(
        &self,
        owner: UserId,
        device: DeviceId,
        after: Option<ObjectId>,
        limit: usize,
    ) -> Result<Vec<SegmentEnvelope>> {
        let prefix = chain_key(owner, device);
        let txn = self.db.begin_read()?;
        let table = txn.open_table(SEGMENTS)?;

        let start = segment_key(owner, device, 0);
        let end = segment_key(owner, device, u64::MAX);

        let mut out = Vec::new();
        let mut skipping = after.is_some();

        for row in table.range(start.as_slice()..=end.as_slice())? {
            let (key, value) = row?;
            if !key.value().starts_with(&prefix) {
                break;
            }

            let envelope = SegmentEnvelope::decode(value.value())?;

            if skipping {
                // Resume *after* the named segment, so the caller does not
                // receive one it already has on every round.
                if Some(envelope.segment_id) == after {
                    skipping = false;
                }
                continue;
            }

            out.push(envelope);
            if out.len() >= limit {
                break;
            }
        }

        // `after` naming a segment this vault does not hold means the caller is
        // ahead of us, or is talking about a different chain. Returning
        // everything would re-send history it already has; returning nothing is
        // the honest answer.
        if skipping {
            return Ok(Vec::new());
        }

        Ok(out)
    }

    /// Chain tips held for one owner, across all their devices.
    pub fn heads_for(&self, owner: UserId) -> Result<Vec<(DeviceId, ObjectId, u64)>> {
        let txn = self.db.begin_read()?;
        let heads = txn.open_table(HEADS)?;
        let lengths = txn.open_table(CHAIN_LENGTHS)?;

        let mut out = Vec::new();
        for row in heads.iter()? {
            let (key, value) = row?;
            let key = key.value();

            if key.len() != CHAIN_KEY_LEN || !key.starts_with(owner.as_bytes()) {
                continue;
            }

            let device = DeviceId::from_slice(&key[32..])?;
            let head = ObjectId::from_slice(value.value())?;
            let length = lengths.get(key)?.map_or(0, |v| v.value());

            out.push((device, head, length));
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;

    #[test]
    fn a_segment_with_a_bad_signature_is_refused_before_it_is_stored() {
        // A host that stored unverified envelopes becomes a way to flood a
        // user's peers with garbage attributed to one of their devices.
        let (_dir, vault) = vault();
        let user = keys(3);
        let dev = device(3);

        let mut forged = segment(&user, &dev, None, 1);
        forged.sealed_body[0] ^= 0xFF;

        assert!(
            vault.put_segment(&forged).is_err(),
            "a segment with a broken signature was accepted for storage"
        );
        assert_eq!(
            vault.heads_for(user.user_id()).unwrap(),
            [] as [(itsanas_crypto::DeviceId, itsanas_crypto::ObjectId, u64); 0]
        );
    }

    #[test]
    fn a_chain_is_stored_and_served_in_order() {
        let (_dir, vault) = vault();
        let user = keys(4);
        let dev = device(4);

        let first = segment(&user, &dev, None, 1);
        let second = segment(&user, &dev, Some(first.segment_id), 2);
        let third = segment(&user, &dev, Some(second.segment_id), 3);

        for envelope in [&first, &second, &third] {
            assert!(vault.put_segment(envelope).unwrap());
        }

        let served = vault
            .segments_for(user.user_id(), dev.device_id(), None, 10)
            .unwrap();

        assert_eq!(served.len(), 3);
        assert_eq!(served[0].segment_id, first.segment_id);
        assert_eq!(served[1].segment_id, second.segment_id);
        assert_eq!(served[2].segment_id, third.segment_id);
        crate::oplog::validate_chain(&served).expect("the served chain must validate");
    }

    #[test]
    fn a_segment_that_does_not_continue_the_chain_is_refused() {
        // Otherwise a host can be induced to store a chain with a hole and then
        // serve that hole to a peer as though it were complete.
        let (_dir, vault) = vault();
        let user = keys(5);
        let dev = device(5);

        let first = segment(&user, &dev, None, 1);
        let second = segment(&user, &dev, Some(first.segment_id), 2);
        let third = segment(&user, &dev, Some(second.segment_id), 3);

        vault.put_segment(&first).unwrap();

        assert!(
            vault.put_segment(&third).is_err(),
            "a segment was stored on top of a chain it does not continue"
        );
        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn re_offering_the_current_tip_is_accepted_as_a_no_op() {
        // Peers re-offer freely; there is no acknowledgement telling them to
        // stop. This must not be an error, and must not duplicate the segment.
        let (_dir, vault) = vault();
        let user = keys(6);
        let dev = device(6);
        let first = segment(&user, &dev, None, 1);

        assert!(vault.put_segment(&first).unwrap());
        assert!(!vault.put_segment(&first).unwrap());

        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .len(),
            1,
            "re-offering the tip duplicated it"
        );
    }

    #[test]
    fn resuming_after_a_segment_skips_what_the_caller_already_has() {
        let (_dir, vault) = vault();
        let user = keys(7);
        let dev = device(7);

        let first = segment(&user, &dev, None, 1);
        let second = segment(&user, &dev, Some(first.segment_id), 2);
        let third = segment(&user, &dev, Some(second.segment_id), 3);
        for envelope in [&first, &second, &third] {
            vault.put_segment(envelope).unwrap();
        }

        let served = vault
            .segments_for(user.user_id(), dev.device_id(), Some(first.segment_id), 10)
            .unwrap();

        assert_eq!(served.len(), 2);
        assert_eq!(served[0].segment_id, second.segment_id);

        // Resuming after the tip yields nothing.
        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), Some(third.segment_id), 10)
                .unwrap(),
            [] as [crate::oplog::SegmentEnvelope; 0]
        );
    }

    #[test]
    fn the_limit_caps_the_response() {
        let (_dir, vault) = vault();
        let user = keys(8);
        let dev = device(8);

        let mut previous = None;
        for sequence in 1..=5 {
            let envelope = segment(&user, &dev, previous, sequence);
            vault.put_segment(&envelope).unwrap();
            previous = Some(envelope.segment_id);
        }

        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 2)
                .unwrap()
                .len(),
            2,
            "the limit was ignored, so one request can ask for unbounded work"
        );
    }

    #[test]
    fn resuming_after_an_unknown_segment_returns_nothing_rather_than_everything() {
        let (_dir, vault) = vault();
        let user = keys(9);
        let dev = device(9);
        vault.put_segment(&segment(&user, &dev, None, 1)).unwrap();

        let served = vault
            .segments_for(
                user.user_id(),
                dev.device_id(),
                Some(ObjectId::from_bytes([0xEE; 32])),
                10,
            )
            .unwrap();

        assert!(
            served.is_empty(),
            "an unknown resume point caused the whole chain to be re-sent"
        );
    }

    #[test]
    fn heads_are_reported_per_device_and_scoped_to_one_owner() {
        let (_dir, vault) = vault();
        let alice = keys(10);
        let bob = keys(11);
        let alice_laptop = device(10);
        let alice_pi = device(11);
        let bob_vm = device(12);

        let a1 = segment(&alice, &alice_laptop, None, 1);
        let a2 = segment(&alice, &alice_laptop, Some(a1.segment_id), 2);
        let p1 = segment(&alice, &alice_pi, None, 1);
        let b1 = segment(&bob, &bob_vm, None, 1);

        for envelope in [&a1, &a2, &p1, &b1] {
            vault.put_segment(envelope).unwrap();
        }

        let mut heads = vault.heads_for(alice.user_id()).unwrap();
        heads.sort_by_key(|(device, _, _)| device.to_bytes());

        assert_eq!(heads.len(), 2, "expected one head per Alice device");
        assert!(
            heads
                .iter()
                .all(|(device, _, _)| *device == alice_laptop.device_id()
                    || *device == alice_pi.device_id()),
            "Bob's device appeared in Alice's heads"
        );

        let laptop = heads
            .iter()
            .find(|(device, _, _)| *device == alice_laptop.device_id())
            .unwrap();
        assert_eq!(laptop.1, a2.segment_id);
        assert_eq!(laptop.2, 2);
    }

    #[test]
    fn one_owners_segments_are_never_served_under_another_owners_name() {
        let (_dir, vault) = vault();
        let alice = keys(12);
        let bob = keys(13);
        let dev = device(13);

        vault.put_segment(&segment(&alice, &dev, None, 1)).unwrap();

        assert!(
            vault
                .segments_for(bob.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .is_empty(),
            "Alice's segments were served as Bob's"
        );
        assert_eq!(
            vault.heads_for(bob.user_id()).unwrap(),
            [] as [(itsanas_crypto::DeviceId, itsanas_crypto::ObjectId, u64); 0]
        );
    }
}
