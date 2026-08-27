//! Convergence under adversarial conditions.
//!
//! These are the M3 exit criteria from `docs/ROADMAP.md`. The property under
//! test throughout is the same one: **devices that eventually exchange
//! everything end up byte-identical**, regardless of ordering, partitions, or
//! devices that never come back.
//!
//! Every scenario here is deterministic. No randomness, no wall clock, no
//! sleeping. A failure reproduces exactly.

use itsanas_crypto::MasterSecret;
use itsanas_store::CausalOrder;
use itsanas_sync::{
    conflict,
    sim::{Cloud, Swarm},
};

/// Indices used throughout, matching the three real target machines.
const LAPTOP: usize = 0;
const PI: usize = 1;
const VM: usize = 2;

fn swarm() -> Swarm {
    Swarm::new(3).expect("a three-device swarm")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

// ---------------------------------------------------------------------------
// The headline scenario
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Unavailable chunks
// ---------------------------------------------------------------------------

#[test]
fn an_operation_whose_chunks_are_unavailable_is_deferred_not_half_applied() {
    // A segment can reach a device before the chunks it names do. Materialising
    // the file anyway would produce an index entry pointing at chunks that are
    // not there — a file that exists but cannot be read.
    let mut swarm = swarm();

    swarm.set_online(LAPTOP, false);
    swarm.set_online(VM, false);
    swarm
        .device(PI)
        .write("big.txt", b"content that lives only on the Pi")
        .unwrap();
    swarm.device(PI).publish().unwrap();

    // Strip the chunks from the cloud, leaving only the segment. This is the
    // real situation where the only host holding a chunk went offline between
    // the segment arriving and the fetch.
    swarm.cloud().with(Cloud::forget_all_chunks);

    swarm.set_online(LAPTOP, true);
    let report = swarm.device(LAPTOP).sync().unwrap();

    assert!(
        report.deferred > 0,
        "an operation with unavailable chunks was not deferred: {report:?}"
    );
    assert_eq!(report.adopted, 0);
    assert!(
        report.needs_another_round(),
        "a deferred operation must ask to be retried"
    );
    assert_eq!(
        swarm.device(LAPTOP).list().unwrap(),
        Vec::<String>::new(),
        "a file was materialised whose content is not available; reading it \
         would fail"
    );
}

#[test]
fn a_deferred_operation_completes_once_its_chunks_show_up() {
    let mut swarm = swarm();

    swarm.set_online(LAPTOP, false);
    swarm.set_online(VM, false);
    swarm
        .device(PI)
        .write("later.txt", b"arrives eventually")
        .unwrap();
    swarm.device(PI).publish().unwrap();
    swarm.cloud().with(Cloud::forget_all_chunks);

    swarm.set_online(LAPTOP, true);
    assert!(swarm.device(LAPTOP).sync().unwrap().deferred > 0);

    // The Pi comes back and republishes its chunks.
    swarm.device(PI).publish().unwrap();

    let report = swarm.device(LAPTOP).sync().unwrap();
    assert!(report.adopted > 0, "the retry did not complete: {report:?}");
    assert_eq!(
        swarm.device(LAPTOP).read("later.txt").unwrap().unwrap(),
        b"arrives eventually"
    );
}

// ---------------------------------------------------------------------------
// What the hosts can see
// ---------------------------------------------------------------------------

#[test]
fn the_hosts_hold_everything_and_can_read_none_of_it() {
    // The devices here belong to Alice, whose canary is published. Every byte
    // that reached the simulated hosts is scanned for it.
    let alice = itsanas_testkit::alice();
    let swarm = Swarm::with_master(3, &alice.master).unwrap();

    for file in &alice.files {
        swarm
            .device(LAPTOP)
            .write(file.path, &file.content)
            .unwrap();
    }
    swarm.settle_and_check().unwrap();

    let held = swarm.cloud().with(|cloud| cloud.all_bytes());
    assert!(
        !held.is_empty(),
        "the hosts hold nothing, so this scan proves nothing"
    );

    assert!(
        !contains(&held, itsanas_testkit::ALICE_CANARY.as_bytes()),
        "Alice's plaintext was found on a host; the hosts can read their \
         peers' data and the entire premise fails"
    );

    // Filenames must not leak either — they are inside the sealed segment body.
    for file in &alice.files {
        assert!(
            !contains(&held, file.path.as_bytes()),
            "the path {:?} appeared in plaintext on a host, so hosts learn what \
             their peers store",
            file.path
        );
    }

    // The vacuity guard: the canary really is in the data that was stored.
    let plaintext: Vec<u8> = alice.files.iter().flat_map(|f| f.content.clone()).collect();
    assert!(contains(
        &plaintext,
        itsanas_testkit::ALICE_CANARY.as_bytes()
    ));
}

#[test]
fn every_segment_a_host_holds_is_verifiable_by_that_host() {
    // Hosts cannot read segments, but they must be able to authenticate them —
    // otherwise anyone could flood a host with garbage attributed to a peer.
    let swarm = swarm();

    swarm.device(LAPTOP).write("a.txt", b"one").unwrap();
    swarm.device(PI).write("b.txt", b"two").unwrap();
    swarm.settle_and_check().unwrap();

    swarm.cloud().with(|cloud| {
        assert!(cloud.segment_count() > 0, "nothing was published");
        for segment in cloud.segments() {
            segment
                .verify_signature()
                .expect("a host must be able to verify what it stores");
        }
    });
}

// ---------------------------------------------------------------------------
// Full-corpus convergence
// ---------------------------------------------------------------------------

#[test]
fn a_full_corpus_converges_across_three_devices_with_partitions() {
    // The realistic end-to-end case: a real data set, real chunking, real
    // sealing, spread across three devices that are never all online together.
    let alice = itsanas_testkit::alice();
    let mut swarm = Swarm::with_master(3, &alice.master).unwrap();

    for (index, file) in alice.files.iter().enumerate() {
        let device = index % 3;
        // Only this device is online when it writes.
        for other in [LAPTOP, PI, VM] {
            swarm.set_online(other, other == device);
        }
        swarm
            .device(device)
            .write(file.path, &file.content)
            .unwrap();
        swarm.device(device).publish().unwrap();
    }

    for device in [LAPTOP, PI, VM] {
        swarm.set_online(device, true);
    }
    swarm.settle_and_check().unwrap();

    for device in [LAPTOP, PI, VM] {
        for file in &alice.files {
            assert_eq!(
                swarm.device(device).read(file.path).unwrap().as_deref(),
                Some(file.content.as_slice()),
                "device {device} does not have {} byte-identical",
                file.path
            );
        }
    }

    assert_eq!(
        swarm.agreed_listing().unwrap().len(),
        alice.files.len(),
        "the converged tree has the wrong number of files"
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
