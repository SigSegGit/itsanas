//! Convergence over the wire: concurrent edits meet, and one key image on
//! two disks stops writing instead of forking the log.

use super::*;

#[test]
fn concurrent_edits_on_two_machines_converge_over_a_socket() {
    // The same property the simulation proves, but through the real protocol,
    // so a bug in the transport that loses or reorders work would show up here.
    let laptop = node(&alice(), 22);
    let pi = node(&alice(), 23);

    // A shared starting point.
    laptop.store.write_file("doc.txt", b"base").unwrap();
    laptop.store.flush_segment().unwrap();

    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();
        session::round(&pi.store, &pi.vault, &mut client).unwrap();
    });
    assert_eq!(pi.store.read_file("doc.txt").unwrap().unwrap(), b"base");

    // Now both edit while apart.
    laptop.store.write_file("doc.txt", b"laptop edit").unwrap();
    laptop.store.flush_segment().unwrap();
    pi.store.write_file("doc.txt", b"pi edit").unwrap();
    pi.store.flush_segment().unwrap();

    // Two rounds: the first carries each side's work across, the second lets
    // the laptop see what the Pi pushed.
    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();
        session::round(&pi.store, &pi.vault, &mut client).unwrap();
        session::round(&pi.store, &pi.vault, &mut client).unwrap();
    });

    let pi_files = pi.store.list().unwrap();
    assert_eq!(
        pi_files.len(),
        2,
        "the Pi should hold both versions after a concurrent edit, got {pi_files:?}"
    );

    let mut bodies: Vec<Vec<u8>> = pi_files
        .iter()
        .map(|path| pi.store.read_file(path).unwrap().unwrap())
        .collect();
    bodies.sort();
    assert_eq!(
        bodies,
        vec![b"laptop edit".to_vec(), b"pi edit".to_vec()],
        "a concurrent edit lost one side's work across the wire"
    );
}

/// One USB key image copied onto two disks, both in use: two machines with one
/// identity, each writing its own changes. The account's log forks on that
/// key, every host refuses the second copy's segments for ever, and in
/// silence: what it writes never reaches anybody. The twin learns it from a
/// segment signed by its own key, at a position where its chain holds
/// another, writes nothing more, and deletes nothing. A copy that is only
/// behind (restored from an older image) is no twin (HANDOVER §8 0e step 6).
/// Sabotage: return `None` from `forked_segment`; or call any head the chain
/// lacks a fork.
#[test]
fn red_team_a_cloned_key_image_stops_writing_and_a_copy_behind_does_not() {
    let host = node(&alice(), 0x71);
    let original = node(&alice(), 0x72);
    original
        .store
        .write_file("from-the-original.txt", b"one")
        .unwrap();
    original.store.flush_segment().unwrap();

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &original.device, original.store.owner(), None).unwrap();
        session::round(&original.store, &original.vault, &mut client).unwrap();

        // The same key, nothing written: a copy restored from an older image.
        let behind = node(&alice(), 0x72);
        let mut client =
            PeerClient::connect(address, &behind.device, behind.store.owner(), None).unwrap();
        let report = session::round(&behind.store, &behind.vault, &mut client).unwrap();
        assert!(
            !report.push.twin,
            "a copy that is only behind was taken for a twin"
        );
        assert!(behind.store.twin().is_none());

        // The same key, its own write: the second disk of the image.
        let twin = node(&alice(), 0x72);
        twin.store.write_file("from-the-twin.txt", b"two").unwrap();
        twin.store.flush_segment().unwrap();
        let mut client =
            PeerClient::connect(address, &twin.device, twin.store.owner(), None).unwrap();
        let report = session::round(&twin.store, &twin.vault, &mut client).unwrap();
        assert!(
            report.push.twin,
            "the second disk of one key image went on writing a forked log"
        );
        assert!(twin.store.twin().is_some());
        assert!(
            twin.store.write_file("after.txt", b"three").is_err(),
            "a twin accepted a write that could never reach another machine"
        );
        assert_eq!(
            twin.store
                .read_file("from-the-twin.txt")
                .unwrap()
                .as_deref(),
            Some(&b"two"[..]),
            "stopping the twin deleted what it held"
        );
    });
}
