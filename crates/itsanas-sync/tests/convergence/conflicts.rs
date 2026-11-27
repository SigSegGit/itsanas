use itsanas_store::CausalOrder;
use itsanas_sync::conflict;

use super::{LAPTOP, PI, VM, swarm};

// ---------------------------------------------------------------------------
// Concurrent edits
// ---------------------------------------------------------------------------

#[test]
fn concurrent_edits_produce_both_files_and_lose_neither() {
    let mut swarm = swarm();

    // Everyone starts from a shared base, so the divergence is genuine.
    swarm
        .device(LAPTOP)
        .write("notes.txt", b"shared base")
        .unwrap();
    swarm.settle_and_check().unwrap();

    // Partition: each device edits in isolation.
    swarm.set_online(PI, false);
    swarm
        .device(LAPTOP)
        .write("notes.txt", b"laptop version")
        .unwrap();
    swarm.device(LAPTOP).publish().unwrap();

    swarm.set_online(LAPTOP, false);
    swarm.set_online(PI, true);
    swarm.device(PI).write("notes.txt", b"pi version").unwrap();
    swarm.device(PI).publish().unwrap();

    // Heal.
    swarm.set_online(LAPTOP, true);
    swarm.settle_and_check().unwrap();

    let listing = swarm.agreed_listing().unwrap();
    assert_eq!(
        listing.len(),
        2,
        "expected the original path plus one conflict sibling, got {listing:?}"
    );

    // Both bodies survive somewhere, on every device.
    for device in [LAPTOP, PI, VM] {
        let mut bodies: Vec<Vec<u8>> = listing
            .iter()
            .map(|path| swarm.device(device).read(path).unwrap().unwrap())
            .collect();
        bodies.sort();

        assert_eq!(
            bodies,
            vec![b"laptop version".to_vec(), b"pi version".to_vec()],
            "device {device} lost one of two concurrent edits"
        );
    }

    assert!(
        listing
            .iter()
            .any(|p| p.contains(conflict::CONFLICT_MARKER)),
        "the losing version was not materialised as a conflict sibling: {listing:?}"
    );
}

#[test]
fn a_three_way_conflict_produces_three_distinct_files() {
    let mut swarm = swarm();

    swarm.device(LAPTOP).write("shared.txt", b"base").unwrap();
    swarm.settle_and_check().unwrap();

    // Full partition: all three edit alone.
    for device in [LAPTOP, PI, VM] {
        swarm.set_online(device, false);
    }
    swarm.device(LAPTOP).write("shared.txt", b"laptop").unwrap();
    swarm.device(PI).write("shared.txt", b"pi").unwrap();
    swarm.device(VM).write("shared.txt", b"vm").unwrap();

    // Heal all at once.
    for device in [LAPTOP, PI, VM] {
        swarm.set_online(device, true);
    }
    swarm.settle_and_check().unwrap();

    let listing = swarm.agreed_listing().unwrap();
    assert_eq!(
        listing.len(),
        3,
        "a three-way conflict should leave three files, got {listing:?}"
    );

    let mut bodies: Vec<Vec<u8>> = listing
        .iter()
        .map(|path| swarm.device(LAPTOP).read(path).unwrap().unwrap())
        .collect();
    bodies.sort();
    assert_eq!(
        bodies,
        vec![b"laptop".to_vec(), b"pi".to_vec(), b"vm".to_vec()],
        "a three-way conflict lost one of the three versions"
    );
}

#[test]
fn a_sequential_edit_is_not_treated_as_a_conflict() {
    // The common case must not produce conflict siblings, or every ordinary
    // edit would litter the user's folder.
    let swarm = swarm();

    swarm.device(LAPTOP).write("doc.txt", b"first").unwrap();
    swarm.settle_and_check().unwrap();

    swarm
        .device(PI)
        .write("doc.txt", b"second, having seen the first")
        .unwrap();
    swarm.settle_and_check().unwrap();

    let listing = swarm.agreed_listing().unwrap();
    assert_eq!(
        listing,
        vec!["doc.txt".to_owned()],
        "an ordinary sequential edit created a conflict sibling: {listing:?}"
    );
    assert_eq!(
        swarm.device(LAPTOP).read("doc.txt").unwrap().unwrap(),
        b"second, having seen the first"
    );
}

#[test]
fn version_vectors_order_sequential_writes_and_flag_concurrent_ones() {
    // A direct check on the primitive the rest of this file depends on, at the
    // level of a real store rather than in isolation.
    let mut swarm = swarm();

    swarm.device(LAPTOP).write("v.txt", b"one").unwrap();
    swarm.settle_and_check().unwrap();
    let base = swarm.device(LAPTOP).store().stat("v.txt").unwrap().unwrap();

    swarm.set_online(PI, false);
    swarm.device(LAPTOP).write("v.txt", b"laptop next").unwrap();
    let sequential = swarm.device(LAPTOP).store().stat("v.txt").unwrap().unwrap();
    swarm.device(PI).write("v.txt", b"pi next").unwrap();
    let concurrent = swarm.device(PI).store().stat("v.txt").unwrap().unwrap();

    assert_eq!(
        base.version.compare(&sequential.version),
        CausalOrder::Before
    );
    assert_eq!(
        sequential.version.compare(&concurrent.version),
        CausalOrder::Concurrent,
        "two edits made during a partition were not detected as concurrent, so \
         one of them is about to be silently discarded"
    );
}
