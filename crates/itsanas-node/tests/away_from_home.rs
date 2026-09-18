//! What a member away from their own network is told, and in what order.
//!
//! A real coordinator on a real socket, three real nodes of one account. The
//! only thing simulated is which machine each node runs on, which is exactly
//! the thing this is about: the addresses in play are the ones a laptop at a
//! friend's house is handed, and the question is whether any of them can be
//! dialled from where it now stands.
//!
//! The unit tests in `coordinator.rs` prove the ordering function orders. This
//! proves the lookup *applies* it — deleting the call leaves every one of those
//! green, which is the shape of a defence that is tested and not wired.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_coord::Directory;
use itsanas_coord::server::CoordServer;
use itsanas_crypto::{DeviceKeys, SecretBytes};
use itsanas_node::{coordinator, node::Node};

const PASSPHRASE: &str = "a passphrase for a test and nowhere else";
const NOW: u64 = 1_700_000_000;

/// Stops the server even when the body panics: otherwise a failed assertion
/// leaves the accept loop running and the harness reports a hang.
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
    let coordinator_device = DeviceKeys::from_seed(&SecretBytes::new([0xC0; 32]));

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&directory, &coordinator_device, &shutdown, |_| {});
        });
        let _stop = StopOnDrop(&shutdown, address);
        body(address)
    })
}

/// A node of `phrase`'s account, registered with `coordinator`, publishing
/// `announce` if it has one.
fn member(
    home: &std::path::Path,
    coordinator_address: SocketAddr,
    announce: Option<&str>,
    phrase: Option<&str>,
) -> (Node, Option<String>) {
    let (mut node, phrase_out) = match phrase {
        None => {
            let (node, phrase) = Node::create(home, PASSPHRASE, "nicolas").expect("create");
            (node, Some(phrase.0.to_string()))
        }
        Some(phrase) => (
            Node::restore(home, PASSPHRASE, "nicolas", phrase).expect("restore"),
            None,
        ),
    };

    node.config.coordinator = Some(coordinator_address.to_string());
    node.config.announce = announce.map(str::to_owned);
    node.save_config().expect("save config");

    coordinator::register_with(&node, None, NOW).expect("register");
    (node, phrase_out)
}

/// THE SCENARIO: the laptop is at a friend's house. The coordinator hands it
/// the account's other machines. One of them published a name that resolves
/// from anywhere; the other published the address it has on the LAN at home,
/// which from here is either nothing or a stranger's machine at the same
/// number. Dialling in the order the coordinator happened to return spends the
/// round on the addresses that cannot answer.
#[test]
fn a_member_elsewhere_is_given_the_address_that_can_answer_first() {
    with_coordinator(|address| {
        let homes = tempfile::tempdir().expect("temp dir");

        // The machine at home with a port forwarded to it: a name, and the
        // outside port of the forward, which is not its listening port.
        let (forwarded, phrase) = member(
            &homes.path().join("pi"),
            address,
            Some("ngas.fr:9801"),
            None,
        );
        let phrase = phrase.expect("the first node returns the account phrase");

        // A second machine at home with no forward: it publishes where it is,
        // which is only meaningful on the LAN it is on.
        let (at_home, _) = member(&homes.path().join("vm"), address, None, Some(&phrase));

        // The laptop, elsewhere. It asks for its own account's machines.
        let (laptop, _) = member(&homes.path().join("laptop"), address, None, Some(&phrase));

        for node in [&forwarded, &at_home, &laptop] {
            let listen = node.config.listen.clone();
            coordinator::announce(node, &listen, NOW).expect("announce");
        }

        let found = coordinator::peers(&laptop, laptop.store.owner()).expect("peers");
        let addresses: Vec<&str> = found.iter().map(|(_, a)| a.as_str()).collect();

        assert_eq!(
            addresses.len(),
            2,
            "the account has three machines and a node does not dial itself; \
             got {addresses:?}"
        );
        assert_eq!(
            addresses[0], "ngas.fr:9801",
            "the machine reachable from another network must be offered first, \
             and with the port of its forward rather than the port it listens \
             on; got {addresses:?}"
        );
        assert!(
            coordinator::is_private_address(addresses[1]),
            "the second address is the one only its own LAN can dial; \
             got {addresses:?}"
        );
    });
}
