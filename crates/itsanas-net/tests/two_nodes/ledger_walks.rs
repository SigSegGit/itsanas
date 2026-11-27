//! The ledger of who holds what: releases resting on real peers, rounds that
//! say nothing in one hash, and the walks that catch a silent loss.

use super::*;

#[test]
fn a_release_rests_on_two_real_peers_and_notices_when_one_stops_holding() {
    // The test the repository did not have, and the reason three defects in the
    // release path were found by hand on a Raspberry Pi and none by 683 tests.
    //
    // Every other release test writes the holder ledger directly — a
    // `DeviceKeys::generate()` that never spoke to anything — and then reads it
    // back. That is sound for testing the *choice*, and useless for testing the
    // release, because a release never fails on the choice. It fails on the
    // provenance of the evidence. A test that fabricates its own input cannot
    // catch a bug in how that input is derived, and all three defects were
    // bugs in derivation.
    //
    // So: nothing here is written into the ledger by hand. Two real hosts, two
    // real pushes over two real sockets, and the release decides from what
    // those exchanges left behind.
    let author = node(&alice(), 90);
    let host_one = node(&MasterSecret::from_bytes([0xC1; 32]), 91);
    let host_two = node(&MasterSecret::from_bytes([0xC2; 32]), 92);

    let payload = itsanas_testkit::filler("provenance", 200 * 1024);
    author.store.write_file("real.bin", &payload).unwrap();
    author.store.flush_segment().unwrap();
    let chunks = author.store.stat("real.bin").unwrap().unwrap().chunks;

    // Push, then **challenge on the same connection**, which is what a real
    // round does and what the release now requires. A push produces a
    // *recorded* holder; only an answered challenge produces a proved one, and
    // proved is what `Store::release` counts. Adding the audit here is not
    // making the test pass -- it is the test finally doing what the daemon and
    // `itsanas_node::round` do, so that "a release rests on two real peers"
    // means the peers were asked rather than believed.
    let push_to = |host: &Node| {
        with_server(host, Pledge::gigabytes(1), |address| {
            let mut client =
                PeerClient::connect(address, &author.device, author.store.owner(), None).unwrap();
            let report = session::push(&author.store, &mut client).unwrap();
            // No assertion on how many were asked: this helper is used again
            // further down, after the author has released the content, and
            // there is nothing left locally to derive an expected answer from.
            // A round asks what it can, and the release decides on the result.
            let audit =
                session::audit(&author.store, &mut client, session::CHALLENGES_PER_ROUND).unwrap();
            assert_eq!(
                audit.failed, 0,
                "an honest host failed a challenge it should have answered"
            );
            report
        })
    };

    push_to(&host_one);
    push_to(&host_two);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let earned = author.store.holder_evidence(&chunks[0], now).unwrap();
    assert_eq!(
        earned.live, 2,
        "two real pushes did not produce two holders"
    );
    assert_eq!(
        earned.fresh, 2,
        "the records were not fresh for this chunk, which is what a release needs"
    );
    assert_eq!(
        earned.proved, 2,
        "the hosts were believed rather than asked, which is the bar a release needs"
    );

    match author.store.release("real.bin", now).unwrap() {
        itsanas_store::Release::Gone(_) => {}
        other => panic!("a release backed by two real hosts was refused: {other:?}"),
    }
    assert!(
        author.store.read_file("real.bin").unwrap().is_none(),
        "nothing was released, so the rest of this test proves nothing"
    );

    // One host throws the data away. The author no longer holds the bytes, so
    // no storage challenge can catch this — `audit` re-derives the expected
    // ciphertext from a local copy that does not exist any more. Before the
    // ledger sweep covered released chunks, NOTHING could catch it, and the
    // author went on reporting two copies for ever.
    for chunk in &chunks {
        assert!(
            host_one
                .vault
                .remove_chunk(author.store.owner(), chunk)
                .unwrap(),
            "the host was not holding what it acknowledged"
        );
    }

    push_to(&host_one);

    let after = author.store.holder_evidence(&chunks[0], now).unwrap();
    assert_eq!(
        after.live, 1,
        "the ledger still counts a host that threw the chunk away"
    );

    // And the consequence that matters: with one holder left, the next release
    // of that content would be refused rather than taking it to zero.
    author.store.write_file("real.bin", &payload).unwrap();
    match author.store.release("real.bin", now).unwrap() {
        itsanas_store::Release::NotSafeYet { evidence, .. } => assert_eq!(evidence.live, 1),
        other => panic!("released down to a single copy: {other:?}"),
    }
}

#[test]
fn a_round_that_has_nothing_to_say_says_it_in_one_hash() {
    // The cost that made a terabyte impossible. A round asked its peer about
    // every chunk it held, every time -- thirty-two bytes each, per round, per
    // peer, which is a two-thousandth of the account each round and a hundred
    // and forty gigabytes a day at a terabyte. It paid that in full to learn
    // what is almost always "nothing has changed".
    //
    // A summary answers that in one hash whatever the account weighs, and when
    // the two sides disagree it says *where*, so only that slice is listed.
    // Nothing is judged on a summary: what follows a mismatch is the same
    // have/missing exchange as before.
    let author = node(&alice(), 95);
    let host = node(&MasterSecret::from_bytes([0xD3; 32]), 96);

    for index in 0..8 {
        let payload = itsanas_testkit::filler(&format!("recon-{index}"), 128 * 1024);
        author
            .store
            .write_file(&format!("file-{index}.bin"), &payload)
            .unwrap();
    }
    author.store.flush_segment().unwrap();
    let held = author.store.stats().unwrap().live_chunks;
    assert!(held > 8, "the fixture is too small: {held} chunks");

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).unwrap();

        let first = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            first.chunks_asked_about, held,
            "the first round should list everything: the host has none of it"
        );
        assert!(first.chunks_accepted > 0, "the host took nothing");

        // Nothing has changed, and the first round was itself the full walk the
        // ledger is owed. Saying "the same" now costs one hash rather than one
        // identifier per chunk.
        let second = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            second.chunks_asked_about, 0,
            "a round with nothing to say listed {} chunks",
            second.chunks_asked_about
        );
        assert_eq!(second.chunks_offered, 0, "the host was sent data it had");

        // One new file. The summary locates the disagreement, so the round
        // lists a slice of the id space rather than the account.
        author
            .store
            .write_file("late.bin", &itsanas_testkit::filler("late", 128 * 1024))
            .unwrap();
        author.store.flush_segment().unwrap();
        let after = author.store.stats().unwrap().live_chunks;

        let third = session::push(&author.store, &mut client).unwrap();
        assert!(
            third.chunks_accepted > 0,
            "the new file never reached the host"
        );
        assert!(
            third.chunks_asked_about < after,
            "a change listed the whole account again: {} of {after}",
            third.chunks_asked_about
        );
    });
}

#[test]
fn a_peer_that_never_agrees_still_gets_its_ledger_walked() {
    // The defect a review found in the first version of the reconciliation, and
    // the one that would have reached a person as "ITSaNAS says my data is
    // nowhere and refuses to free any space".
    //
    // The freshness guard was consulted only in the branch where the two sides
    // *agree*. A peer whose storage budget is smaller than the account
    // disagrees on every round, for ever, by design -- so against such a peer
    // the walk was never due, never performed, never stamped, and the chunks in
    // the agreeing buckets were re-stamped by nobody. Fourteen days later
    // `coverage` reports no copies and `release` refuses, on an account where
    // nothing has gone wrong and a host that is behaving perfectly.
    //
    // The question has to be asked before the verdict, not inside one arm of
    // it.
    let author = node(&alice(), 97);
    let host = node(&MasterSecret::from_bytes([0xE4; 32]), 98);

    for index in 0..6 {
        let payload = itsanas_testkit::filler(&format!("rot-{index}"), 128 * 1024);
        author
            .store
            .write_file(&format!("rot-{index}.bin"), &payload)
            .unwrap();
    }
    author.store.flush_segment().unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let peer_device = host.device.device_id();
    assert!(
        author.store.ledger_walk_due(&peer_device, now).unwrap(),
        "a peer never spoken to should be owed a walk"
    );

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).unwrap();

        // The host holds nothing, so this round disagrees about every bucket.
        let report = session::push(&author.store, &mut client).unwrap();
        assert!(report.chunks_accepted > 0, "the host took nothing");
    });

    assert!(
        !author.store.ledger_walk_due(&peer_device, now).unwrap(),
        "a round that disagreed left the ledger owed a walk that would never \
         come, because the debt is only ever read in the branch where the two \
         sides agree"
    );
}

/// Every chunk the author's index holds, in id order.
fn all_live_chunks(store: &Store) -> Vec<ChunkId> {
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
fn red_team_a_change_asks_only_about_what_the_peer_has_not_confirmed() {
    // The cost that kept a terabyte out of budget after the summary existed.
    // The summary names the buckets where the two sides differ, and the round
    // then listed every chunk in them: at a terabyte a bucket is sixty-five
    // thousand chunks and 2.1 MB, so one saved photograph cost 2.1 MB per peer
    // and the 100 MB/day budget bought three megabytes of change a day.
    //
    // Within a named bucket, a chunk the peer confirmed holding within
    // REFRESH_AFTER is not asked about again. Here: after a full first round,
    // one new file must cost exactly its own new chunks, even though other,
    // already-confirmed chunks share their buckets.
    let author = node(&alice(), 99);
    let host = node(&MasterSecret::from_bytes([0xD5; 32]), 100);

    for index in 0..48 {
        let payload = itsanas_testkit::filler(&format!("crowd-{index}"), 128 * 1024);
        author
            .store
            .write_file(&format!("crowd-{index}.bin"), &payload)
            .unwrap();
    }
    author.store.flush_segment().unwrap();
    let before: std::collections::BTreeSet<_> =
        all_live_chunks(&author.store).into_iter().collect();

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).unwrap();

        let first = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            first.chunks_asked_about,
            before.len(),
            "the first round is a full walk"
        );

        author
            .store
            .write_file(
                "late.bin",
                &itsanas_testkit::filler("late-crowd", 1024 * 1024),
            )
            .unwrap();
        author.store.flush_segment().unwrap();
        let fresh: Vec<_> = all_live_chunks(&author.store)
            .into_iter()
            .filter(|chunk| !before.contains(chunk))
            .collect();
        assert!(!fresh.is_empty(), "the new file added no chunk");

        // The fixture has to put confirmed chunks in the new chunks' buckets,
        // or asking about the whole bucket and asking about the new chunks
        // would be the same number and this test would prove nothing.
        let crowd = before
            .iter()
            .filter(|old| {
                fresh.iter().any(|new| {
                    itsanas_store::summary::bucket_of(new) == itsanas_store::summary::bucket_of(old)
                })
            })
            .count();
        assert!(
            crowd > 0,
            "no confirmed chunk shares a bucket with the change"
        );

        let third = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            third.chunks_asked_about,
            fresh.len(),
            "a change of {} chunks asked about {} -- the {crowd} chunks the peer \
             confirmed in the same buckets were listed again, which is what \
             costs 2.1 MB a bucket at a terabyte",
            fresh.len(),
            third.chunks_asked_about
        );
        assert!(
            third.chunks_accepted > 0,
            "the new file never reached the host"
        );
    });
}

#[test]
fn red_team_a_chunk_the_peer_silently_dropped_is_found_by_the_next_full_walk() {
    // What narrowing gives up, and the bound on it. A narrowed round takes a
    // fresh record at its word, so a peer that throws away a chunk it recently
    // confirmed -- and says nothing -- is not caught by it. The full walk of the
    // ledger asks about everything whatever the records say, and it is due
    // REFRESH_AFTER after the last one, inside LIVE_FOR. If the freshness
    // filter ever reaches the full walk too, a silent drop is never found by
    // the sweep at all, and the author counts a copy that does not exist until
    // an audit happens to land on that one chunk.
    let author = node(&alice(), 101);
    let host = node(&MasterSecret::from_bytes([0xD6; 32]), 102);

    for index in 0..6 {
        let payload = itsanas_testkit::filler(&format!("drop-{index}"), 128 * 1024);
        author
            .store
            .write_file(&format!("drop-{index}.bin"), &payload)
            .unwrap();
    }
    author.store.flush_segment().unwrap();
    let victim = all_live_chunks(&author.store)[0];
    let peer = host.device.device_id();

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).unwrap();
        session::push(&author.store, &mut client).unwrap();

        assert!(
            host.vault
                .remove_chunk(author.store.owner(), &victim)
                .unwrap(),
            "the host was not holding what it acknowledged"
        );

        // The accepted cost: a narrowed round does not ask about it.
        let narrowed = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            narrowed.chunks_offered, 0,
            "the narrowed round asked about a fresh record"
        );

        // REFRESH_AFTER later the walk is due. Simulated by moving the stamp
        // back rather than the clock forward.
        author.store.note_ledger_walk(&peer, 0).unwrap();
        let full = session::push(&author.store, &mut client).unwrap();
        assert!(
            full.chunks_offered >= 1,
            "the full walk did not ask about a chunk the peer silently dropped: \
             the freshness filter reached the walk that exists to bound it"
        );
    });

    assert!(
        host.vault.has_chunk(author.store.owner(), &victim).unwrap(),
        "the dropped chunk was not put back"
    );
}

#[test]
fn red_team_a_full_peer_is_not_sent_what_it_refused_round_after_round() {
    // A host whose pledge is smaller than the account refuses what does not
    // fit, and a refusal leaves no holder record -- so every following round
    // found those chunks unconfirmed, asked about them, and sent their bytes to
    // be refused again. Against a phone holding a slice of a terabyte that is
    // the whole unheld remainder uploaded every five minutes.
    //
    // Three things must hold: a round stops offering at the first refusal; the
    // next round, inside FULL_RETRY, sends nothing and asks only about what the
    // peer is recorded as holding; and once FULL_RETRY has passed, a round
    // offers again, or a peer whose owner raised its pledge never gets more.
    let author = node(&alice(), 103);
    let host = node(&MasterSecret::from_bytes([0xD7; 32]), 104);

    for index in 0..8 {
        let payload = itsanas_testkit::filler(&format!("full-{index}"), 128 * 1024);
        author
            .store
            .write_file(&format!("full-{index}.bin"), &payload)
            .unwrap();
    }
    author.store.flush_segment().unwrap();
    let peer = host.device.device_id();

    with_server(&host, Pledge::bytes(256 * 1024), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).unwrap();

        let first = session::push(&author.store, &mut client).unwrap();
        assert!(
            first.chunks_accepted > 0,
            "the fixture's pledge took nothing"
        );
        assert_eq!(
            first.refusal,
            Some(Refusal::PledgeFull),
            "the host never filled up"
        );
        assert_eq!(
            first.chunks_offered,
            first.chunks_accepted + 1,
            "the round kept sending chunks after the host said it was full: \
             every one of them is bytes on the wire to be refused"
        );

        let second = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            (second.chunks_offered, second.bytes_sent),
            (0, 0),
            "a host that refused for a full pledge was sent chunks again on the \
             next round"
        );
        assert!(
            second.chunks_asked_about <= first.chunks_accepted,
            "the full host was asked about {} chunks, more than the {} it holds: \
             at a terabyte that is every chunk it lacks, every round",
            second.chunks_asked_about,
            first.chunks_accepted
        );

        // FULL_RETRY later. Simulated by moving the stamp back.
        author.store.note_peer_full(&peer, 0).unwrap();
        author.store.note_ledger_walk(&peer, 0).unwrap();
        let retry = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            retry.chunks_offered, 1,
            "once FULL_RETRY has passed the round must probe with one offer, or a \
             host given more room is never offered anything again"
        );
    });
}

/// Six files pushed in full to `host`, the round that also stamps the walk.
fn pushed_to(author: &Node, address: std::net::SocketAddr, tag: &str) -> Vec<ChunkId> {
    for index in 0..6 {
        let payload = itsanas_testkit::filler(&format!("{tag}-{index}"), 128 * 1024);
        author
            .store
            .write_file(&format!("{tag}-{index}.bin"), &payload)
            .unwrap();
    }
    author.store.flush_segment().unwrap();
    let mut client =
        PeerClient::connect(address, &author.device, author.store.owner(), None).unwrap();
    session::push(&author.store, &mut client).unwrap();
    all_live_chunks(&author.store)
}

/// `REFRESH_AFTER` later, without waiting for it: the walk is due and every
/// record is old. Moving the stamps back rather than the clock forward.
fn age_everything(author: &Node, peer: &itsanas_crypto::DeviceId, chunks: &[ChunkId]) {
    author.store.forget_holders(chunks, peer).unwrap();
    author.store.record_holders_at(chunks, peer, 1).unwrap();
    author.store.note_ledger_walk(peer, 0).unwrap();
}

#[test]
fn red_team_an_idle_due_walk_lists_nothing_and_keeps_the_records_fresh() {
    // §8 4c, the number the step exists for. A due walk used to list the whole
    // account per peer every REFRESH_AFTER: 537 MB every three and a half days
    // at a terabyte, about 150 MB a day, over the 100 MB budget on its own.
    // Against a peer whose summary agrees it must now list nothing and still
    // leave every record fresh. Sabotages: list everything on a due walk (the
    // count comes back); skip the re-stamp (the records age out).
    let author = node(&alice(), 105);
    let host = node(&MasterSecret::from_bytes([0xD8; 32]), 106);
    let peer = host.device.device_id();

    with_server(&host, Pledge::gigabytes(1), |address| {
        let chunks = pushed_to(&author, address, "idle");
        age_everything(&author, &peer, &chunks);

        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).unwrap();
        let due = session::push(&author.store, &mut client).unwrap();
        assert_eq!(
            due.chunks_asked_about, 0,
            "an idle due walk listed {} chunks: at a terabyte that is the 537 MB per peer \
             this step removes",
            due.chunks_asked_about
        );
        assert_eq!(due.holders_restamped, chunks.len());

        let now = itsanas_store::now_unix();
        for chunk in &chunks {
            assert!(
                author.store.holder_evidence(chunk, now).unwrap().fresh >= 1,
                "a chunk the agreeing host holds was left with a stale record: in LIVE_FOR \
                 repair counts it as gone and copies it again"
            );
        }
        assert!(
            !author.store.ledger_walk_due(&peer, now).unwrap(),
            "the walk was not stamped, so every round would be a due walk"
        );
    });
}

#[test]
fn red_team_a_host_that_lost_a_blob_behind_its_index_is_caught_by_the_next_due_walk() {
    // The downside §10 8 names, and why the host checks its index against its
    // disk. The host's summary is read from its index; a blob gone from its
    // disk behind the index left that summary agreeing, and since 4c an
    // agreeing summary is believed without asking. The rolling check removes
    // the row, the summary changes, and the owner's due walk lists the bucket,
    // finds the chunk missing and sends it again. Sabotage: the check keeps the
    // row.
    let author = node(&alice(), 107);
    let host = node(&MasterSecret::from_bytes([0xD9; 32]), 108);
    let peer = host.device.device_id();

    with_server(&host, Pledge::gigabytes(1), |address| {
        let chunks = pushed_to(&author, address, "lost");
        let victim = chunks[0];
        let owner = author.store.owner();
        let blobs = itsanas_store::BlobStore::open(
            host.dir
                .path()
                .join("vault")
                .join("owners")
                .join(owner.to_hex()),
        )
        .unwrap();
        blobs.damage(&victim).unwrap();
        assert!(!host.vault.has_chunk(owner, &victim).unwrap());

        // One pass of the host's rolling check.
        let rows = host.vault.chunk_rows().unwrap();
        host.vault
            .check_disk(usize::try_from(rows).unwrap() + 1)
            .unwrap();

        age_everything(&author, &peer, &chunks);
        let mut client = PeerClient::connect(address, &author.device, owner, None).unwrap();
        let due = session::push(&author.store, &mut client).unwrap();
        assert!(
            due.chunks_offered >= 1,
            "the due walk believed the host's summary about a chunk it no longer has: the \
             author counts a copy that does not exist"
        );
        assert!(
            due.chunks_asked_about < chunks.len(),
            "the walk listed the whole account to find one lost chunk"
        );
    });

    assert!(
        host.vault
            .has_chunk(author.store.owner(), &chunks_victim(&author))
            .unwrap(),
        "the lost chunk was not sent again"
    );
}

/// The first live chunk, the one the test above loses.
fn chunks_victim(author: &Node) -> ChunkId {
    all_live_chunks(&author.store)[0]
}
