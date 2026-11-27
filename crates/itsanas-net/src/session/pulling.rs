//! Pulling a peer's log into this device: chain by chain, so one forged
//! chain costs that chain and never the call, and draining the vault.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use itsanas_crypto::{ChunkId, DeviceId, UserId};
use itsanas_store::{SegmentEnvelope, Store, StoreError, Vault};
use itsanas_sync::{ChunkSource, Replay, SyncReport};

use crate::{
    error::{NetError, Result},
    protocol::MAX_SEGMENTS_PER_REQUEST,
    transport::PeerClient,
};

use super::fetching::kept;
use super::{RemoteChunks, Scope};

/// Fetch what the peer has from this user's *other* devices, and merge it.
///
/// # Errors
///
/// If the peer fails, or the store cannot be written.
pub fn pull(store: &Store, vault: &Vault, client: &mut PeerClient) -> Result<SyncReport> {
    pull_scoped(store, vault, client, Scope::Everything)
}

/// Fetch what the peer has from this user's *other* devices, and merge it, at
/// `scope`.
///
/// At [`Scope::Metadata`] the segments are still fetched, verified and kept, so
/// this node can relay them onwards and the next content round resumes instead
/// of starting over. Every operation whose chunks would have to be downloaded
/// comes back as `deferred`. Nothing is half-written: an operation is either
/// applied with its content or left for later, which is the same guarantee a
/// sleeping peer already gets.
///
/// Bring this node's vault up to date with what the peer knows, without
/// applying anything.
///
/// Kilobytes: signed log segments, not content. What it buys is the ability to
/// *decide* — `itsanas_store::catalogue` reads the vault, so after this a device
/// knows every file the account has, with its size and date, and can choose
/// which ones are worth its remaining room before spending a byte of it on
/// content.
///
/// Returns the segments newly fetched in this call, which is what the callers
/// that go on to apply them need.
///
/// # Errors
///
/// If the peer fails, or the vault cannot be written.
pub fn refresh(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
) -> Result<Vec<SegmentEnvelope>> {
    refresh_chains(store, vault, client, Tolerate::Nothing).map(|(fetched, _)| fetched)
}

/// [`refresh`], leaving out a device chain the peer served forged instead of
/// failing the call, and returning the devices it left out.
///
/// For a round that goes on to fetch what it chose: one forged chain must not
/// stop it learning the others.
///
/// # Errors
///
/// If the peer fails, or the vault cannot be written.
pub fn refresh_tolerant(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
) -> Result<(Vec<SegmentEnvelope>, BTreeSet<DeviceId>)> {
    refresh_chains(store, vault, client, Tolerate::RefusedChains)
}

/// Whether a segment that fails its checks ends the call or only its chain.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tolerate {
    /// Any refusal is an error, as [`refresh`] has always answered.
    Nothing,
    /// A refused segment drops the rest of that device's chain for this call;
    /// the other devices' chains are still taken. See [`pull_scoped`].
    RefusedChains,
}

/// [`refresh`], also returning the devices whose chain was refused part-way.
fn refresh_chains(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    tolerate: Tolerate,
) -> Result<(Vec<SegmentEnvelope>, BTreeSet<DeviceId>)> {
    let owner = store.owner();
    let mine = store.device_id();

    let heads = client.heads(owner)?;
    let mut fetched: Vec<SegmentEnvelope> = Vec::new();
    let mut refused = BTreeSet::new();

    for head in heads {
        if head.device == mine {
            continue;
        }

        // Resume from whatever this node's vault already holds for that device.
        let local_head = vault
            .heads_for(owner)?
            .into_iter()
            .find(|(device, _, _)| *device == head.device)
            .map(|(_, head, _)| head);

        if local_head == Some(head.head) {
            // Already current with this device. Nothing to ask for.
            continue;
        }

        let segments = client.segments(owner, head.device, local_head, MAX_SEGMENTS_PER_REQUEST)?;

        let (kept, was_refused) = keep_chain(vault, segments, tolerate)?;
        if was_refused {
            refused.insert(head.device);
        }
        fetched.extend(kept);
    }

    Ok((fetched, refused))
}

/// Put one device's segments into the vault, oldest first.
///
/// Retained so this node can relay them onwards, and so the next pull has a
/// resume point. `put_segment` verifies the signature and refuses a chain with
/// a hole. Under [`Tolerate::RefusedChains`] that refusal keeps the prefix
/// already taken, drops the rest of this chain, and says so; anything else
/// (the vault's own disk failing) is still an error.
fn keep_chain(
    vault: &Vault,
    segments: Vec<SegmentEnvelope>,
    tolerate: Tolerate,
) -> Result<(Vec<SegmentEnvelope>, bool)> {
    let mut kept = Vec::with_capacity(segments.len());
    for envelope in segments {
        match vault.put_segment(&envelope) {
            Ok(_) => kept.push(envelope),
            Err(error) if tolerate == Tolerate::RefusedChains && refuses_segment(&error) => {
                return Ok((kept, true));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok((kept, false))
}

/// The first segment of this machine's own chain, as `client` holds it,
/// that is signed by this machine's key and differs from `chain` at the same
/// position: a fork, so another disk with the same identity. `None` when the
/// peer's chain only extends this one (a machine restored from an older copy
/// is behind, not forked), when it holds nothing past it, or when what it
/// serves is not signed by this key (a peer that lies proves nothing).
pub(super) fn forked_segment(
    client: &mut PeerClient,
    owner: UserId,
    mine: DeviceId,
    chain: &[SegmentEnvelope],
) -> Result<Option<SegmentEnvelope>> {
    let mut after = None;
    let mut index = 0usize;
    // Bounded: a chain of a million segments is walked no further.
    for _ in 0..4096 {
        let page = client.segments(owner, mine, after, MAX_SEGMENTS_PER_REQUEST)?;
        let Some(last) = page.last().map(|envelope| envelope.segment_id) else {
            return Ok(None);
        };
        for envelope in &page {
            if envelope.device != mine || envelope.verify_signature().is_err() {
                return Ok(None);
            }
            match chain.get(index) {
                Some(own) if own.segment_id == envelope.segment_id => {}
                Some(_) => return Ok(Some(envelope.clone())),
                None => return Ok(None),
            }
            index += 1;
        }
        if page.len() < usize::from(MAX_SEGMENTS_PER_REQUEST) {
            return Ok(None);
        }
        after = Some(last);
    }
    Ok(None)
}

/// Whether `put_segment` refused the segment itself, as opposed to failing.
const fn refuses_segment(error: &StoreError) -> bool {
    matches!(
        error,
        StoreError::SegmentSignature { .. } | StoreError::SegmentChainBroken { .. }
    )
}

/// Apply `segments` one device chain at a time, leaving out a chain that does
/// not verify instead of failing the whole call.
///
/// # Why per chain
///
/// `apply_segments` verifies every chain before applying it and answers the
/// first failure with an error for the whole call. A host -- or any stranger
/// with a free device key, since `put_segment` checks a signature and not
/// membership of the account -- could then serve one segment that fails, and
/// stop every honest chain it also serves from being applied, round after
/// round, because the segment is kept in the vault and replayed. So each chain
/// is checked here first (`validate_chain`, then every body opened) and a
/// chain that fails is counted in `refused_chains` and skipped. The engine
/// checks it again; that second walk costs a decrypt per segment, kilobytes.
///
/// Errors while *applying* a chain that verified -- a disk, a chunk source --
/// still end the call, as before: those are not a host's word to refuse.
///
/// # Not this device's own chain
///
/// Only another device's chain is refused and skipped. This device's own
/// chain is signed by its own key, so no stranger can put one in its place:
/// when it fails to open, this disk or this key has gone wrong, and
/// `catalogue` answers that with an error for the same reason. Refusing it
/// quietly would name this device as a stranger and tell its owner to wait
/// for an honest copy of their own work. Under `Replay::OthersOnly` the engine
/// skips it unread, so it is left out here before being checked.
pub(super) fn apply_per_chain(
    store: &Store,
    segments: &[SegmentEnvelope],
    source: &dyn ChunkSource,
    refused: &mut BTreeSet<DeviceId>,
    replay: itsanas_sync::Replay,
) -> itsanas_sync::Result<SyncReport> {
    let mine = store.device_id();
    let mut chains: BTreeMap<DeviceId, Vec<SegmentEnvelope>> = BTreeMap::new();
    for envelope in segments {
        chains
            .entry(envelope.device)
            .or_default()
            .push(envelope.clone());
    }

    let mut accepted = Vec::with_capacity(segments.len());
    for (device, chain) in chains {
        if device == mine {
            if replay == itsanas_sync::Replay::OthersOnly {
                continue;
            }
            // Strict: the fault is this machine's, and its owner's to see.
            itsanas_store::validate_chain(&chain)?;
            for envelope in &chain {
                store.open_segment(envelope)?;
            }
        }
        let verifies = itsanas_store::validate_chain(&chain).is_ok()
            && chain
                .iter()
                .all(|envelope| store.open_segment(envelope).is_ok());
        if !verifies {
            refused.insert(device);
            continue;
        }
        accepted.extend(chain);
    }
    // In one call, every chain that verified: an edit made on one machine
    // supersedes an older version written on another, and the engine applies
    // the older one last, without fetching it (`superseded_upserts`).
    let (report, _) = itsanas_sync::apply_replaying(store, &accepted, source, replay)?;
    Ok(report)
}

/// Fetch and merge everything the peer has, at `scope`.
///
/// A deferred operation writes no index entry, so `Store::list` does not report
/// it. [`catalogue`](mod@itsanas_store::catalogue) is what shows it anyway: it derives the
/// account's whole file list from the vault's segments, marking what this device
/// has not downloaded, and [`fetch_only`](super::fetch_only) brings one down on demand.
///
/// A device that cannot hold its whole account does not call this: it decides
/// what belongs on it and calls [`fetch_only`](super::fetch_only). The two used to be one function
/// with a byte budget, which stopped downloading when the allowance ran out and
/// so kept whatever the log happened to replay first -- a limit on the quantity
/// with no say over the choice.
///
/// # Errors
///
/// If the peer fails, or the store cannot be written.
pub fn pull_scoped(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    scope: Scope,
) -> Result<SyncReport> {
    let owner = store.owner();
    let mine = store.device_id();

    // A refused chain is left out, not fatal: see `apply_per_chain`. The set
    // spans both stages so one device is counted once.
    let (mut fetched, mut refused) = refresh_chains(store, vault, client, Tolerate::RefusedChains)?;

    // A round that can move content applies from the **vault**, not from what
    // this round happened to fetch.
    //
    // Two reasons, and the second is the one that was actually broken. The
    // vault is a superset — every segment fetched above was just written into
    // it — and it is ordered, which matters because chain validation refuses a
    // segment that arrives before the one it follows. Splicing newly fetched
    // segments onto vault ones produced exactly that: "claims to follow X, but
    // the previous segment on this chain is none".
    //
    // And without it, work deferred by an earlier round is never retried. The
    // segments were kept, so the next pull sees the head as already current,
    // asks for nothing and applies nothing — the file that could not be
    // downloaded the first time is then never downloaded at all, silently, on
    // a node reporting a clean sync. That is the ordinary "the device holding
    // the chunks was asleep" case, which QUICKSTART describes as something a
    // later sync resolves, and which no later sync resolved.
    //
    // Applying an already-applied operation is cheap — the version comparison
    // happens before any chunk is fetched — but the walk itself is O(history),
    // and doing it on every round for every peer turned a per-round cost of
    // "the new segments" into "the whole chain, times the number of peers".
    // That was a regression, introduced with the fix and measured afterwards.
    //
    // So it only happens when something is actually outstanding: the vault
    // holds segments this device has not applied. `Store::has_unapplied` is a
    // cheap comparison of two markers, not a walk.
    //
    // Metadata rounds never replay. They could not complete anything anyway,
    // and walking a whole chain to defer it again is work for nothing.
    if scope.moves_content() && (fetched.is_empty() || store.has_unapplied(vault)?) {
        fetched.clear();
        for (device, _, _) in vault.heads_for(owner)? {
            if device == mine {
                continue;
            }
            fetched.extend(vault.segments_for(
                owner,
                device,
                None,
                usize::from(MAX_SEGMENTS_PER_REQUEST),
            )?);
        }
    }

    if fetched.is_empty() {
        let mut report = SyncReport::default();
        report.note_refused(&refused);
        return Ok(report);
    }

    let peer = client.peer_device();
    let (outcome, served) = if scope.moves_content() {
        let source = RemoteChunks {
            client: RefCell::new(client),
            served: RefCell::new(Vec::new()),
        };
        let outcome = apply_per_chain(store, &fetched, &source, &mut refused, Replay::OthersOnly);
        let served = kept(store, source.served.into_inner());
        (outcome, served)
    } else {
        (
            apply_per_chain(
                store,
                &fetched,
                &itsanas_sync::EmptySource,
                &mut refused,
                Replay::OthersOnly,
            ),
            Vec::new(),
        )
    };

    // Before the `?`. A peer that served the bytes held them, whether or not
    // the merge that asked for them then went wrong, and throwing that away
    // because of an unrelated failure would leave the ledger understating
    // replication — which is the direction that hides a real shortage.
    if !served.is_empty() {
        store.record_holders(&served, &peer)?;
    }

    let mut report = outcome.map_err(|error| NetError::Refused(error.to_string()))?;
    report.note_refused(&refused);

    // Only a round that finished everything may move the markers. One deferral
    // and they stay where they are, so the next content round replays.
    //
    // A refused chain does not hold them: it is not needed. Any content round
    // that fetched something new has `has_unapplied` true and replays the whole
    // vault, and one that fetched nothing replays anyway, so the refused chain
    // is looked at -- and reported -- on every content round regardless. A
    // marker condition was written for it and sabotage showed no test, and no
    // path, could tell it was there.
    if scope.moves_content() && report.deferred == 0 {
        store.note_all_applied(vault)?;
    }

    Ok(report)
}

/// Apply this user's own segments that peers have pushed into the vault.
///
/// A push puts segments and chunks into the *receiving* node's vault, where
/// they sit ready to be relayed. Nothing else picks them up: only [`pull`]
/// applies segments to the local store, and a node that never dials anybody
/// never pulls. Without this, a node that only ever accepts connections stays
/// permanently ignorant of the very data it is holding.
///
/// That is not a corner case. Any device behind NAT can push and cannot be
/// dialled, so for its peers this is the *only* way its work arrives.
///
/// Chunks come from the vault, not the network: a peer that pushed a segment
/// pushed the chunks with it, so nothing here needs anyone to be online.
pub fn drain_vault(store: &Store, vault: &Vault) -> Result<SyncReport> {
    let owner = store.owner();
    let mine = store.device_id();

    let mut segments = Vec::new();
    for (device, _, _) in vault.heads_for(owner)? {
        if device == mine {
            continue;
        }
        segments.extend(vault.segments_for(
            owner,
            device,
            None,
            usize::from(MAX_SEGMENTS_PER_REQUEST),
        )?);
    }

    if segments.is_empty() {
        return Ok(SyncReport::default());
    }

    // Per chain, as `pull_scoped`: anyone with a device key can push a forged
    // segment here, and as one batch it failed every drain for good.
    let source = VaultChunks { vault, owner };
    let mut refused = BTreeSet::new();
    let mut report = apply_per_chain(store, &segments, &source, &mut refused, Replay::OthersOnly)
        .map_err(|error| NetError::Refused(error.to_string()))?;
    report.note_refused(&refused);

    Ok(report)
}

/// Serves chunks out of the local vault.
struct VaultChunks<'a> {
    vault: &'a Vault,
    owner: UserId,
}

impl ChunkSource for VaultChunks<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> itsanas_sync::Result<Option<Vec<u8>>> {
        if owner != self.owner {
            return Ok(None);
        }
        self.vault
            .get_chunk(owner, address)
            .map_err(|error| itsanas_sync::SyncError::Source(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three genuine segments on one chain, signed by `seed`'s device.
    fn a_chain(seed: u8) -> Vec<SegmentEnvelope> {
        use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};
        use itsanas_store::{LogEntry, Operation, VersionVector};

        let user = UserKeys::derive(&MasterSecret::from_bytes([0xC2; 32]));
        let device = DeviceKeys::from_seed(&SecretBytes::new([seed; 32]));
        let mut previous = None;
        (1..=3)
            .map(|sequence| {
                let entry = LogEntry {
                    sequence,
                    recorded_unix: 0,
                    operation: Operation::Remove {
                        path: format!("gone-{sequence}"),
                        version: VersionVector::default(),
                    },
                };
                let envelope = SegmentEnvelope::create(
                    user.oplog_root(),
                    user.user_id(),
                    &device,
                    previous,
                    vec![entry],
                )
                .expect("segment");
                previous = Some(envelope.segment_id);
                envelope
            })
            .collect()
    }

    #[test]
    fn red_team_a_host_serving_a_forged_segment_loses_that_chain_not_the_call() {
        // THE ATTACK, at the first door a pull opens: a host whose code serves
        // a tampered segment in the middle of one device's chain. `put_segment`
        // refuses it, and that refusal used to end `refresh` with an error, so
        // the chains of every other device the host also served were never
        // taken. A modified host is not something the honest test server can
        // play, so this checks the per-chain step directly.
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path().join("vault")).expect("vault");

        let mut chain = a_chain(0x31);
        chain[1].sealed_body[0] ^= 0xFF;

        let (kept, refused) =
            keep_chain(&vault, chain.clone(), Tolerate::RefusedChains).expect(concat!(
                "one forged segment ended the whole refresh: every other device's ",
                "chain on this host is stalled behind it"
            ));
        assert!(refused, "the forged chain was taken silently, not reported");
        assert_eq!(
            kept,
            vec![chain[0].clone()],
            "the genuine prefix before the forgery should be kept, nothing after it"
        );

        // And the strict door `refresh` keeps for its other callers still says no.
        let other = tempfile::tempdir().expect("temp dir");
        let strict = Vault::open(other.path().join("vault")).expect("vault");
        assert!(keep_chain(&strict, chain, Tolerate::Nothing).is_err());
    }

    /// This device's own chain failing to open is an error, never a refusal.
    ///
    /// Signed by this device's own key, so no stranger put it there: it is
    /// this disk or this key gone wrong. Refused like a stranger's, `itsanas
    /// get` "succeeded", named this device as the refused one and told its
    /// owner to wait for an honest copy of their own work. Sabotage: drop the
    /// strict block for `device == mine` in `apply_per_chain` -- the replay
    /// then answers `Ok` with this device in `refused`; drop only its
    /// `OthersOnly` skip -- a pull that never replays this chain fails on it.
    #[test]
    fn this_devices_own_chain_failing_to_open_is_an_error_not_a_refusal() {
        use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};
        use itsanas_store::{LogEntry, Operation, VersionVector};

        let dir = tempfile::tempdir().expect("temp dir");
        let user = UserKeys::derive(&MasterSecret::from_bytes([0xB9; 32]));
        let owner = user.user_id();
        let store = Store::open_for_testing(
            dir.path(),
            user,
            DeviceKeys::from_seed(&SecretBytes::new([0xBA; 32])),
            itsanas_store::ChunkerConfig::default(),
        )
        .expect("store");
        // This device's key, a body sealed under a root that is not the
        // account's: what a damaged own segment looks like to `open_segment`.
        let wrong_root = UserKeys::derive(&MasterSecret::from_bytes([0xBB; 32]));
        let unopenable = SegmentEnvelope::create(
            wrong_root.oplog_root(),
            owner,
            &DeviceKeys::from_seed(&SecretBytes::new([0xBA; 32])),
            None,
            vec![LogEntry {
                sequence: 1,
                recorded_unix: 0,
                operation: Operation::Remove {
                    path: "mine.txt".to_owned(),
                    version: VersionVector::default(),
                },
            }],
        )
        .expect("segment");
        let segments = vec![unopenable];

        let mut refused = BTreeSet::new();
        let replayed = apply_per_chain(
            &store,
            &segments,
            &itsanas_sync::EmptySource,
            &mut refused,
            itsanas_sync::Replay::IncludingOwn,
        );
        assert!(
            replayed.is_err(),
            "this device's own chain failed to open and the fetch still succeeded: the \
             owner is told to wait for an honest copy of their own work, not that their \
             disk or key is failing"
        );
        assert!(
            refused.is_empty(),
            "this device named itself as a refused stranger: {refused:?}"
        );

        let pulled = apply_per_chain(
            &store,
            &segments,
            &itsanas_sync::EmptySource,
            &mut refused,
            itsanas_sync::Replay::OthersOnly,
        );
        assert!(
            pulled.is_ok() && refused.is_empty(),
            "a round that never replays this device's own chain failed or refused on it \
             ({pulled:?}, {refused:?}): every pull would stop on a chain it does not read"
        );
    }
}
