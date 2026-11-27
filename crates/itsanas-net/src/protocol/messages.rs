//! The messages themselves: what a node asks, what a peer answers, and
//! which requests are well-formed enough to act on.

use itsanas_crypto::{ChunkId, DeviceId, ObjectId, UserId};
use itsanas_store::SegmentEnvelope;
use serde::{Deserialize, Serialize};

use super::{
    MAX_CLAIM_BYTES, MAX_HAVE_BATCH, MAX_SEGMENT_BYTES, MAX_SEGMENTS_PER_REQUEST,
    MIN_PROTOCOL_VERSION,
};

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

    /// "I am going, and I will not be back soon: stop counting me."
    ///
    /// # Why a device has to be able to say this
    ///
    /// Without it, a machine switched off on purpose is indistinguishable from
    /// one that crashed: every ledger keeps counting it as a holder until its
    /// records age out of `LIVE_FOR`, seven days during which an owner believes
    /// it has a copy that nobody can serve. Saying so costs one message and
    /// lets repair start at once, from the devices still online.
    ///
    /// **Carries nothing on purpose.** The subject is the device this
    /// connection proved, exactly as for [`Request::Dropped`]: a device can
    /// only ever withdraw itself. A field naming the departing device would be
    /// a way to make somebody else's copies disappear from a ledger.
    ///
    /// It is not a promise and nothing is sanctioned on the strength of it. A
    /// device that says it is leaving and comes back is simply confirmed again
    /// by the next audit or push, like any holder.
    ///
    /// Appended last: postcard numbers variants by position.
    Leaving,

    /// "Where are this account's machines, as far as you know?"
    ///
    /// # Why a machine asks another
    ///
    /// Until this, only the coordinator said where the account's machines
    /// were. A machine that moved while the coordinator was down, or before
    /// this one's hourly read, was lost until the next read that worked. A
    /// machine of the same account that reached it since already knows.
    ///
    /// **Carries nothing, like [`Request::Leaving`]:** the subject is the
    /// device TLS proved. Who may ask is decided by the answering node from
    /// that device alone -- a machine its coordinator lists, with an owner's
    /// claim, as one of its own account -- never from anything the request
    /// says, so a stranger cannot ask about somebody else's account.
    ///
    /// Appended last: postcard numbers variants by position.
    Presences,

    /// "This is my account's word that I am its machine, and what I offer."
    ///
    /// An encoded `itsanas_coord::claim::SignedClaim`, as bytes so this crate
    /// need not know the coordinator's types. The host checks that it names
    /// the device TLS proved, is signed by the account it names and is not a
    /// withdrawal, and from then on bounds what it stores for that account
    /// by the pledge in it (§8 1c). Answered [`Response::Claim`] with the
    /// host's own claim, so the dialling side can hold the host to the same
    /// rule when it pulls the host's chunks into its vault: a machine behind
    /// a router is never dialled, and would otherwise never learn whose the
    /// peers it hosts for are. `Stored { accepted: false }` from a host that
    /// bounds nobody.
    ///
    /// Appended last: postcard numbers variants by position.
    Claim { claim: Vec<u8> },

    /// "I am going: stop counting on me for `seconds`, and tell me how much of
    /// yours rests on me alone."
    ///
    /// Sent by `itsanas leave` before [`Request::Leaving`], so the owner
    /// re-places its chunks while the departing copy still exists, and asked
    /// again until the answer is 0 or the person stops waiting. The answer is
    /// [`Response::Exposed`].
    ///
    /// **Names no device, like `Leaving`:** the subject is the device this
    /// connection proved, so a peer can only ever drain itself, and the count
    /// it gets back is about chunks it holds already. `seconds` is a wish;
    /// the owner caps it at `itsanas_store::holders::MAX_DRAIN`.
    ///
    /// Appended last: postcard numbers variants by position.
    Draining { seconds: u64 },
    /// Keep this connection open so callers who cannot reach this device can
    /// be spliced onto it through the answering one (§8 0v). Answered with
    /// [`Response::Parked`], then the same again every
    /// `relay::KEEPALIVE`, then [`Response::Incoming`] once somebody asks;
    /// after that the connection carries raw bytes, the caller's own
    /// end-to-end TLS.
    ///
    /// Appended last: postcard numbers variants by position.
    Park,
    /// Connect this connection to the parked device `to`: answered with
    /// [`Response::Spliced`], after which it carries raw bytes, the caller's
    /// end-to-end TLS with `to`. The relay sees ciphertext and can only drop
    /// or delay it.
    ///
    /// Appended last: postcard numbers variants by position.
    Splice { to: DeviceId },
    /// "Of what you hold for `owner`, these you may evict to make room for
    /// `owner` -- and only for `owner`."
    ///
    /// # Why an owner says this, and why it removes nothing
    ///
    /// A host that has stored what an account's pledge earns refuses that
    /// account's next chunk, however many of the chunks it already holds the
    /// owner has copies of to spare elsewhere. Without this the account is
    /// stuck: the spare copies keep the room the short ones need. With it the
    /// owner marks spares, and the host evicts marked chunks, oldest mark
    /// first, only when that same account is refused for room
    /// (`Vault::evict_marked`) -- never for anyone else's data and never
    /// because a peer asked. The answer is [`Response::MayEvict`].
    ///
    /// Who may say it is decided by the host from the device TLS proved:
    /// one its bound confirms speaks for `owner` (`Owners::speaks_for`), of
    /// another account than the host's own. A host that bounds nobody lets
    /// nobody mark. At most [`MAX_HAVE_BATCH`] chunks, and the host caps the
    /// marks per account per day (`Vault::mark_evictable`), so a device of
    /// the account that turns hostile empties a host only slowly.
    ///
    /// Appended last: postcard numbers variants by position.
    MayEvict { owner: UserId, chunks: Vec<ChunkId> },
    /// The claims devices of `owner` presented here, as presented
    /// ([`Response::Claims`]). Only to a caller that presented a claim for
    /// `owner` itself: who belongs to an account is the account's business.
    /// The caller verifies each signature; a host can withhold a claim and
    /// cannot forge one.
    ///
    /// Appended last: postcard numbers variants by position.
    Claims { owner: UserId },
    /// The root of what [`Request::ChunkSummary`] would answer: equal to the
    /// asker's own, the two hold the same set and nothing more is asked.
    ///
    /// Appended last: postcard numbers variants by position.
    SummaryRoot { owner: UserId },
    /// Whether this peer runs in application mode ([`Response::AppMode`]).
    /// Names no account and reads nothing it holds.
    ///
    /// Appended last: postcard numbers variants by position.
    AppMode,
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
    /// The account's machines, as [`Request::Presences`] asked: each row an
    /// encoded `itsanas_coord::claim::ClaimedPresence`, carried as bytes so
    /// this crate need not know the coordinator's types. Every row is checked
    /// by the receiver, signature by signature; nothing here is taken on the
    /// sender's word. At most [`MAX_RELAYED_ROWS`](super::MAX_RELAYED_ROWS), each at most
    /// [`MAX_RELAYED_ROW_BYTES`](super::MAX_RELAYED_ROW_BYTES).
    ///
    /// Appended last: postcard numbers variants by position.
    Presences(Vec<Vec<u8>>),
    /// The answering host's own claim, encoded, in reply to
    /// [`Request::Claim`]. At most [`MAX_CLAIM_BYTES`].
    ///
    /// Appended last: postcard numbers variants by position.
    Claim(Vec<u8>),
    /// How many of the answering owner's live chunks the caller holds with no
    /// other live, non-draining machine holding them, in reply to
    /// [`Request::Draining`]. The owner's own disk is not counted: 0 means
    /// "somebody else has every one", not "the replication target is met".
    ///
    /// Appended last: postcard numbers variants by position.
    Exposed {
        chunks: u64,
    },
    /// The park holds; repeated as a keepalive.
    ///
    /// Appended last: postcard numbers variants by position.
    Parked,
    /// Somebody, proven to the relay as `from`, is spliced onto this park:
    /// what follows is raw bytes, and `from` must prove itself again over
    /// them -- the relay's word is not taken.
    ///
    /// Appended last: postcard numbers variants by position.
    Incoming {
        from: DeviceId,
    },
    /// The splice is made: what follows is raw bytes to the parked device.
    ///
    /// Appended last: postcard numbers variants by position.
    Spliced,
    /// How many of the chunks a [`Request::MayEvict`] named are now marked
    /// evictable: held here and marked, by this request or an earlier one.
    /// A chunk not held, or past the day's cap, is not counted.
    ///
    /// Appended last: postcard numbers variants by position.
    MayEvict {
        marked: u32,
    },
    /// The answer to [`Request::Claims`]: at most
    /// `itsanas_store::vault::MAX_CLAIMS_PER_ANSWER` claims, each at most
    /// [`MAX_CLAIM_BYTES`].
    ///
    /// Appended last: postcard numbers variants by position.
    Claims(Vec<Vec<u8>>),
    /// The answer to [`Request::SummaryRoot`].
    ///
    /// Appended last: postcard numbers variants by position.
    SummaryRoot([u8; 32]),
    /// The answer to [`Request::AppMode`].
    ///
    /// Appended last: postcard numbers variants by position.
    AppMode {
        on: bool,
    },
}

/// How far one device's chain has advanced, as a peer reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub device: DeviceId,
    pub head: ObjectId,
    /// How many segments this peer holds for the device.
    pub length: u64,
}

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
            //
            // `MayEvict` joins them for the same reason: it changes state on
            // the host for every id it names.
            Self::Dropped { chunks, .. }
            | Self::Hosted { chunks, .. }
            | Self::MayEvict { chunks, .. } => !chunks.is_empty() && chunks.len() <= MAX_HAVE_BATCH,
            Self::Claim { claim } => !claim.is_empty() && claim.len() <= MAX_CLAIM_BYTES,
            Self::HaveChunks { addresses, .. } => {
                !addresses.is_empty() && addresses.len() <= MAX_HAVE_BATCH
            }
            Self::StoreSegment { envelope } => envelope.sealed_body.len() <= MAX_SEGMENT_BYTES,
            _ => true,
        }
    }
}
