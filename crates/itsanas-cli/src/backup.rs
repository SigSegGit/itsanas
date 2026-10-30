//! A cold copy of this account on a disk that goes in a drawer.
//!
//! `itsanas backup export DIR` writes everything another machine of the
//! account would need to rebuild it with no peer at all: every segment of
//! every device's chain, every live chunk, and the claims that put those
//! devices on the account's roster. `itsanas backup restore DIR` reads it into
//! this node's vault and applies it, as a push from a sibling would be.
//!
//! What this refuses to do:
//!
//! - **Write anything readable.** Segments and chunks are copied sealed, as a
//!   host holds them; what the copy says in clear is what a host already sees
//!   (chunk addresses and sizes, operation counts). The 24 words are not in
//!   it: losing the disk loses nothing, and restoring needs the account.
//! - **Restore into another account.** The copy names its owner, and a node
//!   of another account refuses it before touching anything.
//! - **Trust the copy.** Every segment is verified by the vault as it is
//!   stored, every chunk is opened against its address when applied, and a
//!   claim joins the roster only on the account's signature.

use std::path::{Path, PathBuf};

use itsanas_crypto::ChunkId;
use itsanas_node::Node;
use itsanas_store::SegmentEnvelope;

use crate::error::{CliError, Result};

/// The file that says a directory is a backup, and whose.
pub const MARKER: &str = "itsanas-backup";

const SEGMENTS: &str = "segments.bin";
const CLAIMS: &str = "claims.bin";
const CHUNKS: &str = "chunks";

/// How many chunk ids are read from the index at a time.
const PAGE: usize = 4096;

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> CliError + '_ {
    move |source| CliError::Io {
        path: path.to_owned(),
        source,
    }
}

/// A chunk file's name back to its address; `None` for anything else in the
/// directory, which is skipped.
fn chunk_id_of(name: &str) -> Option<ChunkId> {
    if name.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(name.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(ChunkId::from_bytes(bytes))
}

/// What an export or a restore moved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    pub segments: usize,
    pub chunks: usize,
    pub bytes: u64,
}

/// What `backup restore` says when it is done, run in the foreground or
/// handed to the daemon as a disk job.
///
/// One function for both doors. The daemon's job once said only the counts,
/// so a restore from a disk carrying a forged chain read as done there, with
/// that device's changes left out and nothing on the job's line -- and with a
/// daemon running, the job is how every restore runs.
pub fn restore_summary(moved: &Moved, report: &itsanas_sync::SyncReport, from: &str) -> String {
    let mut said = format!(
        "read {} segments and {} chunks ({}) from {from}; {} file(s) adopted",
        moved.segments,
        moved.chunks,
        itsanas_node::config::format_size(moved.bytes),
        report.adopted
    );
    // A backup disk anyone could write to may carry a forged chain; the rest
    // is restored, and the owner is told what was not.
    if let Some(refused) = crate::describe_refused_chains(report, crate::ChainSource::Backup) {
        said.push_str("\n  ");
        said.push_str(&refused);
    }
    said
}

/// Write this account's cold copy into `dir`, which must be empty or new.
///
/// # Errors
///
/// If `dir` holds anything, or the store, the vault or the disk fail.
pub fn export(node: &Node, dir: &Path) -> Result<Moved> {
    // In the synced folder the sealed chunks would be uploaded as this
    // account's files; in the home or the vault, mixed with what they copy.
    let inside = |outer: &Path| dir.starts_with(outer);
    if inside(&node.home)
        || inside(&node.vault_dir())
        || node.config.folder.as_deref().is_some_and(inside)
    {
        return Err(CliError::Usage(format!(
            "{} is inside this node's home, vault or synced folder; a backup goes on a drive \
             of its own",
            dir.display()
        )));
    }
    if std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some()) {
        return Err(CliError::Usage(format!(
            "{} is not empty; a backup goes into an empty or new directory",
            dir.display()
        )));
    }
    let chunks = dir.join(CHUNKS);
    std::fs::create_dir_all(&chunks).map_err(io(&chunks))?;
    let owner = node.store.owner();

    let mut segments = node.store.segments()?;
    for (device, _, _) in node.vault.heads_for(owner)? {
        if device != node.store.device_id() {
            segments.extend(node.vault.segments_for(owner, device, None, usize::MAX)?);
        }
    }
    let path = dir.join(SEGMENTS);
    std::fs::write(&path, postcard::to_stdvec(&segments)?).map_err(io(&path))?;

    let mut claims = node.vault.claims_for(owner)?;
    claims.push(node.claim_bytes(itsanas_discover::now_unix()));
    let path = dir.join(CLAIMS);
    std::fs::write(&path, postcard::to_stdvec(&claims)?).map_err(io(&path))?;

    let mut moved = Moved {
        segments: segments.len(),
        ..Moved::default()
    };
    let mut after = None;
    loop {
        let (page, next) = node.store.live_chunks_page(after.as_ref(), PAGE)?;
        for address in &page {
            let sealed = match node.store.blobs().get(address)? {
                Some(sealed) => Some(sealed),
                None => node.vault.get_chunk(owner, address)?,
            };
            // A chunk this device released and holds nowhere: the copy says
            // so by its absence, and a restore fetches it from a peer later.
            let Some(sealed) = sealed else { continue };
            let path = chunks.join(address.to_hex());
            std::fs::write(&path, &sealed).map_err(io(&path))?;
            moved.chunks += 1;
            moved.bytes += sealed.len() as u64;
        }
        match next {
            Some(next) => after = Some(next),
            None => break,
        }
    }

    // Last, so a copy cut short by a full disk is never taken for a whole one.
    let path = dir.join(MARKER);
    std::fs::write(&path, owner.to_hex()).map_err(io(&path))?;
    Ok(moved)
}

/// Read the cold copy in `dir` into this node and apply it.
///
/// # Errors
///
/// If `dir` is no backup, or another account's, or the store, the vault or
/// the disk fail.
pub fn restore(node: &Node, dir: &Path) -> Result<(Moved, itsanas_sync::SyncReport)> {
    let owner = node.store.owner();
    let marker = dir.join(MARKER);
    let named = std::fs::read_to_string(&marker).map_err(|_| {
        CliError::Usage(format!(
            "{} is no ITSaNAS backup, or an incomplete one (no {MARKER})",
            dir.display()
        ))
    })?;
    if named.trim() != owner.to_hex() {
        return Err(CliError::Usage(format!(
            "{} is a backup of another account; this node is {owner}",
            dir.display()
        )));
    }

    let path = dir.join(CLAIMS);
    let claims: Vec<Vec<u8>> = postcard::from_bytes(&std::fs::read(&path).map_err(io(&path))?)?;
    node.admit_roster(&claims)?;

    let path = dir.join(SEGMENTS);
    let segments: Vec<SegmentEnvelope> =
        postcard::from_bytes(&std::fs::read(&path).map_err(io(&path))?)?;
    let mut moved = Moved::default();
    for envelope in &segments {
        // This device's own chain, from a backup of this very device, is
        // already in its index; the vault would refuse it as a fork anyway.
        if envelope.owner == owner && node.vault.put_segment(envelope)? {
            moved.segments += 1;
        }
    }

    let chunks = dir.join(CHUNKS);
    for entry in std::fs::read_dir(&chunks).map_err(io(&chunks))? {
        let entry = entry.map_err(io(&chunks))?;
        let Some(address) = entry.file_name().to_str().and_then(chunk_id_of) else {
            continue;
        };
        let path: PathBuf = entry.path();
        let sealed = std::fs::read(&path).map_err(io(&path))?;
        // Into the store, verified against its address as any pulled chunk
        // is: a chunk swapped on the backup disk is refused here and never
        // kept, let alone served to a sibling.
        if node.store.accept_chunk(&address, &sealed)? {
            moved.chunks += 1;
            moved.bytes += sealed.len() as u64;
        }
    }

    let report = itsanas_net::session::drain_vault(&node.store, &node.vault)?;
    Ok((moved, report))
}
