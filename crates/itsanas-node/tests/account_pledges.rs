//! What a person joining an account is told its machines offer, asked of the
//! coordinator before the joining machine has a key of its own.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_coord::Directory;
use itsanas_coord::server::CoordServer;
use itsanas_crypto::{DeviceKeys, SecretBytes};
use itsanas_node::{coordinator, node::Node};

const PASSPHRASE: &str = "a passphrase for a test and nowhere else";
const NOW: u64 = 1_700_000_000;
const GIB: u64 = 1 << 30;

/// Stops the server even when the body panics, so a failed assertion is
/// reported as one and not as a hang.
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

/// A machine of the account `phrase` names (a new one when `None`), enrolled,
/// pledging `pledge` bytes and, when `announced`, published.
fn machine(
    home: &std::path::Path,
    coordinator_at: SocketAddr,
    phrase: Option<&str>,
    pledge: u64,
    announced: bool,
) -> String {
    let (mut node, phrase) = match phrase {
        None => {
            let (node, phrase) = Node::create(home, PASSPHRASE, "nicolas").expect("create");
            (node, phrase.0.to_string())
        }
        Some(phrase) => (
            Node::restore(home, PASSPHRASE, "nicolas", phrase).expect("restore"),
            phrase.to_owned(),
        ),
    };
    node.config.coordinator = Some(coordinator_at.to_string());
    node.config.pledge_bytes = pledge;
    node.save_config().expect("save config");
    coordinator::register_with(&node, None, NOW).expect("enrol");
    if announced {
        let listen = node.config.listen.clone();
        coordinator::announce(&node, &listen, NOW).expect("announce");
    }
    phrase
}

#[test]
fn a_person_joining_is_told_the_largest_pledge_of_the_accounts_machines_and_nothing_else() {
    with_coordinator(|address| {
        let at = address.to_string();
        let dir = tempfile::tempdir().expect("temp dir");

        assert_eq!(
            coordinator::account_pledge(&at, "nicolas").expect("asked"),
            None,
            "a figure was offered for a name no account holds"
        );

        let phrase = machine(&dir.path().join("a"), address, None, 10 * GIB, true);
        machine(
            &dir.path().join("b"),
            address,
            Some(&phrase),
            30 * GIB,
            true,
        );
        assert_eq!(
            coordinator::account_pledge(&at, "nicolas").expect("asked"),
            Some(30 * GIB),
            "the figure is not the largest pledge the machines announced"
        );
        assert_eq!(
            coordinator::account_pledge(&at, "voisin").expect("asked"),
            None,
            "another name was told the account's figure"
        );
    });
}

#[test]
fn a_machine_that_has_not_announced_itself_offers_no_figure() {
    with_coordinator(|address| {
        let dir = tempfile::tempdir().expect("temp dir");
        machine(&dir.path().join("a"), address, None, 10 * GIB, false);
        assert_eq!(
            coordinator::account_pledge(&address.to_string(), "nicolas").expect("asked"),
            None,
            "a pledge nobody could check against a published machine was offered"
        );
    });
}

#[test]
fn an_unreachable_coordinator_is_an_error_and_not_a_figure() {
    assert!(
        coordinator::account_pledge("127.0.0.1:1", "nicolas").is_err(),
        "an answer was invented for a coordinator that could not be asked"
    );
}
