//! The coordinator's operator suspends an account (an abuse report, a legal
//! request) without reading anything of it: `Directory::suspend`, run by
//! `itsanas-coordinator --suspend <username>`.
//!
//! A host stores for another account's device only once its coordinator
//! confirms that device's standing (HANDOVER §8 1c (ii)), so a suspension is
//! that confirmation withheld. These run a real coordinator and a real host's
//! `ClaimBook`, as `withdrawals.rs` does.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_coord::Directory;
use itsanas_coord::server::CoordServer;
use itsanas_crypto::{DeviceKeys, SecretBytes};
use itsanas_node::owners::{ClaimBook, UNCONFIRMED};
use itsanas_node::{coordinator, node::Node};

const PASSPHRASE: &str = "a passphrase for a test and nowhere else";
const NOW: u64 = 1_700_000_000;
const HOST_PLEDGE: u64 = 1 << 30;

struct StopOnDrop<'a>(&'a AtomicBool, SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

fn coordinator_device() -> DeviceKeys {
    DeviceKeys::from_seed(&SecretBytes::new([0xC0; 32]))
}

/// A coordinator on loopback, and its directory for the operator to act on.
fn with_coordinator<T>(body: impl FnOnce(SocketAddr, &Directory) -> T) -> T {
    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
    let server = CoordServer::bind("127.0.0.1:0").expect("bind");
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let device = coordinator_device();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&directory, &device, &shutdown, |_| {});
        });
        let _stop = StopOnDrop(&shutdown, address);
        body(address, &directory)
    })
}

/// An enrolled machine of a new account `username`.
fn machine(home: &std::path::Path, coordinator: SocketAddr, username: &str) -> Node {
    let (mut node, _) = Node::create(home, PASSPHRASE, username).expect("create");
    node.config.coordinator = Some(coordinator.to_string());
    node.config.coordinator_device = Some(coordinator_device().device_id().to_hex());
    node.config.pledge_bytes = 7 << 20;
    node.save_config().expect("save config");
    coordinator::register_with(&node, None, NOW).expect("enrol");
    node
}

/// Whether `host`, meeting `member` now with a fresh book, would store for it.
fn host_admits(host: &Node, member: &Node) -> Result<(), String> {
    let book = ClaimBook::new();
    book.take(
        member.store.device_id(),
        &member.claim_bytes(NOW),
        &host.vault,
    )
    .expect("a live claim is taken");
    coordinator::standing(host, &book).expect("the coordinator answers");
    book.admits(
        member.store.device_id(),
        member.store.owner(),
        1,
        &host.store,
        &host.vault,
        HOST_PLEDGE,
    )
}

#[test]
fn red_team_a_suspended_accounts_machine_is_not_confirmed_so_a_host_stores_nothing_for_it() {
    // Sabotage: `standing` in itsanas-coord's service answers the held claim
    // whatever the suspension.
    let root = tempfile::tempdir().expect("temp dir");
    with_coordinator(|address, directory| {
        let host = machine(&root.path().join("alice"), address, "alice");
        let bob = machine(&root.path().join("bob"), address, "bob");
        assert_eq!(
            host_admits(&host, &bob),
            Ok(()),
            "fixture: a confirmed machine should store"
        );

        assert!(directory.suspend(bob.store.owner(), NOW).expect("suspend"));
        assert_eq!(
            host_admits(&host, &bob),
            Err(UNCONFIRMED.to_owned()),
            "an account the operator suspended for abuse still gets new data stored on members' hosts"
        );
        assert!(
            coordinator::register_with(&bob, None, NOW + 1).is_err(),
            "a suspended account re-enrols its machine and is back in the network as if never suspended"
        );
    });
}

#[test]
fn red_team_lifting_a_suspension_lets_hosts_store_for_the_account_again() {
    // Sabotage: `unsuspend` leaves the row in place.
    let root = tempfile::tempdir().expect("temp dir");
    with_coordinator(|address, directory| {
        let host = machine(&root.path().join("alice"), address, "alice");
        let bob = machine(&root.path().join("bob"), address, "bob");
        directory.suspend(bob.store.owner(), NOW).expect("suspend");
        assert!(directory.unsuspend(bob.store.owner()).expect("unsuspend"));
        assert_eq!(
            host_admits(&host, &bob),
            Ok(()),
            "a member cleared after a suspension still cannot back anything up on another member's host"
        );
    });
}

#[test]
fn red_team_suspending_one_account_leaves_every_other_account_storing() {
    // Sabotage: `is_suspended` answers true for anybody once one account is
    // suspended (the table is non-empty).
    let root = tempfile::tempdir().expect("temp dir");
    with_coordinator(|address, directory| {
        let host = machine(&root.path().join("alice"), address, "alice");
        let bob = machine(&root.path().join("bob"), address, "bob");
        let carol = machine(&root.path().join("carol"), address, "carol");
        directory.suspend(bob.store.owner(), NOW).expect("suspend");
        assert_eq!(
            host_admits(&host, &carol),
            Ok(()),
            "one abuse report against bob stops carol, who did nothing, from backing anything up"
        );
        assert_eq!(
            directory
                .live_claims_of(carol.store.owner())
                .expect("claims")
                .len(),
            1,
            "carol's machine disappears from the address book because bob was suspended"
        );
        assert!(
            directory
                .live_claims_of(bob.store.owner())
                .expect("claims")
                .is_empty(),
            "a suspended account is still handed out to others as a place to store"
        );
    });
}
