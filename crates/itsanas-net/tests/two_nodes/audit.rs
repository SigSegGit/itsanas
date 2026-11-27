//! Auditing: making a host prove it still has what it said it had.

use super::*;

#[test]
fn an_audit_confirms_a_host_that_is_still_holding_the_data() {
    // The ledger records that a peer *accepted* a chunk. That is evidence, not
    // proof — a host that accepted and then deleted looks identical from here.
    // An audit is what turns one into the other, for the moment it is asked.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    owner
        .store
        .write_file("audited.txt", b"prove you are still holding this")
        .expect("write");
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");

        let report = session::audit(&owner.store, &mut client, 16).expect("audit");
        assert!(report.asked > 0, "the audit checked nothing at all");
        assert_eq!(report.confirmed, report.asked);
        assert_eq!(report.failed, 0);
        assert!(!report.found_a_liar());
    });
}

/// One of the account's own devices that let chunks go (a phone or a key over
/// its budget) loses those records and nothing else: no failure, no sanction,
/// and a sanction recorded before it was known as one of ours is lifted. "Ours"
/// is the roster, whatever the placement mode. The same scenario without the
/// roster is `red_team_a_host_that_threw_the_data_away_stops_counting_as_a_holder`.
/// Sabotage: count a member's failure; show a member's old sanction; read
/// membership as false.
#[test]
fn red_team_an_own_device_that_let_chunks_go_is_not_called_a_liar() {
    let master = alice();
    let owner = node(&master, 1);
    let phone = node(&master, 2);
    let chunks = owner
        .store
        .write_file("kept.txt", b"kept on the phone until its budget said no")
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");
    with_server(&phone, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });
    for chunk in &chunks {
        phone
            .vault
            .remove_chunk(owner.store.owner(), chunk)
            .expect("let go");
    }
    // A sanction from before, as 0.1.18 left on every PC, then the phone's
    // claim admits it to the roster.
    let id = phone.device.device_id();
    for _ in 0..itsanas_store::FAILURES_BEFORE_PAUSE {
        owner.store.note_audit(&id, false).expect("old");
    }
    owner.store.admit_member(&id, false).expect("admit");
    with_server(&phone, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        let report = session::audit(&owner.store, &mut client, 16).expect("audit");
        assert!(
            !report.found_a_liar() && report.own,
            "the account's own phone was called a liar for its budget: {report:?}"
        );
        assert!(
            report
                .record
                .is_some_and(|record| !record.paused && record.consecutive_failures == 0),
            "the account's own phone is paused for what it let go: {report:?}"
        );
    });
    assert!(
        owner
            .store
            .unreliable_devices()
            .expect("records")
            .is_empty(),
        "an own device that let chunks go keeps the icon orange, and nothing it can do lifts it"
    );
    for chunk in &chunks {
        assert!(
            owner
                .store
                .remote_holders(chunk)
                .expect("holders")
                .is_empty(),
            "a chunk the phone let go still counts as a copy on it"
        );
    }
}

/// `Hosted` is a word: it records only chunks this account uses, and nothing
/// for a host paused for lying. Sabotage: record every id; drop the pause
/// check.
#[test]
fn red_team_hosted_records_only_this_accounts_chunks_and_nothing_from_a_paused_host() {
    let owner = node(&alice(), 1);
    let stranger = node(&MasterSecret::from_bytes([0xB2; 32]), 2);
    let chunks = owner
        .store
        .write_file("mine.txt", b"chunks of this account")
        .expect("write")
        .chunks;
    let minted = ChunkId::from_bytes([0x5A; 32]);
    let id = stranger.device.device_id();
    with_server(&owner, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &stranger.device, stranger.store.owner(), None)
                .expect("dial");
        client.hosted(vec![minted, chunks[0]]).expect("hosted");
        assert!(
            owner
                .store
                .remote_holders(&minted)
                .expect("holders")
                .is_empty(),
            "an id this account never had got a holder row: minted ids fill the index for ever"
        );
        assert_eq!(
            owner
                .store
                .remote_holders(&chunks[0])
                .expect("holders")
                .len(),
            1,
            "an honest claim was not recorded, so this test proves nothing"
        );
        owner.store.forget_holders(&chunks, &id).expect("forget");
        for _ in 0..itsanas_store::FAILURES_BEFORE_PAUSE {
            owner.store.note_audit(&id, false).expect("fail");
        }
        client.hosted(chunks.clone()).expect("hosted");
        assert!(
            owner
                .store
                .remote_holders(&chunks[0])
                .expect("holders")
                .is_empty(),
            "a host paused for lying is recorded as a holder again by saying so"
        );
    });
}

#[test]
fn red_team_a_host_that_threw_the_data_away_stops_counting_as_a_holder() {
    // THE ATTACK, and it costs nothing: accept everything offered, delete it
    // immediately, and keep claiming the space. A node that trusted its own
    // ledger would believe its files were on three machines while two of them
    // held nothing, and would find out on the day the third disk died.
    //
    // Passing an audit does not prove a host will still have the bytes
    // tomorrow — that limit is documented — but silently discarding them has to
    // stop being free.
    //
    // If this test fails, the placement ledger is a list of promises with
    // nothing checking any of them.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    let chunks = owner
        .store
        .write_file("audited.txt", b"prove you are still holding this")
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    for chunk in &chunks {
        assert_eq!(
            owner.store.remote_holders(chunk).expect("holders").len(),
            1,
            "the push was not recorded, so this test would prove nothing"
        );
    }

    // The host quietly deletes what it accepted.
    for chunk in &chunks {
        assert!(
            host.vault
                .remove_chunk(owner.store.owner(), chunk)
                .expect("discard"),
            "the host did not actually hold what it was asked to discard"
        );
    }

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        let report = session::audit(&owner.store, &mut client, 16).expect("audit");

        assert!(
            report.found_a_liar(),
            "a host that discarded everything passed its audit: {report:?}"
        );
        assert_eq!(report.confirmed, 0);
    });

    for chunk in &chunks {
        assert!(
            owner
                .store
                .remote_holders(chunk)
                .expect("holders")
                .is_empty(),
            "a host that failed its audit is still recorded as a holder"
        );
    }

    // And the consequence the owner actually cares about: the data now shows
    // as existing nowhere else, which is what repair acts on.
    assert!(
        owner
            .store
            .under_replicated(2, itsanas_store::now_unix())
            .expect("risk")
            .iter()
            .all(itsanas_store::AtRisk::only_copy),
        "the withdrawal did not make the chunk show as under-replicated"
    );
}

#[test]
fn an_audit_never_asks_about_a_chunk_it_could_not_check() {
    // Verifying a proof means re-deriving the sealed bytes locally. A chunk
    // this device has collected cannot be re-derived, so challenging on it
    // would fail for a reason that is nothing to do with the peer — and would
    // withdraw a perfectly good record.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    let chunks = owner
        .store
        .write_file("audited.txt", b"content that will be collected locally")
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    // The owner loses its own copy while the host keeps its.
    for chunk in &chunks {
        owner.store.blobs().remove(chunk).expect("drop local copy");
    }

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        let report = session::audit(&owner.store, &mut client, 16).expect("audit");

        assert_eq!(
            report.asked, 0,
            "it challenged on something it cannot verify"
        );
        assert!(report.unverifiable > 0, "the skip was not reported");
        assert!(!report.found_a_liar());
    });

    for chunk in &chunks {
        assert_eq!(
            owner.store.remote_holders(chunk).expect("holders").len(),
            1,
            "an honest host lost its record because the owner could not check"
        );
    }
}

#[test]
fn red_team_a_host_that_keeps_discarding_stops_getting_free_uploads() {
    // THE ATTACK, and it is the one auditing alone does not stop. Accept
    // everything, delete it, and wait. The audit catches it every round, the
    // owner re-uploads every round, and the host pays nothing. The more data
    // the owner has, the more it costs them — a free, indefinite drain on their
    // uplink in exchange for agreeing to store data and then not.
    //
    // Detection without memory is not a defence. If this test fails, anyone can
    // exhaust an owner's bandwidth by volunteering to help.
    //
    // What the defence is *not* is a cut-off: one probe chunk still goes each
    // round, because a peer with nothing recorded has nothing to be challenged
    // on and could never earn its way back. So this measures **volume**, which
    // is what the attack costs, rather than whether anything moved at all.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    // Large enough that "everything" and "one chunk" are not close.
    let mut payload = vec![0u8; 2_000_000];
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"bandwidth drain corpus");
    hasher.finalize_xof().fill(&mut payload);

    let chunks = owner
        .store
        .write_file("bait.bin", &payload)
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");
    assert!(
        chunks.len() > 8,
        "the corpus must be many chunks to be meaningful"
    );

    let mut bytes_per_round: Vec<u64> = Vec::new();

    for _ in 0..(itsanas_store::FAILURES_BEFORE_PAUSE + 3) {
        with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");

            // The audit runs first, exactly as the daemon orders it.
            let _ = session::audit(&owner.store, &mut client, 64).expect("audit");
            let report = session::push(&owner.store, &mut client).expect("push");
            bytes_per_round.push(report.bytes_sent);
        });

        // The host discards whatever it just took.
        for chunk in &chunks {
            let _ = host.vault.remove_chunk(owner.store.owner(), chunk);
        }
    }

    let first = bytes_per_round[0];
    let last = *bytes_per_round.last().expect("rounds");
    assert!(
        first > 1_000_000,
        "the first round did not upload the corpus"
    );
    assert!(
        last * 10 < first,
        "the {}th round still cost {last} bytes against the first round's          {first} — the drain was never cut off",
        bytes_per_round.len()
    );

    let record = owner
        .store
        .reliability(&host.store.device_id())
        .expect("record");
    assert!(!record.worth_sending_to());
    assert!(record.failed > 0);
}

/// Questions per audit round in these tests, matching
/// `session::CHALLENGES_PER_ROUND`.
const CHALLENGES: usize = 16;

#[test]
fn red_team_a_host_that_keeps_only_what_it_expects_to_be_asked_is_caught() {
    // THE ATTACK: accept the whole store, work out which chunks the audit will
    // ask about, keep exactly those, delete the rest. If the questions are
    // predictable the host holds a spotless record while storing a rounding
    // error — and for six commits they were not merely predictable, they were
    // a constant.
    //
    // The audit worked through the least recently confirmed records first,
    // which reads like diligence. But a push round re-stamps every record the
    // peer *claims* to hold, a whole batch from one clock reading, so within a
    // batch every timestamp was equal and the sort fell through to its
    // tie-break: the chunk id. The sixteen lowest ids, every round, for ever.
    // At a terabyte that is sixteen chunks out of fourteen million.
    //
    // So this attacker keeps precisely the sixteen lowest ids and nothing else.
    // Under the old rule it survives for ever. If this test fails, a host can
    // pledge a terabyte, store a megabyte, and never be caught by anything in
    // this system.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    // Four mebibytes: at the 64 KiB average chunk that is comfortably more than
    // twice CHALLENGES, which is all the attack needs. It was eight, and on a
    // hosted Windows runner this test took 39-42 s of its 60 s budget and once
    // crossed it; the time is per-chunk work, 5.2 s -> 1.8 s on a laptop.
    let chunks = owner
        .store
        .write_file("hostage.bin", &a_file_of_many_chunks(11, 4 << 20))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");
    assert!(
        chunks.len() > CHALLENGES * 2,
        "the fixture is {} chunks; with fewer than twice the questions per \
         round, keeping the answers is not an attack, it is just storing the \
         data",
        chunks.len()
    );

    // Exactly what the old selection rule would have asked, every round.
    let mut by_id: Vec<_> = chunks.clone();
    by_id.sort_unstable();
    let kept: std::collections::BTreeSet<_> = by_id.iter().take(CHALLENGES).copied().collect();

    let discard_everything_else = || {
        for chunk in &chunks {
            if !kept.contains(chunk) {
                let _ = host.vault.remove_chunk(owner.store.owner(), chunk);
            }
        }
    };

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });
    discard_everything_else();

    // Six rounds. Re-deleting after each one so the owner's own pushes cannot
    // accidentally repair what the attacker threw away.
    let mut caught_in = None;
    for round in 1..=6u32 {
        with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::audit(&owner.store, &mut client, CHALLENGES).expect("audit");
            if report.failed > 0 && caught_in.is_none() {
                caught_in = Some(round);
            }
        });
        discard_everything_else();
    }

    let held = kept.len();
    assert!(
        caught_in.is_some(),
        "a host holding {held} of {} chunks answered six rounds of {CHALLENGES} \
         questions without one failure. The questions are predictable, so the \
         audit proves nothing at all.",
        chunks.len()
    );
}

#[test]
fn a_paused_host_that_starts_answering_again_is_sent_data_again() {
    // The way back, and it has to actually exist — on a real store, not on a
    // fixture of one chunk.
    //
    // A paused peer is offered one chunk a round so it has something it can
    // prove. The first version left the audit to *find* that chunk in the
    // ledger, where it sat as one fresh record among however many thousand
    // stale ones the peer is paused for. Every question landed on a record the
    // peer had already lost, every round failed, and the sanction never lifted:
    // a ban wearing the words of a suspension. The test that was supposed to
    // catch that used a thirty-seven byte file — one chunk, one record, the
    // one case where finding the probe is guaranteed — and so it passed while
    // the mechanism it named did not work.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    let chunks = owner
        .store
        .write_file("bait.bin", &a_file_of_many_chunks(3, 2 << 20))
        .expect("write")
        .chunks;
    assert!(
        chunks.len() > 8,
        "the fixture is {} chunks, which is small enough for the degenerate \
         case to hide a broken selection rule again",
        chunks.len()
    );
    owner.store.flush_segment().expect("flush");

    let round = |discard: bool| {
        with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let _ = session::audit(&owner.store, &mut client, 16).expect("audit");
            let _ = session::push(&owner.store, &mut client).expect("push");
        });
        if discard {
            for chunk in &chunks {
                let _ = host.vault.remove_chunk(owner.store.owner(), chunk);
            }
        }
    };

    for _ in 0..=itsanas_store::FAILURES_BEFORE_PAUSE {
        round(true);
    }

    assert!(
        !owner
            .store
            .worth_sending_to(&host.store.device_id())
            .expect("record"),
        "the host was never paused, so this test would prove nothing"
    );

    // The host is repaired and keeps what it is given from now on. Each round
    // it is handed one chunk the *owner* picked, and the next round's audit
    // asks about that chunk and nothing else. Each answered round pays off one
    // failure.
    //
    // Both ends of that are the point. Recovery that never arrives was the
    // first version of this mechanism. Recovery in a single round was the
    // second, and it made the sanction free: keep one chunk for one round, get
    // the whole store back, discard it, repeat for ever. So this measures how
    // long it actually takes and asserts it is neither.
    let mut rounds = 0u32;
    while !owner
        .store
        .worth_sending_to(&host.store.device_id())
        .expect("record")
    {
        rounds += 1;
        assert!(
            rounds <= itsanas_store::PROBATION_CEILING + 8,
            concat!(
                "a host answering every question for {} rounds is still paused. ",
                "There is no way back, only a ban with a friendlier message"
            ),
            rounds
        );
        round(false);
    }

    assert!(
        rounds >= itsanas_store::FAILURES_BEFORE_PAUSE,
        concat!(
            "the pause lifted after {} answered rounds, having taken {} failures ",
            "to start. A sanction a host escapes faster than it earned costs it ",
            "one chunk for one round and nothing else"
        ),
        rounds,
        itsanas_store::FAILURES_BEFORE_PAUSE
    );

    // And once cleared, the rest of the data flows again.
    round(false);
    for chunk in &chunks {
        assert!(
            !owner
                .store
                .remote_holders(chunk)
                .expect("holders")
                .is_empty(),
            "the recovered host was never sent the rest of the data"
        );
    }

    // Throughout, the log kept flowing, which is what keeps a paused peer able
    // to relay for devices that have done nothing wrong.
    assert_ne!(
        host.vault.heads_for(owner.store.owner()).expect("heads"),
        [] as [(itsanas_crypto::DeviceId, itsanas_crypto::ObjectId, u64); 0]
    );
}

/// A host whose vault's disk is away answers an audit `HOSTING_AWAY`. The
/// owner must read that as "not here" -- an error that ends the audit with
/// no verdict -- and never as "not held", which would count a failed audit
/// against a host that only had its USB disk out. A plain refusal still is a
/// failed audit, which is the control.
#[test]
fn red_team_an_audit_answered_away_is_no_failed_audit() {
    let host = node(&alice(), 91);
    let me = node(&MasterSecret::from_bytes([0xB0; 32]), 92);
    let address = itsanas_crypto::ChunkId::from_bytes([5; 32]);
    let server = PeerServer::bind("127.0.0.1:0").expect("bind loopback");
    let at = server.local_addr().expect("local address");
    let shutdown = AtomicBool::new(false);
    let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
        .with_owners(&itsanas_net::Unscoped)
        .hosting_away(true);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &host.device, &shutdown);
        });
        let _stop = StopOnDrop(&shutdown, at);
        let mut client =
            PeerClient::connect(at, &me.device, me.store.owner(), None).expect("connect");
        match client.challenge(me.store.owner(), address, [7; 32], b"sealed") {
            Err(itsanas_net::NetError::Refused(why)) => {
                assert_eq!(why, itsanas_net::HOSTING_AWAY);
            }
            other => panic!("an away host's audit read as {other:?}, not as absence"),
        }
    });

    with_server(&host, Pledge::gigabytes(1), |at| {
        let mut client =
            PeerClient::connect(at, &me.device, me.store.owner(), None).expect("connect");
        assert!(
            !client
                .challenge(me.store.owner(), address, [7; 32], b"sealed")
                .expect("a verdict"),
            "a chunk the host does not hold passed its audit"
        );
    });
}
