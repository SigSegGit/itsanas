//! What availability-aware placement needs from the index: per-device uptime
//! rings, the queue of chunks that may have fallen short, the census cursor,
//! and the record of copies this node has released.
//!
//! Storage only. Nothing here decides how many copies a chunk needs or where
//! they go — that is `itsanas-placement`, which is pure and never touches a
//! file — and nothing here reads the coordinator: every ring is this device's
//! own observation, on its own clock.
//!
//! It refuses three things on purpose:
//!
//! - a ring for a device this node has never had to do with. An inbound
//!   session or an unreachable dial costs a stranger nothing, and a row per
//!   minted key would let anybody grow this table without bound;
//! - a queue entry for a chunk outside the live set, which nothing would ever
//!   settle or purge;
//! - an unbounded walk inside a trigger. A host that holds a million chunks
//!   and starts draining enqueues at most [`TRIGGER_LIMIT`] of them in the
//!   transaction that noticed; the census reaches the rest within one pass.

use itsanas_crypto::{ChunkId, DeviceId};
use redb::{ReadableTable, TableDefinition};

use super::{CHUNK_REFS, HOLDINGS, Index, META};
use crate::{error::Result, holders};

mod census;
mod released;
mod rings;
mod short_queue;

/// Device → postcard [`Uptime`]: the hours this node tried that device, and
/// the hours it saw it.
///
/// Postcard is not self-describing, so a change to `Uptime`'s fields needs a
/// new table name, as `peer_reliability_v2` did; a ring re-earns itself in a
/// week, which is cheaper than guessing at old bytes.
pub(super) const DEVICE_UPTIME: TableDefinition<'_, &[u8], &[u8]> =
    TableDefinition::new("device_uptime");

/// Device → [`Network::to_bytes`] ‖ this node's clock (little endian) when it
/// last saw that device from that network, over a direct session (§8 7c).
///
/// One row per device this node already knows, like the rings, and only a
/// prefix, never an address: a store that leaves the machine says which /24
/// a friend's house is on, not which host in it. Kept apart from the ring so a
/// change of network does not rewrite the ring's bytes.
pub(super) const DEVICE_NETWORK: TableDefinition<'_, &[u8], &[u8]> =
    TableDefinition::new("device_network");

/// Chunk → this node's clock when it was first found possibly short.
///
/// A queue, like the loss queue: something changed that may have cost a chunk
/// a copy (a holder forgotten, draining, paused, or the chunk is new), and the
/// push loop works through these before the census gets round to them. The
/// timestamp is kept from the first trigger, not refreshed, because the
/// safety net acts on how long a chunk has *stayed* short.
pub(super) const SHORT_SINCE: TableDefinition<'_, &[u8], u64> = TableDefinition::new("short_since");

/// `chunk ‖ device` → when this node released its claim on that copy.
///
/// Remembered so the same copy is not released twice within
/// [`holders::CONFIRMED_FOR`]: a host told twice to evict what it had already
/// re-stored would turn two owners' disagreement into a loop.
pub(super) const RELEASED: TableDefinition<'_, &[u8], u64> = TableDefinition::new("released");

/// Device → this node's clock when that device last passed a storage audit.
///
/// What release's strict evidence reads: a host that passed once, long ago,
/// keeps `Reliability.passed > 0` for ever, and a copy released on that is
/// released on a guess.
pub(super) const DEVICE_PASSED: TableDefinition<'_, &[u8], u64> =
    TableDefinition::new("device_passed");

/// `META` key: the last chunk the census returned (exclusive cursor).
const CENSUS_CURSOR: &str = "census_cursor";
/// `META` key: the highest hour any ring was written at.
const LATEST_HOUR: &str = "uptime_latest_hour";

/// Most of a device's records one trigger enqueues inside its transaction.
///
/// A trigger runs inside a transaction somebody else is waiting on (a drain
/// request, an audit answer). Walking every record of a host that holds a
/// terabyte for us would hold that transaction for sixteen million rows; this
/// caps it, and the census covers whatever was left out.
pub const TRIGGER_LIMIT: usize = 4096;

const SECONDS_PER_HOUR: u64 = 3600;

/// The hour `now` falls in, by this node's clock.
const fn hour_of(now: u64) -> u64 {
    now / SECONDS_PER_HOUR
}

/// `META` key: set (1) while the daemon runs `placement = measured`.
const QUEUE_ARMED: &str = "placement_measured";

/// The short queue, opened in a write transaction together with whether it
/// may grow.
///
/// Every trigger writes through this, so one flag decides them all. In
/// `everyone` nothing ever works the queue off, and a queue that only grows
/// (a row per new chunk, per forgotten record) is a table the size of the
/// account kept for nothing (red team, 5a). Removals are always allowed.
pub(super) struct ShortQueue<'txn> {
    table: redb::Table<'txn, &'static [u8], u64>,
    armed: bool,
}

impl<'txn> ShortQueue<'txn> {
    /// Open the queue in `txn`. `META` is read and closed again before the
    /// queue is opened, so a caller may open `META` itself afterwards.
    pub(super) fn open(txn: &'txn redb::WriteTransaction) -> Result<Self> {
        let armed = Index::read_u64(&txn.open_table(META)?, QUEUE_ARMED)? == 1;
        Ok(Self {
            table: txn.open_table(SHORT_SINCE)?,
            armed,
        })
    }

    /// Queue `key` at `now` unless it is already queued or the queue is not
    /// armed.
    pub(super) fn insert_new(&mut self, key: &[u8], now: u64) -> Result<()> {
        if self.armed && self.table.get(key)?.is_none() {
            self.table.insert(key, now)?;
        }
        Ok(())
    }
}

/// Add `chunk` to the queue if it is live and not already queued.
///
/// Keeping the first timestamp is the point (see [`SHORT_SINCE`]), so an
/// existing entry is left alone. A chunk outside `CHUNK_REFS` is refused
/// because nothing would ever settle it.
pub(super) fn enqueue_in(
    short: &mut ShortQueue<'_>,
    refs: &impl ReadableTable<&'static [u8], u64>,
    chunk: &ChunkId,
    now: u64,
) -> Result<()> {
    let key = chunk.as_bytes().as_slice();
    if !short.armed || refs.get(key)?.is_none() {
        return Ok(());
    }
    short.insert_new(key, now)
}

/// Enqueue up to [`TRIGGER_LIMIT`] of the chunks `device` is recorded as
/// holding. Returns how many rows were looked at.
pub(super) fn enqueue_device(
    short: &mut ShortQueue<'_>,
    refs: &impl ReadableTable<&'static [u8], u64>,
    holdings: &impl ReadableTable<&'static [u8], u64>,
    device: &DeviceId,
    now: u64,
) -> Result<usize> {
    if !short.armed {
        return Ok(0);
    }
    let mut seen = 0;
    for row in holdings.range(
        holders::device_range_start(device).as_slice()
            ..=holders::device_range_end(device).as_slice(),
    )? {
        let (key, _) = row?;
        if let Some((_, chunk)) = holders::split_by_device(key.value()) {
            enqueue_in(short, refs, &chunk, now)?;
        }
        seen += 1;
        if seen == TRIGGER_LIMIT {
            break;
        }
    }
    Ok(seen)
}

/// A page of the short queue: each chunk with when it was first found
/// short, and where to continue.
pub type ShortPage = (Vec<(ChunkId, u64)>, Option<ChunkId>);

/// The census's page: each chunk with every recorded holder and its stamp.
pub type CensusRow = (ChunkId, Vec<(DeviceId, u64)>);

impl Index {
    /// Drop the queue entry and release records of a chunk leaving the live
    /// set, inside the caller's transaction.
    pub(super) fn purge_chunk_in(txn: &redb::WriteTransaction, chunk: &ChunkId) -> Result<()> {
        txn.open_table(SHORT_SINCE)?
            .remove(chunk.as_bytes().as_slice())?;
        let mut released = txn.open_table(RELEASED)?;
        let doomed: Vec<Vec<u8>> = released
            .range(holders::range_start(chunk).as_slice()..=holders::range_end(chunk).as_slice())?
            .filter_map(|row| row.ok().map(|(key, _)| key.value().to_vec()))
            .collect();
        for key in doomed {
            released.remove(key.as_slice())?;
        }
        Ok(())
    }

    /// Enqueue `device`'s recorded chunks inside the caller's transaction.
    pub(super) fn enqueue_device_in(
        txn: &redb::WriteTransaction,
        device: &DeviceId,
        now: u64,
    ) -> Result<usize> {
        enqueue_device(
            &mut ShortQueue::open(txn)?,
            &txn.open_table(CHUNK_REFS)?,
            &txn.open_table(HOLDINGS)?,
            device,
            now,
        )
    }

    /// Drop the placement tables, to stand in for a store written before
    /// them. Only the open-time creation test calls this.
    #[cfg(test)]
    pub(super) fn drop_placement_tables_for_test(&self) {
        let txn = self.db.begin_write().unwrap();
        for name in ["device_uptime", "short_since", "released", "device_passed"] {
            txn.delete_table(TableDefinition::<&[u8], &[u8]>::new(name))
                .unwrap();
        }
        txn.commit().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oplog::FileEntry;

    pub(super) const HOUR: u64 = SECONDS_PER_HOUR;
    pub(super) const T0: u64 = 1_800_000_000;

    pub(super) fn index() -> (tempfile::TempDir, Index) {
        let dir = tempfile::tempdir().expect("temp dir");
        let index = Index::open(dir.path().join("index.redb"), key()).expect("open");
        // Armed, as `measured` arms it: most tests here are about what the
        // triggers queue.
        index.set_placement_measured(true).expect("arm");
        (dir, index)
    }

    pub(super) fn key() -> itsanas_crypto::SymmetricKey {
        itsanas_crypto::SecretBytes::new([0x5A; 32])
    }

    pub(super) fn chunk(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    pub(super) fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    pub(super) fn file(index: &Index, path: &str, chunks: &[ChunkId]) {
        let entry = FileEntry {
            size: 10,
            modified_unix: 1_700_000_000,
            content_hash: [0; 32],
            chunks: chunks.to_vec(),
            version: crate::version::VersionVector::new(),
            author: device(1),
        };
        index.put_file(path, &entry).expect("put");
    }

    pub(super) fn queued(index: &Index) -> Vec<ChunkId> {
        let (page, next) = index.short_page(None, 10_000).expect("page");
        assert!(next.is_none());
        page.into_iter().map(|(chunk, _)| chunk).collect()
    }

    pub(super) fn settle_everything(index: &Index) {
        index.settle_short(&queued(index)).expect("settle");
        assert_eq!(queued(index), [] as [itsanas_crypto::ChunkId; 0]);
    }

    #[test]
    fn collecting_a_chunk_takes_it_off_the_queue_and_the_release_records() {
        let (_dir, index) = index();
        file(&index, "a", &[chunk(1), chunk(2), chunk(3)]);
        index
            .note_released(&[chunk(1), chunk(2)], &device(5), T0)
            .expect("release");
        index.forget_chunk(&chunk(1)).expect("gc");
        index.release_chunk(&chunk(3)).expect("let go");
        assert_eq!(queued(&index), vec![chunk(2)]);
        assert_eq!(
            index.released_at(&chunk(1), &device(5)).expect("read"),
            None
        );
        assert_eq!(
            index.released_at(&chunk(2), &device(5)).expect("read"),
            Some(T0)
        );
    }

    #[test]
    fn the_placement_tables_are_created_on_open_of_a_store_written_before_them() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("index.redb");
        {
            let index = Index::open(&path, key()).expect("open");
            index.drop_placement_tables_for_test();
        }
        let index = Index::open(&path, key()).expect("reopen");
        assert_eq!(index.uptime(&device(1)).expect("rings"), None);
        assert_eq!(index.short_count().expect("queue"), 0);
        assert_eq!(
            index.released_at(&chunk(1), &device(1)).expect("released"),
            None
        );
    }

    #[test]
    fn rings_queue_and_releases_survive_reopening() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("index.redb");
        {
            let index = Index::open(&path, key()).expect("open");
            index.set_placement_measured(true).expect("arm");
            file(&index, "a", &[chunk(1)]);
            index.note_seen(&device(2), T0).expect("seen");
            index
                .note_released(&[chunk(1)], &device(2), T0)
                .expect("released");
        }
        let index = Index::open(&path, key()).expect("reopen");
        assert!(index.uptime(&device(2)).expect("ring").is_some());
        assert_eq!(queued(&index), vec![chunk(1)]);
        assert_eq!(
            index.released_at(&chunk(1), &device(2)).expect("read"),
            Some(T0)
        );
    }
}
