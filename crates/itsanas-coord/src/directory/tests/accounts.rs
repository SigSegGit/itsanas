//! Registering, forgetting and deleting an account, and the names that
//! stay retired.

use super::*;

#[test]
fn a_registration_round_trips() {
    let (_dir, directory) = directory();
    let owner = user(1);

    let account = register(&directory, "nicolas", &owner);
    assert_eq!(account.user.id, owner.user_id());
    assert!(!account.escrow_enabled, "escrow must be off by default");

    assert_eq!(directory.account("nicolas").unwrap(), Some(account.clone()));
    assert_eq!(
        directory.account_of(owner.user_id()).unwrap(),
        Some(account)
    );
}

#[test]
fn a_username_cannot_be_taken_over_by_another_key() {
    // Usernames are what members type when they mean a particular person.
    // A name that can change hands is a name that can impersonate.
    let (_dir, directory) = directory();
    register(&directory, "nicolas", &user(1));

    let impostor = user(2);
    let signed = Registration {
        username: "nicolas".to_owned(),
        user: impostor.public(),
        issued_unix: NOW,
    }
    .sign(&impostor);

    assert!(matches!(
        directory.register(&signed, NOW),
        Err(CoordError::NameTaken(_))
    ));
    assert_eq!(
        directory.account("nicolas").unwrap().unwrap().user.id,
        user(1).user_id()
    );
}

#[test]
fn a_forgotten_account_frees_its_name_and_its_live_devices() {
    // A test account must not hold its name for ever: forgetting it lets
    // the name be registered under another key, and leaves nothing of
    // its live devices behind to be handed out as addresses or claims.
    let (_dir, directory) = directory();
    let old = user(50);
    register(&directory, "partner", &old);
    directory
        .claim(&signed(&old, 1, NOW, false), NOW)
        .expect("claim");
    directory
        .put_escrow(old.user_id(), b"sealed")
        .expect("escrow");

    let forgotten = directory.forget_account("partner").unwrap().unwrap();
    assert_eq!((forgotten.devices, forgotten.withdrawals_kept), (1, 0));
    assert!(directory.account_of(old.user_id()).unwrap().is_none());
    assert_eq!(directory.live_claims_of(old.user_id()).unwrap().len(), 0);
    assert!(
        directory
            .claim_for(device(1).device_id())
            .unwrap()
            .is_none()
    );
    assert!(directory.escrow("partner").unwrap().is_none());

    register(&directory, "partner", &user(51));
    assert_eq!(
        directory.account("partner").unwrap().unwrap().user.id,
        user(51).user_id(),
        "the name must be free for a new key once forgotten"
    );
    assert!(directory.forget_account("nobody").unwrap().is_none());
}

#[test]
fn red_team_forgetting_an_account_keeps_its_withdrawals_final() {
    // A withdrawal is final for its device id (§6). If forgetting erased
    // it, the same keys registering again would bring a stolen, withdrawn
    // machine back with a fresh live claim.
    let (_dir, directory) = directory();
    let owner = user(52);
    register(&directory, "gone", &owner);
    directory
        .claim(&signed(&owner, 2, NOW, false), NOW)
        .expect("claim");
    directory
        .claim(&signed(&owner, 2, NOW + 1, true), NOW + 1)
        .expect("withdraw");

    let forgotten = directory.forget_account("gone").unwrap().unwrap();
    assert_eq!(forgotten.withdrawals_kept, 1);

    register(&directory, "gone", &owner);
    let back = directory.claim(&signed(&owner, 2, NOW + 2, false), NOW + 2);
    assert!(
        !matches!(back, Ok(true)),
        "a withdrawn device came back after its account was forgotten"
    );
    assert!(
        directory
            .claim_for(device(2).device_id())
            .unwrap()
            .unwrap()
            .claim
            .revoked
    );
}

#[test]
fn re_registering_with_the_same_key_cannot_reset_the_joining_date() {
    // Otherwise the joining allowance is renewable forever by
    // re-registering, and it stops being an allowance.
    let (_dir, directory) = directory();
    let owner = user(3);
    register(&directory, "nicolas", &owner);

    let later = Registration {
        username: "nicolas".to_owned(),
        user: owner.public(),
        issued_unix: NOW + 999_999,
    }
    .sign(&owner);
    let account = directory.register(&later, NOW + 999_999).unwrap();

    assert_eq!(account.registered_unix, NOW);
}

#[test]
fn red_team_a_second_username_cannot_renew_the_joining_allowance() {
    // The hole the test above could not reach, and it is the same shape as
    // the freshness guard that lived in one branch of three.
    //
    // `register_admitted` answered two questions from two tables:
    // "has this key been here before?" from BY_ID, and "does this account
    // exist?" from ACCOUNTS keyed by *name*. They agree until one key asks
    // for a second name. Then the key counts as returning -- so no
    // invitation is demanded -- and the name is unknown, so the branch that
    // preserves `registered_unix` is skipped and a fresh account is minted
    // with today's date. BY_ID is then repointed at it, so `account_of`
    // reports the new date from that moment.
    //
    // One signed message every thirty days turned a bounded joining
    // allowance -- 10 GiB regardless of what you pledge -- into a permanent
    // free tier. That is the "grand profiteur" the accounting exists to
    // stop, and it cost an attacker one round trip a month.
    //
    // The two sibling tests covered (same key, same name) and (different
    // key, same name). Nobody wrote (same key, different name), and the
    // catalogue recorded the property as established.
    let (_dir, directory) = directory();
    let owner = user(9);
    register(&directory, "alice", &owner);

    let after = NOW + crate::accounting::JOINING_PERIOD_SECONDS + 1;
    let second = Registration {
        username: "alice2".to_owned(),
        user: owner.public(),
        issued_unix: after,
    }
    .sign(&owner);

    let minted = directory.register(&second, after);
    assert!(
        minted.is_err(),
        "one key took a second username, and with it a fresh joining date"
    );

    // The record of when this member joined is the input to the whole
    // allowance, so assert on it directly rather than on the refusal alone.
    let held = directory
        .account_of(owner.user_id())
        .expect("read")
        .expect("the account is still there");
    assert_eq!(held.username, "alice");
    assert_eq!(held.registered_unix, NOW);
}

#[test]
fn red_team_one_admitted_key_cannot_mint_accounts_on_an_invite_only_coordinator() {
    // The same defect on its other axis. `needs_invitation` is false for a
    // key that already has an account, which is right for somebody
    // re-registering the name they hold and wrong for anything else: an
    // admitted member could open unlimited further accounts without ever
    // presenting an invitation, and usernames here are bound to a key for
    // ever with no release path. One member could squat every short name on
    // the coordinator.
    let (_dir, directory) = directory();
    let founder = user(10);
    found(&directory, &founder, "alice", NOW).expect("the founder");

    let more = join(&directory, &founder, "alice-again", None, NOW + 10);
    assert!(
        more.is_err(),
        "an admitted key opened a second account with no invitation"
    );
}

#[test]
fn a_registration_signed_by_someone_else_is_refused() {
    let (_dir, directory) = directory();
    let victim = user(4);
    let attacker = user(5);

    let mut forged = Registration {
        username: "victim".to_owned(),
        user: victim.public(),
        issued_unix: NOW,
    }
    .sign(&attacker);
    forged.registration.user = victim.public();

    assert!(directory.register(&forged, NOW).is_err());
}

#[test]
fn usernames_are_narrow_on_purpose() {
    // A directory is read out loud and typed back in. Anything that can
    // look like something else is a problem.
    for good in ["nicolas", "pi-4", "a.b.c", "user123"] {
        validate_username(good).unwrap_or_else(|e| panic!("{good:?} rejected: {e}"));
    }
    for bad in [
        "",
        "Nicolas",  // mixed case invites two near-identical accounts
        "nicolas ", // trailing space
        "nicolàs",  // non-ASCII invites homoglyphs
        "-nicolas",
        "nicolas.",
        "under_score",
        &"a".repeat(MAX_USERNAME_LEN + 1),
    ] {
        assert!(validate_username(bad).is_err(), "{bad:?} was accepted");
    }
}

/// THE ATTACK: on an invite-only coordinator, ask to join under each name
/// of a list, with no code or a made-up one, and read the refusals. When
/// the name was looked at before the code, "taken" and "retired until..."
/// answered for every account that exists or once did, to anybody.
#[test]
fn red_team_without_a_valid_code_a_taken_a_retired_and_a_free_name_read_the_same() {
    let (_dir, directory) = directory();
    register(&directory, "prise", &user(71));
    register(&directory, "partie", &user(72));
    directory
        .delete_account(&deletion(&user(72), "partie", NOW), NOW)
        .expect("delete");
    let made_up = [0x5Au8; SECRET_LEN];

    for secret in [None, Some(&made_up)] {
        let refusals: Vec<String> = ["prise", "partie", "libre"]
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let seed = u8::try_from(80 + index).unwrap_or(80);
                join(&directory, &user(seed), name, secret, NOW + 1)
                    .expect_err("a stranger with no valid code was let in")
                    .to_string()
            })
            .collect();
        assert!(
            refusals.windows(2).all(|pair| pair[0] == pair[1]),
            "a stranger with {} told a taken name, a retired one and a free one apart: {:?}",
            if secret.is_some() {
                "a made-up code"
            } else {
                "no code"
            },
            refusals
        );
    }
    assert!(
        directory.account("libre").unwrap().is_none(),
        "a refused join wrote the account anyway"
    );
}

#[test]
fn red_team_a_deleted_name_stays_retired_a_year_and_its_old_key_never_returns() {
    let (_dir, directory) = directory();
    let first = user(61);
    let stranger = user(62);
    register(&directory, "nicolas", &first);
    directory
        .delete_account(&deletion(&first, "nicolas", NOW), NOW)
        .expect("delete");
    assert!(directory.account("nicolas").unwrap().is_none());

    assert!(
        matches!(
            try_register(&directory, "nicolas", &stranger, NOW + 1),
            Err(CoordError::Retired { .. })
        ),
        "a deleted name was free at once: a stranger would appear as nicolas in the lists of \
         people who knew him, and create/delete in a loop would cycle the name at will"
    );
    assert!(
        matches!(
            try_register(&directory, "another", &first, NOW + 1),
            Err(CoordError::Deleted)
        ),
        "a deleted account's key registered again: its hosts, told it was deleted, would \
         hold its chunks for a key that is back"
    );
    assert!(
        try_register(&directory, "nicolas", &stranger, NOW + NAME_RETIRED_FOR + 1).is_ok(),
        "a retired name never came back, so a deletion took it from everyone for ever"
    );
}

#[test]
fn red_team_only_the_account_itself_freshly_deletes_it() {
    let (_dir, directory) = directory();
    let owner = user(63);
    let other = user(64);
    register(&directory, "camille", &owner);
    register(&directory, "eve", &other);

    let mut forged = deletion(&other, "camille", NOW);
    forged.deletion.user = owner.user_id();
    for (attempt, why) in [
        (forged, "a deletion signed by another account's key"),
        (
            deletion(&owner, "camille", NOW - crate::claim::MAX_DELETION_AGE - 1),
            "a stale, kept request",
        ),
        (
            deletion(&other, "camille", NOW),
            "a request naming somebody else's account",
        ),
    ] {
        assert!(
            directory.delete_account(&attempt, NOW).is_err(),
            "{why} deleted an account"
        );
    }
    assert!(directory.account("camille").unwrap().is_some());
    assert_eq!(
        directory
            .deleted_among(&[owner.user_id(), other.user_id()])
            .unwrap(),
        Vec::new()
    );

    directory
        .delete_account(&deletion(&owner, "camille", NOW), NOW)
        .expect("delete");
    let told = directory
        .deleted_among(&[owner.user_id(), other.user_id()])
        .unwrap();
    assert_eq!(
        told.iter().map(|d| d.deletion.user).collect::<Vec<_>>(),
        vec![owner.user_id()],
        "a host asking which of its owners were deleted was not told, so it kept their chunks"
    );
}
