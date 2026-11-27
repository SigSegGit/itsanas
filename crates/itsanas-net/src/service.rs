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

use itsanas_crypto::{DeviceId, UserId};
use itsanas_store::{Store, Vault};

use crate::{
    error::Result,
    protocol::{MAX_SEGMENTS_PER_REQUEST, PROTOCOL_VERSION, Request, Response, challenge_proof},
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
        | Request::SummaryRoot { owner }
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
        | Request::AppMode
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
    /// Set to the time a machine of this node's own account last presented a
    /// claim here ([`Self::noting_siblings`]).
    sibling_seen: Option<&'a std::sync::atomic::AtomicU64>,
    /// Whether this node runs in application mode now, as the daemon keeps
    /// it ([`Self::in_app_mode`]); absent, it does not.
    app_mode: Option<&'a std::sync::atomic::AtomicBool>,
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
            sibling_seen: None,
            app_mode: None,
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
    /// Note in `seen` when a live machine of this node's own account presents
    /// its claim here: a contact it made, which a stranger cannot (the claim
    /// is signed with the account's key). A public machine the others only
    /// dial in to read `isolated` though they reached it at every round.
    #[must_use]
    pub const fn noting_siblings(mut self, seen: &'a std::sync::atomic::AtomicU64) -> Self {
        self.sibling_seen = Some(seen);
        self
    }

    /// Answer [`Request::AppMode`] from `on`, which the daemon sets while
    /// this machine runs in application mode and syncs in full.
    #[must_use]
    pub const fn in_app_mode(mut self, on: &'a std::sync::atomic::AtomicBool) -> Self {
        self.app_mode = Some(on);
        self
    }

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
                // Asked whether it is there, never read: reading each chunk
                // whole to answer made a 32 KB request of 1024 addresses cost
                // the host up to 256 MB of disk, as often as a peer cared to
                // ask (red team, 2026-11-23).
                let mut missing = Vec::new();
                for address in addresses {
                    if marked.contains(address) || !self.holds(*owner, address)? {
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

            Request::StoreSegment { envelope } => self.store_segment(caller, envelope),

            Request::Challenge {
                owner,
                address,
                nonce,
            } => match self.chunk(*owner, address)? {
                Some(sealed) => Ok(Response::ChallengeProof(challenge_proof(nonce, &sealed))),
                None => Ok(Response::Refused("chunk not held".to_owned())),
            },

            Request::WantHosted { limit } => self.want_hosted(*limit, caller),

            Request::ChunkSummary { owner } => Ok(Response::ChunkSummary(self.summary_of(*owner)?)),
            Request::AppMode => Ok(Response::AppMode {
                on: self
                    .app_mode
                    .is_some_and(|on| on.load(std::sync::atomic::Ordering::Relaxed)),
            }),
            Request::SummaryRoot { owner } => Ok(Response::SummaryRoot(
                itsanas_store::summary::root(&self.summary_of(*owner)?),
            )),

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

            Request::Hosted { chunks } => self.hosted(caller, chunks),
        }
    }
}

mod claims;
mod holders;
mod serving;
mod storing;

#[cfg(test)]
mod tests;
