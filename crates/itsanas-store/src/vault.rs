//! Storage for other people's data.
//!
//! This is the half of the bargain you keep. A [`Vault`] holds sealed chunks and
//! signed log segments belonging to *other users*, and it is built so that
//! reading them is not merely forbidden but structurally impossible: the type
//! has no key material of any kind, takes none in any constructor, and calls
//! nothing that decrypts. If a future change tried to make the vault read its
//! contents, it would have to acquire a key from somewhere first, and there is
//! nowhere to get one.
//!
//! Kept deliberately separate from [`Store`](crate::store::Store), which holds
//! *your* data and does have keys. One directory, one database, one set of
//! rules each. The alternative — a single store with an `owner` column and a
//! branch that decides whether to decrypt — puts a single `if` between a
//! stranger's ciphertext and your key material.
//!
//! # What the vault checks before accepting something
//!
//! **Segments:** the envelope signature, always. A host that stored unverified
//! envelopes would become a convenient way to flood a user's peers with garbage
//! attributed to one of their devices.
//!
//! **Chunks:** nothing, and it cannot. A sealed chunk is indistinguishable from
//! random bytes to anyone without the key, so a host has no way to tell a real
//! chunk from a forgery. This is not a gap that can be closed at this layer: the
//! *owner* detects the substitution when the chunk fails to open, which is
//! exactly where the check belongs. What the vault can do is refuse to accept
//! more than it agreed to store, which is a quota question rather than an
//! authenticity one.

use std::path::{Path, PathBuf};

use itsanas_crypto::{ChunkId, DeviceId, ObjectId, UserId};
use redb::{Database, ReadableTable, TableDefinition};

use crate::{
    blob::BlobStore,
    error::{Result, StoreError},
    oplog::SegmentEnvelope,
};

/// `owner ‖ device ‖ position` → postcard-encoded [`SegmentEnvelope`].
///
/// A composite big-endian key rather than a tuple, so a range scan over one
/// `owner ‖ device` prefix returns that device's chain in chain order.
const SEGMENTS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("vault_segments");
/// `owner ‖ device` → how many segments are held for it.
const CHAIN_LENGTHS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("vault_chain_lengths");

/// Bytes of log segment held per chain, so the pledge can count them.
///
/// # Why this exists, and what it was worth
///
/// `would_exceed_pledge` reads `Vault::stats().bytes`, and `bytes` was the sum
/// of the *chunk* blobs alone. Segments live in `vault_segments` and counted
/// for nothing, so `held` stayed at zero however many arrived: every
/// `StoreSegment` passed the quota on any host whose pledge exceeded one
/// segment, for ever.
///
/// A stranger needed no account, no invitation and no coordinator -- a
/// throwaway Ed25519 key completes the handshake, and a self-signed envelope
/// with a random body is indistinguishable from a real one because nobody can
/// decrypt either. Roughly 1,280 requests at just under the 8 MiB frame limit
/// puts 10 GiB on the disk, and there is no upper bound and no segment-removal
/// API. The bytes were also invisible in `itsanas status`, which reads the same
/// field, so the operator watched a disk fill with no cause.
///
/// A running total rather than a walk, for the same reason the chunk index is
/// one: this is read on the path of every stored object.
const CHAIN_BYTES: TableDefinition<'_, &[u8], u64> = TableDefinition::new("vault_chain_bytes");
/// `owner ‖ device` → the most recent segment id held.
const HEADS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("vault_heads");

/// Which chunks this vault holds, keyed by `owner ‖ chunk`.
///
/// The blob directories are the authority on what is on disk; this is an index
/// over them, and it exists for one reason: **order**. Reconciling with an owner
/// means hashing the ids this vault holds for them in ascending order, and the
/// only other way to get that list is `BlobStore::addresses`, a recursive walk
/// whose own documentation says it is never for a hot path. A host with a
/// million chunks would walk a million files to answer "have we both got the
/// same set", which is the question that exists to be cheap.
const CHUNKS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("vault_chunks");

/// Bytes in a `owner ‖ device` key.
const CHAIN_KEY_LEN: usize = 64;

fn chain_key(owner: UserId, device: DeviceId) -> Vec<u8> {
    let mut key = Vec::with_capacity(CHAIN_KEY_LEN);
    key.extend_from_slice(owner.as_bytes());
    key.extend_from_slice(device.as_bytes());
    key
}

fn segment_key(owner: UserId, device: DeviceId, position: u64) -> Vec<u8> {
    let mut key = chain_key(owner, device);
    // Big-endian so lexicographic byte order matches numeric order, which is
    // what makes a prefix range scan return the chain in the right sequence.
    key.extend_from_slice(&position.to_be_bytes());
    key
}

/// Sealed objects held on behalf of other users.
#[derive(Debug)]
pub struct Vault {
    root: PathBuf,
    db: Database,
}

/// What a vault is holding, for quota accounting and `itsanas status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VaultStats {
    pub owners: usize,
    pub chunks: usize,
    pub segments: u64,
    /// Every foreign byte on this disk: chunk blobs **and** log segments.
    ///
    /// This is what the pledge is measured against. Segments were missing from
    /// it, which made the quota unenforceable for half the objects a peer can
    /// send -- see `CHAIN_BYTES`.
    pub bytes: u64,
    /// The segment half of [`Self::bytes`], so an operator can see it.
    pub segment_bytes: u64,
}

/// `owner ‖ chunk`, so one owner's chunks are a contiguous, ordered range.
fn chunk_key(owner: UserId, chunk: &ChunkId) -> [u8; 64] {
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(owner.as_bytes().as_slice());
    out[32..].copy_from_slice(chunk.as_bytes().as_slice());
    out
}

fn chunk_range_start(owner: UserId) -> [u8; 64] {
    chunk_key(owner, &ChunkId::from_bytes([0x00; 32]))
}

fn chunk_range_end(owner: UserId) -> [u8; 64] {
    chunk_key(owner, &ChunkId::from_bytes([0xFF; 32]))
}

fn chunk_from_key(bytes: &[u8]) -> Option<ChunkId> {
    if bytes.len() != 64 {
        return None;
    }
    let mut chunk = [0u8; 32];
    chunk.copy_from_slice(&bytes[32..]);
    Some(ChunkId::from_bytes(chunk))
}

impl Vault {
    /// Open or create a vault at `root`.
    ///
    /// Takes no keys. There is deliberately no constructor that does.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_owned();
        std::fs::create_dir_all(&root).map_err(|error| StoreError::io(root.clone(), error))?;

        let db = Database::create(root.join("vault.redb"))?;
        let txn = db.begin_write()?;
        {
            let _ = txn.open_table(SEGMENTS)?;
            let _ = txn.open_table(CHAIN_LENGTHS)?;
            let _ = txn.open_table(CHAIN_BYTES)?;
            let _ = txn.open_table(HEADS)?;
            let _ = txn.open_table(CHUNKS)?;
        }
        txn.commit()?;

        let vault = Self { root, db };
        vault.backfill_chunks()?;
        vault.backfill_segment_bytes()?;
        Ok(vault)
    }

    /// Per-owner blob directory.
    ///
    /// Separate directories rather than one namespace keyed by owner, so that
    /// evicting a user who left, or auditing how much space one peer occupies,
    /// is a directory operation rather than a full scan.
    fn blobs_for(&self, owner: UserId) -> Result<BlobStore> {
        BlobStore::open(self.root.join("owners").join(owner.to_hex()))
    }

    // ---------------------------------------------------------------- chunks

    /// Accept a sealed chunk for storage.
    ///
    /// Returns whether it was newly stored. The bytes are opaque and are not
    /// validated — see the module docs for why that is not a gap at this layer.
    pub fn put_chunk(&self, owner: UserId, address: &ChunkId, sealed: &[u8]) -> Result<bool> {
        let stored = self.blobs_for(owner)?.put(address, sealed)?;
        // Indexed whether or not it was new: a blob present without its index
        // row is exactly the state that makes a reconciliation disagree for
        // ever, and re-inserting an existing key costs nothing.
        let txn = self.db.begin_write()?;
        {
            txn.open_table(CHUNKS)?
                .insert(chunk_key(owner, address).as_slice(), sealed.len() as u64)?;
        }
        txn.commit()?;
        Ok(stored)
    }

    /// Serve a sealed chunk.
    pub fn get_chunk(&self, owner: UserId, address: &ChunkId) -> Result<Option<Vec<u8>>> {
        self.blobs_for(owner)?.get(address)
    }

    /// Whether this vault holds a chunk.
    pub fn has_chunk(&self, owner: UserId, address: &ChunkId) -> Result<bool> {
        Ok(self.blobs_for(owner)?.contains(address))
    }

    /// Drop a chunk.
    pub fn remove_chunk(&self, owner: UserId, address: &ChunkId) -> Result<bool> {
        let removed = self.blobs_for(owner)?.remove(address)?;
        let txn = self.db.begin_write()?;
        {
            txn.open_table(CHUNKS)?
                .remove(chunk_key(owner, address).as_slice())?;
        }
        txn.commit()?;
        Ok(removed)
    }

    /// Every chunk address held for one owner, in ascending order.
    ///
    /// From the index rather than the directory, so it is a range scan instead
    /// of a recursive walk, and sorted — which is what
    /// [`crate::summary`] requires of both sides.
    pub fn chunks_for(&self, owner: UserId) -> Result<Vec<ChunkId>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CHUNKS)?;

        let mut out = Vec::new();
        for row in
            table.range(chunk_range_start(owner).as_slice()..=chunk_range_end(owner).as_slice())?
        {
            let (key, _) = row?;
            if let Some(chunk) = chunk_from_key(key.value()) {
                out.push(chunk);
            }
        }
        Ok(out)
    }

    /// A summary of what this vault holds for one owner.
    ///
    /// The answer to "have we both got the same set", in one hash. See
    /// [`crate::summary`].
    pub fn chunk_summary(&self, owner: UserId) -> Result<Vec<crate::summary::Digest>> {
        // Streamed, for the same reason the owner's side is: a host holding a
        // terabyte for somebody would otherwise build a `Vec` of sixteen
        // million ids to answer one question about them.
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CHUNKS)?;
        let range =
            table.range(chunk_range_start(owner).as_slice()..=chunk_range_end(owner).as_slice())?;

        let mut failed: Option<StoreError> = None;
        let digests = crate::summary::buckets(range.filter_map(|row| match row {
            Ok((key, _)) => chunk_from_key(key.value()),
            Err(error) => {
                failed.get_or_insert(StoreError::from(error));
                None
            }
        }));

        match failed {
            Some(error) => Err(error),
            None => Ok(digests),
        }
    }

    /// Populate the chunk index from the directories, once.
    ///
    /// Vaults created before the index existed have blobs and no rows. Doing it
    /// at open is a single walk on the first start after an upgrade, which is
    /// the cheapest honest moment: the alternative is a vault that silently
    /// reports an empty set and makes every peer re-send everything it holds.
    fn backfill_chunks(&self) -> Result<()> {
        let txn = self.db.begin_read()?;
        let indexed = txn.open_table(CHUNKS)?.iter()?.next().is_some();
        drop(txn);
        if indexed {
            return Ok(());
        }

        for owner in self.owners()? {
            let addresses = self.blobs_for(owner)?.addresses()?;
            if addresses.is_empty() {
                continue;
            }
            let txn = self.db.begin_write()?;
            {
                let mut table = txn.open_table(CHUNKS)?;
                for address in addresses {
                    let size = self.blobs_for(owner)?.size_of(&address)?.unwrap_or(0);
                    table.insert(chunk_key(owner, &address).as_slice(), size)?;
                }
            }
            txn.commit()?;
        }
        Ok(())
    }

    /// Total up the segments a vault already holds, once.
    ///
    /// Same reasoning as `backfill_chunks`, and the same cost: one walk on the
    /// first start after an upgrade. Without it a vault that filled up before
    /// segments counted would report zero for them for ever, which is the bug
    /// this table exists to fix, preserved.
    fn backfill_segment_bytes(&self) -> Result<()> {
        let txn = self.db.begin_read()?;
        let done = txn.open_table(CHAIN_BYTES)?.iter()?.next().is_some();
        let empty = txn.open_table(SEGMENTS)?.iter()?.next().is_none();
        drop(txn);
        if done || empty {
            return Ok(());
        }

        let mut totals: std::collections::BTreeMap<Vec<u8>, u64> =
            std::collections::BTreeMap::new();
        {
            let txn = self.db.begin_read()?;
            let segments = txn.open_table(SEGMENTS)?;
            for row in segments.iter()? {
                let (key, value) = row?;
                let raw = key.value();
                // The segment key is the chain key with the index appended, so
                // the chain key is its prefix. Taken by length rather than by
                // parsing, because the shape is fixed by `segment_key`.
                if raw.len() < CHAIN_KEY_LEN {
                    continue;
                }
                let chain = raw[..CHAIN_KEY_LEN].to_vec();
                let bytes = value.value().len() as u64;
                *totals.entry(chain).or_default() += bytes;
            }
        }

        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(CHAIN_BYTES)?;
            for (chain, bytes) in totals {
                table.insert(chain.as_slice(), bytes)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    // -------------------------------------------------------------- segments

    /// Accept a segment for storage, after verifying its signature.
    ///
    /// Returns whether it was newly stored. Rejects a segment that does not
    /// continue the chain this vault already holds for that device, so a host
    /// cannot be induced to store a chain with a hole in it and then serve that
    /// hole to a peer as though it were complete.
    pub fn put_segment(&self, envelope: &SegmentEnvelope) -> Result<bool> {
        // Before anything is written. A host that stored unverified envelopes
        // would be a convenient way to attribute garbage to someone's device.
        envelope.verify_signature()?;

        let encoded = envelope.encode()?;
        let key = chain_key(envelope.owner, envelope.device);

        let txn = self.db.begin_write()?;
        let stored;

        {
            let mut segments = txn.open_table(SEGMENTS)?;
            let mut lengths = txn.open_table(CHAIN_LENGTHS)?;
            let mut chain_bytes = txn.open_table(CHAIN_BYTES)?;
            let mut heads = txn.open_table(HEADS)?;

            let length = lengths.get(key.as_slice())?.map_or(0, |v| v.value());
            let head = match heads.get(key.as_slice())? {
                Some(value) => Some(ObjectId::from_slice(value.value())?),
                None => None,
            };

            if head == Some(envelope.segment_id) {
                // Already the tip. Re-offering is normal, not an error.
                stored = false;
            } else if envelope.previous == head {
                segments.insert(
                    segment_key(envelope.owner, envelope.device, length).as_slice(),
                    encoded.as_slice(),
                )?;
                lengths.insert(key.as_slice(), length + 1)?;
                let so_far = chain_bytes.get(key.as_slice())?.map_or(0, |v| v.value());
                chain_bytes.insert(key.as_slice(), so_far.saturating_add(encoded.len() as u64))?;
                heads.insert(key.as_slice(), envelope.segment_id.as_bytes().as_slice())?;
                stored = true;
            } else {
                return Err(StoreError::SegmentChainBroken {
                    segment: envelope.segment_id.short(),
                    expected: head.map_or_else(|| "none".to_owned(), |id| id.short()),
                    found: envelope
                        .previous
                        .map_or_else(|| "none".to_owned(), |id| id.short()),
                });
            }
        }

        txn.commit()?;
        Ok(stored)
    }

    /// Segments held for one device's chain, oldest first.
    ///
    /// `after` resumes from just past a segment the caller already has; `limit`
    /// caps the response so one request cannot ask for an unbounded assembly.
    pub fn segments_for(
        &self,
        owner: UserId,
        device: DeviceId,
        after: Option<ObjectId>,
        limit: usize,
    ) -> Result<Vec<SegmentEnvelope>> {
        let prefix = chain_key(owner, device);
        let txn = self.db.begin_read()?;
        let table = txn.open_table(SEGMENTS)?;

        let start = segment_key(owner, device, 0);
        let end = segment_key(owner, device, u64::MAX);

        let mut out = Vec::new();
        let mut skipping = after.is_some();

        for row in table.range(start.as_slice()..=end.as_slice())? {
            let (key, value) = row?;
            if !key.value().starts_with(&prefix) {
                break;
            }

            let envelope = SegmentEnvelope::decode(value.value())?;

            if skipping {
                // Resume *after* the named segment, so the caller does not
                // receive one it already has on every round.
                if Some(envelope.segment_id) == after {
                    skipping = false;
                }
                continue;
            }

            out.push(envelope);
            if out.len() >= limit {
                break;
            }
        }

        // `after` naming a segment this vault does not hold means the caller is
        // ahead of us, or is talking about a different chain. Returning
        // everything would re-send history it already has; returning nothing is
        // the honest answer.
        if skipping {
            return Ok(Vec::new());
        }

        Ok(out)
    }

    /// Chain tips held for one owner, across all their devices.
    pub fn heads_for(&self, owner: UserId) -> Result<Vec<(DeviceId, ObjectId, u64)>> {
        let txn = self.db.begin_read()?;
        let heads = txn.open_table(HEADS)?;
        let lengths = txn.open_table(CHAIN_LENGTHS)?;

        let mut out = Vec::new();
        for row in heads.iter()? {
            let (key, value) = row?;
            let key = key.value();

            if key.len() != CHAIN_KEY_LEN || !key.starts_with(owner.as_bytes()) {
                continue;
            }

            let device = DeviceId::from_slice(&key[32..])?;
            let head = ObjectId::from_slice(value.value())?;
            let length = lengths.get(key)?.map_or(0, |v| v.value());

            out.push((device, head, length));
        }

        Ok(out)
    }

    /// Every owner this vault holds anything for.
    ///
    /// Unions two sources, and needs both. Deriving the list from the segment
    /// table alone misses a peer whose chunks are held but whose log is not —
    /// which is the *normal* state for a host, since chunk data is the bulk of
    /// what gets stored and a host may hold chunks for a device whose segments
    /// went to a different host entirely. Quota accounting built on the segment
    /// table alone reports zero bytes for such a peer and lets the disk fill.
    pub fn owners(&self) -> Result<Vec<UserId>> {
        let mut out: Vec<UserId> = Vec::new();

        let txn = self.db.begin_read()?;
        let heads = txn.open_table(HEADS)?;
        for row in heads.iter()? {
            let (key, _) = row?;
            let key = key.value();
            if key.len() != CHAIN_KEY_LEN {
                continue;
            }
            let owner = UserId::from_slice(&key[..32])?;
            if !out.contains(&owner) {
                out.push(owner);
            }
        }

        let directory = self.root.join("owners");
        match std::fs::read_dir(&directory) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| StoreError::io(directory.clone(), error))?;
                    // A directory whose name is not a user id was not created
                    // by us; ignoring it is right, because everything derived
                    // from this list eventually decides what to delete.
                    if let Some(name) = entry.file_name().to_str()
                        && let Ok(owner) = name.parse::<UserId>()
                        && !out.contains(&owner)
                    {
                        out.push(owner);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(StoreError::io(directory, error)),
        }

        out.sort_unstable();
        Ok(out)
    }

    /// What this vault holds for one owner.
    ///
    /// Worth separating from [`Self::stats`], because a vault legitimately
    /// holds two very different things: other people's data, which is the
    /// hosting bargain, and *your own account's* segments from your other
    /// devices, which is what lets this machine relay them onwards. Reporting
    /// the two as one number tells an operator they are hosting for a stranger
    /// when they are not.
    pub fn stats_for(&self, owner: UserId) -> Result<VaultStats> {
        let blobs = self.blobs_for(owner)?;
        let prefix = owner.as_bytes();

        let txn = self.db.begin_read()?;
        let lengths = txn.open_table(CHAIN_LENGTHS)?;

        let mut segments = 0u64;
        for row in lengths.iter()? {
            let (key, value) = row?;
            if key.value().starts_with(prefix) {
                segments = segments.saturating_add(value.value());
            }
        }

        let chain_bytes = txn.open_table(CHAIN_BYTES)?;
        let mut segment_bytes = 0u64;
        for row in chain_bytes.iter()? {
            let (key, value) = row?;
            if key.value().starts_with(prefix) {
                segment_bytes = segment_bytes.saturating_add(value.value());
            }
        }

        Ok(VaultStats {
            owners: 1,
            chunks: blobs.addresses()?.len(),
            segments,
            // Both halves, for the same reason as `stats`: this is what an
            // operator reads to answer "how much of my disk is theirs".
            bytes: blobs.total_bytes()?.saturating_add(segment_bytes),
            segment_bytes,
        })
    }

    /// What this vault is holding, in total.
    pub fn stats(&self) -> Result<VaultStats> {
        let owners = self.owners()?;
        let mut stats = VaultStats {
            owners: owners.len(),
            ..VaultStats::default()
        };

        for owner in &owners {
            let blobs = self.blobs_for(*owner)?;
            stats.chunks += blobs.addresses()?.len();
            stats.bytes = stats.bytes.saturating_add(blobs.total_bytes()?);
        }

        let txn = self.db.begin_read()?;
        let lengths = txn.open_table(CHAIN_LENGTHS)?;
        for row in lengths.iter()? {
            let (_, value) = row?;
            stats.segments = stats.segments.saturating_add(value.value());
        }

        // Segments are foreign data on this disk exactly as chunks are, and
        // `bytes` is what the pledge is measured against. Leaving them out made
        // the quota unenforceable for half the objects a peer can send.
        let chain_bytes = txn.open_table(CHAIN_BYTES)?;
        for row in chain_bytes.iter()? {
            let (_, value) = row?;
            stats.segment_bytes = stats.segment_bytes.saturating_add(value.value());
            stats.bytes = stats.bytes.saturating_add(value.value());
        }

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

    use crate::oplog::{FileEntry, LogEntry, Operation};

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open");
        (dir, vault)
    }

    fn keys(byte: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([byte; 32]))
    }

    fn device(byte: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&SecretBytes::new([byte; 32]))
    }

    fn entry(sequence: u64) -> LogEntry {
        LogEntry {
            sequence,
            recorded_unix: 1_700_000_000 + sequence,
            operation: Operation::Upsert {
                path: format!("file-{sequence}.txt"),
                entry: FileEntry {
                    size: 1,
                    modified_unix: 1_700_000_000,
                    content_hash: [0; 32],
                    chunks: Vec::new(),
                    version: crate::version::VersionVector::new(),
                    author: DeviceId::from_bytes([1; 32]),
                },
            },
        }
    }

    fn segment(
        user: &UserKeys,
        dev: &DeviceKeys,
        previous: Option<ObjectId>,
        sequence: u64,
    ) -> SegmentEnvelope {
        SegmentEnvelope::create(
            user.oplog_root(),
            user.user_id(),
            dev,
            previous,
            vec![entry(sequence)],
        )
        .expect("segment")
    }

    #[test]
    fn a_chunk_round_trips_without_the_vault_ever_holding_a_key() {
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        let address = ChunkId::from_bytes([9; 32]);

        assert!(vault.put_chunk(owner, &address, b"sealed bytes").unwrap());
        assert_eq!(
            vault.get_chunk(owner, &address).unwrap().unwrap(),
            b"sealed bytes"
        );
        assert!(vault.has_chunk(owner, &address).unwrap());
    }

    #[test]
    fn two_owners_chunks_do_not_collide_even_at_the_same_address() {
        // Chunk ids are blinded per user, so a collision across owners should
        // not happen — but the vault must not depend on that for correctness.
        let (_dir, vault) = vault();
        let alice = keys(1).user_id();
        let bob = keys(2).user_id();
        let address = ChunkId::from_bytes([7; 32]);

        vault.put_chunk(alice, &address, b"alice's bytes").unwrap();
        vault.put_chunk(bob, &address, b"bob's bytes").unwrap();

        assert_eq!(
            vault.get_chunk(alice, &address).unwrap().unwrap(),
            b"alice's bytes"
        );
        assert_eq!(
            vault.get_chunk(bob, &address).unwrap().unwrap(),
            b"bob's bytes",
            "one owner's chunk overwrote another's at the same address"
        );
    }

    #[test]
    fn a_segment_with_a_bad_signature_is_refused_before_it_is_stored() {
        // A host that stored unverified envelopes becomes a way to flood a
        // user's peers with garbage attributed to one of their devices.
        let (_dir, vault) = vault();
        let user = keys(3);
        let dev = device(3);

        let mut forged = segment(&user, &dev, None, 1);
        forged.sealed_body[0] ^= 0xFF;

        assert!(
            vault.put_segment(&forged).is_err(),
            "a segment with a broken signature was accepted for storage"
        );
        assert!(vault.heads_for(user.user_id()).unwrap().is_empty());
    }

    #[test]
    fn a_chain_is_stored_and_served_in_order() {
        let (_dir, vault) = vault();
        let user = keys(4);
        let dev = device(4);

        let first = segment(&user, &dev, None, 1);
        let second = segment(&user, &dev, Some(first.segment_id), 2);
        let third = segment(&user, &dev, Some(second.segment_id), 3);

        for envelope in [&first, &second, &third] {
            assert!(vault.put_segment(envelope).unwrap());
        }

        let served = vault
            .segments_for(user.user_id(), dev.device_id(), None, 10)
            .unwrap();

        assert_eq!(served.len(), 3);
        assert_eq!(served[0].segment_id, first.segment_id);
        assert_eq!(served[1].segment_id, second.segment_id);
        assert_eq!(served[2].segment_id, third.segment_id);
        crate::oplog::validate_chain(&served).expect("the served chain must validate");
    }

    #[test]
    fn a_segment_that_does_not_continue_the_chain_is_refused() {
        // Otherwise a host can be induced to store a chain with a hole and then
        // serve that hole to a peer as though it were complete.
        let (_dir, vault) = vault();
        let user = keys(5);
        let dev = device(5);

        let first = segment(&user, &dev, None, 1);
        let second = segment(&user, &dev, Some(first.segment_id), 2);
        let third = segment(&user, &dev, Some(second.segment_id), 3);

        vault.put_segment(&first).unwrap();

        assert!(
            vault.put_segment(&third).is_err(),
            "a segment was stored on top of a chain it does not continue"
        );
        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn re_offering_the_current_tip_is_accepted_as_a_no_op() {
        // Peers re-offer freely; there is no acknowledgement telling them to
        // stop. This must not be an error, and must not duplicate the segment.
        let (_dir, vault) = vault();
        let user = keys(6);
        let dev = device(6);
        let first = segment(&user, &dev, None, 1);

        assert!(vault.put_segment(&first).unwrap());
        assert!(!vault.put_segment(&first).unwrap());

        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .len(),
            1,
            "re-offering the tip duplicated it"
        );
    }

    #[test]
    fn resuming_after_a_segment_skips_what_the_caller_already_has() {
        let (_dir, vault) = vault();
        let user = keys(7);
        let dev = device(7);

        let first = segment(&user, &dev, None, 1);
        let second = segment(&user, &dev, Some(first.segment_id), 2);
        let third = segment(&user, &dev, Some(second.segment_id), 3);
        for envelope in [&first, &second, &third] {
            vault.put_segment(envelope).unwrap();
        }

        let served = vault
            .segments_for(user.user_id(), dev.device_id(), Some(first.segment_id), 10)
            .unwrap();

        assert_eq!(served.len(), 2);
        assert_eq!(served[0].segment_id, second.segment_id);

        // Resuming after the tip yields nothing.
        assert!(
            vault
                .segments_for(user.user_id(), dev.device_id(), Some(third.segment_id), 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_limit_caps_the_response() {
        let (_dir, vault) = vault();
        let user = keys(8);
        let dev = device(8);

        let mut previous = None;
        for sequence in 1..=5 {
            let envelope = segment(&user, &dev, previous, sequence);
            vault.put_segment(&envelope).unwrap();
            previous = Some(envelope.segment_id);
        }

        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 2)
                .unwrap()
                .len(),
            2,
            "the limit was ignored, so one request can ask for unbounded work"
        );
    }

    #[test]
    fn resuming_after_an_unknown_segment_returns_nothing_rather_than_everything() {
        let (_dir, vault) = vault();
        let user = keys(9);
        let dev = device(9);
        vault.put_segment(&segment(&user, &dev, None, 1)).unwrap();

        let served = vault
            .segments_for(
                user.user_id(),
                dev.device_id(),
                Some(ObjectId::from_bytes([0xEE; 32])),
                10,
            )
            .unwrap();

        assert!(
            served.is_empty(),
            "an unknown resume point caused the whole chain to be re-sent"
        );
    }

    #[test]
    fn heads_are_reported_per_device_and_scoped_to_one_owner() {
        let (_dir, vault) = vault();
        let alice = keys(10);
        let bob = keys(11);
        let alice_laptop = device(10);
        let alice_pi = device(11);
        let bob_vm = device(12);

        let a1 = segment(&alice, &alice_laptop, None, 1);
        let a2 = segment(&alice, &alice_laptop, Some(a1.segment_id), 2);
        let p1 = segment(&alice, &alice_pi, None, 1);
        let b1 = segment(&bob, &bob_vm, None, 1);

        for envelope in [&a1, &a2, &p1, &b1] {
            vault.put_segment(envelope).unwrap();
        }

        let mut heads = vault.heads_for(alice.user_id()).unwrap();
        heads.sort_by_key(|(device, _, _)| device.to_bytes());

        assert_eq!(heads.len(), 2, "expected one head per Alice device");
        assert!(
            heads
                .iter()
                .all(|(device, _, _)| *device == alice_laptop.device_id()
                    || *device == alice_pi.device_id()),
            "Bob's device appeared in Alice's heads"
        );

        let laptop = heads
            .iter()
            .find(|(device, _, _)| *device == alice_laptop.device_id())
            .unwrap();
        assert_eq!(laptop.1, a2.segment_id);
        assert_eq!(laptop.2, 2);
    }

    #[test]
    fn one_owners_segments_are_never_served_under_another_owners_name() {
        let (_dir, vault) = vault();
        let alice = keys(12);
        let bob = keys(13);
        let dev = device(13);

        vault.put_segment(&segment(&alice, &dev, None, 1)).unwrap();

        assert!(
            vault
                .segments_for(bob.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .is_empty(),
            "Alice's segments were served as Bob's"
        );
        assert!(vault.heads_for(bob.user_id()).unwrap().is_empty());
    }

    #[test]
    fn an_owner_whose_chunks_are_held_but_whose_log_is_not_still_counts() {
        // The normal state for a host: chunk data is the bulk of what gets
        // stored, and the segments for that device may have gone elsewhere.
        // Counting only segment-bearing owners reports zero bytes here and the
        // quota check that depends on it lets the disk fill.
        let (_dir, vault) = vault();
        let guest = keys(20).user_id();

        vault
            .put_chunk(guest, &ChunkId::from_bytes([1; 32]), &[0u8; 900])
            .unwrap();

        assert_eq!(vault.owners().unwrap(), vec![guest]);

        let stats = vault.stats().unwrap();
        assert_eq!(stats.owners, 1);
        assert_eq!(stats.chunks, 1);
        assert_eq!(stats.segments, 0);
        assert_eq!(
            stats.bytes, 900,
            "a chunk-only owner was invisible to accounting"
        );
    }

    #[test]
    fn per_owner_stats_separate_hosting_from_relaying() {
        // A vault holds two different things: other people's data, and your own
        // account's segments from your other devices. Reporting them as one
        // number tells an operator they are hosting for a stranger when they
        // are not.
        let (_dir, vault) = vault();
        let mine = keys(21);
        let stranger = keys(22);
        let dev = device(21);

        vault.put_segment(&segment(&mine, &dev, None, 1)).unwrap();
        vault
            .put_chunk(
                stranger.user_id(),
                &ChunkId::from_bytes([1; 32]),
                &[0u8; 500],
            )
            .unwrap();

        let own = vault.stats_for(mine.user_id()).unwrap();
        assert_eq!(own.segments, 1);
        assert_eq!(
            own.bytes - own.segment_bytes,
            0,
            "no foreign chunks are held for my own account"
        );
        // The segment relayed for my own account does weigh something, and it
        // now says so. It used to report zero, which is how a disk filled with
        // segments read as an empty vault.
        assert!(own.segment_bytes > 0, "a relayed segment weighed nothing");

        let theirs = vault.stats_for(stranger.user_id()).unwrap();
        assert_eq!(theirs.bytes, 500);
        assert_eq!(theirs.segment_bytes, 0);
        assert_eq!(theirs.segments, 0);

        let total = vault.stats().unwrap();
        assert_eq!(total.owners, 2);
        assert_eq!(total.bytes - total.segment_bytes, 500, "the chunk half");
        assert_eq!(total.segments, 1);
    }

    #[test]
    fn stats_account_for_every_owner() {
        let (_dir, vault) = vault();
        let alice = keys(14);
        let bob = keys(15);
        let dev = device(14);

        vault.put_segment(&segment(&alice, &dev, None, 1)).unwrap();
        vault.put_segment(&segment(&bob, &dev, None, 1)).unwrap();
        vault
            .put_chunk(alice.user_id(), &ChunkId::from_bytes([1; 32]), &[0u8; 100])
            .unwrap();
        vault
            .put_chunk(bob.user_id(), &ChunkId::from_bytes([2; 32]), &[0u8; 250])
            .unwrap();

        let stats = vault.stats().unwrap();
        assert_eq!(stats.owners, 2);
        assert_eq!(stats.chunks, 2);
        assert_eq!(stats.segments, 2);
        // `bytes` is every foreign byte, chunks *and* segments. It used to be
        // the chunk half alone, which is what let a stranger fill a host's disk
        // with segments while the pledge check read zero for ever.
        assert_eq!(stats.bytes - stats.segment_bytes, 350, "the chunk half");
        assert!(stats.segment_bytes > 0, "two segments weighed nothing");
    }

    #[test]
    fn red_team_segments_count_against_the_pledge_like_any_other_foreign_byte() {
        // The hole: `would_exceed_pledge` reads `stats().bytes`, and `bytes`
        // summed the chunk blobs alone. Segments live in their own table and
        // counted for nothing, so `held` stayed at zero however many arrived --
        // every `StoreSegment` passed the quota, for ever, on any host whose
        // pledge exceeded one segment.
        //
        // It needed no account and no invitation: a throwaway device key
        // completes the handshake, and a self-signed envelope full of random
        // bytes is indistinguishable from a real one because nobody can decrypt
        // either. There is no segment-removal API and redb does not shrink, so
        // the space was not reclaimable even by an operator who noticed -- and
        // they would not have, because `itsanas status` reads the same field.
        let (_dir, vault) = vault();
        let user = keys(31);
        let dev = device(31);

        let before = vault.stats().unwrap();
        assert_eq!(before.segment_bytes, 0);

        vault.put_segment(&segment(&user, &dev, None, 1)).unwrap();
        let after = vault.stats().unwrap();
        assert!(
            after.segment_bytes > 0,
            "a stored segment weighed nothing against the pledge"
        );
        assert_eq!(
            after.bytes, after.segment_bytes,
            "the vault holds no chunks, so every byte here is segment"
        );

        // And it accumulates rather than being overwritten by the next link.
        let first = after.segment_bytes;
        let head = vault.heads_for(user.user_id()).unwrap()[0].1;
        let second = segment(&user, &dev, Some(head), 2);
        vault.put_segment(&second).unwrap();
        assert!(
            vault.stats().unwrap().segment_bytes > first,
            "a second segment did not add to the total"
        );
    }

    #[test]
    fn everything_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let user = keys(16);
        let dev = device(16);
        let first = segment(&user, &dev, None, 1);

        {
            let vault = Vault::open(dir.path()).unwrap();
            vault.put_segment(&first).unwrap();
            vault
                .put_chunk(user.user_id(), &ChunkId::from_bytes([3; 32]), b"bytes")
                .unwrap();
        }

        let vault = Vault::open(dir.path()).unwrap();
        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            vault
                .get_chunk(user.user_id(), &ChunkId::from_bytes([3; 32]))
                .unwrap()
                .unwrap(),
            b"bytes"
        );
    }
}
