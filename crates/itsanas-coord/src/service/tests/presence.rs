//! Silent devices, the expiry pass, and departures kept apart from silence.

use super::*;

/// What `peers_of` hands out and what `device list` shows must not move
/// before the keep, and past it a machine has lost a line in a file, not its
/// place in the account: switching it back on is all it takes.
#[test]
fn a_device_silent_past_the_keep_stays_enrolled_and_comes_back_by_announcing() {
    let directory = directory();
    let owner = user(33);
    let (laptop, phone) = (device(33), device(34));
    enrol(&directory, &owner, &laptop);
    directory
        .claim(&claim_of(&owner, &phone, false, NOW), NOW)
        .expect("claim");
    announce_address(&directory, &owner, &laptop, "203.0.113.7:9797");
    let service = CoordService::new(&directory);
    let user_id = owner.user_id();

    let peers = |now| match ask_at(
        &service,
        &Request::Peers { user: user_id },
        phone.device_id(),
        now,
    ) {
        Response::Peers(peers) => peers.len(),
        other => panic!("expected peers, got {other:?}"),
    };
    let listed = |now| match ask_at(
        &service,
        &Request::Devices { user: user_id },
        phone.device_id(),
        now,
    ) {
        Response::Devices(devices) => devices
            .into_iter()
            .find(|row| row.device == laptop.device_id())
            .map(|row| (row.silent_for, row.address.is_some()))
            .expect("the laptop is enrolled and must be listed"),
        other => panic!("expected devices, got {other:?}"),
    };

    assert_eq!(
        peers(NOW + PRESENCE_TTL),
        1,
        "an address inside its week stopped being handed out: its machine cannot be found"
    );
    assert_eq!(
        peers(NOW + PRESENCE_TTL + 1),
        0,
        "an address past its week is still handed out: every peer pays a dial for it"
    );
    assert_eq!(
        listed(NOW + PRESENCE_TTL + 1),
        (Some(PRESENCE_TTL + 1), true),
        "past the week its owner must still see how long it has been silent"
    );

    let at_the_keep = NOW + PRESENCE_KEPT_FOR;
    assert_eq!(
        service.expire(at_the_keep).unwrap().addresses,
        0,
        "a pass erased an address inside the keep"
    );
    assert_eq!(
        listed(at_the_keep),
        (Some(PRESENCE_KEPT_FOR), true),
        "a pass inside the keep changed what the owner's device list says"
    );

    let past_it = at_the_keep + 1;
    assert_eq!(
        service.expire(past_it).unwrap().addresses,
        1,
        "a pass past the keep erased nothing: the address stays on file for good"
    );
    assert_eq!(
        listed(past_it),
        (None, false),
        "the address of a machine silent past the keep is still on file"
    );
    assert_eq!(peers(past_it), 0, "an erased address is still handed out");

    directory
        .announce(
            &Presence {
                device: laptop.device_id(),
                address: "203.0.113.7:9797".to_owned(),
                at_unix: past_it,
                head: 0,
            }
            .sign(&laptop),
            past_it,
        )
        .expect("a machine whose address was erased must be able to announce again");
    assert_eq!(peers(past_it), 1, "it came back on, and nobody can find it");
    assert_eq!(
        listed(past_it),
        (Some(0), true),
        "a machine that came back reads as silent in its owner's device list"
    );
}

/// The server calls one thing; a table that pass forgets is a table whose
/// retention exists only in a test.
#[test]
fn one_pass_erases_the_silent_addresses_the_free_names_and_the_spent_counts() {
    let directory = directory();
    let owner = user(35);
    let laptop = device(35);
    enrol(&directory, &owner, &laptop);
    announce_address(&directory, &owner, &laptop, "203.0.113.7:9797");
    let leaving = user(36);
    directory
        .register(
            &crate::directory::Registration {
                username: "leaving".to_owned(),
                user: leaving.public(),
                issued_unix: NOW,
            }
            .sign(&leaving),
            NOW,
        )
        .expect("register");
    directory
        .delete_account(
            &crate::claim::Deletion {
                user: leaving.user_id(),
                username: "leaving".to_owned(),
                at_unix: NOW,
            }
            .sign(&leaving),
            NOW,
        )
        .expect("delete");
    let limiter = Mutex::new(EscrowLimiter::new());
    let service = CoordService::new(&directory);
    service
        .escrow_allowed(&limiter, "invented", Instant::now(), NOW)
        .expect("count");

    let later = NOW + crate::directory::NAME_RETIRED_FOR;
    assert_eq!(
        service.expire(later).unwrap(),
        Expired {
            addresses: 1,
            names: 1,
            escrow_counts: 1
        },
        "one pass must erase one row of each of the three"
    );
    assert_eq!(
        service.expire(later).unwrap(),
        Expired::default(),
        "a second pass found rows the first said it had erased"
    );
}

/// A departure is history about a device, and a history anybody can write
/// about somebody else is worthless to the regulation it is kept for. Two
/// ways in: a notice signed by the right device but delivered by another
/// (a replay), and a notice naming a device and signed by another (a
/// forgery). Both must leave nothing on record.
#[test]
fn red_team_a_departure_notice_from_another_device_is_refused() {
    let directory = directory();
    let owner = user(12);
    let laptop = device(12);
    let other = device(13);
    enrol(&directory, &owner, &laptop);
    enrol(&directory, &user(13), &other);
    let service = CoordService::new(&directory);

    let genuine = crate::claim::Departure {
        device: laptop.device_id(),
        at_unix: NOW,
    }
    .sign(&laptop);
    let replayed = ask(
        &service,
        &Request::Depart(Box::new(genuine.clone())),
        other.device_id(),
    );
    assert!(
        matches!(replayed, Response::Refused(_)),
        "a member delivered another device's departure: {replayed:?}"
    );

    let forged = crate::claim::Departure {
        device: laptop.device_id(),
        at_unix: NOW,
    }
    .sign(&other);
    let forged = ask(
        &service,
        &Request::Depart(Box::new(forged)),
        laptop.device_id(),
    );
    assert!(
        matches!(forged, Response::Refused(_)),
        "a departure signed by another key was believed: {forged:?}"
    );

    assert_eq!(
        directory.departed(laptop.device_id()).unwrap(),
        None,
        "a refused notice still wrote a departure into the laptop's history"
    );

    // And the device itself is heard, or the refusals above prove nothing.
    let own = ask(
        &service,
        &Request::Depart(Box::new(genuine)),
        laptop.device_id(),
    );
    assert!(matches!(own, Response::Done), "{own:?}");
    assert_eq!(directory.departed(laptop.device_id()).unwrap(), Some(NOW));
}

/// A departure and a silence are different facts, and the only reason to
/// record departures is to keep them apart. A device that stopped
/// announcing without a word has no departure on record, however long it
/// has been gone.
#[test]
fn a_departure_is_recorded_apart_from_a_silence() {
    let directory = directory();
    let owner = user(14);
    let polite = device(14);
    let silent = device(15);
    enrol(&directory, &owner, &polite);
    enrol(&directory, &user(15), &silent);
    announce_address(&directory, &owner, &polite, "a:1");
    announce_address(&directory, &user(15), &silent, "b:1");

    directory
        .depart(
            &crate::claim::Departure {
                device: polite.device_id(),
                at_unix: NOW,
            }
            .sign(&polite),
            NOW + 60,
        )
        .unwrap();

    assert_eq!(
        directory.departed(polite.device_id()).unwrap(),
        Some(NOW + 60)
    );
    assert_eq!(
        directory.departed(silent.device_id()).unwrap(),
        None,
        "a silent device was recorded as having left politely"
    );
    // Recording it changes nothing else yet: its last presence stands.
    assert_eq!(directory.last_seen(polite.device_id()).unwrap(), Some(NOW));
}
