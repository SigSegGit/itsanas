//! The account's budget: writes and pulls checked against the account's
//! allowance and the disk's ceiling, and a refused write leaving nothing.

use std::time::Duration;

use itsanas_crypto::MasterSecret;
use itsanas_store::{StoreError, WriteBudget};
use itsanas_testkit as testkit;

use crate::store_for;

// ---------------------------------------------------------------------------
// The account's budget (HANDOVER §8 0n, 1b)
// ---------------------------------------------------------------------------

const KIB: u64 = 1024;

/// A file the account has no room for is refused, and refusing it costs
/// nothing but the refusal.
///
/// Written through `write_file`, with no size asked first, so the refusal can
/// only come part-way through the stream -- after the first chunks of the file
/// have already been sealed and stored. That is the path that could leave
/// debris, and a folder pass retries a refused file every time it runs: debris
/// here is a disk that fills with the first part of the same video, again and
/// again, while the account stays exactly as full as it was.
/// The account may have room while the disk has not: what this machine's
/// pledge still owes is space promised to others. A write past the disk room
/// is refused with the numbers and leaves nothing behind; a write inside it
/// still succeeds, so the test cannot pass on a store that refuses all.
#[test]
fn red_team_a_write_past_the_disk_room_is_refused_and_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([62; 32]), dir.path());
    store
        .set_write_budget(Some(WriteBudget {
            allowed: u64::MAX,
            elsewhere: 0,
            local_ceiling: Some(300 * KIB),
            folder_copy: itsanas_store::FolderCopy::None,
        }))
        .unwrap();

    store
        .write_file("small.bin", &testkit::filler("small", 100 * 1024))
        .expect("a file inside the disk room is stored");
    let before = store.stats().unwrap();

    match store.write_file("big.bin", &testkit::filler("big-disk", 1024 * 1024)) {
        Err(StoreError::DiskFull { incoming, room, .. }) => {
            assert_eq!(
                room,
                200 * KIB,
                "the refusal misstates the room: 300 KiB less the 100 held"
            );
            assert!(incoming > room, "refused at {incoming}, not past {room}");
        }
        other => panic!(
            "a 1 MiB file was written to a disk with 300 KiB to spare beyond the \
             pledge: {other:?}. The space promised to other people is eaten by \
             our own files, and their data has nowhere to go"
        ),
    }
    let after = store.stats().unwrap();
    assert_eq!(
        after.bytes_on_disk, before.bytes_on_disk,
        "the refused file left chunks on disk"
    );
    assert!(
        store.read_file("big.bin").unwrap().is_none(),
        "the refused file has an index entry"
    );
}

/// The ceiling bounds the sum, not each write: a folder pass imports many
/// files after one `bound_writes`, and a per-write room let a hundred 1 GB
/// files into 10 GB (found by the `itsanas-redteam` agent). Five 100 KiB
/// files against 300 KiB: three fit, the fourth is refused. And an edit is
/// charged its growth -- rewriting a stored file in place still fits.
#[test]
fn red_team_many_small_writes_cannot_pass_the_disk_ceiling_together() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([63; 32]), dir.path());
    store
        .set_write_budget(Some(WriteBudget {
            allowed: u64::MAX,
            elsewhere: 0,
            local_ceiling: Some(300 * KIB),
            folder_copy: itsanas_store::FolderCopy::None,
        }))
        .unwrap();

    for i in 0..3 {
        store
            .write_file(
                &format!("f{i}.bin"),
                &testkit::filler(&format!("f{i}"), 100 * 1024),
            )
            .expect("inside the ceiling");
    }
    assert!(
        matches!(
            store.write_file("f3.bin", &testkit::filler("f3", 100 * 1024)),
            Err(StoreError::DiskFull { .. })
        ),
        "four 100 KiB files went into 300 KiB: each write was checked alone against a \
         snapshot, so a pass of small files eats the whole pledge"
    );
    store
        .write_file("f0.bin", &testkit::filler("f0-edited", 100 * 1024))
        .expect("an edit of the same size is charged its growth, which is nothing");
}

/// A pull writes the file twice when a folder is synced: the store's chunks
/// and the folder's plaintext. With the folder on the home's volume, a pull
/// of X under a room of X would put 2X on the disk, the second X out of what
/// the pledge owes other people; so both copies are charged, and the bytes
/// pulled since the budget was set count twice as well. With the folder on
/// another volume, that volume's free space bounds the folder copy.
#[test]
fn red_team_a_pull_charges_the_folder_copy_too() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([64; 32]), dir.path());
    let budget = |folder_copy| WriteBudget {
        allowed: u64::MAX,
        elsewhere: 0,
        local_ceiling: Some(100 * KIB),
        folder_copy,
    };
    let same = itsanas_store::FolderCopy::SameVolume { local_then: 0 };
    store.set_write_budget(Some(budget(same))).unwrap();
    assert!(
        store
            .pull_room("big.bin", 100 * KIB, false)
            .unwrap()
            .is_some(),
        "a pull of X under a room of X was let in: with the folder's copy it \
         puts 2X on the disk"
    );
    assert_eq!(store.pull_room("half.bin", 50 * KIB, false).unwrap(), None);

    store
        .write_file("here.bin", &testkit::filler("here", 25 * 1024))
        .expect("inside the ceiling");
    assert_eq!(
        store.pull_room("a.bin", 25 * KIB, false).unwrap(),
        None,
        "25 KiB already here (50 with its folder copy) and 25 more (50) fill 100 exactly"
    );
    assert!(
        store.pull_room("b.bin", 26 * KIB, false).unwrap().is_some(),
        "the bytes pulled since the budget was set were charged once, not twice"
    );

    store
        .set_write_budget(Some(budget(itsanas_store::FolderCopy::OtherVolume {
            local_then: 0,
            room: 10 * KIB,
        })))
        .unwrap();
    assert!(
        store.pull_room("c.bin", 20 * KIB, false).unwrap().is_some(),
        "the folder's volume has 10 KiB free and the pull was let in: the export fails half-written"
    );
}

#[test]
fn red_team_a_write_past_the_budget_leaves_no_chunk_no_entry_and_no_log() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([61; 32]), dir.path());
    store
        .set_write_budget(Some(WriteBudget {
            allowed: 600 * KIB,
            elsewhere: 0,
            local_ceiling: None,
            folder_copy: itsanas_store::FolderCopy::None,
        }))
        .unwrap();

    store
        .write_file("kept.bin", &testkit::filler("kept", 256 * 1024))
        .expect("a file inside the budget is stored");
    let before = store.stats().unwrap();
    let unsealed = store.unsealed_entries().unwrap().len();

    let refused = store.write_file("big.bin", &testkit::filler("big", 1024 * 1024));
    match refused {
        Err(StoreError::OverBudget {
            account,
            total,
            allowed,
            ..
        }) => {
            assert_eq!(account, 256 * KIB, "the refusal misstates the account");
            assert_eq!(allowed, 600 * KIB, "the refusal misstates the limit");
            assert!(total > allowed, "refused at {total}, not past {allowed}");
        }
        other => panic!(
            "a 1 MiB file was accepted into an account with 344 KiB left: {other:?}. \
             Nothing bounds what an honest client writes, so an account grows past \
             what its pledge earns and the network is asked to hold it"
        ),
    }

    let after = store.stats().unwrap();
    assert_eq!(
        after.bytes_on_disk,
        before.bytes_on_disk,
        "the refused file left {} bytes of chunks on disk. A folder pass retries it \
         every round, so the disk fills with a file the account never took",
        after.bytes_on_disk.saturating_sub(before.bytes_on_disk)
    );
    assert!(
        store.stat("big.bin").unwrap().is_none(),
        "the refused file has an index entry, so it is listed and will be announced"
    );
    assert_eq!(
        store.unsealed_entries().unwrap().len(),
        unsealed,
        "the refused file was written to the log, so every device will look for it"
    );
    assert_eq!(
        store.read_file("kept.bin").unwrap().unwrap(),
        testkit::filler("kept", 256 * 1024),
        "cleaning up after the refusal damaged a file that was already there"
    );
}

/// A reader that hands over `data`, stopping once at `pause_at` until told to
/// go on: a write held part-way, for something else to run meanwhile.
struct PausedReader {
    data: Vec<u8>,
    at: usize,
    pause_at: usize,
    paused: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
}

impl std::io::Read for PausedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at == self.pause_at
            && let Some((reached, go)) = self.paused.take()
        {
            let _ = reached.send(());
            let _ = go.recv();
        }
        let end = if self.at < self.pause_at {
            self.pause_at
        } else {
            self.data.len()
        };
        let n = buf.len().min(end - self.at);
        buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// A chunk repair brings back while a refused write that shares it is under
/// way is still there afterwards: the write's cleanup does not take it with
/// the chunks it created, now that repair has stopped looking for it.
/// Sabotage: drop the `write_lock` in `accept_chunk`.
#[test]
fn a_refused_write_does_not_remove_a_chunk_repair_brought_back_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([0x5A; 32]), dir.path());
    let shared = testkit::filler("shared", 512 * 1024);
    let entry = store.write_file("f.bin", &shared).unwrap();
    let lost = entry.chunks[0];
    let genuine = store
        .blobs()
        .get(&lost)
        .unwrap()
        .expect("the chunk just written");
    store.blobs().damage(&lost).unwrap();
    store.verify_integrity(false).unwrap();
    assert_eq!(
        store.loss_count().unwrap(),
        1,
        "fixture: the loss is queued"
    );
    store
        .set_write_budget(Some(WriteBudget {
            allowed: 1200 * KIB,
            elsewhere: 0,
            local_ceiling: None,
            folder_copy: itsanas_store::FolderCopy::None,
        }))
        .unwrap();

    let (reached, paused) = std::sync::mpsc::channel();
    let (go, wait) = std::sync::mpsc::channel();
    let mut data = shared.clone();
    data.extend(testkit::filler("more", 1024 * 1024));
    let reader = PausedReader {
        data,
        at: 0,
        pause_at: shared.len(),
        paused: Some((reached, wait)),
    };
    std::thread::scope(|scope| {
        let write = scope.spawn(|| store.write_stream("g.bin", reader));
        paused.recv().unwrap();
        let repair = scope.spawn(|| store.accept_chunk(&lost, &genuine));
        std::thread::sleep(Duration::from_millis(300));
        go.send(()).unwrap();
        assert!(
            write.join().unwrap().is_err(),
            "fixture: the write is refused"
        );
        assert!(
            repair.join().unwrap().unwrap(),
            "the genuine chunk was refused"
        );
    });

    assert!(
        store.has_chunk(&lost) || store.loss_count().unwrap() == 1,
        "a refused write removed a chunk repair had brought back, and repair no longer looks for it"
    );
    assert_eq!(store.read_file("f.bin").unwrap().unwrap(), shared);
}

/// The bound charges what a write adds, not what it weighs.
///
/// Kept beside the red-team test so that one cannot pass on a store that
/// refuses everything, and so that an account at its limit can still edit its
/// own files: charging an edit its whole size would lock a full account out of
/// changing a single byte.
#[test]
fn a_write_inside_the_budget_succeeds_and_an_edit_is_charged_only_its_growth() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([62; 32]), dir.path());
    store
        .set_write_budget(Some(WriteBudget {
            allowed: 1024 * KIB,
            elsewhere: 0,
            local_ceiling: None,
            folder_copy: itsanas_store::FolderCopy::None,
        }))
        .unwrap();

    store
        .write_file("a.bin", &testkit::filler("a1", 600 * 1024))
        .expect("600 KiB fits in 1 MiB");
    store
        .write_file("a.bin", &testkit::filler("a2", 700 * 1024))
        .expect(
            "growing a 600 KiB file to 700 KiB in a 1 MiB account was refused: the \
             edit was charged its whole size, as if the old version still counted",
        );

    assert!(
        matches!(
            store.check_room("b.bin", 400 * KIB),
            Err(StoreError::OverBudget { account, .. }) if account == 700 * KIB
        ),
        "400 KiB more on top of 700 KiB was not refused against a 1 MiB budget"
    );

    store.remove_file("a.bin").unwrap();
    store
        .check_room("b.bin", 1024 * KIB)
        .expect("a deleted file still counts against the account, so deleting never makes room");
}

/// Files this device has not downloaded are still the account's.
///
/// A phone keeping 2 GB of a 40 GB account holds 2 GB here; counting only that
/// would let it write 38 GB past what the account may hold.
#[test]
fn red_team_what_the_account_holds_elsewhere_counts_against_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([63; 32]), dir.path());
    store
        .set_write_budget(Some(WriteBudget {
            allowed: 1024 * KIB,
            elsewhere: 900 * KIB,
            local_ceiling: None,
            folder_copy: itsanas_store::FolderCopy::None,
        }))
        .unwrap();

    match store.check_room("new.bin", 200 * KIB) {
        Err(StoreError::OverBudget { account, total, .. }) => {
            assert_eq!(account, 900 * KIB);
            assert_eq!(total, 1100 * KIB);
        }
        other => panic!(
            "200 KiB was accepted into an account holding 900 KiB of 1 MiB on other \
             devices: {other:?}. A device holding part of an account can write past \
             what the whole account may hold"
        ),
    }
    store
        .check_room("new.bin", 100 * KIB)
        .expect("100 KiB fits exactly in what is left");
}
