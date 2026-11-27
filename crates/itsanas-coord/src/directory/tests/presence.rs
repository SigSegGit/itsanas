//! Presence: what a device announces, and rows in an older format.

use super::*;

/// The fleet, 2026-11-16 to 11-22: after the coordinator moved to the
/// presence format with the chain's length, a row written in the old
/// format by a device silent since made every listing of its account fail
/// ("Hit the end of buffer"), and no machine of the account found the
/// others. Sabotage: decode with `?` again in `presence_of` or
/// `presences_after`; or keep an unreadable row in `expire_presence`.
#[test]
fn red_team_a_presence_in_an_old_format_hides_only_its_own_device() {
    #[derive(serde::Serialize)]
    struct OldPresence {
        device: DeviceId,
        address: String,
        at_unix: u64,
    }
    #[derive(serde::Serialize)]
    struct OldSigned {
        presence: OldPresence,
        signature: itsanas_crypto::Signature,
    }

    let (_dir, directory) = directory();
    let owner = user(31);
    register(&directory, "camille", &owner);
    enrol(&directory, &owner, &device(31), 1024);
    enrol(&directory, &owner, &device(32), 1024);
    let fresh = Presence {
        device: device(31).device_id(),
        address: "10.0.0.31:9797".to_owned(),
        at_unix: NOW,
        head: 3,
    }
    .sign(&device(31));
    directory.announce(&fresh, NOW).unwrap();
    let old = OldSigned {
        presence: OldPresence {
            device: device(32).device_id(),
            address: "10.0.0.32:9797".to_owned(),
            at_unix: NOW,
        },
        signature: fresh.signature,
    };
    {
        let txn = directory.db.begin_write().unwrap();
        txn.open_table(PRESENCE)
            .unwrap()
            .insert(
                device(32).device_id().to_bytes().as_slice(),
                postcard::to_stdvec(&old).unwrap().as_slice(),
            )
            .unwrap();
        txn.commit().unwrap();
    }

    assert_eq!(
        directory.presence_of(device(32).device_id()).unwrap(),
        None,
        "a presence in the old format read as a device seen"
    );
    assert_eq!(
        directory.presence_of(device(31).device_id()).unwrap(),
        Some(fresh.clone()),
        "the readable presence was lost beside an unreadable one"
    );
    assert_eq!(
        directory
            .presences_after(device(31).device_id(), 10)
            .unwrap(),
        vec![fresh],
        "one unreadable row failed the whole sample: the account's machines find nobody"
    );
    assert_eq!(
        directory.expire_presence(NOW, 3600).unwrap(),
        1,
        "an unreadable row was kept by the expiry pass, for a month"
    );
}

#[test]
fn an_unclaimed_device_cannot_announce_an_address() {
    let (_dir, directory) = directory();
    let dev = device(10);

    let announced = Presence {
        device: dev.device_id(),
        address: "10.0.0.1:9797".to_owned(),
        at_unix: NOW,
        head: 0,
    }
    .sign(&dev);

    assert!(matches!(
        directory.announce(&announced, NOW),
        Err(CoordError::UnclaimedDevice(_))
    ));
}

#[test]
fn presence_is_recorded_for_a_claimed_device() {
    let (_dir, directory) = directory();
    let owner = user(11);
    let dev = device(11);
    register(&directory, "nicolas", &owner);
    enrol(&directory, &owner, &dev, 1024);

    let announced = Presence {
        device: dev.device_id(),
        address: "10.0.0.1:9797".to_owned(),
        at_unix: NOW,
        head: 0,
    }
    .sign(&dev);
    directory.announce(&announced, NOW).unwrap();

    assert_eq!(
        directory
            .presence_of(dev.device_id())
            .unwrap()
            .unwrap()
            .presence
            .address,
        "10.0.0.1:9797"
    );
}
