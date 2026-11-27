//! Notices a peer sends about itself: withdrawn records, leaving, draining,
//! drops.

use super::*;

#[test]
fn red_team_a_peer_can_only_withdraw_records_about_itself() {
    // `Dropped` lets a device correct a ledger, which is exactly the shape
    // of request that becomes an attack if the subject is taken from the
    // message. A host that could say "device B no longer holds these" would
    // be able to make an owner believe its data is unreplicated and trigger
    // an endless repair, or -- worse in the other direction -- be used to
    // erase the record of the only holder that still has it.
    //
    // The subject is the connection's proven device and cannot be named in
    // the request at all, so this test is about keeping it that way.
    let node = node(&alice(), 23);
    node.store.write_file("notes.txt", b"content").unwrap();
    let entry = node.store.stat("notes.txt").unwrap().unwrap();
    let chunk = entry.chunks[0];

    let honest = DeviceId::from_bytes([0xA1; 32]);
    let liar = DeviceId::from_bytes([0xB2; 32]);
    node.store.record_holders(&[chunk], &honest).unwrap();
    node.store.record_holders(&[chunk], &liar).unwrap();

    let service = service(&node);
    let response = service
        .handle(
            &Request::Dropped {
                owner: node.store.owner(),
                chunks: vec![chunk],
            },
            liar,
        )
        .unwrap();
    assert!(matches!(response, Response::Stored { accepted: true }));

    let left: Vec<_> = node
        .store
        .remote_holders(&chunk)
        .unwrap()
        .into_iter()
        .map(|holder| holder.device)
        .collect();
    assert_eq!(
        left,
        vec![honest],
        "a peer's notice removed somebody else's record"
    );
}

#[test]
fn red_team_a_leaving_notice_withdraws_only_the_caller() {
    // `Leaving` withdraws every record about a device at once, so it is the
    // most destructive notice a peer can send: pointed at the wrong device
    // it would make an owner forget the only holder that still has its
    // data. It carries no device for that reason, and this is the test
    // that keeps it so. After it, the chunk is under-replicated, which is
    // what makes the next round repair it rather than wait a week.
    let node = node(&alice(), 26);
    node.store.write_file("notes.txt", b"content").unwrap();
    let chunk = node.store.stat("notes.txt").unwrap().unwrap().chunks[0];

    let stays = DeviceId::from_bytes([0xA3; 32]);
    let goes = DeviceId::from_bytes([0xB4; 32]);
    node.store.record_holders(&[chunk], &stays).unwrap();
    node.store.record_holders(&[chunk], &goes).unwrap();

    let response = service(&node).handle(&Request::Leaving, goes).unwrap();
    assert!(matches!(response, Response::Stored { accepted: true }));

    let left: Vec<_> = node
        .store
        .remote_holders(&chunk)
        .unwrap()
        .into_iter()
        .map(|holder| holder.device)
        .collect();
    assert_eq!(left, vec![stays], "leaving withdrew the wrong device");

    let at_risk = node
        .store
        .under_replicated(itsanas_store::REPLICATION_TARGET, itsanas_store::now_unix())
        .unwrap();
    assert!(
        at_risk.iter().any(|risk| risk.chunk == chunk),
        "a departed holder is still counted, so nothing repairs"
    );
}

#[test]
fn red_team_a_draining_notice_uncounts_only_the_caller() {
    // `Draining` makes an owner stop counting a holder. Pointed at somebody
    // else it would make the owner ignore -- and replace -- a machine that
    // is staying. It carries no device; this keeps it so, and checks the
    // answer counts what rests on the caller alone.
    let node = node(&alice(), 27);
    node.store.write_file("notes.txt", b"content").unwrap();
    let chunk = node.store.stat("notes.txt").unwrap().unwrap().chunks[0];

    let stays = DeviceId::from_bytes([0xA5; 32]);
    let goes = DeviceId::from_bytes([0xB6; 32]);
    node.store.record_holders(&[chunk], &goes).unwrap();

    let response = service(&node)
        .handle(&Request::Draining { seconds: 600 }, goes)
        .unwrap();
    assert_eq!(
        response,
        Response::Exposed { chunks: 1 },
        "the only holder drained and was told nothing rests on it"
    );
    let now = itsanas_store::now_unix();
    assert!(node.store.is_draining(&goes, now).unwrap());
    assert!(
        !node.store.is_draining(&stays, now).unwrap(),
        "a notice uncounted a device other than its sender"
    );

    node.store.record_holders(&[chunk], &stays).unwrap();
    let response = service(&node)
        .handle(&Request::Draining { seconds: 600 }, goes)
        .unwrap();
    assert_eq!(
        response,
        Response::Exposed { chunks: 0 },
        "a second holder appeared and the drain was still told to wait"
    );
}

#[test]
fn a_drop_notice_for_another_account_is_refused() {
    // A node hosting somebody else's sealed data keeps no ledger about it:
    // the owner does. Accepting the notice silently would look like the
    // record had been withdrawn somewhere, which is the kind of quiet
    // success that hides a hole for months.
    let node = node(&alice(), 24);
    let response = service(&node)
        .handle(
            &Request::Dropped {
                owner: itsanas_crypto::UserId::from_bytes([0xCC; 32]),
                chunks: vec![ChunkId::from_bytes([1; 32])],
            },
            DeviceId::from_bytes([2; 32]),
        )
        .unwrap();

    assert!(matches!(response, Response::Refused(_)));
}
