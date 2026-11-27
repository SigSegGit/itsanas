//! Escrow: off until asked for, bounded in size, and a fetch budget that
//! survives a restart.

use super::*;

#[test]
fn red_team_a_coordinator_restart_does_not_hand_out_a_fresh_escrow_budget() {
    // HANDOVER §9: the counters were in memory only, so anybody who could
    // provoke a restart -- or wait for one -- ground the passphrase with a
    // fresh budget each time.
    let (dir, directory) = directory();
    for attempt in 1..=3 {
        assert!(
            directory
                .count_escrow_attempt("nicolas", NOW, 3, 60, 8)
                .unwrap(),
            "attempt {attempt} refused"
        );
    }
    let path = dir.path().join("coord.redb");
    drop(directory);
    let reopened = Directory::open(&path).expect("reopen");
    assert!(
        !reopened
            .count_escrow_attempt("nicolas", NOW + 1, 3, 60, 8)
            .unwrap(),
        "the restart cleared the count: a fourth guess got through"
    );
    assert!(
        reopened
            .count_escrow_attempt("nicolas", NOW + 61, 3, 60, 8)
            .unwrap(),
        "the window never reopened: a person who mistyped is locked out for good"
    );
}

#[test]
fn red_team_invented_names_cannot_evict_a_live_escrow_count() {
    let (_dir, directory) = directory();
    for _ in 0..3 {
        assert!(
            directory
                .count_escrow_attempt("victim", NOW, 3, 60, 4)
                .unwrap()
        );
    }
    for index in 0..50 {
        let _ = directory.count_escrow_attempt(&format!("invented-{index}"), NOW, 3, 60, 4);
    }
    assert!(
        !directory
            .count_escrow_attempt("victim", NOW, 3, 60, 4)
            .unwrap(),
        "a flood of invented names cleared the attacker's own count"
    );
    assert!(
        directory
            .count_escrow_attempt("someone", NOW + 61, 3, 60, 4)
            .unwrap(),
        "expired counts were never pruned: a full table refused everyone for good"
    );
}

#[test]
fn escrow_is_off_until_it_is_asked_for() {
    // Anyone who knows a username can fetch the blob, so its security is
    // exactly the passphrase. That is a decision to make deliberately.
    let (_dir, directory) = directory();
    let owner = user(15);
    register(&directory, "nicolas", &owner);

    directory.put_escrow(owner.user_id(), b"sealed").unwrap();
    assert_eq!(
        directory.escrow("nicolas").unwrap(),
        None,
        "an escrow blob was served for an account that never enabled it"
    );

    directory.set_escrow_enabled(owner.user_id(), true).unwrap();
    assert_eq!(directory.escrow("nicolas").unwrap().unwrap(), b"sealed");
}

#[test]
fn an_oversized_escrow_blob_is_refused() {
    let (_dir, directory) = directory();
    let owner = user(16);
    register(&directory, "nicolas", &owner);

    assert!(
        directory
            .put_escrow(owner.user_id(), &vec![0u8; MAX_ESCROW_LEN + 1])
            .is_err(),
        "the directory can be used as free storage"
    );
}

#[test]
fn escrow_for_an_unknown_username_is_absent_rather_than_an_error() {
    // A recovery attempt with a typo should say "no such account", not leak
    // whether the name exists through a different error shape.
    let (_dir, directory) = directory();
    assert_eq!(directory.escrow("nobody").unwrap(), None);
}
