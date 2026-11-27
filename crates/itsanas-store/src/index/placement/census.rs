//! The census: one ordered pass over every live chunk with its holders and
//! their stamps, from a random start.

use itsanas_crypto::{ChunkId, DeviceId};
use redb::ReadableTable;

use super::{CENSUS_CURSOR, CensusRow};
use crate::{
    error::Result,
    holders,
    index::{HOLDERS, Index, META},
};

impl Index {
    // --------------------------------------------------------------- census

    /// A page of live chunks after `after` (exclusive), each with every
    /// recorded holder and that record's stamp, plus where to continue.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn census_page(
        &self,
        after: Option<&ChunkId>,
        limit: usize,
    ) -> Result<(Vec<CensusRow>, Option<ChunkId>)> {
        let (chunks, next) = self.live_chunks_page(after, limit)?;
        Ok((self.with_stamps(&chunks)?, next))
    }

    /// The next `limit` live chunks of the rolling census, with their holders.
    ///
    /// Continues from the cursor kept in `META`, wraps past the end, and never
    /// returns a chunk twice in one call. With no cursor yet it starts at
    /// `random` rather than the lowest id: every node starting a census at the
    /// same end would have the head of the id space examined first and the
    /// tail last, everywhere, after every upgrade.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn census_next(&self, limit: usize, random: &ChunkId) -> Result<Vec<CensusRow>> {
        let after = {
            let txn = self.db.begin_read()?;
            match txn.open_table(META)?.get(CENSUS_CURSOR)? {
                Some(value) => ChunkId::from_slice(value.value())?,
                None => *random,
            }
        };
        let (mut chunks, next) = self.live_chunks_page(Some(&after), limit)?;
        if next.is_none() && chunks.len() < limit {
            // Wrap: from the lowest id up to and including `after`, which is
            // exactly what the first part did not cover.
            let (wrapped, _) = self.live_chunks_page(None, limit - chunks.len())?;
            chunks.extend(wrapped.into_iter().take_while(|chunk| chunk <= &after));
        }
        if let Some(last) = chunks.last() {
            let txn = self.db.begin_write()?;
            txn.open_table(META)?
                .insert(CENSUS_CURSOR, last.as_bytes().as_slice())?;
            txn.commit()?;
        }
        self.with_stamps(&chunks)
    }

    fn with_stamps(&self, chunks: &[ChunkId]) -> Result<Vec<CensusRow>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(HOLDERS)?;
        chunks
            .iter()
            .map(|chunk| Ok((*chunk, stamps_in(&table, chunk)?)))
            .collect()
    }

    /// Every other device recorded as holding `chunk`, with its record's
    /// stamp, sorted by device.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn holders_with_stamps(&self, chunk: &ChunkId) -> Result<Vec<(DeviceId, u64)>> {
        let txn = self.db.begin_read()?;
        stamps_in(&txn.open_table(HOLDERS)?, chunk)
    }
}

fn stamps_in(
    table: &impl ReadableTable<&'static [u8], u64>,
    chunk: &ChunkId,
) -> Result<Vec<(DeviceId, u64)>> {
    let mut out = Vec::new();
    for row in table
        .range(holders::range_start(chunk).as_slice()..=holders::range_end(chunk).as_slice())?
    {
        let (key, value) = row?;
        if let Some((_, device)) = holders::split(key.value()) {
            out.push((device, value.value()));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;

    #[test]
    fn the_census_starts_at_random_wraps_and_covers_every_chunk_once_a_pass() {
        let (_dir, index) = index();
        let all: Vec<ChunkId> = (1..=5).map(|n| chunk(n * 10)).collect();
        file(&index, "a", &all);
        index
            .record_holder(&all[1], &device(5), T0)
            .expect("record");

        // Starts just after the random point, not at the lowest id.
        let first = index.census_next(2, &chunk(25)).expect("census");
        let ids = |rows: &[CensusRow]| rows.iter().map(|(c, _)| *c).collect::<Vec<_>>();
        assert_eq!(ids(&first), vec![chunk(30), chunk(40)]);

        // The random start is used once: the cursor persists from here on.
        let second = index.census_next(2, &chunk(0)).expect("census");
        assert_eq!(
            ids(&second),
            vec![chunk(50), chunk(10)],
            "wraps past the end"
        );
        let third = index.census_next(2, &chunk(0)).expect("census");
        assert_eq!(ids(&third), vec![chunk(20), chunk(30)]);
        assert_eq!(
            third[0].1,
            vec![(device(5), T0)],
            "holders come with stamps"
        );

        // A page larger than the live set returns each chunk once.
        let whole = index.census_next(50, &chunk(0)).expect("census");
        assert_eq!(whole.len(), all.len());
        let mut sorted = ids(&whole);
        sorted.sort();
        assert_eq!(sorted, all);
    }

    #[test]
    fn two_censuses_with_different_random_starts_begin_in_different_places() {
        let (_one, a) = index();
        let (_two, b) = index();
        let all: Vec<ChunkId> = (1..=5).map(|n| chunk(n * 10)).collect();
        file(&a, "x", &all);
        file(&b, "x", &all);
        let from_a = a.census_next(1, &chunk(5)).expect("census");
        let from_b = b.census_next(1, &chunk(35)).expect("census");
        assert_ne!(from_a[0].0, from_b[0].0);
    }

    #[test]
    fn census_pages_and_holder_stamps_match_the_ledger() {
        let (_dir, index) = index();
        file(&index, "a", &[chunk(1), chunk(2)]);
        index
            .record_holder(&chunk(2), &device(6), T0)
            .expect("record");
        index
            .record_holder(&chunk(2), &device(5), T0 + 1)
            .expect("record");
        assert_eq!(
            index.holders_with_stamps(&chunk(2)).expect("stamps"),
            vec![(device(5), T0 + 1), (device(6), T0)]
        );
        let (page, next) = index.census_page(None, 1).expect("page");
        assert_eq!(page, vec![(chunk(1), Vec::new())]);
        let (page, end) = index.census_page(next.as_ref(), 5).expect("page");
        assert_eq!(page[0].1.len(), 2);
        assert_eq!(end, None);
    }
}
