//! Device claims: the bound per account, withdrawals that stay final, and
//! machines claimed by nobody or by two accounts.

use super::*;

/// [`MAX_DEVICES_PER_ACCOUNT`] as a device seed: a full account holds
/// seeds 1 to `FULL`, and `FULL + 1` is the device one too many.
// The assert, at compile time, is what makes the cast lossless.
#[allow(clippy::cast_possible_truncation)]
const FULL: u8 = {
    assert!(MAX_DEVICES_PER_ACCOUNT < 200, "device seeds are a u8");
    MAX_DEVICES_PER_ACCOUNT as u8
};

/// An account with exactly [`MAX_DEVICES_PER_ACCOUNT`] live devices,
/// seeds 1 to [`FULL`].
fn a_full_account(directory: &Directory) -> UserKeys {
    let owner = user(40);
    register(directory, "nicolas", &owner);
    for seed in 1..=FULL {
        directory
            .claim(&signed(&owner, seed, NOW, false), NOW)
            .expect("a device under the bound is admitted");
    }
    owner
}

/// THE DECISION (Nicolas, 2026-09-30, raised from 5 to 10 on 2026-10-22
/// and to 20 on 2026-11-20):
/// [`MAX_DEVICES_PER_ACCOUNT`] live devices per account. One more is
/// refused on the coordinator, which is the half a rebuilt client
/// cannot remove, and the refusal writes nothing: no claim, no index row
/// that `rebuild_owner_index_if_missing` would later count. The message
/// names the devices and the command, because it reaches clients older
/// than the bound as plain text and is all they will show.
#[test]
fn red_team_one_device_too_many_is_refused_and_nothing_is_written() {
    let (_dir, directory) = directory();
    let owner = a_full_account(&directory);
    let before = directory.enrolled_counts().expect("counts");
    assert_eq!(MAX_DEVICES_PER_ACCOUNT, 20);

    let refused = directory.claim(&signed(&owner, FULL + 1, NOW + 1, false), NOW + 1);

    let Err(CoordError::TooManyDevices {
        live,
        limit,
        devices,
    }) = refused
    else {
        panic!("one device too many was not refused as one too many: {refused:?}");
    };
    assert_eq!(
        (live, limit),
        (MAX_DEVICES_PER_ACCOUNT, MAX_DEVICES_PER_ACCOUNT)
    );
    assert!(
        devices.contains(&device(1).device_id().to_string()),
        "the refusal does not give the full ids an unenrolled machine needs: {devices}"
    );
    let text = CoordError::TooManyDevices {
        live,
        limit,
        devices,
    }
    .to_string();
    assert!(
        text.contains("itsanas device forget <id>"),
        "the refusal does not say how to free a slot: {text}"
    );
    assert!(
        directory
            .claim_for(device(FULL + 1).device_id())
            .expect("read")
            .is_none(),
        "the refused device was written anyway, and the address book will list it"
    );
    assert_eq!(
        directory.enrolled_counts().expect("counts"),
        before,
        "a refused claim changed the claims or the index"
    );
    assert_eq!(
        directory
            .live_claims_of(owner.user_id())
            .expect("live")
            .len(),
        MAX_DEVICES_PER_ACCOUNT
    );
}

/// `itsanas leave` is not a withdrawal: the departed device keeps its
/// claim, so it still holds one of the [`MAX_DEVICES_PER_ACCOUNT`] slots
/// and one device more is still refused. Freeing the slot is `itsanas device forget <id>` from
/// another device. Pinned here because a coordinator that freed slots on
/// departure would let a stolen keystore re-enrol after one `leave`, and
/// one that does not must say so to the person at the limit.
#[test]
fn red_team_a_departed_device_keeps_its_slot() {
    let (_dir, directory) = directory();
    let owner = a_full_account(&directory);
    directory
        .depart(
            &crate::claim::Departure {
                device: device(3).device_id(),
                at_unix: NOW + 1,
            }
            .sign(&device(3)),
            NOW + 1,
        )
        .expect("a departure is recorded");

    assert!(
        directory
            .claim_for(device(3).device_id())
            .expect("read")
            .is_some_and(|claim| !claim.claim.revoked),
        "leaving withdrew the claim: the docs promise it stays"
    );
    assert!(
        matches!(
            directory.claim(&signed(&owner, FULL + 1, NOW + 2, false), NOW + 2),
            Err(CoordError::TooManyDevices { live, .. }) if live == MAX_DEVICES_PER_ACCOUNT
        ),
        "a departure freed a slot: `leave` says the slot is kept until \
         `itsanas device forget`"
    );
}

/// A withdrawal frees its slot, and exactly one: the device it frees
/// cannot be brought back into it by replaying its own old live claim,
/// which the master secret on every keystore would make trivial.
#[test]
fn red_team_a_withdrawn_slot_lets_one_more_in_and_the_withdrawn_device_stays_out() {
    let (_dir, directory) = directory();
    let owner = a_full_account(&directory);
    let replay = signed(&owner, 3, NOW + 5, false);

    directory
        .claim(&signed(&owner, 3, NOW + 1, true), NOW + 1)
        .expect("withdraw device 3");
    directory
        .claim(&signed(&owner, FULL + 1, NOW + 2, false), NOW + 2)
        .expect("the freed slot admits a new device");
    assert!(
        matches!(
            directory.claim(&signed(&owner, FULL + 2, NOW + 3, false), NOW + 3),
            Err(CoordError::TooManyDevices { .. })
        ),
        "one withdrawal let two devices in"
    );

    // The withdrawn device, re-signed after its withdrawal: refused, still
    // withdrawn, and not counted as live on the way.
    assert!(
        directory.claim(&replay, NOW + 5).is_err(),
        "a withdrawn device came back"
    );
    assert!(
        directory
            .claim_for(device(3).device_id())
            .expect("read")
            .is_some_and(|claim| claim.claim.revoked),
        "the replay resurrected the withdrawn device"
    );
    assert_eq!(
        directory
            .live_claims_of(owner.user_id())
            .expect("live")
            .len(),
        MAX_DEVICES_PER_ACCOUNT
    );
}

/// Re-signing a device already live -- a new pledge, `itsanas register`
/// run twice -- is not a new device. Counting it would lock a full account
/// out of changing any pledge.
#[test]
fn red_team_re_signing_a_live_device_on_a_full_account_takes_no_slot() {
    let (_dir, directory) = directory();
    let owner = a_full_account(&directory);

    let changed = directory
        .claim(&signed(&owner, 2, NOW + 1, false), NOW + 1)
        .expect("a full account can still re-sign one of its own devices");
    assert!(changed, "the newer claim was not kept");
    assert_eq!(
        directory
            .live_claims_of(owner.user_id())
            .expect("live")
            .len(),
        MAX_DEVICES_PER_ACCOUNT
    );
}

/// An account enrolled above the bound before it existed is not broken:
/// every device stays live and can re-sign, it simply cannot add another,
/// and it is told how many it has.
#[test]
fn an_account_already_above_the_bound_keeps_its_devices_and_cannot_add_one() {
    let (_dir, directory) = directory();
    let owner = user(41);
    register(&directory, "nicolas", &owner);
    directory.bound_devices(false);
    let above = FULL + 2;
    for seed in 1..=above {
        directory
            .claim(&signed(&owner, seed, NOW, false), NOW)
            .expect("an older coordinator admitted them all");
    }
    directory.bound_devices(true);

    assert_eq!(
        directory
            .live_claims_of(owner.user_id())
            .expect("live")
            .len(),
        usize::from(above)
    );
    directory
        .claim(&signed(&owner, 4, NOW + 1, false), NOW + 1)
        .expect("a device of an account above the bound can still re-sign");
    let refused = directory.claim(&signed(&owner, above + 1, NOW + 2, false), NOW + 2);
    assert!(
        matches!(refused, Err(CoordError::TooManyDevices { live, .. }) if live == usize::from(above)),
        "one more device was not refused with the real count: {refused:?}"
    );
    assert_eq!(
        directory
            .live_claims_of(owner.user_id())
            .expect("live")
            .len(),
        usize::from(above)
    );
}

/// The bound is per account: one member's full roster says nothing about
/// anybody else's.
#[test]
fn a_full_account_does_not_stop_another_account_enrolling() {
    let (_dir, directory) = directory();
    let _full = a_full_account(&directory);
    let other = user(42);
    register(&directory, "partner", &other);
    directory
        .claim(&signed(&other, FULL + 1, NOW, false), NOW)
        .expect("another account's first device");
}

#[test]
fn a_device_cannot_be_claimed_without_an_account() {
    // Otherwise the node set fills with machines belonging to nobody and
    // the accounting has no member to attribute them to.
    let (_dir, directory) = directory();
    let owner = user(6);

    let claim = NodeClaim {
        owner: owner.user_id(),
        device: device(6).device_id(),
        pledged_bytes: 1024,
        issued_unix: NOW,
        revoked: false,
    }
    .sign(&owner);

    assert!(matches!(
        directory.claim(&claim, NOW),
        Err(CoordError::NoSuchAccount(_))
    ));
}

#[test]
fn a_device_cannot_be_claimed_by_two_accounts() {
    let (_dir, directory) = directory();
    let first = user(7);
    let second = user(8);
    register(&directory, "first", &first);
    register(&directory, "second", &second);

    enrol(&directory, &first, &device(7), 1024);

    let stolen = NodeClaim {
        owner: second.user_id(),
        device: device(7).device_id(),
        pledged_bytes: 1024,
        issued_unix: NOW + 10,
        revoked: false,
    }
    .sign(&second);

    assert!(matches!(
        directory.claim(&stolen, NOW + 10),
        Err(CoordError::Rejected(_))
    ));
}

#[test]
fn a_revoked_device_leaves_the_live_set() {
    let (_dir, directory) = directory();
    let owner = user(9);
    register(&directory, "nicolas", &owner);
    enrol(&directory, &owner, &device(9), 1024);

    assert_eq!(directory.live_claims().unwrap().len(), 1);

    let revoked = NodeClaim {
        owner: owner.user_id(),
        device: device(9).device_id(),
        pledged_bytes: 0,
        issued_unix: NOW + 100,
        revoked: true,
    }
    .sign(&owner);
    assert!(directory.claim(&revoked, NOW + 100).unwrap());

    assert_eq!(
        directory.live_claims().unwrap(),
        [] as [crate::claim::SignedClaim; 0]
    );
}

#[test]
fn red_team_a_machine_holding_the_master_key_cannot_bring_a_withdrawn_device_back() {
    // THE ATTACK: a laptop is stolen with its passphrase -- on Windows the
    // logon task reads it from a file beside the keystore. The owner runs
    // `itsanas device forget` from another machine. The thief runs
    // `itsanas register`, which signs a fresh claim with the master secret
    // every keystore holds, dated after the withdrawal. By timestamp alone
    // that claim won, the device was enrolled again, and the coordinator
    // said `Done`.
    let (_dir, directory) = directory();
    let owner = user(12);
    let stolen = device(12);
    register(&directory, "nicolas", &owner);
    enrol(&directory, &owner, &stolen, 1024);

    let withdrawal = NodeClaim {
        owner: owner.user_id(),
        device: stolen.device_id(),
        pledged_bytes: 0,
        issued_unix: NOW + 100,
        revoked: true,
    }
    .sign(&owner);
    assert!(directory.claim(&withdrawal, NOW + 100).unwrap());

    let re_enrolment = NodeClaim {
        owner: owner.user_id(),
        device: stolen.device_id(),
        pledged_bytes: 1024,
        issued_unix: NOW + 200,
        revoked: false,
    }
    .sign(&owner);

    assert!(
        matches!(
            directory.claim(&re_enrolment, NOW + 200),
            Err(CoordError::Rejected(_))
        ),
        "a re-enrolment of a withdrawn device was not refused out loud, so \
         `itsanas register` on the stolen machine reports success"
    );
    assert!(
        directory.live_claims().unwrap().is_empty(),
        "the withdrawn device is enrolled again"
    );
}

/// THE ATTACK: a device id is public while its account lives (claimed
/// peers, hosts). Once the account deletes itself, another member claims
/// that id under their own account, and the machine, still announcing,
/// is listed as theirs. Sabotage: drop the deleted-devices check in
/// `claim`.
#[test]
fn red_team_a_deleted_accounts_device_cannot_be_claimed_by_another() {
    let (_dir, directory) = directory();
    let (alice, mallory) = (user(95), user(96));
    register(&directory, "alice", &alice);
    register(&directory, "mallory", &mallory);
    directory
        .claim(&signed(&alice, 7, NOW, false), NOW)
        .expect("alice enrols the machine");
    directory
        .delete_account(&deletion(&alice, "alice", NOW + 1), NOW + 1)
        .expect("delete");

    let taken = directory.claim(&signed(&mallory, 7, NOW + 2, false), NOW + 2);
    assert!(
        taken.is_err(),
        "another member claimed a deleted account's device: {taken:?}"
    );
    directory
        .claim(&signed(&mallory, 8, NOW + 2, false), NOW + 2)
        .expect("a device of her own still enrols");
}
