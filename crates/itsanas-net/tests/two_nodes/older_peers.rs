//! Peers on an older protocol: never asked what they cannot answer, and a
//! machine of the oldest release in the window still syncs.

use super::*;

/// A `Relay` that counts how often it was asked.
struct CountingRelay(std::sync::atomic::AtomicUsize);

impl itsanas_net::Relay for CountingRelay {
    fn presences_for(&self, _caller: itsanas_crypto::DeviceId) -> Option<Vec<Vec<u8>>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Some(Vec::new())
    }
}

/// A machine not yet upgraded to protocol 6 does not know `Presences`; asking
/// it anyway costs a refusal per round, and an older peer that misparses the
/// unknown request drops the connection the round still needed. The client
/// must not ask. The control half proves the counter counts: a v6 service is
/// asked exactly once.
#[test]
fn red_team_a_peer_speaking_protocol_5_is_never_asked_for_presences() {
    use itsanas_net::protocol::PROTOCOL_WITH_PRESENCES;
    let host = node(&alice(), 75);
    let me = node(&alice(), 76);

    for (version, expected) in [
        (PROTOCOL_WITH_PRESENCES, 1),
        (PROTOCOL_WITH_PRESENCES - 1, 0),
    ] {
        let relay = CountingRelay(std::sync::atomic::AtomicUsize::new(0));
        let server = PeerServer::bind("127.0.0.1:0").expect("bind loopback");
        let address = server.local_addr().expect("local address");
        let shutdown = AtomicBool::new(false);
        let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
            .with_owners(&itsanas_net::Unscoped)
            .with_relay(&relay)
            .speaking_at_most(version);

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = server.serve_until(&service, &host.device, &shutdown);
            });
            let _stop = StopOnDrop(&shutdown, address);

            let mut client =
                PeerClient::connect(address, &me.device, me.store.owner(), None).expect("connect");
            let answer = client.presences().expect("presences");
            assert_eq!(
                relay.0.load(Ordering::SeqCst),
                expected,
                "a peer speaking protocol {version} was asked for presences {} time(s)",
                relay.0.load(Ordering::SeqCst)
            );
            if expected == 0 {
                assert_eq!(answer, None, "an old peer's silence must read as `None`");
            }
        });
    }
}

/// Application mode is what makes two machines each other's privileged hosts
/// (HANDOVER §8 00): a peer that says it is on is dialled first and every
/// minute. So the answer must follow the host's own switch, both ways, and a
/// peer from before protocol 13 must not be asked at all -- it would close the
/// connection on a verb it cannot decode, and the round would lose the peer.
#[test]
fn red_team_a_peer_says_its_application_mode_as_it_is_and_an_older_one_is_never_asked() {
    use itsanas_net::protocol::PROTOCOL_WITH_APP_MODE;
    let host = node(&alice(), 77);
    let me = node(&alice(), 78);

    for (version, on, expected) in [
        (PROTOCOL_WITH_APP_MODE, true, Some(true)),
        (PROTOCOL_WITH_APP_MODE, false, Some(false)),
        // On, but too old to say so: read as not, and never asked.
        (PROTOCOL_WITH_APP_MODE - 1, true, None),
    ] {
        let switch = AtomicBool::new(on);
        let server = PeerServer::bind("127.0.0.1:0").expect("bind loopback");
        let address = server.local_addr().expect("local address");
        let shutdown = AtomicBool::new(false);
        let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
            .with_owners(&itsanas_net::Unscoped)
            .in_app_mode(&switch)
            .speaking_at_most(version);

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = server.serve_until(&service, &host.device, &shutdown);
            });
            let _stop = StopOnDrop(&shutdown, address);

            let mut client =
                PeerClient::connect(address, &me.device, me.store.owner(), None).expect("connect");
            assert_eq!(
                client.app_mode().expect("asked"),
                expected,
                "a host at protocol {version} with application mode {on} was read wrong"
            );
            // The connection is still good for the round after the question.
            assert!(
                client.presences().is_ok(),
                "asking for application mode cost the connection the round needed"
            );
        });
    }
}

/// A machine of the oldest release the window keeps open (`MIN_PROTOCOL_VERSION`)
/// still syncs with this one, both ways, and has a host keep its data: a member
/// who opens the app once a month is not cut off by a release (HANDOVER §8 18).
/// Sabotage: raise `MIN_PROTOCOL_VERSION` to `PROTOCOL_VERSION`.
#[test]
fn red_team_a_machine_of_the_oldest_release_in_the_window_still_syncs() {
    // The oldest version kept, written out: closing the window on a release
    // still in use is a decision (§8 18), made by changing this line too.
    const OLDEST_KEPT: u16 = 4;
    let master = alice();
    let laptop = node(&master, 1);
    let old = node(&master, 2);
    let host = node(&MasterSecret::from_bytes([0xC6; 32]), 57);
    laptop
        .store
        .write_file("new.txt", b"from today")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    old.store
        .write_file("old.txt", b"from last month")
        .expect("write");
    old.store.flush_segment().expect("flush");

    let dial_old = |address| {
        let client = PeerClient::connect_offering(
            address,
            &old.device,
            old.store.owner(),
            None,
            OLDEST_KEPT,
        )
        .expect("a machine of the oldest release in the window was refused");
        assert_eq!(client.spoken(), OLDEST_KEPT);
        client
    };
    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client = dial_old(address);
        session::pull_scoped(
            &old.store,
            &old.vault,
            &mut client,
            session::Scope::Everything,
        )
        .expect("the old machine pulls");
        session::push(&old.store, &mut client).expect("the old machine pushes");
    });
    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client = dial_old(address);
        session::push(&old.store, &mut client).expect("a host keeps the old machine's data");
    });
    pull_all(&laptop, &old);
    assert_eq!(
        old.store.read_file("new.txt").expect("read"),
        Some(b"from today".to_vec()),
        "the old machine did not get today's file"
    );
    assert_eq!(
        laptop.store.read_file("old.txt").expect("read"),
        Some(b"from last month".to_vec()),
        "today's machine did not get the old machine's file"
    );
}
