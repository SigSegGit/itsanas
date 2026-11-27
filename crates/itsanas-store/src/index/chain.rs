//! This device's operation log: pending entries, the segment chain, and the
//! heads of other devices' chains applied or known.

use itsanas_crypto::{DeviceId, ObjectId};
use redb::ReadableTable;

use super::{
    APPLIED, Index, KNOWN_HEADS, META, META_CHAIN_LENGTH, META_HEAD_SEGMENT, META_NEXT_SEQUENCE,
    PENDING, SEGMENTS,
};
use crate::{
    error::{Result, StoreError},
    oplog::{LogEntry, SegmentEnvelope},
};

impl Index {
    // ------------------------------------------------------------- segments

    /// Record a log entry that has not yet been sealed into a segment.
    ///
    /// Also advances the next-sequence counter, in the same transaction, so two
    /// entries can never be handed the same sequence number even across a crash.
    pub fn push_pending(&self, entry: &LogEntry) -> Result<()> {
        let encoded = postcard::to_stdvec(entry)?;
        let txn = self.db.begin_write()?;
        {
            let mut pending = txn.open_table(PENDING)?;
            let mut meta = txn.open_table(META)?;
            pending.insert(entry.sequence, encoded.as_slice())?;
            meta.insert(
                META_NEXT_SEQUENCE,
                (entry.sequence + 1).to_le_bytes().as_slice(),
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Entries waiting to be sealed, in sequence order.
    pub fn pending_entries(&self) -> Result<Vec<LogEntry>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(PENDING)?;

        let mut out = Vec::new();
        for row in table.iter()? {
            let (_, value) = row?;
            out.push(postcard::from_bytes(value.value())?);
        }
        Ok(out)
    }

    /// Append a segment to this device's chain and drop the pending entries it
    /// covers, atomically.
    ///
    /// Doing both in one transaction is the point. Committing the segment first
    /// and clearing pending afterwards would re-emit those entries in the next
    /// segment after a crash; clearing first would lose them outright.
    ///
    /// Rejects a segment whose `previous` does not match the current head, so
    /// the on-disk chain cannot develop a hole through a local bug.
    pub fn append_segment(&self, envelope: &SegmentEnvelope) -> Result<u64> {
        let encoded = envelope.encode()?;
        let txn = self.db.begin_write()?;
        let position;

        {
            let mut segments = txn.open_table(SEGMENTS)?;
            let mut pending = txn.open_table(PENDING)?;
            let mut meta = txn.open_table(META)?;

            for sequence in envelope.first_sequence..=envelope.last_sequence {
                pending.remove(sequence)?;
            }

            let head = Self::read_object_id(&meta, META_HEAD_SEGMENT)?;
            if envelope.previous != head {
                return Err(StoreError::SegmentChainBroken {
                    segment: envelope.segment_id.short(),
                    expected: head.map_or_else(|| "none".to_owned(), |id| id.short()),
                    found: envelope
                        .previous
                        .map_or_else(|| "none".to_owned(), |id| id.short()),
                });
            }

            position = Self::read_u64(&meta, META_CHAIN_LENGTH)?;
            segments.insert(position, encoded.as_slice())?;

            meta.insert(META_CHAIN_LENGTH, (position + 1).to_le_bytes().as_slice())?;
            meta.insert(META_HEAD_SEGMENT, envelope.segment_id.as_bytes().as_slice())?;
            // Never let the counter go backwards: entries may already be
            // pending beyond this segment's range, and reusing a sequence
            // number would fork the log.
            let next = Self::read_u64(&meta, META_NEXT_SEQUENCE)?.max(envelope.last_sequence + 1);
            meta.insert(META_NEXT_SEQUENCE, next.to_le_bytes().as_slice())?;
        }

        txn.commit()?;
        Ok(position)
    }

    /// The whole chain, oldest first.
    pub fn segments(&self) -> Result<Vec<SegmentEnvelope>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(SEGMENTS)?;

        let mut out = Vec::new();
        for row in table.iter()? {
            let (_, value) = row?;
            out.push(SegmentEnvelope::decode(value.value())?);
        }
        Ok(out)
    }

    /// The most recent segment this device wrote.
    pub fn head_segment(&self) -> Result<Option<ObjectId>> {
        let txn = self.db.begin_read()?;
        let meta = txn.open_table(META)?;
        Self::read_object_id(&meta, META_HEAD_SEGMENT)
    }

    /// The sequence number the next log entry should use.
    pub fn next_sequence(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let meta = txn.open_table(META)?;
        Ok(Self::read_u64(&meta, META_NEXT_SEQUENCE)?.max(1))
    }

    /// How many segments the chain holds.
    pub fn chain_length(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let meta = txn.open_table(META)?;
        Self::read_u64(&meta, META_CHAIN_LENGTH)
    }

    pub(super) fn read_u64(
        meta: &impl ReadableTable<&'static str, &'static [u8]>,
        key: &str,
    ) -> Result<u64> {
        match meta.get(key)? {
            Some(value) => {
                let bytes: [u8; 8] = value.value().try_into().map_err(|_| {
                    StoreError::Corrupt(format!("metadata key {key} is not 8 bytes"))
                })?;
                Ok(u64::from_le_bytes(bytes))
            }
            None => Ok(0),
        }
    }

    fn read_object_id(
        meta: &impl ReadableTable<&'static str, &'static [u8]>,
        key: &str,
    ) -> Result<Option<ObjectId>> {
        match meta.get(key)? {
            Some(value) => Ok(Some(ObjectId::from_slice(value.value())?)),
            None => Ok(None),
        }
    }

    // ------------------------------------------------------- applied heads

    /// Record that `device`'s chain was applied up to `head`, with nothing
    /// deferred.
    pub fn set_applied_head(&self, device: &DeviceId, head: &ObjectId) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(APPLIED)?
                .insert(device.as_bytes().as_slice(), head.as_bytes().as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Record that `device` signed a chain at least `length` segments long.
    /// A shorter word than one already held changes nothing, and writes
    /// nothing (an older presence read again every round).
    pub fn note_known_head(&self, device: &DeviceId, length: u64) -> Result<()> {
        let key = device.as_bytes().as_slice();
        let held = self
            .db
            .begin_read()?
            .open_table(KNOWN_HEADS)?
            .get(key)?
            .map_or(0, |row| row.value());
        if length <= held {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            txn.open_table(KNOWN_HEADS)?.insert(key, length)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Every device's chain length known from its own signed word.
    pub fn known_heads(&self) -> Result<Vec<(DeviceId, u64)>> {
        let txn = self.db.begin_read()?;
        let mut out = Vec::new();
        for row in txn.open_table(KNOWN_HEADS)?.iter()? {
            let (key, value) = row?;
            out.push((DeviceId::from_slice(key.value())?, value.value()));
        }
        Ok(out)
    }

    /// The head this device last applied cleanly for `device`, if any.
    pub fn applied_head(&self, device: &DeviceId) -> Result<Option<ObjectId>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(APPLIED)?;
        match table.get(device.as_bytes().as_slice())? {
            Some(value) => Ok(Some(ObjectId::from_slice(value.value())?)),
            None => Ok(None),
        }
    }
}
