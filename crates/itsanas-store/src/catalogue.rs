//! Every file this account has, including the ones this device has not
//! downloaded.
//!
//! # The gap this closes
//!
//! A metadata-only sync round fetches, verifies and keeps the signed log
//! segments without downloading any content. Every operation it cannot complete
//! comes back *deferred* — nothing half-written, local state untouched, ask
//! again later.
//!
//! Which means the file is not in the index. `Store::list` does not report it,
//! and a client can show only what it has already downloaded. That is not the
//! behaviour anybody expects from a phone: everything should be listed, and
//! tapping one should fetch it.
//!
//! # Derived, not recorded
//!
//! The obvious implementation is a table updated as segments arrive. It would
//! be faster to read and it would drift, because it would be a second copy of
//! something the vault already holds — and the day the two disagree, the one
//! the user sees is the wrong one.
//!
//! So this walks the vault instead. It is read-only, it cannot be stale, and
//! there is no repair path to write because there is nothing to repair. The
//! cost is decoding the segment chain on each call, which is CPU and no
//! network.
//!
//! **That cost is O(history) and will need attention.** It is the same walk
//! `session::pull` already does on every content round and the same one
//! `drain_vault` does every daemon loop, so it is not a new class of problem —
//! it joins the queue behind pack files. A phone listing a few thousand
//! operations will not notice; a phone listing a million will.
//!
//! # What it deliberately does not do
//!
//! It does not write an index entry for an absent file. That would be faster
//! still and it would break the invariant the rest of the store leans on: a
//! listed file is a readable file. The conflict and delete logic both assume
//! that `stat` returning `Some` means the content can be opened, and quietly
//! making that untrue is the kind of change that produces a bug nobody can
//! locate six months later.

use std::collections::{BTreeMap, BTreeSet};

/// Most segments read from one device's chain in a single walk.
///
/// Not a tuning knob — a memory bound. `segments_for` returns a `Vec`, so an
/// unlimited walk materialises an entire history in RAM, which is the property
/// the rest of this crate spends real effort protecting. The same mistake was
/// already found once, in `blobs().addresses()`, by a benchmark.
///
/// A listing that hits this bound is incomplete, and says so through
/// [`Catalogue::complete`] rather than silently showing fewer files than exist.
/// Two hundred and fifty-six matches the network layer's per-request limit,
/// which exists for the same reason.
pub const MAX_SEGMENTS_WALKED: usize = 256;

use itsanas_crypto::{ChunkId, UserId};

use crate::error::Result;
use crate::oplog::{FileEntry, Operation};
use crate::store::Store;
use crate::vault::Vault;
use crate::version::{CausalOrder, VersionVector};

/// Whether this device can open the file right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Downloaded. `Store::read_file` will return it.
    Local,
    /// Known to exist from a peer's log, and not downloaded.
    ///
    /// A client shows these and fetches on demand. Reading one fails until a
    /// content-moving sync round completes it.
    Absent,
}

/// One file, wherever this device stands with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    /// The logical path.
    pub path: String,
    /// Size in bytes, as the writing device recorded it.
    pub size: u64,
    /// When the writing device last modified it. Advisory: clocks lie.
    pub modified_unix: u64,
    /// Whether the content is here.
    pub presence: Presence,
}

/// What the log says about one path, while the walk is in progress.
struct Latest {
    version: VersionVector,
    /// `None` for a delete.
    ///
    /// The whole entry rather than its size and date, because the chunk list is
    /// what turns "you have a file you have not downloaded" into "here it is".
    /// Keeping two walks -- one for listing and one for fetching -- is how the
    /// listing and the fetch come to disagree about what a path is.
    file: Option<FileEntry>,
}

/// A listing, and whether it is the whole story.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalogue {
    /// The files, sorted by path.
    pub files: Vec<Known>,
    /// Whether every log segment was read.
    ///
    /// False when a device's chain is longer than [`MAX_SEGMENTS_WALKED`]. The
    /// listing is then a prefix rather than the whole account, and a caller
    /// showing it to a person should say so — a short list presented as
    /// complete is how somebody concludes their files are gone.
    pub complete: bool,
}

/// Every file this account has, downloaded or not, sorted by path.
///
/// Combines what the index holds — which is by definition downloaded — with
/// what this device's vault knows from other devices' logs but has not applied.
///
/// A path that is both is reported once, as [`Presence::Local`]: having the
/// content beats knowing about it.
pub fn catalogue(store: &Store, vault: &Vault) -> Result<Catalogue> {
    let owner = store.owner();
    let mine = store.device_id();

    let mut out: BTreeMap<String, Known> = BTreeMap::new();
    for path in store.list()? {
        let Some(entry) = store.stat(&path)? else {
            // Listed and then gone: another process collected it between the
            // two calls. Not an error, and not something to show.
            continue;
        };
        out.insert(
            path.clone(),
            Known {
                path,
                size: entry.size,
                modified_unix: entry.modified_unix,
                presence: Presence::Local,
            },
        );
    }

    let (from_log, complete) = walk_vault(store, vault, owner, mine)?;
    for (path, latest) in from_log {
        // Downloaded already. The vault may hold an older or a newer version;
        // either way this device can open the file, and a content round is what
        // resolves the difference.
        if out.contains_key(&path) {
            continue;
        }

        // The log's last word on this path is a delete. Nothing to show.
        let Some(entry) = &latest.file else {
            continue;
        };
        let (size, modified_unix) = (entry.size, entry.modified_unix);

        // A delete recorded *here* that the remote edit did not see. The
        // asymmetry is deliberate and documented in `sync`: a delete concurrent
        // with an edit loses, so only a delete that demonstrably saw this
        // version keeps the file hidden.
        if let Some(tombstone) = store.tombstone(&path)?
            && matches!(
                tombstone.version.compare(&latest.version),
                CausalOrder::After | CausalOrder::Equal
            )
        {
            continue;
        }

        out.insert(
            path.clone(),
            Known {
                path,
                size,
                modified_unix,
                presence: Presence::Absent,
            },
        );
    }

    Ok(Catalogue {
        files: out.into_values().collect(),
        complete,
    })
}

/// The chunks a known-but-absent file is made of.
///
/// `None` when the account has no such live file. `Some` with the chunk ids in
/// order, which is what a caller needs to go and fetch exactly that file rather
/// than syncing the whole account -- the difference between opening one
/// document on a phone and downloading somebody's photo library.
///
/// Read from the same walk that produces the listing, so a path that appears in
/// `catalogue` cannot fail to resolve here.
///
/// # Errors
///
/// If the vault or the store cannot be read.
pub fn chunks_for(store: &Store, vault: &Vault, path: &str) -> Result<Option<Vec<ChunkId>>> {
    let owner = store.owner();
    let mine = store.device_id();
    let (from_log, _) = walk_vault(store, vault, owner, mine)?;

    let Some(latest) = from_log.get(path) else {
        return Ok(None);
    };

    Ok(still_a_file(store, path, latest)?.map(|entry| entry.chunks.clone()))
}

/// The chunks of many known-but-absent files, from a single walk.
///
/// Paths the account does not have are silently absent from the answer rather
/// than an error: the caller is naming what it would like, and a file deleted
/// between the listing and this call is an ordinary race.
///
/// # Why not `chunks_for` in a loop
///
/// Each call walks every segment of every other device's chain. A device
/// deciding what to fetch names as many files as its budget holds, so the loop
/// is O(files x history) on the one class of machine -- a phone -- least able to
/// afford it. This is O(history).
///
/// # Errors
///
/// If the vault or the store cannot be read.
pub fn chunks_for_all(
    store: &Store,
    vault: &Vault,
    paths: &BTreeSet<String>,
) -> Result<BTreeSet<ChunkId>> {
    if paths.is_empty() {
        return Ok(BTreeSet::new());
    }

    let owner = store.owner();
    let mine = store.device_id();
    let (from_log, _) = walk_vault(store, vault, owner, mine)?;

    let mut wanted = BTreeSet::new();
    for path in paths {
        let Some(latest) = from_log.get(path) else {
            continue;
        };
        if let Some(entry) = still_a_file(store, path, latest)? {
            wanted.extend(entry.chunks.iter().copied());
        }
    }
    Ok(wanted)
}

/// The log's last word on `path`, unless a local delete supersedes it.
///
/// A delete recorded here that the remote edit did not see loses, exactly as it
/// does in [`catalogue`] and in the merge engine: a delete concurrent with an
/// edit is resolved in favour of the edit, because an unexpected file costs a
/// second to remove again and a lost edit is unrecoverable. Written once and
/// called from each place that asks, because three copies of a rule is three
/// answers to one question.
fn still_a_file<'a>(
    store: &Store,
    path: &str,
    latest: &'a Latest,
) -> Result<Option<&'a FileEntry>> {
    let Some(entry) = &latest.file else {
        return Ok(None);
    };

    if let Some(tombstone) = store.tombstone(path)?
        && matches!(
            tombstone.version.compare(&latest.version),
            CausalOrder::After | CausalOrder::Equal
        )
    {
        return Ok(None);
    }

    Ok(Some(entry))
}

/// How many files are known but not downloaded.
///
/// For a status line, without building the whole list.
pub fn absent_count(store: &Store, vault: &Vault) -> Result<usize> {
    Ok(catalogue(store, vault)?
        .files
        .into_iter()
        .filter(|known| known.presence == Presence::Absent)
        .count())
}

/// The last word each path gets across every log this device can read.
///
/// # Including this device's own
///
/// The obvious version walked only *other* devices' chains, on the reasoning
/// that the index is the authority for anything this machine wrote. That stopped
/// being true the day a device could let go of content: a file this device
/// created and then released has no index entry and no entry in anybody else's
/// chain either, so it vanished from its own listing entirely and `itsanas get`
/// answered "no such file" for a file two other machines were holding.
///
/// Found by running it, not by reading it: a 200 KiB file put on a Raspberry Pi
/// with a 300 KiB limit, pushed to two hosts, released as designed, and then
/// absent from `itsanas ls` on the machine that had made it.
fn walk_vault(
    store: &Store,
    vault: &Vault,
    owner: UserId,
    mine: itsanas_crypto::DeviceId,
) -> Result<(BTreeMap<String, Latest>, bool)> {
    let mut latest: BTreeMap<String, Latest> = BTreeMap::new();
    let mut complete = true;

    for (device, _, _) in vault.heads_for(owner)? {
        if device == mine {
            continue;
        }

        let segments = vault.segments_for(owner, device, None, MAX_SEGMENTS_WALKED)?;
        if segments.len() == MAX_SEGMENTS_WALKED {
            // Possibly truncated. Reported rather than guessed at: the
            // alternative is a listing that is quietly short.
            complete = false;
        }

        fold(store, segments, &mut latest)?;
    }

    // And this device's own chain, for the same reason and with the same bound.
    let mine = store.segments()?;
    let mine = if mine.len() > MAX_SEGMENTS_WALKED {
        complete = false;
        mine[mine.len() - MAX_SEGMENTS_WALKED..].to_vec()
    } else {
        mine
    };
    fold(store, mine, &mut latest)?;

    // Including what this device has written and not yet announced. A file
    // written and released before the next flush would otherwise be in no log
    // this walk reads, which is the same disappearance by a narrower door.
    for entry in store.unsealed_entries()? {
        let (path, file) = match &entry.operation {
            Operation::Upsert { path, entry } => (path.clone(), Some(entry.clone())),
            Operation::Remove { path, .. } => (path.clone(), None),
        };
        let version = entry.operation.version().clone();
        record(&mut latest, path, version, file);
    }

    Ok((latest, complete))
}

/// Apply a run of segments to the running answer.
fn fold(
    store: &Store,
    segments: Vec<crate::SegmentEnvelope>,
    latest: &mut BTreeMap<String, Latest>,
) -> Result<()> {
    for envelope in segments {
        let body = store.open_segment(&envelope)?;
        for entry in body.entries {
            let (path, file) = match &entry.operation {
                Operation::Upsert { path, entry } => (path.clone(), Some(entry.clone())),
                Operation::Remove { path, .. } => (path.clone(), None),
            };
            let version = entry.operation.version().clone();
            record(latest, path, version, file);
        }
    }
    Ok(())
}

/// Keep the operation that should decide what a listing shows.
fn record(
    latest: &mut BTreeMap<String, Latest>,
    path: String,
    version: VersionVector,
    file: Option<FileEntry>,
) {
    match latest.get_mut(&path) {
        None => {
            latest.insert(path, Latest { version, file });
        }
        Some(held) => match version.compare(&held.version) {
            CausalOrder::After => {
                held.version = version;
                held.file = file;
            }
            CausalOrder::Before | CausalOrder::Equal => {}
            // Concurrent. A delete racing an edit loses, because an unexpected
            // file costs a second and a lost edit is unrecoverable — the same
            // asymmetry the merge engine applies. So if either side is a file,
            // the listing shows a file.
            CausalOrder::Concurrent => {
                if held.file.is_none() && file.is_some() {
                    held.version = version;
                    held.file = file;
                }
            }
        },
    }
}
