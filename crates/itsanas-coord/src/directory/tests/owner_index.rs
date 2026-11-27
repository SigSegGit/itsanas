//! The owner index: one account's devices as a range, repaired on open, and
//! what survives reopening.

use super::*;

/// THE LEAK THIS SHAPE INVITES: one account's devices are now a *range*
/// rather than a filtered scan, so the filter is the key layout. Get the
/// boundary wrong and a member's lookup returns the neighbouring account's
/// machines -- which is both a privacy failure and an address book that
/// tells people to dial strangers.
///
/// Tested on the property that makes the range safe, with adjacent account
/// ids constructed on purpose: every key of account *n* sorts below every
/// key of account *n+1*, whatever devices either of them holds.
#[test]
fn red_team_one_accounts_range_cannot_reach_into_the_next_accounts_devices() {
    let mut lower = [0x42u8; 32];
    lower[31] = 0xfe;
    let mut upper = lower;
    upper[31] = 0xff;

    let lower = UserId::from_bytes(lower);
    let upper = UserId::from_bytes(upper);

    let highest_of_lower = owner_device_key(lower, DeviceId::from_bytes([0xff; 32]));
    let lowest_of_upper = owner_device_key(upper, DeviceId::from_bytes([0x00; 32]));

    assert!(
        highest_of_lower < lowest_of_upper,
        "the last device of one account sorts at or above the first device \
         of the next, so a range scan spills between accounts"
    );
    assert_eq!(
        &highest_of_lower[..32],
        &lower.to_bytes(),
        "the account must be the leading part of the key, or the range is \
         not a range over one account at all"
    );
}

/// The two tables answer the same question and must never disagree, however
/// a claim got there: a first enrolment, a superseding claim, a withdrawal.
/// The one this catches is a write path that updates one and not the other,
/// which reads as a member's machines vanishing from the address book.
#[test]
fn the_index_and_the_claims_never_disagree_whatever_is_done_to_them() {
    let (_dir, directory) = directory();
    let alice = user(1);
    let bob = user(2);
    register(&directory, "alice", &alice);
    register(&directory, "bob", &bob);

    let laptop = DeviceKeys::from_seed(&SecretBytes::new([11; 32]));
    let pi = DeviceKeys::from_seed(&SecretBytes::new([12; 32]));
    let bobs = DeviceKeys::from_seed(&SecretBytes::new([13; 32]));

    let claim_it = |owner: &UserKeys, device: &DeviceKeys, pledged, revoked, at| {
        directory.claim(
            &NodeClaim {
                owner: owner.user_id(),
                device: device.device_id(),
                pledged_bytes: pledged,
                issued_unix: at,
                revoked,
            }
            .sign(owner),
            at,
        )
    };

    claim_it(&alice, &laptop, 1 << 30, false, NOW).expect("enrol the laptop");
    claim_it(&alice, &pi, 2 << 30, false, NOW).expect("enrol the pi");
    claim_it(&bob, &bobs, 1 << 30, false, NOW).expect("enrol bob's machine");

    // A superseding claim: same device, new pledge.
    claim_it(&alice, &pi, 4 << 30, false, NOW + 1).expect("re-pledge the pi");

    let mine: Vec<_> = directory
        .live_claims_of(alice.user_id())
        .expect("alice's devices")
        .into_iter()
        .map(|claim| claim.claim.device)
        .collect();
    assert_eq!(mine.len(), 2, "alice has two live machines, got {mine:?}");
    assert!(mine.contains(&laptop.device_id()) && mine.contains(&pi.device_id()));
    assert!(
        !mine.contains(&bobs.device_id()),
        "another account's machine appeared in this account's list"
    );

    // The whole-table walk and the indexed lookup must agree, always.
    let by_scan: Vec<_> = directory
        .live_claims()
        .expect("every claim")
        .into_iter()
        .filter(|claim| claim.claim.owner == alice.user_id())
        .map(|claim| claim.claim.device)
        .collect();
    assert_eq!(
        sorted(by_scan),
        sorted(mine),
        "the index and the table disagree about who owns what"
    );

    // A withdrawal, which keeps its row and must stop being live.
    claim_it(&alice, &laptop, 1 << 30, true, NOW + 2).expect("withdraw the laptop");
    let after: Vec<_> = directory
        .live_claims_of(alice.user_id())
        .expect("alice's devices")
        .into_iter()
        .map(|claim| claim.claim.device)
        .collect();
    assert_eq!(
        after,
        vec![pi.device_id()],
        "a withdrawn machine is still listed as live through the index"
    );
}

/// THE OPERATION THAT BREAKS IT, and it is one Nicolas has performed:
/// downgrading the coordinator binary. An older build enrols devices by
/// writing `CLAIMS` and knowing nothing of the index. Coming back up, a
/// repair conditioned on "the index is empty" would skip -- the index is not
/// empty, it is *stale* -- and every machine enrolled during the downgrade
/// would be permanently invisible: `claim_for` knows it, it can announce,
/// and `peers_of` never returns it. Silent, permanent, and it looks to the
/// member like their machine never joined.
#[test]
fn a_device_enrolled_by_an_older_binary_is_found_again_at_the_next_start() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("directory.redb");

    let alice = user(5);
    let first = DeviceKeys::from_seed(&SecretBytes::new([21; 32]));
    let during_downgrade = DeviceKeys::from_seed(&SecretBytes::new([22; 32]));

    let enrol = |directory: &Directory, device: &DeviceKeys, at: u64| {
        directory
            .claim(
                &NodeClaim {
                    owner: alice.user_id(),
                    device: device.device_id(),
                    pledged_bytes: 1 << 30,
                    issued_unix: at,
                    revoked: false,
                }
                .sign(&alice),
                at,
            )
            .expect("enrol");
    };

    {
        let directory = Directory::open(&path).expect("directory");
        register(&directory, "alice", &alice);
        enrol(&directory, &first, NOW);

        // What the older binary does: a claim written to `CLAIMS` alone.
        // The index keeps the row it already had, so it is stale rather
        // than empty -- which is the whole point of this test.
        let txn = directory.db.begin_write().expect("write");
        {
            let mut claims = txn.open_table(CLAIMS).expect("claims");
            let signed = NodeClaim {
                owner: alice.user_id(),
                device: during_downgrade.device_id(),
                pledged_bytes: 1 << 30,
                issued_unix: NOW + 1,
                revoked: false,
            }
            .sign(&alice);
            claims
                .insert(
                    during_downgrade.device_id().to_bytes().as_slice(),
                    postcard::to_stdvec(&signed).expect("encode").as_slice(),
                )
                .expect("insert");
        }
        txn.commit().expect("commit");

        let found = directory.live_claims_of(alice.user_id()).expect("devices");
        assert_eq!(
            found.len(),
            1,
            "the fixture is wrong: the older binary's claim was already indexed"
        );
    }

    let directory = Directory::open(&path).expect("reopen");
    let found: Vec<_> = directory
        .live_claims_of(alice.user_id())
        .expect("devices")
        .into_iter()
        .map(|claim| claim.claim.device)
        .collect();

    assert_eq!(
        found.len(),
        2,
        "a machine enrolled while the coordinator ran an older binary stayed \
         invisible after the upgrade; it announces and nobody is ever told"
    );
    assert!(found.contains(&during_downgrade.device_id()));
}

/// THE UPGRADE: a coordinator that has been running since before this table
/// existed holds claims and no index. Reading that as "this account has no
/// devices" would be silent and total -- every member told their machines
/// are gone, the address book answering nothing, and the only clue being
/// that it started at an upgrade.
#[test]
fn a_directory_written_before_the_index_existed_is_repaired_on_open() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("directory.redb");

    let alice = user(3);
    let laptop = DeviceKeys::from_seed(&SecretBytes::new([14; 32]));
    {
        let directory = Directory::open(&path).expect("directory");
        register(&directory, "alice", &alice);
        directory
            .claim(
                &NodeClaim {
                    owner: alice.user_id(),
                    device: laptop.device_id(),
                    pledged_bytes: 1 << 30,
                    issued_unix: NOW,
                    revoked: false,
                }
                .sign(&alice),
                NOW,
            )
            .expect("enrol");

        // Make it look like a file written by the older code: the claims
        // are there and the index is not.
        let txn = directory.db.begin_write().expect("write");
        {
            let mut index = txn.open_table(CLAIMS_BY_OWNER).expect("index");
            index.retain(|_, ()| false).expect("empty the index");
        }
        txn.commit().expect("commit");
        assert!(
            directory
                .live_claims_of(alice.user_id())
                .unwrap()
                .is_empty(),
            "the fixture is wrong: the index was not emptied"
        );
    }

    let directory = Directory::open(&path).expect("reopen");
    let found = directory
        .live_claims_of(alice.user_id())
        .expect("alice's devices");
    assert_eq!(
        found.len(),
        1,
        "an upgrade lost every machine of every account"
    );
    assert_eq!(found[0].claim.device, laptop.device_id());
}

fn sorted(mut ids: Vec<DeviceId>) -> Vec<DeviceId> {
    ids.sort_unstable();
    ids
}

#[test]
fn everything_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("coord.redb");
    let owner = user(18);

    {
        let directory = Directory::open(&path).unwrap();
        register(&directory, "nicolas", &owner);
        enrol(&directory, &owner, &device(18), 4096);
    }

    let directory = Directory::open(&path).unwrap();
    assert!(directory.account("nicolas").unwrap().is_some());
    assert_eq!(directory.live_claims().unwrap().len(), 1);
}
