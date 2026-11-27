//! Coverage: what the ledger can say about how many copies each chunk has
//! elsewhere, and which chunks are closest to being lost.

use std::collections::BTreeMap;

use itsanas_crypto::{ChunkId, DeviceId};
use redb::ReadableTable;

use super::{CHUNK_REFS, DEVICE_SEEN, DRAINING, HOLDERS, Index, RELIABILITY, counted_in};
use crate::{
    error::Result,
    holders::{self, AtRisk, Coverage, HolderEvidence},
    reliability::Reliability,
};

impl Index {
    /// What this node can actually say about who else holds `chunk`.
    ///
    /// Two numbers, because they answer two different questions and conflating
    /// them is how a release comes to rest on a belief.
    ///
    /// `live` counts holders whose **machine** has been heard from within
    /// [`holders::CONFIRMED_FOR`] — the same rule [`Index::coverage`] uses. It
    /// says the holder still exists.
    ///
    /// `fresh` counts holders whose record **for this chunk** was refreshed
    /// within the same window — by a successful audit, or by that peer
    /// answering "I am not missing it" during a push. It says somebody
    /// answered about *this* chunk, not merely that the machine picked up the
    /// phone about something else.
    ///
    /// The distinction was invisible until content could be released. A
    /// per-machine rule cannot tell a peer that still has your data from one
    /// that emptied its disk and stayed online, and the one operation that
    /// destroys data on the strength of these numbers is exactly the one that
    /// should not be told they are the same.
    pub fn holder_evidence(&self, chunk: &ChunkId, now: u64) -> Result<HolderEvidence> {
        let fresh_since = now.saturating_sub(holders::CONFIRMED_FOR);
        let txn = self.db.begin_read()?;
        let seen = txn.open_table(DEVICE_SEEN)?;
        let holders_table = txn.open_table(HOLDERS)?;
        let reliability = txn.open_table(RELIABILITY)?;

        let mut evidence = HolderEvidence::default();
        for row in holders_table
            .range(holders::range_start(chunk).as_slice()..=holders::range_end(chunk).as_slice())?
        {
            let (key, value) = row?;
            let Some((_, device)) = holders::split(key.value()) else {
                continue;
            };

            let last_seen = seen
                .get(device.as_bytes().as_slice())?
                .map_or(0, |v| v.value());
            if last_seen < fresh_since {
                continue;
            }
            evidence.live += 1;

            // The timestamp on the record itself, which is written every time
            // that peer confirms this chunk and was never read until now.
            if value.value() >= fresh_since {
                evidence.fresh += 1;
            }

            // Has this device ever answered a challenge correctly? `passed` was
            // written from the first audit and marked "diagnostics only"; it is
            // the only thing in the ledger a liar cannot produce by talking.
            // A device with no row at all -- which is every stranger that has
            // only ever dialled in -- reads as zero.
            let proved = reliability
                .get(device.as_bytes().as_slice())?
                .map(|value| postcard::from_bytes::<Reliability>(value.value()))
                .transpose()?
                .is_some_and(|record| record.vouches());
            // And is still about: an audit passed last month says nothing of a
            // machine wiped last week. Uninstalling a phone tells nobody, and
            // two such holders must not let the owner drop the last real copy.
            let audible_since = now.saturating_sub(holders::LIVE_FOR);
            if proved && last_seen >= audible_since && value.value() >= audible_since {
                evidence.proved += 1;
            }
        }
        Ok(evidence)
    }

    /// Live chunks held by fewer than `target` devices, worst first.
    ///
    /// **`target` counts this device.** A target of three asks for two remote
    /// holders, because the copy on this disk is the third. This is the one
    /// place the two are added up, and it is here rather than at every call
    /// site so that no caller has to remember to do it.
    ///
    /// Ordered by how close each chunk is to being lost rather than by chunk
    /// id, so a repair pass that is interrupted — a laptop closing, a Pi
    /// rebooting — has spent its time on the chunks with the least margin.
    /// How many complete copies of this node's data exist on other machines.
    ///
    /// The minimum holder count over every live chunk, not counting this
    /// machine. See [`Coverage`] for why a minimum rather than an average, and
    /// why this machine does not count.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn coverage(&self, now: u64) -> Result<Coverage> {
        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;
        let holders_table = txn.open_table(HOLDERS)?;

        // Anything from a device nobody has heard from since this is a memory,
        // not an observation.
        //
        // Per *device*, not per chunk, and the arithmetic on DEVICE_SEEN says
        // why: the audit re-checks sixteen chunks per peer per round, so a
        // per-chunk rule collapses to "no copies" on any account past a few
        // gigabytes, however healthy. Liveness is asked of the machine; whether
        // it still holds a particular chunk is the audit's job, and a failed
        // challenge removes that holder outright.
        let fresh_since = now.saturating_sub(holders::CONFIRMED_FOR);
        let seen_table = txn.open_table(DEVICE_SEEN)?;
        let draining = txn.open_table(DRAINING)?;
        let reliability = txn.open_table(RELIABILITY)?;
        let mut alive: BTreeMap<DeviceId, bool> = BTreeMap::new();

        let mut live_chunks = 0usize;
        let mut only_here = 0usize;
        let mut fewest: Option<usize> = None;
        let mut fewest_claimed: Option<usize> = None;
        let mut stale_records = 0usize;
        // How much of this account each other machine holds. The largest entry
        // is the one worth reporting: it is the closest anybody else is to
        // having a whole copy.
        let mut per_device: BTreeMap<DeviceId, usize> = BTreeMap::new();

        for row in refs.iter()? {
            let (key, value) = row?;
            if value.value() == 0 {
                continue;
            }
            live_chunks += 1;

            let chunk = ChunkId::from_slice(key.value())?;
            let mut confirmed = 0usize;
            let mut claimed = 0usize;
            for row in holders_table.range(
                holders::range_start(&chunk).as_slice()..=holders::range_end(&chunk).as_slice(),
            )? {
                let (key, _) = row?;
                claimed += 1;

                let Some(device) = holders::device_from_key(key.value()) else {
                    stale_records += 1;
                    continue;
                };

                // Cached per device: a chunk held by twenty machines would
                // otherwise cost twenty lookups of the same row.
                let live = if let Some(live) = alive.get(&device) {
                    *live
                } else {
                    let seen = seen_table
                        .get(device.as_bytes().as_slice())?
                        .map_or(0, |v| v.value());
                    // A draining device is still there but must not be
                    // counted on: it asked for its copies to be replaced.
                    let live =
                        seen >= fresh_since && counted_in(&draining, &reliability, &device, now)?;
                    alive.insert(device, live);
                    live
                };

                if !live {
                    stale_records += 1;
                    continue;
                }
                confirmed += 1;

                // Only live holders count towards concentration too. A machine
                // that has gone quiet is not one that holds a share of you; it
                // is one nobody can say anything about.
                *per_device.entry(device).or_default() += 1;
            }

            if confirmed == 0 {
                only_here += 1;
            }
            fewest = Some(fewest.map_or(confirmed, |least: usize| least.min(confirmed)));
            fewest_claimed =
                Some(fewest_claimed.map_or(claimed, |least: usize| least.min(claimed)));
        }

        Ok(Coverage {
            complete_elsewhere: fewest.unwrap_or(0),
            claimed_elsewhere: fewest_claimed.unwrap_or(0),
            stale_records,
            live_chunks,
            only_here,
            distinct_holders: per_device.len(),
            largest_share: per_device.into_values().max().unwrap_or(0),
        })
    }

    /// Ordering by id would make the work random with respect to risk.
    pub fn under_replicated(&self, target: usize, now: u64) -> Result<Vec<AtRisk>> {
        // The window that decides whether a record stands in for a live copy.
        // Counting every record whatever its age -- which this did until
        // 2026-09-21 -- means a machine that died six months ago still counts
        // as one of your three copies, so repair never fires. Its records are
        // only withdrawn by a **failed audit**, and an audit needs that machine
        // to answer; a dead one never does. The ledger was optimistic in the
        // one direction that loses data.
        let live_since = now.saturating_sub(holders::LIVE_FOR);
        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;
        let holders_table = txn.open_table(HOLDERS)?;
        let draining = txn.open_table(DRAINING)?;
        let reliability = txn.open_table(RELIABILITY)?;

        let mut out = Vec::new();
        for row in refs.iter()? {
            let (key, value) = row?;
            if value.value() == 0 {
                continue;
            }
            let chunk = ChunkId::from_slice(key.value())?;
            let mut held_by = 0usize;
            for holder in holders_table.range(
                holders::range_start(&chunk).as_slice()..=holders::range_end(&chunk).as_slice(),
            )? {
                let (key, confirmed) = holder?;
                // A device draining still holds the chunk, and that is the
                // point: repair runs while its copy is a margin, not a count.
                if let Some(device) = holders::device_from_key(key.value())
                    && !counted_in(&draining, &reliability, &device, now)?
                {
                    continue;
                }
                // The timestamp is written every time that peer confirms this
                // chunk, which is what `Index::holder_evidence` calls `fresh`:
                // somebody answered about *this* chunk, not merely that the
                // machine picked up the phone about something else.
                if confirmed.value() >= live_since {
                    held_by += 1;
                }
            }
            // The copy on this disk. Referenced chunks are the ones this
            // node's own files use, so the blob is here unless a pull left the
            // entry ahead of its data — which `doctor` reports separately, and
            // which would make this an under-count rather than an over-count.
            let held_by = held_by + 1;
            if held_by < target {
                out.push(AtRisk {
                    chunk,
                    held_by,
                    target,
                });
            }
        }

        out.sort_by_key(|risk| (risk.held_by, risk.chunk));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;
    use super::*;

    #[test]
    fn a_target_counts_this_device_so_three_asks_for_two_elsewhere() {
        // The counting convention, pinned. Getting this off by one means the
        // repair loop targets two copies while reporting three, and nothing
        // ever says so — it shows up as data loss after two machines die
        // instead of after three.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1])).unwrap();

        assert_eq!(
            index.under_replicated(3, 1).unwrap()[0].held_by,
            1,
            "with no remote holders, the local copy is the only one"
        );

        index.record_holder(&chunk(1), &device(7), 1).unwrap();
        assert_eq!(index.under_replicated(3, 1).unwrap()[0].held_by, 2);

        index.record_holder(&chunk(1), &device(8), 1).unwrap();
        assert!(
            index.under_replicated(3, 1).unwrap().is_empty(),
            "two remote holders plus this device meets a target of three"
        );
    }

    #[test]
    fn one_chunk_nobody_holds_makes_the_whole_account_unrecoverable() {
        // The reason this is a minimum and not an average, in one test.
        //
        // A file comes back only if every one of its chunks does. So an account
        // whose chunks are almost all on three other machines, with one on
        // none, has ZERO complete copies elsewhere -- not "nearly three". An
        // average would report 2.0 here and read as comfortable.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1, 2, 3])).unwrap();

        for id in [1u8, 2] {
            index.record_holder(&chunk(id), &device(7), 1).unwrap();
            index.record_holder(&chunk(id), &device(8), 1).unwrap();
            index.record_holder(&chunk(id), &device(9), 1).unwrap();
        }
        // Chunk 3 is held by nobody.

        let coverage = index.coverage(1).unwrap();
        assert_eq!(
            coverage.complete_elsewhere, 0,
            concat!(
                "eight of nine holder records exist, and not one complete copy: ",
                "the account cannot be rebuilt without this machine"
            )
        );
        assert_eq!(coverage.only_here, 1);
        assert_eq!(coverage.live_chunks, 3);
        assert!(!coverage.meets(2));

        // And the moment that last chunk is placed once, one copy exists.
        index.record_holder(&chunk(3), &device(7), 1).unwrap();
        let coverage = index.coverage(1).unwrap();
        assert_eq!(coverage.complete_elsewhere, 1);
        assert_eq!(coverage.only_here, 0);
        assert!(!coverage.meets(2), "one copy is not two");

        // Two holders for the weakest chunk is two complete copies, whatever
        // the others have.
        index.record_holder(&chunk(3), &device(8), 1).unwrap();
        assert!(index.coverage(1).unwrap().meets(2));
    }

    #[test]
    fn a_peer_that_holds_every_chunk_is_reported_as_holding_everything() {
        // Two things this measures, and they pull in opposite directions.
        //
        // A peer holding a complete set is one broken cipher away from reading
        // the account. Past a certain size the goal is that nobody but the
        // owner ever holds a whole one -- and it is also what decides whether
        // this scales at all, because if the unit of hosting is "a complete
        // copy" then somebody offering four terabytes needs peers who can each
        // take four terabytes.
        //
        // On a small network it is not yet a failure: with two peers you cannot
        // have two copies and no complete holder at once. It is reported so
        // that it is noticed, because nobody notices a property they are never
        // shown.
        let (_dir, index) = fresh();
        index.put_file("a", &entry(&[1, 2, 3])).unwrap();

        for id in [1u8, 2, 3] {
            index.record_holder(&chunk(id), &device(7), 1).unwrap();
        }
        let coverage = index.coverage(1).unwrap();
        assert_eq!(coverage.largest_share, 3);
        assert!(
            coverage.someone_holds_everything(),
            "one peer holds all three chunks and that was not reported"
        );

        // Spread instead: each chunk on a different machine. Same number of
        // holder records, same one complete copy -- and now nobody has a whole
        // set of anything.
        let (_spread_dir, spread) = fresh();
        spread.put_file("a", &entry(&[1, 2, 3])).unwrap();
        spread.record_holder(&chunk(1), &device(7), 1).unwrap();
        spread.record_holder(&chunk(2), &device(8), 1).unwrap();
        spread.record_holder(&chunk(3), &device(9), 1).unwrap();

        let coverage = spread.coverage(1).unwrap();
        assert_eq!(
            coverage.complete_elsewhere, 1,
            "the account is still reconstructible from the network"
        );
        assert_eq!(
            coverage.largest_share, 1,
            "no machine holds more than its share"
        );
        assert!(!coverage.someone_holds_everything());
    }

    #[test]
    fn this_machine_is_not_one_of_the_copies() {
        // The question is what survives losing this machine. Counting it is how
        // a backup report comes to say two when the answer is one.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1])).unwrap();

        assert_eq!(
            index.coverage(1).unwrap().complete_elsewhere,
            0,
            concat!(
                "with no remote holder there is no copy elsewhere, though the ",
                "data is perfectly safe on this disk right now"
            )
        );
        assert_eq!(
            index.under_replicated(3, 1).unwrap()[0].held_by,
            1,
            concat!(
                "under_replicated counts this machine, and deliberately: the ",
                "two measurements answer different questions"
            )
        );
    }

    #[test]
    fn a_holder_nobody_has_heard_from_stops_counting_as_a_copy() {
        // The failure this prevents was live in this project's own fleet on
        // 2026-09-06: a device that had been destroyed was still listed as a
        // holder, and was removed by hand only because somebody read a log.
        //
        // A holder record is a memory. It says a device once acknowledged this
        // chunk; it says nothing about whether that device still exists. Counted
        // as a copy, it is how a backup report stays green after the machine it
        // describes has been thrown away.
        let (_dir, index) = fresh();
        index.put_file("a", &entry(&[1, 2])).unwrap();

        let long_ago = 1_000;
        let now = long_ago + holders::CONFIRMED_FOR + 1;

        // Two holders each, but one of them answered a fortnight and a second
        // ago and has not been heard from since.
        for id in [1u8, 2] {
            index.record_holder(&chunk(id), &device(7), now).unwrap();
            index
                .record_holder(&chunk(id), &device(8), long_ago)
                .unwrap();
        }

        let coverage = index.coverage(now).unwrap();
        assert_eq!(
            coverage.claimed_elsewhere, 2,
            "the optimistic count is what the ledger remembers"
        );
        assert_eq!(
            coverage.complete_elsewhere, 1,
            "only one holder has been heard from, so there is one copy"
        );
        assert_eq!(coverage.stale_records, 2);
        assert!(coverage.resting_on_memory());
        assert!(
            !coverage.meets(2),
            "two claims and one observation is not two copies"
        );

        // A stale holder is not part of anybody's share either: a machine that
        // has gone quiet is not one that holds a third of you, it is one
        // nobody can say anything about.
        assert_eq!(coverage.largest_share, 2);
        assert_eq!(coverage.distinct_holders, 1);
    }

    #[test]
    fn a_big_account_does_not_report_itself_lost_because_the_audit_is_slow() {
        // The arithmetic that made a per-chunk freshness rule wrong, in a test.
        //
        // The audit re-checks sixteen chunks per peer per round at three
        // hundred seconds a round, so a chunk waits `chunks / 16` rounds for its
        // turn -- past about sixty-four thousand chunks, four or five
        // gigabytes, longer than the fortnight. A rule that asked "was THIS
        // CHUNK confirmed recently" would report zero complete copies for a
        // fleet whose every audit passes, at exactly the size this project is
        // for.
        //
        // So the question is asked of the machine: acknowledgements recorded
        // long ago still count while the device that made them is answering.
        let (_dir, index) = fresh();
        index.put_file("a", &entry(&[1, 2, 3])).unwrap();

        let long_ago = 1_000;
        let now = long_ago + holders::CONFIRMED_FOR * 10;

        for id in [1u8, 2, 3] {
            index
                .record_holder(&chunk(id), &device(7), long_ago)
                .unwrap();
        }
        // Every acknowledgement is ancient.
        assert_eq!(
            index.coverage(now).unwrap().complete_elsewhere,
            0,
            "a device nobody has heard from must not count"
        );

        // The device answers one challenge now. Nothing about the chunks
        // changed; what changed is that the machine is known to be there.
        index.note_audit(&device(7), true, now).unwrap();

        let coverage = index.coverage(now).unwrap();
        assert_eq!(
            coverage.complete_elsewhere, 1,
            concat!(
                "acknowledgements from a live device stopped counting because ",
                "they were old, which is the bug that breaks every account over ",
                "a few gigabytes"
            )
        );
        assert_eq!(coverage.stale_records, 0);
        assert!(!coverage.resting_on_memory());
    }

    #[test]
    fn an_account_with_nothing_stored_is_not_reported_as_unsafe() {
        // Zero data is no question, not a failure. Reporting "0 copies" for an
        // empty account would train somebody to ignore the number that matters.
        let (_dir, index) = index();
        let coverage = index.coverage(1).unwrap();
        assert_eq!(coverage.live_chunks, 0);
        assert!(coverage.meets(2));
    }

    #[test]
    fn red_team_a_draining_holder_stops_counting_so_repair_starts_before_it_goes() {
        // The drain of §8 0w (7): a machine about to be removed asks its owners
        // to stop counting on it. If its records still counted, repair would
        // wait for `Leaving`, and the window with one replica fewer would open
        // at the moment the machine is switched off instead of closing before.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1, 2])).unwrap();
        let now = 10_000;
        for id in [1u8, 2] {
            index.record_holder(&chunk(id), &device(7), now).unwrap();
        }
        index.record_holder(&chunk(1), &device(8), now).unwrap();
        index.note_audit(&device(7), true, now).unwrap();
        index.note_audit(&device(8), true, now).unwrap();
        assert_eq!(index.under_replicated(2, now).unwrap(), Vec::new());
        assert_eq!(index.coverage(now).unwrap().complete_elsewhere, 1);

        index.note_draining(&device(7), now + 600, now).unwrap();

        let at_risk: Vec<ChunkId> = index
            .under_replicated(2, now)
            .unwrap()
            .into_iter()
            .map(|risk| risk.chunk)
            .collect();
        assert_eq!(
            at_risk,
            vec![chunk(2)],
            "a draining machine still counted as a holder, so nothing is re-placed \
             before it leaves and its departure costs a replica"
        );
        assert_eq!(
            index.coverage(now).unwrap().complete_elsewhere,
            0,
            "`status` counted a draining machine as a copy it is about to lose"
        );
        assert_eq!(
            index.exposed_by(&device(7), now).unwrap(),
            1,
            "only chunk 2 rests on the draining machine alone; chunk 1 is on device 8"
        );
    }

    #[test]
    fn the_chunks_closest_to_being_lost_are_reported_first() {
        // A repair pass on a laptop gets interrupted by the lid closing. If the
        // order were by chunk id, the work done before the interruption would
        // be random with respect to risk, and the chunk with one copy left
        // could wait behind a thousand that had two.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1, 2, 3])).unwrap();
        index.record_holder(&chunk(2), &device(7), 1).unwrap();
        index.record_holder(&chunk(3), &device(7), 1).unwrap();
        index.record_holder(&chunk(3), &device(8), 1).unwrap();

        let risky = index.under_replicated(4, 1).unwrap();
        let order: Vec<usize> = risky.iter().map(|r| r.held_by).collect();
        assert_eq!(order, vec![1, 2, 3], "worst first");
        assert!(risky[0].only_copy());
    }

    /// THE LEDGER WAS OPTIMISTIC IN THE ONE DIRECTION THAT LOSES DATA. Repair
    /// drained `under_replicated`, which counted every holder record whatever
    /// its age. A machine that died six months ago still counted as one of your
    /// three copies, so repair never fired -- and the only thing that withdraws
    /// its records is a **failed audit**, which needs that machine to answer.
    /// A dead one never does. The account believed it had three copies and had
    /// one, and nothing anywhere said otherwise.
    #[test]
    fn red_team_a_holder_silent_past_the_window_stops_counting_as_a_copy() {
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1])).unwrap();

        let long_ago = 1_000;
        let now = long_ago + holders::LIVE_FOR + 1;

        // Two machines confirmed this chunk, and then both went quiet.
        index
            .record_holder(&chunk(1), &device(7), long_ago)
            .unwrap();
        index
            .record_holder(&chunk(1), &device(8), long_ago)
            .unwrap();

        // While they were fresh, three copies existed and nothing was at risk.
        assert!(
            index.under_replicated(3, long_ago).unwrap().is_empty(),
            "two live holders and this disk are three copies"
        );

        // Past the window they are a belief, not a copy.
        let risky = index.under_replicated(3, now).unwrap();
        assert_eq!(
            risky.len(),
            1,
            "two holders silent past the liveness window still counted as copies, \
             so nothing would ever be re-replicated"
        );
        assert_eq!(
            risky[0].held_by, 1,
            "only this machine's own copy is evidence; got {risky:?}"
        );
        assert!(risky[0].only_copy());
    }

    /// The other half, and the one that stops the window being a data-loss
    /// machine of its own: a holder that keeps confirming keeps counting. A
    /// window that expired live records would re-replicate a healthy fleet's
    /// entire content on a schedule.
    #[test]
    fn a_holder_that_keeps_answering_keeps_counting() {
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1])).unwrap();

        let start = 1_000;
        let now = start + holders::LIVE_FOR * 4;

        index.record_holder(&chunk(1), &device(7), start).unwrap();
        index.record_holder(&chunk(1), &device(8), start).unwrap();
        // Both answered again recently, which is what an audit or a push does.
        index.record_holder(&chunk(1), &device(7), now - 1).unwrap();
        index.record_holder(&chunk(1), &device(8), now - 1).unwrap();

        assert!(
            index.under_replicated(3, now).unwrap().is_empty(),
            "holders that confirmed a moment ago were treated as gone"
        );
    }

    #[test]
    fn an_unreferenced_chunk_is_not_reported_as_under_replicated() {
        // Overwritten or deleted data is waiting for garbage collection. Asking
        // the repair loop to restore its replication would be work done to keep
        // something that is on its way out.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1])).unwrap();
        index.remove_file("a", &tombstone()).unwrap();

        assert_eq!(
            index.under_replicated(3, 1).unwrap(),
            [] as [holders::AtRisk; 0]
        );
    }
}
