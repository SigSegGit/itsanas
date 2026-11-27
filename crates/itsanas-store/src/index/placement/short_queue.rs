//! The short queue: chunks found possibly short of copies, worked off by the
//! push loop before the census reaches them.

use itsanas_crypto::ChunkId;
use redb::ReadableTableMetadata;

use super::{QUEUE_ARMED, SHORT_SINCE, ShortPage, ShortQueue, TRIGGER_LIMIT, enqueue_in};
use crate::{
    error::Result,
    index::{CHUNK_REFS, Index, META},
};

impl Index {
    // ---------------------------------------------------------- short queue

    /// Queue `chunks` as possibly short, keeping any earlier timestamp.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn enqueue_short(&self, chunks: &[ChunkId], now: u64) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            let mut short = ShortQueue::open(&txn)?;
            let refs = txn.open_table(CHUNK_REFS)?;
            for chunk in chunks {
                enqueue_in(&mut short, &refs, chunk, now)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Arm the short queue (`measured`) or disarm and empty it (`everyone`).
    ///
    /// The daemon calls this every round with its mode; it writes only on a
    /// change. Emptied on the way back to `everyone`, because nothing settles
    /// the queue there and its timestamps would be stale on the next switch:
    /// a chunk queued a month ago reads as a month short, past the safety
    /// grace, and goes to every eligible host.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn set_placement_measured(&self, measured: bool) -> Result<()> {
        let armed = {
            let txn = self.db.begin_read()?;
            Self::read_u64(&txn.open_table(META)?, QUEUE_ARMED)? == 1
        };
        if armed == measured && (measured || self.short_count()? == 0) {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        txn.open_table(META)?
            .insert(QUEUE_ARMED, u64::from(measured).to_le_bytes().as_slice())?;
        if !measured {
            txn.delete_table(SHORT_SINCE)?;
            let _ = txn.open_table(SHORT_SINCE)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Take `chunks` off the queue: their placement goal is met.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn settle_short(&self, chunks: &[ChunkId]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            let mut short = txn.open_table(SHORT_SINCE)?;
            for chunk in chunks {
                short.remove(chunk.as_bytes().as_slice())?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// A page of the queue in chunk order, after `after` (exclusive), with
    /// where to continue or `None` at the end.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn short_page(&self, after: Option<&ChunkId>, limit: usize) -> Result<ShortPage> {
        if limit == 0 {
            return Ok((Vec::new(), None));
        }
        let txn = self.db.begin_read()?;
        let short = txn.open_table(SHORT_SINCE)?;
        let start: Vec<u8> = after.map_or_else(Vec::new, |chunk| chunk.as_bytes().to_vec());
        let mut out = Vec::with_capacity(limit.min(TRIGGER_LIMIT));
        for row in short.range(start.as_slice()..)? {
            let (key, value) = row?;
            let chunk = ChunkId::from_slice(key.value())?;
            if after == Some(&chunk) {
                continue;
            }
            out.push((chunk, value.value()));
            if out.len() == limit {
                return Ok((out, Some(chunk)));
            }
        }
        Ok((out, None))
    }

    /// How many chunks are queued.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn short_count(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        Ok(txn.open_table(SHORT_SINCE)?.len()?)
    }

    /// The first queued chunk at or after `cursor`, wrapping to the lowest.
    ///
    /// For a random start, so a queue longer than one round's budget is not
    /// worked from the same end every round while its tail waits for ever.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn short_near(&self, cursor: &ChunkId) -> Result<Option<ChunkId>> {
        let txn = self.db.begin_read()?;
        let short = txn.open_table(SHORT_SINCE)?;
        let lowest: &[u8] = &[];
        for start in [cursor.as_bytes().as_slice(), lowest] {
            if let Some(row) = short.range(start..)?.next() {
                return Ok(Some(ChunkId::from_slice(row?.0.value())?));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use crate::reliability::FAILURES_BEFORE_PAUSE;

    #[test]
    fn a_chunk_s_first_reference_queues_it_and_a_second_does_not_add_a_row() {
        let (_dir, index) = index();
        file(&index, "a", &[chunk(1), chunk(2), chunk(3)]);
        assert_eq!(queued(&index), vec![chunk(1), chunk(2), chunk(3)]);

        settle_everything(&index);
        file(&index, "b", &[chunk(2), chunk(4)]);
        assert_eq!(
            queued(&index),
            vec![chunk(4)],
            "only the chunk new to this account is queued"
        );
    }

    #[test]
    fn forgetting_holders_queues_only_the_records_really_removed() {
        let (_dir, index) = index();
        file(&index, "a", &[chunk(1), chunk(2), chunk(3)]);
        settle_everything(&index);
        let host = device(5);
        index
            .record_holders(&[chunk(1), chunk(2)], &host, T0)
            .expect("record");

        index
            .forget_holders(&[chunk(1), chunk(3)], &host)
            .expect("forget");
        assert_eq!(queued(&index), vec![chunk(1)]);

        index.forget_holder(&chunk(2), &host).expect("forget one");
        assert_eq!(queued(&index), vec![chunk(1), chunk(2)]);
    }

    #[test]
    fn a_device_leaving_queues_every_chunk_it_held() {
        let (_dir, index) = index();
        file(&index, "a", &[chunk(1), chunk(2), chunk(3)]);
        settle_everything(&index);
        index
            .record_holders(&[chunk(1), chunk(3)], &device(5), T0)
            .expect("record");
        index.forget_device(&device(5)).expect("leave");
        assert_eq!(queued(&index), vec![chunk(1), chunk(3)]);
    }

    #[test]
    fn a_draining_device_queues_its_chunks_with_the_grant() {
        let (_dir, index) = index();
        file(&index, "a", &[chunk(1), chunk(2), chunk(3)]);
        settle_everything(&index);
        index
            .record_holders(&[chunk(2), chunk(3)], &device(5), T0)
            .expect("record");
        assert!(index.note_draining(&device(5), T0 + 60, T0).expect("drain"));
        assert_eq!(queued(&index), vec![chunk(2), chunk(3)]);
    }

    #[test]
    fn a_host_paused_by_its_audits_queues_its_chunks_once() {
        let (_dir, index) = index();
        file(&index, "a", &[chunk(1), chunk(2), chunk(3)]);
        settle_everything(&index);
        let host = device(5);
        index
            .record_holders(&[chunk(1), chunk(2)], &host, T0)
            .expect("record");
        for _ in 1..FAILURES_BEFORE_PAUSE {
            index.note_audit(&host, false, T0).expect("fail");
        }
        assert!(queued(&index).is_empty(), "not paused yet");
        let record = index.note_audit(&host, false, T0).expect("fail");
        assert!(record.paused);
        assert_eq!(queued(&index), vec![chunk(1), chunk(2)]);

        settle_everything(&index);
        index.note_audit(&host, false, T0).expect("fail again");
        assert!(
            queued(&index).is_empty(),
            "only the transition to paused queues"
        );
    }

    #[test]
    fn red_team_a_trigger_walks_a_bounded_number_of_records() {
        let (_dir, index) = index();
        let count = TRIGGER_LIMIT + 50;
        let chunks: Vec<ChunkId> = (0..count)
            .map(|n| {
                let mut bytes = [0u8; 32];
                bytes[..8].copy_from_slice(&(n as u64).to_be_bytes());
                ChunkId::from_bytes(bytes)
            })
            .collect();
        file(&index, "big", &chunks);
        settle_everything(&index);
        index
            .record_holders(&chunks, &device(5), T0)
            .expect("record");
        assert!(index.note_draining(&device(5), T0 + 60, T0).expect("drain"));
        assert_eq!(
            index.short_count().expect("count"),
            TRIGGER_LIMIT as u64,
            "the census covers the rest; the drain's transaction must not"
        );
    }

    #[test]
    fn a_chunk_outside_the_live_set_is_never_queued() {
        let (_dir, index) = index();
        index.enqueue_short(&[chunk(9)], T0).expect("enqueue");
        index
            .record_holder(&chunk(9), &device(5), T0)
            .expect("record");
        index.forget_holder(&chunk(9), &device(5)).expect("forget");
        assert_eq!(queued(&index), [] as [itsanas_crypto::ChunkId; 0]);
    }

    #[test]
    fn the_queue_keeps_the_first_time_a_chunk_was_found_short() {
        let (_dir, index) = index();
        file(&index, "a", &[chunk(1)]);
        settle_everything(&index);
        index.enqueue_short(&[chunk(1)], T0).expect("enqueue");
        index.enqueue_short(&[chunk(1)], T0 + HOUR).expect("again");
        assert_eq!(
            index.short_page(None, 10).expect("page").0,
            vec![(chunk(1), T0)]
        );
    }

    #[test]
    fn the_queue_is_read_from_a_random_start_and_wraps() {
        let (_dir, index) = index();
        file(&index, "a", &[chunk(2), chunk(4), chunk(6)]);
        assert_eq!(index.short_near(&chunk(3)).expect("near"), Some(chunk(4)));
        assert_eq!(index.short_near(&chunk(7)).expect("wrap"), Some(chunk(2)));
        let (first, next) = index.short_page(None, 2).expect("page");
        assert_eq!(first.len(), 2);
        let (rest, end) = index.short_page(next.as_ref(), 2).expect("page");
        assert_eq!(
            rest.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            vec![chunk(6)]
        );
        assert_eq!(end, None);
    }

    #[test]
    fn red_team_everyone_mode_never_grows_the_short_queue() {
        // In `everyone` nothing works the queue off: every trigger writing a
        // row would be a table the size of the account kept for nothing.
        // And switching back to `everyone` empties what `measured` left.
        // Sabotages: drop the `armed` check in `enqueue_in`/`insert_new`;
        // drop the clear in `set_placement_measured`.
        let dir = tempfile::tempdir().expect("temp dir");
        let index = Index::open(dir.path().join("index.redb"), key()).expect("open");
        file(&index, "a", &[chunk(1), chunk(2)]);
        index
            .record_holders(&[chunk(1)], &device(5), T0)
            .expect("record");
        index.forget_device(&device(5)).expect("leave");
        index.enqueue_short(&[chunk(2)], T0).expect("enqueue");
        assert_eq!(index.short_count().expect("count"), 0);

        index.set_placement_measured(true).expect("arm");
        file(&index, "b", &[chunk(3)]);
        assert_eq!(index.short_count().expect("count"), 1);
        index.set_placement_measured(false).expect("disarm");
        assert_eq!(index.short_count().expect("count"), 0);
        file(&index, "c", &[chunk(4)]);
        assert_eq!(index.short_count().expect("count"), 0);
    }
}
