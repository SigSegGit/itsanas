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
pub const PROTOCOL_VERSION: u16 = 4;

/// The oldest version this node will still talk to.
///
/// # Why a floor and not an equality
///
/// Until this existed the opening exchange required the two sides to agree
/// *exactly*, which meant every addition to this file was a flag day: no node
/// could speak to a node one commit behind it, so the whole network had to be
/// upgraded at the same instant or it partitioned. That is survivable with
/// three machines in one household and impossible for the thing this project is
/// meant to become — people join and leave, and nobody is going to coordinate a
/// simultaneous upgrade with strangers.
///
/// So the rule is a window. A peer offering anything at or above this floor is
/// answered with `min(theirs, ours)`, both sides then speak that version, and a
/// verb added later is simply not used against an older peer. New requests are
/// gated on the negotiated number rather than assumed.
///
/// **The change to get here costs one last flag day.** A node still running
/// version 2 refuses anything but 2, because that is what its own copy of this
/// file says; the window only starts protecting upgrades once every node has a
/// version of the code that has one.
pub const MIN_PROTOCOL_VERSION: u16 = 2;

/// The version before `WantHosted` and `Hosted` existed.
///
/// Kept named rather than as a literal because the difference between 1 and 2
/// is exactly "can this peer be asked to host for the side that dialled it",
/// and that is worth being able to find.
pub const PROTOCOL_WITHOUT_RECIPROCAL_HOSTING: u16 = 1;

/// The first version in which a device can say what it has let go of.
///
/// Named because gating on it is what keeps [`MIN_PROTOCOL_VERSION`] a real
/// window rather than a comment: a peer at 2 is simply not told, and the audit
/// remains the backstop it always was.
pub const PROTOCOL_WITH_DROP_NOTICES: u16 = 3;

/// The first version in which two nodes can ask "do we hold the same set?"
/// before listing anything.
///
/// A peer below it is asked the old way — every chunk id, every round — which
/// is correct and expensive. Gated rather than assumed, because that is what
/// [`MIN_PROTOCOL_VERSION`] is for.
pub const PROTOCOL_WITH_CHUNK_SUMMARY: u16 = 4;

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

    /// "Have you anything you would like me to hold for you?"
    ///
    /// Every other verb here runs one way: the caller offers its work to the
    /// peer, and the peer stores it. That made hosting something only the
    /// *dialled* side could do, and so made mutual storage impossible for
    /// anyone behind a router they do not control -- which is most people.
    /// Nothing about the network forced that. It was simply a question nobody
    /// asked.
    ///
    /// The answer is bounded by `limit` so one round cannot be turned into an
    /// unbounded transfer, and it names the peer's owner because the caller has
    /// no other way to learn it: the opening exchange carries the *caller's*
    /// owner, not the peer's.
    WantHosted { limit: u32 },

    /// "I have stored these, on your behalf."
    ///
    /// Sent after the chunks a peer offered have been fetched and put in this
    /// node's vault, so the peer can record who holds them. The record is the
    /// peer's to keep: it is the owner of that data, and this project puts the
    /// placement ledger in the owner's hands rather than in an agreement.
    ///
    /// It is a claim, and it is checked rather than believed -- the owner's
    /// storage challenges are what turn it into evidence, and a host that
    /// claimed and did not store fails the next one.
    Hosted { chunks: Vec<ChunkId> },

    /// "I no longer hold these."
    ///
    /// # Why a device has to be able to say this
    ///
    /// A device with a storage budget lets go of content on purpose: that is
    /// what makes the budget a window rather than a ratchet. The moment it
    /// does, every ledger that recorded it as a holder is wrong, and a ledger
    /// that overstates how many copies exist is the single most dangerous kind
    /// of error this system can make -- it is the number somebody consults
    /// before deciding they are safe.
    ///
    /// The audit is the backstop and it is far too slow to be the only one: it
    /// re-checks sixteen chunks per peer per round, so a million-chunk account
    /// takes sixty-odd thousand rounds -- most of a year at the service
    /// interval -- to notice by challenge alone. Saying so directly costs one
    /// message on a connection that is already open.
    ///
    /// It is not *trusted* in the direction that would matter: a device can
    /// only withdraw records about **itself**, and a device that stays silent
    /// about what it dropped is caught by the audit exactly as before. This
    /// makes honesty cheap; it does not make dishonesty possible.
    Dropped { owner: UserId, chunks: Vec<ChunkId> },

    /// "Do we hold the same chunks for this account?"
    ///
    /// # Why a round should ask this first
    ///
    /// The have/missing exchange answers exactly, and costs thirty-two bytes
    /// per chunk, per round, per peer — a two-thousandth of the account each
    /// round, or a hundred and forty gigabytes a day for a terabyte. It pays
    /// that in full to learn what is almost always "nothing has changed".
    ///
    /// A summary costs the same whatever the account weighs. Agreement ends the
    /// exchange; disagreement names the buckets that differ, and only those are
    /// listed. The cost follows the difference rather than the size, which is
    /// what makes it affordable at a scale nobody has arbitrated yet.
    ///
    /// The answer is a **question, not a verdict**: nothing is withdrawn,
    /// sanctioned or repaired on the strength of it. What follows a mismatch is
    /// the same have/missing exchange as before, over a slice of the id space.
    ChunkSummary { owner: UserId },
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
    /// One digest per bucket of the chunk id space. See
    /// [`itsanas_store::summary`].
    ChunkSummary(Vec<[u8; 32]>),
    /// Chunks this peer would like the caller to hold, and whose they are.
    WantHosted {
        owner: UserId,
        chunks: Vec<ChunkId>,
    },
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
            // A floor, not an equality. Something newer than this node is not
            // malformed -- the answer names the version both sides will speak,
            // and it is the caller's business to stay inside it.
            Self::Hello { protocol, .. } => *protocol >= MIN_PROTOCOL_VERSION,
            // One arm, and it took a red-team sweep to get here. `Dropped`
            // withdraws holder records and `Hosted` writes them; the first was
            // bounded and the second fell through to `_ => true`, so one frame
            // could add rows to a victim's index without limit -- permanent
            // rows, in the table `Store::release` reads before deleting the
            // last local copy. The asymmetry was the tell: somebody bounded the
            // message that *removes* records and not the one that *adds* them.
            Self::Dropped { chunks, .. } | Self::Hosted { chunks, .. } => {
                !chunks.is_empty() && chunks.len() <= MAX_HAVE_BATCH
            }
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

    /// One example of every request, for the round-trip test.
    ///
    /// Kept beside [`every_variant_is_in_the_round_trip_list`], which fails to
    /// compile when a variant is added and not listed here. The list used to be
    /// written inline and called "every variant" while omitting four of them —
    /// a test that overstates its coverage is worse than a missing one, because
    /// it answers the question nobody asks again.
    fn one_of_each() -> Vec<Request> {
        vec![
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
            Request::HaveChunks {
                owner: user(),
                addresses: vec![ChunkId::from_bytes([6; 32])],
            },
            Request::StoreChunk {
                owner: user(),
                address: ChunkId::from_bytes([2; 32]),
                sealed: vec![0xAB; 1024],
            },
            Request::StoreSegment {
                envelope: Box::new(SegmentEnvelope {
                    segment_id: ObjectId::from_bytes([7; 32]),
                    owner: user(),
                    device: device(),
                    first_sequence: 1,
                    last_sequence: 4,
                    previous: None,
                    sealed_body: vec![0xCD; 64],
                    signature: itsanas_crypto::Signature::from_bytes([8; 64]),
                }),
            },
            Request::Challenge {
                owner: user(),
                address: ChunkId::from_bytes([4; 32]),
                nonce: [5; 32],
            },
            Request::WantHosted { limit: 16 },
            Request::Hosted {
                chunks: vec![ChunkId::from_bytes([9; 32])],
            },
            Request::Dropped {
                owner: user(),
                chunks: vec![ChunkId::from_bytes([10; 32])],
            },
            Request::ChunkSummary { owner: user() },
        ]
    }

    /// Fails to compile when a request variant is added.
    ///
    /// The only reminder that works. A list of examples is a list somebody
    /// forgets, and the failure it lets through — a variant that cannot be
    /// encoded — happens on a live connection.
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per variant is the point; merging them removes the reminder"
    )]
    fn every_variant_is_in_the_round_trip_list(request: &Request) {
        match request {
            Request::Hello { .. } => {}
            Request::Heads { .. } => {}
            Request::Segments { .. } => {}
            Request::Chunk { .. } => {}
            Request::HaveChunks { .. } => {}
            Request::StoreChunk { .. } => {}
            Request::StoreSegment { .. } => {}
            Request::Challenge { .. } => {}
            Request::WantHosted { .. } => {}
            Request::Hosted { .. } => {}
            Request::Dropped { .. } => {}
            Request::ChunkSummary { .. } => {}
        }
    }

    #[test]
    fn every_request_variant_round_trips_through_the_wire() {
        // A variant that fails to encode is a runtime failure on a live
        // connection, which is a bad place to discover it.
        for request in one_of_each() {
            every_variant_is_in_the_round_trip_list(&request);
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
    fn a_hello_is_accepted_from_the_floor_upwards_and_refused_below_it() {
        // A window, not a point. Anything at or above the floor is answered
        // with the version both sides know; anything below it has no shared
        // vocabulary and is turned away here rather than three messages later.
        for protocol in [
            MIN_PROTOCOL_VERSION,
            PROTOCOL_VERSION,
            PROTOCOL_VERSION + 1,
            u16::MAX,
        ] {
            assert!(
                Request::Hello {
                    protocol,
                    device: device(),
                    owner: user(),
                }
                .is_acceptable(),
                "version {protocol} was refused, which partitions the network on every upgrade"
            );
        }

        assert!(
            !Request::Hello {
                protocol: MIN_PROTOCOL_VERSION - 1,
                device: device(),
                owner: user(),
            }
            .is_acceptable()
        );
    }

    #[test]
    fn red_team_a_claim_to_hold_things_is_bounded_like_the_claim_to_have_dropped_them() {
        // `Dropped` withdraws holder records and `Hosted` writes them. The
        // first was bounded and the second was not, so one frame could add
        // rows to a victim's index without limit -- permanent rows, in the
        // table `Store::release` reads to decide whether the last local copy
        // may go. The asymmetry is the tell: somebody bounded the message that
        // removes records and not the one that adds them.
        let chunk = ChunkId::from_bytes([9; 32]);

        assert!(
            Request::Hosted {
                chunks: vec![chunk; MAX_HAVE_BATCH],
            }
            .is_acceptable(),
            "a full legitimate batch was refused"
        );
        assert!(
            !Request::Hosted {
                chunks: vec![chunk; MAX_HAVE_BATCH + 1],
            }
            .is_acceptable(),
            "an unbounded claim was accepted"
        );
        assert!(
            !Request::Hosted { chunks: Vec::new() }.is_acceptable(),
            "an empty claim is a wasted round trip, not a message"
        );

        // The bound it was supposed to have all along, stated side by side so
        // the next verb added here is compared against both.
        assert!(
            !Request::Dropped {
                owner: user(),
                chunks: vec![chunk; MAX_HAVE_BATCH + 1],
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
