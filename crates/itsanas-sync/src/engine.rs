//! Applying another device's operations to this one.
//!
//! Everything here is a pure decision about *causality*: given what this device
//! already believes about a path and what a peer is claiming, which of the two
//! survives, and does the other need somewhere to live?
//!
//! # The decision table
//!
//! | Local state | Remote says | Outcome |
//! | --- | --- | --- |
//! | nothing | upsert | adopt |
//! | nothing | remove | record the tombstone |
//! | file, older | upsert | adopt |
//! | file, newer or equal | anything | ignore |
//! | file, concurrent | upsert | **conflict** — both survive, one moves aside |
//! | file, concurrent | remove | keep the file; the delete loses |
//! | tombstone, older | upsert | adopt — the file comes back |
//! | tombstone, concurrent | upsert | adopt — the edit beats the delete |
//! | tombstone, newer or equal | upsert | stay deleted |
//!
//! # Why a delete loses every race it does not clearly win
//!
//! A delete that is concurrent with an edit is discarded, and the file stays.
//! That is deliberate asymmetry. If the delete wins, somebody's edit is gone
//! and no action they can take recovers it. If the edit wins, somebody sees a
//! file they thought they had deleted, and deleting it again takes one second.
//! The two errors are not remotely equal in cost, so the tie goes to the edit.
//!
//! A delete that *demonstrably saw* the edit — its version dominates — is
//! honoured normally. This rule only governs genuine races.

use itsanas_crypto::{ChunkId, DeviceId, UserId};
use itsanas_store::{
    CausalOrder, FileEntry, LogEntry, Operation, SegmentEnvelope, Store, Tombstone, VersionVector,
};

use crate::{conflict, error::Result, source::ChunkSource};

/// What applying one operation did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The remote version was newer, or the path was unknown. Taken.
    Adopted,
    /// This device already knew this exact version.
    AlreadyKnown,
    /// This device holds something strictly newer. Nothing to do.
    Superseded,
    /// Concurrent edits. Both survive; the loser was written to `sibling`.
    Conflicted {
        /// Where the losing version now lives.
        sibling: String,
        /// Whether the incoming version is the one that kept the original path.
        remote_kept_original: bool,
    },
    /// A delete raced an edit and lost. The file stays.
    DeleteLostToEdit,
    /// The operation could not be applied because its chunks are not available
    /// from any host this device can currently reach.
    ///
    /// Not an error: the usual cause is that the device holding them is asleep.
    /// The operation is simply retried on a later sync round.
    Deferred {
        /// How many of the operation's chunks are missing.
        missing: usize,
    },
}

/// One operation's outcome, with the path it applied to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub path: String,
    pub applied: Applied,
}

/// Statistics for one sync round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub adopted: usize,
    pub already_known: usize,
    pub superseded: usize,
    pub conflicted: usize,
    pub deletes_lost: usize,
    pub deferred: usize,
}

impl SyncReport {
    fn record(&mut self, applied: &Applied) {
        match applied {
            Applied::Adopted => self.adopted += 1,
            Applied::AlreadyKnown => self.already_known += 1,
            Applied::Superseded => self.superseded += 1,
            Applied::Conflicted { .. } => self.conflicted += 1,
            Applied::DeleteLostToEdit => self.deletes_lost += 1,
            Applied::Deferred { .. } => self.deferred += 1,
        }
    }

    /// Whether anything still needs another round.
    #[must_use]
    pub const fn needs_another_round(&self) -> bool {
        self.deferred > 0
    }

    /// Whether this round changed anything at all.
    #[must_use]
    pub const fn changed_anything(&self) -> bool {
        self.adopted > 0 || self.conflicted > 0
    }
}

/// Apply a peer's segments to `store`, fetching any chunks it needs.
///
/// Segments must be presented oldest first. Their envelopes are verified and
/// their chain checked before anything is applied, so a host that tampered with
/// or dropped a segment is caught before its content can influence local state.
pub fn apply_segments(
    store: &Store,
    segments: &[SegmentEnvelope],
    source: &dyn ChunkSource,
) -> Result<(SyncReport, Vec<Outcome>)> {
    // Chains are per device. A host serves segments from several devices
    // interleaved, so they have to be separated before any chain can be
    // checked — validating the interleaved sequence would report a break on
    // every alternation.
    let mut by_device: std::collections::BTreeMap<DeviceId, Vec<SegmentEnvelope>> =
        std::collections::BTreeMap::new();
    for envelope in segments {
        by_device
            .entry(envelope.device)
            .or_default()
            .push(envelope.clone());
    }

    let mut report = SyncReport::default();
    let mut outcomes = Vec::new();

    // BTreeMap iterates in device-id order, so two devices given the same
    // segments apply them in the same order. The merge rules are designed to
    // be order-independent anyway, but determinism here means a divergence
    // shows up as a reproducible test failure rather than a flaky one.
    for (device, chain) in by_device {
        // A device's own segments are already reflected in its state; replaying
        // them would be harmless but pointless work.
        if device == store.device_id() {
            continue;
        }

        itsanas_store::validate_chain(&chain)?;

        for envelope in &chain {
            let body = store.open_segment(envelope)?;
            for entry in &body.entries {
                let outcome = apply_entry(store, device, entry, envelope.owner, source)?;
                report.record(&outcome.applied);
                outcomes.push(outcome);
            }
        }
    }

    Ok((report, outcomes))
}

/// Apply one log entry.
fn apply_entry(
    store: &Store,
    device: DeviceId,
    entry: &LogEntry,
    owner: UserId,
    source: &dyn ChunkSource,
) -> Result<Outcome> {
    let path = entry.operation.path().to_owned();

    let applied = match &entry.operation {
        Operation::Upsert {
            entry: remote_entry,
            ..
        } => apply_upsert(store, &path, remote_entry, owner, source)?,
        Operation::Remove { version, .. } => apply_remove(store, &path, entry, version, device)?,
    };

    Ok(Outcome { path, applied })
}

fn apply_upsert(
    store: &Store,
    path: &str,
    remote: &FileEntry,
    owner: UserId,
    source: &dyn ChunkSource,
) -> Result<Applied> {
    // Decide first, fetch second. Fetching is the expensive part, and most
    // incoming operations are ones this device already knows about.
    let local = store.stat(path)?;

    if let Some(local) = &local {
        match local.version.compare(&remote.version) {
            CausalOrder::Equal => return Ok(Applied::AlreadyKnown),
            CausalOrder::After => return Ok(Applied::Superseded),
            CausalOrder::Before => {}
            CausalOrder::Concurrent => {
                return apply_conflict(store, path, remote, local, owner, source);
            }
        }
    } else if let Some(tombstone) = store.tombstone(path)? {
        match tombstone.version.compare(&remote.version) {
            // The delete saw this edit, or a later one. Stay deleted.
            CausalOrder::After | CausalOrder::Equal => return Ok(Applied::Superseded),
            // The edit came after the delete, or raced it. Either way the file
            // comes back — see the module docs for why a race resurrects.
            CausalOrder::Before | CausalOrder::Concurrent => {}
        }
    }

    match fetch_missing(store, owner, &remote.chunks, source)? {
        0 => {
            store.adopt_entry(path, remote)?;
            Ok(Applied::Adopted)
        }
        missing => Ok(Applied::Deferred { missing }),
    }
}

fn apply_conflict(
    store: &Store,
    path: &str,
    remote: &FileEntry,
    local: &FileEntry,
    owner: UserId,
    source: &dyn ChunkSource,
) -> Result<Applied> {
    // Both versions are about to exist, so both need their chunks.
    let missing = fetch_missing(store, owner, &remote.chunks, source)?;
    if missing > 0 {
        return Ok(Applied::Deferred { missing });
    }

    // Authorship, not "who is running this code". The local copy may itself
    // have been adopted from a third device, and using this device's identity
    // for it would make two devices reach different verdicts about the same
    // pair of versions.
    let (remote_device, remote_sequence) = remote.authorship();
    let (local_device, local_sequence) = local.authorship();

    let remote_wins = conflict::wins_original_path(
        (remote_device, remote_sequence),
        (local_device, local_sequence),
    );

    let (loser, loser_device, loser_sequence) = if remote_wins {
        (local, local_device, local_sequence)
    } else {
        (remote, remote_device, remote_sequence)
    };
    let sibling = conflict::sibling_path(path, loser_device, loser_sequence);

    // Idempotence. Hosts re-serve segments freely — there is no acknowledgement
    // protocol telling one it can stop — so applying the same conflicting
    // operation twice must be a no-op. Without this check a resolved conflict
    // is re-resolved on every sync round, and a settle loop that stops when
    // nothing changes would never stop.
    if let Some(existing) = store.stat(&sibling)?
        && existing.version.compare(&loser.version) == CausalOrder::Equal
    {
        return Ok(Applied::AlreadyKnown);
    }

    if remote_wins {
        // The incoming version takes the original path; ours moves aside.
        store.adopt_entry(&sibling, local)?;
        store.adopt_entry(path, remote)?;
    } else {
        // We keep the original path; the incoming version moves aside.
        store.adopt_entry(&sibling, remote)?;
    }

    Ok(Applied::Conflicted {
        sibling,
        remote_kept_original: remote_wins,
    })
}

fn apply_remove(
    store: &Store,
    path: &str,
    entry: &LogEntry,
    version: &VersionVector,
    device: DeviceId,
) -> Result<Applied> {
    if let Some(local) = store.stat(path)? {
        match local.version.compare(version) {
            // We hold a version the delete never saw. The edit wins.
            CausalOrder::Concurrent | CausalOrder::After => {
                return Ok(Applied::DeleteLostToEdit);
            }
            CausalOrder::Before | CausalOrder::Equal => {}
        }
    } else if let Some(existing) = store.tombstone(path)? {
        match existing.version.compare(version) {
            CausalOrder::Equal => return Ok(Applied::AlreadyKnown),
            CausalOrder::After => return Ok(Applied::Superseded),
            CausalOrder::Before | CausalOrder::Concurrent => {}
        }
    }

    store.adopt_tombstone(
        path,
        &Tombstone {
            version: version.clone(),
            removed_unix: entry.recorded_unix,
            author: device,
        },
    )?;
    Ok(Applied::Adopted)
}

/// Fetch any of `chunks` this store does not already hold.
///
/// Returns how many are still missing afterwards.
fn fetch_missing(
    store: &Store,
    owner: UserId,
    chunks: &[ChunkId],
    source: &dyn ChunkSource,
) -> Result<usize> {
    let mut missing = 0;

    for address in chunks {
        if store.has_chunk(address) {
            continue;
        }
        match source.fetch(owner, address)? {
            Some(sealed) => {
                store.put_sealed_chunk(address, &sealed)?;
            }
            None => missing += 1,
        }
    }

    Ok(missing)
}

/// Every path both stores agree on, for convergence checking.
///
/// Two devices have converged when this returns no differences.
pub fn diff(left: &Store, right: &Store) -> Result<Vec<Divergence>> {
    let mut differences = Vec::new();

    let left_files: std::collections::BTreeMap<String, FileEntry> =
        left.entries()?.into_iter().collect();
    let right_files: std::collections::BTreeMap<String, FileEntry> =
        right.entries()?.into_iter().collect();

    for (path, entry) in &left_files {
        match right_files.get(path) {
            None => differences.push(Divergence::OnlyOnLeft { path: path.clone() }),
            Some(other) if other.content_hash != entry.content_hash => {
                differences.push(Divergence::ContentDiffers { path: path.clone() });
            }
            Some(_) => {}
        }
    }

    for path in right_files.keys() {
        if !left_files.contains_key(path) {
            differences.push(Divergence::OnlyOnRight { path: path.clone() });
        }
    }

    Ok(differences)
}

/// One way in which two stores disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Divergence {
    OnlyOnLeft { path: String },
    OnlyOnRight { path: String },
    ContentDiffers { path: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_counts_every_outcome_kind() {
        let mut report = SyncReport::default();
        for applied in [
            Applied::Adopted,
            Applied::AlreadyKnown,
            Applied::Superseded,
            Applied::Conflicted {
                sibling: "x".to_owned(),
                remote_kept_original: true,
            },
            Applied::DeleteLostToEdit,
            Applied::Deferred { missing: 2 },
        ] {
            report.record(&applied);
        }

        assert_eq!(report.adopted, 1);
        assert_eq!(report.already_known, 1);
        assert_eq!(report.superseded, 1);
        assert_eq!(report.conflicted, 1);
        assert_eq!(report.deletes_lost, 1);
        assert_eq!(report.deferred, 1);
        assert!(report.needs_another_round());
        assert!(report.changed_anything());
    }

    #[test]
    fn a_quiet_round_reports_no_work_and_no_retry() {
        let mut report = SyncReport::default();
        report.record(&Applied::AlreadyKnown);

        assert!(!report.needs_another_round());
        assert!(
            !report.changed_anything(),
            "a round that only recognised things it already knew reported \
             progress; a sync loop would never settle"
        );
    }
}
