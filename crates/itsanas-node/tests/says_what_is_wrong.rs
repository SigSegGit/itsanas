//! What a member is told about their own reachability, end to end.
//!
//! A real coordinator on a real socket, a real node with a real listener. The
//! unit tests in `itsanas-coord` prove that the decision refuses what it must
//! and that a probe can tell one machine from another; this proves the two
//! halves are joined, and that what comes back is a sentence a person can act
//! on rather than a boolean nothing reads.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_coord::Directory;
use itsanas_coord::server::CoordServer;
use itsanas_crypto::{DeviceKeys, SecretBytes};
use itsanas_net::{PeerServer, PeerService, Pledge};
use itsanas_node::{coordinator, node::Node};

const PASSPHRASE: &str = "a passphrase for a test and nowhere else";
const NOW: u64 = 1_700_000_000;

/// Stops a server even when the body panics: otherwise a failed assertion
/// leaves an accept loop running and the harness reports a hang.
struct StopOnDrop<'a>(&'a AtomicBool, SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

fn with_coordinator<T>(body: impl FnOnce(SocketAddr) -> T) -> T {
    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
    let server = CoordServer::bind("127.0.0.1:0").expect("bind");
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let device = DeviceKeys::from_seed(&SecretBytes::new([0xC0; 32]));

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&directory, &device, &shutdown, |_| {});
        });
        let _stop = StopOnDrop(&shutdown, address);
        body(address)
    })
}

/// A node registered with `coordinator`, announcing `announce`.
fn member(home: &std::path::Path, coordinator_address: SocketAddr, announce: Option<&str>) -> Node {
    let (mut node, _phrase) = Node::create(home, PASSPHRASE, "nicolas").expect("create");
    node.config.coordinator = Some(coordinator_address.to_string());
    node.config.announce = announce.map(str::to_owned);
    node.save_config().expect("save config");
    coordinator::register_with(&node, None, NOW).expect("register");
    node
}

/// THE QUESTION A MACHINE CANNOT ANSWER ABOUT ITSELF. A node knows it reached
/// the coordinator, because it just did. Nothing tells it whether anything can
/// come back the other way -- and a member whose port forward is wrong looks,
/// to every other member, exactly like a member who is switched off.
#[test]
fn a_member_who_publishes_an_address_only_their_lan_can_dial_is_told_why() {
    with_coordinator(|coordinator_address| {
        let homes = tempfile::tempdir().expect("temp dir");
        let node = member(&homes.path().join("pi"), coordinator_address, None);

        // A real listener, on loopback, as this device. The announced address
        // has to be one the coordinator will dial, and loopback is refused for
        // good reason -- so the node publishes where it really listens and the
        // probe is aimed there by the announce, not by the test.
        let server = PeerServer::bind("127.0.0.1:0").expect("bind");
        let listening = server.local_addr().expect("address");
        let shutdown = AtomicBool::new(false);
        let service = PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1));

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = server.serve_until(&service, &node.device, &shutdown);
            });
            let _stop = StopOnDrop(&shutdown, listening);

            coordinator::announce(&node, &listening.to_string(), NOW).expect("announce");

            let answer = coordinator::check_me(&node)
                .expect("the coordinator answers")
                .expect("this coordinator is new enough to try");

            // Loopback is refused as a probe target, on purpose: a coordinator
            // dialling private addresses scans its own network. So what this
            // asserts is that the refusal *says so*, rather than reporting the
            // member as broken.
            assert!(
                !answer.reachable && answer.detail.contains("private"),
                concat!(
                    "a private announced address must be explained rather than ",
                    "reported as a failure of the member; got {:?}"
                ),
                answer
            );
        });
    });
}

/// The case that costs a person an evening: something answers at the announced
/// address, and it is not them. A forward pointing at the wrong host on the
/// LAN, or a public address shared with somebody else. An open port would read
/// as success; a handshake does not.
#[test]
fn a_member_whose_address_reaches_the_wrong_machine_is_told_which_way_it_is_wrong() {
    with_coordinator(|coordinator_address| {
        let homes = tempfile::tempdir().expect("temp dir");
        let node = member(
            &homes.path().join("pi"),
            coordinator_address,
            // A name that can never resolve, so this test can never reach out
            // of the machine it runs on.
            Some("nothing-here.invalid:9801"),
        );

        coordinator::announce(&node, &node.config.listen.clone(), NOW).expect("announce");

        let answer = coordinator::check_me(&node)
            .expect("the coordinator answers")
            .expect("this coordinator is new enough to try");

        assert!(!answer.reachable, "an unresolvable name is not reachable");
        assert!(
            answer.detail.contains("does not resolve"),
            concat!(
                "DNS and a closed port are different problems and a person ",
                "fixes them in different places; got {:?}"
            ),
            answer
        );
    });
}

/// A member may ask once an hour. The second ask inside it is answered from
/// what is already known rather than with another outbound connection -- which
/// is the whole reason this is affordable with three thousand machines.
#[test]
fn red_team_asking_twice_in_a_row_does_not_cost_the_coordinator_twice() {
    with_coordinator(|coordinator_address| {
        let homes = tempfile::tempdir().expect("temp dir");
        let node = member(
            &homes.path().join("pi"),
            coordinator_address,
            Some("nothing-here.invalid:9801"),
        );
        coordinator::announce(&node, &node.config.listen.clone(), NOW).expect("announce");

        let first = coordinator::check_me(&node)
            .expect("answered")
            .expect("new enough");
        assert!(first.detail.contains("does not resolve"));

        let second = coordinator::check_me(&node)
            .expect("answered")
            .expect("new enough");
        assert!(
            second.detail.contains("within the hour"),
            concat!(
                "a second probe inside the window spent another outbound ",
                "connection; got {:?}"
            ),
            second
        );
    });
}
