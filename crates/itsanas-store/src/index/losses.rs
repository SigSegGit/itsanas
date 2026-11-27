//! The loss queue: chunks this node's files need whose bytes are not on this
//! disk, written by `doctor` and the repair scan, drained by repair.

use itsanas_crypto::ChunkId;
use redb::{ReadableTable, ReadableTableMetadata};

use super::{Index, LOSSES};
use crate::error::Result;

impl Index {
    /// Record that `chunk` is referenced but its bytes are not on this disk.
    ///
    /// Idempotent, and the timestamp is only for an operator reading the table:
    /// nothing sorts by it, because a loss does not become less urgent with age.
    pub fn note_loss(&self, chunk: &ChunkId, now: u64) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(LOSSES)?
                .insert(chunk.as_bytes().as_slice(), now)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Record `missing` as lost and forget any loss among `present`, in one
    /// transaction. Writes only when something changes, so a pass over a
    /// healthy disk commits nothing.
    pub fn settle_losses(&self, present: &[ChunkId], missing: &[ChunkId], now: u64) -> Result<()> {
        let recovered: Vec<&ChunkId> = {
            let txn = self.db.begin_read()?;
            let table = txn.open_table(LOSSES)?;
            let mut out = Vec::new();
            for chunk in present {
                if table.get(chunk.as_bytes().as_slice())?.is_some() {
                    out.push(chunk);
                }
            }
            out
        };
        if recovered.is_empty() && missing.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(LOSSES)?;
            for chunk in recovered {
                table.remove(chunk.as_bytes().as_slice())?;
            }
            for chunk in missing {
                if table.get(chunk.as_bytes().as_slice())?.is_none() {
                    table.insert(chunk.as_bytes().as_slice(), now)?;
                }
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Forget a loss, because the chunk is back or was never really gone.
    pub fn clear_loss(&self, chunk: &ChunkId) -> Result<()> {
        // Almost always nothing to forget; a read costs no commit.
        if self
            .db
            .begin_read()?
            .open_table(LOSSES)?
            .get(chunk.as_bytes().as_slice())?
            .is_none()
        {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            txn.open_table(LOSSES)?
                .remove(chunk.as_bytes().as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Up to `limit` known losses, starting where `cursor` lands and wrapping.
    ///
    /// Both halves of that signature are load-bearing, and the first version
    /// had neither.
    ///
    /// **Bounded**, because a node that lost a disk has millions of these and
    /// reading them all into a `Vec` would make the mechanism meant to heal it
    /// allocate a gigabyte before it did anything.
    ///
    /// **From a cursor**, because reading from the start of the key order every
    /// round means a run of losses that no reachable peer holds sits at the
    /// front for ever and starves everything behind it. The chunk that *can* be
    /// repaired is never reached, and nothing anywhere says why.
    ///
    /// Same shape as [`Self::live_chunks_from`] and the audit draw: seek to a
    /// random point, take what follows, wrap at the end.
    pub fn known_losses_from(&self, cursor: &ChunkId, limit: usize) -> Result<Vec<ChunkId>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let txn = self.db.begin_read()?;
        let table = txn.open_table(LOSSES)?;

        let mut out = Vec::with_capacity(limit.min(256));
        let lowest: &[u8] = &[];
        for start in [cursor.as_bytes().as_slice(), lowest] {
            for row in table.range(start..)? {
                let (key, _) = row?;
                let chunk = ChunkId::from_slice(key.value())?;
                if !out.contains(&chunk) {
                    out.push(chunk);
                }
                if out.len() == limit {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }

    /// How many losses are outstanding.
    pub fn loss_count(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        Ok(txn.open_table(LOSSES)?.len()?)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;

    #[test]
    fn a_loss_is_recorded_once_and_cleared_when_the_chunk_returns() {
        let (_dir, index) = index();
        assert_eq!(index.loss_count().unwrap(), 0);

        index.note_loss(&chunk(4), 100).unwrap();
        index.note_loss(&chunk(4), 200).unwrap();
        assert_eq!(
            index.loss_count().unwrap(),
            1,
            "the same loss counted twice"
        );

        index.clear_loss(&chunk(4)).unwrap();
        assert_eq!(index.loss_count().unwrap(), 0);
    }

    #[test]
    fn collecting_a_chunk_stops_it_being_a_loss() {
        // A chunk nothing references any more cannot be missing. Leaving the
        // entry would have repair asking peers, for ever, about data this node
        // deliberately threw away — and telling them what it no longer has.
        let (_dir, index) = index();
        index.put_file("a.txt", &entry(&[9])).unwrap();
        index.note_loss(&chunk(9), 1).unwrap();
        index.remove_file("a.txt", &tombstone()).unwrap();

        index.forget_chunk(&chunk(9)).unwrap();
        assert_eq!(
            index.loss_count().unwrap(),
            0,
            "a garbage-collected chunk is still queued for repair"
        );
    }

    #[test]
    fn the_loss_queue_is_read_from_a_moving_start_and_wraps() {
        // Both properties matter and the first version had neither.
        //
        // Bounded, because a node that lost a disk has millions of these and
        // reading them all would allocate a gigabyte before healing anything.
        //
        // From a cursor, because reading from the top of the key order every
        // round means a run of losses that no reachable peer holds sits at the
        // front for ever and starves everything behind it. The chunk that could
        // have been repaired is never reached and nothing says why.
        let (_dir, index) = index();
        for byte in 0..=255u8 {
            index.note_loss(&chunk(byte), 1).unwrap();
        }

        let low = index.known_losses_from(&chunk(0), 4).unwrap();
        let high = index.known_losses_from(&chunk(200), 4).unwrap();
        assert_eq!(low.len(), 4, "the window is not bounded");
        assert_eq!(high.len(), 4);
        assert_ne!(
            low, high,
            "the cursor is ignored, so the same four losses are considered \
             every round and everything behind them starves"
        );

        // A cursor past the last entry wraps rather than coming back empty, or
        // the losses at the top of the id space would be the only ones the
        // repair loop ever reached.
        let wrapped = index.known_losses_from(&chunk(255), 4).unwrap();
        assert_eq!(
            wrapped.len(),
            4,
            "a cursor near the end returned a short window"
        );

        // Every loss is reachable from some cursor.
        let mut seen = std::collections::BTreeSet::new();
        for byte in 0..=255u8 {
            seen.extend(index.known_losses_from(&chunk(byte), 1).unwrap());
        }
        assert_eq!(seen.len(), 256, "some queued loss can never be reached");
    }
}
