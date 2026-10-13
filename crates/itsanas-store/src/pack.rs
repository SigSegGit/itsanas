//! Pack files: sealed chunks appended into large files, found through an index.
//!
//! One file per chunk is 14.7 million files a terabyte, and creating a file is
//! the operation NTFS is slow at (ROADMAP M9, M10). Here a chunk is a record
//! appended to the current pack, and a redb index maps its address to
//! `(pack, offset, length)`. Removing a chunk drops its index row; the bytes
//! stay until [`Packs::compact`] rewrites a pack that is mostly dead.
//!
//! # Layout
//!
//! ```text
//! <root>/packs/index.redb
//! <root>/packs/00000001.pack   records: [address 32][length u32 LE][sealed bytes]
//! ```
//!
//! # Durability
//!
//! The record is appended and flushed to the disk *before* its index row is
//! committed, so a row never names bytes that are not there. A crash between
//! the two leaves bytes no row names: dead space, reclaimed by compaction, the
//! pack-file equivalent of the stray temp file the file layout leaves.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use itsanas_crypto::ChunkId;
use redb::{Database, ReadableTable as _, ReadableTableMetadata as _, TableDefinition};

use crate::error::{Result, StoreError};

/// Address -> pack (u32 LE), offset of the sealed bytes (u64 LE), length (u32 LE).
const CHUNKS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("pack_chunks");
/// (pack u32 BE, address) -> (): which chunks a pack holds, for compaction.
const BY_PACK: TableDefinition<'_, &[u8], ()> = TableDefinition::new("pack_members");
/// Pack -> bytes still named by a row (u64 LE).
const LIVE: TableDefinition<'_, u32, u64> = TableDefinition::new("pack_live");

/// `"mark"` -> (pack u32 LE, offset u64 LE): every record before it has its
/// row in a *durable* commit. Rows after it may have been committed without
/// waiting for the disk, and are rebuilt from the records at open.
const META: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("pack_meta");

/// A durable commit, and a new mark, every this many puts...
const DURABLE_EVERY_PUTS: u32 = 256;
/// ...or this many bytes, whichever comes first: bounds the replay at open.
const DURABLE_EVERY_BYTES: u64 = 64 << 20;

/// A pack is closed, and a new one started, past this size.
pub const PACK_TARGET: u64 = 256 << 20;

/// Record header: address and length.
const HEADER_LEN: usize = 32 + 4;
/// The same, as a file offset.
const HEADER: u64 = HEADER_LEN as u64;

/// One open index per directory in this process: redb refuses a second
/// `Database` on one file, and the vault opens a store per call.
fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<Packs>>> {
    static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Packs>>>> = OnceLock::new();
    OPEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Where one chunk's bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Place {
    pack: u32,
    offset: u64,
    len: u32,
}

impl Place {
    fn encode(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[..4].copy_from_slice(&self.pack.to_le_bytes());
        out[4..12].copy_from_slice(&self.offset.to_le_bytes());
        out[12..].copy_from_slice(&self.len.to_le_bytes());
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let field = |range: std::ops::Range<usize>| {
            bytes
                .get(range)
                .ok_or_else(|| StoreError::Corrupt("a pack index row is too short".to_owned()))
        };
        Ok(Self {
            pack: u32::from_le_bytes(field(0..4)?.try_into().unwrap_or_default()),
            offset: u64::from_le_bytes(field(4..12)?.try_into().unwrap_or_default()),
            len: u32::from_le_bytes(field(12..16)?.try_into().unwrap_or_default()),
        })
    }
}

fn member_key(pack: u32, address: &ChunkId) -> [u8; 36] {
    let mut key = [0u8; 36];
    key[..4].copy_from_slice(&pack.to_be_bytes());
    key[4..].copy_from_slice(address.as_bytes());
    key
}

/// The pack store of one directory.
#[derive(Debug)]
pub struct Packs {
    dir: PathBuf,
    db: Database,
    /// The pack being appended to, and its length; one writer at a time.
    current: Mutex<(u32, u64)>,
    /// Puts and bytes since the last durable commit.
    pending: Mutex<(u32, u64)>,
}

impl Packs {
    /// Open (creating if needed) the packs under `root/packs`, shared with any
    /// other open of the same directory in this process.
    pub fn open(root: &Path) -> Result<Arc<Self>> {
        let dir = root.join("packs");
        fs::create_dir_all(&dir).map_err(|error| StoreError::io(dir.clone(), error))?;
        let key = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        let mut open = registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(packs) = open.get(&key).and_then(Weak::upgrade) {
            return Ok(packs);
        }
        let db = Database::create(dir.join("index.redb"))?;
        let txn = db.begin_write()?;
        {
            let _ = txn.open_table(CHUNKS)?;
            let _ = txn.open_table(BY_PACK)?;
            let _ = txn.open_table(LIVE)?;
            let _ = txn.open_table(META)?;
        }
        txn.commit()?;
        let last = Self::pack_numbers(&dir)?.into_iter().max().unwrap_or(1);
        let len = fs::metadata(Self::pack_path_in(&dir, last)).map_or(0, |meta| meta.len());
        let packs = Arc::new(Self {
            dir,
            db,
            current: Mutex::new((last, len)),
            pending: Mutex::new((0, 0)),
        });
        packs.replay()?;
        open.retain(|_, weak| weak.strong_count() > 0);
        open.insert(key, Arc::downgrade(&packs));
        Ok(packs)
    }

    /// Whether `root` holds packs already.
    #[must_use]
    pub fn present(root: &Path) -> bool {
        root.join("packs").join("index.redb").is_file()
    }

    fn pack_path_in(dir: &Path, pack: u32) -> PathBuf {
        dir.join(format!("{pack:08}.pack"))
    }

    /// Where pack `pack` is on disk: public so a test can damage one.
    #[must_use]
    pub fn pack_path(&self, pack: u32) -> PathBuf {
        Self::pack_path_in(&self.dir, pack)
    }

    fn pack_numbers(dir: &Path) -> Result<Vec<u32>> {
        let mut found = Vec::new();
        for entry in fs::read_dir(dir).map_err(|error| StoreError::io(dir.to_owned(), error))? {
            let entry = entry.map_err(|error| StoreError::io(dir.to_owned(), error))?;
            let name = entry.file_name();
            if let Some(number) = name
                .to_str()
                .and_then(|name| name.strip_suffix(".pack"))
                .and_then(|stem| stem.parse::<u32>().ok())
            {
                found.push(number);
            }
        }
        Ok(found)
    }

    fn place(&self, address: &ChunkId) -> Result<Option<Place>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CHUNKS)?;
        table
            .get(address.as_bytes().as_slice())?
            .map(|row| Place::decode(row.value()))
            .transpose()
    }

    /// Whether `address` has a row whose record is there: its header read
    /// back, as the file layout stats the file. A row whose record is gone
    /// or names another address is dropped, as [`Self::get`] does, so the
    /// disk check and repair see a lost chunk as lost.
    #[must_use]
    pub fn contains(&self, address: &ChunkId) -> bool {
        let Ok(Some(place)) = self.place(address) else {
            return false;
        };
        let mut header = [0u8; HEADER_LEN];
        let read = File::open(self.pack_path(place.pack)).and_then(|mut file| {
            file.seek(SeekFrom::Start(place.offset - HEADER))?;
            file.read_exact(&mut header)
        });
        let whole = read.is_ok()
            && header[..32] == address.as_bytes()[..]
            && header[32..] == place.len.to_le_bytes();
        if !whole
            && read.as_ref().is_err_and(|e| {
                e.kind() != std::io::ErrorKind::UnexpectedEof
                    && e.kind() != std::io::ErrorKind::NotFound
            })
        {
            // An I/O failure is not evidence of loss: say no, keep the row.
            return false;
        }
        if !whole {
            let _ = self.remove(address);
        }
        whole
    }

    /// Damage `address`'s record the way a disk does, behind the index: a
    /// test hook, as [`crate::BlobStore::damage`] is for every layout.
    pub fn damage(&self, address: &ChunkId) -> Result<()> {
        let Some(place) = self.place(address)? else {
            return Ok(());
        };
        let path = self.pack_path(place.pack);
        let mut file = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|error| StoreError::io(path.clone(), error))?;
        file.seek(SeekFrom::Start(place.offset - HEADER))
            .and_then(|_| file.write_all(&[0u8; 32]))
            .map_err(|error| StoreError::io(path, error))
    }

    /// Append `sealed` under `address`; `false` when already present.
    pub fn put(&self, address: &ChunkId, sealed: &[u8]) -> Result<bool> {
        let len = u32::try_from(sealed.len())
            .map_err(|_| StoreError::Corrupt("a chunk too large for a pack record".to_owned()))?;
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Checked under the writer's lock, so two puts of one address cannot
        // both append.
        if self.place(address)?.is_some() {
            return Ok(false);
        }
        if current.1 >= PACK_TARGET {
            *current = (current.0 + 1, 0);
        }
        let (pack, start) = *current;
        let path = self.pack_path(pack);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| StoreError::io(path.clone(), error))?;
        // A torn earlier append may have left the file longer than we think:
        // write where the file ends, and say so in the row.
        let end = file
            .seek(SeekFrom::End(0))
            .map_err(|error| StoreError::io(path.clone(), error))?;
        // Bytes past what we wrote are a torn append: the replay must never
        // parse them, so this record's commit waits for the disk and moves
        // the mark past them.
        let torn = end > start;
        let start = end.max(start);
        let mut record = Vec::with_capacity(sealed.len() + HEADER_LEN);
        record.extend_from_slice(address.as_bytes());
        record.extend_from_slice(&len.to_le_bytes());
        record.extend_from_slice(sealed);
        file.write_all(&record)
            .and_then(|()| file.sync_data())
            .map_err(|error| StoreError::io(path.clone(), error))?;
        current.1 = start + record.len() as u64;

        let place = Place {
            pack,
            offset: start + HEADER,
            len,
        };
        // The record is on the disk; its row need not be yet. A row is only
        // waited for every so often, and the replay at open rebuilds those
        // a crash took: two waits per chunk made packs slower than files.
        let durable = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pending.0 += 1;
            pending.1 += u64::from(len);
            let due = torn || pending.0 >= DURABLE_EVERY_PUTS || pending.1 >= DURABLE_EVERY_BYTES;
            if due {
                *pending = (0, 0);
            }
            due
        };
        let mut txn = self.db.begin_write()?;
        if !durable {
            txn.set_durability(redb::Durability::None);
        }
        {
            Self::index_record(&txn, address, place)?;
            if durable {
                Self::write_mark(&txn, pack, current.1)?;
            }
        }
        txn.commit()?;
        Ok(true)
    }

    fn index_record(txn: &redb::WriteTransaction, address: &ChunkId, place: Place) -> Result<()> {
        txn.open_table(CHUNKS)?
            .insert(address.as_bytes().as_slice(), place.encode().as_slice())?;
        txn.open_table(BY_PACK)?
            .insert(member_key(place.pack, address).as_slice(), ())?;
        let mut live = txn.open_table(LIVE)?;
        let was = live.get(place.pack)?.map_or(0, |v| v.value());
        live.insert(place.pack, was + u64::from(place.len))?;
        Ok(())
    }

    fn write_mark(txn: &redb::WriteTransaction, pack: u32, offset: u64) -> Result<()> {
        let mut mark = pack.to_le_bytes().to_vec();
        mark.extend_from_slice(&offset.to_le_bytes());
        txn.open_table(META)?.insert("mark", mark.as_slice())?;
        Ok(())
    }

    /// Rebuild the rows of every whole record after the durable mark that has
    /// none: the ones a crash took with a commit that did not wait for the
    /// disk. A record cut short by the crash ends the replay of its pack.
    fn replay(&self) -> Result<()> {
        let (mark_pack, mark_offset) = {
            let txn = self.db.begin_read()?;
            let meta = txn.open_table(META)?;
            meta.get("mark")?.map_or((1, 0), |row| {
                let bytes = row.value();
                let pack = bytes
                    .get(..4)
                    .and_then(|b| b.try_into().ok())
                    .map_or(1, u32::from_le_bytes);
                let offset = bytes
                    .get(4..12)
                    .and_then(|b| b.try_into().ok())
                    .map_or(0, u64::from_le_bytes);
                (pack, offset)
            })
        };
        let mut packs: Vec<u32> = Self::pack_numbers(&self.dir)?
            .into_iter()
            .filter(|&pack| pack >= mark_pack)
            .collect();
        packs.sort_unstable();
        let txn = self.db.begin_write()?;
        let mut end = (mark_pack, mark_offset);
        for pack in packs {
            let path = self.pack_path(pack);
            let mut file =
                File::open(&path).map_err(|error| StoreError::io(path.clone(), error))?;
            let size = file
                .metadata()
                .map_err(|error| StoreError::io(path.clone(), error))?
                .len();
            let mut offset = if pack == mark_pack { mark_offset } else { 0 };
            while offset + HEADER <= size {
                let mut header = [0u8; HEADER_LEN];
                file.seek(SeekFrom::Start(offset))
                    .and_then(|_| file.read_exact(&mut header))
                    .map_err(|error| StoreError::io(path.clone(), error))?;
                let len = u32::from_le_bytes(header[32..].try_into().unwrap_or_default());
                if offset + HEADER + u64::from(len) > size {
                    // Cut short by the crash: nothing follows it, and the
                    // next append goes after it.
                    offset = size;
                    break;
                }
                let bytes: [u8; 32] = header[..32].try_into().unwrap_or_default();
                let address = ChunkId::from_bytes(bytes);
                let place = Place {
                    pack,
                    offset: offset + HEADER,
                    len,
                };
                let known = txn
                    .open_table(CHUNKS)?
                    .get(address.as_bytes().as_slice())?
                    .map(|row| Place::decode(row.value()))
                    .transpose()?;
                // Only a record with no row at all: one whose row points at
                // another copy (compaction moved it) keeps that row.
                if known.is_none() {
                    Self::index_record(&txn, &address, place)?;
                }
                offset += HEADER + u64::from(len);
            }
            // Whatever is left (a header cut short) is dead space: the mark
            // goes past it, so no replay ever parses it.
            let _ = offset;
            end = (pack, size);
        }
        Self::write_mark(&txn, end.0, end.1)?;
        txn.commit()?;
        Ok(())
    }

    /// Read the sealed bytes stored under `address`.
    ///
    /// A record that is not there to read -- cut short, or naming another
    /// address -- is treated as the file layout treats a lost blob file: its
    /// row is dropped and the chunk reads as missing, so repair fetches it
    /// from a holder and `put` can store it again. An error here would
    /// instead have left a row that `contains` believes and nothing ever
    /// replaces (Rodin, 2026-10-13). Other I/O errors stay errors.
    pub fn get(&self, address: &ChunkId) -> Result<Option<Vec<u8>>> {
        let Some(place) = self.place(address)? else {
            return Ok(None);
        };
        let path = self.pack_path(place.pack);
        let mut header = [0u8; HEADER_LEN];
        let mut bytes = vec![0u8; place.len as usize];
        let read = File::open(&path).and_then(|mut file| {
            file.seek(SeekFrom::Start(place.offset - HEADER))?;
            file.read_exact(&mut header)?;
            file.read_exact(&mut bytes)
        });
        match read {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::NotFound
                ) =>
            {
                self.remove(address)?;
                return Ok(None);
            }
            Err(error) => return Err(StoreError::io(path, error)),
        }
        // The record names its own address: a row pointing at the wrong bytes
        // (a damaged pack, a pack rewritten under it) is caught here rather
        // than handed on as somebody else's chunk.
        if header[..32] != address.as_bytes()[..] || header[32..] != place.len.to_le_bytes() {
            self.remove(address)?;
            return Ok(None);
        }
        Ok(Some(bytes))
    }

    /// Drop `address`'s row; its bytes become dead until compaction.
    ///
    /// Durable, and it moves the mark to the end of the pack: the replay at
    /// open must never cover a removed record, or it would bring it back.
    /// (A durable commit in redb also makes every earlier one durable, so
    /// every record before the new mark has its row on the disk.)
    pub fn remove(&self, address: &ChunkId) -> Result<bool> {
        let writer = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let txn = self.db.begin_write()?;
        Self::write_mark(&txn, writer.0, writer.1)?;
        *self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = (0, 0);
        let removed = {
            let mut chunks = txn.open_table(CHUNKS)?;
            let place = chunks
                .remove(address.as_bytes().as_slice())?
                .map(|row| Place::decode(row.value()))
                .transpose()?;
            if let Some(place) = place {
                txn.open_table(BY_PACK)?
                    .remove(member_key(place.pack, address).as_slice())?;
                let mut live = txn.open_table(LIVE)?;
                let was = live.get(place.pack)?.map_or(0, |v| v.value());
                live.insert(place.pack, was.saturating_sub(u64::from(place.len)))?;
                true
            } else {
                false
            }
        };
        txn.commit()?;
        Ok(removed)
    }

    /// Length of `address`'s sealed bytes.
    pub fn size_of(&self, address: &ChunkId) -> Result<Option<u64>> {
        Ok(self.place(address)?.map(|place| u64::from(place.len)))
    }

    /// Every address with a row, handed to `visit` one at a time.
    pub fn for_each_address(&self, visit: &mut dyn FnMut(ChunkId) -> Result<()>) -> Result<()> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CHUNKS)?;
        for row in table.iter()? {
            let (key, _) = row?;
            let bytes: [u8; 32] = key.value().try_into().map_err(|_| {
                StoreError::Corrupt("a pack index key is not an address".to_owned())
            })?;
            visit(ChunkId::from_bytes(bytes))?;
        }
        Ok(())
    }

    /// Bytes named by a row, across every pack.
    pub fn total_bytes(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let live = txn.open_table(LIVE)?;
        let mut total = 0u64;
        for row in live.iter()? {
            total = total.saturating_add(row?.1.value());
        }
        Ok(total)
    }

    /// How many chunks have a row.
    pub fn count(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        Ok(txn.open_table(CHUNKS)?.len()?)
    }

    /// Rewrite every closed pack less than half of which is still named by a
    /// row: its live records are appended to the current pack, then the file
    /// goes. Returns the bytes given back to the disk.
    ///
    /// Crash-safe in that order: each record is copied, then its row moved to
    /// the copy in one durable commit, and only then is the old file removed.
    /// At every point the row names bytes that exist. (The first version
    /// removed the row, then put the copy: a crash between the two lost the
    /// chunk, the old record being before the mark the replay starts from.)
    pub fn compact(&self) -> Result<u64> {
        let current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
        let mut freed = 0u64;
        for pack in Self::pack_numbers(&self.dir)? {
            if pack >= current {
                continue;
            }
            let path = self.pack_path(pack);
            let size = fs::metadata(&path).map_or(0, |meta| meta.len());
            let live = {
                let txn = self.db.begin_read()?;
                txn.open_table(LIVE)?.get(pack)?.map_or(0, |v| v.value())
            };
            if live.saturating_mul(2) >= size {
                continue;
            }
            for address in self.members(pack)? {
                // A record that no longer reads stays where it is (its pack
                // with it): one damaged chunk must not stop every later
                // compaction, and moving it would only copy the damage.
                let Ok(Some(bytes)) = self.get(&address) else {
                    continue;
                };
                self.relocate(&address, pack, &bytes)?;
            }
            if self.members(pack)?.is_empty() {
                fs::remove_file(&path).map_err(|error| StoreError::io(path.clone(), error))?;
                let txn = self.db.begin_write()?;
                txn.open_table(LIVE)?.remove(pack)?;
                txn.commit()?;
                freed = freed.saturating_add(size);
            }
        }
        Ok(freed)
    }

    /// Append a copy of `address`'s record and point its row at the copy, in
    /// one durable commit. A crash before the commit leaves the old row and
    /// an unnamed copy, which the replay skips (the address has a row).
    fn relocate(&self, address: &ChunkId, from: u32, sealed: &[u8]) -> Result<()> {
        let len = u32::try_from(sealed.len())
            .map_err(|_| StoreError::Corrupt("a chunk too large for a pack record".to_owned()))?;
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.1 >= PACK_TARGET {
            *current = (current.0 + 1, 0);
        }
        let (pack, start) = *current;
        let path = self.pack_path(pack);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| StoreError::io(path.clone(), error))?;
        let start = file
            .seek(SeekFrom::End(0))
            .map_err(|error| StoreError::io(path.clone(), error))?
            .max(start);
        let mut record = Vec::with_capacity(sealed.len() + HEADER_LEN);
        record.extend_from_slice(address.as_bytes());
        record.extend_from_slice(&len.to_le_bytes());
        record.extend_from_slice(sealed);
        file.write_all(&record)
            .and_then(|()| file.sync_data())
            .map_err(|error| StoreError::io(path.clone(), error))?;
        current.1 = start + record.len() as u64;
        let txn = self.db.begin_write()?;
        {
            let mut members = txn.open_table(BY_PACK)?;
            members.remove(member_key(from, address).as_slice())?;
            drop(members);
            let mut live = txn.open_table(LIVE)?;
            let was = live.get(from)?.map_or(0, |v| v.value());
            live.insert(from, was.saturating_sub(u64::from(len)))?;
            drop(live);
            Self::index_record(
                &txn,
                address,
                Place {
                    pack,
                    offset: start + HEADER,
                    len,
                },
            )?;
            Self::write_mark(&txn, pack, current.1)?;
        }
        txn.commit()?;
        *self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = (0, 0);
        Ok(())
    }

    fn members(&self, pack: u32) -> Result<Vec<ChunkId>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(BY_PACK)?;
        let start = member_key(pack, &ChunkId::from_bytes([0; 32]));
        let end = member_key(pack, &ChunkId::from_bytes([0xff; 32]));
        let mut out = Vec::new();
        for row in table.range(start.as_slice()..=end.as_slice())? {
            let (key, ()) = {
                let (key, value) = row?;
                (key, value.value())
            };
            let bytes: [u8; 32] = key.value()[4..]
                .try_into()
                .map_err(|_| StoreError::Corrupt("a pack member key is short".to_owned()))?;
            out.push(ChunkId::from_bytes(bytes));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    #[test]
    fn chunks_round_trip_deduplicate_and_survive_a_reopen() {
        let dir = tempfile::tempdir().expect("temp dir");
        {
            let packs = Packs::open(dir.path()).expect("open");
            assert!(packs.put(&address(1), b"one").unwrap());
            assert!(packs.put(&address(2), b"two two").unwrap());
            assert!(
                !packs.put(&address(1), b"one").unwrap(),
                "a second copy was appended"
            );
            assert_eq!(packs.total_bytes().unwrap(), 10);
            assert!(packs.remove(&address(2)).unwrap());
            assert!(!packs.contains(&address(2)));
        }
        let packs = Packs::open(dir.path()).expect("reopen");
        assert_eq!(
            packs.get(&address(1)).unwrap().as_deref(),
            Some(&b"one"[..])
        );
        assert_eq!(packs.get(&address(2)).unwrap(), None);
        assert_eq!(packs.count().unwrap(), 1);
    }

    #[test]
    fn red_team_a_torn_append_is_dead_space_not_a_shifted_pack() {
        // A crash mid-append leaves bytes no row names. The next record must
        // land after them, at the offset its row records: a store that
        // trusted its own idea of the length would point every later row at
        // the torn bytes.
        // A failed write in this process (a full disk) is the same: the file
        // is longer than the length kept in memory.
        let dir = tempfile::tempdir().expect("temp dir");
        let packs = Packs::open(dir.path()).expect("open");
        packs.put(&address(1), b"before").unwrap();
        let path = dir.path().join("packs").join("00000001.pack");
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"half a record that never got its row")
            .unwrap();
        drop(file);
        packs.put(&address(2), b"after").unwrap();
        assert_eq!(
            packs.get(&address(1)).unwrap().as_deref(),
            Some(&b"before"[..])
        );
        assert_eq!(
            packs.get(&address(2)).unwrap().as_deref(),
            Some(&b"after"[..]),
            "a record written after a torn append was read from the wrong place"
        );
    }

    #[test]
    fn red_team_rows_a_crash_took_are_rebuilt_and_removed_chunks_stay_removed() {
        // Rows are committed without waiting for the disk between marks. A
        // crash loses them; the records are on the disk and name themselves,
        // so the replay at open must bring every one back -- and never a
        // record whose chunk was removed.
        let dir = tempfile::tempdir().expect("temp dir");
        let packs = Packs::open(dir.path()).expect("open");
        packs.put(&address(1), b"kept").unwrap();
        packs.put(&address(2), b"removed").unwrap();
        packs.remove(&address(2)).unwrap();
        packs.put(&address(3), b"after the mark").unwrap();
        // The crash: the rows after the mark never reached the disk.
        {
            let txn = packs.db.begin_write().unwrap();
            txn.open_table(CHUNKS)
                .unwrap()
                .remove(address(3).as_bytes().as_slice())
                .unwrap();
            txn.commit().unwrap();
        }
        let path = packs.pack_path(1);
        drop(packs);
        // And a record cut short at the tail.
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[0xAB; 20]).unwrap();
        drop(file);
        let packs = Packs::open(dir.path()).expect("reopen");
        assert_eq!(
            packs.get(&address(3)).unwrap().as_deref(),
            Some(&b"after the mark"[..]),
            "a chunk whose record was on the disk was lost with its row"
        );
        assert_eq!(
            packs.get(&address(2)).unwrap(),
            None,
            "a removed chunk came back"
        );
        assert_eq!(
            packs.get(&address(1)).unwrap().as_deref(),
            Some(&b"kept"[..])
        );
        packs.put(&address(4), b"after the torn tail").unwrap();
        {
            let txn = packs.db.begin_write().unwrap();
            txn.open_table(CHUNKS)
                .unwrap()
                .remove(address(4).as_bytes().as_slice())
                .unwrap();
            txn.commit().unwrap();
        }
        drop(packs);
        let packs = Packs::open(dir.path()).expect("reopen again");
        assert_eq!(
            packs.get(&address(4)).unwrap().as_deref(),
            Some(&b"after the torn tail"[..])
        );
        assert_eq!(
            packs.count().unwrap(),
            3,
            "the replay invented a chunk from torn bytes"
        );
    }

    #[test]
    fn red_team_a_damaged_record_reads_as_missing_so_repair_can_replace_it() {
        // A damaged pack, or a row left pointing where another record now is,
        // must never hand one chunk's bytes out as another's.
        let dir = tempfile::tempdir().expect("temp dir");
        let packs = Packs::open(dir.path()).expect("open");
        packs.put(&address(1), b"first").unwrap();
        packs.put(&address(2), b"secnd").unwrap();
        let path = packs.pack_path(1);
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 0xff; // the first record's address
        fs::write(&path, &bytes).unwrap();
        assert_eq!(
            packs.get(&address(1)).unwrap(),
            None,
            "bytes whose header names another address were served"
        );
        // Missing, as a lost blob file is: repair can store it again.
        assert!(
            !packs.contains(&address(1)),
            "the row of a damaged record was kept"
        );
        assert!(
            packs.put(&address(1), b"first").unwrap(),
            "the repair could not store it again"
        );
        assert_eq!(
            packs.get(&address(1)).unwrap().as_deref(),
            Some(&b"first"[..])
        );
        assert_eq!(
            packs.get(&address(2)).unwrap().as_deref(),
            Some(&b"secnd"[..])
        );
    }

    #[test]
    fn red_team_compaction_frees_a_dead_pack_and_keeps_every_live_chunk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let packs = Packs::open(dir.path()).expect("open");
        let chunk = vec![7u8; 1 << 20];
        for byte in 0..10u8 {
            packs.put(&address(byte), &chunk).unwrap();
        }
        // Close pack 1 by hand, as reaching PACK_TARGET would.
        {
            let mut current = packs.current.lock().unwrap();
            *current = (2, 0);
        }
        for byte in 0..8u8 {
            packs.remove(&address(byte)).unwrap();
        }
        let before = fs::metadata(packs.pack_path(1)).unwrap().len();
        let freed = packs.compact().unwrap();
        assert_eq!(freed, before, "a pack eight tenths dead was kept");
        assert!(!packs.pack_path(1).exists());
        for byte in 8..10u8 {
            assert_eq!(
                packs.get(&address(byte)).unwrap().as_deref(),
                Some(chunk.as_slice()),
                "compaction lost a live chunk"
            );
        }
        assert_eq!(packs.total_bytes().unwrap(), 2 << 20);
    }

    #[test]
    fn red_team_compaction_never_leaves_a_chunk_without_bytes_its_row_names() {
        // Rodin, 2026-10-13: the first compaction removed the row, then put
        // the copy; a crash between the two lost the chunk for good. Here
        // every step of a relocation is checked: before and after its commit
        // the row names readable bytes, and a reopen in between agrees.
        let dir = tempfile::tempdir().expect("temp dir");
        let packs = Packs::open(dir.path()).expect("open");
        packs.put(&address(1), b"moved").unwrap();
        packs.put(&address(2), b"dead").unwrap();
        {
            let mut current = packs.current.lock().unwrap();
            *current = (2, 0);
        }
        packs.remove(&address(2)).unwrap();
        // The crash, simulated: the copy appended, its commit never made.
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(packs.pack_path(2))
            .unwrap();
        let mut record = address(1).as_bytes().to_vec();
        record.extend_from_slice(&5u32.to_le_bytes());
        record.extend_from_slice(b"moved");
        file.write_all(&record).unwrap();
        drop(file);
        drop(packs);
        let packs = Packs::open(dir.path()).expect("reopen");
        assert_eq!(
            packs.get(&address(1)).unwrap().as_deref(),
            Some(&b"moved"[..]),
            "a crash in the middle of a relocation lost the chunk"
        );
        // And a whole compaction after it still keeps it.
        packs.compact().unwrap();
        assert_eq!(
            packs.get(&address(1)).unwrap().as_deref(),
            Some(&b"moved"[..])
        );
        assert!(!packs.pack_path(1).exists(), "the dead pack was kept");
    }

    #[test]
    fn two_opens_of_one_directory_share_one_index() {
        // redb refuses a second Database on one file; the vault opens a
        // store per call.
        let dir = tempfile::tempdir().expect("temp dir");
        let first = Packs::open(dir.path()).expect("open");
        let second = Packs::open(dir.path()).expect("a second open was refused");
        first.put(&address(3), b"seen").unwrap();
        assert!(second.contains(&address(3)));
    }
}
