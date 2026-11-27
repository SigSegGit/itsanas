//! What stops being kept: silent addresses, retired names and escrow counts.

use super::*;

/// The directory's rows are held for as long as the file exists, so a row
/// nothing erases is an address that outlives its machine in every copy of
/// that file. The rule is the coordinator's own record of the silence, on
/// the same `>` as `PRESENCE_TTL`.
#[test]
fn red_team_an_address_silent_past_the_keep_is_erased_with_its_availability() {
    let (_dir, directory) = directory();
    let owner = user(70);
    register(&directory, "camille", &owner);
    let (gone, edge, here, withdrawn) = (device(71), device(72), device(73), device(74));
    for dev in [&gone, &edge, &here, &withdrawn] {
        enrol(&directory, &owner, dev, 1 << 30);
    }
    let later = NOW + KEPT + 1;
    announce_at(&directory, &gone, NOW, NOW);
    announce_at(&directory, &withdrawn, NOW, NOW);
    announce_at(&directory, &edge, NOW + 1, NOW + 1);
    announce_at(&directory, &here, later, later);
    let withdrawal = NodeClaim {
        owner: owner.user_id(),
        device: withdrawn.device_id(),
        pledged_bytes: 0,
        issued_unix: NOW + 1,
        revoked: true,
    }
    .sign(&owner);
    directory.claim(&withdrawal, NOW + 1).expect("withdraw");

    assert_eq!(
        directory.expire_presence(later, KEPT).unwrap(),
        2,
        "a pass must erase exactly the two devices silent past the keep"
    );
    for dev in [&gone, &withdrawn] {
        assert_eq!(
            directory.presence_of(dev.device_id()).unwrap(),
            None,
            "the address of a machine silent past the keep is still held, so every copy \
             of this file still says where its owner was"
        );
        assert_eq!(
            directory.last_seen(dev.device_id()).unwrap(),
            None,
            "the address went and the date it was last heard from stayed: the file still \
             says when the machine was last on"
        );
    }
    assert!(
        directory.presence_of(edge.device_id()).unwrap().is_some(),
        "a machine silent for exactly the keep lost its address a second too soon"
    );
    assert_eq!(
        directory.last_seen(edge.device_id()).unwrap(),
        Some(NOW + 1),
        "a machine inside the keep lost the date its owner's device list reads"
    );
    assert!(
        directory.presence_of(here.device_id()).unwrap().is_some(),
        "a machine heard from just now lost its address: its owner cannot be found"
    );
    assert_eq!(
        directory.live_claims_of(owner.user_id()).unwrap().len(),
        3,
        "erasing an address took a machine out of its account; a member would have to \
         enrol it again because it was switched off for a month"
    );
    assert_eq!(
        directory.expire_presence(later, KEPT).unwrap(),
        0,
        "a second pass found rows the first said it had erased"
    );
}

/// A VM can start with its clock far behind, and every row then looks dated
/// in the future. That says something about the clock and nothing about the
/// machines, and each of them would become unfindable until it next announced.
#[test]
fn red_team_a_clock_set_back_erases_no_address() {
    let (_dir, directory) = directory();
    let owner = user(75);
    register(&directory, "camille", &owner);
    let laptop = device(75);
    enrol(&directory, &owner, &laptop, 1 << 30);
    announce_at(&directory, &laptop, NOW, NOW);

    for clock in [0, 1, NOW - KEPT - 1, NOW - 1] {
        assert_eq!(
            directory.expire_presence(clock, KEPT).unwrap(),
            0,
            "a coordinator whose clock read {clock} erased a machine heard from at {NOW}; \
             every member becomes unfindable the day its clock is wrong"
        );
    }
    assert_eq!(
        directory.last_seen(laptop.device_id()).unwrap(),
        Some(NOW),
        "a clock set back erased the date its owner's device list reads"
    );
}

/// The date inside a presence is the announcing machine's opinion. A Pi
/// with no real-time clock says 1970 every hour, and judging by that erases
/// the address of the one machine that is always on.
#[test]
fn red_team_a_machine_with_no_real_time_clock_keeps_its_address() {
    let (_dir, directory) = directory();
    let owner = user(76);
    register(&directory, "camille", &owner);
    let pi = device(76);
    enrol(&directory, &owner, &pi, 1 << 30);
    announce_at(&directory, &pi, 0, NOW);

    assert_eq!(
        directory.expire_presence(NOW, KEPT).unwrap(),
        0,
        "a Pi that says 1970 and was heard just now lost its address at the first pass"
    );
    assert!(
        directory.presence_of(pi.device_id()).unwrap().is_some(),
        "the pass said it erased nothing and the Pi's address is gone"
    );
}

/// Every write keeps the two tables together, so these rows come only from a
/// damaged or hand-edited file. None may become the one address nothing
/// erases, and a presence's own date is the only one left to judge it by.
#[test]
fn a_presence_with_no_availability_record_is_judged_by_its_own_date() {
    let (_dir, directory) = directory();
    let owner = user(77);
    register(&directory, "camille", &owner);
    let (old, ahead) = (device(77), device(78));
    enrol(&directory, &owner, &old, 1 << 30);
    enrol(&directory, &owner, &ahead, 1 << 30);
    let later = NOW + KEPT + 1;
    announce_at(&directory, &old, NOW, NOW);
    announce_at(&directory, &ahead, later + 3600, later + 3600);
    let txn = directory.db.begin_write().unwrap();
    {
        let mut availability = txn.open_table(AVAILABILITY).unwrap();
        for dev in [&old, &ahead] {
            availability
                .remove(dev.device_id().to_bytes().as_slice())
                .unwrap();
        }
        txn.open_table(PRESENCE)
            .unwrap()
            .insert(
                device(99).device_id().to_bytes().as_slice(),
                b"x".as_slice(),
            )
            .unwrap();
    }
    txn.commit().unwrap();

    assert_eq!(
        directory.expire_presence(later, KEPT).unwrap(),
        2,
        "one row with no date of its own stopped the pass, or stayed for good"
    );
    assert_eq!(
        directory.presence_of(old.device_id()).unwrap(),
        None,
        "a presence with no availability row is never erased"
    );
    assert!(
        directory.presence_of(ahead.device_id()).unwrap().is_some(),
        "a row dated after this clock was erased: the clock is what is wrong"
    );
}

fn retired_names(directory: &Directory) -> Vec<String> {
    let txn = directory.db.begin_read().unwrap();
    let table = txn.open_table(RETIRED).unwrap();
    table
        .iter()
        .unwrap()
        .map(|row| row.unwrap().0.value().to_owned())
        .collect()
}

/// A retired name is what stops somebody appearing under a name a friend's
/// list knows. Purging one early hands that name out; never purging keeps
/// every deleted member's name in the file for good.
#[test]
fn red_team_a_retired_name_is_purged_when_its_year_is_over_and_not_before() {
    let (_dir, directory) = directory();
    let (early, late, stranger) = (user(79), user(80), user(81));
    register(&directory, "early", &early);
    register(&directory, "late", &late);
    directory
        .delete_account(&deletion(&early, "early", NOW), NOW)
        .expect("delete");
    directory
        .delete_account(&deletion(&late, "late", NOW + 10), NOW + 10)
        .expect("delete");
    let free_at = NOW + NAME_RETIRED_FOR;

    for clock in [0, NOW, free_at - 1] {
        assert_eq!(
            directory.expire_retired_names(clock).unwrap(),
            0,
            "a name was purged at {clock}, before its year was over"
        );
    }
    assert!(
        matches!(
            try_register(&directory, "early", &stranger, free_at - 1),
            Err(CoordError::Retired { .. })
        ),
        "a name came free before its year was over: a stranger appears under it in the \
         lists of people who knew its owner"
    );

    assert_eq!(
        directory.expire_retired_names(free_at).unwrap(),
        1,
        "the name of somebody who deleted their account stays in the file for good"
    );
    assert_eq!(
        retired_names(&directory),
        vec!["late".to_owned()],
        "a deleted member's name stayed in the file after its year, or a name still \
         retired went with it"
    );
    assert!(
        matches!(
            try_register(&directory, "late", &stranger, free_at),
            Err(CoordError::Retired { .. })
        ),
        "the name retired ten seconds later came free with the first"
    );
    assert!(
        matches!(
            try_register(&directory, "another", &early, free_at),
            Err(CoordError::Deleted)
        ),
        "the pass took a tombstone with the name: a deleted account's key can come back"
    );
    assert!(
        try_register(&directory, "early", &stranger, free_at).is_ok(),
        "a name purged at the end of its year could not be registered"
    );
}

fn escrow_counts(directory: &Directory) -> Vec<String> {
    let txn = directory.db.begin_read().unwrap();
    let table = txn.open_table(ESCROW_TRIES).unwrap();
    table
        .iter()
        .unwrap()
        .map(|row| row.unwrap().0.value().to_owned())
        .collect()
}

/// The count is the whole defence of the sealed blob, so a pass that cleared
/// a live one would be a fresh budget for a guesser, every day, on schedule.
#[test]
fn red_team_purging_escrow_counts_gives_a_guesser_no_fresh_budget() {
    const WINDOW: u64 = 60;
    let (_dir, directory) = directory();
    for _ in 0..3 {
        assert!(
            directory
                .count_escrow_attempt("victim", NOW, 3, WINDOW, 8)
                .unwrap()
        );
    }
    assert!(
        directory
            .count_escrow_attempt("invented", NOW - WINDOW, 3, WINDOW, 8)
            .unwrap()
    );
    let txn = directory.db.begin_write().unwrap();
    txn.open_table(ESCROW_TRIES)
        .unwrap()
        .insert("unreadable", b"x".as_slice())
        .unwrap();
    txn.commit().unwrap();

    assert_eq!(
        directory.expire_escrow_tries(NOW, WINDOW).unwrap(),
        2,
        "a name a stranger invented stays in the file for good once its window is over"
    );
    assert_eq!(
        escrow_counts(&directory),
        vec!["victim".to_owned()],
        "the pass kept the wrong names: a live count must stay and an old one go"
    );
    assert!(
        !directory
            .count_escrow_attempt("victim", NOW + 1, 3, WINDOW, 8)
            .unwrap(),
        "the pass cleared the count of a name being ground: a fresh budget of guesses"
    );

    assert_eq!(
        directory.expire_escrow_tries(NOW - 1000, WINDOW).unwrap(),
        0,
        "a clock set back reopened a window, which is a fresh budget of guesses"
    );
    assert_eq!(
        directory.expire_escrow_tries(NOW + WINDOW, WINDOW).unwrap(),
        1,
        "a count whose window is over stays in the file, with the name it counts"
    );
    assert_eq!(
        escrow_counts(&directory),
        Vec::<String>::new(),
        "the pass said it dropped the last count and the name is still there"
    );
    assert!(
        directory
            .count_escrow_attempt("victim", NOW + WINDOW, 3, WINDOW, 8)
            .unwrap(),
        "a name whose window was over and whose row was purged stayed refused"
    );
}

#[test]
fn an_account_that_goes_leaves_no_escrow_count_behind() {
    let (_dir, directory) = directory();
    let (forgotten, deleted) = (user(82), user(83));
    register(&directory, "forgotten", &forgotten);
    register(&directory, "deleted", &deleted);
    for name in ["forgotten", "deleted", "invented"] {
        assert!(directory.count_escrow_attempt(name, NOW, 3, 60, 8).unwrap());
    }

    directory.forget_account("forgotten").unwrap().unwrap();
    directory
        .delete_account(&deletion(&deleted, "deleted", NOW), NOW)
        .expect("delete");

    assert_eq!(
        escrow_counts(&directory),
        vec!["invented".to_owned()],
        "a name that left the directory stayed in its table of escrow counts"
    );
}
