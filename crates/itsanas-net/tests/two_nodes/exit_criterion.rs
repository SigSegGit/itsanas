//! The exit criterion: a file crosses a real socket, whole, once.

use super::*;

#[test]
fn two_nodes_sync_a_file_over_a_real_socket() {
    let laptop = node(&alice(), 1);
    let pi = node(&alice(), 2);

    laptop
        .store
        .write_file("reports/q3.txt", b"written on the laptop")
        .unwrap();
    laptop.store.flush_segment().unwrap();

    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &pi.device, pi.store.owner(), None).expect("connect");

        assert_eq!(
            client.peer_device(),
            laptop.store.device_id(),
            "the peer identified itself as the wrong device"
        );

        let report = session::round(&pi.store, &pi.vault, &mut client).expect("sync round");
        assert!(
            report.pull.adopted > 0,
            "nothing was adopted from the peer: {report:?}"
        );
    });

    assert_eq!(
        pi.store.read_file("reports/q3.txt").unwrap().unwrap(),
        b"written on the laptop",
        "the file did not cross the wire intact"
    );
}

/// A member in China syncing with their own machine in France: 250 ms there
/// and back, 64 KiB/s, and the daemon's own budget for one peer in a
/// round (`PEER_SESSION_BUDGET`). A file of several chunks must come across
/// whole within it; a budget or a timeout tighter than such a path needs
/// would leave that member's files on the far side for ever, round after
/// round.
#[test]
fn red_team_a_file_crosses_a_slow_throttled_path_within_the_round_budget() {
    use itsanas_testkit::path::{HostilePath, Rules};
    let france = node(&alice(), 5);
    let china = node(&alice(), 6);
    let payload = itsanas_testkit::filler("across-the-border", 256 * 1024);
    france.store.write_file("photos.bin", &payload).unwrap();
    france.store.flush_segment().unwrap();
    assert!(
        france
            .store
            .stat("photos.bin")
            .unwrap()
            .unwrap()
            .chunks
            .len()
            > 1,
        "one chunk tests no fetch loop"
    );

    with_server(&france, Pledge::gigabytes(1), |address| {
        let path = HostilePath::start(
            address,
            Rules {
                delay: std::time::Duration::from_millis(125),
                rate: Some(64 * 1024),
                ..Rules::default()
            },
        );
        let mut client = PeerClient::connect_within(
            path.address(),
            &china.device,
            china.store.owner(),
            None,
            Some(itsanas_net::PEER_SESSION_BUDGET),
        )
        .expect("the handshake did not survive a slow, throttled path");
        session::round(&china.store, &china.vault, &mut client)
            .expect("a round across the path failed");
    });

    assert_eq!(
        china.store.read_file("photos.bin").unwrap().unwrap(),
        payload,
        "a file did not come across a slow path whole"
    );
}

#[test]
fn a_larger_file_survives_the_wire_byte_for_byte() {
    // Several chunks, so the fetch loop and the reassembly both get exercised.
    let laptop = node(&alice(), 3);
    let pi = node(&alice(), 4);

    let payload = itsanas_testkit::filler("over-the-wire", 3 * 1024 * 1024);
    laptop.store.write_file("big.bin", &payload).unwrap();
    laptop.store.flush_segment().unwrap();

    let entry = laptop.store.stat("big.bin").unwrap().unwrap();
    assert!(entry.chunks.len() > 10, "the test payload was too small");

    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();
        session::round(&pi.store, &pi.vault, &mut client).unwrap();
    });

    assert_eq!(
        pi.store.read_file("big.bin").unwrap().unwrap(),
        payload,
        "a multi-chunk file came back different"
    );
}

#[test]
fn syncing_twice_transfers_nothing_the_second_time() {
    // Without the have/missing exchange this re-uploads everything on every
    // round, which at real sizes saturates the link forever.
    let laptop = node(&alice(), 5);
    let pi = node(&alice(), 6);

    laptop
        .store
        .write_file("stable.txt", b"unchanging content")
        .unwrap();
    laptop.store.flush_segment().unwrap();

    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();

        let first = session::round(&pi.store, &pi.vault, &mut client).unwrap();
        assert!(first.changed_anything());

        let second = session::round(&pi.store, &pi.vault, &mut client).unwrap();
        assert_eq!(
            second.pull.adopted, 0,
            "the second round re-adopted work it already had"
        );
        assert_eq!(
            second.push.chunks_offered, 0,
            "the second round re-offered chunks the peer already holds"
        );
        assert!(
            !second.changed_anything(),
            "a quiet round reported progress: {second:?}"
        );
    });
}

#[test]
fn a_node_that_only_ever_accepts_connections_still_learns_what_was_pushed_to_it() {
    // A real bug, found by running two daemons where only one had the other
    // configured as a peer. A push lands in the receiving node's *vault*, and
    // only `pull` applies segments to the store — so the node that never dialled
    // anybody held its own data and never looked at it.
    //
    // Not a corner case: a device behind NAT can push and cannot be dialled, so
    // for its peers this is the only way its work ever arrives.
    let listener = node(&alice(), 24);
    let dialler = node(&alice(), 25);

    dialler
        .store
        .write_file("pushed.txt", b"sent by the device that dialled")
        .unwrap();
    dialler.store.flush_segment().unwrap();

    with_server(&listener, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &dialler.device, dialler.store.owner(), None).unwrap();
        session::push(&dialler.store, &mut client).expect("push");
    });

    // The listener never pulled, and never will.
    assert_eq!(
        listener.store.read_file("pushed.txt").unwrap(),
        None,
        "this test is meaningless if a push alone already reached the store"
    );

    let report = session::drain_vault(&listener.store, &listener.vault).unwrap();

    assert!(report.adopted > 0, "nothing was drained: {report:?}");
    assert_eq!(
        listener.store.read_file("pushed.txt").unwrap().unwrap(),
        b"sent by the device that dialled",
        "a node that only accepts connections never learned what it was holding"
    );

    // And draining again is a no-op, or a daemon would churn forever.
    let second = session::drain_vault(&listener.store, &listener.vault).unwrap();
    assert!(
        !second.changed_anything(),
        "draining twice changed state: {second:?}"
    );
}
