//! A device replaying an account's log from nothing: a restore, a new phone, a
//! replacement disk (ROADMAP, "A restore depends on chunks that garbage
//! collection will take").

use std::cell::RefCell;
use std::collections::BTreeSet;

use itsanas_crypto::{ChunkId, UserId};
use itsanas_sync::sim::Swarm;
use itsanas_sync::{ChunkSource, Result, apply_segments};

const WRITER: usize = 0;
const NEWCOMER: usize = 1;

/// The cloud as it stands once the writer collected its garbage: the chunks of
/// superseded and deleted versions are gone, and each one asked is noted.
struct Collected<'a> {
    cloud: &'a dyn ChunkSource,
    gone: BTreeSet<ChunkId>,
    asked_for_gone: RefCell<usize>,
}

impl ChunkSource for Collected<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> Result<Option<Vec<u8>>> {
        if self.gone.contains(address) {
            *self.asked_for_gone.borrow_mut() += 1;
            return Ok(None);
        }
        self.cloud.fetch(owner, address)
    }
}

/// A file edited twice and a file deleted: a device that replays the log after
/// the old chunks were collected applies the last operation of each path in one
/// round, and asks no host for a chunk only an earlier operation named.
/// Sabotage: apply every entry in log order (`apply_replaying` without its
/// second pass): the old version is fetched, found nowhere, and the round
/// defers it.
#[test]
fn a_replay_applies_the_last_operation_of_each_path_and_fetches_nothing_older() {
    let mut swarm = Swarm::new(2).expect("a two-device swarm");
    swarm.set_online(NEWCOMER, false);
    let writer = swarm.device(WRITER);
    let chunks_of = |path: &str| -> Vec<ChunkId> {
        writer.store().stat(path).unwrap().expect("written").chunks
    };

    writer
        .write("notes.txt", b"the first version of the notes")
        .unwrap();
    writer.publish().unwrap();
    let mut gone: BTreeSet<ChunkId> = chunks_of("notes.txt").into_iter().collect();
    writer
        .write("notes.txt", b"the second version, the one that stays")
        .unwrap();
    writer.publish().unwrap();
    writer.write("draft.txt", b"a draft deleted later").unwrap();
    writer.publish().unwrap();
    gone.extend(chunks_of("draft.txt"));
    assert!(writer.remove("draft.txt").unwrap());
    writer.publish().unwrap();
    for kept in chunks_of("notes.txt") {
        gone.remove(&kept);
    }

    swarm.set_online(NEWCOMER, true);
    let newcomer = swarm.device(NEWCOMER);
    let segments = swarm.cloud().with(|cloud| cloud.segments().to_vec());
    let source = Collected {
        cloud: swarm.cloud(),
        gone,
        asked_for_gone: RefCell::new(0),
    };
    let (report, _) = apply_segments(newcomer.store(), &segments, &source).unwrap();

    assert_eq!(
        newcomer.read("notes.txt").unwrap().as_deref(),
        Some(&b"the second version, the one that stays"[..]),
        "the replay did not end on the last version of an edited file"
    );
    assert_eq!(
        newcomer.read("draft.txt").unwrap(),
        None,
        "a file deleted in the log came back on the device that replayed it"
    );
    assert_eq!(
        *source.asked_for_gone.borrow(),
        0,
        "the replay asked hosts for chunks only a superseded or deleted version named: a restore downloads the account's whole history"
    );
    assert!(
        !report.needs_another_round(),
        "a replay whose old chunks were collected deferred them, so it never reads as finished: {report:?}"
    );
}

/// The same across machines: a file the laptop wrote and the phone edited
/// after it, replayed by a third machine once the laptop's version was
/// collected, ends on the phone's version in one round and asks for nothing of
/// the laptop's. Sabotage: as above.
#[test]
fn a_version_another_machine_superseded_is_not_fetched_by_a_replay() {
    let mut swarm = Swarm::new(3).expect("a three-device swarm");
    swarm.set_online(2, false);
    // The laptop is the device whose chain the engine reads first, so log
    // order alone would fetch its version before the phone's supersedes it.
    let (laptop, phone) =
        if swarm.device(0).store().device_id() < swarm.device(1).store().device_id() {
            (swarm.device(0), swarm.device(1))
        } else {
            (swarm.device(1), swarm.device(0))
        };
    laptop.write("plan.txt", b"the laptop's plan").unwrap();
    laptop.publish().unwrap();
    let gone: BTreeSet<ChunkId> = laptop
        .store()
        .stat("plan.txt")
        .unwrap()
        .expect("written")
        .chunks
        .into_iter()
        .collect();
    phone.sync().unwrap();
    phone
        .write("plan.txt", b"the plan as the phone left it")
        .unwrap();
    phone.publish().unwrap();

    swarm.set_online(2, true);
    let segments = swarm.cloud().with(|cloud| cloud.segments().to_vec());
    let source = Collected {
        cloud: swarm.cloud(),
        gone,
        asked_for_gone: RefCell::new(0),
    };
    let (report, _) = apply_segments(swarm.device(2).store(), &segments, &source).unwrap();

    assert_eq!(
        swarm.device(2).read("plan.txt").unwrap().as_deref(),
        Some(&b"the plan as the phone left it"[..]),
        "the replay did not end on the version another machine wrote last"
    );
    assert_eq!(
        *source.asked_for_gone.borrow(),
        0,
        "the replay asked for the chunks of a version another machine had superseded"
    );
    assert!(!report.needs_another_round(), "{report:?}");
}
