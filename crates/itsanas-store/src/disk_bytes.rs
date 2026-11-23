//! The bytes of what this crate writes to disk, pinned (HANDOVER §8 17).
//!
//! #435 added a field to a structure and broke three places that read it back,
//! one of them found by chance in a log: the wire was pinned, nothing pinned
//! what a node writes. A node upgraded in place opens the index, the
//! segments and the protected list its previous release wrote; a format that
//! changed without a reader for the old one is a node that no longer starts,
//! or one that drops what it could not read.
//!
//! Each row folds one sample of a type, written with postcard (FNV-1a over
//! the bytes: postcard writes fields in declaration order, so the figure is
//! the same on every build). A row whose figure changes is a format change:
//! read the old form too (a versioned decode, as the address book's
//! `BookFileV1`) or tolerate it (unreadable = absent, with a test that an old
//! row does not stop the read), then add a row for the new form. Never edit a
//! row that shipped.

use std::collections::BTreeMap;

use itsanas_crypto::{ChunkId, DeviceId, ObjectId, Signature, UserId};
use itsanas_placement::uptime::Uptime;
use serde::Serialize;

use crate::{
    LocalState, Reliability, VersionVector,
    oplog::{FileEntry, LogEntry, Operation, SegmentBody, SegmentEnvelope, Tombstone},
};

fn folded<T: Serialize>(value: &T) -> u64 {
    itsanas_testkit::fold(&postcard::to_stdvec(value).expect("encode"))
}

fn entry() -> FileEntry {
    let device = DeviceId::from_bytes([2; 32]);
    FileEntry {
        size: 1_000,
        modified_unix: 1_700_000_000,
        content_hash: [3; 32],
        chunks: vec![ChunkId::from_bytes([4; 32])],
        version: VersionVector::new().advanced(device, 7),
        author: device,
    }
}

fn uptime() -> Uptime {
    let mut uptime = Uptime::new(500_000);
    uptime.note(500_001, true);
    uptime.note(500_002, false);
    uptime
}

/// One row per type and format; a new form is a new row beside the old.
const PINNED: &[(&str, u64)] = &[
    ("FileEntry", 14_026_065_316_767_880_960),
    ("Tombstone", 4_068_901_043_947_100_110),
    ("LogEntry", 4_242_739_424_256_418_366),
    ("SegmentEnvelope", 14_200_803_424_482_940_256),
    ("SegmentBody", 12_376_253_173_780_873_365),
    ("LocalState", 7_914_254_230_711_537_055),
    ("Reliability", 14_614_567_821_536_524_767),
    ("Uptime", 10_789_397_639_158_953_223),
    ("protected", 6_536_339_770_227_908_172),
];

#[test]
fn red_team_what_the_store_writes_to_disk_keeps_its_bytes() {
    // Sabotage: add a field to `LocalState`, or reorder two of `FileEntry`'s.
    let device = DeviceId::from_bytes([2; 32]);
    let version = VersionVector::new().advanced(device, 7);
    let log = LogEntry {
        sequence: 9,
        recorded_unix: 1_700_000_001,
        operation: Operation::Upsert {
            path: "a/b.txt".to_owned(),
            entry: entry(),
        },
    };
    let found = [
        ("FileEntry", folded(&entry())),
        (
            "Tombstone",
            folded(&Tombstone {
                version: version.clone(),
                removed_unix: 1_700_000_002,
                author: device,
            }),
        ),
        ("LogEntry", folded(&log)),
        (
            "SegmentEnvelope",
            folded(&SegmentEnvelope {
                segment_id: ObjectId::from_bytes([5; 32]),
                owner: UserId::from_bytes([6; 32]),
                device,
                first_sequence: 1,
                last_sequence: 9,
                previous: Some(ObjectId::from_bytes([7; 32])),
                sealed_body: vec![8; 16],
                signature: Signature::from_bytes([9; 64]),
            }),
        ),
        (
            "SegmentBody",
            folded(&SegmentBody {
                entries: vec![
                    log,
                    LogEntry {
                        sequence: 10,
                        recorded_unix: 1_700_000_003,
                        operation: Operation::Remove {
                            path: "a/c.txt".to_owned(),
                            version,
                        },
                    },
                ],
            }),
        ),
        (
            "LocalState",
            folded(&LocalState {
                size: 1_000,
                modified_unix: 1_700_000_000,
                content_hash: [3; 32],
            }),
        ),
        (
            "Reliability",
            folded(&Reliability {
                consecutive_failures: 1,
                passed: 20,
                failed: 2,
                last_failure_unix: 1_700_000_004,
                paused: false,
            }),
        ),
        ("Uptime", folded(&uptime())),
        (
            "protected",
            folded(&BTreeMap::from([(
                "a/b.txt".to_owned(),
                (1_700_100_000_u64, entry()),
            )])),
        ),
    ];
    itsanas_testkit::check_pinned(&found, PINNED);
}
