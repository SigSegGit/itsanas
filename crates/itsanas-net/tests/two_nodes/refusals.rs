//! What a peer cannot do: forge, trickle, stay silent, or ask for what is
//! not there, and hold the caller past its budget.

use super::*;

#[test]
fn a_storage_challenge_works_over_the_wire() {
    let host = node(&MasterSecret::from_bytes([0xB2; 32]), 12);
    let owner = node(&alice(), 13);

    owner
        .store
        .write_file("audited.txt", b"prove you are holding this")
        .unwrap();
    owner.store.flush_segment().unwrap();
    let entry = owner.store.stat("audited.txt").unwrap().unwrap();
    let address = entry.chunks[0];

    // The owner can re-derive the sealed bytes without keeping a copy, because
    // chunk sealing is deterministic. That is what makes remote audit possible.
    let expected = owner.store.blobs().get(&address).unwrap().unwrap();

    with_server(&host, Pledge::gigabytes(1), |server_address| {
        let mut client =
            PeerClient::connect(server_address, &owner.device, owner.store.owner(), None).unwrap();

        // Before storing anything, the host cannot pass.
        assert!(
            !client
                .challenge(owner.store.owner(), address, [1; 32], &expected)
                .unwrap(),
            "a host passed a challenge for a chunk it has never seen"
        );

        session::push(&owner.store, &mut client).unwrap();

        assert!(
            client
                .challenge(owner.store.owner(), address, [1; 32], &expected)
                .unwrap(),
            "a host holding the chunk failed its challenge"
        );
        // A different nonce must also work, and must be a different proof.
        assert!(
            client
                .challenge(owner.store.owner(), address, [2; 32], &expected)
                .unwrap()
        );
    });
}

#[test]
fn a_peer_cannot_push_a_forged_segment_into_a_host() {
    let host = node(&MasterSecret::from_bytes([0xB3; 32]), 14);
    let owner = node(&alice(), 15);

    owner.store.write_file("real.txt", b"genuine").unwrap();
    let mut envelope = owner.store.flush_segment().unwrap().unwrap();
    envelope.sealed_body[0] ^= 0xFF;

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).unwrap();

        assert!(
            !matches!(client.store_segment(&envelope).unwrap(), Offer::Taken),
            "a host accepted a segment whose signature does not verify"
        );
    });

    assert_eq!(host.vault.stats().unwrap().segments, 0);
}

/// Connect and feed the handshake a byte every 100 ms for four seconds.
///
/// Returns how long the server let that go on: the moment a write failed, or
/// the whole four seconds if none did. A per-read timeout never ends this,
/// because no single read waits more than a tenth of a second.
fn trickle_until_cut_off(address: std::net::SocketAddr) -> std::time::Duration {
    use std::io::Write;
    let started = std::time::Instant::now();
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    // A TLS record header promising a full-sized record, so the server keeps
    // waiting for the rest instead of rejecting it on sight.
    if stream.write_all(&[0x16, 0x03, 0x01, 0x40, 0x00]).is_err() {
        return started.elapsed();
    }
    for _ in 0..40 {
        if stream.write_all(&[0]).is_err() {
            return started.elapsed();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    started.elapsed()
}

/// THE ATTACK: hold a slot on a node's listener by trickling the handshake.
/// The deadline lives in `itsanas-tls` and is unit-tested there; this proves
/// the node's listener actually applies it, which nothing else checks -- going
/// back to plain `accept` would leave every other test green.
#[test]
fn red_team_a_trickled_handshake_is_cut_off_by_the_node_listener() {
    let host = node(&alice(), 74);
    let server = PeerServer::bind("127.0.0.1:0")
        .expect("bind loopback")
        .with_handshake_deadline(std::time::Duration::from_millis(300));
    let address = server.local_addr().expect("local address");
    let shutdown = AtomicBool::new(false);
    let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
        .with_owners(&itsanas_net::Unscoped);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &host.device, &shutdown);
        });
        let _stop = StopOnDrop(&shutdown, address);

        let held = trickle_until_cut_off(address);
        assert!(
            held < std::time::Duration::from_secs(2),
            "a caller trickling its handshake held a node's connection for \
             {held:?} against a 300 ms deadline; enough of them fill the listener"
        );
    });
}

/// THE ATTACK: connect to a node and say nothing. The listener used to serve
/// one connection at a time, so a silent connection held it for the whole read
/// timeout; opening one every thirty seconds made the node undialable to every
/// honest peer, and cost nothing. On a home network nobody does that. On a
/// forwarded port somebody will.
#[test]
fn red_team_connections_that_say_nothing_do_not_stop_a_node_serving_others() {
    let host = node(&alice(), 72);
    let owner = node(&alice(), 73);

    with_server(&host, Pledge::gigabytes(1), |address| {
        let silent: Vec<std::net::TcpStream> = (0..3)
            .map(|_| std::net::TcpStream::connect(address).unwrap())
            .collect();
        // Long enough for the accept loop to have taken all three.
        std::thread::sleep(std::time::Duration::from_millis(200));

        let started = std::time::Instant::now();
        let answered = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
            .and_then(|mut client| client.heads(owner.store.owner()));
        let took = started.elapsed();

        assert!(
            answered.is_ok() && took < std::time::Duration::from_secs(5),
            "with three silent connections open, an honest peer got {answered:?} \
             after {took:?}; anybody who can reach the port can take this node \
             off the network"
        );
        drop(silent);
    });
}

#[test]
fn a_host_that_has_pledged_nothing_refuses_to_store_but_still_answers() {
    let host = node(&MasterSecret::from_bytes([0xB4; 32]), 16);
    let owner = node(&alice(), 17);

    owner.store.write_file("unwanted.txt", b"content").unwrap();
    owner.store.flush_segment().unwrap();

    with_server(&host, Pledge::NONE, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).unwrap();

        let report = session::push(&owner.store, &mut client).unwrap();
        assert_eq!(
            report.chunks_accepted, 0,
            "a node that pledged nothing accepted data anyway"
        );
        assert_eq!(report.segments_accepted, 0);

        // But it is still a working peer.
        assert!(
            client.heads(owner.store.owner()).is_ok(),
            "refusing to store made the node stop answering entirely"
        );
    });

    assert_eq!(host.vault.stats().unwrap().bytes, 0);
}

#[test]
fn red_team_a_host_that_refuses_everything_is_not_reported_as_nothing_to_send() {
    // THE ATTACK, or the misconfiguration that looks like one: a host takes the
    // connection, answers every question, and refuses every byte -- pledge 0,
    // a full disk, or a node that has decided to leech. The owner's round then
    // printed `sent 0 B, 0 segments`, which is exactly what an idle round
    // prints, so the owner believed its data was replicated and nothing was
    // pending. The acceptance bench lost an afternoon to it on E, F and G.
    let host = node(&MasterSecret::from_bytes([0xB6; 32]), 30);
    let owner = node(&alice(), 31);

    owner
        .store
        .write_file("pending.txt", b"data that has nowhere to go yet")
        .unwrap();
    owner.store.flush_segment().unwrap();

    with_server(&host, Pledge::NONE, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).unwrap();
        let report = session::push(&owner.store, &mut client).unwrap();

        assert_eq!(report.chunks_accepted + report.segments_accepted, 0);
        assert!(
            report.refused > 0,
            "every offer was refused and the report counts no refusal, so the round \
             reads as a round with nothing to send"
        );
        assert_eq!(
            report.refusal,
            Some(Refusal::PledgeFull),
            "the reason is lost, so the owner cannot tell a full host from a hostile one"
        );
    });
}

#[test]
fn a_malformed_request_gets_a_refusal_rather_than_a_dropped_connection() {
    // A peer must not be able to kill a connection, and thereby a sync round,
    // by sending something silly.
    let host = node(&alice(), 18);
    let peer = node(&alice(), 19);

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &peer.device, peer.store.owner(), None).unwrap();

        let response = client
            .request(&Request::Segments {
                owner: peer.store.owner(),
                device: host.store.device_id(),
                after: None,
                limit: 0,
            })
            .expect("the connection survived a malformed request");
        assert!(matches!(response, Response::Refused(_)));

        // The connection still works afterwards.
        assert!(client.heads(peer.store.owner()).is_ok());
    });
}

#[test]
fn a_peer_asking_about_an_unknown_user_gets_an_empty_answer() {
    let host = node(&alice(), 20);
    let stranger = node(&MasterSecret::from_bytes([0xB5; 32]), 21);

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &stranger.device, stranger.store.owner(), None).unwrap();

        assert_eq!(
            client.heads(stranger.store.owner()).unwrap(),
            Vec::new(),
            "a host invented chains for a user it has never heard of"
        );
    });
}

/// A node listening on every interface answers a caller over IPv6 and one
/// over IPv4 on the same port. On Windows those are two sockets, and a
/// server that watched only the first left every IPv6 caller -- the one
/// route between two houses that needs no forward -- waiting on a port
/// that never answered.
#[test]
fn a_node_on_every_interface_serves_an_ipv6_caller_and_an_ipv4_one() {
    // A wildcard bind on a developer's Windows asks the firewall at every new
    // test binary (Nicolas, 2026-11-18: a window per build); CI runs it.
    if cfg!(windows) && std::env::var_os("CI").is_none() {
        return;
    }
    if std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 0)).is_err() {
        // No IPv6 on this runner: nothing to dial it with.
        return;
    }
    let host = node(&alice(), 92);
    let caller = node(&alice(), 93);
    let server = PeerServer::bind("0.0.0.0:0").expect("bind every interface");
    let port = server.local_addr().expect("local address").port();
    let v4: std::net::SocketAddr = (std::net::Ipv4Addr::LOCALHOST, port).into();
    let v6: std::net::SocketAddr = (std::net::Ipv6Addr::LOCALHOST, port).into();
    let shutdown = AtomicBool::new(false);
    let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
        .with_owners(&itsanas_net::Unscoped);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &host.device, &shutdown);
        });
        let _stop = StopOnDrop(&shutdown, v4);
        for address in [v6, v4] {
            let mut client =
                PeerClient::connect(address, &caller.device, caller.store.owner(), None)
                    .unwrap_or_else(|error| {
                        panic!("a caller dialling {address} was not served: {error}")
                    });
            assert!(
                client.heads(caller.store.owner()).is_ok(),
                "a caller over {address} was let in and then not answered"
            );
        }
    });
}

/// A peer that opens a TLS record and then sends one byte a second never trips
/// the thirty-second read timeout, because no single read waits that long. Until
/// `PEER_SESSION_BUDGET` the daemon stayed with such a peer for as long as it
/// kept going -- 45 minutes on a laptop on 2026-10-06, during which none of its
/// files left and none of its peers' files were written. With a budget, the
/// connection is cut when the budget runs out, and the round goes on.
#[test]
fn red_team_a_peer_that_trickles_cannot_hold_the_caller_past_its_budget() {
    use std::io::Write as _;
    use std::time::{Duration, Instant};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let address = listener.local_addr().expect("local address");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // A handshake record announcing 16 KiB, then the bytes one at a time.
            let _ = stream.write_all(&[0x16, 0x03, 0x03, 0x40, 0x00]);
            for _ in 0..90 {
                std::thread::sleep(Duration::from_secs(1));
                if stream.write_all(&[0]).is_err() {
                    break;
                }
            }
        }
    });

    let caller = node(&alice(), 1);
    let (done, outcome) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let started = Instant::now();
        let result = PeerClient::connect_within(
            address,
            &caller.device,
            caller.store.owner(),
            None,
            Some(Duration::from_secs(2)),
        );
        let _ = done.send((result.is_err(), started.elapsed()));
    });

    let (failed, took) = outcome.recv_timeout(Duration::from_secs(20)).expect(
        "still talking to a peer that trickles one byte a second, 20 s into a 2 s \
         budget: one such peer holds a daemon's whole round, and every file in it, \
         for as long as it likes",
    );
    assert!(
        failed,
        "a peer that never finished its handshake was accepted"
    );
    assert!(
        took < Duration::from_secs(10),
        "the budget was 2 s and the caller was held {took:?}"
    );
}
