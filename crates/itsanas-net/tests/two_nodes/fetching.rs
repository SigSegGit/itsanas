//! Fetching chosen files: what a device asked for, never downloaded, or made
//! and released comes back, and nothing else.

use super::*;

#[test]
fn a_device_takes_the_files_it_asked_for_and_none_of_the_others() {
    // The ordinary case for a phone, not an edge case: a few gigabytes free
    // against an account of hundreds. What must NOT happen is the device
    // filling its disk, and what must not happen either is a half-written file.
    //
    // The device names what it wants and the source declines everything else,
    // so the merge engine treats the rest exactly as it treats a peer that is
    // asleep: the operation is deferred and nothing is written. Those files
    // stay known to the account and absent from this device, which is the state
    // a client shows and fetches on demand.
    //
    // This used to be a byte budget inside the pull, which stopped when the
    // allowance ran out and therefore kept whatever the log replayed first.
    // Deciding *which* files is `itsanas_policy::keeping`; this is the network
    // half of it, and the assertion that matters here is that nothing outside
    // the request arrives.
    let big = node(&MasterSecret::from_bytes([0x1A; 32]), 50);
    let small = node(&MasterSecret::from_bytes([0x1A; 32]), 51);

    // Four files of a quarter-megabyte each.
    for name in ["one.bin", "two.bin", "three.bin", "four.bin"] {
        let payload = itsanas_testkit::filler(name, 256 * 1024);
        big.store.write_file(name, &payload).unwrap();
    }
    big.store.flush_segment().unwrap();
    let whole = big.store.stats().unwrap().bytes_on_disk;
    assert!(
        whole > 900 * 1024,
        "the fixture is too small to be selective about"
    );

    with_server(&big, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &small.device, small.store.owner(), None).unwrap();

        // Learn what exists without downloading any of it.
        session::pull_scoped(
            &small.store,
            &small.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .unwrap();

        let wanted: std::collections::BTreeSet<String> = ["two.bin".to_owned()].into();
        let chunks = itsanas_store::chunks_for_all(&small.store, &small.vault, &wanted).unwrap();
        assert!(
            !chunks.is_empty(),
            "the file's chunks could not be resolved"
        );

        session::fetch_only(&small.store, &small.vault, &mut client, &chunks)
            .expect("a selective round is not an error");
    });

    assert_eq!(
        small.store.list().unwrap(),
        vec!["two.bin".to_owned()],
        "the device took files it did not ask for"
    );
    let held = small.store.stats().unwrap().bytes_on_disk;
    assert!(
        held < whole,
        "the small device took the whole account despite asking for one file"
    );

    // Whatever did arrive is whole. A selective fetch that produced a truncated
    // file would be worse than no selection at all.
    for path in small.store.list().unwrap() {
        let content = small
            .store
            .read_file(&path)
            .unwrap()
            .unwrap_or_else(|| panic!("{path} is listed and unreadable"));
        assert_eq!(content.len(), 256 * 1024, "{path} came back truncated");
    }

    // And the rest is listed rather than lost.
    let listing = itsanas_store::catalogue(&small.store, &small.vault).unwrap();
    assert_eq!(listing.files.len(), 4, "the account's other files vanished");
    assert_eq!(
        listing
            .files
            .iter()
            .filter(|file| file.presence == itsanas_store::Presence::Absent)
            .count(),
        3,
        "files nobody asked for were downloaded anyway"
    );
}

#[test]
fn a_file_this_device_never_downloaded_can_be_fetched_when_it_is_asked_for() {
    // The capability the storage budget's whole justification rests on, and
    // which did not exist when the budget shipped: a device lists a file it
    // does not hold, and opening it goes and gets it.
    //
    // Without this, `keep` produces files that are visible and unopenable, and
    // a phone client is a browser for things you cannot read.
    let big = node(&MasterSecret::from_bytes([0x2B; 32]), 60);
    let phone = node(&MasterSecret::from_bytes([0x2B; 32]), 61);

    for name in ["wanted.bin", "ignored.bin"] {
        let payload = itsanas_testkit::filler(name, 256 * 1024);
        big.store.write_file(name, &payload).unwrap();
    }
    big.store.flush_segment().unwrap();
    let wanted_bytes = big.store.read_file("wanted.bin").unwrap().unwrap();

    // Metadata only: the phone learns what exists and downloads nothing.
    with_server(&big, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).unwrap();
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .unwrap();
    });

    let known = itsanas_store::catalogue(&phone.store, &phone.vault).unwrap();
    assert_eq!(known.files.len(), 2, "the phone did not learn what exists");
    assert!(
        known
            .files
            .iter()
            .all(|file| file.presence == itsanas_store::Presence::Absent),
        "the metadata round downloaded content"
    );
    assert!(
        phone.store.read_file("wanted.bin").unwrap().is_none(),
        "the file is present before the fetch, so this test would prove nothing"
    );

    // The chunk list has to be reachable from the path alone, or nothing can
    // go and get it.
    let chunks = itsanas_store::chunks_for(&phone.store, &phone.vault, "wanted.bin")
        .unwrap()
        .expect("a listed file has no chunk list");
    let wanted: std::collections::BTreeSet<_> = chunks.into_iter().collect();

    with_server(&big, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).unwrap();
        session::fetch_only(&phone.store, &phone.vault, &mut client, &wanted).unwrap();
    });

    assert_eq!(
        phone.store.read_file("wanted.bin").unwrap().as_ref(),
        Some(&wanted_bytes),
        "the file asked for did not arrive"
    );

    // And only that file. Opening one document must not pull the rest.
    assert!(
        phone.store.read_file("ignored.bin").unwrap().is_none(),
        "asking for one file downloaded another"
    );
}

#[test]
fn a_file_this_device_made_and_released_can_be_fetched_back_from_a_host() {
    // The other half of the same defect, and the worse half. Once the listing
    // showed a released file again, opening it still answered "no such file" --
    // on a Raspberry Pi, for a file two hosts were holding at that moment.
    //
    // `fetch_only` applies other devices' segments, because the index is the
    // authority for anything this machine wrote. Releasing content removes the
    // index entry while leaving the operation in this device's own chain, so
    // the one log that describes the file was the one log not being read.
    //
    // A listed file that cannot be opened is worse than a file that is not
    // listed: the first looks like corruption, the second like a device that
    // has not synced.
    let mine = node(&MasterSecret::from_bytes([0x3C; 32]), 70);
    let host = node(&MasterSecret::from_bytes([0x4D; 32]), 71);

    let payload = itsanas_testkit::filler("released", 200 * 1024);
    mine.store.write_file("mine.bin", &payload).unwrap();
    mine.store.flush_segment().unwrap();
    let chunks = mine.store.stat("mine.bin").unwrap().unwrap().chunks;

    // The host takes a copy, the ordinary way.
    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &mine.device, mine.store.owner(), None).unwrap();
        session::push(&mine.store, &mut client).unwrap();
        // And challenged, which is what turns the acknowledgement into a copy
        // the release may count. A round does both; a test that did only the
        // first was measuring a claim.
        session::audit(&mine.store, &mut client, session::CHALLENGES_PER_ROUND).unwrap();
    });

    // Two proved holders, so the release is allowed. One is the host that just
    // acknowledged and answered; the second stands for the other machine a real
    // account has, and is written straight into the ledger -- with its audit
    // stamped, because a recorded holder that has never answered a challenge is
    // exactly the forged record `Request::Hosted` lets a stranger write, and
    // this test is about the fetch, not about the threshold.
    let elsewhere =
        itsanas_crypto::DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new([0xEE; 32]))
            .device_id();
    mine.store.record_holders(&chunks, &elsewhere).unwrap();
    mine.store.note_audit(&elsewhere, true).unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    match mine.store.release("mine.bin", now).unwrap() {
        itsanas_store::Release::Gone(_) => {}
        other => panic!("release refused, so this test proves nothing: {other:?}"),
    }
    assert!(
        mine.store.read_file("mine.bin").unwrap().is_none(),
        "the content is still here, so fetching it back proves nothing"
    );

    // Listed, and now fetchable.
    let wanted: std::collections::BTreeSet<_> = chunks.iter().copied().collect();
    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &mine.device, mine.store.owner(), None).unwrap();
        session::fetch_only(&mine.store, &mine.vault, &mut client, &wanted).unwrap();
    });

    assert_eq!(
        mine.store.read_file("mine.bin").unwrap(),
        Some(payload),
        "a file this device made, released and asked for again did not come back"
    );
}

#[test]
fn a_second_push_offers_nothing_and_says_so() {
    // Found by reading three machines' daemon logs after an upgrade: "sent
    // 400 B (0 chunks, 1 segments)" every five minutes, on a fleet where
    // nothing was happening. Two faults met there.
    //
    // A push offered the whole chain on every round, whatever the peer already
    // had. The vault refused each segment it held -- one that is neither the tip
    // nor the next link answers `SegmentChainBroken` -- and `store_segment` maps
    // every refusal to `false`, so the waste was invisible from the pushing
    // side. It grows without bound: a chain gains a segment per batch of edits
    // and is never compacted, so a thousand segments is around 350 KB
    // re-uploaded per round per peer, a hundred megabytes a day, for nothing.
    //
    // And the service answered `accepted: true` for a segment it had not
    // stored, so `changed_anything()` was true on every idle round for ever --
    // in a daemon whose own comment says a quiet round is the common case and
    // that reporting it every five minutes would fill a journal with nothing.
    let author = node(&alice(), 80);
    let host = node(&MasterSecret::from_bytes([0x9F; 32]), 81);

    author.store.write_file("first.txt", b"one").unwrap();
    author.store.flush_segment().unwrap();

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).unwrap();

        let first = session::push(&author.store, &mut client).unwrap();
        assert!(first.segments_offered > 0, "the first push offered nothing");
        assert_eq!(
            first.segments_accepted, first.segments_offered,
            "the host did not take what it had never seen"
        );

        // Nothing has changed. The peer already has the chain, and it should be
        // asked for nothing at all.
        let second = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            second.segments_offered, 0,
            "the whole chain was offered again to a peer that already had it"
        );
        assert_eq!(
            second.segments_accepted, 0,
            "a segment the host already held was counted as stored"
        );
        assert_eq!(second.bytes_sent, 0, "bytes were sent for nothing");

        // One new segment, and exactly one is offered.
        author.store.write_file("second.txt", b"two").unwrap();
        author.store.flush_segment().unwrap();

        let third = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            third.segments_offered, 1,
            "a push after one new segment offered {} of them",
            third.segments_offered
        );
        assert_eq!(third.segments_accepted, 1);
    });
}
