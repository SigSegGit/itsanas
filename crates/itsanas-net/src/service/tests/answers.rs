//! What a peer is answered: hello and versions, reads, limits, presences.

use super::*;

#[test]
fn hello_reports_this_nodes_device_and_agrees_on_a_version() {
    let node = node(&alice(), 1);
    let service = service(&node);

    let response = service
        .handle_from_test(&Request::Hello {
            protocol: PROTOCOL_VERSION,
            device: DeviceId::from_bytes([9; 32]),
            owner: node.store.owner(),
        })
        .unwrap();

    match response {
        Response::Hello { protocol, device } => {
            assert_eq!(protocol, PROTOCOL_VERSION);
            assert_eq!(device, node.store.device_id());
        }
        other => panic!("expected a hello, got {other:?}"),
    }
}

#[test]
fn a_hello_from_a_newer_peer_is_answered_with_the_version_both_sides_know() {
    // The property the version window rests on. Refusing anything but an
    // exact match -- which is what this did -- meant no node could speak to
    // a node one commit ahead of it, so every addition to the protocol
    // partitioned the network until every machine was upgraded at the same
    // instant. Fine for three machines in one house; impossible for people
    // who join and leave.
    let node = node(&alice(), 2);
    let response = service(&node)
        .handle_from_test(&Request::Hello {
            protocol: PROTOCOL_VERSION + 5,
            device: DeviceId::from_bytes([9; 32]),
            owner: node.store.owner(),
        })
        .unwrap();

    match response {
        Response::Hello { protocol, .. } => assert_eq!(
            protocol, PROTOCOL_VERSION,
            "this node claimed to speak a version it has never heard of"
        ),
        other => panic!("a newer peer was turned away: {other:?}"),
    }
}

#[test]
fn a_hello_from_below_the_floor_is_refused_rather_than_guessed_at() {
    // The other half: a window has a bottom. Below it there is no shared
    // vocabulary to fall back to, and pretending otherwise would fail on
    // some later message instead of this one.
    let node = node(&alice(), 22);
    let response = service(&node)
        .handle_from_test(&Request::Hello {
            protocol: crate::protocol::MIN_PROTOCOL_VERSION - 1,
            device: DeviceId::from_bytes([9; 32]),
            owner: node.store.owner(),
        })
        .unwrap();

    assert!(matches!(response, Response::Refused(_)));
}

#[test]
fn a_peer_can_fetch_this_nodes_own_segments_and_chunks() {
    let node = node(&alice(), 3);
    node.store.write_file("notes.txt", b"content").unwrap();
    let entry = node.store.stat("notes.txt").unwrap().unwrap();
    node.store.flush_segment().unwrap();

    let service = service(&node);

    let heads = match service
        .handle_from_test(&Request::Heads {
            owner: node.store.owner(),
        })
        .unwrap()
    {
        Response::Heads(heads) => heads,
        other => panic!("expected heads, got {other:?}"),
    };
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].device, node.store.device_id());
    assert_eq!(heads[0].length, 1);

    let segments = match service
        .handle_from_test(&Request::Segments {
            owner: node.store.owner(),
            device: node.store.device_id(),
            after: None,
            limit: 10,
        })
        .unwrap()
    {
        Response::Segments(segments) => segments,
        other => panic!("expected segments, got {other:?}"),
    };
    assert_eq!(segments.len(), 1);
    segments[0].verify_signature().unwrap();

    let chunk = match service
        .handle_from_test(&Request::Chunk {
            owner: node.store.owner(),
            address: entry.chunks[0],
        })
        .unwrap()
    {
        Response::Chunk(chunk) => chunk,
        other => panic!("expected a chunk, got {other:?}"),
    };
    assert!(chunk.is_some());
}

#[test]
fn what_a_peer_fetches_is_useless_without_the_key() {
    // The reason there is no access-control list: serving this to the wrong
    // person reveals nothing.
    let node = node(&alice(), 4);
    node.store
        .write_file("secret.txt", b"ITSANAS-SERVICE-CANARY-8f2a")
        .unwrap();
    let entry = node.store.stat("secret.txt").unwrap().unwrap();

    let chunk = match service(&node)
        .handle_from_test(&Request::Chunk {
            owner: node.store.owner(),
            address: entry.chunks[0],
        })
        .unwrap()
    {
        Response::Chunk(Some(bytes)) => bytes,
        other => panic!("expected a chunk, got {other:?}"),
    };

    let needle = b"ITSANAS-SERVICE-CANARY-8f2a";
    assert!(
        !chunk.windows(needle.len()).any(|w| w == needle),
        "the bytes served to a peer contain the plaintext"
    );

    // And a different user genuinely cannot open them.
    let stranger = UserKeys::derive(&MasterSecret::from_bytes([0xBB; 32]));
    assert!(
        stranger.open_chunk(&entry.chunks[0], &chunk).is_err(),
        "a stranger opened a chunk served to them"
    );
}

#[test]
fn an_unknown_chunk_is_none_rather_than_an_error() {
    // "I do not have it" is ordinary. Turning it into an error would let a
    // peer's request decide when this node reports a fault.
    let node = node(&alice(), 5);
    let response = service(&node)
        .handle_from_test(&Request::Chunk {
            owner: node.store.owner(),
            address: ChunkId::from_bytes([0xEE; 32]),
        })
        .unwrap();

    assert_eq!(response, Response::Chunk(None));
}

#[test]
fn a_bad_request_never_becomes_a_local_error() {
    // A peer must not be able to decide when this node reports a fault.
    let node = node(&alice(), 13);
    let service = service(&node);

    for request in [
        Request::Segments {
            owner: node.store.owner(),
            device: node.store.device_id(),
            after: None,
            limit: 0,
        },
        Request::Hello {
            protocol: 0,
            device: DeviceId::from_bytes([1; 32]),
            owner: node.store.owner(),
        },
    ] {
        let response = service
            .handle_from_test(&request)
            .expect("a peer's bad request must not be a local error");
        assert!(matches!(response, Response::Refused(_)));
    }
}

#[test]
fn heads_for_an_unknown_owner_are_empty_rather_than_an_error() {
    let node = node(&alice(), 14);
    let response = service(&node)
        .handle_from_test(&Request::Heads {
            owner: UserKeys::derive(&MasterSecret::from_bytes([0xEE; 32])).user_id(),
        })
        .unwrap();

    assert_eq!(response, Response::Heads(Vec::new()));
}

#[test]
fn the_segment_limit_is_clamped_to_the_protocol_maximum() {
    let node = node(&alice(), 15);
    for round in 0..5 {
        node.store
            .write_file(&format!("f{round}.txt"), b"x")
            .unwrap();
        node.store.flush_segment().unwrap();
    }

    match service(&node)
        .handle_from_test(&Request::Segments {
            owner: node.store.owner(),
            device: node.store.device_id(),
            after: None,
            limit: 2,
        })
        .unwrap()
    {
        Response::Segments(segments) => assert_eq!(segments.len(), 2),
        other => panic!("expected segments, got {other:?}"),
    }
}

/// A book that knows one member and hands it the rows it was given.
struct Book {
    member: DeviceId,
    rows: Vec<Vec<u8>>,
}

impl Relay for Book {
    fn presences_for(&self, caller: DeviceId) -> Option<Vec<Vec<u8>>> {
        (caller == self.member).then(|| self.rows.clone())
    }
}

#[test]
fn a_node_that_keeps_no_book_says_so() {
    let node = node(&alice(), 1);
    let response = service(&node)
        .handle_from_test(&Request::Presences)
        .unwrap();
    assert_eq!(response, Response::Refused(NO_BOOK.to_owned()));
}

#[test]
fn red_team_presences_are_answered_to_the_device_tls_proved_and_no_other() {
    // The request names nobody on purpose: who may ask is the device the
    // connection proved. Sabotage: pass anything but `caller` to the book,
    // and a stranger reads the account's addresses.
    let node = node(&alice(), 1);
    let member = DeviceId::from_bytes([0x11; 32]);
    let book = Book {
        member,
        rows: vec![vec![1, 2, 3]],
    };
    let service = service(&node).with_relay(&book);

    assert_eq!(
        service.handle(&Request::Presences, member).unwrap(),
        Response::Presences(vec![vec![1, 2, 3]])
    );
    assert_eq!(
        service
            .handle(&Request::Presences, DeviceId::from_bytes([0x22; 32]))
            .unwrap(),
        Response::Refused(NOT_YOURS.to_owned()),
        "a stranger was handed the account's machines"
    );
}

#[test]
fn an_answer_never_exceeds_what_the_receiver_accepts() {
    // The receiver refuses a padded answer whole (`PeerClient::presences`),
    // so a book past the bounds must cost rows here, not the answer.
    let node = node(&alice(), 1);
    let member = DeviceId::from_bytes([0x11; 32]);
    let mut rows = vec![vec![0; crate::protocol::MAX_RELAYED_ROW_BYTES + 1]];
    rows.extend(std::iter::repeat_n(
        vec![7],
        crate::protocol::MAX_RELAYED_ROWS + 5,
    ));
    let book = Book { member, rows };
    match service(&node)
        .with_relay(&book)
        .handle(&Request::Presences, member)
        .unwrap()
    {
        Response::Presences(rows) => {
            assert_eq!(rows.len(), crate::protocol::MAX_RELAYED_ROWS);
            assert!(rows.iter().all(|row| row == &vec![7]));
        }
        other => panic!("expected presences, got {other:?}"),
    }
}
