//! Convergence under adversarial conditions.
//!
//! These are the M3 exit criteria from `docs/ROADMAP.md`. The property under
//! test throughout is the same one: **devices that eventually exchange
//! everything end up byte-identical**, regardless of ordering, partitions, or
//! devices that never come back.
//!
//! Every scenario here is deterministic. No randomness, no wall clock, no
//! sleeping. A failure reproduces exactly.

mod availability;
mod chunks;
mod conflicts;
mod deletes;
mod disk_bound;
mod hosts;
mod ordering;

use itsanas_sync::sim::Swarm;

/// Indices used throughout, matching the three real target machines.
const LAPTOP: usize = 0;
const PI: usize = 1;
const VM: usize = 2;

fn swarm() -> Swarm {
    Swarm::new(3).expect("a three-device swarm")
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
