//! Releasing local content: refused until other live machines hold it, on
//! evidence fresh for that chunk, and repeatable.

use itsanas_crypto::{DeviceKeys, MasterSecret};

use crate::store_for;

/// Releasing local content refuses until two other live machines hold it.
///
/// The one operation in the store that destroys data if it is wrong. A device
/// with a storage budget has to be able to let go of files -- otherwise the
/// budget fills once and the file edited this morning never arrives -- and the
/// difference between that and eating the second copy to make room is exactly
/// this check. One remaining copy is not a floor; it is the last one.
#[test]
fn content_is_not_released_until_two_other_machines_have_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());

    let payload = vec![7u8; 300_000];
    store.write_file("report.pdf", &payload).expect("write");
    let before = store.stats().expect("stats").bytes_on_disk;
    assert!(before >= 300_000, "the file was not stored");

    // The store's own clock, because `record_holders` stamps contact with the
    // real time and a fixed timestamp in the future would make every record
    // look stale.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();

    // Nobody has acknowledged holding any of it.
    match store.release("report.pdf", now).expect("release") {
        itsanas_store::Release::NotSafeYet { evidence, .. } if evidence.live == 0 => {}
        other => panic!("released the only copy in existence: {other:?}"),
    }

    assert_eq!(
        store.read_file("report.pdf").expect("read"),
        Some(payload.clone()),
        "a refused release still removed the content"
    );
    assert_eq!(
        store.stats().expect("stats").bytes_on_disk,
        before,
        "a refused release still freed space"
    );

    let chunks = store
        .stat("report.pdf")
        .expect("stat")
        .expect("the file is here")
        .chunks;

    // One peer is not enough. Letting go here would leave a single copy, and a
    // single copy is one disk failure from none.
    let first = DeviceKeys::generate().expect("device key").device_id();
    store.record_holders(&chunks, &first).expect("record");
    store.note_audit(&first, true).expect("audit");
    match store.release("report.pdf", now).expect("release") {
        itsanas_store::Release::NotSafeYet { evidence, .. } if evidence.live == 1 => {}
        other => panic!("released with a single copy elsewhere: {other:?}"),
    }

    // Two are.
    let second = DeviceKeys::generate().expect("device key").device_id();
    store.record_holders(&chunks, &second).expect("record");
    store.note_audit(&second, true).expect("audit");

    let freed = match store.release("report.pdf", now).expect("release") {
        itsanas_store::Release::Gone(report) => report,
        other => panic!("release refused after two live holders acknowledged it: {other:?}"),
    };

    assert!(
        freed.bytes >= 300_000,
        "release reported {} bytes freed for a 300 KB file",
        freed.bytes
    );
    assert_eq!(
        store.stats().expect("stats").bytes_on_disk,
        before - freed.bytes,
        "the space was not actually reclaimed"
    );
    assert!(
        store.read_file("report.pdf").expect("read").is_none(),
        "the content is still readable after being released"
    );
    assert!(
        store.tombstone("report.pdf").expect("tombstone").is_none(),
        "releasing content deleted the file from the account"
    );
}

/// A holder that has not been heard from in a fortnight does not authorise a
/// release.
///
/// The distinction the whole ledger rests on: an acknowledgement is evidence
/// about the past. `coverage` already refuses to count a silent machine as a
/// copy; letting go of local content on the strength of one would be worse,
/// because it acts on the belief instead of merely reporting it.
///
/// Note what recording a holder implies: `record_holders` stamps the device as
/// seen, because an acknowledgement *is* contact. The stale case is therefore
/// only reachable by moving the clock, which is why `release` takes one.
#[test]
fn red_team_a_stranger_that_only_claims_to_hold_a_chunk_cannot_make_it_releasable() {
    // The attack a red-team sweep found, and it needed no bug in the crypto,
    // the transport or the accounting -- only two throwaway keypairs.
    //
    // `Request::Hosted` records a holder for any device that completes a
    // handshake, and the answer to "so what stops a liar?" was: the owner's
    // storage challenges. But `session::audit` ran from `sync_once`, which only
    // ever runs against peers this node *dials* -- configured, coordinator
    // listed, or LAN announced. **A device that only ever dials in appears in
    // none of those lists and was never challenged once.** So:
    //
    //   1. mint two device keys, free, in a millisecond each;
    //   2. connect and ask `WantHosted` for the chunks with fewest copies;
    //   3. answer `Hosted` with those ids, holding nothing;
    //   4. the owner now believes two live, fresh holders exist.
    //
    // A device over its keep budget then released its only copy. Free, remote,
    // permanent, and invisible -- `coverage` and `status` both reported the
    // account replicated.
    //
    // The bar is now `proved`: holders that have answered a storage challenge,
    // which costs holding the bytes. This test is the ledger half -- the claim
    // is recorded exactly as the wire path records it, and the release refuses.
    // `a_release_rests_on_two_real_peers_and_notices_when_one_stops_holding` is
    // the other half, over real sockets.
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());

    store
        .write_file("only-copy.txt", b"the last copy of something")
        .expect("write");
    let chunks = store
        .stat("only-copy.txt")
        .expect("stat")
        .expect("here")
        .chunks;

    // Two strangers claim it. `record_holders` is exactly what the service does
    // for `Request::Hosted`, with the device the connection proved.
    let liar_one = DeviceKeys::generate().expect("device key").device_id();
    let liar_two = DeviceKeys::generate().expect("device key").device_id();
    store.record_holders(&chunks, &liar_one).expect("record");
    store.record_holders(&chunks, &liar_two).expect("record");

    let now = store
        .last_seen(&liar_one)
        .expect("last seen")
        .expect("seen");
    let evidence = store.holder_evidence(&chunks[0], now).expect("evidence");
    assert_eq!(
        evidence.live, 2,
        "the claims were not recorded, so this test proves nothing"
    );
    assert_eq!(evidence.fresh, 2, "the records are not fresh, likewise");
    assert_eq!(
        evidence.proved, 0,
        "a device that never answered a challenge counted as proof"
    );

    match store.release("only-copy.txt", now).expect("release") {
        itsanas_store::Release::NotSafeYet { evidence, .. } => {
            assert_eq!(evidence.proved, 0);
        }
        other => panic!("two strangers talked this node out of its only copy: {other:?}"),
    }
    assert!(
        store.read_file("only-copy.txt").expect("read").is_some(),
        "the content is gone, which is the whole damage"
    );

    // And the other side of the rule: one of them answers a challenge, so it
    // becomes a copy -- but one proved holder is still one short of the floor,
    // and the release stays refused. The bar moves with the evidence.
    store.note_audit(&liar_one, true).expect("audit");
    match store.release("only-copy.txt", now).expect("release") {
        itsanas_store::Release::NotSafeYet { evidence, .. } => {
            assert_eq!(evidence.proved, 1);
        }
        other => panic!("released on a single proved holder: {other:?}"),
    }

    // Both proved: this is a real pair of copies and letting go is correct.
    store.note_audit(&liar_two, true).expect("audit");
    match store.release("only-copy.txt", now).expect("release") {
        itsanas_store::Release::Gone(_) => {}
        other => panic!("two proved holders did not authorise a release: {other:?}"),
    }
}

#[test]
fn a_holder_nobody_has_heard_from_does_not_authorise_letting_go() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());

    store
        .write_file("notes.txt", b"small but mine")
        .expect("write");
    let chunks = store.stat("notes.txt").expect("stat").expect("here").chunks;

    let peer = DeviceKeys::generate().expect("device key").device_id();
    let other = DeviceKeys::generate().expect("device key").device_id();
    // Every stamp below is written in several calls, and a second can tick
    // between them. Measuring the window from one of them measured the clock
    // instead: the test failed on Windows runners whenever the tick landed
    // there. Reproduced every time by sleeping a second between the two calls.
    // A clock read before all of them is at or behind each one.
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();
    store.record_holders(&chunks, &peer).expect("record");
    store.note_audit(&peer, true).expect("audit");
    store.record_holders(&chunks, &other).expect("record");
    store.note_audit(&other, true).expect("audit");
    let seen = |device| store.last_seen(device).expect("last seen").expect("seen");

    // A proved holder allows a release for LIVE_FOR, and still does at its end.
    let last_moment = before + itsanas_store::holders::LIVE_FOR;
    match store.release("notes.txt", last_moment).expect("release") {
        itsanas_store::Release::Gone(_) => {}
        other => panic!("a record inside the window was refused: {other:?}"),
    }

    // Put it back and step one second past the window.
    store
        .write_file("notes.txt", b"small but mine")
        .expect("rewrite");
    let chunks = store.stat("notes.txt").expect("stat").expect("here").chunks;
    store.record_holders(&chunks, &peer).expect("record");
    store.note_audit(&peer, true).expect("audit");
    store.record_holders(&chunks, &other).expect("record");
    store.note_audit(&other, true).expect("audit");
    // Past the window for both, so measured from whichever was heard last.
    let latest = seen(&peer).max(seen(&other));

    match store
        .release(
            "notes.txt",
            latest + itsanas_store::holders::CONFIRMED_FOR + 1,
        )
        .expect("release")
    {
        itsanas_store::Release::NotSafeYet { evidence, .. } if evidence.live == 0 => {}
        other => panic!("devices silent for a fortnight authorised a release: {other:?}"),
    }
}

/// Releasing one file does not delete chunks another file still needs.
///
/// Deduplication means two paths can share a chunk. Freeing by path rather than
/// by reference would empty half of a file the device was told to keep, and the
/// damage would only surface the next time somebody opened it.
#[test]
fn releasing_one_file_leaves_a_chunk_another_file_still_uses() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());

    let shared = vec![3u8; 200_000];
    store.write_file("copy-one.bin", &shared).expect("write");
    store.write_file("copy-two.bin", &shared).expect("write");

    let chunks = store
        .stat("copy-one.bin")
        .expect("stat")
        .expect("here")
        .chunks;
    for _ in 0..itsanas_store::holders::SAFE_TO_RELEASE {
        let peer = DeviceKeys::generate().expect("device key").device_id();
        store.record_holders(&chunks, &peer).expect("record");
        store.note_audit(&peer, true).expect("audit");
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();

    match store.release("copy-one.bin", now).expect("release") {
        itsanas_store::Release::Gone(report) => assert_eq!(
            report.chunks, 0,
            "chunks were deleted while another file still referenced them"
        ),
        other => panic!("release refused: {other:?}"),
    }

    assert_eq!(
        store.read_file("copy-two.bin").expect("read"),
        Some(shared),
        "releasing one file destroyed the other"
    );
}

/// A holder that once passed its audits and then went silent proves nothing: a
/// phone uninstalled tells nobody. Sabotage: count `proved` without its age.
#[test]
fn red_team_release_does_not_rest_on_silent_low_presence_holders() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master");
    let store = store_for(&master, dir.path());
    store
        .write_file("only-here.bin", &vec![4u8; 200_000])
        .expect("write");
    let chunks = store
        .stat("only-here.bin")
        .expect("stat")
        .expect("here")
        .chunks;
    for _ in 0..itsanas_store::holders::SAFE_TO_RELEASE {
        let peer = DeviceKeys::generate().expect("device key").device_id();
        store.record_holders(&chunks, &peer).expect("record");
        store.note_audit(&peer, true).expect("audit");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();
    let silent = now + itsanas_store::holders::LIVE_FOR + 3_600;
    assert!(
        matches!(
            store.release("only-here.bin", silent).expect("release"),
            itsanas_store::Release::NotSafeYet { .. }
        ),
        "the owner dropped its copy on the word of holders silent for over a week: wiped, the \
         file is gone"
    );
    assert!(
        matches!(
            store.release("only-here.bin", now).expect("release"),
            itsanas_store::Release::Gone(_)
        ),
        "holders heard from just now must still allow a release"
    );
}

/// Releasing does not make this device forget who else holds the content.
///
/// A limit has to work more than once. The first version erased the holder
/// ledger along with the local copy, reusing the rule garbage collection needs
/// -- where a chunk goes because its *file* went, and repairing it would be
/// work for nobody. A release is the opposite case: the file is still in the
/// account, and the copies elsewhere are exactly what made letting go safe.
///
/// Observed on a Raspberry Pi before it was fixed: a device stuck at 380 KiB
/// against a 300 KiB limit, refusing to let go round after round because it had
/// forgotten the second holder while fetching the file back.
#[test]
fn a_file_can_be_released_fetched_back_and_released_again() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());

    let payload = vec![4u8; 200_000];
    store.write_file("again.bin", &payload).expect("write");
    let chunks = store.stat("again.bin").expect("stat").expect("here").chunks;

    let hosts: Vec<_> = (0..itsanas_store::holders::SAFE_TO_RELEASE)
        .map(|_| DeviceKeys::generate().expect("device key").device_id())
        .collect();
    for host in &hosts {
        store.record_holders(&chunks, host).expect("record");
        store.note_audit(host, true).expect("audit");
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();

    match store.release("again.bin", now).expect("release") {
        itsanas_store::Release::Gone(_) => {}
        other => panic!("the first release was refused: {other:?}"),
    }

    // What the device still knows. Forgetting this is what broke the second
    // round: it is also the only reason the account looks replicated at all
    // once a device holds a slice of it.
    assert_eq!(
        store.remote_holders(&chunks[0]).expect("holders").len(),
        hosts.len(),
        "releasing erased what this device knew about the copies elsewhere"
    );

    // Fetched back the way `itsanas get` fetches it: one holder serves, and is
    // recorded for having served.
    store.write_file("again.bin", &payload).expect("refetch");
    store
        .record_holders(&chunks, &hosts[0])
        .expect("record the server");

    match store.release("again.bin", now).expect("release") {
        itsanas_store::Release::Gone(_) => {}
        other => panic!("the second release was refused: {other:?}"),
    }
}

/// A machine that answers the phone with an empty disk does not authorise a
/// release.
///
/// The case a per-machine liveness rule cannot see, and the one that costs
/// data. `live_holder_count` asked only whether the *device* had been heard
/// from, so a peer that stayed online and threw everything away went on
/// counting as a copy for ever — and after a release there is no audit left to
/// contradict it, because auditing a chunk means re-deriving it from a local
/// copy this device no longer has.
///
/// Two bars now: the machine must be live, and at least one holder's record
/// *for this chunk* must have been refreshed inside the same window.
#[test]
fn a_reachable_machine_with_stale_records_does_not_authorise_a_release() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());

    store
        .write_file("held.bin", &vec![6u8; 200_000])
        .expect("write");
    let chunks = store.stat("held.bin").expect("stat").expect("here").chunks;

    let hosts: Vec<_> = (0..itsanas_store::holders::SAFE_TO_RELEASE)
        .map(|_| DeviceKeys::generate().expect("device key").device_id())
        .collect();
    for host in &hosts {
        store.record_holders(&chunks, host).expect("record");
        store.note_audit(host, true).expect("audit");
    }
    let recorded = store
        .last_seen(&hosts[0])
        .expect("last seen")
        .expect("seen");

    // Right now both bars are clear.
    let fresh = store
        .holder_evidence(&chunks[0], recorded)
        .expect("evidence");
    assert_eq!(fresh.live, hosts.len());
    assert_eq!(fresh.fresh, hosts.len());

    // A fortnight later both machines are still answering — and neither has
    // said anything about this chunk since.
    let later = recorded + itsanas_store::holders::CONFIRMED_FOR + 10;
    for host in &hosts {
        store.note_seen_at(host, later).expect("seen later");
    }

    let stale = store.holder_evidence(&chunks[0], later).expect("evidence");
    assert_eq!(stale.live, hosts.len(), "the machines are reachable");
    assert_eq!(stale.fresh, 0, "nothing was said about this chunk");

    match store.release("held.bin", later).expect("release") {
        itsanas_store::Release::NotSafeYet { evidence, .. } => {
            assert_eq!(evidence.live, hosts.len());
            assert_eq!(evidence.fresh, 0);
        }
        other => {
            panic!("released on the word of two machines that had gone quiet about it: {other:?}")
        }
    }
}
