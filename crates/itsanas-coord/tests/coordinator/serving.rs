//! A serving coordinator: what it erases at start and after, and what a
//! restart keeps.

use super::*;

/// One coordinator run over the directory file at `path`, stopped after `body`.
fn serve_once<T>(path: &std::path::Path, body: impl FnOnce(std::net::SocketAddr) -> T) -> T {
    let directory = Directory::open(path).expect("directory");
    let server = CoordServer::bind("127.0.0.1:0").expect("bind");
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let coordinator = device(0xC0);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&directory, &coordinator, &shutdown, |_| {});
        });
        let _stop = StopOnDrop(&shutdown, address);
        body(address)
    })
}

/// Nothing that only a test calls erases anything: the directory proves the
/// rule, and this proves the process that holds the directory runs it, as it
/// starts and again without a restart. Two runs, because a short period would
/// cover for a pass missing at the start.
/// The wall clock, as the serving coordinator reads it.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs()
}

#[test]
fn red_team_a_serving_coordinator_erases_silent_addresses_at_start_and_again_later() {
    let now = unix_now();
    let long_ago = now - PRESENCE_KEPT_FOR - 3600;

    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
    let owner = user(0xD1);
    directory
        .register(
            &Registration {
                username: "camille".to_owned(),
                user: owner.public(),
                issued_unix: long_ago,
            }
            .sign(&owner),
            long_ago,
        )
        .expect("register");
    let on_since = |dev: &DeviceKeys, heard: u64| {
        let claim = NodeClaim {
            owner: owner.user_id(),
            device: dev.device_id(),
            pledged_bytes: 1 << 30,
            issued_unix: long_ago,
            revoked: false,
        }
        .sign(&owner);
        directory.claim(&claim, long_ago).expect("claim");
        let presence = Presence {
            device: dev.device_id(),
            address: "203.0.113.9:9797".to_owned(),
            at_unix: heard,
            head: 0,
        }
        .sign(dev);
        directory.announce(&presence, heard).expect("announce");
    };
    let (silent_at_start, silent_later, on_now) = (device(0xD1), device(0xD2), device(0xD3));
    on_since(&silent_at_start, long_ago);
    on_since(&on_now, now);

    // The default period, so this run has one pass: the one at the start.
    let events = Mutex::new(Vec::new());
    let server = CoordServer::bind("127.0.0.1:0").expect("bind");
    serving(&directory, &server, &events, |address| {
        // Answered, so the listener got as far as accepting.
        let mut client = dial(address, &device(0xD4), &user(0xD4));
        assert!(
            matches!(
                client
                    .ask(&Request::Hello {
                        version: COORD_VERSION
                    })
                    .unwrap(),
                Response::Welcome { .. }
            ),
            "a coordinator that erased what had expired stopped answering members"
        );
        assert_eq!(
            directory.presence_of(silent_at_start.device_id()).unwrap(),
            None,
            "a coordinator started on an old directory still holds an address past the keep, \
             and a coordinator restarted every night never reaches a daily pass"
        );
        on_since(&silent_later, long_ago);
        // Several times round the accept loop, which polls every 50 ms.
        std::thread::sleep(Duration::from_millis(300));
    });
    assert!(
        directory
            .presence_of(silent_later.device_id())
            .unwrap()
            .is_some(),
        "a second pass ran long before its day was up: every pass holds the directory's one \
         write lock, and every member waits behind it"
    );
    let said = events.lock().unwrap().clone();
    assert_eq!(said.len(), 1, "one pass, one line in the log: {said:?}");
    assert!(
        !said[0].contains("203.0.113"),
        "the log now holds what the pass erased: {said:?}"
    );

    // A short period, so the second pass happens while the test watches.
    let server = CoordServer::bind("127.0.0.1:0")
        .expect("bind")
        .with_expiry_every(Duration::from_millis(50));
    serving(&directory, &server, &events, |_| {
        on_since(&device(0xD5), long_ago);
        let started = Instant::now();
        while directory
            .presence_of(device(0xD5).device_id())
            .unwrap()
            .is_some()
        {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "a coordinator left running never erases another address, so what it holds \
                 is limited only by how rarely it restarts"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    assert!(
        directory.presence_of(on_now.device_id()).unwrap().is_some(),
        "a machine heard from just now lost its address: its owner cannot be found"
    );
}

/// `server` serving `directory` for as long as `body` runs, its log in `events`.
fn serving(
    directory: &Directory,
    server: &CoordServer,
    events: &Mutex<Vec<String>>,
    body: impl FnOnce(std::net::SocketAddr),
) {
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let coordinator = device(0xC0);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(directory, &coordinator, &shutdown, |event| {
                events.lock().unwrap().push(event.to_owned());
            });
        });
        let _stop = StopOnDrop(&shutdown, address);
        body(address);
    });
}

#[test]
fn red_team_a_restarted_coordinator_still_refuses_a_spent_escrow_budget() {
    // The disk count is only as good as its wiring: directory.rs proves
    // the table, this proves the server asks it. A restart cleared the in-memory limiter, and with it the
    // budget of guesses at a passphrase (HANDOVER §9).
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("directory.redb");
    let ask = |address| {
        let mut client = dial(address, &device(0xE1), &user(0xE1));
        client
            .ask(&Request::GetEscrow {
                username: "nobody-yet".to_owned(),
            })
            .unwrap()
    };
    serve_once(&path, |address| {
        for attempt in 1..=itsanas_coord::service::ESCROW_ATTEMPTS {
            assert!(
                !matches!(ask(address), Response::Refused(_)),
                "attempt {attempt} of the budget was refused"
            );
        }
    });
    serve_once(&path, |address| {
        assert!(
            matches!(ask(address), Response::Refused(_)),
            "a restart handed out a fresh budget of guesses"
        );
    });
}
