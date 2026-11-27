use super::{LAPTOP, PI, swarm};

// ---------------------------------------------------------------------------
// The disk bound on pulls (HANDOVER 8.1b)
// ---------------------------------------------------------------------------

/// A disk ceiling for the account on `device`, as `Node::bound_writes` sets it.
fn ceiling(device: &itsanas_sync::sim::SimDevice, bytes: Option<u64>) {
    device
        .store()
        .set_write_budget(Some(itsanas_store::WriteBudget {
            allowed: u64::MAX,
            elsewhere: 0,
            local_ceiling: bytes,
            folder_copy: itsanas_store::FolderCopy::None,
        }))
        .unwrap();
}

fn varied(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| u8::try_from(i * 31 % 251).unwrap_or(0))
        .collect()
}

#[test]
fn red_team_a_pull_past_the_disk_ceiling_fetches_nothing_and_waits() {
    // The laptop has room for 100 KB of the account; the rest of its disk is
    // owed to the people its pledge hosts. The Pi wrote 300 KB. Downloading it
    // anyway fills space promised to others, and the promise is only found
    // broken the day their data arrives.
    let swarm = swarm();
    swarm.device(PI).write("big.bin", &varied(300_000)).unwrap();
    swarm.device(PI).publish().unwrap();
    let chunks = swarm
        .device(PI)
        .store()
        .stat("big.bin")
        .unwrap()
        .unwrap()
        .chunks;

    ceiling(swarm.device(LAPTOP), Some(100_000));
    let report = swarm.device(LAPTOP).sync().unwrap();

    assert_eq!(
        report.no_room, 1,
        "a file past the disk ceiling was not refused: {report:?}"
    );
    assert!(
        report.needs_another_round(),
        "a file refused for room must be retried once room appears, not dropped"
    );
    assert_eq!(swarm.device(LAPTOP).list().unwrap(), Vec::<String>::new());
    assert!(
        chunks
            .iter()
            .all(|chunk| !swarm.device(LAPTOP).store().has_chunk(chunk)),
        "chunks of a refused file were fetched: they fill the disk with no \
         index entry to count them"
    );

    ceiling(swarm.device(LAPTOP), Some(400_000));
    let report = swarm.device(LAPTOP).sync().unwrap();
    assert_eq!(
        report.adopted, 1,
        "room appeared and the file still did not come: {report:?}"
    );
    assert_eq!(
        swarm.device(LAPTOP).read("big.bin").unwrap().unwrap(),
        varied(300_000)
    );
}

#[test]
fn red_team_a_conflict_is_charged_both_versions_against_the_disk() {
    // A conflict keeps both files, so the incoming one frees nothing. Charged
    // only its growth over ours, a 300 KB version against our 1 KB one would
    // pass a ceiling that has room for 1 KB more.
    let mut swarm = swarm();
    swarm.set_online(LAPTOP, false);
    swarm.device(LAPTOP).write("a.bin", &varied(1_000)).unwrap();
    swarm.device(PI).write("a.bin", &varied(300_000)).unwrap();
    swarm.device(PI).publish().unwrap();
    swarm.set_online(LAPTOP, true);

    ceiling(swarm.device(LAPTOP), Some(300_500));
    let report = swarm.device(LAPTOP).sync().unwrap();

    assert_eq!(
        report.no_room, 1,
        "the incoming side of a conflict was charged as a replacement: {report:?}"
    );
    assert_eq!(report.conflicted, 0);
    assert_eq!(
        swarm.device(LAPTOP).list().unwrap(),
        vec!["a.bin".to_owned()]
    );

    // Room for exactly both: the conflict resolves. Then a disk left with no
    // room at all: hosts re-serve the same segment every round, and a conflict
    // already resolved costs nothing more -- it must read as known, not as a
    // file refused for room, or the round never finishes.
    ceiling(swarm.device(LAPTOP), Some(301_000));
    assert_eq!(swarm.device(LAPTOP).sync().unwrap().conflicted, 1);
    ceiling(swarm.device(LAPTOP), Some(301_000));
    let again = swarm.device(LAPTOP).sync().unwrap();
    assert_eq!(
        again.no_room, 0,
        "a conflict resolved earlier was refused for room on the replay: {again:?}"
    );
    assert!(!again.needs_another_round(), "{again:?}");
}
