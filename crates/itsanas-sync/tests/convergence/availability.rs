use super::{LAPTOP, PI, VM, swarm};

// ---------------------------------------------------------------------------
// The headline scenario
// ---------------------------------------------------------------------------

#[test]
fn a_host_that_loses_everything_is_survivable_because_the_devices_still_have_it() {
    // The premise the whole architecture rests on, stated as a scenario rather
    // than as a sentence in a design document: hosts are blind *and*
    // untrustworthy, so losing one entirely must cost nothing but bandwidth.
    //
    // Three devices publish, the cloud drops every chunk it holds — a disk
    // dies, a host walks away, a VPS is deleted — and the swarm re-uploads and
    // converges on the same content. Nothing here is allowed to notice.
    let swarm = swarm();

    swarm
        .device(0)
        .write("notes/a.txt", b"written on device zero")
        .unwrap();
    swarm
        .device(1)
        .write("notes/b.txt", b"written on device one")
        .unwrap();
    swarm.publish_all().unwrap();
    swarm.settle_and_check().unwrap();

    let before = swarm.cloud().with(|cloud| cloud.chunk_count());
    assert!(before > 0, "the fixture never put anything in the cloud");

    swarm
        .cloud()
        .with(itsanas_sync::sim::Cloud::forget_all_chunks);
    assert_eq!(
        swarm.cloud().with(|cloud| cloud.chunk_count()),
        0,
        "the host did not actually lose anything"
    );

    // Every device still has its own data, so a republish restores the host.
    swarm.publish_all().unwrap();
    swarm.settle_and_check().unwrap();

    assert_eq!(
        swarm.device(2).read("notes/a.txt").unwrap().as_deref(),
        Some(&b"written on device zero"[..]),
        "a host losing its disk lost a file that three devices still had"
    );
    assert_eq!(
        swarm.device(2).read("notes/b.txt").unwrap().as_deref(),
        Some(&b"written on device one"[..])
    );
    assert!(
        swarm.cloud().with(|cloud| cloud.chunk_count()) >= before,
        "the cloud was refilled with less than it lost"
    );
}

#[test]
fn a_device_that_is_offline_publishes_nothing_and_blocks_nobody() {
    // The ordinary state of this network: most machines are off most of the
    // time. A device that is down must not stop the others converging, and it
    // must not appear to have published work it never sent.
    let mut swarm = swarm();

    swarm.set_online(1, false);
    assert!(
        !swarm.device(1).is_online(),
        "the fixture did not take the device down"
    );

    swarm
        .device(0)
        .write("awake.txt", b"from the machine that was on")
        .unwrap();
    swarm
        .device(1)
        .write("asleep.txt", b"never left the building")
        .unwrap();
    swarm.publish_all().unwrap();
    swarm.settle_and_check().unwrap();

    assert_eq!(
        swarm.device(2).read("awake.txt").unwrap().as_deref(),
        Some(&b"from the machine that was on"[..]),
        "an offline device stopped the others converging"
    );
    assert_eq!(
        swarm.device(2).read("asleep.txt").unwrap(),
        None,
        "work from an offline device arrived without it ever publishing"
    );

    // It comes back, and its work arrives without anybody doing anything else.
    swarm.set_online(1, true);
    swarm.publish_all().unwrap();
    swarm.settle_and_check().unwrap();
    assert_eq!(
        swarm.device(2).read("asleep.txt").unwrap().as_deref(),
        Some(&b"never left the building"[..]),
        "a device that came back never got its work out"
    );
}

#[test]
fn a_device_that_never_comes_back_still_gets_its_work_to_everyone_else() {
    // The scenario the entire architecture exists for. The Pi writes at 3am
    // while the laptop is shut, publishes to whichever hosts are up, and is
    // then switched off permanently. The laptop and the VM must still end up
    // with the file, having never spoken to the Pi.
    let mut swarm = swarm();

    swarm.set_online(LAPTOP, false);
    swarm.set_online(VM, false);

    swarm
        .device(PI)
        .write("reports/q3.txt", b"written on the Pi at 3am")
        .unwrap();
    swarm.device(PI).publish().unwrap();

    // The Pi dies. It never returns.
    swarm.set_online(PI, false);
    swarm.set_online(LAPTOP, true);
    swarm.set_online(VM, true);

    swarm.settle_and_check().unwrap();

    for device in [LAPTOP, VM] {
        assert_eq!(
            swarm
                .device(device)
                .read("reports/q3.txt")
                .unwrap()
                .unwrap(),
            b"written on the Pi at 3am",
            "device {device} never received the Pi's work, and the Pi is gone"
        );
    }
}

#[test]
fn work_propagates_through_a_third_device_that_only_relays() {
    // The Pi and the VM are never online at the same time, so they never share
    // a host directly. The laptop is the only overlap.
    let mut swarm = swarm();

    swarm.set_online(VM, false);
    swarm
        .device(PI)
        .write("relayed.txt", b"from the Pi")
        .unwrap();
    swarm.device(PI).publish().unwrap();
    swarm.device(LAPTOP).sync().unwrap();

    swarm.set_online(PI, false);
    swarm.set_online(VM, true);

    swarm.settle_and_check().unwrap();

    assert_eq!(
        swarm.device(VM).read("relayed.txt").unwrap().unwrap(),
        b"from the Pi",
        "the VM never learned about a file it could only have heard of second-hand"
    );
}
