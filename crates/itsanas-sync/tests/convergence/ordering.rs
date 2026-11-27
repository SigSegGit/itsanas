use itsanas_crypto::MasterSecret;
use itsanas_sync::sim::Swarm;

use super::{LAPTOP, PI, VM, swarm};

// ---------------------------------------------------------------------------
// Ordering, idempotence and repetition
// ---------------------------------------------------------------------------

#[test]
fn the_final_state_does_not_depend_on_the_order_devices_sync_in() {
    // Runs the same divergence twice, healing in opposite orders. If the merge
    // rules were order-dependent the two runs would end differently — and in
    // production that difference would be permanent and silent.
    fn run(heal_pi_first: bool) -> Vec<(String, Vec<u8>)> {
        let mut swarm = Swarm::with_master(3, &MasterSecret::from_bytes([0x77; 32])).unwrap();

        swarm.device(LAPTOP).write("doc.txt", b"base").unwrap();
        swarm.settle_and_check().unwrap();

        for device in [LAPTOP, PI, VM] {
            swarm.set_online(device, false);
        }
        swarm
            .device(LAPTOP)
            .write("doc.txt", b"from laptop")
            .unwrap();
        swarm.device(PI).write("doc.txt", b"from pi").unwrap();

        if heal_pi_first {
            swarm.set_online(PI, true);
            swarm.device(PI).publish().unwrap();
            swarm.set_online(LAPTOP, true);
        } else {
            swarm.set_online(LAPTOP, true);
            swarm.device(LAPTOP).publish().unwrap();
            swarm.set_online(PI, true);
        }
        swarm.set_online(VM, true);
        swarm.settle_and_check().unwrap();

        let mut state: Vec<(String, Vec<u8>)> = swarm
            .agreed_listing()
            .unwrap()
            .into_iter()
            .map(|path| {
                let body = swarm.device(LAPTOP).read(&path).unwrap().unwrap();
                (path, body)
            })
            .collect();
        state.sort();
        state
    }

    assert_eq!(
        run(true),
        run(false),
        "the converged state depends on which device healed first; two real \
         deployments would silently disagree forever"
    );
}

#[test]
fn syncing_repeatedly_changes_nothing() {
    // Hosts re-serve segments freely; there is no acknowledgement telling one
    // to stop. Applying the same operation twice must be a no-op.
    let swarm = swarm();

    swarm.device(LAPTOP).write("a.txt", b"one").unwrap();
    swarm.device(PI).write("b.txt", b"two").unwrap();
    swarm.settle_and_check().unwrap();

    let before = swarm.agreed_listing().unwrap();

    for _ in 0..5 {
        for device in [LAPTOP, PI, VM] {
            let report = swarm.device(device).sync().unwrap();
            assert!(
                !report.changed_anything(),
                "re-syncing changed state on device {device}: {report:?}"
            );
        }
    }

    assert_eq!(swarm.agreed_listing().unwrap(), before);
}

#[test]
fn re_resolving_a_conflict_is_idempotent() {
    // The specific failure this guards: a conflict that is re-resolved on every
    // round makes a settle loop that stops when nothing changes never stop.
    let mut swarm = swarm();

    swarm.device(LAPTOP).write("c.txt", b"base").unwrap();
    swarm.settle_and_check().unwrap();

    swarm.set_online(PI, false);
    swarm.device(LAPTOP).write("c.txt", b"laptop").unwrap();
    swarm.device(LAPTOP).publish().unwrap();
    swarm.set_online(LAPTOP, false);
    swarm.set_online(PI, true);
    swarm.device(PI).write("c.txt", b"pi").unwrap();
    swarm.device(PI).publish().unwrap();
    swarm.set_online(LAPTOP, true);

    swarm.settle_and_check().unwrap();
    let after_first = swarm.agreed_listing().unwrap();

    for _ in 0..3 {
        for device in [LAPTOP, PI, VM] {
            let report = swarm.device(device).sync().unwrap();
            assert_eq!(
                report.conflicted, 0,
                "device {device} re-resolved a conflict that was already settled"
            );
        }
    }

    assert_eq!(swarm.agreed_listing().unwrap(), after_first);
}

/// A settled two-device conflict on `c.txt`: the swarm, the device whose
/// version kept the original path, and the conflict copy's path.
fn settled_conflict() -> (Swarm, usize, String) {
    let mut swarm = swarm();
    swarm.device(LAPTOP).write("c.txt", b"base").unwrap();
    swarm.settle_and_check().unwrap();

    swarm.set_online(PI, false);
    swarm.device(LAPTOP).write("c.txt", b"laptop").unwrap();
    swarm.device(LAPTOP).publish().unwrap();
    swarm.set_online(LAPTOP, false);
    swarm.set_online(PI, true);
    swarm.device(PI).write("c.txt", b"pi").unwrap();
    swarm.device(PI).publish().unwrap();
    swarm.set_online(LAPTOP, true);
    swarm.settle_and_check().unwrap();

    let winner = match swarm.device(LAPTOP).read("c.txt").unwrap().as_deref() {
        Some(b"laptop") => LAPTOP,
        _ => PI,
    };
    let sibling = swarm
        .agreed_listing()
        .unwrap()
        .into_iter()
        .find(|path| path != "c.txt")
        .expect("the conflict left no copy");
    (swarm, winner, sibling)
}

#[test]
fn red_team_a_replayed_conflict_keeps_an_edit_made_to_the_conflict_copy() {
    // Hosts re-serve the loser's segment every round. On the device that kept
    // the original path, the copy was then edited: the replay must not put the
    // old version back over that edit.
    let (swarm, winner, sibling) = settled_conflict();
    swarm.device(winner).write(&sibling, b"edited").unwrap();

    let report = swarm.device(winner).sync().unwrap();
    assert_eq!(report.conflicted, 0, "the replay re-resolved the conflict");
    assert_eq!(
        swarm.device(winner).read(&sibling).unwrap().as_deref(),
        Some(&b"edited"[..]),
        "a replayed segment overwrote the edited conflict copy"
    );
}

#[test]
fn red_team_a_replayed_conflict_does_not_bring_back_a_deleted_conflict_copy() {
    let (swarm, winner, sibling) = settled_conflict();
    assert!(swarm.device(winner).remove(&sibling).unwrap());

    let report = swarm.device(winner).sync().unwrap();
    assert_eq!(report.conflicted, 0, "the replay re-resolved the conflict");
    assert_eq!(
        swarm.device(winner).read(&sibling).unwrap(),
        None,
        "a replayed segment brought a deleted conflict copy back"
    );
}

#[test]
fn a_long_run_of_alternating_partitions_still_converges() {
    // Ten rounds of "one device is away, the other two work", rotating which is
    // away. Nothing here is exotic; it is simply more history than a hand-built
    // scenario covers, and the point is that the invariant holds throughout.
    let mut swarm = swarm();

    for round in 0..10 {
        let away = round % 3;
        swarm.set_online(away, false);

        for device in [LAPTOP, PI, VM] {
            if device != away {
                swarm
                    .device(device)
                    .write(
                        &format!("round-{round}/device-{device}.txt"),
                        format!("round {round} from device {device}").as_bytes(),
                    )
                    .unwrap();
            }
        }

        swarm.settle().unwrap();
        swarm.set_online(away, true);
        swarm.settle_and_check().unwrap();
    }

    let listing = swarm.agreed_listing().unwrap();
    assert_eq!(
        listing.len(),
        20,
        "expected two files per round for ten rounds, got {}",
        listing.len()
    );

    for device in [LAPTOP, PI, VM] {
        for path in &listing {
            assert!(
                swarm.device(device).read(path).unwrap().is_some(),
                "device {device} is missing {path} after ten partition rounds"
            );
        }
    }
}
