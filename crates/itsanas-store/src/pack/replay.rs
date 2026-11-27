//! The replay at open: the rows of whole records past the durable mark,
//! which a crash took with a commit that did not wait for the disk, rebuilt.

use std::{
    fs::File,
    io::{Read as _, Seek as _, SeekFrom},
};

use itsanas_crypto::ChunkId;
use redb::ReadableTable as _;

use super::{CHUNKS, HEADER, HEADER_LEN, MAX_RECORD, META, Packs, Place};
use crate::error::{Result, StoreError};

impl Packs {
    /// Rebuild the rows of every whole record after the durable mark that has
    /// none: the ones a crash took with a commit that did not wait for the
    /// disk. A record cut short by the crash ends the replay of its pack.
    pub(super) fn replay(&self) -> Result<()> {
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
                if len > MAX_RECORD || offset + HEADER + u64::from(len) > size {
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
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;

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
}
