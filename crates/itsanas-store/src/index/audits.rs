//! Audits: what each peer's record shows, when it was last heard from, the
//! probe a paused peer owes, and which chunks it is challenged on.

use itsanas_crypto::{ChunkId, DeviceId};
use redb::ReadableTable;

use super::{DEVICE_PASSED, DEVICE_SEEN, HOLDINGS, Index, PROBES, RELIABILITY, SEEN_STEP};
use crate::{
    error::Result,
    holders::{self, AuditCursor},
    reliability::Reliability,
};

impl Index {
    // ---------------------------------------------------------- reliability

    /// What auditing has shown about `device`.
    pub fn reliability(&self, device: &DeviceId) -> Result<Reliability> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(RELIABILITY)?;
        match table.get(device.as_bytes().as_slice())? {
            Some(value) => Ok(postcard::from_bytes(value.value())?),
            None => Ok(Reliability::default()),
        }
    }

    /// Record one audit outcome for `device`.
    ///
    /// A pass clears any outstanding probe in the same transaction: the probe
    /// exists to give a paused peer a question it can answer, and once it has
    /// answered one the sanction is over and the marker would only misdirect
    /// the next round's questions.
    /// Record that `device` answered something, whatever it was.
    ///
    /// Liveness, not correctness: a peer that answers and is wrong is recorded
    /// here and punished elsewhere.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    /// Also marks the hour in `device`'s uptime ring, in the same
    /// transaction: a ring written separately could miss the hour of a crash
    /// between the two and read a host as absent when it answered.
    pub fn note_seen(&self, device: &DeviceId, now: u64) -> Result<()> {
        self.note_seen_in(device, now)
    }

    /// This node's clock when `device` last answered, or `None`.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn last_seen(&self, device: &DeviceId) -> Result<Option<u64>> {
        let txn = self.db.begin_read()?;
        let seen = txn.open_table(DEVICE_SEEN)?;
        Ok(seen.get(device.as_bytes().as_slice())?.map(|v| v.value()))
    }

    /// Whether `device`'s contact row was stamped less than [`SEEN_STEP`]
    /// ago, and not in this node's future (a clock set back is rewritten).
    pub(crate) fn seen_within_step(&self, device: &DeviceId, now: u64) -> Result<bool> {
        let txn = self.db.begin_read()?;
        let seen = txn.open_table(DEVICE_SEEN)?;
        Ok(seen
            .get(device.as_bytes().as_slice())?
            .is_some_and(|row| row.value() <= now && now - row.value() < SEEN_STEP))
    }

    pub fn note_audit(&self, device: &DeviceId, passed: bool, now: u64) -> Result<Reliability> {
        let mut record = self.reliability(device)?;
        let was_paused = record.paused;
        if passed {
            // Proof counts for every device, the account's own included:
            // a phone's release needs its siblings' passes.
            record.passed_one();
        } else if self.is_member(device)? {
            record.forgive();
        } else {
            record.failed_one(now);
        }

        let txn = self.db.begin_write()?;
        {
            txn.open_table(RELIABILITY)?.insert(
                device.as_bytes().as_slice(),
                postcard::to_stdvec(&record)?.as_slice(),
            )?;
            // Answering at all is evidence the device exists, whether the
            // answer was right or wrong.
            txn.open_table(DEVICE_SEEN)?
                .insert(device.as_bytes().as_slice(), now)?;
            if passed {
                txn.open_table(PROBES)?
                    .remove(device.as_bytes().as_slice())?;
                // When, not only whether: release trusts a host that passed
                // lately, and `passed > 0` stays true for ever after one
                // pass years ago (red team, HIGH 2).
                txn.open_table(DEVICE_PASSED)?
                    .insert(device.as_bytes().as_slice(), now)?;
            }
            // Only the transition: a paused host's copies stop being worth
            // counting, and re-queueing on every further failure would walk
            // its records once per round for as long as it stays paused.
            if record.paused && !was_paused {
                Self::enqueue_device_in(&txn, device, now)?;
            }
        }
        txn.commit()?;
        Ok(record)
    }

    /// Note the one chunk `device` was offered while paused.
    pub fn note_probe(&self, device: &DeviceId, chunk: &ChunkId) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(PROBES)?
                .insert(device.as_bytes().as_slice(), chunk.as_bytes().as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// The chunk `device` was last offered while paused, if it still owes an
    /// answer on one.
    pub fn probe(&self, device: &DeviceId) -> Result<Option<ChunkId>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(PROBES)?;
        match table.get(device.as_bytes().as_slice())? {
            Some(value) => Ok(Some(ChunkId::from_slice(value.value())?)),
            None => Ok(None),
        }
    }

    /// Every device with something on its record, worst first.
    pub fn unreliable_devices(&self) -> Result<Vec<(DeviceId, Reliability)>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(RELIABILITY)?;

        let mut out = Vec::new();
        for row in table.iter()? {
            let (key, value) = row?;
            let device = DeviceId::from_slice(key.value())?;
            let record: Reliability = postcard::from_bytes(value.value())?;
            // A sanction written before the device was known as one of this
            // account's would otherwise stay on every screen for ever.
            if record.failed > 0 && !self.is_member(&device)? {
                out.push((device, record));
            }
        }
        out.sort_by_key(|(device, record)| {
            (
                std::cmp::Reverse(record.consecutive_failures),
                std::cmp::Reverse(record.failed),
                *device,
            )
        });
        Ok(out)
    }

    /// Chunks to challenge `device` on, one drawn per cursor.
    ///
    /// # Why this is a lottery and not "the least recently confirmed first"
    ///
    /// The first version sorted this device's holdings by when each was last
    /// confirmed and took the oldest. That reads like diligence and is worth
    /// nothing, because a push round re-stamps every record the peer *claims*
    /// to hold — a whole batch per clock reading — so within a batch every
    /// timestamp is equal and the sort falls through to its tie-break, the
    /// chunk id. The same sixteen chunks, the sixteen lowest ids, were
    /// challenged every round for ever. A host could keep sixteen chunks out of
    /// fourteen million, answer every question it was ever asked, and hold a
    /// spotless record while storing a millionth of what it promised.
    ///
    /// A proof of storage is worth exactly the host's inability to guess the
    /// question. So the question is drawn: each cursor seeks to the first record
    /// at or after it in this device's audit order, wrapping past the end.
    ///
    /// # The seek weights by gap, which is why the order is keyed
    ///
    /// Seeking to "the first record at or after a random point" does **not**
    /// pick uniformly. It picks each record with probability proportional to the
    /// gap before it, and gaps between random 256-bit values are exponentially
    /// distributed, so they are very uneven.
    ///
    /// That is harmless only if the host cannot tell which of its chunks sit
    /// behind the widest gaps. Ordered by chunk id it could: it received the
    /// chunks, so it knows every gap. Measured, at sixteen questions a round:
    ///
    /// | host keeps | discarding at random | discarding the narrow gaps |
    /// | --- | --- | --- |
    /// | 90% | passes 18% of rounds | passes **92%** |
    /// | 50% | passes 0.0015% | passes 6.9% |
    /// | 10% | passes 1e-16 | passes 2e-08 |
    ///
    /// A host silently losing a tenth of somebody's files would have been
    /// invisible, which is the failure this whole mechanism exists to catch.
    ///
    /// So the order is `BLAKE3_keyed(audit key, chunk)` — see
    /// [`holders::audit_tag`]. The gaps stay exactly as uneven and become
    /// unguessable, the host's best remaining strategy is to discard at random,
    /// and a host keeping a fraction `f` then passes a round of `n` questions
    /// with probability `f` to the power `n`.
    ///
    /// Coverage becomes probabilistic rather than exhaustive. That is the right
    /// trade and barely a trade at all: the exhaustive version was exhaustive
    /// over sixteen chunks.
    ///
    /// # Cost
    ///
    /// One B-tree seek per cursor. The version this replaces walked every
    /// holding of the device — fourteen million rows per audit at a terabyte,
    /// on the machine least able to afford it.
    ///
    /// Fewer chunks than cursors come back when the device holds fewer records
    /// than that, or when two cursors land on the same one; duplicates are
    /// dropped because asking the same question twice proves nothing twice.
    pub fn chunks_to_challenge(
        &self,
        device: &DeviceId,
        cursors: &[AuditCursor],
    ) -> Result<Vec<ChunkId>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(HOLDINGS)?;
        let start = holders::device_range_start(device);
        let end = holders::device_range_end(device);

        let mut picked: Vec<ChunkId> = Vec::with_capacity(cursors.len());
        for cursor in cursors {
            let from = holders::device_cursor(device, cursor);
            let mut hit = table
                .range(from.as_slice()..=end.as_slice())?
                .next()
                .transpose()?;
            if hit.is_none() {
                // Past this device's last record. A cursor that fell off the
                // end must wrap rather than be discarded, or the records at the
                // top of the order would be the only ones ever asked about and
                // the ones at the bottom would be asked about never.
                hit = table
                    .range(start.as_slice()..=end.as_slice())?
                    .next()
                    .transpose()?;
            }

            let Some((key, _)) = hit else {
                return Ok(picked); // this device holds nothing at all
            };
            if let Some((_, chunk)) = holders::split_by_device(key.value())
                && !picked.contains(&chunk)
            {
                picked.push(chunk);
            }
        }

        Ok(picked)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;
    use super::*;

    #[test]
    fn the_challenges_for_one_device_never_name_another_device_s_chunks() {
        let (_dir, index) = index();
        index.record_holder(&chunk(1), &device(7), 500).unwrap();
        index.record_holder(&chunk(2), &device(7), 100).unwrap();
        index.record_holder(&chunk(3), &device(8), 1).unwrap();

        // Every leading cursor byte, so this is a broad sweep rather than one
        // draw: no cursor may ever wander into a neighbour's range.
        let every: Vec<AuditCursor> = (0..=255u8).map(|b| [b; 8]).collect();
        let drawn = index.chunks_to_challenge(&device(7), &every).unwrap();
        assert_eq!(drawn.len(), 2, "two records, so at most two questions");
        assert!(drawn.contains(&chunk(1)) && drawn.contains(&chunk(2)));
        assert!(
            !drawn.contains(&chunk(3)),
            "a cursor reached across into device 8's holdings; auditing one peer \
             on another peer's records would fail an innocent host"
        );

        assert!(
            index
                .chunks_to_challenge(&device(9), &every)
                .unwrap()
                .is_empty(),
            "a device that holds nothing was asked something"
        );
        assert_eq!(
            index.chunks_to_challenge(&device(7), &[]).unwrap(),
            [] as [itsanas_crypto::ChunkId; 0]
        );
    }

    #[test]
    fn every_holding_is_reachable_by_some_cursor() {
        // The wrap matters: without it a cursor past the device's highest id
        // would be discarded, and the lowest-numbered chunks would be the only
        // ones never asked about — a hole an attacker could park its deletions
        // in.
        let (_dir, index) = index();
        let held: Vec<ChunkId> = [3u8, 40, 200].iter().map(|b| chunk(*b)).collect();
        index.record_holders(&held, &device(7), 1).unwrap();

        let mut seen = std::collections::BTreeSet::new();
        for byte in 0..=255u8 {
            for drawn in index.chunks_to_challenge(&device(7), &[[byte; 8]]).unwrap() {
                seen.insert(drawn);
            }
        }
        assert_eq!(
            seen.len(),
            held.len(),
            "some chunk this peer holds can never be challenged: {seen:?}"
        );
    }

    #[test]
    fn red_team_a_host_cannot_work_out_which_of_its_chunks_will_be_asked_about() {
        // THE ATTACK, and the one the first random version did not stop.
        //
        // Seeking to "the first record at or after a random cursor" does not
        // pick uniformly: each record is picked with probability proportional
        // to the GAP before it, and gaps between random ids are exponentially
        // distributed, so they are wildly uneven. Harmless only while the host
        // cannot tell which of its chunks sit behind the widest gaps.
        //
        // Ordered by chunk id it could: it received the chunks, so it knows
        // every gap. Simulated at sixteen questions a round, a host keeping the
        // best 90% of the data passed 92% of rounds where keeping 90% at random
        // passes 18%. Losing a tenth of somebody's files would have been
        // invisible — which is the failure the audit exists to catch.
        //
        // The fix is to order the ledger under a keyed hash. This is the check
        // that the key does the work: the audit order must not track the chunk
        // id. Ordered by id, every step of the walk goes upwards. Ordered by a
        // key the host does not have, about half do.
        let (_dir, index) = index();
        let holder = device(3);
        let chunks: Vec<ChunkId> = (0..=255u8).map(chunk).collect();
        index.record_holders(&chunks, &holder, 1).unwrap();

        let order = observed_audit_order(&index, &holder);
        assert!(
            order.len() > 100,
            "the sweep reached only {} records, too few to measure",
            order.len()
        );

        let ascending = share_ascending_by_id(&order);
        assert!(
            ascending < 0.75,
            "{:.0}% of audit steps go up the chunk-id order, so the audit order \
             tracks the id and a host can work out what will be asked. Ordered \
             by id this is 100%; independent of it, about 50%.",
            ascending * 100.0
        );
    }

    #[test]
    fn the_audit_order_changes_completely_when_the_key_does() {
        // Two owners must not share an audit order. If they did, a host could
        // learn the order from the challenges one owner sends it and apply that
        // knowledge to another owner's data.
        let dir = tempfile::tempdir().unwrap();
        let chunks: Vec<ChunkId> = (0..=255u8).map(chunk).collect();

        let walk = |name: &str, byte: u8| {
            let index = Index::open(
                dir.path().join(name),
                itsanas_crypto::SecretBytes::new([byte; 32]),
            )
            .unwrap();
            index.record_holders(&chunks, &device(4), 1).unwrap();
            observed_audit_order(&index, &device(4))
        };

        let one = walk("a.redb", 1);
        let two = walk("b.redb", 2);

        // Compare only what both sweeps reached, by position within that
        // shared set: coverage differs between the two orders and that is not
        // what is being measured.
        let common: std::collections::BTreeSet<ChunkId> =
            one.iter().filter(|c| two.contains(c)).copied().collect();
        let left: Vec<ChunkId> = one.iter().filter(|c| common.contains(c)).copied().collect();
        let right: Vec<ChunkId> = two.iter().filter(|c| common.contains(c)).copied().collect();
        assert!(left.len() > 100, "too little overlap to measure");

        let agreeing = left
            .iter()
            .zip(right.iter())
            .filter(|(a, b)| a == b)
            .count();
        assert!(
            agreeing * 4 < left.len(),
            "{agreeing} of {} shared positions agree between two audit keys, so \
             the order barely depends on the key",
            left.len()
        );
    }

    /// What the file looks like from outside: its size and when it was last
    /// written. Not its bytes -- Windows refuses to read a database another
    /// handle has locked -- and taken after a pause, so a write that follows
    /// moves the time even on a coarse filesystem clock.
    fn file_of(dir: &tempfile::TempDir) -> (u64, std::time::SystemTime) {
        let meta = std::fs::metadata(dir.path().join("index.redb")).expect("metadata");
        (meta.len(), meta.modified().expect("mtime"))
    }

    /// An idle round's contacts write nothing (ROADMAP, "An idle node's disk
    /// writes"): forgetting a loss that was never recorded, admitting a
    /// member already admitted, and hearing from a peer again within
    /// `SEEN_STEP` with nothing new for it. Each of these used to be a commit
    /// a round, some twenty kilobytes each on a copy-on-write engine -- on an
    /// SD card, the wear this project has already destroyed one of. And the
    /// stamp still moves once the step has passed, or liveness would freeze.
    /// Sabotaged (each read-before-write removed in turn): red.
    #[test]
    fn an_idle_round_of_contacts_commits_nothing_and_the_stamp_still_moves() {
        let (dir, index) = index();
        let peer = device(7);
        index.record_holders(&[chunk(1)], &peer, 1000).unwrap();
        index.admit_member(&peer, false).unwrap();
        index.note_seen(&peer, 1000).unwrap();
        let before = file_of(&dir);
        std::thread::sleep(std::time::Duration::from_millis(50));

        index.clear_loss(&chunk(1)).unwrap();
        assert!(
            file_of(&dir) == before,
            "forgetting a loss nobody recorded committed"
        );
        index.admit_member(&peer, false).unwrap();
        assert!(
            file_of(&dir) == before,
            "admitting a member already admitted committed"
        );
        index
            .record_holders(&[chunk(1)], &peer, 1000 + SEEN_STEP - 1)
            .unwrap();
        index.note_seen(&peer, 1000 + SEEN_STEP - 1).unwrap();
        assert!(
            file_of(&dir) == before,
            "a contact inside SEEN_STEP with nothing new committed: one commit per peer per round"
        );
        assert_eq!(index.last_seen(&peer).unwrap(), Some(1000));

        index
            .record_holders(&[chunk(1)], &peer, 1000 + SEEN_STEP)
            .unwrap();
        assert_eq!(
            index.last_seen(&peer).unwrap(),
            Some(1000 + SEEN_STEP),
            "the contact stamp no longer moves: a live peer would age into a dead one"
        );
        // A clock set back is written, not taken for fresh.
        index.record_holders(&[chunk(1)], &peer, 500).unwrap();
        assert_eq!(index.last_seen(&peer).unwrap(), Some(500));
        // And a real change still lands inside the step.
        index.note_loss(&chunk(1), 2000).unwrap();
        index.clear_loss(&chunk(1)).unwrap();
        assert_eq!(
            index.known_losses_from(&chunk(0), 10).unwrap(),
            Vec::<ChunkId>::new(),
            "a recorded loss was not forgotten"
        );
    }

    #[test]
    fn red_team_the_same_question_is_not_asked_twice_every_round() {
        // THE ATTACK: keep the sixteen chunks that will be asked about and
        // throw away the other fourteen million. It worked, for six commits.
        // The audit sorted by when each record was last confirmed and a push
        // round re-stamps a whole batch from one clock reading, so every
        // timestamp in the batch was equal and the sort fell through to its
        // tie-break — the chunk id. The same sixteen lowest ids, every round,
        // for ever.
        //
        // If this test fails, a host can store a millionth of what it promised
        // and hold a spotless audit record while doing it.
        let (_dir, index) = index();
        let all: Vec<ChunkId> = (0..=200u8).map(chunk).collect();
        index.record_holders(&all, &device(7), 1000).unwrap();

        // Ten rounds, each with its own draw, exactly as `session::audit` does.
        let mut rounds: Vec<Vec<ChunkId>> = Vec::new();
        for round in 0..10u8 {
            let cursors: Vec<AuditCursor> = (0..16u8)
                .map(|slot| {
                    // Stand-in for getrandom: any spread of cursors will do,
                    // what is being tested is that the *selection* follows them
                    // rather than ignoring them for a fixed order.
                    [round.wrapping_mul(37).wrapping_add(slot.wrapping_mul(11)); 8]
                })
                .collect();
            rounds.push(index.chunks_to_challenge(&device(7), &cursors).unwrap());
        }

        assert!(
            rounds.windows(2).any(|pair| pair[0] != pair[1]),
            "every round asked the identical set of questions, so a host only \
             has to keep the answers to those"
        );

        let union: std::collections::BTreeSet<ChunkId> = rounds.iter().flatten().copied().collect();
        assert!(
            union.len() > 16,
            "ten rounds of sixteen questions only ever touched {} chunks, so \
             the rest of the store is never checked",
            union.len()
        );
    }

    #[test]
    fn a_probe_is_remembered_until_the_peer_answers_for_it() {
        let (_dir, index) = index();
        assert_eq!(index.probe(&device(7)).unwrap(), None);

        index.note_probe(&device(7), &chunk(9)).unwrap();
        assert_eq!(index.probe(&device(7)).unwrap(), Some(chunk(9)));

        // A failure leaves it standing: the peer still owes an answer, and the
        // next round hands it a fresh one.
        index.note_audit(&device(7), false, 1).unwrap();
        assert_eq!(index.probe(&device(7)).unwrap(), Some(chunk(9)));

        // A pass ends the sanction, so the marker has nothing left to point at.
        index.note_audit(&device(7), true, 2).unwrap();
        assert_eq!(index.probe(&device(7)).unwrap(), None);
    }
}
