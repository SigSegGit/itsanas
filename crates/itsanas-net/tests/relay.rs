//! A relay for a machine nobody can reach (§8 0v), over real sockets.
//!
//! Three nodes of one account: R listens, B never accepts a connection and
//! parks on R instead, A reaches B only through R.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use itsanas_crypto::{DeviceId, DeviceKeys, MasterSecret, SecretBytes, UserKeys};
use itsanas_net::{
    PeerClient, PeerServer, PeerService, Pledge, Result,
    relay::{NOT_RELAYING, RELAY_TOO_OLD},
    session,
};
use itsanas_store::{ChunkerConfig, Store, Vault};

struct Node {
    _dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
    device: DeviceKeys,
}

fn node(seed: u8) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let device = DeviceKeys::from_seed(&SecretBytes::new([seed; 32]));
    let store = Store::open_for_testing(
        dir.path().join("store"),
        UserKeys::derive(&MasterSecret::from_bytes([0xA1; 32])),
        DeviceKeys::from_seed(&device.seed()),
        ChunkerConfig::default(),
    )
    .unwrap();
    let vault = Vault::open(dir.path().join("vault")).unwrap();
    Node {
        _dir: dir,
        store,
        vault,
        device,
    }
}

/// The relay's book: the devices it counts as its account's; `None`, all.
struct Book(Option<Vec<DeviceId>>);

impl itsanas_net::Relay for Book {
    fn presences_for(&self, caller: DeviceId) -> Option<Vec<Vec<u8>>> {
        self.0
            .as_ref()
            .is_none_or(|known| known.contains(&caller))
            .then(Vec::new)
    }
}

struct Stop<'a>(&'a AtomicBool, std::net::SocketAddr);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

/// Run `body` with R serving (relaying for its account when `relays`), and
/// `parker` parked on it once.
fn with_relay<T>(
    relay: &Node,
    relays: Option<Vec<DeviceId>>,
    protocol: u16,
    parker: &Node,
    body: impl FnOnce(std::net::SocketAddr) -> T,
) -> T {
    let book = Book(relays);
    let server = PeerServer::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap();
    let shutdown = AtomicBool::new(false);
    let service = PeerService::new(&relay.store, &relay.vault, Pledge::gigabytes(1))
        .with_relay(&book)
        .speaking_at_most(protocol);
    // B's own listener is never dialled: it is here for its TLS identity.
    let parked_server = PeerServer::bind("127.0.0.1:0").unwrap();
    let parked_service = PeerService::new(&parker.store, &parker.vault, Pledge::gigabytes(1));
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &relay.device, &shutdown);
        });
        scope.spawn(|| {
            let _ = parked_server.serve_parked(
                address,
                relay.store.device_id(),
                &parked_service,
                &parker.device,
                &shutdown,
            );
        });
        let _stop = Stop(&shutdown, address);
        // The parker's connection is accepted before the body runs: one
        // arriving after the stop would wait out its handshake timeout.
        let witness = server.witness();
        let until = Instant::now() + Duration::from_secs(5);
        while witness.from_inside() == 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        body(address)
    })
}

/// Dial `target` through the relay, retrying while the park is being made.
fn through(
    address: std::net::SocketAddr,
    relay: &Node,
    target: DeviceId,
    me: &Node,
) -> Result<PeerClient> {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let attempt = PeerClient::connect_relayed(
            address,
            relay.store.device_id(),
            target,
            &me.device,
            me.store.owner(),
            Some(Duration::from_secs(30)),
        );
        if attempt.is_ok() || Instant::now() > until {
            return attempt;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_machine_nobody_can_reach_syncs_through_a_relay() {
    let r = node(10);
    let b = node(11);
    let a = node(12);
    b.store
        .write_file("from-b.txt", b"behind a closed router")
        .unwrap();
    b.store.flush_segment().unwrap();

    with_relay(&r, None, itsanas_net::PROTOCOL_VERSION, &b, |address| {
        let mut client = through(address, &r, b.store.device_id(), &a).expect("through the relay");
        assert_eq!(
            client.peer_device(),
            b.store.device_id(),
            "the end-to-end session did not reach the parked machine"
        );
        session::round(&a.store, &a.vault, &mut client).expect("round through the relay");
    });
    assert_eq!(
        a.store.read_file("from-b.txt").unwrap().unwrap(),
        b"behind a closed router"
    );
    assert_eq!(
        r.store.read_file("from-b.txt").unwrap(),
        None,
        "the relay kept what it carried"
    );
}

#[test]
fn red_team_a_device_parked_on_the_relay_cannot_receive_another_devices_callers() {
    // C parks under the key it proved; asking for B reaches nobody, and
    // the refusal is the one a stranger gets.
    let r = node(20);
    let c = node(21);
    let b = node(22);
    let a = node(23);
    with_relay(&r, None, itsanas_net::PROTOCOL_VERSION, &c, |address| {
        // C's park is in place once a splice to C would succeed... but we
        // ask for B, which nobody parked as.
        std::thread::sleep(Duration::from_millis(300));
        let refused = PeerClient::connect_relayed(
            address,
            r.store.device_id(),
            b.store.device_id(),
            &a.device,
            a.store.owner(),
            Some(Duration::from_secs(10)),
        )
        .expect_err("a splice to an unparked device was made");
        assert!(
            refused.to_string().contains(NOT_RELAYING),
            "unexpected refusal: {refused}"
        );
    });
}

#[test]
fn red_team_a_relay_refuses_strangers_the_same_way_whether_or_not_the_target_is_parked() {
    let r = node(30);
    let b = node(31);
    let a = node(32);
    // B is of the relay's account and parks; A is not.
    let known = Some(vec![b.store.device_id()]);
    with_relay(&r, known, itsanas_net::PROTOCOL_VERSION, &b, |address| {
        std::thread::sleep(Duration::from_millis(300));
        let refused = PeerClient::connect_relayed(
            address,
            r.store.device_id(),
            b.store.device_id(),
            &a.device,
            a.store.owner(),
            Some(Duration::from_secs(10)),
        )
        .expect_err("a relay spliced for a caller it does not relay for");
        assert!(
            refused.to_string().contains(NOT_RELAYING),
            "unexpected refusal: {refused}"
        );
    });
}

#[test]
fn a_relay_older_than_the_relay_protocol_is_never_asked_to_splice() {
    let r = node(40);
    let b = node(41);
    let a = node(42);
    with_relay(
        &r,
        None,
        itsanas_net::protocol::PROTOCOL_WITH_RELAY - 1,
        &b,
        |address| {
            let refused = PeerClient::connect_relayed(
                address,
                r.store.device_id(),
                b.store.device_id(),
                &a.device,
                a.store.owner(),
                Some(Duration::from_secs(10)),
            )
            .expect_err("an old relay was asked to splice");
            assert!(refused.to_string().contains(RELAY_TOO_OLD), "{refused}");
        },
    );
}

#[test]
fn red_team_a_relay_that_closes_the_park_with_nobody_is_told_apart_from_a_caller_served() {
    // The daemon parks again at once after a caller and backs off otherwise:
    // a relay closing every park must not be dialled in a loop.
    let r = node(50);
    let b = node(51);
    let book = Book(None);
    let server = PeerServer::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap();
    let shutdown = AtomicBool::new(false);
    let parker_stop = AtomicBool::new(false);
    let service = PeerService::new(&r.store, &r.vault, Pledge::gigabytes(1)).with_relay(&book);
    let parked_server = PeerServer::bind("127.0.0.1:0").unwrap();
    let parked_service = PeerService::new(&b.store, &b.vault, Pledge::gigabytes(1));
    let parked = std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &r.device, &shutdown);
        });
        let parker = scope.spawn(|| {
            parked_server.serve_parked(
                address,
                r.store.device_id(),
                &parked_service,
                &b.device,
                &parker_stop,
            )
        });
        let witness = server.witness();
        while witness.from_inside() == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(300));
        drop(Stop(&shutdown, address));
        parker.join().unwrap()
    });
    assert!(
        !matches!(parked, Ok(true)),
        "a park closed with nobody read as a caller served: {parked:?}"
    );
}

#[test]
fn red_team_a_parked_machine_lets_go_at_once_and_the_relay_sees_it_gone() {
    // A stopping daemon waited out a 30 s read on its park, and the relay
    // learned a parker was gone only at its next keepalive -- splicing
    // callers onto a dead park meanwhile.
    let r = node(60);
    let b = node(61);
    let a = node(62);
    let book = Book(None);
    let server = PeerServer::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap();
    let shutdown = AtomicBool::new(false);
    let parker_stop = AtomicBool::new(false);
    let service = PeerService::new(&r.store, &r.vault, Pledge::gigabytes(1)).with_relay(&book);
    let parked_server = PeerServer::bind("127.0.0.1:0").unwrap();
    let parked_service = PeerService::new(&b.store, &b.vault, Pledge::gigabytes(1));
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &r.device, &shutdown);
        });
        let _stop = Stop(&shutdown, address);
        let parker = scope.spawn(|| {
            parked_server.serve_parked(
                address,
                r.store.device_id(),
                &parked_service,
                &b.device,
                &parker_stop,
            )
        });
        let witness = server.witness();
        while witness.from_inside() == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(500));
        let stopping = Instant::now();
        parker_stop.store(true, Ordering::Relaxed);
        let _ = parker.join().unwrap();
        assert!(
            stopping.elapsed() < Duration::from_secs(3),
            "a parked machine took {:?} to stop",
            stopping.elapsed()
        );
        std::thread::sleep(Duration::from_millis(1500));
        let refused = PeerClient::connect_relayed(
            address,
            r.store.device_id(),
            b.store.device_id(),
            &a.device,
            a.store.owner(),
            Some(Duration::from_secs(10)),
        )
        .expect_err("a caller was spliced onto a park whose machine had gone");
        assert!(refused.to_string().contains(NOT_RELAYING), "{refused}");
    });
}
