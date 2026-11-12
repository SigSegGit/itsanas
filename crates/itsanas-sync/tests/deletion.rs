//! Deletes between machines with different rules (HANDOVER §8 13): the
//! account's trash, Protect, and rules that every machine reads alike.
//!
//! Deterministic, like `convergence.rs`: the same swarm, no clock but the
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
