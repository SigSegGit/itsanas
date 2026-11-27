//! A real coordinator on a real socket.
//!
//! Everything below the address is production: a real directory, real TLS with
//! device authentication, real signatures, real framing. The only thing these
//! tests simulate is which machine each side runs on.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use itsanas_coord::claim::{NodeClaim, Presence};
use itsanas_coord::directory::Registration;
use itsanas_coord::protocol::{MAX_PEERS_RETURNED, Request, Response};
use itsanas_coord::server::{CoordClient, CoordServer};
use itsanas_coord::service::PRESENCE_KEPT_FOR;
use itsanas_coord::{COORD_VERSION, Directory, MAX_DEVICES_PER_ACCOUNT};
use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

mod escrow;
mod members;
mod names;
mod serving;
mod transport;

const NOW: u64 = 1_700_000_000;

fn user(seed: u8) -> UserKeys {
    UserKeys::derive(&MasterSecret::from_bytes([seed; 32]))
}

fn device(seed: u8) -> DeviceKeys {
    DeviceKeys::from_seed(&SecretBytes::new([seed; 32]))
}

/// Stops the server even when the body panics.
///
/// Without it a failing assertion leaves the accept loop running,
/// `thread::scope` never returns, and the harness reports a hang instead of the
/// assertion — which is how a one-line failure costs ten minutes.
struct StopOnDrop<'a>(&'a AtomicBool, std::net::SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

/// Make `device` a live device of `owner`, written straight into the
/// directory: what a member's machine is when it asks a `Lookup`.
fn enrol_in(directory: &Directory, owner: &UserKeys, device: &DeviceKeys) {
    directory
        .register(
            &Registration {
                username: format!("member-{}", device.device_id().short()),
                user: owner.public(),
                issued_unix: NOW,
            }
            .sign(owner),
            NOW,
        )
        .expect("register");
    directory
        .claim(
            &NodeClaim {
                owner: owner.user_id(),
                device: device.device_id(),
                pledged_bytes: 0,
                issued_unix: NOW,
                revoked: false,
            }
            .sign(owner),
            NOW,
        )
        .expect("claim");
}

/// Run `body` against a coordinator listening on loopback.
fn with_coordinator<T>(body: impl FnOnce(std::net::SocketAddr, &Directory) -> T) -> T {
    with_server(&CoordServer::bind("127.0.0.1:0").expect("bind"), body)
}

/// [`with_coordinator`], with a server set up by the caller.
fn with_server<T>(
    server: &CoordServer,
    body: impl FnOnce(std::net::SocketAddr, &Directory) -> T,
) -> T {
    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let coordinator = device(0xC0);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&directory, &coordinator, &shutdown, |_| {});
        });

        let _stop = StopOnDrop(&shutdown, address);
        body(address, &directory)
    })
}

fn dial(address: std::net::SocketAddr, keys: &DeviceKeys, _owner: &UserKeys) -> CoordClient {
    CoordClient::connect(address, keys, None).expect("dial the coordinator")
}

/// Register `name` and enrol `keys` under it over one connection.
fn enrolled_member(
    address: std::net::SocketAddr,
    name: &str,
    owner: &UserKeys,
    keys: &DeviceKeys,
) -> CoordClient {
    let mut client = dial(address, keys, owner);
    let registration = Registration {
        username: name.to_owned(),
        user: owner.public(),
        issued_unix: NOW,
    }
    .sign(owner);
    assert!(matches!(
        client
            .ask(&Request::Register(Box::new(registration)))
            .unwrap(),
        Response::Account(_)
    ));
    claim_device(&mut client, owner, keys, false, NOW);
    client
}

fn claim_device(
    client: &mut CoordClient,
    owner: &UserKeys,
    keys: &DeviceKeys,
    revoked: bool,
    at: u64,
) {
    let claim = NodeClaim {
        owner: owner.user_id(),
        device: keys.device_id(),
        pledged_bytes: 3 << 30,
        issued_unix: at,
        revoked,
    }
    .sign(owner);
    assert!(matches!(
        client.ask(&Request::Claim(Box::new(claim))).unwrap(),
        Response::Done
    ));
}
