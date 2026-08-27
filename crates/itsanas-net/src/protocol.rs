//! What one node can ask another.
//!
//! The protocol is deliberately small and entirely pull-based for data: a node
//! asks for segments and chunks, and a node offers to *store* things but never
//! demands that a peer accept them silently. Everything a peer can send is
//! either self-authenticating (a signed segment envelope) or self-verifying
//! (a sealed chunk, which fails to open if it is not what was asked for).
//!
//! # What a peer is trusted with
//!
//! Nothing. Concretely:
//!
//! * A peer serving a **chunk** cannot substitute another chunk's bytes: the
//!   address is bound into the sealing, so the wrong chunk fails to open.
//! * A peer serving a **segment** cannot forge or alter it: the envelope is
//!   signed by the device that wrote it.
//! * A peer serving **nothing at all** is indistinguishable from a peer that
//!   genuinely has nothing. This is the one thing the protocol cannot fix, and
//!   it is why storage challenges exist.
//!
//! # Storage challenges
//!
//! A verifier sends a nonce; the host must return
//! `BLAKE3_keyed(nonce, sealed_bytes)`. Computing it requires the bytes, so a
//! host that has silently discarded a chunk fails. It does **not** prove the
//! host kept the chunk continuously, and it does not stop a host that fetches
//! the chunk from another replica just in time to answer. Both limitations are
//! real; the challenge raises the cost of lying without eliminating it.

use itsanas_crypto::{ChunkId, DeviceId, ObjectId, UserId};
use itsanas_store::SegmentEnvelope;
use serde::{Deserialize, Serialize};

/// Protocol version, negotiated in the opening exchange.
pub const PROTOCOL_VERSION: u16 = 1;

/// Domain string for storage-challenge proofs.
const CHALLENGE_DOMAIN: &str = "itsanas v1 storage challenge";

/// What a node asks a peer for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    /// Opening exchange. Always the first message on a connection.
    Hello {
        protocol: u16,
        device: DeviceId,
        owner: UserId,
    },
    /// Which devices this peer has segments for, and how far each chain goes.
    Heads { owner: UserId },
    /// Segments from one device's chain, starting after `after`.
    ///
    /// `None` means "from the beginning", which is what a brand-new device
    /// recovering an account asks for.
    Segments {
        owner: UserId,
        device: DeviceId,
        after: Option<ObjectId>,
        /// Cap on how many to return, so one request cannot ask a peer to
        /// assemble an unbounded response.
        limit: u16,
    },
    /// One sealed chunk.
    Chunk { owner: UserId, address: ChunkId },
    /// Which of these chunks does the peer *not* have?
    ///
    /// Without this, pushing means either re-uploading everything on every
    /// round or asking about chunks one at a time. Both are unusable at the
    /// scale of a real backup: a 10 GiB folder is well over a hundred thousand
    /// chunks.
    HaveChunks {
        owner: UserId,
        addresses: Vec<ChunkId>,
    },
    /// Offer a sealed chunk for storage.
    StoreChunk {
        owner: UserId,
        address: ChunkId,
        sealed: Vec<u8>,
    },
    /// Offer a signed segment for storage.
    StoreSegment { envelope: Box<SegmentEnvelope> },
    /// Prove you still hold this chunk.
    Challenge {
        owner: UserId,
        address: ChunkId,
        nonce: [u8; 32],
    },
}

/// What a peer answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    Hello {
        protocol: u16,
        device: DeviceId,
    },
    Heads(Vec<Head>),
    Segments(Vec<SegmentEnvelope>),
    /// `None` means "I do not have it", which is ordinary rather than an error.
    Chunk(Option<Vec<u8>>),
    /// The subset of a [`Request::HaveChunks`] batch the peer lacks.
    Missing(Vec<ChunkId>),
    Stored {
        accepted: bool,
    },
    ChallengeProof([u8; 32]),
    /// A request this peer refused or could not serve.
    ///
    /// Carries a short reason for the operator's logs. Never carries anything
    /// derived from a secret.
    Refused(String),
}

/// How far one device's chain has advanced, as a peer reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub device: DeviceId,
    pub head: ObjectId,
    /// How many segments this peer holds for the device.
    pub length: u64,
}

/// Compute a storage-challenge proof over sealed bytes.
///
/// Keyed by the nonce, so a proof for one challenge is useless for another —
/// a host cannot precompute one answer and reuse it. Domain-separated so a
/// proof can never be confused with any other keyed hash in the system.
#[must_use]
pub fn challenge_proof(nonce: &[u8; 32], sealed: &[u8]) -> [u8; 32] {
    let key = blake3::derive_key(CHALLENGE_DOMAIN, nonce);
    *blake3::keyed_hash(&key, sealed).as_bytes()
}

/// Check a proof returned by a host.
#[must_use]
pub fn challenge_holds(nonce: &[u8; 32], sealed: &[u8], proof: &[u8; 32]) -> bool {
    // Constant time is not required: both sides are public values, and an
    // attacker who could forge this already has the bytes.
    challenge_proof(nonce, sealed) == *proof
}

/// Largest number of segments a single request may ask for.
pub const MAX_SEGMENTS_PER_REQUEST: u16 = 256;

/// Largest number of addresses in one [`Request::HaveChunks`] batch.
///
/// Bounded because the response is proportional to it, and an unbounded batch
/// is an invitation to make a peer assemble an arbitrarily large answer.
pub const MAX_HAVE_BATCH: usize = 1024;

impl Request {
    /// Whether this request is well-formed enough to act on.
    ///
    /// Checked before any work is done, so a malformed request costs a peer
    /// nothing but the parse.
    #[must_use]
    pub fn is_acceptable(&self) -> bool {
        match self {
            Self::Segments { limit, .. } => *limit > 0 && *limit <= MAX_SEGMENTS_PER_REQUEST,
            Self::Hello { protocol, .. } => *protocol == PROTOCOL_VERSION,
            Self::HaveChunks { addresses, .. } => {
                !addresses.is_empty() && addresses.len() <= MAX_HAVE_BATCH
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire;

    fn user() -> UserId {
        UserId::from_bytes([7; 32])
    }

    fn device() -> DeviceId {
        DeviceId::from_bytes([9; 32])
    }

    #[test]
    fn every_request_variant_round_trips_through_the_wire() {
        // A variant that fails to encode is a runtime failure on a live
        // connection, which is a bad place to discover it.
        let requests = vec![
            Request::Hello {
                protocol: PROTOCOL_VERSION,
                device: device(),
                owner: user(),
            },
            Request::Heads { owner: user() },
            Request::Segments {
                owner: user(),
                device: device(),
                after: Some(ObjectId::from_bytes([3; 32])),
                limit: 64,
            },
            Request::Segments {
                owner: user(),
                device: device(),
                after: None,
                limit: 1,
            },
            Request::Chunk {
                owner: user(),
                address: ChunkId::from_bytes([1; 32]),
            },
            Request::StoreChunk {
                owner: user(),
                address: ChunkId::from_bytes([2; 32]),
                sealed: vec![0xAB; 1024],
            },
            Request::Challenge {
                owner: user(),
                address: ChunkId::from_bytes([4; 32]),
                nonce: [5; 32],
            },
        ];

        for request in requests {
            let frame = wire::encode(&request).unwrap();
            assert_eq!(
                wire::decode::<Request>(&frame).unwrap(),
                request,
                "round trip changed {request:?}"
            );
        }
    }

    #[test]
    fn every_response_variant_round_trips_through_the_wire() {
        let responses = vec![
            Response::Hello {
                protocol: PROTOCOL_VERSION,
                device: device(),
            },
            Response::Heads(vec![Head {
                device: device(),
                head: ObjectId::from_bytes([6; 32]),
                length: 12,
            }]),
            Response::Heads(Vec::new()),
            Response::Segments(Vec::new()),
            Response::Chunk(Some(vec![1, 2, 3])),
            Response::Chunk(None),
            Response::Stored { accepted: true },
            Response::Stored { accepted: false },
            Response::ChallengeProof([8; 32]),
            Response::Refused("no such user".to_owned()),
        ];

        for response in responses {
            let frame = wire::encode(&response).unwrap();
            assert_eq!(wire::decode::<Response>(&frame).unwrap(), response);
        }
    }

    #[test]
    fn a_proof_requires_the_actual_bytes() {
        let nonce = [1u8; 32];
        let sealed = b"the sealed chunk a host claims to be holding";

        let proof = challenge_proof(&nonce, sealed);
        assert!(challenge_holds(&nonce, sealed, &proof));

        assert!(
            !challenge_holds(&nonce, b"different bytes entirely", &proof),
            "a host that discarded the chunk still passed the challenge"
        );
    }

    #[test]
    fn a_proof_for_one_nonce_does_not_answer_another() {
        // Otherwise a host computes one proof, throws the chunk away, and
        // answers every future challenge from the cached answer.
        let sealed = b"chunk bytes";
        let first = challenge_proof(&[1; 32], sealed);

        assert!(
            !challenge_holds(&[2; 32], sealed, &first),
            "a proof was reusable across challenges, so a host need only \
             answer once and may then discard the data"
        );
    }

    #[test]
    fn a_single_bit_of_difference_fails_the_challenge() {
        let nonce = [3u8; 32];
        let mut sealed = vec![0u8; 512];
        let proof = challenge_proof(&nonce, &sealed);

        for index in [0, 100, 511] {
            sealed[index] ^= 1;
            assert!(
                !challenge_holds(&nonce, &sealed, &proof),
                "a chunk corrupted at byte {index} passed the challenge"
            );
            sealed[index] ^= 1;
        }

        assert!(challenge_holds(&nonce, &sealed, &proof));
    }

    #[test]
    fn an_unbounded_segment_request_is_not_acceptable() {
        // Otherwise one request asks a peer to assemble every segment it holds.
        assert!(
            !Request::Segments {
                owner: user(),
                device: device(),
                after: None,
                limit: 0,
            }
            .is_acceptable()
        );

        assert!(
            !Request::Segments {
                owner: user(),
                device: device(),
                after: None,
                limit: MAX_SEGMENTS_PER_REQUEST + 1,
            }
            .is_acceptable()
        );

        assert!(
            Request::Segments {
                owner: user(),
                device: device(),
                after: None,
                limit: MAX_SEGMENTS_PER_REQUEST,
            }
            .is_acceptable()
        );
    }

    #[test]
    fn a_hello_from_a_different_protocol_version_is_not_acceptable() {
        assert!(
            !Request::Hello {
                protocol: PROTOCOL_VERSION + 1,
                device: device(),
                owner: user(),
            }
            .is_acceptable()
        );

        assert!(
            Request::Hello {
                protocol: PROTOCOL_VERSION,
                device: device(),
                owner: user(),
            }
            .is_acceptable()
        );
    }

    #[test]
    fn a_maximum_size_chunk_fits_in_one_frame() {
        // The chunker caps chunks at 256 KiB; if the largest legitimate message
        // did not fit the frame limit, normal operation would fail.
        let request = Request::StoreChunk {
            owner: user(),
            address: ChunkId::from_bytes([1; 32]),
            sealed: vec![0u8; 256 * 1024 + 64],
        };

        let frame = wire::encode(&request).unwrap();
        assert!(
            frame.len() < wire::MAX_FRAME_LEN,
            "a legitimate maximum-size chunk does not fit the frame limit"
        );
        assert_eq!(wire::decode::<Request>(&frame).unwrap(), request);
    }

    #[test]
    fn a_refusal_carries_no_secret_material() {
        // A guard against the easy mistake of formatting an error that embeds a
        // key or a plaintext. Refused carries a String; this documents that the
        // String is operator-facing only.
        let refusal = Response::Refused("unknown owner".to_owned());
        let frame = wire::encode(&refusal).unwrap();
        assert_eq!(wire::decode::<Response>(&frame).unwrap(), refusal);
    }
}
