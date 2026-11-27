//! Admission by invitation: the founding window, one use per code, the
//! monthly bound, and refusals that read the same.

use super::*;

#[test]
fn the_first_member_of_an_invite_only_coordinator_can_join() {
    // The chicken and the egg. An invitation to admit the first member has
    // no author, so requiring one produces a coordinator that is running,
    // reachable, correct in every detail and impossible to join. The first
    // version of this did exactly that and named the problem in a doc
    // comment, as though naming it were the same as handling it.
    let (_dir, directory) = directory();
    found(&directory, &user(1), "alice", 1_000).expect("the founder");
}

#[test]
fn red_team_the_founding_window_is_asked_for_and_shuts_by_itself() {
    // THE ATTACK, and it is one the fix introduced. If an empty directory
    // always admitted its first caller, then on a public address the
    // founder is whoever finds the port first — and the operator learns
    // this by being refused from their own coordinator, with a stranger
    // already inside holding the only account that can invite.
    //
    // So the window is a flag the operator passes, open only while they are
    // standing at the terminal, and it still admits exactly one account.
    let (_dir, directory) = directory();

    // Without the flag, an empty directory admits nobody.
    assert!(
        join(&directory, &user(9), "mallory", None, 900).is_err(),
        "an empty invite-only coordinator admitted a stranger unasked"
    );

    found(&directory, &user(1), "alice", 1_000).expect("the founder");

    assert!(
        found(&directory, &user(2), "bob", 1_100).is_err(),
        "the second caller walked in through the founding window"
    );
    assert!(
        found(&directory, &user(3), "carol", 1_200).is_err(),
        "the window reopened"
    );
}

#[test]
fn red_team_a_registration_that_fails_does_not_burn_the_invitation() {
    // Redeeming and creating the account used to be two transactions: spend
    // the use, commit, then write the account. Anything that failed after
    // the first — and `NameTaken` is trivial to provoke on purpose and easy
    // to provoke by accident — destroyed the invitation and created
    // nothing.
    //
    // Free denial of service against the inviter, and an invitee locked out
    // of the network by their own typing error.
    let (_dir, directory) = directory();
    let alice = user(1);
    let code = [0xBBu8; SECRET_LEN];
    directory
        .register(&open_registration(&alice, "alice", 1_000), 1_000)
        .expect("alice");
    directory
        .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
        .expect("lodge");

    // Bob mistypes and asks for a name Alice already holds.
    assert!(
        join(&directory, &user(2), "alice", Some(&code), 1_100).is_err(),
        "the name was not actually taken, so this proves nothing"
    );

    let lodged = directory
        .invitation(&invitation::code_id(&code))
        .expect("read")
        .expect("filed");
    assert_eq!(
        lodged.remaining, 1,
        "a failed registration spent the invitation"
    );

    // And the same code still works once he types his own name.
    join(&directory, &user(2), "bob", Some(&code), 1_200).expect("bob, correctly");
}

#[test]
fn an_invited_stranger_joins_and_an_uninvited_one_does_not() {
    let (_dir, directory) = directory();
    let alice = user(1);
    let bob = user(2);
    let carol = user(3);
    let code = [0x11u8; SECRET_LEN];

    // Alice is already a member: the first one is admitted by the operator,
    // because an invitation to admit the first member has no author.
    directory
        .register(&open_registration(&alice, "alice", 1_000), 1_000)
        .expect("alice, openly");

    directory
        .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
        .expect("lodge");

    join(&directory, &bob, "bob", Some(&code), 1_100).expect("bob was invited");
    assert!(
        join(&directory, &carol, "carol", None, 1_100).is_err(),
        "a stranger with no code joined a coordinator that admits by invitation"
    );
}

/// THE ATTACK: a member signs invitations for accounts of its own, as
/// many as it likes, and fills the network with them. A member offers at
/// most `INVITED_PLACES` places a month, every use of a code counted: past
/// that a new code is refused and a given one still works; re-lodging a
/// code costs nothing; another member is not held by the first one's
/// count; a month later there is room again. Sabotage: count nothing;
/// count codes, not their uses; never open a new window.
#[test]
fn red_team_a_member_invites_at_most_ten_places_a_month() {
    let (_dir, directory) = directory();
    let (alice, bob) = (user(1), user(2));
    for (who, name) in [(&alice, "alice"), (&bob, "bob")] {
        directory
            .register(&open_registration(who, name, 1_000), 1_000)
            .expect("member");
    }
    let code = |n: u8| [n; SECRET_LEN];
    let lodge = |who: &UserKeys, n: u8, uses: u32, at: u64| {
        directory.lodge_invitation(&invite(who, &code(n), uses, at), at)
    };

    lodge(&alice, 10, 4, 1_000).expect("a household of four");
    for n in 11..17 {
        lodge(&alice, n, 1, 1_000).expect("within ten places");
    }
    lodge(&alice, 10, 4, 1_100).expect("re-lodging a code costs nothing");
    assert!(
        lodge(&alice, 17, 1, 1_200).is_err(),
        "an eleventh place was offered in one month: a member invites as many accounts of its own as it likes"
    );
    join(&directory, &user(3), "invited", Some(&code(16)), 1_300)
        .expect("a code given before the limit still admits");
    lodge(&bob, 30, 10, 1_300).expect("another member is not held by the first's count");

    let month = INVITE_WINDOW + 1_000;
    lodge(&alice, 19, 1, month).expect("a month later there is room again");
}

#[test]
fn red_team_one_invitation_admits_one_stranger_however_many_try_it() {
    // THE ATTACK: a code posted in a group chat, or leaked by the person it
    // was sent to. If uses were not spent, one endorsement would admit
    // everybody who ever saw it, and "membership costs a member's
    // endorsement" would be false for every account after the first.
    //
    // If this test fails, one leaked code is an open door.
    let (_dir, directory) = directory();
    let alice = user(1);
    let code = [0x22u8; SECRET_LEN];
    directory
        .register(&open_registration(&alice, "alice", 1_000), 1_000)
        .expect("alice");
    directory
        .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
        .expect("lodge");

    join(&directory, &user(2), "first", Some(&code), 1_100).expect("the invited one");
    for (seed, name) in [(3u8, "second"), (4, "third"), (5, "fourth")] {
        assert!(
            join(&directory, &user(seed), name, Some(&code), 1_100).is_err(),
            "{name} joined on an invitation that had already been spent"
        );
    }

    let lodged = directory
        .invitation(&invitation::code_id(&code))
        .expect("read")
        .expect("still filed");
    assert_eq!(lodged.remaining, 0);
    assert_eq!(lodged.admitted.len(), 1, "more than one use was recorded");
}

#[test]
fn red_team_re_lodging_a_spent_invitation_does_not_refill_it() {
    // The retry path, and a way round the previous test if it were missed.
    // A client whose connection dropped re-sends what it signed; if lodging
    // reset the counter, an inviter could refill their own code for ever
    // and one endorsement would again admit everybody.
    let (_dir, directory) = directory();
    let alice = user(1);
    let code = [0x33u8; SECRET_LEN];
    directory
        .register(&open_registration(&alice, "alice", 1_000), 1_000)
        .expect("alice");
    let signed = invite(&alice, &code, 1, 1_000);
    directory.lodge_invitation(&signed, 1_000).expect("lodge");
    join(&directory, &user(2), "bob", Some(&code), 1_100).expect("bob");

    let again = directory
        .lodge_invitation(&signed, 1_200)
        .expect("re-lodge");
    assert_eq!(again.remaining, 0, "re-lodging refilled a spent invitation");
    assert!(
        join(&directory, &user(3), "mallory", Some(&code), 1_300).is_err(),
        "a refilled invitation admitted a second stranger"
    );
}

#[test]
fn red_team_a_stranger_cannot_vouch_for_a_stranger() {
    // Otherwise invitation buys nothing: an attacker mints one keypair,
    // signs invitations with it, and admits as many accounts as it likes.
    // The endorsement has to come from somebody already inside.
    let (_dir, directory) = directory();
    let outsider = user(9);
    let code = [0x44u8; SECRET_LEN];

    assert!(
        directory
            .lodge_invitation(&invite(&outsider, &code, 5, 1_000), 1_000)
            .is_err(),
        "a coordinator accepted an endorsement from somebody it has never heard of"
    );
    assert!(
        join(&directory, &user(10), "mallory", Some(&code), 1_100).is_err(),
        "the unlodged invitation admitted somebody anyway"
    );
}

#[test]
fn an_expired_invitation_admits_nobody() {
    let (_dir, directory) = directory();
    let alice = user(1);
    let code = [0x55u8; SECRET_LEN];
    directory
        .register(&open_registration(&alice, "alice", 1_000), 1_000)
        .expect("alice");
    directory
        .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
        .expect("lodge");

    let long_after = 1_000 + crate::invitation::DEFAULT_VALIDITY + 1;
    assert!(
        join(&directory, &user(2), "bob", Some(&code), long_after).is_err(),
        "a code from last year still opened the door"
    );
}

#[test]
fn a_member_re_registering_needs_no_new_invitation() {
    // Re-registering is how a member refreshes their agreement key and how
    // a client retries a dropped connection. Demanding a fresh invitation
    // for either would lock members out of their own accounts, on a
    // coordinator whose whole job is to let them back in.
    let (_dir, directory) = directory();
    let alice = user(1);
    let bob = user(2);
    let code = [0x66u8; SECRET_LEN];
    directory
        .register(&open_registration(&alice, "alice", 1_000), 1_000)
        .expect("alice");
    directory
        .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
        .expect("lodge");
    join(&directory, &bob, "bob", Some(&code), 1_100).expect("bob joins");

    join(&directory, &bob, "bob", None, 1_200).expect("bob comes back without a code");

    let lodged = directory
        .invitation(&invitation::code_id(&code))
        .expect("read")
        .expect("filed");
    assert_eq!(
        lodged.admitted.len(),
        1,
        "the return visit spent a second use"
    );
}

#[test]
fn who_let_them_in_has_an_answer_afterwards() {
    // Attribution is what an endorsement is for. A member who admits forty
    // accounts that all fail their audits has to be findable, or inviting
    // is free in the only sense that matters.
    let (_dir, directory) = directory();
    let alice = user(1);
    let code = [0x77u8; SECRET_LEN];
    directory
        .register(&open_registration(&alice, "alice", 1_000), 1_000)
        .expect("alice");
    directory
        .lodge_invitation(&invite(&alice, &code, 3, 1_000), 1_000)
        .expect("lodge");

    for (seed, name) in [(2u8, "bob"), (3, "carol"), (4, "dave")] {
        join(&directory, &user(seed), name, Some(&code), 1_100).expect(name);
    }

    let lodged = directory
        .invitation(&invitation::code_id(&code))
        .expect("read")
        .expect("filed");
    assert_eq!(lodged.signed.invitation.inviter, alice.user_id());
    assert_eq!(lodged.admitted.len(), 3);
    assert_eq!(lodged.remaining, 0);
}

#[test]
fn every_refusal_reads_the_same_so_codes_cannot_be_enumerated() {
    // A coordinator that said "no such code" for one and "already spent"
    // for another would let anybody probe which codes exist, and the codes
    // are the thing keeping strangers out. Same sentence, every time.
    let (_dir, directory) = directory();
    let alice = user(1);
    let spent = [0x88u8; SECRET_LEN];
    let expired = [0x99u8; SECRET_LEN];
    let unknown = [0xAAu8; SECRET_LEN];
    directory
        .register(&open_registration(&alice, "alice", 1_000), 1_000)
        .expect("alice");
    directory
        .lodge_invitation(&invite(&alice, &spent, 1, 1_000), 1_000)
        .expect("lodge");
    directory
        .lodge_invitation(&invite(&alice, &expired, 1, 1_000), 1_000)
        .expect("lodge");
    join(&directory, &user(2), "bob", Some(&spent), 1_100).expect("bob");

    let after = 1_000 + crate::invitation::DEFAULT_VALIDITY + 1;
    let reasons: Vec<String> = [&spent, &expired, &unknown]
        .iter()
        .enumerate()
        .map(|(index, code)| {
            let name = format!("probe{index}");
            let seed = u8::try_from(20 + index).unwrap_or(20);
            join(&directory, &user(seed), &name, Some(code), after)
                .expect_err("all three must be refused")
                .to_string()
        })
        .collect();

    assert_eq!(
        reasons
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1,
        "three different refusals let a stranger tell live codes from dead \
         ones: {reasons:?}"
    );
}
