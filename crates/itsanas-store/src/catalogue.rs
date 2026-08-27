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

use std::collections::BTreeMap;

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

use itsanas_crypto::UserId;

use crate::error::Result;
use crate::oplog::Operation;
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
    file: Option<(u64, u64)>,
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
        let Some((size, modified_unix)) = latest.file else {
            continue;
        };

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

/// The last word each path gets across every other device's log.
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

        for envelope in segments {
            let body = store.open_segment(&envelope)?;
            for entry in body.entries {
                let (path, file) = match &entry.operation {
                    Operation::Upsert { path, entry } => {
                        (path.clone(), Some((entry.size, entry.modified_unix)))
                    }
                    Operation::Remove { path, .. } => (path.clone(), None),
                };
                let version = entry.operation.version().clone();
                record(&mut latest, path, version, file);
            }
        }
    }

    Ok((latest, complete))
}

/// Keep the operation that should decide what a listing shows.
fn record(
    latest: &mut BTreeMap<String, Latest>,
    path: String,
    version: VersionVector,
    file: Option<(u64, u64)>,
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
