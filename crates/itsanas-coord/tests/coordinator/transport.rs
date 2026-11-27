//! The connection itself: a trickled handshake, versions, slow and cut paths,
//! and the cap on requests.

use super::*;

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

/// THE ATTACK: a coordinator is the one address every member dials, so a slot
/// held by a caller trickling its handshake is a slot no member gets. The
/// deadline is unit-tested in `itsanas-tls`; this proves the coordinator
/// applies it -- going back to plain `accept` leaves every other test green.
#[test]
fn red_team_a_trickled_handshake_is_cut_off_by_the_coordinator() {
    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
    let server = CoordServer::bind("127.0.0.1:0")
        .expect("bind")
        .with_handshake_deadline(std::time::Duration::from_millis(300));
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let coordinator = device(0xC1);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&directory, &coordinator, &shutdown, |_| {});
        });
        let _stop = StopOnDrop(&shutdown, address);

        let held = trickle_until_cut_off(address);
        assert!(
            held < std::time::Duration::from_secs(2),
            "a caller trickling its handshake held a coordinator connection \
             for {held:?} against a 300 ms deadline; sixty-four of them lock \
             every member out"
        );
    });
}

#[test]
fn a_version_below_the_window_is_refused_rather_than_guessed_at() {
    with_coordinator(|address, _| {
        let mut client = dial(address, &device(2), &user(2));
        assert!(matches!(
            client
                .ask(&Request::Hello {
                    version: itsanas_coord::protocol::MIN_COORD_VERSION - 1
                })
                .unwrap(),
            Response::Refused(_)
        ));
    });
}

/// A client newer than the coordinator is answered at the coordinator's
/// version, not refused: until 2026-11-23 any difference cut a member off
/// until both sides updated (§8 18). Sabotage: welcome only an equal version.
#[test]
fn red_team_a_newer_client_is_answered_at_the_coordinators_version() {
    with_coordinator(|address, _| {
        let mut client = dial(address, &device(2), &user(2));
        assert_eq!(
            client
                .ask(&Request::Hello {
                    version: COORD_VERSION + 3
                })
                .unwrap(),
            Response::Welcome {
                version: COORD_VERSION
            },
            "a client one release ahead of the coordinator was cut off"
        );
    });
    assert_eq!(
        itsanas_coord::protocol::agreed(COORD_VERSION),
        Some(COORD_VERSION)
    );
    assert_eq!(
        itsanas_coord::protocol::agreed(COORD_VERSION + 1),
        Some(COORD_VERSION),
        "a client accepts no Welcome from a coordinator older than it"
    );
}

/// A member in China reaching a coordinator in France: a quarter of a second
/// each way and a foreign flow throttled to a few kilobytes a second. What
/// this guards is that no deadline of this project is tighter than such a
/// path needs, the client's whole-session budget included.
#[test]
fn red_team_a_coordinator_behind_a_slow_throttled_path_still_answers() {
    use itsanas_testkit::path::{HostilePath, Rules};
    with_coordinator(|address, _| {
        let path = HostilePath::start(
            address,
            Rules {
                delay: Duration::from_millis(250),
                rate: Some(4 * 1024),
                ..Rules::default()
            },
        );
        let nicolas = user(0x51);
        let started = Instant::now();
        let mut client = CoordClient::connect(path.address(), &device(0x51), None)
            .expect("the handshake did not survive a slow, throttled path");
        let registered = client.ask(&Request::Register(Box::new(
            Registration {
                username: "loin".to_owned(),
                user: nicolas.public(),
                issued_unix: NOW,
            }
            .sign(&nicolas),
        )));
        assert!(
            matches!(registered, Ok(Response::Account(_))),
            "registering across the path failed: {registered:?}"
        );
        let fetched = client.ask(&Request::GetEscrow {
            username: "loin".to_owned(),
        });
        assert!(
            matches!(fetched, Ok(Response::Escrow(_))),
            "a recovery fetch across the path failed: {fetched:?}"
        );
        let sent = path.sent_by_clients();
        assert!(
            !sent
                .windows(7)
                .any(|seen| seen.eq_ignore_ascii_case(b"itsanas")),
            "what a member sent across the border names the project in clear, for a filter to match"
        );
        eprintln!("across the slow path: {:?}", started.elapsed());
    });
}

/// THE ATTACK, or a censor's throttle at its worst: the path lets one byte
/// through now and then, each well inside the per-read timeout, so no single
/// read ever times out. Without a deadline on the whole session, the client
/// (a daemon's round, a person typing a recovery) waited for as long as the
/// path cared to drip.
#[test]
fn red_team_a_coordinator_trickled_by_the_path_is_given_up_at_the_deadline() {
    use itsanas_testkit::path::{HostilePath, Rules};
    with_coordinator(|address, _| {
        let path = HostilePath::start(
            address,
            Rules {
                trickle: Some(Duration::from_millis(100)),
                ..Rules::default()
            },
        );
        let budget = Duration::from_secs(3);
        let through = path.address();
        let (tell, told) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let started = Instant::now();
            let outcome = CoordClient::connect_within(through, &device(0x61), None, budget);
            let _ = tell.send((outcome.is_ok(), started.elapsed()));
        });
        let (connected, took) = told
            .recv_timeout(Duration::from_secs(30))
            .expect("the client was still waiting on a trickling path after 30 s");
        assert!(
            !connected,
            "a handshake of a few kilobytes finished at ten bytes a second"
        );
        assert!(
            took < budget + Duration::from_secs(2),
            "the client gave up after {took:?}, well past its {budget:?} budget"
        );
    });
}

/// A censor cuts a connection it does not like, often in the first round
/// trip. The client must say so at once, not after a timeout, and a dial
/// after it must work: nothing of the cut one is kept.
#[test]
fn a_connection_cut_by_the_path_fails_at_once_and_the_next_dial_answers() {
    use itsanas_testkit::path::{HostilePath, Rules};
    with_coordinator(|address, _| {
        let path = HostilePath::start(
            address,
            Rules {
                cut_first_after: Some(0),
                ..Rules::default()
            },
        );
        let started = Instant::now();
        assert!(
            CoordClient::connect(path.address(), &device(0x71), None).is_err(),
            "a connection the path cut was reported as working"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a cut connection took {:?} to report, as long as a timeout",
            started.elapsed()
        );
        let mut client = CoordClient::connect(path.address(), &device(0x71), None)
            .expect("the dial after a cut one failed");
        assert!(matches!(
            client.ask(&Request::Hosts).unwrap(),
            Response::Hosts(_)
        ));
    });
}

#[test]
fn a_connection_that_asks_too_much_is_told_why_rather_than_cut_off() {
    // A silent close surfaces as "connection aborted by your host software",
    // which reads like a firewall. Somebody debugging that looks at their
    // network for an hour before looking at the protocol.
    with_coordinator(|address, _| {
        let mut client = dial(address, &device(7), &user(7));
        let mut last = Response::Done;
        for _ in 0..40 {
            match client.ask(&Request::Lookup {
                username: "nobody".to_owned(),
            }) {
                Ok(response) => last = response,
                Err(_) => break,
            }
        }
        assert!(
            matches!(last, Response::Refused(_)),
            "the cap was hit without the caller being told, got {last:?}"
        );
    });
}

/// MIXED VERSIONS. A client older than the device bound sends one claim too many
/// as it always did and reads `Refused` as it always has; it must get there
/// at once, with text that names the devices and the command, rather than a
/// closed connection it would retry or a `Done` that lies.
#[test]
fn red_team_a_client_older_than_the_bound_is_refused_in_words_on_the_wire() {
    with_coordinator(|address, directory| {
        let nicolas = user(1);
        let first = enrolled_member(address, "nicolas", &nicolas, &device(1));
        drop(first);
        for seed in 2..=u8::try_from(MAX_DEVICES_PER_ACCOUNT).expect("small") {
            let current = device(seed);
            let mut per_device = dial(address, &current, &nicolas);
            claim_device(&mut per_device, &nicolas, &current, false, NOW);
        }

        let one_too_many = device(u8::try_from(MAX_DEVICES_PER_ACCOUNT + 1).expect("small"));
        let mut client = dial(address, &one_too_many, &nicolas);
        let started = std::time::Instant::now();
        let answer = client
            .ask(&Request::Claim(Box::new(
                NodeClaim {
                    owner: nicolas.user_id(),
                    device: one_too_many.device_id(),
                    pledged_bytes: 1 << 30,
                    issued_unix: NOW,
                    revoked: false,
                }
                .sign(&nicolas),
            )))
            .expect("the coordinator answers rather than hanging up");

        let Response::Refused(why) = answer else {
            panic!("one device too many was not refused: {answer:?}");
        };
        assert!(
            why.contains(&device(1).device_id().to_string())
                && why.contains("itsanas device forget"),
            "the refusal an old client prints does not say what to do: {why}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the refusal stalled"
        );
        assert!(
            directory
                .claim_for(one_too_many.device_id())
                .unwrap()
                .is_none()
        );
    });
}
