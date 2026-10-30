//! Answering a peer.
//!
//! Request in, response out. No sockets, no async, no I/O beyond the store and
//! the vault — which means every rule about what a peer may and may not obtain
//! can be tested directly, including the hostile cases, without standing up a
//! network.
//!
//! # The rule this module exists to enforce
//!
//! **A peer may fetch anything it asks for, because everything it can fetch is
//! either sealed or signed.** There is no access-control list, and there
//! deliberately isn't one: an access-control list is a thing that can be got
//! wrong, and the guarantee here does not need one. A chunk is ciphertext bound
//! to an address; a segment is ciphertext in a signed envelope. Serving either
//! to the wrong person reveals nothing.
//!
//! What the service *does* enforce is narrower and concerns resources rather
//! than secrecy: request limits, quota, and refusing to store things it did not
//! agree to store.

use itsanas_crypto::{ChunkId, DeviceId, UserId};
use itsanas_store::{Store, Vault};

use crate::{
    error::Result,
    protocol::{
        Head, MAX_SEGMENTS_PER_REQUEST, PROTOCOL_VERSION, Request, Response, challenge_proof,
    },
};

/// What a host answers when storing would take it past its pledge.
///
/// A constant because the offering side reads it back: `PeerClient` turns this
/// exact refusal into [`crate::transport::Refusal::PledgeFull`], so an owner can
/// say "that host has no room" rather than "that host said no". Changing the
/// text makes every deployed client report a full host as a hostile one.
pub const PLEDGE_EXHAUSTED: &str = "pledged capacity exhausted";

/// What a host answers an account past what its pledge earns there (§8 1c),
/// followed by the figures, for the operator's log.
///
/// Here rather than in the node's claim book, which writes it, because the
/// offering side reads it back: `PeerClient` matches it as a prefix and turns
/// it into [`crate::transport::Refusal::AccountFull`], and this service reads
/// it to know when evicting the account's marked chunks can make room. Two
/// copies of the text would drift, and a full account would read as a host
/// that rejects what it is sent.
pub const ACCOUNT_FULL: &str = "this account has stored here what its pledge earns";

/// What a host answers a device the coordinator says was withdrawn.
pub const WITHDRAWN: &str = "the coordinator says this device was withdrawn by its account";
/// What a host answers a device of another account it has not confirmed
/// with the coordinator lately.
pub const UNCONFIRMED: &str = "this host has not confirmed this device with the coordinator \
     lately; it stores here once it has (the next round, if the coordinator answers)";

/// What a host answers an account whose offer its audits contradicted.
pub const CONTRADICTED: &str = "a device of this account failed this host's storage audits; \
     it stores here only what its devices have proved they hold for this host";

/// How much foreign data this node has agreed to hold, in bytes.
///
/// Enforced when accepting, not when serving: a node that has already taken
/// someone's data must keep serving it even if the quota is later lowered,
/// because silently refusing to serve data you accepted is how peers lose
/// files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pledge {
    pub bytes: u64,
}

impl Pledge {
    /// A node that has pledged nothing still syncs its own devices; it just
    /// hosts nothing for anyone else.
    pub const NONE: Self = Self { bytes: 0 };

    /// A pledge of exactly this many bytes.
    #[must_use]
    pub const fn bytes(bytes: u64) -> Self {
        Self { bytes }
    }

    #[must_use]
    pub const fn gigabytes(count: u64) -> Self {
        Self {
            bytes: count * 1024 * 1024 * 1024,
        }
    }
}

/// Where a node keeps what it knows of its account's machines, for
/// [`Request::Presences`].
///
/// A trait because the book lives in `itsanas-node` and its rows are the
/// coordinator's types, which this crate does not depend on: the service asks,
/// the book decides.
pub trait Relay: Sync {
    /// The rows to hand `caller`, each an encoded claimed presence; `None` if
    /// `caller` is not a machine of this node's account as the book knows it.
    ///
    /// `caller` is the device TLS proved, never one a request named.
    fn presences_for(&self, caller: DeviceId) -> Option<Vec<Vec<u8>>>;
}

/// The refusal to a read for an account the caller has not shown it belongs
/// to. Names the fix, because the usual cause is an older client.
pub const NOT_YOUR_ACCOUNT: &str = "present this account's claim first: only its own devices read what is held for it \
     (update ITSaNAS on this machine if it already is one)";

/// The refusal of a node whose vault's disk is away (a USB disk unplugged)
/// to anything about another account's data. Its own words, because a caller
/// must read it as "not here", never as "lost your data": an audit answered
/// with it is no failed audit (`PeerClient::challenge`).
pub const HOSTING_AWAY: &str = "this machine's hosting disk is away; ask again later";

/// The account whose hosted data a request touches on this node, if any:
/// what an away node refuses ([`HOSTING_AWAY`]).
#[must_use]
pub fn hosted_owner(request: &Request) -> Option<UserId> {
    match request {
        Request::StoreChunk { owner, .. } | Request::MayEvict { owner, .. } => Some(*owner),
        Request::StoreSegment { envelope } => Some(envelope.owner),
        _ => read_owner(request),
    }
}

/// The account a request reads from this host, if it reads one.
///
/// Exhaustive on purpose: a request added without a decision here does not
/// compile, and a reading one wrongly put in the second arm is what
/// `red_team_every_request_naming_an_account_is_scoped_or_says_why_not` reads.
#[must_use]
pub const fn read_owner(request: &Request) -> Option<UserId> {
    match request {
        Request::Heads { owner }
        | Request::Segments { owner, .. }
        | Request::Chunk { owner, .. }
        | Request::HaveChunks { owner, .. }
        | Request::ChunkSummary { owner }
        | Request::Challenge { owner, .. }
        | Request::Claims { owner } => Some(*owner),
        // Writes, bounded by `Owners::admits` and the pledge; `Dropped`,
        // `Leaving` and `Draining` act on the caller's own records only;
        // `MayEvict` needs `speaks_for`; the rest name no account.
        Request::StoreChunk { .. }
        | Request::StoreSegment { .. }
        | Request::Dropped { .. }
        | Request::MayEvict { .. }
        | Request::Hello { .. }
        | Request::WantHosted { .. }
        | Request::Hosted { .. }
        | Request::Leaving
        | Request::Presences
        | Request::Claim { .. }
        | Request::Draining { .. }
        | Request::Park
        | Request::Splice { .. } => None,
    }
}

/// An [`Owners`] that bounds nobody and lets every caller read every account.
///
/// For tests of transport, sync and placement, which are about something
/// else; never for a node (`red_team_no_node_serves_with_the_unscoped_owners`
/// greps the crates that run one).
#[derive(Clone, Copy, Debug)]
pub struct Unscoped;

impl Owners for Unscoped {
    fn present(&self, _: DeviceId, _: &[u8], _: &Vault) -> std::result::Result<(), String> {
        Ok(())
    }

    fn admits(
        &self,
        _: DeviceId,
        _: UserId,
        _: u64,
        _: &Store,
        _: &Vault,
        _: u64,
    ) -> std::result::Result<(), String> {
        Ok(())
    }

    fn confirmed_account(&self, _: DeviceId) -> Option<UserId> {
        None
    }

    fn may_read(&self, _: DeviceId, _: UserId) -> bool {
        true
    }
}

/// How a host decides how much it stores for one account (§8 1c).
///
/// A trait for the reason [`Relay`] is one: the claims are the coordinator's
/// types, which this crate does not depend on, and the rule is the node's.
/// Before this existed a host bounded only *itself* -- its own pledge -- so a
/// rebuilt client that pledged nothing was served until every host was full.
pub trait Owners: Sync {
    /// Take a claim `caller` presented ([`Request::Claim`]). `Err` carries
    /// the reason for the refusal, for the peer's log.
    ///
    /// # Errors
    ///
    /// When the claim does not check out for `caller`.
    fn present(
        &self,
        caller: DeviceId,
        claim: &[u8],
        vault: &Vault,
    ) -> std::result::Result<(), String>;

    /// The account a claim `caller` presented here names, if it verified and
    /// was not withdrawn, whether or not a coordinator confirmed it since: a
    /// signature by the account's key is what membership is. `None` by
    /// default, which scopes everything that asks to nobody.
    fn presented_for(&self, _caller: DeviceId) -> Option<UserId> {
        None
    }

    /// Whether `presented` tells nothing that `kept`, the claim this host
    /// keeps for the same device, does not: the same claim re-signed a
    /// little later, or an older one. Such a claim is not written again
    /// (an idle round's commit). `false` by default, which keeps every claim.
    fn adds_nothing(&self, _kept: &[u8], _presented: &[u8]) -> bool {
        false
    }

    /// Whether `caller` may read what this host holds for `owner`: its
    /// segments, heads, chunks, summaries and audit proofs. A device of that
    /// account, by a claim it presented ([`Self::presented_for`]); nobody
    /// else, a host of the same account included (ROADMAP, "reads are not
    /// scoped to the account").
    fn may_read(&self, caller: DeviceId, owner: UserId) -> bool {
        self.presented_for(caller) == Some(owner)
    }

    /// Whether `caller` may store `incoming` more bytes for `owner` here.
    /// `Err` is the refusal sent back. Called under the storing lock.
    ///
    /// # Errors
    ///
    /// When the account has no room left on this host, or `caller` never
    /// presented a claim for it.
    fn admits(
        &self,
        caller: DeviceId,
        owner: UserId,
        incoming: u64,
        store: &Store,
        vault: &Vault,
        host_pledge: u64,
    ) -> std::result::Result<(), String>;

    /// The account `caller` is a device of, if its claim was presented here
    /// and confirmed by the coordinator lately: who a host relays for
    /// (§8 0v). `None` by default: relay for nobody but the own account.
    fn confirmed_account(&self, _caller: DeviceId) -> Option<UserId> {
        None
    }

    /// Whether `caller` speaks for `owner` here: its claim for `owner` was
    /// presented and its standing confirmed, the test [`Self::admits`] puts a
    /// store to. What lets a device mark `owner`'s chunks evictable
    /// ([`Request::MayEvict`]). `false` by default: an implementation that
    /// does not say lets nobody mark, which costs room, never copies.
    fn speaks_for(&self, _caller: DeviceId, _owner: UserId) -> bool {
        false
    }
}

/// What a host that bounds accounts answers a device that has presented no
/// claim: one that predates [`crate::protocol::PROTOCOL_WITH_CLAIMS`], or a
/// client that left it out to escape the bound.
pub const UPDATE_REQUIRED: &str =
    "this host stores only for devices that present their account's claim: update ITSaNAS";

fn not_relayed() -> Response {
    Response::Refused(crate::relay::NOT_RELAYED.to_owned())
}

/// What a host answers a [`Request::MayEvict`] from a device it does not
/// count as speaking for that account, or when it bounds nobody.
pub const NOT_YOURS_TO_EVICT: &str =
    "this host lets only a confirmed device of that account mark its chunks evictable";

/// What a node that keeps no book answers [`Request::Presences`] with.
pub const NO_BOOK: &str = "this node relays no presences";

/// What a node answers a device it does not count as its account's.
///
/// The same words whether the caller is a stranger or a machine of the account
/// the coordinator has not listed yet: which of the two it is would be worth
/// knowing to somebody mapping accounts.
pub const NOT_YOURS: &str = "not a machine of this account, as far as this node knows";

/// Answers peer requests from a node's own store and its vault.
pub struct PeerService<'a> {
    store: &'a Store,
    vault: &'a Vault,
    pledge: Pledge,
    relay: Option<&'a dyn Relay>,
    owners: Option<&'a dyn Owners>,
    /// The round's placement, swapped in by the daemon; empty, or absent,
    /// and `WantHosted` answers as before placement.
    placement: Option<&'a crate::placement::PlacementSlot>,
    /// This host's own claim, encoded, sent back to a device that presents
    /// one ([`Request::Claim`]).
    own_claim: Option<Vec<u8>>,
    /// This node's vault's disk is away: it serves its own account and
    /// nobody else's ([`HOSTING_AWAY`]).
    away: bool,
    /// What the disk can still take, when the host knows (a pledge the disk
    /// can no longer hold must stop the chunks, not fail their write).
    disk_room: Option<&'a (dyn Fn() -> Option<u64> + Sync)>,
    /// The newest protocol this service admits to in `Hello`.
    protocol: u16,
    /// Held across "is there room" and "store it".
    ///
    /// The listener serves connections concurrently, and the pledge check is a
    /// read followed by a write. Without this, every connection could see the
    /// same free space and all of them fill it, so a host would take on its
    /// pledge plus one object per open connection -- up to 32 times the
    /// frame limit past what it promised its own disk.
    storing: std::sync::Mutex<()>,
}

impl std::fmt::Debug for PeerService<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerService")
            .field("device", &self.store.device_id())
            .field("pledge", &self.pledge)
            .field("relays", &self.relay.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> PeerService<'a> {
    #[must_use]
    pub const fn new(store: &'a Store, vault: &'a Vault, pledge: Pledge) -> Self {
        Self {
            store,
            vault,
            pledge,
            relay: None,
            owners: None,
            placement: None,
            away: false,
            own_claim: None,
            disk_room: None,
            protocol: PROTOCOL_VERSION,
            storing: std::sync::Mutex::new(()),
        }
    }

    /// Answer `Hello` as a peer of protocol `version` at most would: how a
    /// test stands in for a machine that has not been upgraded, to prove a
    /// client does not ask it what it cannot know. Requests are still served
    /// as by this version; only the version admitted to changes.
    #[doc(hidden)]
    #[must_use]
    pub const fn speaking_at_most(mut self, version: u16) -> Self {
        self.protocol = version;
        self
    }

    /// Answer [`Request::Presences`] from `relay`. Without it, every such
    /// request is refused with [`NO_BOOK`].
    #[must_use]
    pub fn with_relay(mut self, relay: &'a dyn Relay) -> Self {
        self.relay = Some(relay);
        self
    }

    /// Refuse a chunk larger than what `room` says the disk can still take,
    /// whatever the pledge allows.
    #[must_use]
    pub fn with_disk_room(mut self, room: &'a (dyn Fn() -> Option<u64> + Sync)) -> Self {
        self.disk_room = Some(room);
        self
    }

    /// Bound what this host stores per account by `owners` (§8 1c). Without
    /// it only the host's own pledge applies, as before.
    #[must_use]
    pub fn with_owners(mut self, owners: &'a dyn Owners) -> Self {
        self.owners = Some(owners);
        self
    }

    /// Answer `WantHosted` with the caller's share of the round's placement
    /// in `slot`, whenever the daemon has put one there (`measured`).
    #[must_use]
    pub fn with_placement(mut self, slot: &'a crate::placement::PlacementSlot) -> Self {
        self.placement = Some(slot);
        self
    }

    /// Note that `caller` opened an authenticated session here, once per
    /// session: for a host behind a router this is the only evidence of it
    /// the owner ever gets. Only for a device already known -- a ring per
    /// caller would be a table any stranger grows with fresh keys -- and a
    /// failed write costs a measurement, never the session. `from` is the
    /// socket's address for a direct session, `None` through a relay; its
    /// network is what groups hosts behind one router (§8 7c).
    pub fn note_session(&self, caller: DeviceId, from: Option<std::net::IpAddr>) {
        let now = itsanas_store::now_unix();
        if let Ok(true) = self.store.note_seen_if_known_at(&caller, now)
            && let Some(network) = from.and_then(itsanas_placement::Network::of)
        {
            let _ = self.store.note_network_at(&caller, network, now);
        }
    }

    /// Serve as a node whose vault's disk is away: its own account only.
    #[must_use]
    pub const fn hosting_away(mut self, away: bool) -> Self {
        self.away = away;
        self
    }

    /// Answer a presented claim with `claim`, this host's own, so the
    /// presenting device can bound what it pulls from this host.
    #[must_use]
    pub fn claiming(mut self, claim: Vec<u8>) -> Self {
        self.own_claim = Some(claim);
        self
    }

    /// `Some(refusal)` if `owners` does not let `caller` store `incoming`
    /// more bytes for `owner`. Under the storing lock, beside the pledge.
    fn refused_for_owner(
        &self,
        caller: DeviceId,
        owner: UserId,
        incoming: usize,
    ) -> Option<Response> {
        let owners = self.owners?;
        owners
            .admits(
                caller,
                owner,
                incoming as u64,
                self.store,
                self.vault,
                self.pledge.bytes,
            )
            .err()
            .map(Response::Refused)
    }

    /// This node's device identity.
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        self.store.device_id()
    }

    /// Answer one request from a caller whose identity does not matter here.
    ///
    /// Only for tests that are about the answer rather than about who asked.
    /// Anything that records who holds what must use [`Self::handle`] with the
    /// device TLS actually proved.
    #[cfg(test)]
    fn handle_from_test(&self, request: &Request) -> Result<Response> {
        self.handle(request, DeviceId::from_bytes([0xee; 32]))
    }

    /// Answer one request.
    ///
    /// Never returns `Err` for anything a peer did — a misbehaving peer gets a
    /// [`Response::Refused`], because turning a peer's bad request into a local
    /// error would let a peer decide when this node stops working. `Err` is
    /// reserved for this node's own storage failing.
    /// Whether this node relays for `caller` (§8 0v), and in which pool: a
    /// machine of its own account, or of an account it hosts for and has
    /// confirmed with the coordinator. Anybody else is refused, the same way
    /// whether or not the device asked for is parked here.
    #[must_use]
    pub fn relays_for(&self, caller: DeviceId) -> Option<crate::relay::Class> {
        if self
            .relay
            .is_some_and(|relay| relay.presences_for(caller).is_some())
        {
            return Some(crate::relay::Class::Own);
        }
        self.owners
            .and_then(|owners| owners.confirmed_account(caller))
            .map(crate::relay::Class::Hosted)
    }

    /// The refusal to a malformed request, or to a read for an account
    /// `caller` has not shown it belongs to.
    fn refused_before_reading(&self, request: &Request, caller: DeviceId) -> Option<Response> {
        if !request.is_acceptable() {
            return Some(Response::Refused("malformed request".to_owned()));
        }
        if self.away && hosted_owner(request).is_some_and(|owner| owner != self.store.owner()) {
            return Some(Response::Refused(HOSTING_AWAY.to_owned()));
        }
        // Every request that reads what this host holds for an account is
        // answered only to a device of that account. Before peer protocol 11
        // anybody who knew a user id read its device list, every envelope and
        // the sealed chunks from any host of it.
        let owner = read_owner(request)?;
        let allowed = self
            .owners
            .is_some_and(|owners| owners.may_read(caller, owner));
        (!allowed).then(|| Response::Refused(NOT_YOUR_ACCOUNT.to_owned()))
    }

    /// The account this node's store is for.
    #[must_use]
    pub fn owner(&self) -> UserId {
        self.store.owner()
    }

    pub fn handle(&self, request: &Request, caller: DeviceId) -> Result<Response> {
        if let Some(refusal) = self.refused_before_reading(request, caller) {
            return Ok(refusal);
        }

        match request {
            Request::Hello { protocol, .. } => Ok(Response::Hello {
                protocol: (*protocol).min(self.protocol),
                device: self.device_id(),
            }),

            Request::Heads { owner } => self.heads(*owner),

            Request::Segments {
                owner,
                device,
                after,
                limit,
            } => {
                let limit = usize::from((*limit).min(MAX_SEGMENTS_PER_REQUEST));
                Ok(Response::Segments(
                    self.segments(*owner, *device, *after, limit)?,
                ))
            }

            Request::Chunk { owner, address } => Ok(Response::Chunk(self.chunk(*owner, address)?)),

            Request::HaveChunks { owner, addresses } => {
                // A marked chunk is answered missing: "have" would let the
                // owner re-stamp the copy it released, and count it after the
                // eviction. A re-offer then goes through `put_chunk`, which
                // unmarks (red team, HIGH 1).
                let marked = if *owner == self.store.owner() {
                    std::collections::BTreeSet::new()
                } else {
                    self.vault
                        .marked_among(*owner, addresses, itsanas_store::now_unix())?
                };
                let mut missing = Vec::new();
                for address in addresses {
                    if marked.contains(address) || self.chunk(*owner, address)?.is_none() {
                        missing.push(*address);
                    }
                }
                Ok(Response::Missing(missing))
            }

            Request::StoreChunk {
                owner,
                address,
                sealed,
            } => self.store_chunk(caller, *owner, address, sealed),

            Request::StoreSegment { envelope } => {
                let _storing = self.storing_lock();
                if self.would_exceed_pledge(envelope.sealed_body.len())? {
                    return Ok(Response::Refused(PLEDGE_EXHAUSTED.to_owned()));
                }
                if let Some(refusal) =
                    self.refused_for_owner(caller, envelope.owner, envelope.sealed_body.len())
                {
                    return Ok(refusal);
                }
                // A rejected segment is the peer's problem, not ours: refuse it
                // and say why, rather than failing the connection.
                //
                // `accepted` is what `put_segment` actually did, not `Ok(_)`.
                // Re-offering the tip is normal and stores nothing, and claiming
                // it was stored made every idle round look like a round that had
                // moved something -- so the daemon printed a line every five
                // minutes on a fleet where nothing was happening, which is how an
                // operator learns to stop reading the log.
                match self.vault.put_segment(envelope) {
                    Ok(accepted) => Ok(Response::Stored { accepted }),
                    Err(error) => Ok(Response::Refused(error.to_string())),
                }
            }

            Request::Challenge {
                owner,
                address,
                nonce,
            } => match self.chunk(*owner, address)? {
                Some(sealed) => Ok(Response::ChallengeProof(challenge_proof(nonce, &sealed))),
                None => Ok(Response::Refused("chunk not held".to_owned())),
            },

            Request::WantHosted { limit } => self.want_hosted(*limit, caller),

            Request::ChunkSummary { owner } => {
                // Over the same set this node would answer `HaveChunks` from:
                // its own store when the account is its own, its vault when it
                // is holding for somebody else. Any other set and the summary
                // would disagree with the exchange that follows it, which is
                // the one way this mechanism can be worse than none.
                // `held_summary`, not `chunk_summary`: since §8 4c an agreeing
                // bucket re-stamps holder records without asking, so this node
                // must not say it holds a chunk its own disk is known to have
                // lost. The vault's index is kept honest by its rolling check.
                let digests = if *owner == self.store.owner() {
                    self.store.held_summary()?
                } else {
                    self.vault
                        .chunk_summary_at(*owner, itsanas_store::now_unix())?
                };
                Ok(Response::ChunkSummary(digests))
            }

            Request::Dropped { owner, chunks } => {
                // Only the owner keeps a ledger about its own chunks. A node
                // hosting somebody else's data has nothing to correct, and
                // saying so plainly beats a silent success that looks like the
                // record was withdrawn somewhere.
                if *owner != self.store.owner() {
                    return Ok(Response::Refused(
                        "this node keeps no ledger for that account".to_owned(),
                    ));
                }
                // Scoped to the caller by construction: the device is the one
                // this connection proved, not one it named. A peer can only
                // ever withdraw records about itself.
                self.store.forget_holders(chunks, &caller)?;
                Ok(Response::Stored { accepted: true })
            }

            Request::Leaving => {
                // Scoped to the caller by construction, as for `Dropped`: the
                // device withdrawn is the one this connection proved, never one
                // the request names. Every record about it goes, so
                // `under_replicated` sees the gap now rather than when the
                // records age out, and the next round repairs from whoever is
                // still online.
                //
                // Answered the same whatever this node held: a peer that knew
                // nothing of the device has nothing to withdraw, and telling it
                // so would only teach a stranger which ledgers mention it.
                self.store.forget_device(&caller)?;
                Ok(Response::Stored { accepted: true })
            }

            Request::Presences => Ok(self.presences(caller)),

            Request::Claim { claim } => Ok(self.claim(caller, claim)),

            Request::Draining { seconds } => self.draining(caller, *seconds),

            // The transport takes these first; here, a relayed session asks.
            Request::Park | Request::Splice { .. } => Ok(not_relayed()),

            Request::MayEvict { owner, chunks } => self.may_evict(caller, *owner, chunks),
            Request::Claims { owner } => self.claims(caller, *owner),

            Request::Hosted { chunks } => {
                // A claim, recorded and then checked. The storage challenges
                // this node already runs are what turn it into evidence: a peer
                // that says it stored something and did not fails the next one
                // and the chunk is sent somewhere else. Believing it for now
                // costs one audit round and is what makes the offer worth
                // making at all.
                self.store.record_holders(chunks, &caller)?;
                Ok(Response::Stored { accepted: true })
            }
        }
    }

    /// Stop counting `caller` for `seconds`, and say how much rests on it alone.
    ///
    /// Scoped to the caller, as `Leaving` is: a device can only ever ask to be
    /// uncounted itself, so nobody can make an owner stop counting a third
    /// machine. The store caps the duration; the caller's number is a wish.
    fn draining(&self, caller: DeviceId, seconds: u64) -> Result<Response> {
        let now = itsanas_store::now_unix();
        // Whether a grant was written changes nothing in the answer: the
        // count is the same question either way.
        self.store
            .note_draining(&caller, now.saturating_add(seconds), now)?;
        let chunks = self.store.exposed_by(&caller, now)?;
        Ok(Response::Exposed { chunks })
    }

    /// Store one chunk for `owner`, within the host's pledge and the
    /// account's bound, evicting the account's own marked chunks first when
    /// the bound is what refuses it.
    fn store_chunk(
        &self,
        caller: DeviceId,
        owner: UserId,
        address: &ChunkId,
        sealed: &[u8],
    ) -> Result<Response> {
        let _storing = self.storing_lock();
        if self.would_exceed_pledge(sealed.len())? {
            return Ok(Response::Refused(PLEDGE_EXHAUSTED.to_owned()));
        }
        if let Some(refusal) = self.refused_for_owner(caller, owner, sealed.len()) {
            // The account's share here is spent. If the owner marked
            // copies it has to spare, they make room for this one --
            // the account trades its own spare copy for its own short
            // one -- and the bound is asked once more, so eviction
            // never lets an account store past what it earns. Any
            // other refusal evicts nothing: a full pledge is the
            // host's, not the account's, and a device with no claim
            // has no say over the account's chunks.
            let full = matches!(&refusal, Response::Refused(why) if why.starts_with(ACCOUNT_FULL));
            // The real overage, not only the incoming size: an account
            // already past its share (its allowance shrank) is refused again
            // after freeing one chunk's worth, and those copies were evicted
            // for nothing (red team, LOW).
            let needed = match &refusal {
                Response::Refused(why) => overage(why, sealed.len() as u64),
                _ => sealed.len() as u64,
            };
            if !full
                || self
                    .vault
                    .evict_marked(owner, needed, itsanas_store::now_unix())?
                    == 0
            {
                return Ok(refusal);
            }
            if let Some(refusal) = self.refused_for_owner(caller, owner, sealed.len()) {
                return Ok(refusal);
            }
        }
        self.vault.put_chunk(owner, address, sealed)?;
        Ok(Response::Stored { accepted: true })
    }

    /// Mark `chunks` of `owner` evictable for `owner`, if `caller` speaks for
    /// `owner` and `owner` is another account than this node's.
    ///
    /// Fails closed: a host that bounds nobody has no way to tell who speaks
    /// for an account, and a stranger allowed to mark could have the
    /// account's next store evict copies the account still counts. Its own
    /// account is refused too: this node's own chunks are its store's, not
    /// the vault's, and nothing here may remove them.
    fn may_evict(&self, caller: DeviceId, owner: UserId, chunks: &[ChunkId]) -> Result<Response> {
        let allowed = owner != self.store.owner()
            && self
                .owners
                .is_some_and(|owners| owners.speaks_for(caller, owner));
        if !allowed {
            return Ok(Response::Refused(NOT_YOURS_TO_EVICT.to_owned()));
        }
        // The storing lock, as for a store: a mark lands between a store's
        // check and its write otherwise, and an eviction it triggers then
        // takes a chunk just stored.
        let _storing = self.storing_lock();
        let marked = self
            .vault
            .mark_evictable(owner, chunks, itsanas_store::now_unix())?;
        Ok(Response::MayEvict { marked })
    }

    /// Take the claim `caller` presented, if this host bounds accounts.
    fn claim(&self, caller: DeviceId, claim: &[u8]) -> Response {
        match self.owners {
            // A host that bounds nobody has no use for it, and says so the
            // same way it says it took it: nothing for a peer to act on.
            None => Response::Stored { accepted: false },
            Some(owners) => match owners.present(caller, claim, self.vault) {
                Ok(()) => {
                    // Kept as presented, so the account's other machines can
                    // be handed it (`Request::Claims`). A failed write costs
                    // a sibling one round of waiting, never this session.
                    if let Some(owner) = owners.presented_for(caller) {
                        // Read first: every connection re-signs the claim,
                        // and rewriting one that says nothing new was a
                        // commit per peer per round (the idle bench).
                        let unchanged = self.vault.kept_claim(caller).ok().flatten().is_some_and(
                            |(kept_for, kept)| {
                                kept_for == owner && owners.adds_nothing(&kept, claim)
                            },
                        );
                        if !unchanged {
                            let _ = self.vault.keep_claim(caller, owner, claim);
                        }
                        // A sibling: `present` verified this account's
                        // signature and refused a revocation, which is what
                        // admission to the roster is. A machine that is only
                        // ever pushed to learns its siblings here.
                        if owner == self.store.owner() {
                            let _ = self.store.admit_member(&caller, false);
                        }
                    }
                    self.claim_answer()
                }
                Err(why) => Response::Refused(why),
            },
        }
    }

    /// A claim taken: this host's own in return, so the caller can bound
    /// what it pulls from it, or a plain acknowledgement.
    fn claim_answer(&self) -> Response {
        match &self.own_claim {
            Some(own) => Response::Claim(own.clone()),
            None => Response::Stored { accepted: true },
        }
    }

    /// The claims this host keeps for `owner`, for a caller that presented a
    /// claim for `owner` itself. Anyone else learns nothing, not even that
    /// the host knows the account.
    fn claims(&self, caller: DeviceId, owner: UserId) -> Result<Response> {
        let Some(owners) = self.owners else {
            return Ok(Response::Claims(Vec::new()));
        };
        if owners.presented_for(caller) != Some(owner) {
            return Ok(Response::Claims(Vec::new()));
        }
        Ok(Response::Claims(
            self.vault
                .claims_for(owner)?
                .into_iter()
                .filter(|claim| claim.len() <= crate::protocol::MAX_CLAIM_BYTES)
                .collect(),
        ))
    }

    /// The account's machines, for a caller the book counts as one of them.
    ///
    /// Trimmed to the wire's bounds here as well as by the book, so a book
    /// that grew past them costs the caller rows and never the connection.
    fn presences(&self, caller: DeviceId) -> Response {
        let Some(relay) = self.relay else {
            return Response::Refused(NO_BOOK.to_owned());
        };
        match relay.presences_for(caller) {
            Some(rows) => Response::Presences(
                rows.into_iter()
                    .filter(|row| row.len() <= crate::protocol::MAX_RELAYED_ROW_BYTES)
                    .take(crate::protocol::MAX_RELAYED_ROWS)
                    .collect(),
            ),
            None => Response::Refused(NOT_YOURS.to_owned()),
        }
    }

    /// Chunks this node would like the caller to hold for it.
    ///
    /// The ones with fewest copies first, which is what
    /// [`Store::under_replicated`] already orders by, so the scarcest coverage
    /// is what a round of reciprocal hosting buys.
    ///
    /// Bounded twice: by what the caller asked for, and by a ceiling here. A
    /// peer that asks for everything gets a page, because the alternative is
    /// letting a caller decide how much work this node does in one answer.
    fn want_hosted(&self, limit: u32, caller: DeviceId) -> Result<Response> {
        const CEILING: usize = 64;

        let wanted = (limit as usize).min(CEILING);
        // Cloned out of the lock at once: evaluating holds no lock the
        // daemon's swap would wait on.
        let placement = self
            .placement
            .and_then(|slot| slot.read().ok().and_then(|held| held.clone()))
            // Own devices get everything, as before placement existed.
            .filter(|placement| !placement.is_own(&caller));
        let chunks = match placement {
            // A host of the roster, once per round: each answer evaluates a
            // page of the queue, and a caller allowed to ask on every
            // connection, or a stranger with no share at all, would set this
            // node's CPU to work as often as it liked (red team, MEDIUM 6).
            Some(placement) if placement.take_pull(&caller) => {
                self.share_of(&placement, caller, wanted)?
            }
            Some(_) => Vec::new(),
            None => self
                .store
                .under_replicated(itsanas_store::REPLICATION_TARGET, itsanas_store::now_unix())?
                .into_iter()
                .take(wanted)
                .map(|risk| risk.chunk)
                .collect(),
        };

        Ok(Response::WantHosted {
            owner: self.store.owner(),
            chunks,
        })
    }

    /// `caller`'s share of the short queue under `placement`: what a push to
    /// it would offer, at most `wanted`. One page of the queue, as a push
    /// reads: a host that pulls is not sent everything short, which is what
    /// turned five hosts into five full copies.
    fn share_of(
        &self,
        placement: &crate::placement::Placement,
        caller: DeviceId,
        wanted: usize,
    ) -> Result<Vec<ChunkId>> {
        let mut share = Vec::new();
        for (chunk, since) in crate::placed::short_page(self.store)? {
            if share.len() >= wanted {
                break;
            }
            if !self.store.has_chunk(&chunk) {
                continue; // released here: nothing to hand over
            }
            if placement.offers(&caller, &placement.evaluate(self.store, &chunk)?, since) {
                share.push(chunk);
            }
        }
        Ok(share)
    }

    /// Chain tips, from both this node's own log and its vault.
    ///
    /// Both sources matter. Asking a peer "what do you have for Alice" must
    /// include Alice's own devices if this *is* one of Alice's devices, and the
    /// segments it holds for her as a host if it is not.
    fn heads(&self, owner: UserId) -> Result<Response> {
        let mut heads: Vec<Head> = self
            .vault
            .heads_for(owner)?
            .into_iter()
            .map(|(device, head, length)| Head {
                device,
                head,
                length,
            })
            .collect();

        if owner == self.store.owner()
            && let Some(head) = self.store.head_segment()?
        {
            heads.push(Head {
                device: self.store.device_id(),
                head,
                length: self.store.chain_length()?,
            });
        }

        // Deterministic order, so two identical nodes answer identically and a
        // difference in a test is a real difference.
        heads.sort_by_key(|head| head.device.to_bytes());
        Ok(Response::Heads(heads))
    }

    fn segments(
        &self,
        owner: UserId,
        device: DeviceId,
        after: Option<itsanas_crypto::ObjectId>,
        limit: usize,
    ) -> Result<Vec<itsanas_store::SegmentEnvelope>> {
        // This node's own chain, if that is what was asked for.
        if owner == self.store.owner() && device == self.store.device_id() {
            let mine = self.store.segments()?;
            let start = match after {
                None => 0,
                Some(id) => match mine.iter().position(|s| s.segment_id == id) {
                    Some(index) => index + 1,
                    // A resume point we do not recognise. Returning everything
                    // would re-send history the caller already has.
                    None => return Ok(Vec::new()),
                },
            };
            return Ok(mine.into_iter().skip(start).take(limit).collect());
        }

        Ok(self.vault.segments_for(owner, device, after, limit)?)
    }

    fn chunk(&self, owner: UserId, address: &itsanas_crypto::ChunkId) -> Result<Option<Vec<u8>>> {
        // Our own chunks first: if this is our data, the store is authoritative
        // and the vault would not hold it.
        if owner == self.store.owner()
            && let Some(sealed) = self.store.blobs().get(address)?
        {
            return Ok(Some(sealed));
        }

        Ok(self.vault.get_chunk(owner, address)?)
    }

    /// The storing lock, recovered if a panic poisoned it.
    ///
    /// Poisoning here means one request panicked between a check and a write.
    /// Refusing every later store would turn one bug into a host that never
    /// accepts anything again; the check that follows re-reads the vault, so
    /// carrying on is safe.
    fn storing_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.storing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn would_exceed_pledge(&self, incoming: usize) -> Result<bool> {
        // A running total, not `stats()`: this runs under the storing lock on
        // every offer, and a walk here let a peer sending offers it knew would
        // be refused hold up every honest store behind a pass over the disk.
        if let Some(room) = self.disk_room.and_then(|room| room())
            && incoming as u64 > room
        {
            return Ok(true);
        }
        let held = self.vault.held_bytes()?;
        Ok(held.saturating_add(incoming as u64) > self.pledge.bytes)
    }
}

/// Bytes an account-full refusal says must go before `incoming` fits:
/// `mine + incoming - allowance`, read from "`{why}: {mine} of {allowance}
/// bytes ...`". Falls back to `incoming` for a message of another shape,
/// which is what was asked before and never more than the store needs.
fn overage(why: &str, incoming: u64) -> u64 {
    let figures = why
        .split_once(": ")
        .map(|(_, rest)| rest.split(' ').collect::<Vec<_>>());
    if let Some(words) = figures
        && words.len() >= 3
        && words[1] == "of"
        && let (Ok(mine), Ok(allowance)) = (words[0].parse::<u64>(), words[2].parse::<u64>())
    {
        let over = mine.saturating_add(incoming).saturating_sub(allowance);
        if over > 0 {
            return over;
        }
    }
    incoming
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::{ChunkId, DeviceKeys, MasterSecret, SecretBytes, UserKeys};
    use itsanas_store::ChunkerConfig;

    use crate::protocol::challenge_holds;

    struct Node {
        _dir: tempfile::TempDir,
        store: Store,
        vault: Vault,
    }

    fn node(master: &MasterSecret, device_seed: u8) -> Node {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open_for_testing(
            dir.path().join("store"),
            UserKeys::derive(master),
            DeviceKeys::from_seed(&SecretBytes::new([device_seed; 32])),
            ChunkerConfig::default(),
        )
        .expect("store");
        let vault = Vault::open(dir.path().join("vault")).expect("vault");

        Node {
            _dir: dir,
            store,
            vault,
        }
    }

    fn service(node: &Node) -> PeerService<'_> {
        PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1)).with_owners(&Unscoped)
    }

    fn alice() -> MasterSecret {
        MasterSecret::from_bytes([0xA1; 32])
    }

    #[test]
    fn red_team_only_a_direct_session_from_a_known_device_records_its_network() {
        // §8 7c. The network groups hosts behind one router; a relay's
        // address, a loopback or a stranger must group nobody.
        let host = node(&alice(), 1);
        let service = service(&host);
        let known = DeviceKeys::from_seed(&SecretBytes::new([2; 32])).device_id();
        let stranger = DeviceKeys::from_seed(&SecretBytes::new([3; 32])).device_id();
        let public: std::net::IpAddr = "203.0.113.7".parse().expect("address");
        host.store.note_seen(&known).expect("known");

        service.note_session(stranger, Some(public));
        assert_eq!(host.store.network_since(&stranger, 0).expect("read"), None);

        service.note_session(known, None);
        assert_eq!(
            host.store.network_since(&known, 0).expect("read"),
            None,
            "a relayed session recorded the relay's network as the caller's"
        );
        service.note_session(known, Some("127.0.0.1".parse().expect("address")));
        assert_eq!(host.store.network_since(&known, 0).expect("read"), None);

        service.note_session(known, Some(public));
        assert_eq!(
            host.store.network_since(&known, 0).expect("read"),
            itsanas_placement::Network::of(public)
        );
    }

    #[test]
    fn red_team_an_away_node_serves_its_own_account_and_refuses_hosting_as_away() {
        // A USB vault unplugged: the node's own files keep syncing, and
        // every request about another account's data gets `HOSTING_AWAY`,
        // which an owner reads as "not here" -- never an audit "not held",
        // which would pause a host that only had its disk out.
        let node = node(&alice(), 10);
        let away = service(&node).hosting_away(true);
        let stranger = UserKeys::derive(&MasterSecret::from_bytes([0xB0; 32])).user_id();
        let address = ChunkId::from_bytes([3; 32]);
        let refused = Response::Refused(HOSTING_AWAY.to_owned());
        for request in [
            Request::Challenge {
                owner: stranger,
                address,
                nonce: [1; 32],
            },
            Request::Chunk {
                owner: stranger,
                address,
            },
            Request::HaveChunks {
                owner: stranger,
                addresses: vec![address],
            },
            Request::StoreChunk {
                owner: stranger,
                address,
                sealed: vec![0; 8],
            },
        ] {
            assert_eq!(
                away.handle_from_test(&request).unwrap(),
                refused,
                "{request:?}"
            );
        }
        assert_ne!(
            away.handle_from_test(&Request::Heads {
                owner: node.store.owner()
            })
            .unwrap(),
            refused,
            "an away node stopped serving its own account"
        );
        assert_ne!(
            service(&node)
                .handle_from_test(&Request::Chunk {
                    owner: stranger,
                    address
                })
                .unwrap(),
            refused,
            "a node with its disk in said it was away"
        );
    }

    #[test]
    fn red_team_a_host_that_bounds_nobody_serves_no_read() {
        // The default is the safe side: a service built without an `Owners`
        // cannot tell an account's device from a stranger, so it reads to
        // nobody. Every request `read_owner` names is checked here.
        let node = node(&alice(), 10);
        let bare = PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1));
        let owner = node.store.owner();
        let address = ChunkId::from_bytes([3; 32]);
        for request in [
            Request::Heads { owner },
            Request::Segments {
                owner,
                device: node.store.device_id(),
                after: None,
                limit: 1,
            },
            Request::Chunk { owner, address },
            Request::HaveChunks {
                owner,
                addresses: vec![address],
            },
            Request::ChunkSummary { owner },
            Request::Challenge {
                owner,
                address,
                nonce: [1; 32],
            },
            Request::Claims { owner },
        ] {
            assert_eq!(
                read_owner(&request),
                Some(owner),
                "{request:?} is not classed as a read"
            );
            assert_eq!(
                bare.handle_from_test(&request).unwrap(),
                Response::Refused(NOT_YOUR_ACCOUNT.to_owned()),
                "{request:?} was served by a host that cannot tell who asks"
            );
        }
    }

    #[test]
    fn red_team_no_node_serves_with_the_unscoped_owners() {
        // `Unscoped` exists for tests. A node built with it would serve every
        // account to anyone, so the code that runs one must never name it.
        // Each file is cut off at its `#[cfg(test)] mod tests`.
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let mut checked = 0;
        for krate in ["itsanas-cli", "itsanas-node", "itsanas-android"] {
            let mut stack = vec![crates.join(krate).join("src")];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().is_some_and(|ext| ext == "rs")
                        && !path.ends_with("tests.rs")
                    {
                        let text = std::fs::read_to_string(&path).unwrap();
                        // Only the test module goes: a `#[cfg(test)]` helper
                        // higher up must not hide the code after it.
                        let code = text
                            .split("#[cfg(test)]\nmod tests")
                            .next()
                            .unwrap_or_default();
                        assert!(
                            !code.contains("Unscoped"),
                            "{} names `Unscoped` outside its tests: that node serves every \
                             account to anyone",
                            path.display()
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(
            checked > 20,
            "the scan found {checked} files: it would pass blind"
        );
    }

    #[test]
    fn hello_reports_this_nodes_device_and_agrees_on_a_version() {
        let node = node(&alice(), 1);
        let service = service(&node);

        let response = service
            .handle_from_test(&Request::Hello {
                protocol: PROTOCOL_VERSION,
                device: DeviceId::from_bytes([9; 32]),
                owner: node.store.owner(),
            })
            .unwrap();

        match response {
            Response::Hello { protocol, device } => {
                assert_eq!(protocol, PROTOCOL_VERSION);
                assert_eq!(device, node.store.device_id());
            }
            other => panic!("expected a hello, got {other:?}"),
        }
    }

    #[test]
    fn a_hello_from_a_newer_peer_is_answered_with_the_version_both_sides_know() {
        // The property the version window rests on. Refusing anything but an
        // exact match -- which is what this did -- meant no node could speak to
        // a node one commit ahead of it, so every addition to the protocol
        // partitioned the network until every machine was upgraded at the same
        // instant. Fine for three machines in one house; impossible for people
        // who join and leave.
        let node = node(&alice(), 2);
        let response = service(&node)
            .handle_from_test(&Request::Hello {
                protocol: PROTOCOL_VERSION + 5,
                device: DeviceId::from_bytes([9; 32]),
                owner: node.store.owner(),
            })
            .unwrap();

        match response {
            Response::Hello { protocol, .. } => assert_eq!(
                protocol, PROTOCOL_VERSION,
                "this node claimed to speak a version it has never heard of"
            ),
            other => panic!("a newer peer was turned away: {other:?}"),
        }
    }

    #[test]
    fn a_hello_from_below_the_floor_is_refused_rather_than_guessed_at() {
        // The other half: a window has a bottom. Below it there is no shared
        // vocabulary to fall back to, and pretending otherwise would fail on
        // some later message instead of this one.
        let node = node(&alice(), 22);
        let response = service(&node)
            .handle_from_test(&Request::Hello {
                protocol: crate::protocol::MIN_PROTOCOL_VERSION - 1,
                device: DeviceId::from_bytes([9; 32]),
                owner: node.store.owner(),
            })
            .unwrap();

        assert!(matches!(response, Response::Refused(_)));
    }

    #[test]
    fn a_peer_can_fetch_this_nodes_own_segments_and_chunks() {
        let node = node(&alice(), 3);
        node.store.write_file("notes.txt", b"content").unwrap();
        let entry = node.store.stat("notes.txt").unwrap().unwrap();
        node.store.flush_segment().unwrap();

        let service = service(&node);

        let heads = match service
            .handle_from_test(&Request::Heads {
                owner: node.store.owner(),
            })
            .unwrap()
        {
            Response::Heads(heads) => heads,
            other => panic!("expected heads, got {other:?}"),
        };
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].device, node.store.device_id());
        assert_eq!(heads[0].length, 1);

        let segments = match service
            .handle_from_test(&Request::Segments {
                owner: node.store.owner(),
                device: node.store.device_id(),
                after: None,
                limit: 10,
            })
            .unwrap()
        {
            Response::Segments(segments) => segments,
            other => panic!("expected segments, got {other:?}"),
        };
        assert_eq!(segments.len(), 1);
        segments[0].verify_signature().unwrap();

        let chunk = match service
            .handle_from_test(&Request::Chunk {
                owner: node.store.owner(),
                address: entry.chunks[0],
            })
            .unwrap()
        {
            Response::Chunk(chunk) => chunk,
            other => panic!("expected a chunk, got {other:?}"),
        };
        assert!(chunk.is_some());
    }

    #[test]
    fn what_a_peer_fetches_is_useless_without_the_key() {
        // The reason there is no access-control list: serving this to the wrong
        // person reveals nothing.
        let node = node(&alice(), 4);
        node.store
            .write_file("secret.txt", b"ITSANAS-SERVICE-CANARY-8f2a")
            .unwrap();
        let entry = node.store.stat("secret.txt").unwrap().unwrap();

        let chunk = match service(&node)
            .handle_from_test(&Request::Chunk {
                owner: node.store.owner(),
                address: entry.chunks[0],
            })
            .unwrap()
        {
            Response::Chunk(Some(bytes)) => bytes,
            other => panic!("expected a chunk, got {other:?}"),
        };

        let needle = b"ITSANAS-SERVICE-CANARY-8f2a";
        assert!(
            !chunk.windows(needle.len()).any(|w| w == needle),
            "the bytes served to a peer contain the plaintext"
        );

        // And a different user genuinely cannot open them.
        let stranger = UserKeys::derive(&MasterSecret::from_bytes([0xBB; 32]));
        assert!(
            stranger.open_chunk(&entry.chunks[0], &chunk).is_err(),
            "a stranger opened a chunk served to them"
        );
    }

    #[test]
    fn an_unknown_chunk_is_none_rather_than_an_error() {
        // "I do not have it" is ordinary. Turning it into an error would let a
        // peer's request decide when this node reports a fault.
        let node = node(&alice(), 5);
        let response = service(&node)
            .handle_from_test(&Request::Chunk {
                owner: node.store.owner(),
                address: ChunkId::from_bytes([0xEE; 32]),
            })
            .unwrap();

        assert_eq!(response, Response::Chunk(None));
    }

    #[test]
    fn a_node_stores_and_serves_a_strangers_chunk_without_reading_it() {
        // The whole mutual-storage bargain, in one test.
        let host = node(&alice(), 6);
        let guest_keys = UserKeys::derive(&MasterSecret::from_bytes([0xC0; 32]));
        let (address, sealed) = guest_keys.seal_chunk(b"the guest's private data").unwrap();

        let service = service(&host);

        assert_eq!(
            service
                .handle_from_test(&Request::StoreChunk {
                    owner: guest_keys.user_id(),
                    address,
                    sealed: sealed.clone(),
                })
                .unwrap(),
            Response::Stored { accepted: true }
        );

        let served = match service
            .handle_from_test(&Request::Chunk {
                owner: guest_keys.user_id(),
                address,
            })
            .unwrap()
        {
            Response::Chunk(Some(bytes)) => bytes,
            other => panic!("the host did not serve back what it stored: {other:?}"),
        };
        assert_eq!(served, sealed);

        // The host's own keys are useless against it.
        assert!(
            UserKeys::derive(&alice())
                .open_chunk(&address, &served)
                .is_err(),
            "the host could read the data it is storing for someone else"
        );
        // The guest's keys work.
        assert_eq!(
            guest_keys.open_chunk(&address, &served).unwrap(),
            b"the guest's private data"
        );
    }

    #[test]
    fn a_storage_challenge_passes_when_held_and_fails_when_not() {
        let host = node(&alice(), 7);
        let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC1; 32]));
        let (address, sealed) = guest.seal_chunk(b"prove you have this").unwrap();

        let service = service(&host);
        service
            .handle_from_test(&Request::StoreChunk {
                owner: guest.user_id(),
                address,
                sealed: sealed.clone(),
            })
            .unwrap();

        let nonce = [0x5A; 32];
        let proof = match service
            .handle_from_test(&Request::Challenge {
                owner: guest.user_id(),
                address,
                nonce,
            })
            .unwrap()
        {
            Response::ChallengeProof(proof) => proof,
            other => panic!("expected a proof, got {other:?}"),
        };

        assert!(
            challenge_holds(&nonce, &sealed, &proof),
            "a host holding the chunk failed its own challenge"
        );

        // A chunk it never had.
        assert!(matches!(
            service
                .handle_from_test(&Request::Challenge {
                    owner: guest.user_id(),
                    address: ChunkId::from_bytes([0xDD; 32]),
                    nonce,
                })
                .unwrap(),
            Response::Refused(_)
        ));
    }

    #[test]
    fn a_host_that_discarded_a_chunk_cannot_fake_the_proof() {
        let host = node(&alice(), 8);
        let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC2; 32]));
        let (address, sealed) = guest.seal_chunk(b"data the host will drop").unwrap();

        let service = service(&host);
        service
            .handle_from_test(&Request::StoreChunk {
                owner: guest.user_id(),
                address,
                sealed: sealed.clone(),
            })
            .unwrap();

        // The host quietly deletes it to save space.
        host.vault.remove_chunk(guest.user_id(), &address).unwrap();

        assert!(
            matches!(
                service
                    .handle_from_test(&Request::Challenge {
                        owner: guest.user_id(),
                        address,
                        nonce: [1; 32],
                    })
                    .unwrap(),
                Response::Refused(_)
            ),
            "a host that discarded a chunk still passed its challenge"
        );
    }

    #[test]
    fn storing_beyond_the_pledge_is_refused() {
        // A node must be able to bound what it takes on, or "pledge 10 GB" is
        // meaningless and the disk fills.
        let host = node(&alice(), 9);
        let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC3; 32]));
        let service = PeerService::new(&host.store, &host.vault, Pledge { bytes: 512 });

        let (first, first_sealed) = guest.seal_chunk(&vec![1u8; 400]).unwrap();
        assert_eq!(
            service
                .handle_from_test(&Request::StoreChunk {
                    owner: guest.user_id(),
                    address: first,
                    sealed: first_sealed,
                })
                .unwrap(),
            Response::Stored { accepted: true }
        );

        let (second, second_sealed) = guest.seal_chunk(&vec![2u8; 400]).unwrap();
        assert!(
            matches!(
                service
                    .handle_from_test(&Request::StoreChunk {
                        owner: guest.user_id(),
                        address: second,
                        sealed: second_sealed,
                    })
                    .unwrap(),
                Response::Refused(_)
            ),
            "the node accepted more than it pledged and will fill its disk"
        );
    }

    /// THE ATTACK: many connections offer a chunk at the same instant. The
    /// listener serves connections concurrently, and "is there room" is a read
    /// that happens before "store it". Every connection sees the same free
    /// space, every one is accepted, and the host has taken on its pledge many
    /// times over -- its owner's disk fills with a stranger's data.
    #[test]
    fn red_team_concurrent_stores_cannot_take_a_host_past_its_pledge() {
        const CALLERS: usize = 16;
        let host = node(&alice(), 11);
        let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC4; 32]));
        let offers: Vec<_> = (0..CALLERS)
            .map(|n| guest.seal_chunk(&n.to_le_bytes().repeat(50)).unwrap())
            .collect();
        let room = offers[0].1.len() as u64 * 4;
        let service = PeerService::new(&host.store, &host.vault, Pledge::bytes(room));
        let start = std::sync::Barrier::new(CALLERS);
        let owner = guest.user_id();

        std::thread::scope(|scope| {
            for (address, sealed) in &offers {
                let (service, start) = (&service, &start);
                scope.spawn(move || {
                    start.wait();
                    service
                        .handle_from_test(&Request::StoreChunk {
                            owner,
                            address: *address,
                            sealed: sealed.clone(),
                        })
                        .unwrap();
                });
            }
        });

        let held = host.vault.stats().unwrap().bytes;
        assert!(
            held <= room,
            "{CALLERS} simultaneous offers left the host holding {held} bytes \
             against a pledge of {room}; concurrent connections can fill a \
             disk its owner never offered"
        );
    }

    #[test]
    fn a_node_that_pledged_nothing_still_serves_its_own_data() {
        // Hosting nothing must not break syncing your own devices.
        let node = node(&alice(), 10);
        node.store.write_file("mine.txt", b"my own file").unwrap();
        node.store.flush_segment().unwrap();

        let service =
            PeerService::new(&node.store, &node.vault, Pledge::NONE).with_owners(&Unscoped);

        match service
            .handle_from_test(&Request::Heads {
                owner: node.store.owner(),
            })
            .unwrap()
        {
            Response::Heads(heads) => assert_eq!(heads.len(), 1),
            other => panic!("expected heads, got {other:?}"),
        }
    }

    #[test]
    fn a_forged_segment_is_refused_rather_than_stored() {
        let host = node(&alice(), 11);
        let guest_master = MasterSecret::from_bytes([0xC4; 32]);
        let guest = node(&guest_master, 12);

        guest.store.write_file("theirs.txt", b"content").unwrap();
        let mut envelope = guest.store.flush_segment().unwrap().unwrap();
        envelope.sealed_body[0] ^= 0xFF;

        let response = service(&host)
            .handle_from_test(&Request::StoreSegment {
                envelope: Box::new(envelope),
            })
            .unwrap();

        assert!(
            matches!(response, Response::Refused(_)),
            "a host accepted a segment with a broken signature"
        );
    }

    #[test]
    fn red_team_a_peer_can_only_withdraw_records_about_itself() {
        // `Dropped` lets a device correct a ledger, which is exactly the shape
        // of request that becomes an attack if the subject is taken from the
        // message. A host that could say "device B no longer holds these" would
        // be able to make an owner believe its data is unreplicated and trigger
        // an endless repair, or -- worse in the other direction -- be used to
        // erase the record of the only holder that still has it.
        //
        // The subject is the connection's proven device and cannot be named in
        // the request at all, so this test is about keeping it that way.
        let node = node(&alice(), 23);
        node.store.write_file("notes.txt", b"content").unwrap();
        let entry = node.store.stat("notes.txt").unwrap().unwrap();
        let chunk = entry.chunks[0];

        let honest = DeviceId::from_bytes([0xA1; 32]);
        let liar = DeviceId::from_bytes([0xB2; 32]);
        node.store.record_holders(&[chunk], &honest).unwrap();
        node.store.record_holders(&[chunk], &liar).unwrap();

        let service = service(&node);
        let response = service
            .handle(
                &Request::Dropped {
                    owner: node.store.owner(),
                    chunks: vec![chunk],
                },
                liar,
            )
            .unwrap();
        assert!(matches!(response, Response::Stored { accepted: true }));

        let left: Vec<_> = node
            .store
            .remote_holders(&chunk)
            .unwrap()
            .into_iter()
            .map(|holder| holder.device)
            .collect();
        assert_eq!(
            left,
            vec![honest],
            "a peer's notice removed somebody else's record"
        );
    }

    #[test]
    fn red_team_a_leaving_notice_withdraws_only_the_caller() {
        // `Leaving` withdraws every record about a device at once, so it is the
        // most destructive notice a peer can send: pointed at the wrong device
        // it would make an owner forget the only holder that still has its
        // data. It carries no device for that reason, and this is the test
        // that keeps it so. After it, the chunk is under-replicated, which is
        // what makes the next round repair it rather than wait a week.
        let node = node(&alice(), 26);
        node.store.write_file("notes.txt", b"content").unwrap();
        let chunk = node.store.stat("notes.txt").unwrap().unwrap().chunks[0];

        let stays = DeviceId::from_bytes([0xA3; 32]);
        let goes = DeviceId::from_bytes([0xB4; 32]);
        node.store.record_holders(&[chunk], &stays).unwrap();
        node.store.record_holders(&[chunk], &goes).unwrap();

        let response = service(&node).handle(&Request::Leaving, goes).unwrap();
        assert!(matches!(response, Response::Stored { accepted: true }));

        let left: Vec<_> = node
            .store
            .remote_holders(&chunk)
            .unwrap()
            .into_iter()
            .map(|holder| holder.device)
            .collect();
        assert_eq!(left, vec![stays], "leaving withdrew the wrong device");

        let at_risk = node
            .store
            .under_replicated(itsanas_store::REPLICATION_TARGET, itsanas_store::now_unix())
            .unwrap();
        assert!(
            at_risk.iter().any(|risk| risk.chunk == chunk),
            "a departed holder is still counted, so nothing repairs"
        );
    }

    #[test]
    fn red_team_a_draining_notice_uncounts_only_the_caller() {
        // `Draining` makes an owner stop counting a holder. Pointed at somebody
        // else it would make the owner ignore -- and replace -- a machine that
        // is staying. It carries no device; this keeps it so, and checks the
        // answer counts what rests on the caller alone.
        let node = node(&alice(), 27);
        node.store.write_file("notes.txt", b"content").unwrap();
        let chunk = node.store.stat("notes.txt").unwrap().unwrap().chunks[0];

        let stays = DeviceId::from_bytes([0xA5; 32]);
        let goes = DeviceId::from_bytes([0xB6; 32]);
        node.store.record_holders(&[chunk], &goes).unwrap();

        let response = service(&node)
            .handle(&Request::Draining { seconds: 600 }, goes)
            .unwrap();
        assert_eq!(
            response,
            Response::Exposed { chunks: 1 },
            "the only holder drained and was told nothing rests on it"
        );
        let now = itsanas_store::now_unix();
        assert!(node.store.is_draining(&goes, now).unwrap());
        assert!(
            !node.store.is_draining(&stays, now).unwrap(),
            "a notice uncounted a device other than its sender"
        );

        node.store.record_holders(&[chunk], &stays).unwrap();
        let response = service(&node)
            .handle(&Request::Draining { seconds: 600 }, goes)
            .unwrap();
        assert_eq!(
            response,
            Response::Exposed { chunks: 0 },
            "a second holder appeared and the drain was still told to wait"
        );
    }

    #[test]
    fn a_drop_notice_for_another_account_is_refused() {
        // A node hosting somebody else's sealed data keeps no ledger about it:
        // the owner does. Accepting the notice silently would look like the
        // record had been withdrawn somewhere, which is the kind of quiet
        // success that hides a hole for months.
        let node = node(&alice(), 24);
        let response = service(&node)
            .handle(
                &Request::Dropped {
                    owner: itsanas_crypto::UserId::from_bytes([0xCC; 32]),
                    chunks: vec![ChunkId::from_bytes([1; 32])],
                },
                DeviceId::from_bytes([2; 32]),
            )
            .unwrap();

        assert!(matches!(response, Response::Refused(_)));
    }

    #[test]
    fn a_segment_already_held_is_answered_not_stored() {
        // `accepted` has to be what happened, not what the call returned. The
        // handler mapped `Ok(_)` to `accepted: true`, so re-offering the tip --
        // which stores nothing, and which every push used to do on every round
        // -- was reported as a segment moved. Downstream, `changed_anything()`
        // was true on every idle round for ever, and the daemon printed a line
        // every five minutes on a fleet where nothing was happening.
        //
        // Found by reading three machines' logs, not by a test. The push side
        // no longer re-offers, so this is the only place that keeps the answer
        // honest for any other caller.
        let node = node(&alice(), 25);
        node.store.write_file("notes.txt", b"content").unwrap();
        let envelope = node
            .store
            .flush_segment()
            .unwrap()
            .expect("a written file produces a segment");

        let service = service(&node);
        let request = Request::StoreSegment {
            envelope: Box::new(envelope),
        };

        match service.handle_from_test(&request).unwrap() {
            Response::Stored { accepted } => assert!(accepted, "the first offer was not stored"),
            other => panic!("expected stored, got {other:?}"),
        }

        match service.handle_from_test(&request).unwrap() {
            Response::Stored { accepted } => assert!(
                !accepted,
                "a segment the vault already held was reported as stored"
            ),
            other => panic!("expected stored, got {other:?}"),
        }
    }

    #[test]
    fn a_bad_request_never_becomes_a_local_error() {
        // A peer must not be able to decide when this node reports a fault.
        let node = node(&alice(), 13);
        let service = service(&node);

        for request in [
            Request::Segments {
                owner: node.store.owner(),
                device: node.store.device_id(),
                after: None,
                limit: 0,
            },
            Request::Hello {
                protocol: 0,
                device: DeviceId::from_bytes([1; 32]),
                owner: node.store.owner(),
            },
        ] {
            let response = service
                .handle_from_test(&request)
                .expect("a peer's bad request must not be a local error");
            assert!(matches!(response, Response::Refused(_)));
        }
    }

    #[test]
    fn heads_for_an_unknown_owner_are_empty_rather_than_an_error() {
        let node = node(&alice(), 14);
        let response = service(&node)
            .handle_from_test(&Request::Heads {
                owner: UserKeys::derive(&MasterSecret::from_bytes([0xEE; 32])).user_id(),
            })
            .unwrap();

        assert_eq!(response, Response::Heads(Vec::new()));
    }

    #[test]
    fn the_segment_limit_is_clamped_to_the_protocol_maximum() {
        let node = node(&alice(), 15);
        for round in 0..5 {
            node.store
                .write_file(&format!("f{round}.txt"), b"x")
                .unwrap();
            node.store.flush_segment().unwrap();
        }

        match service(&node)
            .handle_from_test(&Request::Segments {
                owner: node.store.owner(),
                device: node.store.device_id(),
                after: None,
                limit: 2,
            })
            .unwrap()
        {
            Response::Segments(segments) => assert_eq!(segments.len(), 2),
            other => panic!("expected segments, got {other:?}"),
        }
    }

    /// A book that knows one member and hands it the rows it was given.
    struct Book {
        member: DeviceId,
        rows: Vec<Vec<u8>>,
    }

    impl Relay for Book {
        fn presences_for(&self, caller: DeviceId) -> Option<Vec<Vec<u8>>> {
            (caller == self.member).then(|| self.rows.clone())
        }
    }

    #[test]
    fn a_node_that_keeps_no_book_says_so() {
        let node = node(&alice(), 1);
        let response = service(&node)
            .handle_from_test(&Request::Presences)
            .unwrap();
        assert_eq!(response, Response::Refused(NO_BOOK.to_owned()));
    }

    #[test]
    fn red_team_presences_are_answered_to_the_device_tls_proved_and_no_other() {
        // The request names nobody on purpose: who may ask is the device the
        // connection proved. Sabotage: pass anything but `caller` to the book,
        // and a stranger reads the account's addresses.
        let node = node(&alice(), 1);
        let member = DeviceId::from_bytes([0x11; 32]);
        let book = Book {
            member,
            rows: vec![vec![1, 2, 3]],
        };
        let service = service(&node).with_relay(&book);

        assert_eq!(
            service.handle(&Request::Presences, member).unwrap(),
            Response::Presences(vec![vec![1, 2, 3]])
        );
        assert_eq!(
            service
                .handle(&Request::Presences, DeviceId::from_bytes([0x22; 32]))
                .unwrap(),
            Response::Refused(NOT_YOURS.to_owned()),
            "a stranger was handed the account's machines"
        );
    }

    #[test]
    fn an_answer_never_exceeds_what_the_receiver_accepts() {
        // The receiver refuses a padded answer whole (`PeerClient::presences`),
        // so a book past the bounds must cost rows here, not the answer.
        let node = node(&alice(), 1);
        let member = DeviceId::from_bytes([0x11; 32]);
        let mut rows = vec![vec![0; crate::protocol::MAX_RELAYED_ROW_BYTES + 1]];
        rows.extend(std::iter::repeat_n(
            vec![7],
            crate::protocol::MAX_RELAYED_ROWS + 5,
        ));
        let book = Book { member, rows };
        match service(&node)
            .with_relay(&book)
            .handle(&Request::Presences, member)
            .unwrap()
        {
            Response::Presences(rows) => {
                assert_eq!(rows.len(), crate::protocol::MAX_RELAYED_ROWS);
                assert!(rows.iter().all(|row| row == &vec![7]));
            }
            other => panic!("expected presences, got {other:?}"),
        }
    }

    /// Lets one device store, after it presented the claim `b"ok"`.
    struct Gate {
        allowed: std::sync::Mutex<Option<DeviceId>>,
    }

    impl Owners for Gate {
        fn may_read(&self, _: DeviceId, _: UserId) -> bool {
            true
        }

        fn present(
            &self,
            caller: DeviceId,
            claim: &[u8],
            _vault: &Vault,
        ) -> std::result::Result<(), String> {
            if claim != b"ok" {
                return Err("bad claim".to_owned());
            }
            *self.allowed.lock().unwrap() = Some(caller);
            Ok(())
        }
        fn admits(
            &self,
            caller: DeviceId,
            _owner: UserId,
            _incoming: u64,
            _store: &Store,
            _vault: &Vault,
            _host_pledge: u64,
        ) -> std::result::Result<(), String> {
            match *self.allowed.lock().unwrap() {
                Some(allowed) if allowed == caller => Ok(()),
                _ => Err(UPDATE_REQUIRED.to_owned()),
            }
        }
    }

    #[test]
    fn red_team_a_host_that_bounds_accounts_asks_before_every_store() {
        // §8 1c. The bound is worth nothing if one of the two store verbs skips
        // it, or if the claim is credited to a device other than the one TLS
        // proved. Sabotages: skip the gate in `StoreChunk`; pass a fixed device
        // to `present`.
        let node = node(&alice(), 1);
        let gate = Gate {
            allowed: std::sync::Mutex::new(None),
        };
        let service = service(&node).with_owners(&gate);
        let (them, other) = (
            DeviceId::from_bytes([0x31; 32]),
            DeviceId::from_bytes([0x32; 32]),
        );
        let store = Request::StoreChunk {
            owner: node.store.owner(),
            address: itsanas_crypto::ChunkId::from_bytes([9; 32]),
            sealed: vec![1; 10],
        };

        assert_eq!(
            service.handle(&store, them).unwrap(),
            Response::Refused(UPDATE_REQUIRED.to_owned()),
            "a chunk was stored by a device that presented no claim"
        );
        assert_eq!(
            service
                .handle(
                    &Request::Claim {
                        claim: b"ok".to_vec()
                    },
                    them
                )
                .unwrap(),
            Response::Stored { accepted: true }
        );
        assert_eq!(
            service.handle(&store, them).unwrap(),
            Response::Stored { accepted: true }
        );
        assert_eq!(
            service.handle(&store, other).unwrap(),
            Response::Refused(UPDATE_REQUIRED.to_owned()),
            "a claim presented by one device let another store"
        );

        node.store.write_file("a.txt", b"segment").unwrap();
        let envelope = node.store.flush_segment().unwrap().expect("a segment");
        assert_eq!(
            service
                .handle(
                    &Request::StoreSegment {
                        envelope: Box::new(envelope),
                    },
                    other
                )
                .unwrap(),
            Response::Refused(UPDATE_REQUIRED.to_owned()),
            "a segment was stored by a device that presented no claim"
        );

        // Two-way: a host that knows its own claim answers with it, so a
        // dialling machine behind a router can hold this host to the same
        // rule when it pulls this host's chunks.
        let answering = super::PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1))
            .with_owners(&gate)
            .claiming(b"mine".to_vec());
        assert_eq!(
            answering
                .handle(
                    &Request::Claim {
                        claim: b"ok".to_vec()
                    },
                    them
                )
                .unwrap(),
            Response::Claim(b"mine".to_vec()),
            "a host answered a claim without its own, so a machine that is never \
             dialled can never learn whose the hosts it pulls for are"
        );
    }

    /// Bob's device speaks for Bob, and Bob may hold `room` bytes here.
    struct Bound {
        speaks: std::collections::BTreeMap<DeviceId, UserId>,
        room: u64,
    }

    impl Owners for Bound {
        fn may_read(&self, _: DeviceId, _: UserId) -> bool {
            true
        }

        fn present(&self, _: DeviceId, _: &[u8], _: &Vault) -> std::result::Result<(), String> {
            Ok(())
        }
        fn admits(
            &self,
            caller: DeviceId,
            owner: UserId,
            incoming: u64,
            _store: &Store,
            vault: &Vault,
            _host_pledge: u64,
        ) -> std::result::Result<(), String> {
            if self.speaks.get(&caller) != Some(&owner) {
                return Err(UPDATE_REQUIRED.to_owned());
            }
            let mine = vault.held_bytes_for(owner).map_err(|e| e.to_string())?;
            if mine.saturating_add(incoming) > self.room {
                return Err(format!("{ACCOUNT_FULL}: {mine} of {} bytes", self.room));
            }
            Ok(())
        }
        fn speaks_for(&self, caller: DeviceId, owner: UserId) -> bool {
            self.speaks.get(&caller) == Some(&owner)
        }
    }

    fn bob() -> UserId {
        UserKeys::derive(&MasterSecret::from_bytes([0xB0; 32])).user_id()
    }

    fn carol() -> UserId {
        UserKeys::derive(&MasterSecret::from_bytes([0xC0; 32])).user_id()
    }

    const BOBS: DeviceId = DeviceId::from_bytes([0x41; 32]);
    const CAROLS: DeviceId = DeviceId::from_bytes([0x42; 32]);

    fn bound(room: u64) -> Bound {
        Bound {
            speaks: [(BOBS, bob()), (CAROLS, carol())].into_iter().collect(),
            room,
        }
    }

    fn store_for(owner: UserId, byte: u8) -> Request {
        Request::StoreChunk {
            owner,
            address: ChunkId::from_bytes([byte; 32]),
            sealed: vec![byte; 10],
        }
    }

    fn may_evict(owner: UserId, bytes: &[u8]) -> Request {
        Request::MayEvict {
            owner,
            chunks: bytes
                .iter()
                .map(|b| ChunkId::from_bytes([*b; 32]))
                .collect(),
        }
    }

    #[test]
    fn red_team_permission_to_evict_is_refused_from_another_account_a_stranger_and_with_no_bound() {
        // A mark lets the account's next store remove a copy the account
        // still counts. Anyone but a device that speaks for the account --
        // Carol's device naming Bob, a device in no book, any device on a
        // host that bounds nobody, or a device naming this host's own
        // account -- must mark nothing. Sabotages: skip `speaks_for`; treat a
        // host without `Owners` as open; drop the own-account check.
        let node = node(&alice(), 3);
        let gate = bound(1_000);
        let bounded = service(&node).with_owners(&gate);
        let open = service(&node);
        bounded.handle(&store_for(bob(), 1), BOBS).unwrap();
        let refused = Response::Refused(NOT_YOURS_TO_EVICT.to_owned());

        let stranger = DeviceId::from_bytes([0x43; 32]);
        assert_eq!(
            bounded.handle(&may_evict(bob(), &[1]), CAROLS).unwrap(),
            refused
        );
        assert_eq!(
            bounded.handle(&may_evict(bob(), &[1]), stranger).unwrap(),
            refused
        );
        assert_eq!(open.handle(&may_evict(bob(), &[1]), BOBS).unwrap(), refused);
        let mine = Bound {
            speaks: [(BOBS, node.store.owner())].into_iter().collect(),
            room: 1_000,
        };
        assert_eq!(
            service(&node)
                .with_owners(&mine)
                .handle(&may_evict(node.store.owner(), &[1]), BOBS)
                .unwrap(),
            refused,
            "a device marked this host's own account's chunks"
        );
        assert!(
            !node
                .vault
                .is_evictable(bob(), &ChunkId::from_bytes([1; 32]))
                .unwrap()
        );

        assert_eq!(
            bounded.handle(&may_evict(bob(), &[1, 2]), BOBS).unwrap(),
            Response::MayEvict { marked: 1 },
            "the account's own device was not let mark the one chunk held"
        );
    }

    #[test]
    fn red_team_a_marked_chunk_is_answered_missing_and_left_out_of_the_summary() {
        // A host that still says "have" for a chunk it is about to evict has
        // the owner re-stamp the copy it just released, and count it after
        // the eviction. Answered missing, and summarised as absent, the owner
        // re-offers it, and that store unmarks it. Sabotages: answer
        // `HaveChunks` from the blobs alone; summarise every row.
        let node = node(&alice(), 5);
        let gate = bound(1_000);
        let service = service(&node).with_owners(&gate);
        for byte in [1, 2] {
            service.handle(&store_for(bob(), byte), BOBS).unwrap();
        }
        let summary = |service: &super::PeerService<'_>| {
            service
                .handle(&Request::ChunkSummary { owner: bob() }, BOBS)
                .unwrap()
        };
        let before = summary(&service);
        service.handle(&may_evict(bob(), &[1]), BOBS).unwrap();

        let have = Request::HaveChunks {
            owner: bob(),
            addresses: vec![ChunkId::from_bytes([1; 32]), ChunkId::from_bytes([2; 32])],
        };
        assert_eq!(
            service.handle(&have, BOBS).unwrap(),
            Response::Missing(vec![ChunkId::from_bytes([1; 32])])
        );
        assert_ne!(summary(&service), before, "a marked chunk was summarised");

        // The re-offer goes through `put_chunk`, which unmarks.
        service.handle(&store_for(bob(), 1), BOBS).unwrap();
        assert_eq!(
            service.handle(&have, BOBS).unwrap(),
            Response::Missing(Vec::new())
        );
        assert_eq!(summary(&service), before);
    }

    #[test]
    fn red_team_eviction_is_asked_for_the_real_overage() {
        // An account already past its share (its allowance shrank) needs
        // more freed than the incoming chunk. Asking for the incoming size
        // evicts one chunk, is refused anyway, and the copy went for
        // nothing. Sabotage: ask `evict_marked` for `sealed.len()`.
        assert_eq!(super::overage("account full: 50 of 20 bytes", 10), 40);
        assert_eq!(super::overage("account full: 15 of 20 bytes", 10), 5);
        assert_eq!(super::overage("something else", 10), 10);

        let node = node(&alice(), 6);
        let gate = bound(1_000);
        let roomy = service(&node).with_owners(&gate);
        for byte in 1..=4 {
            roomy.handle(&store_for(bob(), byte), BOBS).unwrap();
        }
        roomy.handle(&may_evict(bob(), &[1, 2, 3]), BOBS).unwrap();
        // Bob's share shrinks to 20 bytes: he holds 40, wants 10 more, and
        // 30 must go. His three marks cover it.
        let tight = bound(20);
        let tight = service(&node).with_owners(&tight);
        assert_eq!(
            tight.handle(&store_for(bob(), 5), BOBS).unwrap(),
            Response::Stored { accepted: true }
        );
        assert_eq!(node.vault.held_bytes_for(bob()).unwrap(), 20);
    }

    #[test]
    fn a_full_account_makes_room_from_its_own_marks_and_never_from_anothers() {
        // Bob is at his share; Carol has marked spares. Bob's store must not
        // evict Carol's, and must evict his own only once his account is the
        // one refused for room. Sabotages: evict for any refusal; evict
        // before asking the bound; evict across owners.
        let node = node(&alice(), 4);
        let gate = bound(20);
        let service = service(&node).with_owners(&gate);
        for byte in [1, 2] {
            assert_eq!(
                service.handle(&store_for(bob(), byte), BOBS).unwrap(),
                Response::Stored { accepted: true }
            );
            service.handle(&store_for(carol(), byte), CAROLS).unwrap();
        }
        service
            .handle(&may_evict(carol(), &[1, 2]), CAROLS)
            .unwrap();

        match service.handle(&store_for(bob(), 3), BOBS).unwrap() {
            Response::Refused(why) => assert!(why.starts_with(ACCOUNT_FULL), "{why}"),
            other => panic!("Bob stored past his share on Carol's marks: {other:?}"),
        }
        assert_eq!(node.vault.held_bytes_for(carol()).unwrap(), 20);

        // A device with no say gets the same refusal and evicts nothing.
        service.handle(&may_evict(bob(), &[1]), BOBS).unwrap();
        let stranger = DeviceId::from_bytes([0x43; 32]);
        assert_eq!(
            service.handle(&store_for(bob(), 3), stranger).unwrap(),
            Response::Refused(UPDATE_REQUIRED.to_owned())
        );
        assert!(
            node.vault
                .has_chunk(bob(), &ChunkId::from_bytes([1; 32]))
                .unwrap()
        );

        // Not full: a store that fits evicts nothing even with marks waiting.
        let roomy = bound(1_000);
        let roomy = super::PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1))
            .with_owners(&roomy);
        roomy.handle(&store_for(bob(), 4), BOBS).unwrap();
        assert!(
            node.vault
                .has_chunk(bob(), &ChunkId::from_bytes([1; 32]))
                .unwrap()
        );
        node.vault
            .remove_chunk(bob(), &ChunkId::from_bytes([4; 32]))
            .unwrap();

        assert_eq!(
            service.handle(&store_for(bob(), 3), BOBS).unwrap(),
            Response::Stored { accepted: true },
            "Bob's own spare did not make room for Bob"
        );
        assert!(
            !node
                .vault
                .has_chunk(bob(), &ChunkId::from_bytes([1; 32]))
                .unwrap()
        );
        assert!(
            node.vault
                .has_chunk(bob(), &ChunkId::from_bytes([2; 32]))
                .unwrap()
        );
        assert_eq!(node.vault.held_bytes_for(bob()).unwrap(), 20);
    }
}
