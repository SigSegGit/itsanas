//! Five live devices per account, from the machine being enrolled.
//!
//! Decided by Nicolas on 2026-09-30. The coordinator's half is proved in
//! `itsanas-coord` (`directory.rs`, `tests/coordinator.rs`); this proves the
//! client's half is wired: a node refuses, before it signs anything, to enrol
//! a sixth device, and names the five to choose from. The coordinator here
//! plays one older than the bound (`Directory::bound_devices(false)`), so
//! that what is refused is refused by the client and not by the other side.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_coord::server::CoordServer;
use itsanas_coord::{Directory, MAX_DEVICES_PER_ACCOUNT};
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

fn with_directory<T>(body: impl FnOnce(SocketAddr, &Directory) -> T) -> T {
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
        body(address, &directory)
    })
}

/// A node of the account `phrase` names (a new account when `None`), pointed
/// at `coordinator` and not yet enrolled.
fn machine(
    home: &std::path::Path,
    coordinator: SocketAddr,
    phrase: Option<&str>,
) -> (Node, String) {
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
    node.config.coordinator = Some(coordinator.to_string());
    node.save_config().expect("save config");
    (node, phrase)
}

/// Five machines of one account, enrolled, and announced when `announced`, so
/// that a sixth that is not enrolled yet can see them (`ClaimedPeers`).
/// Unannounced, they are machines lost long ago: only the coordinator's own
/// count knows them.
fn a_full_account(
    root: &std::path::Path,
    coordinator: SocketAddr,
    announced: bool,
) -> (Vec<Node>, String) {
    let mut nodes = Vec::new();
    let mut phrase: Option<String> = None;
    for index in 0..MAX_DEVICES_PER_ACCOUNT {
        let (node, said) = machine(
            &root.join(format!("m{index}")),
            coordinator,
            phrase.as_deref(),
        );
        coordinator::register_with(&node, None, NOW).expect("a device under the bound enrols");
        if announced {
            let listen = node.config.listen.clone();
            coordinator::announce(&node, &listen, NOW).expect("announce");
        }
        phrase = Some(said);
        nodes.push(node);
    }
    (nodes, phrase.expect("five machines"))
}

/// THE CLIENT'S HALF. Against a coordinator that would take a sixth, the
/// machine being enrolled refuses on its own, sends no claim, and says which
/// five devices exist and how to free a slot.
#[test]
fn red_team_a_sixth_machine_refuses_to_enrol_itself_even_where_the_coordinator_would_not() {
    with_directory(|address, directory| {
        directory.bound_devices(false);
        let dir = tempfile::tempdir().expect("temp dir");
        let (nodes, phrase) = a_full_account(dir.path(), address, true);

        let (sixth, _) = machine(&dir.path().join("sixth"), address, Some(&phrase));
        let refused =
            coordinator::register_with(&sixth, None, NOW).expect_err("a sixth device was enrolled");

        let text = refused.to_string();
        assert!(
            text.contains("nothing was sent") && text.contains("itsanas device forget"),
            "the refusal is not the client's, or does not say how to free a slot: {text}"
        );
        assert!(
            nodes
                .iter()
                .all(|node| text.contains(&node.store.device_id().to_string())),
            "the refusal does not name the five devices to choose from: {text}"
        );
        assert!(
            directory
                .claim_for(sixth.store.device_id())
                .expect("read")
                .is_none(),
            "the client sent the claim anyway"
        );
        // The Android app has no `device forget` to type: it reads the ids
        // out of this refusal to put a withdraw button beside each.
        let named = coordinator::cap_named(&refused).expect("not read as the limit's refusal");
        assert_eq!(
            named.len(),
            nodes.len(),
            "the phone would not be offered every device: {named:?}"
        );
    });
}

/// `itsanas register` run again on a machine of a full account is a
/// re-signing, and takes no slot. Counting it would refuse every pledge change
/// on a full account.
#[test]
fn red_team_a_machine_of_a_full_account_can_register_again() {
    with_directory(|address, _| {
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut nodes, _) = a_full_account(dir.path(), address, true);

        let first = nodes.remove(0);
        coordinator::register_with(&first, None, NOW + 1)
            .expect("a live device of a full account was refused its own re-signing");
    });
}

/// THE LOCKOUT `itsanas-redteam` found. All five machines of a full
/// account are lost or reinstalled; the new machine is the only one left and
/// is not enrolled. It is refused a slot, and the refusal must carry ids it
/// can act on from where it stands: a withdrawal by full id needs no listing,
/// and frees the slot it then takes. Without this the account could never
/// add a device again.
#[test]
fn red_team_a_new_machine_of_a_full_account_whose_machines_are_all_lost_can_free_a_slot() {
    with_directory(|address, _| {
        let dir = tempfile::tempdir().expect("temp dir");
        // Never announced: the client sees none of them, so the refusal is
        // the coordinator's, which is the one an old client also gets.
        let (nodes, phrase) = a_full_account(dir.path(), address, false);
        let lost = nodes[0].store.device_id();
        drop(nodes);

        let (fresh, _) = machine(&dir.path().join("fresh"), address, Some(&phrase));
        let refused = coordinator::register_with(&fresh, None, NOW + 1)
            .expect_err("a sixth device was enrolled")
            .to_string();
        let named = refused
            .split_whitespace()
            .find(|word| word.trim_matches(|c: char| !c.is_ascii_hexdigit()) == lost.to_string())
            .expect("the refusal does not carry a full id the new machine can withdraw by");
        let wanted: itsanas_crypto::DeviceId = named
            .trim_matches(|c: char| !c.is_ascii_hexdigit())
            .parse()
            .expect("a full device id");

        coordinator::forget_device(&fresh, wanted, NOW + 2)
            .expect("an unenrolled machine of the account could not withdraw a lost one");
        coordinator::register_with(&fresh, None, NOW + 3)
            .expect("the freed slot did not let the new machine in");
    });
}

/// The decision as a pure function, where the count and the re-signing rule
/// can be seen at once.
#[test]
fn room_is_counted_in_live_devices_and_a_device_already_live_needs_none() {
    let device = |seed: u8| DeviceKeys::from_seed(&SecretBytes::new([seed; 32])).device_id();
    let live: Vec<_> = (1..=4).map(|seed| (device(seed), String::new())).collect();
    coordinator::room_for(device(9), &live).expect("four devices leave room for a fifth");

    let full: Vec<_> = (1..=5).map(|seed| (device(seed), String::new())).collect();
    assert!(
        coordinator::room_for(device(9), &full).is_err(),
        "a sixth was given room"
    );
    coordinator::room_for(device(3), &full).expect("a live device re-signing takes no slot");
}
