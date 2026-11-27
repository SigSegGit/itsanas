//! Storing for a stranger: pledges, challenges, forged and repeated segments.

use super::*;

#[test]
fn a_node_stores_and_serves_a_strangers_chunk_without_reading_it() {
    // The whole mutual-storage bargain, in one test.
    let host = node(&alice(), 6);
    let guest_keys = UserKeys::derive(&MasterSecret::from_bytes([0xC0; 32]));
    let (address, sealed) = guest_keys.seal_chunk(b"the guest's private data").unwrap();

    let service = service(&host);

    assert_eq!(
        service
            .handle_from_test(&Request::StoreChunk {
                owner: guest_keys.user_id(),
                address,
                sealed: sealed.clone(),
            })
            .unwrap(),
        Response::Stored { accepted: true }
    );

    let served = match service
        .handle_from_test(&Request::Chunk {
            owner: guest_keys.user_id(),
            address,
        })
        .unwrap()
    {
        Response::Chunk(Some(bytes)) => bytes,
        other => panic!("the host did not serve back what it stored: {other:?}"),
    };
    assert_eq!(served, sealed);

    // The host's own keys are useless against it.
    assert!(
        UserKeys::derive(&alice())
            .open_chunk(&address, &served)
            .is_err(),
        "the host could read the data it is storing for someone else"
    );
    // The guest's keys work.
    assert_eq!(
        guest_keys.open_chunk(&address, &served).unwrap(),
        b"the guest's private data"
    );
}

#[test]
fn a_storage_challenge_passes_when_held_and_fails_when_not() {
    let host = node(&alice(), 7);
    let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC1; 32]));
    let (address, sealed) = guest.seal_chunk(b"prove you have this").unwrap();

    let service = service(&host);
    service
        .handle_from_test(&Request::StoreChunk {
            owner: guest.user_id(),
            address,
            sealed: sealed.clone(),
        })
        .unwrap();

    let nonce = [0x5A; 32];
    let proof = match service
        .handle_from_test(&Request::Challenge {
            owner: guest.user_id(),
            address,
            nonce,
        })
        .unwrap()
    {
        Response::ChallengeProof(proof) => proof,
        other => panic!("expected a proof, got {other:?}"),
    };

    assert!(
        challenge_holds(&nonce, &sealed, &proof),
        "a host holding the chunk failed its own challenge"
    );

    // A chunk it never had.
    assert!(matches!(
        service
            .handle_from_test(&Request::Challenge {
                owner: guest.user_id(),
                address: ChunkId::from_bytes([0xDD; 32]),
                nonce,
            })
            .unwrap(),
        Response::Refused(_)
    ));
}

#[test]
fn a_host_that_discarded_a_chunk_cannot_fake_the_proof() {
    let host = node(&alice(), 8);
    let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC2; 32]));
    let (address, sealed) = guest.seal_chunk(b"data the host will drop").unwrap();

    let service = service(&host);
    service
        .handle_from_test(&Request::StoreChunk {
            owner: guest.user_id(),
            address,
            sealed: sealed.clone(),
        })
        .unwrap();

    // The host quietly deletes it to save space.
    host.vault.remove_chunk(guest.user_id(), &address).unwrap();

    assert!(
        matches!(
            service
                .handle_from_test(&Request::Challenge {
                    owner: guest.user_id(),
                    address,
                    nonce: [1; 32],
                })
                .unwrap(),
            Response::Refused(_)
        ),
        "a host that discarded a chunk still passed its challenge"
    );
}

#[test]
fn storing_beyond_the_pledge_is_refused() {
    // A node must be able to bound what it takes on, or "pledge 10 GB" is
    // meaningless and the disk fills.
    let host = node(&alice(), 9);
    let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC3; 32]));
    let service = PeerService::new(&host.store, &host.vault, Pledge { bytes: 512 });

    let (first, first_sealed) = guest.seal_chunk(&vec![1u8; 400]).unwrap();
    assert_eq!(
        service
            .handle_from_test(&Request::StoreChunk {
                owner: guest.user_id(),
                address: first,
                sealed: first_sealed,
            })
            .unwrap(),
        Response::Stored { accepted: true }
    );

    let (second, second_sealed) = guest.seal_chunk(&vec![2u8; 400]).unwrap();
    assert!(
        matches!(
            service
                .handle_from_test(&Request::StoreChunk {
                    owner: guest.user_id(),
                    address: second,
                    sealed: second_sealed,
                })
                .unwrap(),
            Response::Refused(_)
        ),
        "the node accepted more than it pledged and will fill its disk"
    );
}

/// THE ATTACK: many connections offer a chunk at the same instant. The
/// listener serves connections concurrently, and "is there room" is a read
/// that happens before "store it". Every connection sees the same free
/// space, every one is accepted, and the host has taken on its pledge many
/// times over -- its owner's disk fills with a stranger's data.
#[test]
fn red_team_concurrent_stores_cannot_take_a_host_past_its_pledge() {
    const CALLERS: usize = 16;
    let host = node(&alice(), 11);
    let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC4; 32]));
    let offers: Vec<_> = (0..CALLERS)
        .map(|n| guest.seal_chunk(&n.to_le_bytes().repeat(50)).unwrap())
        .collect();
    let room = offers[0].1.len() as u64 * 4;
    let service = PeerService::new(&host.store, &host.vault, Pledge::bytes(room));
    let start = std::sync::Barrier::new(CALLERS);
    let owner = guest.user_id();

    std::thread::scope(|scope| {
        for (address, sealed) in &offers {
            let (service, start) = (&service, &start);
            scope.spawn(move || {
                start.wait();
                service
                    .handle_from_test(&Request::StoreChunk {
                        owner,
                        address: *address,
                        sealed: sealed.clone(),
                    })
                    .unwrap();
            });
        }
    });

    let held = host.vault.stats().unwrap().bytes;
    assert!(
        held <= room,
        "{CALLERS} simultaneous offers left the host holding {held} bytes \
         against a pledge of {room}; concurrent connections can fill a \
         disk its owner never offered"
    );
}

#[test]
fn a_node_that_pledged_nothing_still_serves_its_own_data() {
    // Hosting nothing must not break syncing your own devices.
    let node = node(&alice(), 10);
    node.store.write_file("mine.txt", b"my own file").unwrap();
    node.store.flush_segment().unwrap();

    let service = PeerService::new(&node.store, &node.vault, Pledge::NONE).with_owners(&Unscoped);

    match service
        .handle_from_test(&Request::Heads {
            owner: node.store.owner(),
        })
        .unwrap()
    {
        Response::Heads(heads) => assert_eq!(heads.len(), 1),
        other => panic!("expected heads, got {other:?}"),
    }
}

#[test]
fn red_team_pledge_zero_takes_the_accounts_log_from_a_member_only() {
    // A pledge is room for other accounts: a sibling's segments are taken
    // at pledge 0 (#409's iPhone run). The owner an envelope names is the
    // sender's word, so the roster decides who is a sibling: with no claim
    // book to refuse a stranger later, the pledge is all that stops one
    // filling this disk in the account's name.
    let host = node(&alice(), 40);
    let sibling = node(&alice(), 41);
    let stranger = node(&MasterSecret::from_bytes([0xC7; 32]), 42);
    sibling.store.write_file("deleted.txt", b"gone").unwrap();
    let envelope = sibling.store.flush_segment().unwrap().unwrap();
    host.store
        .admit_member(&sibling.store.device_id(), false)
        .unwrap();
    let service = PeerService::new(&host.store, &host.vault, Pledge::NONE);
    let offer = Request::StoreSegment {
        envelope: Box::new(envelope),
    };

    let from_stranger = service.handle(&offer, stranger.store.device_id()).unwrap();
    assert!(
        matches!(from_stranger, Response::Refused(_)),
        "a host at pledge 0 stored its account's segment for a device outside its roster: anybody can fill its disk ({from_stranger:?})"
    );
    let from_sibling = service.handle(&offer, sibling.store.device_id()).unwrap();
    assert!(
        matches!(from_sibling, Response::Stored { accepted: true }),
        "a host at pledge 0 refused its own account's log from a member: a delete made on the phone never reaches it ({from_sibling:?})"
    );
}

#[test]
fn a_forged_segment_is_refused_rather_than_stored() {
    let host = node(&alice(), 11);
    let guest_master = MasterSecret::from_bytes([0xC4; 32]);
    let guest = node(&guest_master, 12);

    guest.store.write_file("theirs.txt", b"content").unwrap();
    let mut envelope = guest.store.flush_segment().unwrap().unwrap();
    envelope.sealed_body[0] ^= 0xFF;

    let response = service(&host)
        .handle_from_test(&Request::StoreSegment {
            envelope: Box::new(envelope),
        })
        .unwrap();

    assert!(
        matches!(response, Response::Refused(_)),
        "a host accepted a segment with a broken signature"
    );
}

#[test]
fn a_segment_already_held_is_answered_not_stored() {
    // `accepted` has to be what happened, not what the call returned. The
    // handler mapped `Ok(_)` to `accepted: true`, so re-offering the tip --
    // which stores nothing, and which every push used to do on every round
    // -- was reported as a segment moved. Downstream, `changed_anything()`
    // was true on every idle round for ever, and the daemon printed a line
    // every five minutes on a fleet where nothing was happening.
    //
    // Found by reading three machines' logs, not by a test. The push side
    // no longer re-offers, so this is the only place that keeps the answer
    // honest for any other caller.
    let node = node(&alice(), 25);
    node.store.write_file("notes.txt", b"content").unwrap();
    let envelope = node
        .store
        .flush_segment()
        .unwrap()
        .expect("a written file produces a segment");

    let service = service(&node);
    let request = Request::StoreSegment {
        envelope: Box::new(envelope),
    };

    match service.handle_from_test(&request).unwrap() {
        Response::Stored { accepted } => assert!(accepted, "the first offer was not stored"),
        other => panic!("expected stored, got {other:?}"),
    }

    match service.handle_from_test(&request).unwrap() {
        Response::Stored { accepted } => assert!(
            !accepted,
            "a segment the vault already held was reported as stored"
        ),
        other => panic!("expected stored, got {other:?}"),
    }
}
