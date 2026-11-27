//! Bringing an index written by an earlier version up to date on open:
//! the device-first ordering rebuilt, the holder counts built, superseded
//! tables dropped.

use std::collections::BTreeMap;

use itsanas_crypto::DeviceId;
use redb::{ReadableTable, ReadableTableMetadata, TableDefinition};

use super::{
    HOLDER_COUNTS, HOLDER_COUNTS_BUILT, HOLDERS, HOLDINGS, Index, META, SUPERSEDED_TABLES,
};
use crate::{
    error::Result,
    holders::{self},
};

impl Index {
    /// Count `HOLDINGS` rows per device, once, for a store written before
    /// `HOLDER_COUNTS` existed. One pass over the table, on the first open.
    pub(super) fn build_holder_counts_if_missing(&self) -> Result<()> {
        let built = {
            let txn = self.db.begin_read()?;
            txn.open_table(META)?.get(HOLDER_COUNTS_BUILT)?.is_some()
        };
        if built {
            return Ok(());
        }
        let mut counts: BTreeMap<DeviceId, u64> = BTreeMap::new();
        {
            let txn = self.db.begin_read()?;
            for row in txn.open_table(HOLDINGS)?.iter()? {
                let (key, _) = row?;
                if let Some((device, _)) = holders::split_by_device(key.value()) {
                    *counts.entry(device).or_default() += 1;
                }
            }
        }
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(HOLDER_COUNTS)?;
            table.retain(|_, _| false)?;
            for (device, count) in &counts {
                table.insert(device.as_bytes().as_slice(), *count)?;
            }
            txn.open_table(META)?
                .insert(HOLDER_COUNTS_BUILT, [1u8].as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Tables an older version wrote that nothing reads any more.
    ///
    /// `reliability` was superseded by `peer_reliability_v2` when the record
    /// gained a field. Left in place it is invisible dead weight, and worse, a
    /// precedent: by the third version there would be three fossils in the file
    /// and nothing saying which one is authoritative. Deleting the old one is
    /// the difference between a migration and an accumulation.
    ///
    /// `delete_table` on a name that was never created is a no-op, so this
    /// costs one write transaction on the first open and nothing afterwards.
    pub(super) fn drop_superseded_tables(&self) -> Result<()> {
        let txn = self.db.begin_write()?;
        for name in SUPERSEDED_TABLES {
            let _ = txn.delete_table(TableDefinition::<&[u8], &[u8]>::new(name))?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Rebuild the device-first ordering for a store written before it existed.
    ///
    /// Every path that writes a holder record writes both orderings in one
    /// transaction, so the two cannot drift *while running*. They can still
    /// start out disagreeing: a file written by an earlier version has the
    /// chunk-first table populated and the device-first one absent.
    ///
    /// Left alone that is silent and total. `chunks_to_challenge` reads the
    /// device-first table, so it would return nothing, so no audit would ever
    /// ask a peer anything, so `note_audit` would never be reached and the
    /// daemon would print nothing — a node that has stopped checking its hosts
    /// looks exactly like a node whose hosts are all honest. `forget_device`
    /// would likewise report zero and leave every row behind.
    ///
    /// Runs once: after it, the second table is non-empty and the check is a
    /// pair of O(1) length reads on every subsequent open.
    pub(super) fn rebuild_holdings_if_stale(&self) -> Result<()> {
        {
            let txn = self.db.begin_read()?;
            let holders = txn.open_table(HOLDERS)?;
            let holdings = txn.open_table(HOLDINGS)?;
            if holders.len()? == 0 {
                return Ok(());
            }
            // Two ways a file can arrive here disagreeing with itself: written
            // before the second ordering existed (empty), or written before the
            // ordering was keyed (the right number of rows, the wrong shape).
            // The second is the dangerous one, because it looks healthy.
            let stale = holdings.len()? == 0
                || holdings
                    .iter()?
                    .next()
                    .transpose()?
                    .is_some_and(|(key, _)| key.value().len() != holders::HOLDING_KEY_LEN);
            if !stale {
                return Ok(());
            }
        }

        let txn = self.db.begin_write()?;
        {
            let holders = txn.open_table(HOLDERS)?;
            let mut holdings = txn.open_table(HOLDINGS)?;

            let doomed: Vec<Vec<u8>> = holdings
                .iter()?
                .filter_map(|row| row.ok().map(|(key, _)| key.value().to_vec()))
                .collect();
            for key in doomed {
                holdings.remove(key.as_slice())?;
            }

            let mut carried: Vec<([u8; holders::HOLDING_KEY_LEN], u64)> = Vec::new();
            for row in holders.iter()? {
                let (key, value) = row?;
                if let Some((chunk, device)) = holders::split(key.value()) {
                    let tag = holders::audit_tag(&self.audit_key, &chunk);
                    carried.push((holders::by_device(&device, &tag, &chunk), value.value()));
                }
            }
            for (key, when) in carried {
                holdings.insert(key.as_slice(), when)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Drop the device-first ordering, reproducing a file written before it
    /// existed.
    ///
    /// Only the backfill test calls this. Reproducing an old store any other
    /// way would mean keeping a binary fixture, which rots.
    #[cfg(test)]
    fn forget_the_device_ordering_for_test(&self) {
        let txn = self.db.begin_write().unwrap();
        {
            let mut holdings = txn.open_table(HOLDINGS).unwrap();
            let keys: Vec<Vec<u8>> = holdings
                .iter()
                .unwrap()
                .filter_map(|row| row.ok().map(|(key, _)| key.value().to_vec()))
                .collect();
            for key in keys {
                holdings.remove(key.as_slice()).unwrap();
            }
        }
        txn.commit().unwrap();
    }

    /// Rewrite the device-first table keyed by chunk id, as it was before the
    /// audit key existed.
    ///
    /// Only the rebuild test calls this. Keeping a binary fixture of an old
    /// file instead would rot, and this is the shape that matters: right number
    /// of rows, wrong ordering, nothing visibly broken.
    #[cfg(test)]
    fn order_the_device_table_by_chunk_id_for_test(&self) {
        let txn = self.db.begin_write().unwrap();
        {
            let holders = txn.open_table(HOLDERS).unwrap();
            let mut holdings = txn.open_table(HOLDINGS).unwrap();
            let doomed: Vec<Vec<u8>> = holdings
                .iter()
                .unwrap()
                .filter_map(|row| row.ok().map(|(key, _)| key.value().to_vec()))
                .collect();
            for key in doomed {
                holdings.remove(key.as_slice()).unwrap();
            }
            let mut carried = Vec::new();
            for row in holders.iter().unwrap() {
                let (key, value) = row.unwrap();
                if let Some((chunk, device)) = holders::split(key.value()) {
                    let mut old = Vec::with_capacity(64);
                    old.extend_from_slice(device.as_bytes());
                    old.extend_from_slice(chunk.as_bytes());
                    carried.push((old, value.value()));
                }
            }
            for (key, when) in carried {
                holdings.insert(key.as_slice(), when).unwrap();
            }
        }
        txn.commit().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;

    #[test]
    fn a_ledger_written_before_the_second_ordering_is_rebuilt_on_open() {
        // Every write path writes both orderings, so they cannot drift while
        // running. They can still *start* apart: a store written by an earlier
        // version has the chunk-first table and no other. Left alone that is
        // silent and total — `chunks_to_challenge` reads the device-first
        // table, so no audit would ever ask anything, and a node that has
        // stopped checking its hosts looks exactly like one whose hosts are
        // honest.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.redb");

        {
            let index = Index::open(&path, test_audit_key()).unwrap();
            index
                .record_holders(&[chunk(1), chunk(2), chunk(3)], &device(7), 5)
                .unwrap();
            // Reach past the API to reproduce the old file: remove the second
            // ordering and leave the first, which is exactly what an upgrade
            // from before it existed looks like.
            index.forget_the_device_ordering_for_test();
            assert!(
                index
                    .chunks_to_challenge(&device(7), &[[0u8; 8]])
                    .unwrap()
                    .is_empty(),
                "the fixture did not actually reproduce the old file"
            );
        }

        let index = Index::open(&path, test_audit_key()).unwrap();
        let orderings = index.holder_orderings().unwrap();
        assert_eq!(
            orderings.by_chunk, orderings.by_device,
            "opening an older store left the two orderings disagreeing"
        );
        assert_eq!(
            observed_audit_order(&index, &device(7)).len(),
            3,
            "audits are silently dead on a store written before this table"
        );
    }

    #[test]
    fn a_ledger_ordered_by_chunk_id_is_rebuilt_under_the_audit_key_on_open() {
        // The second shape of the same problem, and the more dangerous one,
        // because it looks healthy: a file written when the device-first
        // ordering existed but was keyed by the chunk id rather than by a
        // secret. Row count right, row shape wrong. Left alone, every audit on
        // that store would go on being predictable to the host — the exact
        // weakness the key was added to remove — and nothing anywhere would
        // say so.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.redb");
        let chunks: Vec<ChunkId> = (0..=255u8).map(chunk).collect();

        {
            let index = Index::open(&path, test_audit_key()).unwrap();
            index.record_holders(&chunks, &device(7), 5).unwrap();
            index.order_the_device_table_by_chunk_id_for_test();

            let stale = observed_audit_order(&index, &device(7));
            assert!(
                share_ascending_by_id(&stale) > 0.99,
                "the fixture did not actually reproduce the old ordering"
            );
        }

        let index = Index::open(&path, test_audit_key()).unwrap();
        let rebuilt = observed_audit_order(&index, &device(7));
        assert!(
            rebuilt.len() > 100,
            "the rebuild lost records: only {} came back",
            rebuilt.len()
        );
        assert!(
            share_ascending_by_id(&rebuilt) < 0.75,
            "opening the store left it ordered by chunk id, so its audits stay \
             predictable to the host and nothing reports it"
        );
        let orderings = index.holder_orderings().unwrap();
        assert_eq!(orderings.by_chunk, orderings.by_device);
    }
}
