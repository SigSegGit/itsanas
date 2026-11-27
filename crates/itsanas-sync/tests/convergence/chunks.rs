use itsanas_sync::sim::Cloud;

use super::{LAPTOP, PI, VM, swarm};

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
fn red_team_a_chunk_that_does_not_match_its_address_leaves_the_file_deferred() {
    // THE ATTACK. The only reachable host answers every fetch with noise of
    // the right length. `accept_chunk` refuses the noise, and the engine used
    // to count the refused chunk as fetched: the file was adopted with a hole
    // in it and the round reported nothing left to do, so a session moved its
    // markers past the segment and never asked for the real bytes again.
    let mut swarm = swarm();

    swarm.set_online(LAPTOP, false);
    swarm.set_online(VM, false);
    swarm
        .device(PI)
        .write("thesis.txt", b"the only draft, on the Pi")
        .unwrap();
    swarm.device(PI).publish().unwrap();
    swarm.cloud().with(Cloud::corrupt_all_chunks);

    swarm.set_online(LAPTOP, true);
    let report = swarm.device(LAPTOP).sync().unwrap();

    assert_eq!(
        report.adopted, 0,
        "a file was adopted from chunks that failed to open; the round said it \
         was finished and nothing would fetch the real bytes: {report:?}"
    );
    assert!(
        report.needs_another_round(),
        "noise in place of a chunk must leave the file to a later round: {report:?}"
    );
    assert_eq!(swarm.device(LAPTOP).list().unwrap(), Vec::<String>::new());

    // And the honest bytes, once served, complete it.
    swarm.device(PI).publish().unwrap();
    swarm.device(LAPTOP).sync().unwrap();
    assert_eq!(
        swarm.device(LAPTOP).read("thesis.txt").unwrap().unwrap(),
        b"the only draft, on the Pi"
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
