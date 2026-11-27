//! Compaction: a closed pack mostly dead rewritten, its live records
//! copied into the current pack before the file goes.

use std::{
    fs::{self, OpenOptions},
    io::{Seek as _, SeekFrom, Write as _},
};

use itsanas_crypto::ChunkId;
use redb::ReadableTable as _;

use super::{BY_PACK, CHUNKS, HEADER, HEADER_LEN, LIVE, PACK_TARGET, Packs, Place, member_key};
use crate::error::{Result, StoreError};

impl Packs {
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
                let Ok(Some(place)) = self.place(&address) else {
                    continue;
                };
                let Ok(Some(bytes)) = self.get_at(&address, place) else {
                    continue;
                };
                self.relocate(&address, place, &bytes)?;
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
    /// A row no longer at `from` (removed, or moved, since it was read) is
    /// left as it is, and the copy is dead bytes before the mark.
    fn relocate(&self, address: &ChunkId, from: Place, sealed: &[u8]) -> Result<()> {
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
        let still = txn
            .open_table(CHUNKS)?
            .get(address.as_bytes().as_slice())?
            .map(|row| Place::decode(row.value()))
            .transpose()?
            == Some(from);
        if still {
            let from = from.pack;
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
        }
        Self::write_mark(&txn, pack, current.1)?;
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
    use super::super::tests::*;
    use super::super::*;

    /// A reader that found a chunk's record gone after a compaction moved it
    /// (its place read before the move, its pack opened after the old file
    /// went) reads the good copy and drops nothing. Sabotage: `remove_row`
    /// ignores `seen`.
    #[test]
    fn a_read_from_a_place_compaction_moved_does_not_drop_the_moved_row() {
        let dir = tempfile::tempdir().expect("temp dir");
        let packs = Packs::open(dir.path()).expect("open");
        assert!(packs.put(&address(1), b"moved by compaction").unwrap());
        *packs.current.lock().unwrap() = (2, 0);
        let stale = packs.place(&address(1)).unwrap().expect("a row");
        packs
            .relocate(&address(1), stale, b"moved by compaction")
            .unwrap();
        fs::remove_file(packs.pack_path(stale.pack)).unwrap();

        assert_eq!(
            packs.get_at(&address(1), stale).unwrap().as_deref(),
            Some(&b"moved by compaction"[..]),
            "a read from the old place lost the chunk compaction had moved"
        );
        assert_eq!(packs.count().unwrap(), 1, "the moved row was dropped");
        assert!(packs.contains(&address(1)));
    }

    /// A compaction that read a chunk before it was removed brings nothing
    /// back when it moves it. Sabotage: drop the `still` check in `relocate`.
    #[test]
    fn a_relocation_after_a_remove_brings_nothing_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let packs = Packs::open(dir.path()).expect("open");
        assert!(packs.put(&address(1), b"deleted meanwhile").unwrap());
        *packs.current.lock().unwrap() = (2, 0);
        let seen = packs.place(&address(1)).unwrap().expect("a row");
        assert!(packs.remove(&address(1)).unwrap());
        packs
            .relocate(&address(1), seen, b"deleted meanwhile")
            .unwrap();

        assert_eq!(packs.count().unwrap(), 0, "a removed chunk came back");
        assert_eq!(packs.total_bytes().unwrap(), 0);
        assert_eq!(packs.get(&address(1)).unwrap(), None);
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
}
