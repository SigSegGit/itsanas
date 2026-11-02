//! Stand-ins for the network the shells' tests need: a real coordinator, in
//! process, and a machine of the account that serves. Compiled for this crate's
//! own tests and, behind the `testing` feature, for a shell crate's.

#![allow(clippy::missing_panics_doc)]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_coord::Directory;
use itsanas_coord::directory::Admission;
use itsanas_coord::server::CoordServer;
use itsanas_crypto::{DeviceId, DeviceKeys, SecretBytes};
use itsanas_net::{PeerServer, PeerService, Pledge};
use itsanas_node::Node;
use itsanas_node::owners::ClaimBook;

pub const PASSPHRASE: &str = "a passphrase for a test and nowhere else";
pub const NOW: u64 = 1_700_000_000;

/// Stops the server even when the body panics, so a failed assertion is
/// reported as one and not as a hang.
struct StopOnDrop<'a>(&'a AtomicBool, SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

pub fn with_directory<T>(body: impl FnOnce(SocketAddr, &Directory) -> T) -> T {
    admitting(Admission::Open, body)
}

/// [`with_directory`], under a stated admission policy.
pub fn admitting<T>(admission: Admission, body: impl FnOnce(SocketAddr, &Directory) -> T) -> T {
    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
    let server = CoordServer::bind("127.0.0.1:0").expect("bind");
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let coordinator_device = DeviceKeys::from_seed(&SecretBytes::new([0xC0; 32]));

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_admitting(
                &directory,
                &coordinator_device,
                admission,
                &shutdown,
                |_| {},
            );
        });
        let _stop = StopOnDrop(&shutdown, address);
        body(address, &directory)
    })
}

/// A phone of the account `phrase` names (a new account `username` when
/// `None`), pointed at `coordinator`, not yet enrolled.
#[must_use]
pub fn phone(
    home: &Path,
    coordinator: SocketAddr,
    username: &str,
    phrase: Option<&str>,
) -> (Node, String) {
    let (mut node, phrase) = match phrase {
        None => {
            let (node, phrase) = Node::create(home, PASSPHRASE, username).expect("create");
            (node, phrase.0.to_string())
        }
        Some(phrase) => (
            Node::restore(home, PASSPHRASE, username, phrase).expect("restore"),
            phrase.to_owned(),
        ),
    };
    node.config.coordinator = Some(coordinator.to_string());
    node.save_config().expect("save config");
    (node, phrase)
}

pub fn live(directory: &Directory, device: DeviceId) -> bool {
    directory
        .claim_for(device)
        .expect("read")
        .is_some_and(|signed| !signed.claim.revoked)
}

/// Stops the peer server even when the body panics.
struct StopServing<'a>(&'a AtomicBool, std::net::SocketAddr);

impl Drop for StopServing<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

/// `host` serving as a machine of the account does, for the length of `body`.
pub fn serving<T>(host: &Node, body: impl FnOnce(std::net::SocketAddr) -> T) -> T {
    serving_on(&PeerServer::bind("127.0.0.1:0").expect("bind"), host, body)
}

/// [`serving`] on a listener already bound, for a test that has to publish
/// the address first.
pub fn serving_on<T>(
    server: &PeerServer,
    host: &Node,
    body: impl FnOnce(std::net::SocketAddr) -> T,
) -> T {
    let book = ClaimBook::new();
    let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
        .with_owners(&book)
        .claiming(host.claim_bytes(itsanas_discover::now_unix()));
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &host.device, &shutdown);
        });
        let _stop = StopServing(&shutdown, address);
        body(address)
    })
}
