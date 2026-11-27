//! Standing: what a host is told about a device it is asked to store for.

use super::*;

/// HANDOVER §8 1c (ii). Every node holds its account key, so a withdrawn
/// device signs itself a fresh live claim and presents it to a host; the
/// coordinator's withdrawal, final here, is the only thing that says
/// otherwise, and it has to reach the host. Sabotage: answer with the
/// presented claim instead of the one held.
#[test]
fn red_team_standing_answers_the_withdrawal_not_the_claim_presented() {
    let directory = directory();
    let owner = user(21);
    let laptop = device(21);
    enrol(&directory, &owner, &laptop);
    directory
        .claim(&claim_of(&owner, &laptop, true, NOW + 1), NOW + 1)
        .expect("withdraw");
    let service = CoordService::new(&directory);

    let resigned = claim_of(&owner, &laptop, false, NOW + 100);
    match ask(
        &service,
        &Request::Standing(Box::new(resigned)),
        device(99).device_id(),
    ) {
        Response::Standing(Some(held)) => assert!(
            held.claim.revoked,
            "the coordinator answered a re-signed live claim for a withdrawn device"
        ),
        other => panic!("expected the withdrawal, got {other:?}"),
    }
}

/// The question carries a claim so that it cannot be asked of a device
/// the caller has never been shown: a claim not signed by the account it
/// names is refused, and one signed by another account than the device's
/// own learns nothing. Sabotage: drop either check.
#[test]
fn red_team_standing_tells_nothing_about_a_device_the_caller_was_not_shown() {
    let directory = directory();
    let owner = user(22);
    let laptop = device(22);
    enrol(&directory, &owner, &laptop);
    let service = CoordService::new(&directory);
    let snoop = user(23);

    let mut forged = claim_of(&owner, &laptop, false, NOW);
    forged.claim.pledged_bytes += 1;
    assert!(
        matches!(
            ask(
                &service,
                &Request::Standing(Box::new(forged)),
                device(99).device_id()
            ),
            Response::Refused(_)
        ),
        "a claim that does not verify was answered"
    );

    let own_key = NodeClaim {
        owner: snoop.user_id(),
        device: laptop.device_id(),
        pledged_bytes: 1,
        issued_unix: NOW,
        revoked: false,
    }
    .sign(&snoop);
    assert_eq!(
        ask(
            &service,
            &Request::Standing(Box::new(own_key)),
            device(99).device_id()
        ),
        Response::Standing(None),
        "a claim signed under another account learnt whose the device is"
    );
}

#[test]
fn standing_answers_a_live_device_with_its_claim_and_an_unknown_one_with_nothing() {
    let directory = directory();
    let owner = user(24);
    let laptop = device(24);
    enrol(&directory, &owner, &laptop);
    let service = CoordService::new(&directory);
    let presented = claim_of(&owner, &laptop, false, NOW);
    match ask(
        &service,
        &Request::Standing(Box::new(presented)),
        device(99).device_id(),
    ) {
        Response::Standing(Some(held)) => assert!(!held.claim.revoked),
        other => panic!("expected the live claim, got {other:?}"),
    }
    let stranger = claim_of(&owner, &device(25), false, NOW);
    assert_eq!(
        ask(
            &service,
            &Request::Standing(Box::new(stranger)),
            device(99).device_id()
        ),
        Response::Standing(None)
    );
}
