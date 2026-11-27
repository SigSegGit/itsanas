//! Fetching chosen files or versions from a peer, rather than everything.

use std::cell::RefCell;
use std::collections::BTreeSet;

use itsanas_crypto::{ChunkId, DeviceId, UserId};
use itsanas_store::{Store, Vault};
use itsanas_sync::{ChunkSource, SyncReport};

use crate::{
    error::{NetError, Result},
    protocol::MAX_SEGMENTS_PER_REQUEST,
    transport::PeerClient,
};

use super::pulling::apply_per_chain;

/// Bring here, from `client`, the chunks of the version of `path` named `id`
/// ([`itsanas_store::Version::id`]) that this machine lacks, so it can be
/// brought back here: a version used to come back only on a machine that
/// had kept every chunk of it (HANDOVER §8 15). Each chunk is checked as any
/// chunk is ([`Store::accept_chunk`]), one at a time, so memory stays one
/// chunk. Answers whether every chunk is here now.
///
/// # Errors
///
/// When the store cannot be read or written, or the peer fails.
pub fn fetch_version(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    path: &str,
    id: &str,
) -> Result<bool> {
    let Some(chosen) = itsanas_store::versions(store, vault, path)?
        .into_iter()
        .find(|version| version.id() == id)
    else {
        return Ok(false);
    };
    for chunk in chosen.chunks() {
        if store.has_chunk(chunk) {
            continue;
        }
        let Some(sealed) = client.chunk(store.owner(), *chunk)? else {
            return Ok(false);
        };
        if !store.accept_chunk(chunk, &sealed)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Fetch exactly the chunks named, from a peer, and apply what they complete.
///
/// # The gap this closes
///
/// A device with a storage budget, or one that synced over a metered link, ends
/// up with files it knows about and has not downloaded --
/// `itsanas_store::catalogue` lists them, which is what a phone should show.
/// Until this existed there was no way to then *open* one: `itsanas get`
/// answered "no such file" for a file the account plainly had, and the design
/// that justified the budget had no mechanism behind it.
///
/// # Why a filtered source rather than a plain pull
///
/// The merge engine drives which chunks it asks for, so restricting a fetch to
/// one file means restricting what the *source* will serve. Everything outside
/// `wanted` is declined, the engine defers those operations exactly as it does
/// for a sleeping peer, and nothing else is downloaded. Opening one document on
/// a phone does not pull somebody's photo library.
///
/// # Errors
///
/// If the peer fails, or the store cannot be written.
pub fn fetch_only(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    wanted: &BTreeSet<ChunkId>,
) -> Result<SyncReport> {
    fetch_only_noting(store, vault, client, wanted, &mut BTreeSet::new())
}

/// [`fetch_only`], adding to `refused` the devices whose chain it left out.
///
/// For a round that refused chains at its [`refresh_tolerant`](super::refresh_tolerant) step already:
/// its report then counts each device once across both.
///
/// # Errors
///
/// If the peer fails, or the store cannot be written.
pub fn fetch_only_noting(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    wanted: &BTreeSet<ChunkId>,
    refused: &mut BTreeSet<DeviceId>,
) -> Result<SyncReport> {
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

    // And this device's own chain, which the ordinary pull skips because the
    // index is the authority for anything this machine wrote. That stops being
    // true once content can be *released*: the file is still in the account and
    // its operation is still in this chain, but there is no index entry, so
    // without this `itsanas get` answers "no such file" for a file two other
    // machines are holding. Measured on a Raspberry Pi, after the catalogue had
    // been fixed to list it -- listing it and being unable to fetch it is the
    // worse of the two failures.
    //
    // Safe to replay because the filter decides: everything outside `wanted`
    // is declined and deferred, and a `Remove` later in the same chain still
    // wins, so nothing this device deleted comes back.
    segments.extend(store.segments()?);

    if segments.is_empty() {
        let mut report = SyncReport::default();
        report.note_refused(refused);
        return Ok(report);
    }

    let source = SelectedChunks {
        client: RefCell::new(client),
        wanted,
        served: RefCell::new(Vec::new()),
    };
    // Replaying this device's own chain as well, which the ordinary pull does
    // not: see the comment above `segments.extend(store.segments()?)`. Safe
    // here and only here, because this source serves a chosen set and declines
    // everything else -- replaying with a source that serves everything would
    // undo every release on the next round. Per chain, as `pull_scoped`: the
    // vault keeps a forged chain for good, and opening a file must not wait on
    // it.
    let mut report = apply_per_chain(
        store,
        &segments,
        &source,
        refused,
        itsanas_sync::Replay::IncludingOwn,
    )
    .map_err(|error| NetError::Refused(error.to_string()))?;
    report.note_refused(refused);

    let served = kept(store, source.served.into_inner());
    if !served.is_empty() {
        let peer = source.client.into_inner().peer_device();
        store.record_holders(&served, &peer)?;
    }

    Ok(report)
}

/// Of the chunks a peer answered, the ones that are now on this disk.
///
/// A source records every answer, and the engine only then checks the bytes.
/// Noise under a real address is refused by `Store::accept_chunk`, and a peer
/// that answered with noise does not hold the chunk: writing it down as a
/// holder would count a copy that does not exist, and repair would ask it
/// first. A chunk is fetched only when it is absent, so present afterwards
/// means accepted.
pub(super) fn kept(store: &Store, served: Vec<ChunkId>) -> Vec<ChunkId> {
    served
        .into_iter()
        .filter(|address| store.has_chunk(address))
        .collect()
}

/// A remote source that serves only the chunks of one file.
struct SelectedChunks<'a> {
    client: RefCell<&'a mut PeerClient>,
    wanted: &'a BTreeSet<ChunkId>,
    served: RefCell<Vec<ChunkId>>,
}

impl ChunkSource for SelectedChunks<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> itsanas_sync::Result<Option<Vec<u8>>> {
        if !self.wanted.contains(address) {
            return Ok(None);
        }

        let fetched = self
            .client
            .borrow_mut()
            .chunk(owner, *address)
            .map_err(|error| itsanas_sync::SyncError::Source(error.to_string()))?;

        if fetched.is_some() {
            self.served.borrow_mut().push(*address);
        }
        Ok(fetched)
    }
}
