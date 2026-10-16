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
use itsanas_placement::Uptime;
use redb::{ReadableTable, ReadableTableMetadata, TableDefinition};

use super::{CHUNK_REFS, DEVICE_SEEN, HOLDERS, HOLDINGS, Index, META};
use crate::{error::Result, holders};

/// Device → postcard [`Uptime`]: the hours this node tried that device, and
/// the hours it saw it.
///
/// Postcard is not self-describing, so a change to `Uptime`'s fields needs a
/// new table name, as `peer_reliability_v2` did; a ring re-earns itself in a
/// week, which is cheaper than guessing at old bytes.
pub(super) const DEVICE_UPTIME: TableDefinition<'_, &[u8], &[u8]> =
    TableDefinition::new("device_uptime");

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

/// Add `chunk` to the queue if it is live and not already queued.
///
/// Keeping the first timestamp is the point (see [`SHORT_SINCE`]), so an
/// existing entry is left alone. A chunk outside `CHUNK_REFS` is refused
/// because nothing would ever settle it.
pub(super) fn enqueue_in(
    short: &mut redb::Table<'_, &'static [u8], u64>,
    refs: &impl ReadableTable<&'static [u8], u64>,
    chunk: &ChunkId,
    now: u64,
) -> Result<()> {
    let key = chunk.as_bytes().as_slice();
    if refs.get(key)?.is_none() || short.get(key)?.is_some() {
        return Ok(());
    }
    short.insert(key, now)?;
    Ok(())
}

/// Enqueue up to [`TRIGGER_LIMIT`] of the chunks `device` is recorded as
/// holding. Returns how many rows were looked at.
pub(super) fn enqueue_device(
    short: &mut redb::Table<'_, &'static [u8], u64>,
    refs: &impl ReadableTable<&'static [u8], u64>,
    holdings: &impl ReadableTable<&'static [u8], u64>,
    device: &DeviceId,
    now: u64,
) -> Result<usize> {
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

/// Note one observation of `device` in `now`'s hour, writing only on change.
///
/// Returns whether the ring was written. The caller decides whether the device
/// is known; this only spares a write when the hour is already marked, which
/// is every session after the first in an hour.
fn note_ring(
    rings: &mut redb::Table<'_, &'static [u8], &'static [u8]>,
    meta: &mut redb::Table<'_, &'static str, &'static [u8]>,
    device: &DeviceId,
    now: u64,
    seen: bool,
) -> Result<bool> {
    let hour = hour_of(now);
    let key = device.as_bytes().as_slice();
    let existing: Option<Uptime> = match rings.get(key)? {
        Some(value) => Some(postcard::from_bytes(value.value())?),
        None => None,
    };
    let fresh = existing.is_none();
    let mut ring = existing.unwrap_or_else(|| Uptime::new(hour));
    if !ring.note(hour, seen) && !fresh {
        return Ok(false);
    }
    rings.insert(key, postcard::to_stdvec(&ring)?.as_slice())?;
    let latest = Index::read_u64(meta, LATEST_HOUR)?;
    if hour > latest {
        meta.insert(LATEST_HOUR, hour.to_le_bytes().as_slice())?;
    }
    Ok(true)
}

/// Whether this node has had to do with `device`: a ring, or a contact.
fn known(
    rings: &impl ReadableTable<&'static [u8], &'static [u8]>,
    seen: &impl ReadableTable<&'static [u8], u64>,
    device: &DeviceId,
) -> Result<bool> {
    let key = device.as_bytes().as_slice();
    Ok(rings.get(key)?.is_some() || seen.get(key)?.is_some())
}

/// A page of the short queue: each chunk with when it was first found
/// short, and where to continue.
pub type ShortPage = (Vec<(ChunkId, u64)>, Option<ChunkId>);

/// The census's page: each chunk with every recorded holder and its stamp.
pub type CensusRow = (ChunkId, Vec<(DeviceId, u64)>);

impl Index {
    /// Record that `device` answered, in its ring and in `DEVICE_SEEN`, in one
    /// transaction.
    ///
    /// For sessions this node dialled or a peer it already knows: the caller
    /// vouches that `device` is somebody. Inbound sessions from anyone go
    /// through [`Index::note_seen_if_known`].
    pub(super) fn note_seen_in(&self, device: &DeviceId, now: u64) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(DEVICE_SEEN)?
                .insert(device.as_bytes().as_slice(), now)?;
            note_ring(
                &mut txn.open_table(DEVICE_UPTIME)?,
                &mut txn.open_table(META)?,
                device,
                now,
                true,
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Record that this node tried `device` in `now`'s hour and did not reach
    /// it. Nothing for a device it has never had to do with.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn note_unreached(&self, device: &DeviceId, now: u64) -> Result<bool> {
        self.note_if_known(device, now, false)
    }

    /// [`Index::note_seen`] for an inbound session: only for a device already
    /// known, so a stranger dialling in leaves no row. Returns whether it was.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn note_seen_if_known(&self, device: &DeviceId, now: u64) -> Result<bool> {
        self.note_if_known(device, now, true)
    }

    fn note_if_known(&self, device: &DeviceId, now: u64, seen: bool) -> Result<bool> {
        let txn = self.db.begin_write()?;
        let noted;
        {
            let mut rings = txn.open_table(DEVICE_UPTIME)?;
            let mut contact = txn.open_table(DEVICE_SEEN)?;
            noted = known(&rings, &contact, device)?;
            if noted {
                if seen {
                    contact.insert(device.as_bytes().as_slice(), now)?;
                }
                note_ring(&mut rings, &mut txn.open_table(META)?, device, now, seen)?;
            }
        }
        if noted {
            txn.commit()?;
        } else {
            txn.abort()?;
        }
        Ok(noted)
    }

    /// `device`'s ring, or `None` if this node has never observed it.
    ///
    /// # Errors
    ///
    /// If the index cannot be read, or the row does not decode.
    pub fn uptime(&self, device: &DeviceId) -> Result<Option<Uptime>> {
        let txn = self.db.begin_read()?;
        let rings = txn.open_table(DEVICE_UPTIME)?;
        match rings.get(device.as_bytes().as_slice())? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }

    /// Every ring, sorted by device. One row per device this node knows, which
    /// is what the roster is built from anyway.
    ///
    /// # Errors
    ///
    /// If the index cannot be read, or a row does not decode.
    pub fn uptimes(&self) -> Result<Vec<(DeviceId, Uptime)>> {
        let txn = self.db.begin_read()?;
        let rings = txn.open_table(DEVICE_UPTIME)?;
        let mut out = Vec::new();
        for row in rings.iter()? {
            let (key, value) = row?;
            out.push((
                DeviceId::from_slice(key.value())?,
                postcard::from_bytes(value.value())?,
            ));
        }
        Ok(out)
    }

    /// The highest hour any ring was written at; 0 before the first.
    ///
    /// A clock that now reads earlier than this has stepped back, and a
    /// release decided on it would read every ring as from the future.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn latest_hour_written(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        Self::read_u64(&txn.open_table(META)?, LATEST_HOUR)
    }

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
            let refs = txn.open_table(CHUNK_REFS)?;
            let mut short = txn.open_table(SHORT_SINCE)?;
            for chunk in chunks {
                enqueue_in(&mut short, &refs, chunk, now)?;
            }
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

    // ------------------------------------------------------------- released

    /// Record that this node released its claim on `device`'s copy of each of
    /// `chunks`, at `now`.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn note_released(&self, chunks: &[ChunkId], device: &DeviceId, now: u64) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            let mut released = txn.open_table(RELEASED)?;
            for chunk in chunks {
                released.insert(holders::key(chunk, device).as_slice(), now)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// When this node last released `device`'s copy of `chunk`, if it has.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn released_at(&self, chunk: &ChunkId, device: &DeviceId) -> Result<Option<u64>> {
        let txn = self.db.begin_read()?;
        Ok(txn
            .open_table(RELEASED)?
            .get(holders::key(chunk, device).as_slice())?
            .map(|value| value.value()))
    }

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
            &mut txn.open_table(SHORT_SINCE)?,
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
        for name in ["device_uptime", "short_since", "released"] {
            txn.delete_table(TableDefinition::<&[u8], &[u8]>::new(name))
                .unwrap();
        }
        txn.commit().unwrap();
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
    use super::*;
    use crate::{oplog::FileEntry, reliability::FAILURES_BEFORE_PAUSE};

    const HOUR: u64 = SECONDS_PER_HOUR;
    const T0: u64 = 1_800_000_000;

    fn index() -> (tempfile::TempDir, Index) {
        let dir = tempfile::tempdir().expect("temp dir");
        let index = Index::open(dir.path().join("index.redb"), key()).expect("open");
        (dir, index)
    }

    fn key() -> itsanas_crypto::SymmetricKey {
        itsanas_crypto::SecretBytes::new([0x5A; 32])
    }

    fn chunk(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    fn file(index: &Index, path: &str, chunks: &[ChunkId]) {
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

    fn queued(index: &Index) -> Vec<ChunkId> {
        let (page, next) = index.short_page(None, 10_000).expect("page");
        assert!(next.is_none());
        page.into_iter().map(|(chunk, _)| chunk).collect()
    }

    fn settle_everything(index: &Index) {
        index.settle_short(&queued(index)).expect("settle");
        assert!(queued(index).is_empty());
    }

    #[test]
    fn note_seen_writes_the_ring_and_the_contact_together() {
        let (_dir, index) = index();
        index.note_seen(&device(7), T0).expect("seen");
        let ring = index.uptime(&device(7)).expect("read").expect("a ring");
        assert_eq!(ring.seen_hours(), 1);
        assert_eq!(ring.last_seen_hour(), Some(T0 / HOUR));
        assert_eq!(index.last_seen(&device(7)).expect("read"), Some(T0));
        assert_eq!(index.latest_hour_written().expect("read"), T0 / HOUR);
    }

    #[test]
    fn a_ring_is_rewritten_only_when_the_observation_changes_it() {
        let (_dir, index) = index();
        let txn = index.db.begin_write().expect("txn");
        {
            let mut rings = txn.open_table(DEVICE_UPTIME).expect("rings");
            let mut meta = txn.open_table(META).expect("meta");
            let d = device(7);
            assert!(note_ring(&mut rings, &mut meta, &d, T0, true).expect("first"));
            assert!(
                !note_ring(&mut rings, &mut meta, &d, T0 + 60, true).expect("same hour"),
                "a second session in the same hour must not write"
            );
            assert!(note_ring(&mut rings, &mut meta, &d, T0 + HOUR, true).expect("next hour"));
            assert!(
                !note_ring(&mut rings, &mut meta, &d, T0, true).expect("step back"),
                "a clock stepping back changes nothing and writes nothing"
            );
        }
        txn.commit().expect("commit");
    }

    #[test]
    fn the_latest_hour_written_never_moves_back_with_the_clock() {
        let (_dir, index) = index();
        index.note_seen(&device(7), T0 + 5 * HOUR).expect("seen");
        index.note_seen(&device(8), T0).expect("seen");
        assert_eq!(index.latest_hour_written().expect("read"), T0 / HOUR + 5);
    }

    #[test]
    fn red_team_a_stranger_never_gets_a_ring() {
        let (_dir, index) = index();
        let stranger = device(9);
        for hour in 0..50 {
            assert!(
                !index
                    .note_unreached(&stranger, T0 + hour * HOUR)
                    .expect("unreached")
            );
            assert!(
                !index
                    .note_seen_if_known(&stranger, T0 + hour * HOUR)
                    .expect("inbound")
            );
        }
        assert_eq!(index.uptime(&stranger).expect("read"), None);
        assert_eq!(index.last_seen(&stranger).expect("read"), None);
        assert!(index.uptimes().expect("read").is_empty());
        assert_eq!(index.latest_hour_written().expect("read"), 0);
    }

    #[test]
    fn a_known_device_is_noted_reached_or_not() {
        let (_dir, index) = index();
        let known = device(3);
        index.note_seen(&known, T0).expect("seen");
        assert!(index.note_unreached(&known, T0 + HOUR).expect("unreached"));
        assert!(
            index
                .note_seen_if_known(&known, T0 + 2 * HOUR)
                .expect("inbound")
        );
        let ring = index.uptime(&known).expect("read").expect("ring");
        assert_eq!(ring.attempted_hours(), 3);
        assert_eq!(ring.seen_hours(), 2);
        assert_eq!(index.last_seen(&known).expect("read"), Some(T0 + 2 * HOUR));

        // Known by contact alone (a holder record), with no ring yet.
        index
            .record_holder(&chunk(1), &device(4), T0)
            .expect("record");
        assert!(
            index
                .note_unreached(&device(4), T0 + HOUR)
                .expect("unreached")
        );
        assert_eq!(
            index.uptimes().expect("all").len(),
            2,
            "one ring per known device"
        );
    }

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
        assert!(queued(&index).is_empty());
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
