//! Deletes and renames of files only listed here: they reach the account,
//! and never lose an edit they did not see.

use super::*;

/// A phone lists everything and holds little, so most of what it shows can only
/// be deleted this way. Sabotage: start the tombstone from an empty version, or
/// log nothing.
#[test]
fn red_team_a_file_only_listed_here_can_be_deleted_from_the_account() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    laptop
        .store
        .write_file("old.jpg", b"a photo")
        .expect("write");
    laptop.store.flush_segment().expect("flush");

    pull_listing(&phone, &laptop);
    assert_eq!(
        phone.store.list().expect("list").len(),
        0,
        "the setup holds the file"
    );
    assert!(
        itsanas_store::remove_known(&phone.store, &phone.vault, "old.jpg").expect("remove"),
        "a file on screen could not be deleted: it stays in the account for ever"
    );
    phone.store.flush_segment().expect("flush");
    assert!(
        itsanas_store::catalogue(&phone.store, &phone.vault)
            .expect("catalogue")
            .files
            .is_empty(),
        "the deleted file is still listed on the phone that deleted it"
    );

    pull_all(&laptop, &phone);
    assert_eq!(
        laptop.store.read_file("old.jpg").expect("read"),
        None,
        "the deletion never reached the machine that holds the file"
    );
}

/// ARCHITECTURE: a concurrent delete loses to an edit, here too. Sabotage: make
/// the tombstone beat everything.
#[test]
fn red_team_a_delete_of_a_listed_file_loses_to_an_edit_it_did_not_see() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    laptop
        .store
        .write_file("essay.txt", b"first")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    pull_listing(&phone, &laptop);

    // Edited on the laptop, unseen by the phone, which then deletes what it saw.
    laptop
        .store
        .write_file("essay.txt", b"second")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    assert!(itsanas_store::remove_known(&phone.store, &phone.vault, "essay.txt").expect("remove"));
    phone.store.flush_segment().expect("flush");

    pull_all(&laptop, &phone);
    assert_eq!(
        laptop.store.read_file("essay.txt").expect("read"),
        Some(b"second".to_vec()),
        "a delete that never saw the edit destroyed it: a lost edit cannot be undone"
    );
    pull_listing(&phone, &laptop);
    let listed = itsanas_store::catalogue(&phone.store, &phone.vault)
        .expect("catalogue")
        .files;
    assert_eq!(
        listed.len(),
        1,
        "the edit that won the race is hidden on the phone: its owner cannot see or open it"
    );
}

/// `itsanas mv` on a phone, which lists the account and holds none of it: a
/// folder renamed there arrives on the laptop under its new name, opens there,
/// and the old names are gone. Sabotage: write no delete of the old name (the
/// laptop shows both); or no upsert of the new one (the files vanish).
#[test]
fn red_team_a_folder_renamed_on_a_phone_that_holds_nothing_moves_on_the_laptop() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    laptop
        .store
        .write_file("photos/a.jpg", b"the first photograph")
        .expect("write");
    laptop
        .store
        .write_file("photos/b.jpg", b"the second photograph")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    pull_listing(&phone, &laptop);

    assert_eq!(
        itsanas_store::rename_known(&phone.store, &phone.vault, "photos", "2024/photos")
            .expect("rename"),
        itsanas_store::Renamed::Moved(2)
    );
    let on_phone: Vec<String> = itsanas_store::catalogue(&phone.store, &phone.vault)
        .expect("catalogue")
        .files
        .into_iter()
        .map(|known| known.path)
        .collect();
    assert_eq!(
        on_phone,
        ["2024/photos/a.jpg", "2024/photos/b.jpg"],
        "the phone that renamed the folder does not list it at its new place"
    );

    pull_all(&laptop, &phone);
    assert_eq!(
        laptop.store.read_file("2024/photos/a.jpg").expect("read"),
        Some(b"the first photograph".to_vec()),
        "a file renamed on the phone does not open under its new name on the laptop"
    );
    assert_eq!(
        laptop.store.list().expect("list"),
        ["2024/photos/a.jpg", "2024/photos/b.jpg"],
        "the laptop keeps the old names beside the new: every rename doubles a folder"
    );
}

/// A rename that met an edit it never saw keeps the edit: the old name comes
/// back with it, the new name has what was renamed. Two files where one was
/// meant costs a second; an edit lost cannot be undone. The rule is
/// `remove_unheld`'s, sabotaged in
/// `red_team_a_delete_of_a_listed_file_loses_to_an_edit_it_did_not_see`.
#[test]
fn a_rename_does_not_lose_an_edit_it_did_not_see() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    laptop
        .store
        .write_file("essay.txt", b"first")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    pull_listing(&phone, &laptop);
    laptop
        .store
        .write_file("essay.txt", b"second")
        .expect("write");
    laptop.store.flush_segment().expect("flush");

    itsanas_store::rename_known(&phone.store, &phone.vault, "essay.txt", "final.txt")
        .expect("rename");
    pull_all(&laptop, &phone);
    assert_eq!(
        laptop.store.read_file("essay.txt").expect("read"),
        Some(b"second".to_vec()),
        "a rename made on a phone that never saw the edit destroyed it"
    );
    assert_eq!(
        laptop.store.read_file("final.txt").expect("read"),
        Some(b"first".to_vec()),
        "the renamed file is not under its new name"
    );
}

/// A rename never replaces a file, never touches the account's own files, and
/// keeps the file's date: it is the same file. Sabotage: drop the taken check
/// (b.txt is replaced and its content lost); or write the date of the rename.
#[test]
fn a_rename_onto_a_taken_name_moves_nothing_and_a_rename_keeps_the_date() {
    let master = alice();
    let laptop = node(&master, 1);
    laptop.store.write_file("a.txt", b"a").expect("write");
    laptop.store.write_file("b.txt", b"b").expect("write");
    laptop.store.flush_segment().expect("flush");

    assert_eq!(
        itsanas_store::rename_known(&laptop.store, &laptop.vault, "a.txt", "b.txt")
            .expect("rename"),
        itsanas_store::Renamed::Taken("b.txt".to_owned())
    );
    assert_eq!(
        laptop.store.read_file("b.txt").expect("read"),
        Some(b"b".to_vec()),
        "a rename replaced a file that was there: its content is lost without a word"
    );
    assert_eq!(
        itsanas_store::rename_known(&laptop.store, &laptop.vault, "nothing", "else")
            .expect("rename"),
        itsanas_store::Renamed::NoSuchFile
    );
    assert!(
        itsanas_store::rename_known(&laptop.store, &laptop.vault, "a.txt", ".itsanas/a.txt")
            .is_err(),
        "a file was moved among the account's own files, where no listing shows it"
    );
    for outside in ["../a.txt", "/a.txt", "notes/../../a.txt"] {
        assert!(
            itsanas_store::rename_known(&laptop.store, &laptop.vault, "a.txt", outside).is_err(),
            "a rename to {outside} was accepted: a name that climbs out of the account"
        );
        assert!(
            itsanas_store::versions(&laptop.store, &laptop.vault, outside).is_err(),
            "versions of {outside} was answered"
        );
    }

    let before = laptop.store.stat("a.txt").expect("stat").expect("a.txt");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    itsanas_store::rename_known(&laptop.store, &laptop.vault, "a.txt", "notes/a.txt")
        .expect("rename");
    let after = laptop
        .store
        .stat("notes/a.txt")
        .expect("stat")
        .expect("renamed");
    assert_eq!(
        after.modified_unix, before.modified_unix,
        "a rename dated the file as edited now: sorting by date puts an old file first"
    );
    assert_eq!(laptop.store.stat("a.txt").expect("stat"), None);
}

#[test]
fn deleting_what_the_account_does_not_have_deletes_nothing() {
    let master = alice();
    let phone = node(&master, 2);
    assert!(!itsanas_store::remove_known(&phone.store, &phone.vault, "never.txt").expect("remove"));
    assert_eq!(phone.store.tombstone("never.txt").expect("tombstone"), None);
}

#[test]
fn a_file_deleted_elsewhere_is_never_offered_for_download() {
    // A phone that lists a file deleted a week ago, and downloads it when
    // tapped, has resurrected it. The catalogue reads the log's last word,
    // which is the delete.
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);

    laptop
        .store
        .write_file("gone.txt", b"temporary")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    laptop.store.remove_file("gone.txt").expect("remove");
    laptop.store.flush_segment().expect("flush");

    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .expect("metadata pull");
    });

    assert!(
        itsanas_store::catalogue(&phone.store, &phone.vault)
            .expect("catalogue")
            .files
            .is_empty(),
        "a deleted file was offered for download"
    );
}

#[test]
fn a_delete_racing_an_edit_still_leaves_the_file_listed() {
    // The asymmetry the whole merge design rests on: a delete concurrent with
    // an edit loses, because an unexpected file costs a second and a lost edit
    // is unrecoverable. A listing that applied the opposite rule would hide a
    // file the merge engine is about to keep — and the person looking at the
    // phone would conclude their edit was lost.
    let master = alice();
    let laptop = node(&master, 1);
    let pi = node(&master, 2);
    let phone = node(&master, 3);

    laptop.store.write_file("doc.txt", b"base").expect("write");
    laptop.store.flush_segment().expect("flush");

    // The Pi learns about the file, then the two lose sight of each other.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &pi.device, pi.store.owner(), None).expect("dial");
        session::round(&pi.store, &pi.vault, &mut client).expect("round");
    });
    assert!(pi.store.read_file("doc.txt").expect("read").is_some());

    // Apart: one edits, the other deletes.
    laptop.store.write_file("doc.txt", b"edited").expect("edit");
    laptop.store.flush_segment().expect("flush");
    pi.store.remove_file("doc.txt").expect("delete");
    pi.store.flush_segment().expect("flush");

    // The phone hears both sides and downloads nothing.
    for host in [&laptop, &pi] {
        with_server(host, Pledge { bytes: 1 << 30 }, |address| {
            let mut client = PeerClient::connect(address, &phone.device, phone.store.owner(), None)
                .expect("dial");
            session::pull_scoped(
                &phone.store,
                &phone.vault,
                &mut client,
                session::Scope::Metadata,
            )
            .expect("metadata pull");
        });
    }

    let known = itsanas_store::catalogue(&phone.store, &phone.vault)
        .expect("catalogue")
        .files;
    assert_eq!(
        known.len(),
        1,
        "the edit lost to a concurrent delete in the listing, got {known:?}"
    );
    assert_eq!(known[0].path, "doc.txt");
    assert_eq!(known[0].presence, itsanas_store::Presence::Absent);
}
