use super::{LAPTOP, PI, VM, swarm};

// ---------------------------------------------------------------------------
// Deletes
// ---------------------------------------------------------------------------

#[test]
fn a_delete_racing_an_edit_never_destroys_the_edit() {
    // Asymmetric on purpose. An unexpected resurrection costs the user one
    // second; a lost edit is unrecoverable.
    let mut swarm = swarm();

    swarm
        .device(LAPTOP)
        .write("contested.txt", b"original")
        .unwrap();
    swarm.settle_and_check().unwrap();

    swarm.set_online(PI, false);
    swarm.device(LAPTOP).remove("contested.txt").unwrap();
    swarm.device(LAPTOP).publish().unwrap();

    // The Pi, not having seen the delete, edits the file.
    swarm
        .device(PI)
        .write("contested.txt", b"edited while apart")
        .unwrap();

    swarm.set_online(PI, true);
    swarm.settle_and_check().unwrap();

    assert_eq!(
        swarm.device(LAPTOP).read("contested.txt").unwrap(),
        Some(b"edited while apart".to_vec()),
        "a delete concurrent with an edit destroyed the edit"
    );
    assert_eq!(
        swarm.device(PI).read("contested.txt").unwrap(),
        Some(b"edited while apart".to_vec())
    );
}

#[test]
fn a_delete_that_saw_the_edit_is_honoured() {
    // The counterpart to the rule above: a delete is only overridden when it
    // genuinely raced. A normal delete must actually delete, or the product is
    // useless.
    let swarm = swarm();

    swarm
        .device(LAPTOP)
        .write("doomed.txt", b"content")
        .unwrap();
    swarm.settle_and_check().unwrap();

    swarm.device(PI).write("doomed.txt", b"edited").unwrap();
    swarm.settle_and_check().unwrap();

    // The laptop has seen the Pi's edit, and now deletes.
    swarm.device(LAPTOP).remove("doomed.txt").unwrap();
    swarm.settle_and_check().unwrap();

    for device in [LAPTOP, PI, VM] {
        assert_eq!(
            swarm.device(device).read("doomed.txt").unwrap(),
            None,
            "device {device} still holds a file that was deliberately deleted"
        );
    }
    assert_eq!(swarm.agreed_listing().unwrap(), Vec::<String>::new());
}

#[test]
fn an_offline_device_does_not_resurrect_a_file_deleted_while_it_slept() {
    // Without tombstones the returning device re-announces the file it still
    // holds and it comes back from the dead on every machine.
    let mut swarm = swarm();

    swarm
        .device(LAPTOP)
        .write("temp.txt", b"delete me")
        .unwrap();
    swarm.settle_and_check().unwrap();

    // The VM goes to sleep still holding the file.
    swarm.set_online(VM, false);

    swarm.device(LAPTOP).remove("temp.txt").unwrap();
    swarm.settle().unwrap();

    // The VM returns, makes no edit, and simply syncs.
    swarm.set_online(VM, true);
    swarm.settle_and_check().unwrap();

    for device in [LAPTOP, PI, VM] {
        assert_eq!(
            swarm.device(device).read("temp.txt").unwrap(),
            None,
            "device {device} resurrected a deleted file"
        );
    }
}

#[test]
fn re_creating_a_deleted_file_works_and_converges() {
    let swarm = swarm();

    swarm
        .device(LAPTOP)
        .write("phoenix.txt", b"first life")
        .unwrap();
    swarm.settle_and_check().unwrap();
    swarm.device(LAPTOP).remove("phoenix.txt").unwrap();
    swarm.settle_and_check().unwrap();

    swarm
        .device(PI)
        .write("phoenix.txt", b"second life")
        .unwrap();
    swarm.settle_and_check().unwrap();

    for device in [LAPTOP, PI, VM] {
        assert_eq!(
            swarm.device(device).read("phoenix.txt").unwrap().unwrap(),
            b"second life",
            "device {device} did not accept the re-created file"
        );
    }
    assert_eq!(
        swarm.agreed_listing().unwrap(),
        vec!["phoenix.txt".to_owned()]
    );
}

/// Two machines delete the same file apart, and a third hears of both. Each
/// delete replaced the other's trace, so the third swapped between them every
/// round and the account never settled: every round sent the traces again
/// (found 2026-11-24 while writing the trace-keeping choice, whose two
/// machines replace one file apart). The two traces are now one, the union of
/// what both deletes saw. Sabotage: adopt the incoming trace when concurrent.
#[test]
fn red_team_two_machines_deleting_the_same_file_apart_still_settle() {
    let swarm = swarm();
    swarm.device(LAPTOP).write("same.txt", b"bytes").unwrap();
    swarm.settle_and_check().unwrap();
    swarm.device(PI).remove("same.txt").unwrap();
    swarm.device(LAPTOP).remove("same.txt").unwrap();
    swarm
        .settle_and_check()
        .expect("two deletes made apart left the account swapping traces for ever");
    for device in [LAPTOP, PI, VM] {
        assert!(
            swarm.device(device).read("same.txt").unwrap().is_none(),
            "a file both machines deleted came back on device {device}"
        );
    }
}
