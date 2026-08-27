//! The operation log: how a device's changes reach peers that were asleep.
//!
//! Direct device-to-device sync cannot work here, because the common case is
//! that the writing device is offline when the reading device wakes up. Instead
//! each device appends its changes to its own log, batches them into sealed
//! segments, and replicates those segments to whichever hosts happen to be
//! online — hosts which cannot read them.
//!
//! # What a host can and cannot do
//!
//! A segment's *body* is sealed, so a host learns nothing about which files
//! changed. Its *envelope* is plaintext, because a blind host still has to be
//! able to order segments and serve the right ones. The envelope is signed, so
//! a host cannot forge or alter one.
//!
//! That leaves withholding. A host asked for "everything after segment N" can
//! simply answer with less than it has. Two mechanisms narrow this:
//!
//! * **Chaining.** Each segment names its predecessor. A peer replaying a chain
//!   detects a hole immediately, so a host cannot drop a segment from the
//!   middle — only truncate the tail.
//! * **Sequence numbers.** Monotonic per device, so a peer that has seen
//!   sequence 40 will never silently accept a chain that stops at 30.
//!
//! Tail truncation by a host that serves an internally consistent prefix is
//! *not* solved here. It needs signed, timestamped head records gossiped
//! between peers, which is [`M3`]-and-later work. Until then a peer's guarantee
//! is "what I have is authentic and gap-free", not "what I have is current".
//!
//! [`M3`]: https://github.com/SigSeg/itsanas/blob/main/docs/ROADMAP.md

use itsanas_crypto::{
    ChunkId, DeviceId, DeviceKeys, ObjectId, SealContext, Signature, SymmetricKey, UserId,
    open_random, seal_random, verify,
};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};

/// Signature domain for segment envelopes.
pub const SEGMENT_SIGNING_DOMAIN: &str = "itsanas v1 oplog segment envelope";

/// Seal purpose for segment bodies.
pub const SEGMENT_SEAL_PURPOSE: &str = "oplog-segment";

/// What one file looks like at one point in time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Plaintext length in bytes.
    pub size: u64,
    /// Modification time, seconds since the Unix epoch.
    pub modified_unix: u64,
    /// BLAKE3 of the whole plaintext, so a materialised file can be verified
    /// end to end and not merely chunk by chunk.
    pub content_hash: [u8; 32],
    /// Chunk addresses in order. Concatenating their plaintexts reproduces the
    /// file exactly.
    pub chunks: Vec<ChunkId>,
}

/// A single change to the file tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    /// Create or replace a file.
    Upsert { path: String, entry: FileEntry },
    /// Delete a file. Kept as a tombstone rather than dropped, so a device that
    /// was offline during the delete does not resurrect the file when it
    /// returns.
    Remove { path: String },
}

impl Operation {
    /// The path this operation affects.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Upsert { path, .. } | Self::Remove { path } => path,
        }
    }
}

/// One entry in a device's log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Monotonic per device, starting at 1. Gaps mean data was lost.
    pub sequence: u64,
    /// When the writing device recorded this, seconds since the Unix epoch.
    /// Advisory only — clocks lie, so ordering never depends on it.
    pub recorded_unix: u64,
    pub operation: Operation,
}

/// The sealed body of a segment: the entries themselves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentBody {
    pub entries: Vec<LogEntry>,
}

/// The plaintext, signed wrapper a host sees and can order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentEnvelope {
    /// This segment's address.
    pub segment_id: ObjectId,
    /// Whose log this belongs to.
    pub owner: UserId,
    /// Which of the owner's devices wrote it.
    pub device: DeviceId,
    /// Sequence number of the first entry in the body.
    pub first_sequence: u64,
    /// Sequence number of the last entry in the body.
    pub last_sequence: u64,
    /// The previous segment written by this device, or `None` for the first.
    /// This is what makes a hole in the middle of a chain detectable.
    pub previous: Option<ObjectId>,
    /// Sealed [`SegmentBody`].
    pub sealed_body: Vec<u8>,
    /// Device signature over everything above.
    pub signature: Signature,
}

impl SegmentEnvelope {
    /// Canonical bytes covered by [`Self::signature`].
    ///
    /// Every variable-length field is length-prefixed, so no two distinct
    /// envelopes can produce the same payload and share a signature.
    fn signing_payload(
        segment_id: ObjectId,
        owner: UserId,
        device: DeviceId,
        first_sequence: u64,
        last_sequence: u64,
        previous: Option<ObjectId>,
        sealed_body: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 * 4 + 8 * 2 + 1 + 4 + sealed_body.len());
        out.extend_from_slice(segment_id.as_bytes());
        out.extend_from_slice(owner.as_bytes());
        out.extend_from_slice(device.as_bytes());
        out.extend_from_slice(&first_sequence.to_le_bytes());
        out.extend_from_slice(&last_sequence.to_le_bytes());
        match previous {
            Some(id) => {
                out.push(1);
                out.extend_from_slice(id.as_bytes());
            }
            None => out.push(0),
        }
        let len = u32::try_from(sealed_body.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(sealed_body);
        out
    }

    /// Seal, sign and wrap a batch of entries.
    ///
    /// `oplog_root` is the owner's log sealing root; `device` signs. The two are
    /// separate on purpose: the body is readable only by the owner, but the
    /// envelope must be verifiable by anyone holding the device's public id.
    pub fn create(
        oplog_root: &SymmetricKey,
        owner: UserId,
        device: &DeviceKeys,
        previous: Option<ObjectId>,
        entries: Vec<LogEntry>,
    ) -> Result<Self> {
        if entries.is_empty() {
            return Err(StoreError::Corrupt(
                "refusing to create an empty log segment".to_owned(),
            ));
        }

        let first_sequence = entries.first().expect("non-empty").sequence;
        let last_sequence = entries.last().expect("non-empty").sequence;

        if last_sequence < first_sequence {
            return Err(StoreError::Corrupt(
                "segment entries are not in ascending sequence order".to_owned(),
            ));
        }

        let segment_id = random_object_id()?;
        let body = SegmentBody { entries };
        let encoded = postcard::to_stdvec(&body)?;

        let sealed_body = seal_random(
            oplog_root,
            &SealContext {
                purpose: SEGMENT_SEAL_PURPOSE,
                owner,
                address: segment_id.as_bytes(),
            },
            &encoded,
        )?;

        let device_id = device.device_id();
        let payload = Self::signing_payload(
            segment_id,
            owner,
            device_id,
            first_sequence,
            last_sequence,
            previous,
            &sealed_body,
        );
        let signature = device.sign(SEGMENT_SIGNING_DOMAIN, &payload);

        Ok(Self {
            segment_id,
            owner,
            device: device_id,
            first_sequence,
            last_sequence,
            previous,
            sealed_body,
            signature,
        })
    }

    /// Check the envelope signature against the device it claims to come from.
    ///
    /// This is what a *host* can do: it proves the envelope is authentic without
    /// revealing anything about the body.
    pub fn verify_signature(&self) -> Result<()> {
        let payload = Self::signing_payload(
            self.segment_id,
            self.owner,
            self.device,
            self.first_sequence,
            self.last_sequence,
            self.previous,
            &self.sealed_body,
        );

        verify(
            self.device.as_bytes(),
            SEGMENT_SIGNING_DOMAIN,
            &payload,
            self.signature,
        )
        .map_err(|_| StoreError::SegmentSignature {
            segment: self.segment_id.short(),
        })
    }

    /// Verify, then open the body. Only the owner can do this.
    ///
    /// The signature is checked *before* decryption so a forged envelope is
    /// rejected without ever handing attacker-chosen bytes to the AEAD.
    pub fn open(&self, oplog_root: &SymmetricKey) -> Result<SegmentBody> {
        self.verify_signature()?;

        let plaintext = open_random(
            oplog_root,
            &SealContext {
                purpose: SEGMENT_SEAL_PURPOSE,
                owner: self.owner,
                address: self.segment_id.as_bytes(),
            },
            &self.sealed_body,
        )?;

        let body: SegmentBody = postcard::from_bytes(&plaintext)?;

        // The envelope is public and the body is not, so a peer must not trust
        // the envelope's claims about the body without checking them.
        let first = body.entries.first().map(|e| e.sequence);
        let last = body.entries.last().map(|e| e.sequence);

        if first != Some(self.first_sequence) || last != Some(self.last_sequence) {
            return Err(StoreError::Corrupt(format!(
                "segment {} envelope claims sequences {}..={} but its body holds {:?}..={:?}",
                self.segment_id.short(),
                self.first_sequence,
                self.last_sequence,
                first,
                last
            )));
        }

        Ok(body)
    }

    /// Serialise for storage or the wire.
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(postcard::to_stdvec(self)?)
    }

    /// Parse an envelope. Does **not** verify it; call [`Self::verify_signature`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(postcard::from_bytes(bytes)?)
    }
}

/// Walk a device's segment chain and confirm it has no holes.
///
/// `chain` must be ordered oldest first. Returns an error naming the break, so
/// an operator sees *which* host served an inconsistent chain rather than a
/// generic sync failure.
pub fn validate_chain(chain: &[SegmentEnvelope]) -> Result<()> {
    let mut expected_previous: Option<ObjectId> = None;
    let mut expected_sequence: Option<u64> = None;

    for segment in chain {
        segment.verify_signature()?;

        if let Some(expected) = expected_previous
            && segment.previous != Some(expected)
        {
            return Err(StoreError::SegmentChainBroken {
                segment: segment.segment_id.short(),
                expected: expected.short(),
                found: segment
                    .previous
                    .map_or_else(|| "none".to_owned(), |id| id.short()),
            });
        }

        if let Some(next) = expected_sequence
            && segment.first_sequence != next
        {
            return Err(StoreError::Corrupt(format!(
                "segment {} starts at sequence {} but {} was expected; \
                 {} entries are missing",
                segment.segment_id.short(),
                segment.first_sequence,
                next,
                segment.first_sequence.saturating_sub(next)
            )));
        }

        expected_previous = Some(segment.segment_id);
        expected_sequence = Some(segment.last_sequence + 1);
    }

    Ok(())
}

fn random_object_id() -> Result<ObjectId> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| StoreError::Corrupt(format!("no entropy for a segment id: {error}")))?;
    Ok(ObjectId::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::{MasterSecret, UserKeys};

    fn user(byte: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([byte; 32]))
    }

    fn device(seed: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new([seed; 32]))
    }

    fn entry(sequence: u64, path: &str) -> LogEntry {
        LogEntry {
            sequence,
            recorded_unix: 1_700_000_000 + sequence,
            operation: Operation::Upsert {
                path: path.to_owned(),
                entry: FileEntry {
                    size: 3,
                    modified_unix: 1_700_000_000,
                    content_hash: *blake3::hash(b"abc").as_bytes(),
                    chunks: vec![ChunkId::from_bytes(
                        [u8::try_from(sequence & 0xff).expect("masked to one byte"); 32],
                    )],
                },
            },
        }
    }

    fn segment(
        keys: &UserKeys,
        dev: &DeviceKeys,
        previous: Option<ObjectId>,
        sequences: std::ops::RangeInclusive<u64>,
    ) -> SegmentEnvelope {
        let entries = sequences.map(|s| entry(s, "notes.txt")).collect();
        SegmentEnvelope::create(keys.oplog_root(), keys.user_id(), dev, previous, entries)
            .expect("segment creation")
    }

    #[test]
    fn a_segment_round_trips_for_its_owner() {
        let keys = user(1);
        let dev = device(1);
        let envelope = segment(&keys, &dev, None, 1..=3);

        let body = envelope.open(keys.oplog_root()).unwrap();
        assert_eq!(body.entries.len(), 3);
        assert_eq!(body.entries[0].sequence, 1);
        assert_eq!(body.entries[2].sequence, 3);
        assert_eq!(envelope.first_sequence, 1);
        assert_eq!(envelope.last_sequence, 3);
    }

    #[test]
    fn a_host_can_verify_a_segment_without_being_able_to_read_it() {
        // This is the entire bargain: hosts police authenticity, owners read.
        let keys = user(2);
        let dev = device(2);
        let envelope = segment(&keys, &dev, None, 1..=2);

        envelope
            .verify_signature()
            .expect("a host must be able to check authenticity");

        let stranger = user(99);
        assert!(
            envelope.open(stranger.oplog_root()).is_err(),
            "a host opened a log segment it was merely storing"
        );
    }

    #[test]
    fn the_sealed_body_does_not_leak_the_path_in_plaintext() {
        let keys = user(3);
        let dev = device(3);
        let entries = vec![entry(1, "tax-return-2025.pdf")];
        let envelope =
            SegmentEnvelope::create(keys.oplog_root(), keys.user_id(), &dev, None, entries)
                .unwrap();

        let on_the_wire = envelope.encode().unwrap();
        let needle = b"tax-return-2025.pdf";

        assert!(
            !on_the_wire
                .windows(needle.len())
                .any(|window| window == needle),
            "a filename appeared in plaintext in the encoded segment; hosts \
             would learn what their peers are storing"
        );
    }

    #[test]
    fn tampering_with_any_envelope_field_invalidates_the_signature() {
        let keys = user(4);
        let dev = device(4);
        let original = segment(&keys, &dev, None, 5..=9);

        let mut sequence_changed = original.clone();
        sequence_changed.last_sequence = 10;
        assert!(sequence_changed.verify_signature().is_err());

        let mut owner_changed = original.clone();
        owner_changed.owner = user(5).user_id();
        assert!(owner_changed.verify_signature().is_err());

        let mut previous_changed = original.clone();
        previous_changed.previous = Some(ObjectId::from_bytes([7; 32]));
        assert!(previous_changed.verify_signature().is_err());

        let mut body_changed = original.clone();
        body_changed.sealed_body[10] ^= 0x01;
        assert!(body_changed.verify_signature().is_err());

        let mut id_changed = original.clone();
        id_changed.segment_id = ObjectId::from_bytes([3; 32]);
        assert!(id_changed.verify_signature().is_err());
    }

    #[test]
    fn a_segment_signed_by_another_device_is_rejected() {
        let keys = user(6);
        let honest = device(6);
        let attacker = device(7);

        let mut envelope = segment(&keys, &honest, None, 1..=1);
        // Claim to be the honest device while having been signed by the attacker.
        let payload = SegmentEnvelope::signing_payload(
            envelope.segment_id,
            envelope.owner,
            envelope.device,
            envelope.first_sequence,
            envelope.last_sequence,
            envelope.previous,
            &envelope.sealed_body,
        );
        envelope.signature = attacker.sign(SEGMENT_SIGNING_DOMAIN, &payload);

        assert!(
            envelope.verify_signature().is_err(),
            "one device forged a segment attributed to another"
        );
    }

    #[test]
    fn an_envelope_that_lies_about_its_body_is_caught_on_open() {
        // A host cannot read the body, but it can still try to make a peer
        // believe a segment covers sequences it does not. The envelope is
        // signed, so this requires a compromised device rather than a host —
        // but a compromised device must not be able to desynchronise a peer's
        // sequence tracking either.
        let keys = user(8);
        let dev = device(8);
        let entries = vec![entry(1, "a"), entry(2, "b")];

        let mut envelope =
            SegmentEnvelope::create(keys.oplog_root(), keys.user_id(), &dev, None, entries)
                .unwrap();

        envelope.last_sequence = 50;
        let payload = SegmentEnvelope::signing_payload(
            envelope.segment_id,
            envelope.owner,
            envelope.device,
            envelope.first_sequence,
            envelope.last_sequence,
            envelope.previous,
            &envelope.sealed_body,
        );
        envelope.signature = dev.sign(SEGMENT_SIGNING_DOMAIN, &payload);

        // The signature is now valid, so only the body cross-check can catch it.
        envelope.verify_signature().expect("re-signed correctly");
        let error = envelope.open(keys.oplog_root()).unwrap_err();
        assert!(
            matches!(error, StoreError::Corrupt(message) if message.contains("envelope claims")),
            "an envelope lying about its sequence range was accepted"
        );
    }

    #[test]
    fn an_empty_segment_is_refused() {
        let keys = user(9);
        let dev = device(9);
        assert!(
            SegmentEnvelope::create(keys.oplog_root(), keys.user_id(), &dev, None, Vec::new())
                .is_err(),
            "an empty segment wastes a sequence number and an object id"
        );
    }

    #[test]
    fn two_segments_with_identical_entries_are_still_distinct_objects() {
        // Randomised sealing plus a random object id: otherwise two identical
        // batches would collide in the blob store and one would be lost.
        let keys = user(10);
        let dev = device(10);

        let first = segment(&keys, &dev, None, 1..=2);
        let second = segment(&keys, &dev, None, 1..=2);

        assert_ne!(first.segment_id, second.segment_id);
        assert_ne!(first.sealed_body, second.sealed_body);
    }

    #[test]
    fn encoding_round_trips_through_the_wire_format() {
        let keys = user(11);
        let dev = device(11);
        let envelope = segment(&keys, &dev, None, 1..=4);

        let decoded = SegmentEnvelope::decode(&envelope.encode().unwrap()).unwrap();
        assert_eq!(decoded, envelope);
        decoded.verify_signature().unwrap();
        assert_eq!(decoded.open(keys.oplog_root()).unwrap().entries.len(), 4);
    }

    #[test]
    fn a_valid_chain_validates() {
        let keys = user(12);
        let dev = device(12);

        let first = segment(&keys, &dev, None, 1..=2);
        let second = segment(&keys, &dev, Some(first.segment_id), 3..=4);
        let third = segment(&keys, &dev, Some(second.segment_id), 5..=5);

        validate_chain(&[first, second, third]).expect("an honest chain must validate");
    }

    #[test]
    fn a_host_dropping_a_segment_from_the_middle_is_detected() {
        // The concrete attack: a host holds segments 1, 2 and 3 and serves only
        // 1 and 3, hiding whatever change segment 2 carried.
        let keys = user(13);
        let dev = device(13);

        let first = segment(&keys, &dev, None, 1..=2);
        let second = segment(&keys, &dev, Some(first.segment_id), 3..=4);
        let third = segment(&keys, &dev, Some(second.segment_id), 5..=6);

        let error = validate_chain(&[first, third]).unwrap_err();
        assert!(
            matches!(error, StoreError::SegmentChainBroken { .. }),
            "a host silently dropped a log segment and the peer accepted it: {error}"
        );
    }

    #[test]
    fn a_sequence_gap_is_detected_even_when_the_chain_links_up() {
        // A compromised device could chain correctly while skipping sequence
        // numbers, which would leave a peer believing it had a complete history.
        let keys = user(14);
        let dev = device(14);

        let first = segment(&keys, &dev, None, 1..=2);
        let second = segment(&keys, &dev, Some(first.segment_id), 7..=8);

        let error = validate_chain(&[first, second]).unwrap_err();
        assert!(
            matches!(&error, StoreError::Corrupt(message) if message.contains("missing")),
            "a sequence gap went undetected: {error}"
        );
    }

    #[test]
    fn a_chain_whose_first_segment_claims_a_predecessor_is_still_walked() {
        // Starting mid-chain is legitimate (a peer catching up), so validation
        // must not demand that the first segment have no predecessor.
        let keys = user(15);
        let dev = device(15);

        let first = segment(&keys, &dev, None, 1..=1);
        let second = segment(&keys, &dev, Some(first.segment_id), 2..=2);
        let third = segment(&keys, &dev, Some(second.segment_id), 3..=3);

        validate_chain(&[second, third]).expect("a partial chain must validate on its own");
    }

    #[test]
    fn an_empty_chain_is_vacuously_valid() {
        validate_chain(&[]).expect("nothing to check");
    }

    #[test]
    fn malformed_bytes_do_not_panic_the_decoder() {
        // A host controls these bytes entirely.
        assert!(SegmentEnvelope::decode(&[]).is_err());
        assert!(SegmentEnvelope::decode(&[0xff; 8]).is_err());

        let keys = user(16);
        let dev = device(16);
        let valid = segment(&keys, &dev, None, 1..=2).encode().unwrap();

        for cut in 0..valid.len() {
            // Must return an error, never panic.
            let _ = SegmentEnvelope::decode(&valid[..cut]);
        }
        for index in 0..valid.len().min(200) {
            let mut corrupted = valid.clone();
            corrupted[index] ^= 0xff;
            let _ = SegmentEnvelope::decode(&corrupted);
        }
    }
}
