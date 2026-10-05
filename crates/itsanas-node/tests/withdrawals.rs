//! A host asks the coordinator before it stores for another account's device
//! (HANDOVER §8 1c (ii)).
//!
//! Every node holds its account key, so a device its owner withdrew signs
//! itself a fresh live claim and presents it to a host. Only the coordinator
//! knows of the withdrawal. These tests run a real coordinator and prove the
//! node's half is wired: `coordinator::standing` asks, `ClaimBook` listens,
//! and with no answer nothing is stored (Nicolas, 2026-10-05).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_coord::Directory;
use itsanas_coord::server::CoordServer;
use itsanas_crypto::{DeviceKeys, SecretBytes};
use itsanas_node::owners::{ClaimBook, UNCONFIRMED, WITHDRAWN};
use itsanas_node::{coordinator, node::Node};

const PASSPHRASE: &str = "a passphrase for a test and nowhere else";
const NOW: u64 = 1_700_000_000;
const HOST_PLEDGE: u64 = 1 << 30;

/// Stops the server even when the body panics: otherwise a failed assertion
/// leaves the accept loop running and the harness reports a hang.
struct StopOnDrop<'a>(&'a AtomicBool, SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

/// The coordinator's device id, as `itsanas coordinator --device` pins it.
fn coordinator_id() -> String {
    DeviceKeys::from_seed(&SecretBytes::new([0xC0; 32]))
        .device_id()
        .to_hex()
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

/// An enrolled machine of `username`'s account: a new account when `phrase`
/// is `None`, else another machine of the account that phrase restores.
fn machine(
    home: &std::path::Path,
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
    node.config.coordinator_device = Some(coordinator_id());
    // Something to be credited with: 3/7 of it, here.
    node.config.pledge_bytes = 7 << 20;
    node.save_config().expect("save config");
    coordinator::register_with(&node, None, NOW).expect("enrol");
    (node, phrase)
}

fn admits(host: &Node, book: &ClaimBook, member: &Node) -> Result<(), String> {
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
fn red_team_a_withdrawn_device_that_re_signs_stores_nothing_on_a_host() {
    // The attack, end to end: Bob withdraws his laptop from his desktop; the
    // laptop, still holding the account key, signs a fresh live claim and
    // presents it to Alice's host. The host asks the coordinator and is told
    // of the withdrawal. Sabotage: `standing` never asks, or `ClaimBook`
    // ignores the verdict.
    let root = tempfile::tempdir().expect("temp dir");
    with_coordinator(|address| {
        let (host, _) = machine(&root.path().join("alice"), address, "alice", None);
        let (laptop, phrase) = machine(&root.path().join("laptop"), address, "bob", None);
        let (desktop, _) = machine(&root.path().join("desktop"), address, "bob", Some(&phrase));

        let book = ClaimBook::new();
        book.take(
            laptop.store.device_id(),
            &laptop.claim_bytes(NOW),
            &host.vault,
        )
        .expect("a live claim is taken");
        let first = coordinator::standing(&host, &book).expect("the coordinator answers");
        assert_eq!(first.live, 1, "{first:?}");
        assert_eq!(
            admits(&host, &book, &laptop),
            Ok(()),
            "fixture: a confirmed live device should store"
        );

        coordinator::withdraw_device(&desktop, laptop.store.device_id(), NOW + 1)
            .expect("withdraw the laptop");

        // A host that meets the laptop after the withdrawal, re-signed live.
        let elsewhere = ClaimBook::new();
        elsewhere
            .take(
                laptop.store.device_id(),
                &laptop.claim_bytes(NOW + 100),
                &host.vault,
            )
            .expect("the re-signed claim checks out on its own");
        let second = coordinator::standing(&host, &elsewhere).expect("the coordinator answers");
        assert_eq!(second.withdrawn, 1, "{second:?}");
        assert_eq!(
            admits(&host, &elsewhere, &laptop),
            Err(WITHDRAWN.to_owned()),
            "a withdrawn device that signed itself a fresh claim stored on a host"
        );
    });
}

#[test]
fn red_team_a_host_whose_coordinator_does_not_answer_stores_for_no_other_account() {
    // Nicolas, 2026-10-05: no answer means no storing. The coordinator is
    // stopped before the host asks. Sabotage: admit an unconfirmed device.
    let root = tempfile::tempdir().expect("temp dir");
    let (mut host, member) = with_coordinator(|address| {
        let (host, _) = machine(&root.path().join("alice"), address, "alice", None);
        let (member, _) = machine(&root.path().join("bob"), address, "bob", None);
        (host, member)
    });
    // The coordinator is gone; point the host at a port nothing listens on.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    host.config.coordinator = Some(closed.local_addr().expect("address").to_string());
    drop(closed);

    let book = ClaimBook::new();
    book.take(
        member.store.device_id(),
        &member.claim_bytes(NOW),
        &host.vault,
    )
    .expect("a live claim is taken");
    assert!(coordinator::standing(&host, &book).is_err());
    assert_eq!(
        admits(&host, &book, &member),
        Err(UNCONFIRMED.to_owned()),
        "a host that could not reach its coordinator stored for another account"
    );
}

#[test]
fn a_host_that_asks_when_a_device_presents_lets_it_store_on_its_first_round() {
    // The inline half, against a real coordinator: `coordinator::asker`
    // answers before the first store of the same connection would be
    // weighed, so a newcomer is not refused its first round.
    let root = tempfile::tempdir().expect("temp dir");
    with_coordinator(|address| {
        let (host, _) = machine(&root.path().join("alice"), address, "alice", None);
        let (member, _) = machine(&root.path().join("bob"), address, "bob", None);
        let book = ClaimBook::new().asking(host.store.owner(), coordinator::asker(&host));
        book.take(
            member.store.device_id(),
            &member.claim_bytes(NOW),
            &host.vault,
        )
        .expect("a live claim is taken");
        assert_eq!(admits(&host, &book, &member), Ok(()));
    });
}

#[test]
fn red_team_a_host_whose_coordinator_is_not_pinned_stores_for_no_other_account() {
    // Found by the redteam agent: unpinned, whoever answers is believed, and
    // the withdrawn device on the path answers "live" by echoing its own
    // claim. Neither the round nor the inline ask asks an unpinned
    // coordinator. Sabotage: drop `pinned` from either.
    let root = tempfile::tempdir().expect("temp dir");
    with_coordinator(|address| {
        let (mut host, _) = machine(&root.path().join("alice"), address, "alice", None);
        let (member, _) = machine(&root.path().join("bob"), address, "bob", None);
        host.config.coordinator_device = None;

        let round = ClaimBook::new();
        round
            .take(
                member.store.device_id(),
                &member.claim_bytes(NOW),
                &host.vault,
            )
            .expect("a live claim is taken");
        let report = coordinator::standing(&host, &round).expect("nothing to fail");
        assert!(report.unpinned && report.asked == 0, "{report:?}");
        assert_eq!(admits(&host, &round, &member), Err(UNCONFIRMED.to_owned()));

        let inline = ClaimBook::new().asking(host.store.owner(), coordinator::asker(&host));
        inline
            .take(
                member.store.device_id(),
                &member.claim_bytes(NOW),
                &host.vault,
            )
            .expect("a live claim is taken");
        assert_eq!(
            admits(&host, &inline, &member),
            Err(UNCONFIRMED.to_owned()),
            "an unpinned coordinator's answer let another account's device store"
        );
    });
}
