//! Where this account's chunks should go, frozen for one round.
//!
//! The pure rules live in [`itsanas_placement`]: what a chunk needs
//! ([`assess`]), who should hold the next copy ([`want`]), which copy a full
//! host may drop ([`release_holder`]). This module feeds them from the store:
//! who the hosts are, what each one has proven, and which records still count
//! as copies. A push then asks it one question per chunk.
//!
//! What this module refuses to do:
//!
//! - **Read the coordinator.** Availability is what this device measured from
//!   its own sessions, on its own clock ([`Store::uptimes`]). A coordinator
//!   that reported every host as always on would otherwise decide where every
//!   chunk goes.
//! - **Change during a round.** [`Placement::build`] reads the store once. A
//!   host that fills up mid-round is still ranked until the next build; the
//!   push that hit the refusal stops offering, and the next round places
//!   elsewhere. Rebuilding per chunk would let two pushes of one round rank
//!   one chunk two ways.
//! - **Place on own devices.** They get everything, as before placement
//!   existed; they are left out of the roster and count only as one unit of
//!   availability, never for durability.
//! - **Believe an unknown device.** A holder not in the roster is
//!   [`Account::Unknown`] and unproven: it counts towards the cap and for
//!   nothing else.
//! - **Walk without end.** [`Placement::census`] takes the page size it is
//!   given; everything per chunk is one holder range.

use std::collections::{BTreeMap, BTreeSet};

use itsanas_crypto::{ChunkId, DeviceId, UserId};
use itsanas_placement::{
    Account, GOAL, Goal, Holding, Host, Roster, Uptime, Verdict, Want, assess, lost_after,
    release_holder, want,
};
use itsanas_store::{
    Store,
    holders::{LIVE_FOR, REFRESH_AFTER},
};

use crate::{
    error::{NetError, Result},
    session::FULL_RETRY,
};

/// How long a chunk may lack durability before any eligible host is offered
/// it, ranked or not.
///
/// The ranked hosts get the first day: a host that is asleep tonight is
/// usually back tomorrow, and sending its share elsewhere meanwhile is how a
/// chunk ends up on every host that answered. Past a day the chunk is closer
/// to a loss than to a misplacement, and any eligible host is better than
/// none.
pub const SAFETY_GRACE: u64 = 24 * 60 * 60;

/// Where a round's [`Placement`] waits for the listener, which outlives
/// rounds: the daemon swaps a new one in each round, and an empty slot means
/// the service answers as it did before placement (`everyone`).
pub type PlacementSlot = std::sync::RwLock<Option<std::sync::Arc<Placement>>>;

/// One round's placement: the hosts, this account's own devices, and "now".
#[derive(Clone, Debug)]
pub struct Placement {
    roster: Roster,
    own: BTreeSet<DeviceId>,
    /// Rings of the own devices, for the one availability unit they make.
    own_rings: BTreeMap<DeviceId, Uptime>,
    this: DeviceId,
    owner: UserId,
    goal: Goal,
    now: u64,
}

/// How one chunk stands, and who should take it next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub verdict: Verdict,
    pub want: Want,
    /// Proven off-account holders. Zero means no copy anywhere this device
    /// can vouch for, which is when the first host reached takes it.
    pub remote: usize,
    /// Every device whose record still counts for this chunk, sorted.
    pub holders: Vec<DeviceId>,
}

/// What one census page found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CensusReport {
    pub examined: usize,
    /// Queued (or kept) as short of the goal.
    pub short: usize,
    /// Taken off the queue as satisfied.
    pub settled: usize,
}

impl Placement {
    /// Freeze the round's placement.
    ///
    /// Hosts are devices this node keeps a ring for and has seen within
    /// [`LIVE_FOR`], minus `own`, minus this device. Account from `accounts`
    /// (claims this node checked), else [`Account::Unknown`]. Eligible: not
    /// paused, not full under either refusal within [`FULL_RETRY`], not
    /// draining. Proven: passed an audit, not paused, not draining.
    ///
    /// A device seen before rings existed has none until its next contact
    /// writes one, and is left out until then: one round, not a gap.
    ///
    /// # Errors
    ///
    /// If the store cannot be read.
    pub fn build(
        store: &Store,
        own: BTreeSet<DeviceId>,
        accounts: &BTreeMap<DeviceId, UserId>,
        now: u64,
    ) -> Result<Self> {
        let this = store.device_id();
        let mut hosts = Vec::new();
        let mut own_rings = BTreeMap::new();
        for (device, uptime) in store.uptimes()? {
            if device == this {
                continue;
            }
            if own.contains(&device) {
                own_rings.insert(device, uptime);
                continue;
            }
            let recent = store
                .last_seen(&device)?
                .is_some_and(|seen| seen >= now.saturating_sub(LIVE_FOR));
            if !recent {
                continue;
            }
            let reliability = store.reliability(&device)?;
            let draining = store.is_draining(&device, now)?;
            let full = store
                .peer_full_since(&device)?
                .is_some_and(|since| since >= now.saturating_sub(FULL_RETRY));
            hosts.push(Host {
                device,
                account: accounts
                    .get(&device)
                    .map_or(Account::Unknown, |user| Account::Known(*user)),
                uptime: Some(uptime),
                eligible: reliability.worth_sending_to() && !full && !draining,
                proven: reliability.passed > 0 && !reliability.paused && !draining,
                recent,
            });
        }
        Ok(Self {
            roster: Roster::new(hosts, now / 3600),
            own,
            own_rings,
            this,
            owner: store.owner(),
            goal: GOAL,
            now,
        })
    }

    /// Whether `peer` is a host of this round's roster.
    #[must_use]
    pub fn is_placed(&self, peer: &DeviceId) -> bool {
        self.roster.get(peer).is_some()
    }

    /// Whether `peer` is one of this account's own devices (or this one).
    #[must_use]
    pub fn is_own(&self, peer: &DeviceId) -> bool {
        *peer == self.this || self.own.contains(peer)
    }

    /// Whether `peer` may be offered new chunks this round.
    #[must_use]
    pub fn is_eligible(&self, peer: &DeviceId) -> bool {
        self.roster.get(peer).is_some_and(|host| host.eligible)
    }

    /// The round's clock.
    #[must_use]
    pub const fn now(&self) -> u64 {
        self.now
    }

    /// Whether a short chunk, queued since `since` and standing as
    /// `evaluation`, is offered to `peer`: the one rule a push and a pull
    /// (`WantHosted`) share, so a host gets the same share whichever side
    /// opened the session.
    ///
    /// Never to a holder already counted. To an ineligible host of the
    /// roster, nothing. Otherwise when `peer` is wanted, when no proven copy
    /// exists anywhere (the first host reached takes it), or when durability
    /// has been missing past [`SAFETY_GRACE`] -- durability only: a chunk
    /// short of availability is no better off on the next eligible host than
    /// on the ranked one, and one at the cap would go everywhere.
    #[must_use]
    pub(crate) fn offers(&self, peer: &DeviceId, evaluation: &Evaluation, since: u64) -> bool {
        if evaluation.holders.binary_search(peer).is_ok() {
            return false;
        }
        if self.is_placed(peer) && !self.is_eligible(peer) {
            return false;
        }
        let wants = evaluation.want.hosts.contains(peer);
        let first = evaluation.remote == 0;
        let safety = !evaluation.verdict.durable
            && self.now.saturating_sub(since) > SAFETY_GRACE
            && self.is_eligible(peer);
        wants || first || safety
    }

    /// How `chunk` stands, from its holder records.
    ///
    /// # Errors
    ///
    /// If the store cannot be read.
    pub fn evaluate(&self, store: &Store, chunk: &ChunkId) -> Result<Evaluation> {
        Ok(self.evaluate_stamps(chunk, &store.holders_with_stamps(chunk)?))
    }

    /// Examine the next `rows` chunks of the rolling census and queue or
    /// settle each, so a chunk that lost a holder this device never heard
    /// about is still found within one pass.
    ///
    /// # Errors
    ///
    /// If the store cannot be read or written, or there is no entropy for a
    /// starting point.
    pub fn census(&self, store: &Store, rows: usize) -> Result<CensusReport> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| NetError::Refused(format!("no entropy for a census: {error}")))?;
        let page = store.census_next(rows, &ChunkId::from_bytes(bytes))?;
        let mut short = Vec::new();
        let mut settled = Vec::new();
        for (chunk, stamps) in &page {
            if self.evaluate_stamps(chunk, stamps).verdict.satisfied() {
                settled.push(*chunk);
            } else {
                short.push(*chunk);
            }
        }
        store.enqueue_short(&short, self.now)?;
        store.settle_short(&settled)?;
        Ok(CensusReport {
            examined: page.len(),
            short: short.len(),
            settled: settled.len(),
        })
    }

    /// The holder §E would release for `chunk`, judged on strict evidence
    /// only: proven and seen within [`LIVE_FOR`], a record newer than
    /// [`REFRESH_AFTER`], and no window stretched by a host's history. The
    /// soft evidence that is right for "is it still there" is wrong for "may
    /// a copy go": a copy released on a guess is a copy gone.
    ///
    /// # Errors
    ///
    /// If the store cannot be read.
    pub fn release_choice(&self, store: &Store, chunk: &ChunkId) -> Result<Option<DeviceId>> {
        let fresh = self.now.saturating_sub(REFRESH_AFTER);
        let stamps = store.holders_with_stamps(chunk)?;
        let strict: Vec<Holding<'_>> = stamps
            .iter()
            .filter(|(_, stamp)| *stamp >= fresh)
            .filter_map(|(device, _)| self.holding(device))
            .filter(|holding| holding.own || (holding.proven && holding.recent))
            .collect();
        let order: Vec<DeviceId> = self
            .roster
            .order(self.owner, chunk)
            .into_iter()
            .map(|host| host.device)
            .collect();
        Ok(release_holder(&strict, &order, &self.goal, self.now / 3600))
    }

    fn evaluate_stamps(&self, chunk: &ChunkId, stamps: &[(DeviceId, u64)]) -> Evaluation {
        let now_hour = self.now / 3600;
        // §C: a record counts while its host could still be merely away. The
        // window is the host's own (twice its longest measured absence,
        // clamped); a fixed one either drops a laptop that sleeps every
        // weekend or keeps counting a host that is gone.
        let holdings: Vec<Holding<'_>> = stamps
            .iter()
            .filter_map(|(device, stamp)| {
                let holding = self.holding(device)?;
                (*stamp >= self.now.saturating_sub(lost_after(holding.uptime))).then_some(holding)
            })
            .collect();
        let ranked: Vec<Holding<'_>> = self
            .roster
            .ranked(self.owner, chunk)
            .into_iter()
            .filter(|host| !holdings.iter().any(|held| held.device == host.device))
            .map(|host| Self::of_host(host))
            .collect();
        let verdict = assess(&holdings, &self.goal, now_hour);
        let wanted = want(&holdings, &ranked, &self.goal, now_hour);
        let mut holders: Vec<DeviceId> = holdings.iter().map(|held| held.device).collect();
        holders.sort_unstable();
        Evaluation {
            verdict,
            want: wanted,
            remote: holdings
                .iter()
                .filter(|held| !held.own && held.proven)
                .count(),
            holders,
        }
    }

    /// One record's holder, as the rules see it; `None` for this device.
    fn holding(&self, device: &DeviceId) -> Option<Holding<'_>> {
        if *device == self.this {
            return None;
        }
        if self.own.contains(device) {
            return Some(Holding {
                device: *device,
                account: Account::Known(self.owner),
                own: true,
                uptime: self.own_rings.get(device),
                proven: false,
                recent: false,
            });
        }
        Some(self.roster.get(device).map_or(
            Holding {
                device: *device,
                account: Account::Unknown,
                own: false,
                uptime: None,
                proven: false,
                recent: false,
            },
            Self::of_host,
        ))
    }

    fn of_host(host: &Host) -> Holding<'_> {
        Holding {
            device: host.device,
            account: host.account,
            own: false,
            uptime: host.uptime.as_ref(),
            proven: host.proven,
            recent: host.recent,
        }
    }
}
