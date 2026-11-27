//! Deletes between machines with different rules (HANDOVER §8 13): the
//! account's trash, Protect, and rules that every machine reads alike.
//!
//! Deterministic, like `convergence/`: the same swarm, no clock but the
//! store's.

use itsanas_store::deletion::{self, Mode, Receive, Rules};
use itsanas_sync::sim::Swarm;

const PHONE: usize = 0;
const PC: usize = 1;

fn swarm() -> Swarm {
    Swarm::new(3).expect("a three-device swarm")
}

fn now() -> u64 {
    itsanas_store::now_unix()
}

/// A file in the account's trash is on every machine's trash list, still a file
/// of the account, and a restore from the other machine brings it back for
/// both. Sabotage: drop the marker write in `deletion::trash_shared`.
#[test]
fn red_team_a_file_trashed_on_one_machine_is_restored_from_another() {
    let swarm = swarm();
    swarm.device(PHONE).write("photo.jpg", b"pixels").unwrap();
    swarm.settle_and_check().unwrap();

    deletion::trash_shared(swarm.device(PHONE).store(), "photo.jpg", now(), 30).unwrap();
    swarm.settle_and_check().unwrap();
    assert!(
        deletion::shared_known_here(swarm.device(PC).store())
            .unwrap()
            .contains_key("photo.jpg"),
        "the computer does not see the file the phone put in the account's trash"
    );
    assert_eq!(
        swarm.device(PC).read("photo.jpg").unwrap().as_deref(),
        Some(&b"pixels"[..]),
        "a trashed file left the account before anybody emptied the trash"
    );

    deletion::restore_shared_known(swarm.device(PC).store(), "photo.jpg").unwrap();
    swarm.device(PC).store().flush_segment().unwrap();
    swarm.settle_and_check().unwrap();
    assert!(
        deletion::shared_known_here(swarm.device(PHONE).store())
            .unwrap()
            .is_empty(),
        "a restore on the computer left the file in the phone's trash"
    );
}

/// A file edited on another machine after it was trashed is that machine's
/// newer file: it leaves the trash rather than being purged with it, and an
/// untouched trashed file stays in. Sabotage: `deletion::edited` keeps nothing.
#[test]
fn red_team_an_edit_made_elsewhere_takes_a_trashed_file_out_of_the_trash() {
    let swarm = swarm();
    swarm
        .device(PHONE)
        .write("notes.txt", b"first draft")
        .unwrap();
    swarm
        .device(PHONE)
        .write("old.txt", b"nobody wants this")
        .unwrap();
    swarm.settle_and_check().unwrap();

    let phone = swarm.device(PHONE).store();
    deletion::trash_shared(phone, "notes.txt", now(), 30).unwrap();
    deletion::trash_shared(phone, "old.txt", now(), 30).unwrap();
    phone.flush_segment().unwrap();
    // The computer, not yet told, edits the note; then both learn of each other.
    swarm
        .device(PC)
        .write("notes.txt", b"second draft, much longer")
        .unwrap();
    swarm.settle_and_check().unwrap();

    let taken = deletion::untrash_edited_known(phone).unwrap();
    phone.flush_segment().unwrap();
    swarm.settle_and_check().unwrap();
    assert_eq!(
        taken,
        ["notes.txt"],
        "an edit made on the computer stayed in the phone's trash: the purge would delete it"
    );
    let left = deletion::shared_known_here(swarm.device(PC).store()).unwrap();
    assert!(
        !left.contains_key("notes.txt"),
        "the edited note is still in the account's trash on the computer"
    );
    assert!(
        left.contains_key("old.txt"),
        "a file nobody touched left the trash: deleting it did nothing"
    );
    assert_eq!(
        swarm.device(PHONE).read("notes.txt").unwrap().as_deref(),
        Some(&b"second draft, much longer"[..]),
        "the edit is not the file the account keeps"
    );
}

/// A machine that protects keeps, for its days, what another deleted outright,
/// and publishing it again brings it back on both. Sabotage: return early in
/// `deletion::protect`.
#[test]
fn red_team_a_direct_delete_elsewhere_is_kept_by_a_machine_that_protects() {
    let swarm = swarm();
    swarm
        .device(PHONE)
        .write("screenshot.png", b"kept by the pc")
        .unwrap();
    swarm.settle_and_check().unwrap();

    swarm.device(PHONE).remove("screenshot.png").unwrap();
    swarm.settle_and_check().unwrap();
    let pc = swarm.device(PC).store();
    assert!(
        deletion::protected(pc.root()).contains_key("screenshot.png"),
        "the computer kept nothing of a file the phone deleted outright"
    );

    assert!(deletion::restore_protected(pc, "screenshot.png").unwrap());
    swarm.settle_and_check().unwrap();
    assert_eq!(
        swarm
            .device(PHONE)
            .read("screenshot.png")
            .unwrap()
            .as_deref(),
        Some(&b"kept by the pc"[..]),
        "a protected file published again did not come back on the phone"
    );
}

/// A machine set to apply keeps nothing, and a file deleted out of the
/// account's trash is protected by nobody: it had its days already.
/// Sabotage: drop the `trashed` check in `deletion::protect`.
#[test]
fn red_team_nothing_is_protected_out_of_the_trash_or_on_a_machine_that_applies() {
    let swarm = swarm();
    for path in ["emptied.txt", "applied.txt"] {
        swarm.device(PHONE).write(path, b"bytes").unwrap();
    }
    swarm.settle_and_check().unwrap();

    let phone = swarm.device(PHONE).store();
    deletion::trash_shared(phone, "emptied.txt", now(), 30).unwrap();
    phone.flush_segment().unwrap();
    swarm.settle_and_check().unwrap();
    // Out of the trash for good: the tombstone first, then the marker.
    phone.remove_file("emptied.txt").unwrap();
    phone.flush_segment().unwrap();
    deletion::restore_shared_known(phone, "emptied.txt").unwrap();
    phone.flush_segment().unwrap();
    swarm.settle_and_check().unwrap();
    assert!(
        !deletion::protected(swarm.device(PC).store().root()).contains_key("emptied.txt"),
        "a file emptied from the trash was kept again: the trash never really empties"
    );

    deletion::set_rules_known(
        swarm.device(PC).store(),
        Some(&Rules {
            receive: Receive::Apply,
            ..Rules::default()
        }),
    )
    .unwrap();
    swarm.device(PHONE).remove("applied.txt").unwrap();
    swarm.settle_and_check().unwrap();
    assert!(
        !deletion::protected(swarm.device(PC).store().root()).contains_key("applied.txt"),
        "a machine set to apply still kept a deleted file"
    );
}

/// A machine whose index holds no trash marker (a phone that downloads
/// nothing) still knows from its noted trash that a delete emptied the trash,
/// and keeps nothing. Sabotage: drop the noted trash from `deletion::protect`.
#[test]
fn red_team_a_machine_without_the_marker_still_keeps_nothing_emptied_from_the_trash() {
    let swarm = swarm();
    swarm.device(PHONE).write("emptied.txt", b"bytes").unwrap();
    swarm.settle_and_check().unwrap();
    let pc = swarm.device(PC).store();
    deletion::note_shared(
        pc.root(),
        &std::collections::BTreeMap::from([("emptied.txt".to_owned(), now())]),
    )
    .unwrap();
    swarm.device(PHONE).remove("emptied.txt").unwrap();
    swarm.settle_and_check().unwrap();
    assert!(
        !deletion::protected(pc.root()).contains_key("emptied.txt"),
        "a file emptied from the trash was kept for days by a machine that held no marker"
    );
}

/// A file restored from the account's trash and then deleted outright is a
/// delete like any other, and a machine that protects keeps it. A machine
/// that downloads nothing reads the trash it last noted: until its next note,
/// that note still named the restored file, and the outright delete read as
/// the trash being emptied, so nothing was kept (§8 13, "Left"). Sabotage:
/// leave the note alone when a marker goes while its file stays.
#[test]
fn red_team_a_file_restored_then_deleted_outright_is_protected_before_the_next_note() {
    let swarm = swarm();
    swarm.device(PHONE).write("lettre.txt", b"words").unwrap();
    swarm.settle_and_check().unwrap();
    let phone = swarm.device(PHONE).store();
    let due = now() + 30 * 24 * 3600;
    deletion::trash_shared(phone, "lettre.txt", now(), 30).unwrap();
    swarm.settle_and_check().unwrap();
    let pc = swarm.device(PC).store();
    deletion::note_shared(
        pc.root(),
        &std::collections::BTreeMap::from([("lettre.txt".to_owned(), due)]),
    )
    .unwrap();

    deletion::restore_shared_known(phone, "lettre.txt").unwrap();
    phone.flush_segment().unwrap();
    swarm.settle_and_check().unwrap();
    assert!(
        !deletion::noted_shared(pc.root()).contains_key("lettre.txt"),
        "a file restored elsewhere is still noted as in the trash here"
    );
    swarm.device(PHONE).remove("lettre.txt").unwrap();
    swarm.settle_and_check().unwrap();
    assert!(
        deletion::protected(pc.root()).contains_key("lettre.txt"),
        "a file restored and then deleted outright was let go: the stale note read it as emptied from the trash"
    );
}

/// How long traces are kept is the account's choice, not a machine's: one
/// machine forgetting earlier is enough for a machine that was off to bring a
/// deleted file back through it. Set on the phone, the computer reads it; two
/// choices made apart read as the one that keeps more, on both. Sabotage:
/// read the first choice instead of the most protective.
#[test]
fn red_team_how_long_traces_are_kept_is_one_choice_for_the_whole_account() {
    use itsanas_store::traces::{self, Keep};
    let swarm = swarm();
    let (phone, pc) = (swarm.device(PHONE).store(), swarm.device(PC).store());
    assert_eq!(
        traces::known_here(pc).unwrap(),
        Keep::For(365),
        "the default is not a year"
    );

    traces::set_known(phone, Keep::For(30)).unwrap();
    swarm.settle_and_check().unwrap();
    assert_eq!(
        traces::known_here(pc).unwrap(),
        Keep::For(30),
        "a choice made on the phone did not reach the computer"
    );

    traces::set_known(pc, Keep::Forever).unwrap();
    traces::set_known(phone, Keep::For(10)).unwrap();
    swarm.settle_and_check().unwrap();
    for (name, store) in [("phone", phone), ("computer", pc)] {
        assert_eq!(
            traces::known_here(store).unwrap(),
            Keep::Forever,
            "the {name} read a shorter choice made apart over keeping forever"
        );
    }
    assert!(
        swarm
            .device(PC)
            .store()
            .list()
            .unwrap()
            .iter()
            .all(|path| !path.starts_with(traces::PREFIX)
                || path.ends_with("never")
                || path.ends_with("10")),
        "a choice left a file nobody reads"
    );
}

/// A trace goes once it is older than the account keeps, at most once a day,
/// and never while the account keeps them for ever. Sabotage: sweep whatever
/// the date of the last sweep; or sweep under `Forever`.
#[test]
fn red_team_traces_go_after_their_days_once_a_day_and_never_when_kept_for_ever() {
    use itsanas_store::traces::{self, Keep};
    let swarm = swarm();
    let dir = tempfile::tempdir().expect("vault dir");
    let vault = itsanas_store::Vault::open(dir.path().join("vault")).expect("vault");
    let phone = swarm.device(PHONE).store();
    swarm.device(PHONE).write("old.txt", b"bytes").unwrap();
    swarm.device(PHONE).remove("old.txt").unwrap();
    phone.flush_segment().unwrap();
    let day = 24 * 3600;

    traces::set_known(phone, Keep::Forever).unwrap();
    let later = now() + 2000 * day;
    assert_eq!(
        traces::sweep(phone, &vault, later).unwrap(),
        0,
        "a trace went while the account keeps them for ever"
    );

    traces::set_known(phone, Keep::For(10)).unwrap();
    // Two traces: the file's, and the choice `never` the new choice replaced.
    assert_eq!(
        traces::sweep(phone, &vault, later + day).unwrap(),
        2,
        "a trace older than the account keeps stayed"
    );
    swarm.device(PHONE).write("old2.txt", b"bytes").unwrap();
    swarm.device(PHONE).remove("old2.txt").unwrap();
    phone.flush_segment().unwrap();
    assert_eq!(
        traces::sweep(phone, &vault, later + day + 3600).unwrap(),
        0,
        "a second sweep the same day ran: every round would walk every trace"
    );
}

/// Each machine's rules are read the same everywhere, two changes made apart
/// settle on the one that keeps more, and "apply to all" leaves one set.
/// Sabotage: make `rules_under` take the first record instead of the most
/// protective.
#[test]
fn red_team_rules_are_read_alike_everywhere_and_a_tie_keeps_more() {
    let swarm = swarm();
    let phone = swarm.device(PHONE).store();
    let local = Rules {
        mode: Mode::Local,
        ..Rules::default()
    };
    deletion::set_rules_known(phone, Some(&local)).unwrap();
    swarm.settle_and_check().unwrap();
    let pc = swarm.device(PC).store();
    let phone_id = phone.device_id().to_hex();
    assert_eq!(
        deletion::rules_known_here(pc).unwrap(),
        Rules::default(),
        "the phone's own rules changed the computer's"
    );
    let reserved: Vec<String> = pc
        .list()
        .unwrap()
        .into_iter()
        .filter(|path| path.contains(&phone_id))
        .collect();
    assert_eq!(
        reserved,
        [format!(".itsanas/machines/{phone_id}/{}", local.to_word())],
        "the computer does not know the phone's rules"
    );

    // Two account defaults written apart: both machines read the one that keeps more.
    let loose = Rules {
        mode: Mode::Direct,
        receive: Receive::Apply,
        ..Rules::default()
    };
    phone
        .write_file(&format!(".itsanas/account/{}", loose.to_word()), b"")
        .unwrap();
    pc.write_file(
        &format!(".itsanas/account/{}", Rules::default().to_word()),
        b"",
    )
    .unwrap();
    swarm.settle_and_check().unwrap();
    deletion::set_rules_known(phone, None).unwrap();
    swarm.settle_and_check().unwrap();
    for device in [PHONE, PC] {
        assert_eq!(
            deletion::rules_known_here(swarm.device(device).store()).unwrap(),
            Rules::default(),
            "device {device} read the looser of two defaults written apart"
        );
    }
}
