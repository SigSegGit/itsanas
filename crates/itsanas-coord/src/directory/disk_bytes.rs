//! The bytes of what the coordinator writes to its database, pinned (HANDOVER
//! §8 17). #435 changed a presence and the coordinator, upgraded in place,
//! read its own rows as garbage (handled in #443). A row whose figure moves is
//! a format change: read the old rows too, or skip them as absent
//! (`Directory::presence_of`), then add a row here. Never edit one that
//! shipped.

use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};
use serde::Serialize;

use super::{Account, AvailabilityRecord, DepartureRecord, LodgedInvitation, UsageRecord};
use crate::claim::{ClaimedPresence, Departure, NodeClaim, Presence};

fn folded<T: Serialize>(value: &T) -> u64 {
    itsanas_testkit::fold(&postcard::to_stdvec(value).expect("encode"))
}

/// One row per table value and format.
const PINNED: &[(&str, u64)] = &[
    ("Account", 10_134_497_676_643_606_378),
    ("SignedClaim", 3_825_189_355_919_324_400),
    ("SignedPresence", 15_344_322_816_238_024_710),
    ("ClaimedPresence", 14_088_321_274_259_623_295),
    ("LodgedInvitation", 3_468_511_414_918_896_217),
    ("DepartureRecord", 17_748_005_916_057_684_242),
    ("AvailabilityRecord", 16_448_725_866_307_916_807),
    ("UsageRecord", 18_053_223_756_777_079_223),
];

#[test]
fn red_team_what_the_coordinator_writes_to_disk_keeps_its_bytes() {
    // Sabotage: reorder two fields of `UsageRecord`.
    let owner = UserKeys::derive(&MasterSecret::from_bytes([1; 32]));
    let keys = DeviceKeys::from_seed(&SecretBytes::new([2; 32]));
    let user = owner.user_id();
    let claim = NodeClaim {
        owner: user,
        device: keys.device_id(),
        pledged_bytes: 3 << 30,
        issued_unix: 1_700_000_000,
        revoked: false,
    }
    .sign(&owner);
    let presence = Presence {
        device: keys.device_id(),
        address: "a:1".to_owned(),
        at_unix: 1_700_000_001,
        head: 4,
    }
    .sign(&keys);
    let invitation = crate::invitation::Invitation {
        inviter: user,
        code: [5; 32],
        issued_unix: 1_700_000_002,
        expires_unix: 1_700_100_000,
        uses: 3,
    }
    .sign(&owner);
    let found = [
        (
            "Account",
            folded(&Account {
                username: "a".to_owned(),
                user: owner.public(),
                registered_unix: 1_700_000_003,
                escrow_enabled: true,
            }),
        ),
        ("SignedClaim", folded(&claim)),
        ("SignedPresence", folded(&presence)),
        (
            "ClaimedPresence",
            folded(&ClaimedPresence {
                presence: presence.clone(),
                claim,
            }),
        ),
        (
            "LodgedInvitation",
            folded(&LodgedInvitation {
                signed: invitation,
                remaining: 2,
                admitted: vec![user],
            }),
        ),
        (
            "DepartureRecord",
            folded(&DepartureRecord {
                notice: Departure {
                    device: keys.device_id(),
                    at_unix: 1_700_000_004,
                }
                .sign(&keys),
                received_unix: 1_700_000_005,
            }),
        ),
        (
            "AvailabilityRecord",
            folded(&AvailabilityRecord {
                per_mille: 873,
                last_tick_unix: 1_700_000_006,
                last_seen_unix: 1_700_000_007,
            }),
        ),
        (
            "UsageRecord",
            folded(&UsageRecord {
                bytes: 1 << 30,
                over_since_unix: Some(1_700_000_008),
                reported_unix: 1_700_000_009,
            }),
        ),
    ];
    itsanas_testkit::check_pinned(&found, PINNED);
}
