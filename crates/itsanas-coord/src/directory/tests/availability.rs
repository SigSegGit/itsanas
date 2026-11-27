//! Availability folded from what the coordinator heard, never from what a
//! device says, and what each device contributes.

use super::*;

#[test]
fn a_node_cannot_inflate_its_own_availability_by_saying_so() {
    // Availability is measured by the coordinator noticing heartbeats, not
    // asserted by the node. Announcing once must not buy full marks.
    let (_dir, directory) = directory();
    let owner = user(12);
    let dev = device(12);
    register(&directory, "nicolas", &owner);
    enrol(&directory, &owner, &dev, 100 * 1024 * 1024 * 1024);

    directory
        .announce(
            &Presence {
                device: dev.device_id(),
                address: "a:1".to_owned(),
                at_unix: NOW,
                head: 0,
            }
            .sign(&dev),
            NOW,
        )
        .unwrap();

    let contribution = directory.contributions(owner.user_id()).unwrap();
    assert_eq!(contribution.len(), 1);
    assert_eq!(
        contribution[0].availability_per_mille,
        crate::accounting::AVAILABILITY_FLOOR_PER_MILLE,
        "a single heartbeat bought more than the floor"
    );
}

#[test]
fn staying_up_raises_availability_and_going_away_lowers_it() {
    let (_dir, directory) = directory();
    let owner = user(13);
    let dev = device(13);
    register(&directory, "nicolas", &owner);
    enrol(&directory, &owner, &dev, 1024);

    let announce_at = |at: u64| {
        directory
            .announce(
                &Presence {
                    device: dev.device_id(),
                    address: "a:1".to_owned(),
                    at_unix: at,
                    head: 0,
                }
                .sign(&dev),
                at,
            )
            .unwrap();
    };
    let availability =
        || directory.contributions(owner.user_id()).unwrap()[0].availability_per_mille;

    announce_at(NOW);
    let start = availability();

    // Heartbeat every period for a good while.
    let mut clock = NOW;
    for _ in 0..200 {
        clock += TICK_SECONDS;
        announce_at(clock);
        directory.tick(clock).unwrap();
    }
    let after_uptime = availability();
    assert!(
        after_uptime > start,
        "staying up did not raise availability: {start} -> {after_uptime}"
    );

    // Now vanish for the same stretch.
    for _ in 0..200 {
        clock += TICK_SECONDS;
        directory.tick(clock).unwrap();
    }
    assert!(
        availability() < after_uptime,
        "going away did not lower availability"
    );
}

#[test]
fn a_coordinator_that_was_itself_down_does_not_annihilate_everyone() {
    // A coordinator offline for a month must not come back and fold a
    // month of "absent" into every member at once — the members were fine,
    // the coordinator was not.
    let (_dir, directory) = directory();
    let owner = user(14);
    let dev = device(14);
    register(&directory, "nicolas", &owner);
    enrol(&directory, &owner, &dev, 1024);

    directory
        .announce(
            &Presence {
                device: dev.device_id(),
                address: "a:1".to_owned(),
                at_unix: NOW,
                head: 0,
            }
            .sign(&dev),
            NOW,
        )
        .unwrap();

    let mut clock = NOW;
    for _ in 0..300 {
        clock += TICK_SECONDS;
        directory
            .announce(
                &Presence {
                    device: dev.device_id(),
                    address: "a:1".to_owned(),
                    at_unix: clock,
                    head: 0,
                }
                .sign(&dev),
                clock,
            )
            .unwrap();
        directory.tick(clock).unwrap();
    }
    let healthy = directory.contributions(owner.user_id()).unwrap()[0].availability_per_mille;

    // One tick, a year later.
    directory.tick(clock + 365 * 24 * 3600).unwrap();
    let after = directory.contributions(owner.user_id()).unwrap()[0].availability_per_mille;

    assert!(
        after > healthy / 2,
        "a year-long coordinator outage cut availability from {healthy} to \
         {after}; the cap on folded periods is not working"
    );
}

#[test]
fn contributions_are_sorted_so_two_readers_agree() {
    let (_dir, directory) = directory();
    let owner = user(17);
    register(&directory, "nicolas", &owner);
    for byte in [30u8, 10, 20] {
        enrol(&directory, &owner, &device(byte), 1024);
    }

    let contributions = directory.contributions(owner.user_id()).unwrap();
    assert_eq!(contributions.len(), 3);
    assert!(
        contributions
            .windows(2)
            .all(|pair| pair[0].device.to_bytes() <= pair[1].device.to_bytes())
    );
}
