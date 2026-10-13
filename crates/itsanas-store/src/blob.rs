//! Content-addressed storage for sealed objects.
//!
//! Everything this module writes to disk is already ciphertext: it is handed
//! sealed bytes and an address, and it never sees a key. That is deliberate —
//! it means the same code stores *your* chunks and the chunks you host for
//! other people, and there is no code path where hosting could accidentally
//! decrypt.
//!
//! # Layout
//!
//! ```text
//! <root>/blobs/ab/cd/abcd…ef.blob
//! ```
//!
//! Two levels of 256-way fan-out from the hex address. A flat directory with a
//! million entries is pathological on ext4 and worse on NTFS; this keeps any
//! one directory to a few thousand files at the scale we care about.
//!
//! # Durability
//!
//! Writes go to a temporary file in `<root>/tmp`, are flushed, and are then
//! renamed into place. A crash mid-write therefore leaves a stray temp file,
//! never a truncated blob that would later fail to authenticate and be
//! misreported as a malicious host.

use std::{
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
};

use itsanas_crypto::ChunkId;

use crate::error::{Result, StoreError};

/// File extension for a sealed blob.
const BLOB_EXTENSION: &str = "blob";

/// Content-addressed store of sealed bytes.
#[derive(Debug, Clone)]
pub struct BlobStore {
    blobs: PathBuf,
    tmp: PathBuf,
    /// Set when this store keeps its chunks in pack files ([`crate::pack`]).
    packs: Option<std::sync::Arc<crate::pack::Packs>>,
}

/// Set to `1` to start *new* stores in pack files (M10, experimental). A
/// store that already has packs keeps them, one that has blob files keeps
/// those: nothing is converted.
pub const PACKS_ENV: &str = "ITSANAS_PACKS";

impl BlobStore {
    /// Open (creating if needed) a blob store rooted at `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let blobs = root.join("blobs");
        let tmp = root.join("tmp");
        let fresh = fs::read_dir(&blobs).map_or(true, |mut entries| entries.next().is_none());
        let packed = crate::pack::Packs::present(root)
            || (fresh && std::env::var(PACKS_ENV).is_ok_and(|v| v == "1"));
        if packed {
            return Self::open_packed(root);
        }

        for directory in [&blobs, &tmp] {
            fs::create_dir_all(directory)
                .map_err(|error| StoreError::io(directory.clone(), error))?;
        }

        Ok(Self {
            blobs,
            tmp,
            packs: None,
        })
    }

    /// Open (creating if needed) a store rooted at `root` in pack files.
    pub fn open_packed(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        Ok(Self {
            blobs: root.join("blobs"),
            tmp: root.join("tmp"),
            packs: Some(crate::pack::Packs::open(root)?),
        })
    }

    /// Move a store in blob files at `root` into pack files; returns how many
    /// chunks moved. The store must be closed (no daemon on it).
    ///
    /// Crash-safe by staging: every blob is copied into packs under
    /// `root/converting`, which nothing opens; only when all are there is
    /// that directory renamed to `root/packs`, the one step that switches
    /// the layout, and only after it are the blob files removed. A crash
    /// before the rename leaves the blob store whole (the staging is
    /// started over); after it, the packs are whole and the leftover blob
    /// files are deleted by the next call. A store already in packs is left
    /// as it is.
    pub fn convert_to_packs(root: impl AsRef<Path>) -> Result<u64> {
        let root = root.as_ref();
        let blobs = Self::open(root)?;
        let moved = if blobs.packs.is_some() {
            0
        } else {
            let staging = root.join("converting");
            if staging.exists() {
                fs::remove_dir_all(&staging)
                    .map_err(|error| StoreError::io(staging.clone(), error))?;
            }
            let mut moved = 0u64;
            {
                let target = crate::pack::Packs::open(&staging)?;
                blobs.for_each_address(&mut |address| {
                    if let Some(sealed) = blobs.get(&address)? {
                        target.put(&address, &sealed)?;
                        moved += 1;
                    }
                    Ok(())
                })?;
                target.flush()?;
            }
            let from = staging.join("packs");
            let to = root.join("packs");
            fs::rename(&from, &to).map_err(|error| StoreError::io(to.clone(), error))?;
            let _ = fs::remove_dir_all(&staging);
            moved
        };
        // The switch is made: blob files left behind, from this call or from
        // one a crash cut short after its rename, are no longer read.
        let files = root.join("blobs");
        if files.exists() && crate::pack::Packs::present(root) {
            fs::remove_dir_all(&files).map_err(|error| StoreError::io(files.clone(), error))?;
            let tmp = root.join("tmp");
            let _ = fs::remove_dir_all(tmp);
        }
        Ok(moved)
    }

    /// The pack files behind this store, when it has them.
    #[must_use]
    pub fn packs(&self) -> Option<&crate::pack::Packs> {
        self.packs.as_deref()
    }

    /// Give back the space of mostly dead packs; 0 for blob files.
    pub fn compact(&self) -> Result<u64> {
        self.packs.as_ref().map_or(Ok(0), |packs| packs.compact())
    }

    /// Path a given address maps to.
    fn path_for(&self, address: &ChunkId) -> PathBuf {
        let hex = address.to_hex();
        self.blobs
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(format!("{hex}.{BLOB_EXTENSION}"))
    }

    /// Lose `address` the way a disk does, behind the store's back: its blob
    /// file deleted, or its pack record overwritten. A test hook for every
    /// layout, where a file path only fits blob files.
    pub fn damage(&self, address: &ChunkId) -> Result<()> {
        if let Some(packs) = &self.packs {
            return packs.damage(address);
        }
        let path = self.path_for(address);
        fs::remove_file(&path).map_err(|error| StoreError::io(path, error))
    }

    /// Whether this store already holds `address`.
    #[must_use]
    pub fn contains(&self, address: &ChunkId) -> bool {
        if let Some(packs) = &self.packs {
            return packs.contains(address);
        }
        self.path_for(address).is_file()
    }

    /// Store `sealed` under `address`.
    ///
    /// Returns `true` if the blob was newly written and `false` if it was
    /// already present. Sealing is deterministic, so re-storing identical
    /// content is a no-op rather than a rewrite — this is where deduplication
    /// actually pays off.
    pub fn put(&self, address: &ChunkId, sealed: &[u8]) -> Result<bool> {
        if let Some(packs) = &self.packs {
            return packs.put(address, sealed);
        }
        let destination = self.path_for(address);
        if destination.is_file() {
            return Ok(false);
        }

        let parent = destination
            .parent()
            .ok_or_else(|| StoreError::Corrupt("blob path has no parent".to_owned()))?;
        fs::create_dir_all(parent).map_err(|error| StoreError::io(parent.to_owned(), error))?;

        let staging = self.staging_path()?;
        {
            let mut file =
                File::create(&staging).map_err(|error| StoreError::io(staging.clone(), error))?;
            file.write_all(sealed)
                .map_err(|error| StoreError::io(staging.clone(), error))?;
            // Flush the contents before the rename publishes the name, so a
            // crash cannot expose a blob whose bytes never reached the disk.
            file.sync_all()
                .map_err(|error| StoreError::io(staging.clone(), error))?;
        }

        fs::rename(&staging, &destination).map_err(|error| {
            // Losing the temp file matters less than reporting the real error.
            let _ = fs::remove_file(&staging);
            StoreError::io(destination.clone(), error)
        })?;

        Ok(true)
    }

    /// Read the sealed bytes stored under `address`.
    pub fn get(&self, address: &ChunkId) -> Result<Option<Vec<u8>>> {
        if let Some(packs) = &self.packs {
            return packs.get(address);
        }
        let path = self.path_for(address);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(StoreError::io(path, error)),
        }
    }

    /// Remove `address`. Returns whether anything was removed.
    pub fn remove(&self, address: &ChunkId) -> Result<bool> {
        if let Some(packs) = &self.packs {
            return packs.remove(address);
        }
        let path = self.path_for(address);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(StoreError::io(path, error)),
        }
    }

    /// Number of bytes `address` occupies, without reading it.
    pub fn size_of(&self, address: &ChunkId) -> Result<Option<u64>> {
        if let Some(packs) = &self.packs {
            return packs.size_of(address);
        }
        let path = self.path_for(address);
        match fs::metadata(&path) {
            Ok(meta) => Ok(Some(meta.len())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(StoreError::io(path, error)),
        }
    }

    /// Every address this store holds.
    ///
    /// Walks the fan-out directories. This is a full scan and is only used by
    /// garbage collection and integrity checking, never on a hot path.
    pub fn addresses(&self) -> Result<Vec<ChunkId>> {
        let mut found = Vec::new();
        if let Some(packs) = &self.packs {
            packs.for_each_address(&mut |address| {
                found.push(address);
                Ok(())
            })?;
            return Ok(found);
        }
        Self::walk(&self.blobs, &mut found)?;
        Ok(found)
    }

    /// Every address this store holds, handed to `visit` one at a time:
    /// [`Self::addresses`] without holding them all, for a walk over a vault
    /// of millions (a terabyte in 64 KiB chunks is 16 million).
    pub fn for_each_address(&self, visit: &mut dyn FnMut(ChunkId) -> Result<()>) -> Result<()> {
        if let Some(packs) = &self.packs {
            return packs.for_each_address(visit);
        }
        Self::visit(&self.blobs, visit)
    }

    fn visit(directory: &Path, visit: &mut dyn FnMut(ChunkId) -> Result<()>) -> Result<()> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(StoreError::io(directory.to_owned(), error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| StoreError::io(directory.to_owned(), error))?;
            let path = entry.path();
            if path.is_dir() {
                Self::visit(&path, visit)?;
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some(BLOB_EXTENSION) {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(address) = stem.parse::<ChunkId>()
            {
                visit(address)?;
            }
        }
        Ok(())
    }

    fn walk(directory: &Path, out: &mut Vec<ChunkId>) -> Result<()> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(StoreError::io(directory.to_owned(), error)),
        };

        for entry in entries {
            let entry = entry.map_err(|error| StoreError::io(directory.to_owned(), error))?;
            let path = entry.path();

            if path.is_dir() {
                Self::walk(&path, out)?;
                continue;
            }

            if path.extension().and_then(|e| e.to_str()) != Some(BLOB_EXTENSION) {
                continue;
            }

            // A file whose name is not a valid address cannot have been written
            // by us. Skipping it is right: garbage collection must never delete
            // something it does not understand.
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(address) = stem.parse::<ChunkId>()
            {
                out.push(address);
            }
        }

        Ok(())
    }

    /// Total bytes held, across every blob.
    pub fn total_bytes(&self) -> Result<u64> {
        if let Some(packs) = &self.packs {
            return packs.total_bytes();
        }
        let mut total = 0u64;
        for address in self.addresses()? {
            total = total.saturating_add(self.size_of(&address)?.unwrap_or(0));
        }
        Ok(total)
    }

    /// Delete leftover staging files from a previous crash.
    ///
    /// Returns how many were removed. Called on store open: without it, a
    /// machine that loses power mid-write leaks a temp file on every restart.
    pub fn sweep_staging(&self) -> Result<usize> {
        if self.packs.is_some() {
            return Ok(0);
        }
        let entries = match fs::read_dir(&self.tmp) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(StoreError::io(self.tmp.clone(), error)),
        };

        let mut removed = 0;
        for entry in entries {
            let entry = entry.map_err(|error| StoreError::io(self.tmp.clone(), error))?;
            if entry.path().is_file() && fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn staging_path(&self) -> Result<PathBuf> {
        let mut suffix = [0u8; 16];
        getrandom::fill(&mut suffix).map_err(|error| {
            StoreError::Corrupt(format!("no entropy for staging name: {error}"))
        })?;

        let mut name = String::with_capacity(32 + 4);
        for byte in suffix {
            use std::fmt::Write as _;
            let _ = write!(name, "{byte:02x}");
        }
        name.push_str(".tmp");

        Ok(self.tmp.join(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    fn store() -> (tempfile::TempDir, BlobStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = BlobStore::open(dir.path()).expect("open");
        (dir, store)
    }

    #[test]
    fn round_trips_what_it_was_given() {
        let (_dir, store) = store();
        assert!(store.put(&address(1), b"sealed bytes").unwrap());
        assert_eq!(store.get(&address(1)).unwrap().unwrap(), b"sealed bytes");
    }

    #[test]
    fn a_missing_address_is_none_not_an_error() {
        let (_dir, store) = store();
        assert!(store.get(&address(2)).unwrap().is_none());
        assert!(!store.contains(&address(2)));
        assert!(store.size_of(&address(2)).unwrap().is_none());
    }

    #[test]
    fn storing_the_same_address_twice_writes_once() {
        let (_dir, store) = store();
        assert!(store.put(&address(3), b"payload").unwrap());
        assert!(
            !store.put(&address(3), b"payload").unwrap(),
            "the second put reported a fresh write; deduplication is not working"
        );
        assert_eq!(store.addresses().unwrap().len(), 1);
    }

    #[test]
    fn addresses_lists_everything_across_the_fan_out() {
        let (_dir, store) = store();
        // Spread across different first and second bytes so several fan-out
        // directories are exercised.
        let written: Vec<ChunkId> = (0u8..40).map(address).collect();
        for id in &written {
            store.put(id, b"x").unwrap();
        }

        let mut listed = store.addresses().unwrap();
        listed.sort_unstable();
        let mut expected = written.clone();
        expected.sort_unstable();

        assert_eq!(listed, expected);
    }

    #[test]
    fn removal_is_idempotent() {
        let (_dir, store) = store();
        store.put(&address(4), b"x").unwrap();
        assert!(store.remove(&address(4)).unwrap());
        assert!(!store.remove(&address(4)).unwrap());
        assert!(!store.contains(&address(4)));
    }

    #[test]
    fn a_blob_lands_at_a_sharded_path_not_a_flat_one() {
        let (dir, store) = store();
        let id = address(0xAB);
        store.put(&id, b"x").unwrap();

        let hex = id.to_hex();
        let expected = dir
            .path()
            .join("blobs")
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(format!("{hex}.blob"));

        assert!(
            expected.is_file(),
            "blob was not written to the sharded path; a flat directory will \
             degrade badly at a million chunks"
        );
    }

    #[test]
    fn no_staging_file_survives_a_successful_write() {
        let (dir, store) = store();
        store.put(&address(5), b"payload").unwrap();

        let leftovers: Vec<_> = fs::read_dir(dir.path().join("tmp"))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();

        assert!(
            leftovers.is_empty(),
            "a staging file was left behind, so every write leaks a temp file"
        );
    }

    #[test]
    fn sweeping_removes_crash_leftovers_but_not_blobs() {
        let (dir, store) = store();
        store.put(&address(6), b"keep me").unwrap();

        fs::write(dir.path().join("tmp").join("abandoned.tmp"), b"partial").unwrap();
        assert_eq!(store.sweep_staging().unwrap(), 1);

        assert_eq!(store.get(&address(6)).unwrap().unwrap(), b"keep me");
        assert_eq!(store.addresses().unwrap().len(), 1);
    }

    #[test]
    fn files_that_are_not_blobs_are_ignored_by_the_scan() {
        // Garbage collection deletes what the scan reports. If the scan ever
        // reported a foreign file, GC would delete a stranger's data.
        let (dir, store) = store();
        store.put(&address(7), b"x").unwrap();

        let stray = dir.path().join("blobs").join("aa");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("README.txt"), b"not ours").unwrap();
        fs::write(stray.join("not-an-address.blob"), b"not ours").unwrap();

        assert_eq!(store.addresses().unwrap(), vec![address(7)]);
    }

    #[test]
    fn total_bytes_counts_stored_bytes() {
        let (_dir, store) = store();
        store.put(&address(8), &[0u8; 100]).unwrap();
        store.put(&address(9), &[0u8; 250]).unwrap();
        assert_eq!(store.total_bytes().unwrap(), 350);
    }

    #[test]
    fn an_empty_blob_is_storable_and_distinguishable_from_a_missing_one() {
        let (_dir, store) = store();
        store.put(&address(10), b"").unwrap();
        assert_eq!(store.get(&address(10)).unwrap(), Some(Vec::new()));
        assert!(store.contains(&address(10)));
    }
}
