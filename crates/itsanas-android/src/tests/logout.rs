//! A logout, what it leaves for the next open, and what it would lose.

use super::*;

/// A phone cannot promise a disk it has not got: pledging more than is free
/// made every later write and pull refuse, with no sentence that said why.
/// Sabotage: drop either check in `check_phone_disk`.
/// A logout with no network and an erase cut short are finished by the next open:
/// nothing else does, and a phone holds its slot at the coordinator for ever
/// otherwise. Sabotage: drop the call from `open_into`; drop the sweep.
#[test]
fn red_team_opening_a_phone_finishes_what_a_logout_or_a_removal_left() {
    with_directory(|address, directory| {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (mut node, phrase) = phone(&home, address, "camille", None);
        coordinator::register_with(&node, None, NOW).unwrap();
        let device = node.store.device_id();
        node.config.coordinator = Some("127.0.0.1:1".to_owned());
        let slot = Mutex::new(Some(Arc::new(node)));
        logout_into(&slot, &home, NOW + 1).unwrap();
        assert!(live(directory, device), "the logout had no network to ask");

        let leftover = dir.path().join("node.removing-cut-short");
        std::fs::create_dir(&leftover).unwrap();
        drop(phone(&home, address, "camille", Some(&phrase)));
        open_into(&slot, &home, PASSPHRASE, &network_at(address), NOW + 2).unwrap();

        assert!(
            !live(directory, device),
            "the old device stays listed: this phone's slot is held for ever"
        );
        assert!(
            !leftover.exists(),
            "an erase cut short stays on the disk with the account's keys in it"
        );
    });
}

/// The reason a withdrawal is safe: what the phone wrote outlives it. Written
/// here, pushed to a sibling that serves, logged out; a new login dialling that
/// sibling lists the file and reads it. Sabotage: ask the host for no claims
/// in `Node::introduce` (the new login's roster refuses the old phone's chain:
/// the file is listed and cannot be read).
#[test]
fn red_team_files_written_by_a_phone_that_logged_out_reach_the_next_login() {
    with_directory(|address, _| {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("node");
        let (mut phone_node, phrase) = phone(&home, address, "camille", None);
        let (sibling, _) = phone(&dir.path().join("nas"), address, "camille", Some(&phrase));
        for node in [&phone_node, &sibling] {
            coordinator::register_with(node, None, NOW).expect("enrol");
        }

        let source = dir.path().join("source");
        std::fs::write(&source, b"written on the phone").expect("source");
        crate::put_file(
            &phone_node,
            "Photos/one.jpg",
            source.to_str().expect("path"),
        )
        .expect("put");

        serving(&sibling, |at| {
            phone_node.config.peers.push(at.to_string());
            phone_node.save_config().expect("save");

            let check = answer(&logout_check(&phone_node, false).expect("check"));
            assert!(check["reached"].as_u64() >= Some(1), "{check}");
            assert_eq!(
                check["onlyHere"], 0,
                "a pushed file is still called the only copy: the person is warned for \
                 nothing, and learns to type through it: {check}"
            );

            let slot = Mutex::new(Some(Arc::new(phone_node)));
            let said = answer(&logout_into(&slot, &home, NOW + 1).expect("logout"));
            assert_eq!(said["withdrawn"], true, "{said}");
            assert!(said["told"].as_u64() >= Some(1), "{said}");

            let restored = restore_account(
                &home,
                NEW_PASSPHRASE,
                "camille",
                &phrase,
                &network_at(address),
            )
            .expect("login");
            *slot.lock().expect("lock") = Some(Arc::new(restored));
            let mut guard = slot.lock().expect("lock");
            let node = Arc::get_mut(guard.as_mut().expect("held")).expect("alone");
            node.config.peers.push(at.to_string());

            crate::run_round(
                node,
                &crate::targets(node),
                itsanas_net::session::Scope::Everything,
            );
            let listing = itsanas_store::catalogue(&node.store, &node.vault).expect("list");
            assert!(
                listing
                    .files
                    .iter()
                    .any(|file| file.path == "Photos/one.jpg"),
                "the next login does not see what the logged-out phone wrote: withdrawing \
                 a phone orphans its files. Listed: {:?}",
                listing
                    .files
                    .iter()
                    .map(|file| &file.path)
                    .collect::<Vec<_>>()
            );
            let out = dir.path().join("out");
            crate::get_file(node, "Photos/one.jpg", out.to_str().expect("path")).expect("get");
            assert_eq!(std::fs::read(&out).expect("read"), b"written on the phone");
        });
    });
}

/// What a logout would lose is said before it happens, and the check closes
/// nothing. Sabotage: report the coverage from before the round.
#[test]
fn a_phone_with_no_machine_is_told_its_files_are_only_here() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (mut node, _) =
        Node::create(&dir.path().join("node"), PASSPHRASE, "camille").expect("create");
    node.config.coordinator = Some("127.0.0.1:1".to_owned());
    let source = dir.path().join("source");
    std::fs::write(&source, b"the only copy").expect("source");
    crate::put_file(&node, "one.txt", source.to_str().expect("path")).expect("put");

    let said = answer(&logout_check(&node, true).expect("check"));
    assert_eq!(said["reached"], 0, "{said}");
    assert_eq!(said["username"], "camille", "{said}");
    assert!(said["onlyHere"].as_u64() >= Some(1), "{said}");
    assert_eq!(said["onlyHere"], said["liveChunks"], "{said}");
    assert_eq!(said["heldForOthers"], 0, "{said}");
}
