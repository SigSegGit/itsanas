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

mod messages;

pub use messages::{Head, Request, Response};

/// Protocol version, negotiated in the opening exchange.
pub const PROTOCOL_VERSION: u16 = 13;

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
///
/// **Raised from 2 to 4 on 2026-09-14, because the window was not one.** The
/// wire writes an enum variant as its position, and version 4 inserted
/// `Response::ChunkSummary` in the middle of `Response`: `WantHosted` moved from
/// 7 to 8 and `Refused` from 8 to 9. A version-2 peer's `WantHosted` decoded
/// here as a `ChunkSummary` ("Hit the end of buffer") and its `Refused` as a
/// `WantHosted`; the fleet logged that on every round for a week. Versions 2 and
/// 3 share the old order, so refusing below 4 turns garbage into one line
/// naming the two versions. The numbers are now pinned by
/// `red_team_every_variant_keeps_its_number_on_the_wire`: append, never insert.
pub const MIN_PROTOCOL_VERSION: u16 = 4;

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

/// The first version in which a device can say it is leaving.
///
/// A peer at 4 does not know [`Request::Leaving`] and would close the
/// connection on it, so it is simply not told: its records about the departing
/// device age out of [`itsanas_store::holders::LIVE_FOR`] exactly as a crash's
/// would. Gated, not assumed, because the window is what makes a mixed fleet
/// work at all.
pub const PROTOCOL_WITH_LEAVING: u16 = 5;

/// The first version in which a machine can ask another of its account where
/// the account's machines are ([`Request::Presences`]).
///
/// A peer at 5 does not know the request and would close the connection on
/// it, so it is not asked: the coordinator stays the only source, as before.
pub const PROTOCOL_WITH_PRESENCES: u16 = 6;

/// The first version in which a device presents its owner's claim
/// ([`Request::Claim`]) before asking a host to store anything.
///
/// A host that bounds what it stores per account (§8 1c) refuses `StoreChunk`
/// and `StoreSegment` from a device that has not presented one, so a peer
/// below this version can sync and read but no longer store on such a host.
/// That is the flag day Nicolas accepted on 2026-10-04: otherwise a modified
/// client escapes every bound by claiming to be old.
pub const PROTOCOL_WITH_CLAIMS: u16 = 7;

/// The first version in which a departing device can drain
/// ([`Request::Draining`]) before it leaves (§8 0w (7)).
///
/// A peer at 7 does not know the request, so it is not asked: `leave` counts
/// it as an owner it could not ask, and its records go at `Leaving` as before.
pub const PROTOCOL_WITH_DRAINING: u16 = 8;

/// The first version in which a peer can relay for a machine nobody can
/// reach ([`Request::Park`], [`Request::Splice`], §8 0v).
///
/// A peer at 8 does not know either request, so it is never asked to relay
/// and is never told to dial a `relay:` address: that waits, like
/// several addresses did, until the fleet speaks 9.
pub const PROTOCOL_WITH_RELAY: u16 = 9;

/// The first version in which an owner can tell a host which of its chunks
/// the host may evict to make room for that owner ([`Request::MayEvict`]).
///
/// A peer at 9 does not know the request and would close the connection on
/// it, so it is never asked: the owner keeps counting the copies it would
/// have released there, which is the conservative side -- more copies, never
/// fewer -- and the host stays as full for that account as it was.
pub const PROTOCOL_WITH_EVICTION: u16 = 10;

/// The first version in which a host hands an account the claims its devices
/// presented ([`Request::Claims`]), so the account's other machines can admit
/// a sibling they never met to their roster (HANDOVER §10 7).
///
/// A peer at 10 is not asked: the owner learns siblings only from sessions
/// with them, and a chain from one it never met waits.
pub const PROTOCOL_WITH_ROSTER: u16 = 11;

/// The first version in which a peer answers the root of its chunk summary
/// alone ([`Request::SummaryRoot`]): 32 bytes where the 256 digests were about
/// 8 KB, asked every round of every pair whatever changed. An idle phone round
/// on Wi-Fi was 13.9 KB, most of it this (`tests/idle.rs`, 2026-11-23).
///
/// A peer at 11 is asked the whole summary, as before.
pub const PROTOCOL_WITH_SUMMARY_ROOT: u16 = 12;

/// The first version in which a peer says whether it runs in application
/// mode ([`Request::AppMode`]): two machines that both do are each other's
/// privileged hosts, dialled first and every minute (HANDOVER §8 00).
///
/// A peer at 12 is not asked, and is taken as not in application mode.
pub const PROTOCOL_WITH_APP_MODE: u16 = 13;

/// Largest encoded claim a host reads. A `SignedClaim` is two ids, three
/// integers, a flag and a signature: under 200 bytes.
pub const MAX_CLAIM_BYTES: usize = 512;

/// Most rows one [`Response::Presences`] carries.
///
/// The receiver's address book holds 256 devices (`itsanas_node::contact`),
/// so more rows than that can only be padding.
pub const MAX_RELAYED_ROWS: usize = 256;

/// Largest encoded row in a [`Response::Presences`].
///
/// A row is a presence and a claim: two ids, an address of at most 255 bytes,
/// two signatures and a few integers -- under 500 bytes. Double that, and a
/// row that needs more is not one.
pub const MAX_RELAYED_ROW_BYTES: usize = 1024;

/// Domain string for storage-challenge proofs.
const CHALLENGE_DOMAIN: &str = "itsanas v1 storage challenge";

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

/// Most bytes of segments one [`Response::Segments`] carries, and so the
/// largest sealed body a [`Request::StoreSegment`] may hold: one frame, less
/// room for the envelope around it. A page stops before passing it and holds
/// at least one segment, so every stored segment can be served.
pub const MAX_SEGMENT_BYTES: usize = crate::wire::MAX_FRAME_LEN - 64 * 1024;

/// Largest number of addresses in one [`Request::HaveChunks`] batch.
///
/// Bounded because the response is proportional to it, and an unbounded batch
/// is an invitation to make a peer assemble an arbitrarily large answer.
pub const MAX_HAVE_BATCH: usize = 1024;

#[cfg(test)]
mod message_tests;

#[cfg(test)]
mod tests {
    use super::*;

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
    fn eviction_is_a_verb_of_the_current_version_and_of_no_older_one() {
        // The client gates `MayEvict` on this number; a version bump that
        // forgot it would send the verb to a peer that closes the connection
        // on it. Sabotage: leave `PROTOCOL_VERSION` at 11.
        assert_eq!(PROTOCOL_WITH_APP_MODE, PROTOCOL_VERSION);
        const { assert!(PROTOCOL_WITH_APP_MODE > PROTOCOL_WITH_SUMMARY_ROOT) };
        const { assert!(PROTOCOL_WITH_SUMMARY_ROOT > PROTOCOL_WITH_ROSTER) };
        const { assert!(PROTOCOL_WITH_ROSTER > PROTOCOL_WITH_EVICTION) };
        const { assert!(PROTOCOL_WITH_EVICTION > PROTOCOL_WITH_RELAY) };
    }
}
