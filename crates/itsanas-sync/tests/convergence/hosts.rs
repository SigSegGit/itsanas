use itsanas_sync::sim::Swarm;

use super::{LAPTOP, PI, swarm};

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
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

#[test]
fn red_team_an_armed_roster_applies_no_chain_from_a_key_the_account_never_claimed() {
    // HANDOVER §10 7. A host holding a genuine segment of this account can
    // name a free keypair of its own as the signer: the envelope verifies,
    // the body opens, and the owner replays its own operations -- every
    // release undone. The engine cannot tell a re-signed body from an honest
    // one; it can tell a signer the account never claimed. The Pi stands for
    // that signer here: until the laptop's roster names it, nothing it signed
    // is applied, and once a claim admits it, everything is.
    let swarm = swarm();
    let laptop = swarm.device(LAPTOP).store();
    laptop.admit_member(&laptop.device_id(), false).unwrap();

    swarm.device(PI).write("plan.txt", b"from the Pi").unwrap();
    swarm.device(PI).publish().unwrap();
    let report = swarm.device(LAPTOP).sync().unwrap();
    assert_eq!(
        report.adopted, 0,
        "a chain from an unclaimed key was applied: {report:?}"
    );
    assert_eq!(report.unrostered_chains, 1, "{report:?}");
    assert!(swarm.device(LAPTOP).read("plan.txt").unwrap().is_none());

    let pi = swarm.device(PI).store().device_id();
    laptop.admit_member(&pi, false).unwrap();
    swarm.device(LAPTOP).sync().unwrap();
    assert_eq!(
        swarm.device(LAPTOP).read("plan.txt").unwrap().as_deref(),
        Some(&b"from the Pi"[..]),
        "a sibling admitted later was still left out"
    );

    // A revocation is final: the Pi's later work is no longer applied.
    laptop.admit_member(&pi, true).unwrap();
    laptop.admit_member(&pi, false).unwrap();
    swarm
        .device(PI)
        .write("after.txt", b"after the theft")
        .unwrap();
    swarm.device(PI).publish().unwrap();
    swarm.device(LAPTOP).sync().unwrap();
    assert!(
        swarm.device(LAPTOP).read("after.txt").unwrap().is_none(),
        "a withdrawn device was readmitted by a later claim"
    );
}

#[test]
fn an_unarmed_roster_gates_nothing() {
    // A store no claim was ever admitted to is a test fixture or a tool, not
    // a node: `Node::open` arms it. Gating there would refuse every sibling.
    let swarm = swarm();
    swarm.device(PI).write("plan.txt", b"from the Pi").unwrap();
    swarm.device(PI).publish().unwrap();
    swarm.device(LAPTOP).sync().unwrap();
    assert!(swarm.device(LAPTOP).read("plan.txt").unwrap().is_some());
}
