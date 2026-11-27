//! What this device holds and what its ledger vouches for: paging the live
//! chunks, losses on this disk, paused hosts, and resting machines.

use std::collections::HashSet;

use itsanas_crypto::{ChunkId, DeviceKeys, MasterSecret};
use itsanas_store::Store;
use itsanas_testkit as testkit;

use crate::store_for;

/// The sweep enumerates from the index, not from the blob directory.
///
/// The push loop asks a peer "which of my chunks are you missing", and it used
/// to build that list with `BlobStore::addresses`, whose own documentation says
/// it walks the fan-out directories and is "never on a hot path". It was on the
/// hottest path there is: every round, per peer. At a terabyte that is a
/// recursive `readdir` over sixteen million files every five minutes, and a
/// `Vec` of sixteen million ids — 537 MB resident in one allocation, on
/// machines whose measured peak is 17 MiB.
///
/// This is what replaced it: paged, in chunk order, from a table the index
/// already keeps. The page is exact — no chunk twice, none missed — because a
/// sweep that skips is a holder record that never gets corrected.
#[test]
fn the_chunks_a_device_holds_can_be_paged_without_walking_the_directories() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());

    for index in 0..6 {
        let payload = testkit::filler(&format!("page-{index}"), 200 * 1024);
        store
            .write_file(&format!("file-{index}.bin"), &payload)
            .expect("write");
    }

    let everything: HashSet<ChunkId> = store
        .blobs()
        .addresses()
        .expect("addresses")
        .into_iter()
        .collect();
    assert!(
        everything.len() > 8,
        "the fixture is too small to page: {} chunks",
        everything.len()
    );

    // Three at a time, which forces several pages and a boundary that is not a
    // multiple of the total.
    let mut seen: Vec<ChunkId> = Vec::new();
    let mut cursor = None;
    loop {
        let (page, next) = store.live_chunks_page(cursor.as_ref(), 3).expect("page");
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 3, "a page came back over its limit");
        seen.extend(page);
        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    let unique: HashSet<ChunkId> = seen.iter().copied().collect();
    assert_eq!(
        unique.len(),
        seen.len(),
        "a chunk was returned on two pages"
    );
    assert_eq!(
        unique, everything,
        "paging from the index did not see what the device holds"
    );

    // And an overwrite leaves the superseded chunks in the sweep until
    // collection takes them: a replacement device replaying the log needs the
    // chunks of the operation it is applying, not only of the current state.
    store
        .write_file("file-0.bin", b"second thoughts")
        .expect("overwrite");

    let mut after = Vec::new();
    let mut cursor = None;
    loop {
        let (page, next) = store.live_chunks_page(cursor.as_ref(), 64).expect("page");
        if page.is_empty() {
            break;
        }
        after.extend(page);
        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let after: HashSet<ChunkId> = after.into_iter().collect();
    assert!(
        everything.is_subset(&after),
        "chunks awaiting collection dropped out of the sweep, so a peer would \
         never be offered what a replay still needs"
    );
}

/// Every live chunk, in id order.
fn live(store: &Store) -> Vec<ChunkId> {
    let mut out = Vec::new();
    let mut cursor = None;
    loop {
        let (page, next) = store.live_chunks_page(cursor.as_ref(), 1024).unwrap();
        out.extend(page);
        match next {
            Some(next) => cursor = Some(next),
            None => return out,
        }
    }
}

#[test]
fn red_team_a_node_does_not_tell_its_own_account_it_holds_what_its_disk_lost() {
    // §8 4c, found by Rodin on the plan. A machine answers a summary for its own
    // account from its index, and since 4c an agreeing summary re-stamps holder
    // records without a `HaveChunks`. So a Pi of the same account whose disk lost
    // a blob kept agreeing, and the laptop kept counting it as a copy. The disk
    // check records the loss; the summary served to the account must leave it
    // out. Sabotage: count lost chunks in `held_summary`.
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([0x4C; 32]), dir.path());
    store
        .write_file("photo.bin", &testkit::filler("held", 512 * 1024))
        .unwrap();
    store.flush_segment().unwrap();
    let chunks = live(&store);
    assert!(
        chunks.len() > 1,
        "fixture: one chunk proves nothing about the rest"
    );
    assert_eq!(
        store.held_summary().unwrap(),
        store.chunk_summary().unwrap()
    );

    let lost = chunks[0];
    store.blobs().damage(&lost).unwrap();
    let (missing, next) = store.check_disk(None, chunks.len() + 1).unwrap();
    assert_eq!(
        (missing, next),
        (1, None),
        "one pass of the check missed the loss"
    );
    assert_eq!(store.loss_count().unwrap(), 1, "repair was not told");

    let kept: Vec<ChunkId> = chunks[1..].to_vec();
    assert_eq!(
        store.held_summary().unwrap(),
        itsanas_store::summary::buckets(kept),
        "the summary this node gives its own account still counts a chunk its disk lost: \
         the account's other machines re-stamp it as a copy and stop looking"
    );
    assert_eq!(
        store.chunk_summary().unwrap(),
        itsanas_store::summary::buckets(chunks),
        "the set this node wants held must not shrink because it lost a copy"
    );
}

/// The local-loss sweep runs every round over a slice of the live chunks.
/// On a healthy disk it used to commit once per chunk it looked at -- four
/// of an idle round's eleven commits (ROADMAP, "An idle node's disk
/// writes"); now a pass that finds every blob where it should be writes
/// nothing, and a blob gone is still recorded as lost. Sabotaged (the
/// per-chunk `clear_loss` back): red.
#[test]
fn a_loss_sweep_over_a_healthy_disk_writes_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = store_for(&testkit::alice().master, dir.path());
    store
        .write_file("a.txt", &vec![1u8; 200_000])
        .expect("write");
    store.flush_segment().expect("flush");
    let index = dir.path().join("index.redb");
    let start = ChunkId::from_bytes([0; 32]);
    assert_eq!(
        store.missing_locally(&start, 64).expect("sweep"),
        Vec::<ChunkId>::new()
    );
    // Size and write time, not bytes: Windows refuses to read a database
    // another handle has locked. After a pause, so a write moves the time.
    let file = || {
        let meta = std::fs::metadata(&index).expect("metadata");
        (meta.len(), meta.modified().expect("mtime"))
    };
    let before = file();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(
        store.missing_locally(&start, 64).expect("sweep"),
        Vec::<ChunkId>::new()
    );
    assert!(
        file() == before,
        "a sweep that found every blob in place committed to the index"
    );
}

/// A host paused for lying is no copy: not proved for a release, not counted
/// in coverage or for repair, however it once passed and however often it
/// says `Hosted`. Sabotage: count a paused holder; let an old pass vouch.
#[test]
fn red_team_a_paused_host_with_an_old_pass_vouches_for_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());
    store
        .write_file("kept.txt", b"held once, then thrown away")
        .expect("write");
    let chunks = store.stat("kept.txt").expect("stat").expect("here").chunks;
    let host = DeviceKeys::generate().expect("device key").device_id();
    store.record_holders(&chunks, &host).expect("record");
    store.note_audit(&host, true).expect("pass");
    let now = store.last_seen(&host).expect("seen").expect("seen");
    assert_eq!(
        store
            .holder_evidence(&chunks[0], now)
            .expect("evidence")
            .proved,
        1,
        "the pass was not recorded, so this test proves nothing"
    );
    for _ in 0..itsanas_store::FAILURES_BEFORE_PAUSE {
        store.note_audit(&host, false).expect("fail");
    }
    // It keeps saying it holds them.
    store.record_holders(&chunks, &host).expect("record");
    let now = store.last_seen(&host).expect("seen").expect("seen");
    assert_eq!(
        store
            .holder_evidence(&chunks[0], now)
            .expect("evidence")
            .proved,
        0,
        "a host paused for lying still counts towards dropping the last copy"
    );
    assert_eq!(
        store.coverage(now).expect("coverage").complete_elsewhere,
        0,
        "a host paused for lying is shown as a copy, so the person sees green"
    );
    assert!(
        !store.under_replicated(2, now).expect("at risk").is_empty(),
        "a host paused for lying stops repair from placing a real copy"
    );
}

/// A machine of the account not heard from for over a week is named resting,
/// the most silent first, never this machine, and one heard from lately is
/// not: a key in a drawer said as what it is, where the copy count would
/// otherwise read as copies lost (HANDOVER 8 0e, step 4). Sabotage: compare
/// with `<` instead of `>`, or drop the sort.
#[test]
fn red_team_a_machine_silent_past_a_week_is_named_resting_the_most_silent_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([31; 32]), dir.path());
    let now = 1_800_000_000;
    let day = 24 * 3600;
    let id = |seed: u8| {
        itsanas_crypto::DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new([seed; 32]))
            .device_id()
    };
    let (drawer, holiday, laptop, never) = (id(41), id(42), id(43), id(44));
    for member in [drawer, holiday, laptop, never, store.device_id()] {
        store.admit_member(&member, false).unwrap();
    }
    store.note_seen_at(&drawer, now - 90 * day).unwrap();
    store.note_seen_at(&holiday, now - 9 * day).unwrap();
    store.note_seen_at(&laptop, now - day).unwrap();

    let resting: Vec<_> = store
        .dormant_members(now)
        .unwrap()
        .into_iter()
        .map(|(device, _)| device)
        .collect();
    assert_eq!(
        resting,
        vec![never, drawer, holiday],
        "not the machines silent past a week, the most silent first"
    );
}
