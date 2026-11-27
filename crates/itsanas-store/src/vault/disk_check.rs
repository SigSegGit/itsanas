//! The rolling disk check: index rows walked against the blob files from a
//! cursor kept on disk, and a row whose blob is gone removed with its bytes.

use itsanas_crypto::UserId;
use redb::ReadableTable;

use super::{
    CHECK_CURSOR, CHECK_CURSOR_KEY, CHUNK_BYTES, CHUNKS, OWNER_CHUNK_BYTES, TOTALS, Vault,
    chunk_from_key, commit_if, eviction::unmark,
};
use crate::{blob::BlobStore, error::Result};

/// Most rows one call to [`Vault::check_disk`] or `Store::check_disk`
/// examines, whatever it is asked for.
///
/// The slice a round needs grows with the account and the interval: 190 477
/// rows at a terabyte on an hourly policy, 12 MB of keys in one `Vec` on a
/// machine whose measured peak is 17 MiB. Found by the CI reviewer. The daemon
/// calls again until its slice is done; memory stays at one batch.
/// How long the disk check's cursor may move in memory before it is written
/// ([`Vault::check_disk`]).
///
/// It is on disk so that a pass survives restarts -- a laptop restarted every
/// day would otherwise check only the first rows of a big vault -- and it was
/// written at every round: one of an idle round's two commits (2026-10-27).
/// A restart now resumes at most this far back, checking those rows twice.
pub const CHECK_CURSOR_STEP: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The rows one disk check looks at ([`Vault::check_disk`]).
struct CheckSlice {
    rows: Vec<[u8; 64]>,
    /// Whether they run to the last row: the pass ends with them.
    reached_end: bool,
    /// The cursor as written on disk.
    on_disk: Option<Vec<u8>>,
}

/// The disk check's cursor as this process last moved it, and when it was
/// last written ([`CHECK_CURSOR_STEP`]).
#[derive(Debug)]
pub(super) struct CursorMemo {
    cursor: Option<Vec<u8>>,
    written: std::time::Instant,
}

pub const MAX_CHECK_BATCH: usize = 4096;

/// What one slice of [`Vault::check_disk`] found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskCheck {
    /// Index rows examined.
    pub checked: usize,
    /// Rows whose blob was no longer on disk, now removed from the index and
    /// the totals.
    pub removed: usize,
}

/// Move the disk check's cursor to `next` (`None`: a pass ended), in memory,
/// and on disk at the first step, with a removal, at the end of a pass, or a
/// step after the last write ([`CHECK_CURSOR_STEP`]). Whether the disk's
/// cursor changed.
fn pace_cursor(
    txn: &redb::WriteTransaction,
    memo: &mut Option<CursorMemo>,
    next: Option<&[u8; 64]>,
    on_disk: Option<&[u8]>,
    removed: bool,
    now: std::time::Instant,
) -> Result<bool> {
    let write = removed
        || next.is_none()
        || memo
            .as_ref()
            .is_none_or(|memo| now.saturating_duration_since(memo.written) >= CHECK_CURSOR_STEP);
    if write {
        let mut cursor = txn.open_table(CHECK_CURSOR)?;
        match next {
            Some(last) => {
                cursor.insert(CHECK_CURSOR_KEY, last.as_slice())?;
            }
            None => {
                cursor.remove(CHECK_CURSOR_KEY)?;
            }
        }
    }
    let written = match memo.as_ref() {
        Some(memo) if !write => memo.written,
        _ => now,
    };
    *memo = Some(CursorMemo {
        cursor: next.map(|last| last.to_vec()),
        written,
    });
    Ok(write && on_disk != next.map(<[u8; 64]>::as_slice))
}

impl Vault {
    /// Check up to `limit` index rows against the disk, from where the last
    /// call stopped, and remove each row whose blob is gone.
    ///
    /// # Why the index has to be checked against the disk
    ///
    /// The summary an owner compares ([`Self::chunk_summary`]) is read from the
    /// index; `HaveChunks` is answered from the blob files. Until §8 4c the
    /// owner's full walk asked `HaveChunks` about everything every
    /// `REFRESH_AFTER`, so a blob that vanished from this disk behind the
    /// index -- a bad sector, a careless `rm`, a restored backup -- was noticed
    /// within that time. Since 4c an owner whose summary agrees with this one
    /// re-stamps its records without asking, and the index is the only thing
    /// it hears. Without this check an honest host that lost files would keep
    /// counting as a copy of them for as long as it kept agreeing.
    ///
    /// Removing the row is what makes the loss visible: the summary of that
    /// owner's bucket changes, the owner's next round lists the bucket, finds
    /// the chunk missing and sends it again. The bytes leave the totals in the
    /// same transaction, so the pledge stops counting what is not there.
    ///
    /// The daemon sizes its slice so a whole pass fits in `REFRESH_AFTER`
    /// ([`crate::holders::rows_per_round`]) and calls this until the slice is
    /// done; one call examines at most [`MAX_CHECK_BATCH`] rows. The cursor
    /// wraps: a call that reaches the last row starts the next one from the
    /// first.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn check_disk(&self, limit: usize) -> Result<DiskCheck> {
        self.check_disk_at(limit, std::time::Instant::now())
    }

    /// The next `limit` rows after the disk check's cursor -- this process's,
    /// which may be ahead of the disk's, or the disk's at its first check --
    /// whether they reach the last row, and the cursor on disk.
    fn check_slice(&self, memo: Option<&CursorMemo>, limit: usize) -> Result<CheckSlice> {
        let txn = self.db.begin_read()?;
        let on_disk = txn
            .open_table(CHECK_CURSOR)?
            .get(CHECK_CURSOR_KEY)?
            .map(|value| value.value().to_vec());
        let cursor = memo.map_or_else(|| on_disk.clone(), |memo| memo.cursor.clone());
        let table = txn.open_table(CHUNKS)?;
        let mut rows: Vec<[u8; 64]> = Vec::with_capacity(limit.min(1 << 16));
        let range = match &cursor {
            Some(after) => table.range::<&[u8]>((
                std::ops::Bound::Excluded(after.as_slice()),
                std::ops::Bound::Unbounded,
            ))?,
            None => table.range::<&[u8]>(..)?,
        };
        let mut reached_end = true;
        for row in range {
            if rows.len() == limit {
                reached_end = false;
                break;
            }
            let (key, _) = row?;
            if let Ok(key) = <[u8; 64]>::try_from(key.value()) {
                rows.push(key);
            }
        }
        Ok(CheckSlice {
            rows,
            reached_end,
            on_disk,
        })
    }

    /// [`Self::check_disk`] at `now`, which paces the cursor's writes.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn check_disk_at(&self, limit: usize, now: std::time::Instant) -> Result<DiskCheck> {
        let mut check = DiskCheck::default();
        let limit = limit.min(MAX_CHECK_BATCH);
        if limit == 0 {
            return Ok(check);
        }
        let mut memo = self
            .check_cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let CheckSlice {
            rows,
            reached_end,
            on_disk,
        } = self.check_slice(memo.as_ref(), limit)?;

        let mut gone: Vec<[u8; 64]> = Vec::new();
        let mut owner_blobs: Option<(UserId, BlobStore)> = None;
        for key in &rows {
            check.checked += 1;
            let mut owner_bytes = [0u8; 32];
            owner_bytes.copy_from_slice(&key[..32]);
            let owner = UserId::from_bytes(owner_bytes);
            let Some(address) = chunk_from_key(key) else {
                continue;
            };
            if owner_blobs.as_ref().is_none_or(|(held, _)| *held != owner) {
                owner_blobs = Some((owner, self.blobs_for(owner)?));
            }
            if let Some((_, blobs)) = &owner_blobs
                && !blobs.contains(&address)
            {
                gone.push(*key);
            }
        }

        // Under the writes lock, and each row re-checked inside it: a put that
        // landed between the read above and here has written its blob first,
        // so a row whose file exists now is kept.
        let _writing = self.chunk_writes();
        let txn = self.db.begin_write()?;
        let changed;
        {
            let mut table = txn.open_table(CHUNKS)?;
            let mut totals = txn.open_table(TOTALS)?;
            let mut per_owner = txn.open_table(OWNER_CHUNK_BYTES)?;
            for key in &gone {
                let mut owner_bytes = [0u8; 32];
                owner_bytes.copy_from_slice(&key[..32]);
                let owner = UserId::from_bytes(owner_bytes);
                let Some(address) = chunk_from_key(key) else {
                    continue;
                };
                if self.blobs_for(owner)?.contains(&address) {
                    continue;
                }
                let Some(old) = table.remove(key.as_slice())?.map(|old| old.value()) else {
                    continue;
                };
                check.removed += 1;
                unmark(&txn, owner, &address)?;
                let total = totals.get(CHUNK_BYTES)?.map_or(0, |total| total.value());
                totals.insert(CHUNK_BYTES, total.saturating_sub(old))?;
                let mine = per_owner
                    .get(owner_bytes.as_slice())?
                    .map_or(0, |total| total.value());
                per_owner.insert(owner_bytes.as_slice(), mine.saturating_sub(old))?;
            }
            let next = rows.last().filter(|_| !reached_end);
            let moved = pace_cursor(
                &txn,
                &mut memo,
                next,
                on_disk.as_deref(),
                check.removed > 0,
                now,
            )?;
            changed = check.removed > 0 || moved;
        }
        // A pass over a small vault ends where it began, every round.
        commit_if(txn, changed)?;
        // A full pass done: give back the space of what was removed since,
        // for owners whose chunks are in packs (blob files gave it back at
        // removal). Once a pass, not per removal: a compaction moves up to a
        // pack's worth of bytes, which a request must not wait for.
        if reached_end {
            for owner in self.owners()? {
                self.blobs_for(owner)?.compact()?;
            }
        }
        Ok(check)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;

    /// Three chunks for one owner, ids in key order.
    fn three_chunks(vault: &Vault, owner: UserId) -> [ChunkId; 3] {
        let chunks = [
            ChunkId::from_bytes([0x10; 32]),
            ChunkId::from_bytes([0x20; 32]),
            ChunkId::from_bytes([0x30; 32]),
        ];
        for (index, chunk) in chunks.iter().enumerate() {
            vault
                .put_chunk(owner, chunk, &vec![u8::try_from(index).unwrap(); 1000])
                .unwrap();
        }
        chunks
    }

    /// Lose a blob the way a disk does: the file goes, the index row stays.
    fn lose_behind_the_index(vault: &Vault, owner: UserId, chunk: &ChunkId) {
        vault.blobs_for(owner).unwrap().damage(chunk).unwrap();
    }

    /// The idle bench's commits, counted over idle rounds only (2026-10-27):
    /// one of the two left was the disk check's cursor, moved and written at
    /// every round. Written now at the first step, when a row is removed, when
    /// a pass ends, and otherwise at most every [`CHECK_CURSOR_STEP`]; between
    /// writes it moves in memory, so the pass goes on. A restart resumes at the
    /// cursor last written: a few rows checked twice, none skipped.
    #[test]
    fn red_team_the_disk_checks_cursor_is_not_written_at_every_step() {
        use std::time::{Duration, Instant};
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        let chunks: Vec<ChunkId> = (1..=8u8)
            .map(|byte| ChunkId::from_bytes([byte * 0x10; 32]))
            .collect();
        for chunk in &chunks {
            vault.put_chunk(owner, chunk, b"bytes").unwrap();
        }
        let start = Instant::now();
        let minute = Duration::from_secs(60);

        let before = crate::commits();
        vault.check_disk_at(1, start).unwrap();
        assert_eq!(
            crate::commits() - before,
            1,
            "the first step's cursor was not written"
        );

        let before = crate::commits();
        for step in 1..4 {
            vault.check_disk_at(1, start + minute * step).unwrap();
        }
        assert_eq!(
            crate::commits() - before,
            0,
            "the disk check's cursor was written at every step: a commit every idle round"
        );

        lose_behind_the_index(&vault, owner, &chunks[4]);
        assert_eq!(
            vault.check_disk_at(1, start + minute * 4).unwrap().removed,
            1,
            "the fifth row was not the fifth step's: the cursor kept in memory did not move"
        );

        let before = crate::commits();
        vault
            .check_disk_at(1, start + minute * 5 + CHECK_CURSOR_STEP)
            .unwrap();
        assert_eq!(
            crate::commits() - before,
            1,
            "the cursor went a whole step unwritten: every restart would begin the pass again"
        );
    }

    #[test]
    fn red_team_a_blob_lost_behind_the_index_is_found_within_one_pass() {
        // §8 4c. An owner whose summary agrees with this host's re-stamps its
        // records without asking, and the summary is read from the index, not
        // the disk. A blob that vanished behind the index left the summary
        // agreeing for ever: the owner counted a copy this host could not
        // serve. Sabotage: keep the row when the blob is missing.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        let chunks = three_chunks(&vault, owner);
        let before = vault.chunk_summary(owner).unwrap();
        let held = vault.held_bytes_for(owner).unwrap();

        lose_behind_the_index(&vault, owner, &chunks[1]);
        assert_eq!(
            vault.chunk_summary(owner).unwrap(),
            before,
            "fixture: the index alone should not have noticed"
        );

        let mut removed = 0;
        for _ in 0..3 {
            removed += vault.check_disk(1).unwrap().removed;
        }
        assert_eq!(removed, 1, "one pass of the check missed the lost blob");
        assert_ne!(
            vault.chunk_summary(owner).unwrap(),
            before,
            "the summary still says the lost chunk is here: its owner keeps counting this \
             host as a copy it cannot serve"
        );
        assert_eq!(vault.chunks_for(owner).unwrap(), vec![chunks[0], chunks[2]]);
        assert_eq!(
            vault.held_bytes_for(owner).unwrap(),
            held - 1000,
            "the pledge still counts bytes that are not on the disk"
        );
    }

    #[test]
    fn red_team_the_disk_check_resumes_where_it_stopped_after_a_restart() {
        // A daemon restarted more often than a pass takes would otherwise check
        // the first rows for ever and never reach the last. Sabotage: do not
        // keep the cursor on disk.
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(1).user_id();
        let chunks = {
            let vault = Vault::open(dir.path()).unwrap();
            let chunks = three_chunks(&vault, owner);
            lose_behind_the_index(&vault, owner, &chunks[2]);
            assert_eq!(vault.check_disk(2).unwrap().removed, 0);
            chunks
        };
        let vault = Vault::open(dir.path()).unwrap();
        assert_eq!(
            vault.check_disk(2).unwrap().removed,
            1,
            "after a restart the check started from the top again, and the last row was \
             never reached"
        );
        assert!(!vault.chunks_for(owner).unwrap().contains(&chunks[2]));
    }
    #[test]
    fn red_team_one_disk_check_call_holds_at_most_one_batch() {
        // A slice of 190 477 rows asked for in one call was 12 MB of keys at
        // once on a Pi. Sabotage: drop the cap.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        for index in 0..(MAX_CHECK_BATCH + 10) {
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&(index as u64).to_be_bytes());
            vault
                .put_chunk(owner, &ChunkId::from_bytes(id), &[1])
                .unwrap();
        }
        assert_eq!(
            vault.check_disk(usize::MAX).unwrap().checked,
            MAX_CHECK_BATCH,
            "one call held more than a batch of keys in memory"
        );
    }

    #[test]
    fn the_disk_check_keeps_every_row_whose_blob_is_there() {
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        three_chunks(&vault, owner);
        let before = vault.chunk_summary(owner).unwrap();
        let check = vault.check_disk(100).unwrap();
        assert_eq!(
            check,
            DiskCheck {
                checked: 3,
                removed: 0
            }
        );
        assert_eq!(vault.chunk_summary(owner).unwrap(), before);
        assert_eq!(vault.chunk_rows().unwrap(), 3);
    }

    /// The check runs every round, and on a small vault a pass ends where it
    /// began: nothing removed and the cursor unchanged is no commit
    /// (ROADMAP, "An idle node's disk writes"). A pass that does remove a
    /// row still commits it. Sabotaged (always commit): red.
    #[test]
    fn a_disk_check_that_finds_nothing_to_do_writes_nothing() {
        let (dir, vault) = vault();
        let owner = keys(1).user_id();
        three_chunks(&vault, owner);
        vault.check_disk(100).unwrap();
        // Size and write time, not bytes: Windows refuses to read a database
        // another handle has locked. After a pause, so a write moves the time.
        let file = || {
            let meta = std::fs::metadata(dir.path().join("vault.redb")).unwrap();
            (meta.len(), meta.modified().unwrap())
        };
        let before = file();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(vault.check_disk(100).unwrap().removed, 0);
        assert!(
            file() == before,
            "a disk check that found nothing to do committed"
        );
    }
}
