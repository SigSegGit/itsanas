//! Which host gets which chunk: spread evenly, past a full host, the
//! safety net after a day, and the due walk that asks only what it recorded.

use super::*;

#[test]
fn chunks_spread_over_five_hosts_about_evenly() {
    // Every chunk already has one proven copy with an anchor host, so each
    // needs one more account: the best-ranked of five. Spread is what the
    // rendezvous order gives, and a push to each host in turn sends each
    // chunk once. Sabotage: offer every short chunk to every host.
    let now = now_unix();
    let me = node(0xA1, 1);
    let chunks = write(&me, "photos/a.bin", 1, 1 << 20);
    let n = chunks.len();
    assert!(
        n >= 60,
        "the fixture needs enough chunks to measure a spread: {n}"
    );

    let hosts: Vec<Node> = (0..5).map(|i| node(0xB0 + i, 10 + i)).collect();
    let mut accounts = BTreeMap::new();
    let anchor = fake(0xEE);
    proven(&me.store, &anchor, now);
    accounts.insert(anchor, user(0xEE));
    me.store.record_holders_at(&chunks, &anchor, now).unwrap();
    for host in &hosts {
        proven(&me.store, &host.id(), now);
        accounts.insert(host.id(), host.store.owner());
    }

    let placement = placement(&me, &accounts, now);
    let taken: Vec<usize> = hosts
        .iter()
        .map(|host| push_plain(&me, host, &placement).chunks_accepted)
        .collect();

    assert_eq!(
        taken.iter().sum::<usize>(),
        n,
        "each chunk once, nowhere twice: {taken:?}"
    );
    for share in &taken {
        assert!(
            *share >= n / 10 && *share <= n * 3 / 10,
            "one fifth each, within a bound: {taken:?} of {n}"
        );
    }
    assert_eq!(me.store.short_count().unwrap(), 0, "every chunk satisfied");
}

#[test]
fn a_host_full_under_either_refusal_falls_through_to_the_next() {
    // A full host is ineligible from the next round on, and its share goes
    // to the next host in the order. Both refusals, the host's pledge and the
    // account's share, count. Sabotage: stamp `note_peer_full` only for
    // `PledgeFull` in a placed push.
    for account_full in [false, true] {
        let now = now_unix();
        let me = node(0xA1, 1);
        let chunks = write(&me, "photos/a.bin", 2, 512 << 10);
        let full = node(0xB1, 11);
        let next = node(0xB2, 12);
        let anchor = fake(0xEE);
        let mut accounts = BTreeMap::new();
        proven(&me.store, &anchor, now);
        accounts.insert(anchor, user(0xEE));
        me.store.record_holders_at(&chunks, &anchor, now).unwrap();
        for host in [&full, &next] {
            proven(&me.store, &host.id(), now);
            accounts.insert(host.id(), host.store.owner());
        }

        let gate = AccountFullFor(me.id(), me.store.owner());
        let refusing = if account_full {
            PeerService::new(&full.store, &full.vault, Pledge::gigabytes(1)).with_owners(&gate)
        } else {
            PeerService::new(&full.store, &full.vault, Pledge::bytes(0))
                .with_owners(&itsanas_net::Unscoped)
        };
        let expected = if account_full {
            Refusal::AccountFull
        } else {
            Refusal::PledgeFull
        };

        let first = placement(&me, &accounts, now);
        let refused = push_to(&me, &full, &refusing, &first);
        assert_eq!(refused.refusal, Some(expected), "{refused:?}");
        assert_eq!(refused.chunks_accepted, 0);
        let early = push_plain(&me, &next, &first).chunks_accepted;
        assert!(early < chunks.len(), "the full host's share waits a round");

        let second = placement(&me, &accounts, now + 60);
        let late = push_plain(&me, &next, &second).chunks_accepted;
        assert_eq!(
            early + late,
            chunks.len(),
            "account_full={account_full}: the full host's share fell through"
        );
    }
}

#[test]
fn a_chunk_with_no_proven_copy_goes_to_the_first_host_reached() {
    // No delay, no ranking: a new photograph's first copy goes to whoever
    // answers, as before placement. Once a proven host holds it, a stranger
    // reached later is offered nothing. Sabotage: drop the "no proven remote
    // holder" rule.
    let now = now_unix();
    let me = node(0xA1, 1);
    let chunks = write(&me, "photos/new.bin", 3, 256 << 10);
    let stranger = node(0xB1, 11);

    let report = push_plain(&me, &stranger, &placement(&me, &BTreeMap::new(), now));
    assert_eq!(report.chunks_accepted, chunks.len(), "{report:?}");

    // The stranger proves itself; a second stranger now gets nothing.
    proven(&me.store, &stranger.id(), now);
    let mut accounts = BTreeMap::new();
    accounts.insert(stranger.id(), stranger.store.owner());
    let later = node(0xB2, 12);
    let report = push_plain(&me, &later, &placement(&me, &accounts, now));
    assert_eq!(report.chunks_offered, 0, "{report:?}");
}

#[test]
fn red_team_the_safety_net_waits_a_day_and_serves_only_durability() {
    // The host shares the anchor's account, so it is never wanted: the
    // chunk needs another account, and that host is ranked. Only after a day
    // without durability does any eligible host do. Sabotage: drop the
    // SAFETY_GRACE comparison, or the `!durable` condition.
    let now = now_unix();
    let me = node(0xA1, 1);
    let chunks = write(&me, "photos/a.bin", 4, 256 << 10);
    let host = node(0xB1, 11);
    let anchor = fake(0xEE);
    let ranked = fake(0xEF);
    let mut accounts = BTreeMap::new();
    for (device, account) in [
        (anchor, user(0xEE)),
        (ranked, user(0xEF)),
        (host.id(), user(0xEE)),
    ] {
        proven(&me.store, &device, now);
        accounts.insert(device, account);
    }
    me.store.record_holders_at(&chunks, &anchor, now).unwrap();
    let placement = placement(&me, &accounts, now);

    me.store.settle_short(&chunks).unwrap();
    me.store.enqueue_short(&chunks, now - 23 * HOUR).unwrap();
    let report = push_plain(&me, &host, &placement);
    assert_eq!(report.chunks_offered, 0, "under a day: {report:?}");

    me.store.settle_short(&chunks).unwrap();
    me.store.enqueue_short(&chunks, now - 25 * HOUR).unwrap();
    let report = push_plain(&me, &host, &placement);
    assert_eq!(
        report.chunks_accepted,
        chunks.len(),
        "past a day: {report:?}"
    );
}

#[test]
fn red_team_a_capped_chunk_short_only_on_availability_is_not_sent_everywhere() {
    // Two proven accounts that are each up half the time: durable, not
    // available, and twelve remote copies counting the unproven ones. A day
    // later still nobody new is offered it: that is the chunk "send it to
    // whoever answers" would send to every host. The control removes one
    // unproven copy, under the cap, and the host is wanted again.
    // Sabotage: let the safety net take a durable chunk, or drop the cap.
    for unproven in [10u8, 9] {
        let now = now_unix();
        let me = node(0xA1, 1);
        let chunks = write(&me, "photos/a.bin", 5, 128 << 10);
        let host = node(0xB1, 11);
        let mut accounts = BTreeMap::new();
        for byte in [0xE0, 0xE1] {
            history(&me.store, &fake(byte), now, |index| index % 2 == 0);
            me.store.note_audit(&fake(byte), true).unwrap();
            accounts.insert(fake(byte), user(byte));
            me.store
                .record_holders_at(&chunks, &fake(byte), now)
                .unwrap();
        }
        for byte in 0..unproven {
            me.store
                .record_holders_at(&chunks, &fake(byte), now)
                .unwrap();
        }
        proven(&me.store, &host.id(), now);
        accounts.insert(host.id(), host.store.owner());

        me.store.settle_short(&chunks).unwrap();
        me.store.enqueue_short(&chunks, now - 48 * HOUR).unwrap();
        let report = push_plain(&me, &host, &placement(&me, &accounts, now));
        if unproven == 10 {
            assert_eq!(report.chunks_offered, 0, "capped: {report:?}");
        } else {
            assert_eq!(
                report.chunks_accepted,
                chunks.len(),
                "under the cap: {report:?}"
            );
        }
    }
}

#[test]
fn the_due_walk_restamps_on_recorded_only_digests_and_asks_only_recorded_chunks() {
    // The host holds the first file only. Against this device's whole set
    // every bucket would differ; against what is recorded for the host they
    // agree, so the walk asks nothing. Then the host loses one chunk: only
    // that bucket's recorded chunks are asked about, and the lost one is
    // sent back by the short queue (one more question). Sabotage: hash the
    // whole bucket, or ask the whole bucket.
    let wall = now_unix();
    let me = node(0xA1, 1);
    let held = write(&me, "photos/a.bin", 6, 256 << 10);
    let host = node(0xB1, 11);
    serving(
        &host,
        &PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
            .with_owners(&itsanas_net::Unscoped),
        |address| {
            let mut client =
                PeerClient::connect(address, &me.device, me.store.owner(), None).expect("dial");
            session::push(&me.store, &mut client).expect("push");
        },
    );
    let extra = write(&me, "photos/b.bin", 7, 256 << 10);
    me.store.settle_short(&live(&me)).unwrap();
    proven(&me.store, &host.id(), wall);
    let mut accounts = BTreeMap::new();
    accounts.insert(host.id(), host.store.owner());

    let now = wall + REFRESH_AFTER + 60;
    let report = push_plain(&me, &host, &placement(&me, &accounts, now));
    assert_eq!(report.chunks_asked_about, 0, "{report:?}");
    assert_eq!(report.holders_restamped, held.len(), "{report:?}");
    assert_ne!(extra, [] as [itsanas_crypto::ChunkId; 0]);

    let lost = held[0];
    assert!(host.vault.remove_chunk(me.store.owner(), &lost).unwrap());
    let in_bucket = held
        .iter()
        .filter(|chunk| chunk.as_bytes()[0] == lost.as_bytes()[0])
        .count();
    let later = now + REFRESH_AFTER + 60;
    me.store.note_seen_at(&host.id(), later).unwrap();
    let report = push_plain(&me, &host, &placement(&me, &accounts, later));
    assert_eq!(
        report.chunks_asked_about,
        in_bucket + 1,
        "the bucket's recorded chunks, then the lost one offered back: {report:?}"
    );
    assert_eq!(report.holders_restamped, held.len() - in_bucket);
    assert_eq!(report.chunks_accepted, 1, "the lost chunk went back");
}

#[test]
fn red_team_a_session_that_moves_nothing_evaluates_each_queued_chunk_once() {
    // Re-evaluating every offer after the exchange, whatever it moved,
    // doubled the CPU of a session refused everything. Sabotage: evaluate
    // every offered chunk again after `ask`.
    let now = now_unix();
    let me = node(0xA1, 1);
    let chunks = write(&me, "photos/new.bin", 12, 48 << 10);
    let host = node(0xB1, 11);
    proven(&me.store, &host.id(), now);
    let accounts: BTreeMap<DeviceId, UserId> = [(host.id(), user(0xB1))].into_iter().collect();
    let placement = placement(&me, &accounts, now);
    let queued = usize::try_from(me.store.short_count().unwrap()).unwrap();
    assert!(queued >= chunks.len() && queued > 1);

    let gate = AccountFullFor(me.id(), me.store.owner());
    let service =
        PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1)).with_owners(&gate);
    let report = push_to(&me, &host, &service, &placement);
    assert_eq!(report.chunks_accepted, 0, "{report:?}");
    assert!(
        placement.evaluations() <= u64::try_from(queued).unwrap(),
        "{} evaluations for {queued} queued chunks",
        placement.evaluations()
    );
}
