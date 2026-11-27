//! Reciprocal hosting: the side that dialled hosts too, within its pledge
//! and the bound of the account it hosts for.

use super::*;

#[test]
fn the_side_that_dialled_ends_up_hosting_too() {
    // The whole point of the reciprocal half, and the thing that decides
    // whether somebody behind a router they do not control can take part.
    //
    // Only ONE of these two can be dialled: `reachable` runs a server, and
    // `behind_nat` does not. That is not a simulation of NAT, it is the same
    // asymmetry NAT produces -- one side can accept connections and the other
    // can only make them.
    //
    // Before `host_for` existed, everything the dialling side could do was give
    // its own data away. It could be hosted and could host nobody, which in a
    // system built on members holding each other's data means it cannot keep
    // its half of the bargain at all.
    let reachable = node(&MasterSecret::from_bytes([0xA1; 32]), 40);
    let behind_nat = node(&MasterSecret::from_bytes([0xB2; 32]), 41);

    // The reachable node has data of its own and nowhere to put it.
    let payload = itsanas_testkit::filler("reciprocal", 400 * 1024);
    reachable.store.write_file("theirs.bin", &payload).unwrap();
    reachable.store.flush_segment().unwrap();

    let at_risk = reachable
        .store
        .under_replicated(itsanas_store::REPLICATION_TARGET, itsanas_store::now_unix())
        .unwrap();
    assert!(
        !at_risk.is_empty(),
        "the reachable node has nothing under-replicated, so this test would \
         pass without hosting anything"
    );

    let held_before = behind_nat.vault.stats().unwrap().bytes;
    assert_eq!(
        held_before, 0,
        "the dialling node started with a full vault"
    );

    with_server(&reachable, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &behind_nat.device, behind_nat.store.owner(), None)
                .unwrap();

        let report = session::host_for(&behind_nat.vault, &mut client, Pledge::gigabytes(1))
            .expect("asking a peer what it wants held must not fail");

        assert!(
            report.wanted > 0,
            "the peer asked for nothing to be held, though it has \
             under-replicated chunks"
        );
        assert!(
            report.taken > 0,
            "the dialling node took nothing, so hosting is still one-way"
        );
    });

    let held_after = behind_nat.vault.stats().unwrap().bytes;
    assert!(
        held_after > held_before,
        "the vault of the node that dialled did not grow: it is not hosting"
    );
}

#[test]
fn the_owner_learns_who_is_holding_after_a_reciprocal_round() {
    // Taking the chunks is half of it. An owner who does not know where its
    // copies went cannot audit them, cannot count them, and will keep asking
    // somebody else to hold what is already held.
    let reachable = node(&MasterSecret::from_bytes([0xC3; 32]), 42);
    let behind_nat = node(&MasterSecret::from_bytes([0xD4; 32]), 43);

    let payload = itsanas_testkit::filler("ledger", 300 * 1024);
    reachable.store.write_file("mine.bin", &payload).unwrap();
    reachable.store.flush_segment().unwrap();

    let before = reachable
        .store
        .under_replicated(itsanas_store::REPLICATION_TARGET, itsanas_store::now_unix())
        .unwrap();
    let worst_before = before.first().map_or(0, |risk| risk.held_by);

    with_server(&reachable, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &behind_nat.device, behind_nat.store.owner(), None)
                .unwrap();
        session::host_for(&behind_nat.vault, &mut client, Pledge::gigabytes(1)).unwrap();
    });

    let after = reachable
        .store
        .under_replicated(itsanas_store::REPLICATION_TARGET, itsanas_store::now_unix())
        .unwrap();
    let worst_after = after.first().map_or(0, |risk| risk.held_by);

    assert!(
        worst_after > worst_before,
        "the owner's ledger did not record the new holder: still {worst_after} \
         copies against {worst_before} before, so the next round would ask \
         somebody to hold what is already held"
    );
}

#[test]
fn a_pledge_of_nothing_takes_nothing_on() {
    // Hosting is opt-in, and it stays opt-in over the reciprocal path. A node
    // that offered the network no space must not have its disk filled by a
    // peer that asked nicely.
    let reachable = node(&MasterSecret::from_bytes([0xE5; 32]), 44);
    let stingy = node(&MasterSecret::from_bytes([0xF6; 32]), 45);

    let payload = itsanas_testkit::filler("no-room", 200 * 1024);
    reachable.store.write_file("theirs.bin", &payload).unwrap();
    reachable.store.flush_segment().unwrap();

    with_server(&reachable, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &stingy.device, stingy.store.owner(), None).unwrap();

        let report = session::host_for(&stingy.vault, &mut client, Pledge::NONE)
            .expect("a full node still answers");

        assert_eq!(report.taken, 0, "a node pledging nothing took data on");
        assert!(report.pledge_full, "it did not say why it took nothing");
    });

    assert_eq!(
        stingy.vault.stats().unwrap().bytes,
        0,
        "a node that pledged nothing is holding something"
    );
}

#[test]
fn red_team_a_vault_already_at_its_pledge_takes_nothing_more_on() {
    // `host_for` reads the vault's running total, not a walk. If that total
    // forgot what is already held, a full host would be handed a whole
    // pledge's worth of room again every round and fill past what it offered.
    let reachable = node(&MasterSecret::from_bytes([0xE7; 32]), 46);
    let full = node(&MasterSecret::from_bytes([0xF8; 32]), 47);

    let stranger = UserKeys::derive(&MasterSecret::from_bytes([0x5E; 32])).user_id();
    full.vault
        .put_chunk(
            stranger,
            &ChunkId::from_bytes([0x5E; 32]),
            &vec![0u8; 1 << 20],
        )
        .unwrap();
    let pledge = Pledge { bytes: 1 << 20 };
    let held_before = full.vault.stats().unwrap().bytes;

    let payload = itsanas_testkit::filler("already-full", 200 * 1024);
    reachable.store.write_file("theirs.bin", &payload).unwrap();
    reachable.store.flush_segment().unwrap();

    with_server(&reachable, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &full.device, full.store.owner(), None).unwrap();
        let report =
            session::host_for(&full.vault, &mut client, pledge).expect("a full node still answers");
        assert_eq!(report.taken, 0, "a host at its pledge took more data on");
        assert!(report.pledge_full, "it did not say it was full");
    });

    assert_eq!(
        full.vault.stats().unwrap().bytes,
        held_before,
        "a host at its pledge holds more than it did: it is past what it offered"
    );
}

/// A host's bound that refuses every account: what a peer with no claim, or
/// one past its share, meets.
struct RefuseAll;

impl itsanas_net::Owners for RefuseAll {
    fn may_read(&self, _: itsanas_crypto::DeviceId, _: itsanas_crypto::UserId) -> bool {
        true
    }

    fn present(
        &self,
        _caller: itsanas_crypto::DeviceId,
        _claim: &[u8],
        _vault: &Vault,
    ) -> Result<(), String> {
        Ok(())
    }
    fn admits(
        &self,
        _caller: itsanas_crypto::DeviceId,
        _owner: itsanas_crypto::UserId,
        _incoming: u64,
        _store: &Store,
        _vault: &Vault,
        _host_pledge: u64,
    ) -> Result<(), String> {
        Err(itsanas_net::UPDATE_REQUIRED.to_owned())
    }
}

#[test]
fn red_team_a_peer_refused_on_push_cannot_be_hosted_by_being_pulled() {
    // Found by the redteam agent on §8 1c: the per-account bound sat on
    // `StoreChunk` and `StoreSegment`, and the hosting pull -- the peer names
    // the owner and the chunks, this node fetches them into its vault -- was
    // checked against this node's own pledge only. A client refused on push
    // offered the same chunks here. Sabotage: skip the bound in
    // `host_for_bounded`.
    let reachable = node(&MasterSecret::from_bytes([0xA3; 32]), 42);
    let dialler = node(&MasterSecret::from_bytes([0xB3; 32]), 43);
    reachable
        .store
        .write_file(
            "theirs.bin",
            &itsanas_testkit::filler("bounded", 400 * 1024),
        )
        .unwrap();
    reachable.store.flush_segment().unwrap();

    with_server(&reachable, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &dialler.device, dialler.store.owner(), None).unwrap();
        let bound: (&dyn itsanas_net::Owners, &Store) = (&RefuseAll, &dialler.store);
        let report = session::host_for_bounded(
            &dialler.vault,
            &mut client,
            Pledge::gigabytes(1),
            Some(bound),
        )
        .unwrap();
        assert!(report.wanted > 0, "fixture: the peer wanted nothing held");
        assert_eq!(
            report.taken, 0,
            "a peer the bound refuses was hosted anyway, by being pulled"
        );
        assert!(report.bounded.is_some(), "the refusal was not reported");
    });
    assert_eq!(dialler.vault.stats().unwrap().bytes, 0);
}
